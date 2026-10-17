#![cfg(unix)]
use revust_core::{
    config::{
        extensions::EffectivePolicy,
        runtime::{Invocation, RuntimeConfig},
    },
    request::Request,
    runtime::{
        Attempt, Limits as RunLimits, MonotonicClock, RunControl, Stage, StopReason,
        retry::{Failure, RetryPolicy},
        worker::{AttemptFailure, Outcome, run_unit},
    },
};
use revust_executors::supervision::{self, Limits, Stream};
use std::{
    collections::BTreeMap,
    fs,
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
fn real_child_failure_then_backoff_obeys_the_original_run_deadline() {
    let root = std::env::temp_dir().join(format!("revust-worker-process-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    let _cleanup = Cleanup(root.clone());
    let runtime = RuntimeConfig::resolve(
        &root,
        None,
        &Invocation {
            config_dir: Some(root.join("user")),
            ..Invocation::default()
        },
    )
    .unwrap();
    let policy =
        EffectivePolicy::resolve(&runtime, &[], "comprehensive", &serde_json::json!({})).unwrap();
    let retry = RetryPolicy::from_policy(&policy, Stage::Find).unwrap();
    let run = RunControl::new(
        RunLimits {
            max_calls: Some(3),
            elapsed: Some(Duration::from_millis(150)),
            max_parallel: 1,
            stage_parallel: [1; 3],
        },
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    let started = Instant::now();
    let result = run_unit(
        &run,
        Attempt {
            stage: Stage::Find,
            unit: "synthetic".into(),
            number: 0,
        },
        &retry,
        std::thread::sleep,
        |_| Duration::ZERO,
        |_, run| {
            let mut stderr = Vec::new();
            let output = supervision::run(
                &Request {
                    program: "/bin/sh".into(),
                    argv: vec!["-c".into(), "printf synthetic-failure >&2; exit 7".into()],
                    stdin: String::new(),
                    cwd: root.clone(),
                },
                &BTreeMap::new(),
                Limits {
                    idle: None,
                    hard: Some(Duration::from_secs(2)),
                },
                || run.check_running(),
                |stream, bytes| {
                    if stream == Stream::Stderr {
                        stderr.extend_from_slice(bytes);
                    }
                    Ok(true)
                },
            )
            .unwrap();
            assert_eq!(output.status.unwrap().code(), Some(7));
            Err::<(), _>(AttemptFailure {
                kind: Failure::NonzeroExit,
                diagnostic: String::from_utf8(stderr).unwrap(),
            })
        },
        |_| Ok(()),
    );
    assert_eq!(result.outcome, Outcome::Stopped(StopReason::Deadline));
    assert_eq!(run.calls_spent(), 1);
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(
        result.attempts[0].result.as_ref().unwrap_err().diagnostic,
        "synthetic-failure"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}
