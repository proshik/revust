use ratatui::{buffer::Buffer, layout::Rect};
use revust_core::{
    prompts::{Agent, runner::Runner},
    report::Report,
    runtime::events::{Event, Kind},
};
use revust_tui::{Model, status::Status};

fn runner(executor: &str, model: &str, effort: &str) -> Runner {
    Runner {
        executor: executor.into(),
        model: model.into(),
        effort: effort.into(),
    }
}
fn started(name: &str) -> Event {
    let mut event = Event::agent(Kind::AgentStarted, name, "started");
    event.at = "2026-10-11T12:00:00Z".into();
    event
}
fn rows(model: &mut Model, width: u16) -> Vec<String> {
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, 20));
    Status::default().frame(model, &mut buffer, |_, _| vec![]);
    (0..20)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}
fn roster() -> Vec<Agent> {
    vec![Agent {
        name: "finder".into(),
        runner: runner("claude", "sonnet", "low"),
        lenses: vec![],
        color: String::new(),
        color_name: String::new(),
    }]
}

#[test]
fn archived_table_shows_saved_runners_for_finder_synthesis_and_verify_groups() {
    let report: Report = serde_json::from_value(serde_json::json!({
        "stats":{"stages":[
            {"name":"synthesis","executor":"codex","model":"gpt-5","effort":"high"},
            {"name":"verify","executor":"opencode","model":"fixture/check"}
        ]}
    }))
    .unwrap();
    let mut model = Model::from_archive(
        roster(),
        [
            started("finder"),
            started("synthesis"),
            started("verify root"),
            started("verify src"),
        ],
        report,
    )
    .unwrap();
    let screen = rows(&mut model, 130);
    for label in ["EXECUTOR", "MODEL", "EFFORT"] {
        assert!(screen[2].contains(label), "{}", screen[2]);
    }
    for (row, executor, model, effort) in [
        (3, "claude", "sonnet", "low"),
        (4, "codex", "gpt-5", "high"),
        (5, "opencode", "fixture/check", "-"),
        (6, "opencode", "fixture/check", "-"),
    ] {
        for value in [executor, model, effort] {
            assert!(
                screen[row].split_whitespace().any(|s| s == value),
                "{}",
                screen[row]
            );
        }
    }
    let mut legacy =
        Model::from_archive(vec![], [started("synthesis")], Report::default()).unwrap();
    let screen = rows(&mut legacy, 130);
    assert_eq!(
        screen[3].split_whitespace().filter(|s| *s == "-").count(),
        4
    );
}

#[test]
fn live_table_uses_frozen_stage_settings_before_completion_and_after_resize() {
    let mut model = Model::new(roster()).with_stage_runners(
        runner("codex", "gpt-5", "high"),
        runner("opencode", "fixture/check", ""),
    );
    model.apply(started("synthesis")).unwrap();
    model.apply(started("verify root")).unwrap();
    let wide = rows(&mut model, 130);
    assert!(wide[4].contains("codex") && wide[4].contains("gpt-5") && wide[4].contains("high"));
    assert!(wide[5].contains("opencode") && wide[5].contains("fixture/check"));
    assert!(model.report.is_none());
    let narrow = rows(&mut model, 65);
    assert!(narrow[4].contains("running") && narrow[4].contains("codex"));
    assert!(narrow[5].contains("opencode"));
    assert_eq!(rows(&mut model, 130), wide);
}

