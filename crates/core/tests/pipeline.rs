use revust_core::{
    config::{
        extensions::EffectivePolicy,
        runtime::{Invocation, RuntimeConfig},
    },
    prompts::{Agent, runner::Runner},
    runtime::{
        Attempt, Limits, MonotonicClock, RunControl, Stage, StopReason,
        data::Categories,
        pipeline::{self, Backend, Call, Definition, State},
        retry::Failure,
        worker::{AttemptFailure, Event},
    },
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
fn definition(overrides: Value, agents: &[&str]) -> Definition {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "revust-pipeline-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&root).unwrap();
    let runtime = RuntimeConfig::resolve(
        &root,
        None,
        &Invocation {
            config_dir: Some(root.join("user")),
            ..Invocation::default()
        },
    )
    .unwrap();
    let policy = EffectivePolicy::resolve(&runtime, &[], "comprehensive", &overrides).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    Definition {
        agents: agents
            .iter()
            .map(|name| Agent {
                name: (*name).into(),
                lenses: vec!["correctness".into()],
                runner: Runner {
                    executor: "codex".into(),
                    ..Runner::default()
                },
                color: String::new(),
                color_name: String::new(),
            })
            .collect(),
        synthesis: Runner {
            executor: "claude".into(),
            ..Runner::default()
        },
        verify: Runner {
            executor: "opencode".into(),
            model: "test/model".into(),
            ..Runner::default()
        },
        policy,
    }
}
struct Controlled<F> {
    published: Mutex<Vec<revust_core::runtime::events::Event>>,
    execute: F,
    activity: bool,
    calls: Mutex<Vec<Attempt>>,
    records: Mutex<Vec<(Attempt, bool)>>,
}
impl<F> Backend for Controlled<F>
where
    F: Fn(Call<'_>) -> Result<Value, AttemptFailure> + Sync,
{
    fn publish(&self, event: revust_core::runtime::events::Event) -> Result<(), String> {
        self.published.lock().unwrap().push(event);
        Ok(())
    }
    fn execute(&self, call: Call<'_>, _: &RunControl) -> Result<Value, AttemptFailure> {
        self.calls.lock().unwrap().push(call.attempt.clone());
        if self.activity {
            (call.activity)();
        }
        (self.execute)(call)
    }
    fn record(&self, event: Event<'_, Categories>) -> Result<(), String> {
        self.records.lock().unwrap().push(match event {
            Event::Retrying(_) => return Ok(()),
            Event::Started(id) => (id.clone(), false),
            Event::Finished(done) => (done.id.clone(), true),
        });
        Ok(())
    }
    fn wait(&self, d: Duration) {
        std::thread::sleep(d)
    }
    fn jitter(&self, _: Duration) -> Duration {
        Duration::ZERO
    }
}
fn backend<F: Fn(Call<'_>) -> Result<Value, AttemptFailure> + Sync>(execute: F) -> Controlled<F> {
    Controlled {
        published: Mutex::new(vec![]),
        execute,
        activity: true,
        calls: Mutex::new(vec![]),
        records: Mutex::new(vec![]),
    }
}
fn control(d: &Definition) -> RunControl {
    RunControl::new(
        Limits::from_policy(&d.policy).unwrap(),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap()
}
fn answer(call: Call<'_>) -> Result<Value, AttemptFailure> {
    Ok(match call.attempt.stage {
        Stage::Find => json!({"findings":[{"file":"src/a.rs","title":"test","confidence":90}]}),
        Stage::Synthesis => {
            json!({"findings":[{"merged_ids":call.findings.iter().map(|f| &f.id).collect::<Vec<_>>(),"file":"src/a.rs","title":"merged"}]})
        }
        Stage::Verify => {
            json!({"verdicts":call.findings.iter().map(|f|json!({"id":f.id,"verdict":"confirmed"})).collect::<Vec<_>>()})
        }
    })
}
#[test]
fn ordered_stages_use_the_same_budget_and_keep_attempts_and_attribution() {
    let d = definition(json!({}), &["a", "b"]);
    let c = control(&d);
    let b = backend(answer);
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(r.state, State::Complete);
    assert!(r.policy_satisfied());
    assert_eq!(c.calls_spent(), 4);
    assert_eq!(r.successful_reviewers, ["a", "b"]);
    assert_eq!(r.categories.findings[0].sources, ["a", "b"]);
    assert_eq!(r.categories.findings[0].verdict, "confirmed");
    let calls = b.calls.lock().unwrap();
    assert!(calls[..2].iter().all(|a| a.stage == Stage::Find));
    assert_eq!(calls[2].stage, Stage::Synthesis);
    assert_eq!(calls[3].stage, Stage::Verify);
    assert_eq!(b.records.lock().unwrap().len(), 8);
    assert_eq!(r.units.len(), 4);
}
#[test]
fn budget_exhaustion_retains_found_evidence_and_prevents_later_stages() {
    let d = definition(json!({"budget":{"max_calls":1}}), &["a"]);
    let c = control(&d);
    let b = backend(answer);
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(r.state, State::Stopped(StopReason::CallBudget));
    assert!(!r.policy_satisfied());
    assert_eq!(r.categories.findings[0].id, "a-1");
    assert_eq!(b.calls.lock().unwrap().len(), 1);
}
#[test]
fn required_failure_is_separate_from_legacy_partial_success() {
    for strict in [false, true] {
        let d = definition(
            json!({"completeness":{"required_reviewers":if strict{vec!["bad"]}else{vec![]}},"retries":{"find":{"count":0}}}),
            &["good", "bad"],
        );
        let c = control(&d);
        let b = backend(|call| {
            if call.attempt.unit == "bad" {
                Err(AttemptFailure {
                    kind: Failure::MissingOutput,
                    diagnostic: "missing".into(),
                })
            } else {
                answer(call)
            }
        });
        let r = pipeline::run(&d, &c, &b).unwrap();
        assert_eq!(r.state, State::Partial);
        assert_eq!(r.policy_satisfied(), !strict);
        assert_eq!(r.successful_reviewers, ["good"]);
    }
}
#[test]
fn synthesis_fallback_stays_partial_and_strict_verification_checks_other_categories() {
    let d = definition(
        json!({"completeness":{"synthesis_failure":"passthrough","require_synthesis_success":true},"retries":{"synthesis":{"count":0}}}),
        &["a"],
    );
    let c = control(&d);
    let b = backend(|call| {
        if call.attempt.stage == Stage::Synthesis {
            Ok(json!({"invalid":true}))
        } else {
            answer(call)
        }
    });
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(r.state, State::Partial);
    assert!(!r.synthesis_succeeded);
    assert!(!r.policy_satisfied());
    assert_eq!(r.categories.findings[0].verdict, "confirmed");
    let d = definition(
        json!({"completeness":{"require_all_verified":true}}),
        &["a"],
    );
    let c = control(&d);
    let b = backend(|call| {
        if call.attempt.stage == Stage::Synthesis {
            Ok(json!({"findings":[],"open_questions":[{"merged_ids":["a-1"],"file":"src/a.rs"}]}))
        } else if call.attempt.stage == Stage::Verify {
            assert_eq!(call.findings[0].id, "a-1");
            Ok(json!({"verdicts":[]}))
        } else {
            answer(call)
        }
    });
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert!(!r.policy_satisfied());
    assert_eq!(r.categories.open_questions[0].verdict, "unverified");
    assert_eq!(c.calls_spent(), 3);
}
#[test]
fn invalid_roster_fails_before_spending_calls() {
    for agents in [&["a", "a"][..], &["a"][..]] {
        let d = definition(
            json!({"completeness":{"required_reviewers":["absent"]}}),
            agents,
        );
        let c = control(&d);
        let b = backend(answer);
        assert!(pipeline::run(&d, &c, &b).is_err());
        assert_eq!(c.calls_spent(), 0);
    }
}

#[test]
fn source_grouping_uses_resolved_legacy_knob_and_rejected_verdicts_remain_auditable() {
    let d = definition(
        json!({"stages":{"synthesis":{"enabled":false}},"runtime":{"verify_group_by":"source"},"completeness":{"require_all_verified":true}}),
        &["a", "b"],
    );
    let c = control(&d);
    let b = backend(|call| {
        if call.attempt.stage == Stage::Verify {
            assert_eq!(call.findings.len(), 1);
            Ok(json!({"verdicts":[{"id":call.findings[0].id,"verdict":"rejected"}]}))
        } else {
            answer(call)
        }
    });
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(c.calls_spent(), 4);
    assert!(r.policy_satisfied());
    assert!(r.categories.findings.is_empty());
    assert_eq!(r.verification_evidence.len(), 2);
}
#[test]
fn synthesis_runs_on_empty_findings_and_invalid_synthesis_stops_legacy_pipeline() {
    for invalid in [false, true] {
        let d = definition(json!({}), &["a"]);
        let c = control(&d);
        let b = backend(|call| {
            assert_ne!(call.attempt.stage, Stage::Verify);
            Ok(if call.attempt.stage == Stage::Find || !invalid {
                json!({"findings":[]})
            } else {
                json!({"wrong":[]})
            })
        });
        let r = pipeline::run(&d, &c, &b).unwrap();
        assert_eq!(c.calls_spent(), 2);
        assert_eq!(
            r.state,
            if invalid {
                State::Failed
            } else {
                State::Complete
            }
        );
        assert_eq!(r.policy_satisfied(), !invalid);
    }
}
#[test]
fn verification_output_follows_group_order_instead_of_finder_completion_order() {
    let d = definition(
        json!({"stages":{"synthesis":{"enabled":false}},"verification":{"min_directory_group_size":1}}),
        &["a", "b"],
    );
    let c = control(&d);
    let b = backend(|call| {
        if call.attempt.stage == Stage::Find {
            Ok(json!({"findings":[{"file":if call.attempt.unit=="a"{"z/file"}else{"a/file"}}]}))
        } else {
            answer(call)
        }
    });
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(
        r.categories
            .findings
            .iter()
            .map(|f| f.id.as_str())
            .collect::<Vec<_>>(),
        ["b-1", "a-1"]
    );
}

#[test]
fn concurrent_finders_obey_both_limits_and_finish_before_synthesis() {
    use std::sync::Barrier;
    let d = definition(
        json!({"runtime":{"max_parallel":3},"stages":{"find":{"parallelism":2}}}),
        &["a", "b", "c", "d"],
    );
    let c = control(&d);
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let pairs = Barrier::new(2);
    let b = backend(|call| {
        if call.attempt.stage == Stage::Find {
            let n = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(n, Ordering::SeqCst);
            pairs.wait();
            let result = answer(call);
            active.fetch_sub(1, Ordering::SeqCst);
            completed.fetch_add(1, Ordering::SeqCst);
            result
        } else {
            assert_eq!(completed.load(Ordering::SeqCst), 4);
            assert_eq!(active.load(Ordering::SeqCst), 0);
            answer(call)
        }
    });
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(r.state, State::Complete);
    assert_eq!(peak.load(Ordering::SeqCst), 2);
    assert_eq!(c.calls_spent(), 6);
}
#[test]
fn retry_consumes_remaining_budget_and_preserves_separate_failure() {
    let d = definition(
        json!({"budget":{"max_calls":2},"retries":{"find":{"delay":{"base":"0s","jitter":"0s"}}}}),
        &["a"],
    );
    let c = control(&d);
    let b = backend(|call| {
        if call.attempt.number == 0 {
            Err(AttemptFailure {
                kind: Failure::MissingOutput,
                diagnostic: "first attempt evidence".into(),
            })
        } else {
            answer(call)
        }
    });
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(r.state, State::Stopped(StopReason::CallBudget));
    assert_eq!(r.units[0].attempts.len(), 2);
    assert_eq!(
        r.units[0].attempts[0]
            .result
            .as_ref()
            .unwrap_err()
            .diagnostic,
        "first attempt evidence"
    );
    assert_eq!(r.units[0].attempts[1].id.number, 1);
    assert_eq!(r.categories.findings[0].id, "a-1");
    assert_eq!(c.calls_spent(), 2);
}

#[test]
fn legacy_secondary_categories_do_not_gain_verification_or_extra_calls() {
    let d = definition(json!({}), &["a"]);
    let c = control(&d);
    let b = backend(|call| {
        if call.attempt.stage == Stage::Synthesis {
            Ok(json!({"findings":[],"pre_existing":[{"merged_ids":["a-1"]}]}))
        } else {
            answer(call)
        }
    });
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(c.calls_spent(), 2);
    assert_eq!(r.categories.pre_existing[0].verdict, "");
    assert!(r.verification_evidence.is_empty());
}

#[test]
fn stagger_timeout_releases_silent_leader_followers_without_rearming_for_verify() {
    let d = definition(
        json!({"runtime":{"stagger_delay":"40ms"},"stages":{"synthesis":{"enabled":false}}}),
        &["a", "b"],
    );
    let c = control(&d);
    let start = std::time::Instant::now();
    let mut b = backend(|call| {
        if call.attempt.stage == Stage::Find && call.attempt.unit == "b" {
            assert!(start.elapsed() >= Duration::from_millis(40));
        }
        answer(call)
    });
    b.activity = false;
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(r.state, State::Complete);
    assert_eq!(c.calls_spent(), 3);
}
#[test]
fn stagger_wait_obeys_deadline_and_cancellation_without_spending_calls() {
    for cancel in [false, true] {
        let d = definition(
            json!({"budget":{"elapsed":"50ms"},"runtime":{"stagger_delay":"30s"}}),
            &["a", "b"],
        );
        let c = control(&d);
        let mut b = backend(|call| {
            assert_eq!(call.attempt.unit, "a");
            if cancel {
                c.cancel();
            }
            answer(call)
        });
        b.activity = false;
        let r = pipeline::run(&d, &c, &b).unwrap();
        assert_eq!(
            r.state,
            State::Stopped(if cancel {
                StopReason::Cancelled
            } else {
                StopReason::Deadline
            })
        );
        assert_eq!(c.calls_spent(), 1);
        assert_eq!(r.categories.findings.len(), 1);
    }
}
#[test]
fn leader_activity_releases_followers_while_leader_is_still_running() {
    let d = definition(json!({"runtime":{"stagger_delay":"10m"}}), &["a", "b"]);
    let c = control(&d);
    let (sender, receiver) = std::sync::mpsc::channel();
    let receiver = Mutex::new(receiver);
    let b = backend(|call| {
        if call.attempt.stage == Stage::Find {
            if call.attempt.unit == "a" {
                receiver
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2))
                    .expect("activity must release follower before leader completes");
            } else {
                sender.send(()).unwrap();
            }
        }
        answer(call)
    });
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(r.state, State::Complete);
}

#[test]
fn minimum_success_and_disabled_stages_never_fabricate_strict_acceptance() {
    let d = definition(
        json!({"stages":{"synthesis":{"enabled":false},"verify":{"enabled":false}},"completeness":{"min_successful_reviewers":2,"require_synthesis_success":true,"require_all_verified":true},"retries":{"find":{"count":0}}}),
        &["good", "bad"],
    );
    let c = control(&d);
    let b = backend(|call| {
        if call.attempt.unit == "bad" {
            Err(AttemptFailure {
                kind: Failure::MissingOutput,
                diagnostic: "failed reviewer".into(),
            })
        } else {
            answer(call)
        }
    });
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(c.calls_spent(), 2);
    assert_eq!(r.state, State::Partial);
    assert!(!r.policy_satisfied());
    assert_eq!(r.policy_failures.len(), 3);
    assert_eq!(r.stages.len(), 1);
    assert_eq!(r.stages[0].name, "find");
    assert_eq!(r.categories.findings[0].verdict, "unverified");
    let d = definition(json!({"retries":{"find":{"count":0}}}), &["bad"]);
    let c = control(&d);
    let b = backend(|_| {
        Err(AttemptFailure {
            kind: Failure::MissingOutput,
            diagnostic: "no source".into(),
        })
    });
    let r = pipeline::run(&d, &c, &b).unwrap();
    assert_eq!(r.state, State::Failed);
    assert!(!r.policy_satisfied());
    assert_eq!(c.calls_spent(), 1);
}

#[test]
fn synthesis_receives_successful_empty_and_degraded_sources_in_roster_order() {
    let d = definition(
        json!({"retries":{"find":{"count":0}},"stages":{"verify":{"enabled":false}}}),
        &["empty", "problem", "failed"],
    );
    let b = backend(|call| match call.attempt.stage {
        Stage::Find if call.attempt.unit == "failed" => Err(AttemptFailure {
            kind: Failure::NonzeroExit,
            diagnostic: "synthetic failure".into(),
        }),
        Stage::Find if call.attempt.unit == "empty" => Ok(json!({"findings":[]})),
        Stage::Find => Ok(json!({"findings":[{"title":"one","file":"src/lib.rs"}]})),
        Stage::Synthesis => {
            assert_eq!(
                call.sources
                    .iter()
                    .map(|s| s.agent.name.as_str())
                    .collect::<Vec<_>>(),
                ["empty", "problem", "failed"]
            );
            assert!(call.sources[0].findings.as_ref().unwrap().is_empty());
            assert_eq!(
                call.sources[1].findings.as_ref().unwrap()[0].id,
                "problem-1"
            );
            assert!(call.sources[2].findings.is_none());
            Ok(json!({"findings":[]}))
        }
        _ => panic!("verification is disabled"),
    });
    let report = pipeline::run(&d, &control(&d), &b).unwrap();
    assert_eq!(report.state, State::Partial);
    assert_eq!(report.successful_reviewers, ["empty", "problem"]);
}

#[test]
fn stage_timings_use_run_clock_and_empty_verify_has_no_runner() {
    use std::sync::atomic::AtomicU64;
    struct Clock(AtomicU64);
    impl revust_core::runtime::Clock for Clock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::SeqCst))
        }
    }
    for empty in [false, true] {
        let d = definition(json!({"runtime":{"stagger_delay":"0s"}}), &["a"]);
        let clock = Arc::new(Clock(AtomicU64::new(100)));
        let c = RunControl::new(Limits::from_policy(&d.policy).unwrap(), clock.clone()).unwrap();
        let b = backend(|call: Call<'_>| {
            clock.0.fetch_add(
                match call.attempt.stage {
                    Stage::Find => 10,
                    Stage::Synthesis => 20,
                    Stage::Verify => 30,
                },
                Ordering::SeqCst,
            );
            if empty {
                Ok(json!({"findings":[]}))
            } else {
                answer(call)
            }
        });
        let r = pipeline::run(&d, &c, &b).unwrap();
        assert_eq!(r.duration_ms, if empty { 30 } else { 60 });
        assert_eq!(
            r.stages
                .iter()
                .map(|s| (s.name.as_str(), s.duration_ms))
                .collect::<Vec<_>>(),
            vec![
                ("find", 10),
                ("synthesis", 20),
                ("verify", if empty { 0 } else { 30 })
            ]
        );
        assert!(r.stages[0].executor.is_empty());
        assert_eq!(r.stages[1].executor, "claude");
        assert_eq!(r.stages[2].executor, if empty { "" } else { "opencode" });
    }
}

