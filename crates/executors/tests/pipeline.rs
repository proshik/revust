#![cfg(unix)]
use revust_core::{
    config::{
        extensions::EffectivePolicy,
        runtime::{Invocation, RuntimeConfig},
    },
    prompts::{Agent, runner::Runner},
    request::Request,
    runtime::{
        self, Limits, MonotonicClock, RunControl, Stage, StopReason,
        data::Categories,
        journal::{self, Identity, Journal},
        pipeline::{Backend, Call, Definition, State},
        retry::Failure,
        worker::{AttemptFailure, Event},
    },
};
use revust_executors::{configured::Settings, supervision::Stream};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
struct Controlled<'a> {
    root: PathBuf,
    journal: &'a Journal,
    settings: [Settings; 3],
}
impl Backend for Controlled<'_> {
    fn execute(&self, call: Call<'_>, run: &RunControl) -> Result<Value, AttemptFailure> {
        let fail = call.attempt.stage == Stage::Find
            && call.attempt.unit == "a"
            && call.attempt.number == 0;
        let value = match call.attempt.stage {
            Stage::Find => json!({"findings":[{"file":"src/a.rs","title":"synthetic"}]}),
            Stage::Synthesis => {
                json!({"findings":[{"file":"src/a.rs","merged_ids":call.findings.iter().map(|f|&f.id).collect::<Vec<_>>()}]})
            }
            Stage::Verify => {
                json!({"verdicts":call.findings.iter().map(|f|json!({"id":f.id,"verdict":"confirmed"})).collect::<Vec<_>>()})
            }
        };
        let request = if fail {
            Request {
                program: "/bin/sh".into(),
                argv: vec![
                    "-c".into(),
                    "printf first-attempt-diagnostic >&2; exit 7".into(),
                ],
                stdin: String::new(),
                cwd: self.root.clone(),
            }
        } else {
            Request {
                program: "/bin/sh".into(),
                argv: vec!["-c".into(), "/bin/cat".into()],
                stdin: value.to_string(),
                cwd: self.root.clone(),
            }
        };
        let mut stdout = vec![];
        let mut stderr = vec![];
        let settings = &self.settings[match call.attempt.stage {
            Stage::Find => 0,
            Stage::Synthesis => 1,
            Stage::Verify => 2,
        }];
        let output = settings
            .run(request, run, |stream, bytes| {
                self.journal
                    .append(
                        call.attempt,
                        match stream {
                            Stream::Stdout => journal::Stream::Stdout,
                            Stream::Stderr => journal::Stream::Stderr,
                        },
                        bytes,
                    )
                    .map_err(std::io::Error::other)?;
                if stream == Stream::Stdout {
                    stdout.extend_from_slice(bytes);
                    (call.activity)();
                } else {
                    stderr.extend_from_slice(bytes);
                }
                Ok(stream == Stream::Stdout)
            })
            .map_err(|e| AttemptFailure {
                kind: Failure::ProcessError,
                diagnostic: e.to_string(),
            })?;
        if !output.status.is_some_and(|s| s.success()) {
            return Err(AttemptFailure {
                kind: Failure::NonzeroExit,
                diagnostic: String::from_utf8_lossy(&stderr).into(),
            });
        }
        serde_json::from_slice(&stdout).map_err(|e| AttemptFailure {
            kind: Failure::InvalidOutput,
            diagnostic: e.to_string(),
        })
    }
    fn record(&self, _: Event<'_, Categories>) -> Result<(), String> {
        Ok(())
    }
    fn wait(&self, d: Duration) {
        std::thread::sleep(d)
    }
    fn jitter(&self, _: Duration) -> Duration {
        Duration::ZERO
    }
}
#[test]
fn real_controlled_pipeline_archives_retries_and_reopens_without_dispatch() {
    for calls in [4, 5] {
        let root = std::env::temp_dir().join(format!(
            "revust-full-pipeline-{}-{calls}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
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
        let policy=EffectivePolicy::resolve(&runtime,&[],"comprehensive",&json!({"budget":{"max_calls":calls},"runtime":{"idle_timeout":"1s","hard_timeout":"2s"},"executors":{"codex":{"executable":"/bin/sh"}},"retries":{"find":{"delay":{"base":"0s","jitter":"0s"}}}})).unwrap();
        let control = RunControl::new(
            Limits::from_policy(&policy).unwrap(),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
        let settings = [Stage::Find, Stage::Synthesis, Stage::Verify]
            .map(|stage| Settings::resolve(&policy, stage, "codex").unwrap());
        let definition = Definition {
            agents: ["a", "b"]
                .map(|name| Agent {
                    name: name.into(),
                    lenses: vec!["bugs".into()],
                    runner: Runner {
                        executor: "codex".into(),
                        ..Runner::default()
                    },
                    color: String::new(),
                    color_name: String::new(),
                })
                .into(),
            synthesis: Runner {
                executor: "codex".into(),
                ..Runner::default()
            },
            verify: Runner {
                executor: "codex".into(),
                ..Runner::default()
            },
            policy,
        };
        let j = Journal::create(
            &root,
            Identity {
                task: "synthetic".into(),
                run: "1".into(),
                request: "synthetic-request".into(),
            },
        )
        .unwrap();
        let backend = Controlled {
            root: root.clone(),
            journal: &j,
            settings,
        };
        let report = j.run(&definition, &control, &backend).unwrap();
        assert_eq!(control.calls_spent(), calls);
        assert_eq!(
            report.state,
            if calls == 5 {
                State::Complete
            } else {
                State::Stopped(StopReason::CallBudget)
            }
        );
        assert!(j.run(&definition, &control, &backend).is_err());
        let before = std::fs::read(root.join("revust/events.jsonl")).unwrap();
        let snapshot = journal::inspect(&root).unwrap();
        assert_eq!(snapshot.report.unwrap().state, report.state);
        assert_eq!(snapshot.attempts.len() as u64, calls);
        let failed = snapshot
            .attempts
            .iter()
            .find(|a| {
                a.id == runtime::Attempt {
                    stage: Stage::Find,
                    unit: "a".into(),
                    number: 0,
                }
            })
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(failed.directory.join("stderr")).unwrap(),
            "first-attempt-diagnostic"
        );
        assert!(failed.result.as_ref().unwrap().is_err());
        assert_eq!(
            std::fs::read(root.join("revust/events.jsonl")).unwrap(),
            before
        );
        assert_eq!(control.calls_spent(), calls);
    }
}