#[test]
fn failed_retried_and_cancelled_units_are_terminal_live_and_after_replay() {
    use revust_core::{
        archive::{Archive, reader},
        config::{
            extensions::EffectivePolicy,
            runtime::{Invocation, RuntimeConfig},
        },
        runtime::{
            Limits, MonotonicClock, RunControl, Stage,
            data::Categories,
            journal::Identity,
            pipeline::{Backend, Call, Definition},
            retry::Failure,
            worker::{AttemptFailure, Event as WorkerEvent},
        },
    };
    use serde_json::{Value, json};
    use std::{fs, sync::Arc, time::Duration};
    struct Failing {
        stage: Stage,
        cancel: bool,
    }
    impl Backend for Failing {
        fn execute(&self, call: Call<'_>, control: &RunControl) -> Result<Value, AttemptFailure> {
            if call.attempt.stage == self.stage {
                if self.cancel {
                    control.cancel();
                }
                return Err(AttemptFailure {
                    kind: Failure::NonzeroExit,
                    diagnostic: "synthetic failure".into(),
                });
            }
            Ok(match call.attempt.stage {
                Stage::Find => {
                    json!({"findings":[{"title":"candidate","file":"src/a.rs","severity":"major"}]})
                }
                Stage::Synthesis => {
                    json!({"findings":[{"title":"candidate","file":"src/a.rs","severity":"major","merged_ids":["finder-1"]}]})
                }
                Stage::Verify => json!({"verdicts":[{"id":"finder-1","verdict":"confirmed"}]}),
            })
        }
        fn record(&self, _: WorkerEvent<'_, Categories>) -> Result<(), String> {
            Ok(())
        }
        fn wait(&self, _: Duration) {}
        fn jitter(&self, _: Duration) -> Duration {
            Duration::ZERO
        }
    }
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    for (stage, name) in [
        (Stage::Find, "finder"),
        (Stage::Synthesis, "synthesis"),
        (Stage::Verify, "verify src"),
    ] {
        for mode in ["failed", "retried", "cancelled", "passthrough"] {
            if mode == "passthrough" && stage != Stage::Synthesis {
                continue;
            }
            let temp = Temp(std::env::temp_dir().join(format!(
                "revust-terminal-unit-{}-{stage:?}-{mode}",
                std::process::id()
            )));
            let round = temp.0.join("tasks/test/01");
            fs::create_dir_all(round.join("input")).unwrap();
            let config = RuntimeConfig::resolve(
                &temp.0,
                None,
                &Invocation {
                    config_dir: Some(temp.0.join("user")),
                    ..Default::default()
                },
            )
            .unwrap();
            let mut policy = json!({"runtime":{"stagger_delay":"0s"},"retries":{}});
            for key in ["find", "synthesis", "verify"] {
                policy["retries"][key] = json!({"count":if mode == "retried" {1} else {0},"delay":{"base":"0s","jitter":"0s"}});
            }
            if mode == "passthrough" {
                policy["completeness"] = json!({"synthesis_failure":"passthrough"});
            }
            let definition = Definition {
                agents: roster(),
                synthesis: runner("claude", "fixture", ""),
                verify: runner("claude", "fixture", ""),
                policy: EffectivePolicy::resolve(&config, &[], "comprehensive", &policy).unwrap(),
            };
            let mut archive = Archive::claim(&temp.0.join("tasks"), "test", "01").unwrap();
            let journal = archive
                .journal(Identity {
                    task: "test".into(),
                    run: "01".into(),
                    request: "synthetic".into(),
                })
                .unwrap();
            let observer = journal.subscribe().unwrap();
            let control = RunControl::new(
                Limits::from_policy(&definition.policy).unwrap(),
                Arc::new(MonotonicClock::default()),
            )
            .unwrap();
            let result = journal
                .run(
                    &definition,
                    &control,
                    &Failing {
                        stage,
                        cancel: mode == "cancelled",
                    },
                )
                .unwrap();
            let events: Vec<_> = observer.try_iter().collect();
            let unit_events: Vec<_> = events.iter().filter(|e| e.agent == name).collect();
            assert!(matches!(
                unit_events.first().unwrap().kind,
                Kind::AgentStarted
            ));
            assert!(
                matches!(unit_events.last().unwrap().kind, Kind::AgentDegraded),
                "{stage:?} {mode}"
            );
            assert_eq!(
                unit_events
                    .iter()
                    .filter(|e| matches!(e.kind, Kind::AgentRetried))
                    .count(),
                usize::from(mode == "retried")
            );
            if mode == "passthrough" {
                assert!(result.policy_satisfied());
                assert_eq!(result.categories.findings.len(), 1);
            }
            let mut live = Model::new(roster());
            for event in events {
                live.apply(event).unwrap();
            }
            let report = Report {
                outcome: Some(revust_core::report::ExecutionOutcome::from_runtime(&result)),
                scope: revust_core::report::Scope {
                    task: "test".into(),
                    run: "01".into(),
                    ..Default::default()
                },
                ..Default::default()
            };
            live.complete(report.clone());
            assert_eq!(
                live.agents.iter().find(|a| a.name == name).unwrap().state,
                revust_tui::State::Degraded
            );
            archive.save_report(&report).unwrap();
            archive.finish(json!({"task":"test","run":"01","finished_at":"2026-11-04T10:00:00Z","outcome":report.outcome}).to_string().as_bytes()).unwrap();
            drop(archive);
            let saved = reader::load(&round).unwrap();
            let mut replay =
                Model::from_saved_events(roster(), saved.events, saved.report).unwrap();
            assert_eq!(
                replay.agents.iter().find(|a| a.name == name).unwrap().state,
                revust_tui::State::Degraded
            );
            assert!(
                rows(&mut replay, 130)
                    .iter()
                    .any(|row| row.contains(name) && row.contains("degraded"))
            );
        }
    }
}