#[test]
fn report_accounting_keeps_failed_attempt_tokens_and_unknown_models() {
    use revust_core::report::{
        Scope,
        accounting::{self, Usage},
    };
    use std::collections::BTreeMap;
    let d = definition(
        json!({"runtime":{"stagger_delay":"0s"},"retries":{"find":{"delay":{"base":"0s","jitter":"0s"}}}}),
        &["a", "b"],
    );
    let c = control(&d);
    let b = backend(|call: Call<'_>| {
        if call.attempt.stage == Stage::Find && call.attempt.unit == "b" {
            return Ok(json!({"findings":"bad schema"}));
        }
        if call.attempt.stage == Stage::Find && call.attempt.number == 0 {
            return Err(AttemptFailure {
                kind: Failure::NonzeroExit,
                diagnostic: "retry".into(),
            });
        }
        answer(call)
    });
    let runtime = pipeline::run(&d, &c, &b).unwrap();
    let mut usage = BTreeMap::new();
    for unit in &runtime.units {
        for attempt in &unit.attempts {
            usage.insert(
                attempt.id.clone(),
                Usage {
                    tokens: Some(7),
                    actual_model: if attempt.id.number == 0 {
                        Some("observed".into())
                    } else {
                        None
                    },
                },
            );
        }
    }
    let report = accounting::assemble(
        Scope {
            task: "test".into(),
            run: "01".into(),
            scope_path: "/synthetic/scope.md".into(),
        },
        &d,
        &runtime,
        &usage,
        "2026-10-10T10:00:00Z".into(),
        "2026-10-10T10:00:02Z".into(),
    )
    .unwrap();
    assert_eq!(report.sources.expected, 2);
    assert_eq!(report.sources.reported, 1);
    assert_eq!(report.sources.degraded, ["b"]);
    assert_eq!(report.sources.agents[0].tokens, 14);
    assert_eq!(report.sources.agents[0].actual_model, "observed");
    assert_eq!(report.sources.agents[0].raised, 1);
    assert_eq!(report.sources.agents[1].tokens, 7);
    assert_eq!(report.sources.agents[1].raised, 0);
    assert!(report.sources.agents[1].degraded);
    assert_eq!(report.stats.tokens, 35);
    assert_eq!(report.findings[0].verdict, "confirmed");
    assert_eq!(report.stats.stages.len(), 3);
    let unknown = accounting::assemble(
        Scope::default(),
        &d,
        &runtime,
        &BTreeMap::new(),
        "0001-01-01T00:00:00Z".into(),
        "0001-01-01T00:00:00Z".into(),
    )
    .unwrap();
    assert_eq!(unknown.stats.tokens, 0);
    assert!(unknown.sources.agents[0].actual_model.is_empty());
}

