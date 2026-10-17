#![cfg(unix)]
use revust_core::{request::Request, runtime::StopReason};
use revust_executors::supervision::{Limits, Stream, Termination, run};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
fn request(script: &str, input: String) -> Request {
    Request {
        program: "/bin/sh".into(),
        argv: vec!["-c".into(), script.into()],
        stdin: input,
        cwd: std::env::temp_dir(),
    }
}
fn limits() -> Limits {
    Limits {
        idle: Some(Duration::from_secs(1)),
        hard: Some(Duration::from_secs(3)),
    }
}

#[test]
fn pipes_make_progress_in_both_directions_and_keep_diagnostics_separate() {
    let request = request(
        "printf diagnostic >&2; /bin/cat; printf \"$REVUST_SYNTHETIC_CHILD\"",
        "x".repeat(2 * 1024 * 1024),
    );
    let before = std::env::var_os("REVUST_SYNTHETIC_CHILD");
    let env = BTreeMap::from([("REVUST_SYNTHETIC_CHILD".into(), "child-only".into())]);
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let result = run(
        &request,
        &env,
        limits(),
        || None,
        |stream, bytes| {
            if stream == Stream::Stdout {
                stdout.extend_from_slice(bytes);
            } else {
                stderr.extend_from_slice(bytes);
            }
            Ok(true)
        },
    )
    .unwrap();
    assert_eq!(result.termination, Termination::Exited);
    assert_eq!(result.status.unwrap().code(), Some(0));
    assert_eq!(stdout, (request.stdin + "child-only").as_bytes());
    assert_eq!(stderr, b"diagnostic");
    assert_eq!(std::env::var_os("REVUST_SYNTHETIC_CHILD"), before);
}

#[test]
fn idle_watchdog_ignores_non_activity_and_hard_watchdog_bounds_active_output() {
    for meaningful in [false, true] {
        let started = Instant::now();
        let result = run(
            &request(
                "while :; do printf tick; /bin/sleep 0.02; done",
                String::new(),
            ),
            &BTreeMap::new(),
            Limits {
                idle: Some(Duration::from_millis(150)),
                hard: Some(Duration::from_millis(500)),
            },
            || None,
            |_, _| Ok(meaningful),
        )
        .unwrap();
        assert_eq!(
            result.termination,
            if meaningful {
                Termination::HardTimeout
            } else {
                Termination::IdleTimeout
            }
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}

#[test]
fn cancellation_and_deadline_stop_children_even_when_stdin_is_blocked() {
    for reason in [StopReason::Cancelled, StopReason::Deadline] {
        let observed = AtomicBool::new(false);
        let started = Instant::now();
        let result = run(
            &request("printf ready; /bin/sleep 30", "x".repeat(2 * 1024 * 1024)),
            &BTreeMap::new(),
            limits(),
            || observed.load(Ordering::SeqCst).then_some(reason),
            |_, bytes| {
                if !bytes.is_empty() {
                    observed.store(true, Ordering::SeqCst);
                }
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(result.termination, Termination::RunStopped(reason));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    // Cancellation before dispatch must not even attempt an unavailable program.
    let mut unavailable = request("", String::new());
    unavailable.program = "/nonexistent/revust-controlled-provider".into();
    let result = run(
        &unavailable,
        &BTreeMap::new(),
        limits(),
        || Some(StopReason::Cancelled),
        |_, _| Ok(true),
    )
    .unwrap();
    assert_eq!(result.pid, None);
}

#[test]
fn isolated_environment_contains_only_explicit_variables() {
    let mut stdout = vec![];
    let result = revust_executors::supervision::run_isolated(
        &request("/usr/bin/env", String::new()),
        &BTreeMap::from([("REVUST_ONLY".into(), "synthetic".into())]),
        limits(),
        || None,
        |stream, bytes| {
            if stream == Stream::Stdout {
                stdout.extend_from_slice(bytes);
            }
            Ok(true)
        },
    )
    .unwrap();
    assert!(result.status.unwrap().success());
    let output = String::from_utf8(stdout).unwrap();
    assert!(output.contains("REVUST_ONLY=synthetic\n"));
    // /bin/sh may add PWD, SHLVL and _. It must not receive the harness environment.
    assert!(
        output
            .lines()
            .all(|line| ["REVUST_ONLY=", "PWD=", "SHLVL=", "_="]
                .iter()
                .any(|prefix| line.starts_with(prefix))),
        "unexpected inherited variable"
    );
}