#[test]
fn presentation_failure_cancels_before_any_provider_dispatch() {
    struct Unwritable;
    impl Backend for Unwritable {
        fn publish(&self, _: revust_core::runtime::events::Event) -> Result<(), String> {
            Err("event storage unavailable".into())
        }
        fn execute(&self, _: Call<'_>, _: &RunControl) -> Result<Value, AttemptFailure> {
            panic!("must not dispatch after event failure")
        }
        fn record(&self, _: Event<'_, Categories>) -> Result<(), String> {
            unreachable!()
        }
        fn wait(&self, _: Duration) {
            unreachable!()
        }
        fn jitter(&self, _: Duration) -> Duration {
            unreachable!()
        }
    }
    let definition = definition(json!({}), &["one"]);
    let control = control(&definition);
    assert!(
        matches!(pipeline::run(&definition, &control, &Unwritable), Err(error) if error == "event storage unavailable")
    );
    assert_eq!(control.calls_spent(), 0);
    assert!(control.check_running().is_some());
}

#[test]
fn synthesis_drops_and_verifier_changes_are_explained_by_lifecycle_events() {
    use revust_core::runtime::events::Kind;
    let definition = definition(json!({"runtime":{"stagger_delay":"0s"}}), &["first"]);
    let control = control(&definition);
    let backend = backend(|call| {
        Ok(match call.attempt.stage {
            Stage::Find => json!({"findings":[
            {"file":"src/a.rs","title":"kept","severity":"critical"},
            {"file":"src/b.rs","title":"dropped","severity":"major"}]}),
            Stage::Synthesis => {
                json!({"findings":[{"merged_ids":["first-1"],"file":"src/a.rs","title":"merged","severity":"critical"}]})
            }
            Stage::Verify => {
                json!({"verdicts":[{"id":"first-1","verdict":"refined","severity":"minor"}]})
            }
        })
    });
    pipeline::run(&definition, &control, &backend).unwrap();
    let events = backend.published.lock().unwrap();
    let dropped = events
        .iter()
        .find(|e| matches!(e.kind, Kind::Dropped))
        .unwrap();
    assert_eq!(dropped.findings[0].id, "first-2");
    assert!(!dropped.text.is_empty());
    let verified = events
        .iter()
        .find(|e| matches!(e.kind, Kind::AgentDone) && e.agent == "verify src")
        .unwrap();
    assert!(!verified.text.is_empty());
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, Kind::Findings) && e.agent.starts_with("verify "))
    );
}

#[test]
fn accounting_rejects_duplicate_attempts_and_ignores_unexecuted_observations() {
    use revust_core::report::{
        Scope,
        accounting::{self, Usage},
    };
    use std::collections::BTreeMap;
    let definition = definition(
        json!({"runtime":{"stagger_delay":"0s"},"stages":{"synthesis":{"enabled":false},"verify":{"enabled":false}}}),
        &["a"],
    );
    let control = control(&definition);
    let backend = backend(answer);
    let mut runtime = pipeline::run(&definition, &control, &backend).unwrap();
    let usage = BTreeMap::from([(
        Attempt {
            stage: Stage::Find,
            unit: "not-executed".into(),
            number: 0,
        },
        Usage {
            tokens: Some(1000),
            actual_model: Some("unobserved".into()),
        },
    )]);
    let report = accounting::assemble(
        Scope::default(),
        &definition,
        &runtime,
        &usage,
        String::new(),
        String::new(),
    )
    .unwrap();
    assert_eq!(report.stats.tokens, 0);
    assert_eq!(report.sources.agents[0].actual_model, "");
    let duplicate =
        serde_json::from_value(serde_json::to_value(&runtime.units[0]).unwrap()).unwrap();
    runtime.units.push(duplicate);
    assert!(
        accounting::assemble(
            Scope::default(),
            &definition,
            &runtime,
            &usage,
            String::new(),
            String::new()
        )
        .is_err()
    );
}
