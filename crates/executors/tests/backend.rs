#![cfg(unix)]
use revust_core::{
    config::runtime::Invocation,
    plan::PromptPlan,
    runtime::{
        Limits, MonotonicClock, RunControl,
        journal::{self, Identity, Journal},
        pipeline::State,
    },
};
use revust_executors::backend::{Backend, Options};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc, time::Duration};
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn protocol(executor: &str, payload: Value) -> String {
    match executor {
  "claude"=>json!({"type":"result","structured_output":payload,"modelUsage":{"observed-model":{"outputTokens":3}},"usage":{"input_tokens":2,"output_tokens":3}}).to_string()+"\n",
  "codex"=>payload.to_string()+"\n",
  _=>[json!({"type":"text","part":{"id":"text","messageID":"message","text":payload.to_string()}}),json!({"type":"step_finish","part":{"messageID":"message","reason":"stop","tokens":{"input":2,"output":3,"cache":{"read":0,"write":0}}}})].iter().map(|v|v.to_string()+"\n").collect(),
 }
}
fn setup(index: usize) -> (Temp, revust_core::plan::ExecutionPlan, Journal) {
    setup_with_policy(index, json!({}))
}
fn setup_with_policy(
    index: usize,
    extra_policy: Value,
) -> (Temp, revust_core::plan::ExecutionPlan, Journal) {
    let root = std::env::temp_dir().join(format!("revust-backend-{}-{index}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let temp = Temp(root);
    let config = temp.0.join(".revust");
    fs::create_dir_all(config.join("extension-profiles")).unwrap();
    fs::create_dir(config.join("prompts")).unwrap();
    fs::write(config.join("profile.md"), "frozen calibration").unwrap();
    fs::write(
        config.join("prompts/synthesis.md"),
        "SYNTHESIS\n{{FINDINGS}}\n{{SOURCES}}\n",
    )
    .unwrap();
    fs::write(config.join("prompts/verify.md"), "VERIFY\n{{FINDINGS}}\n").unwrap();
    let executors = ["claude", "codex", "opencode"];
    let synthesis = executors[index % 3];
    let verify = executors[(index + 1) % 3];
    let profile = format!(
        "---\nrunner:\n  executor: claude\n  model: fixture/model\nagents:\n  - name: a\n    lenses: [bugs]\n    runner:\n      executor: claude\n      model: fixture/model\n  - name: b\n    lenses: [bugs]\n    runner:\n      executor: codex\n      model: fixture/model\n  - name: c\n    lenses: [bugs]\n    runner:\n      executor: opencode\n      model: fixture/model\nstages:\n  synthesis:\n    executor: {synthesis}\n    model: fixture/model\n  verify:\n    executor: {verify}\n    model: fixture/model\n---\nFIND\n{{SCOPE}}\n"
    );
    fs::write(config.join("extension-profiles/mixed.md"), profile).unwrap();
    let mut overrides = json!({});
    for executor in executors {
        let path = temp.0.join(executor);
        let find = protocol(
            executor,
            json!({"findings":[{"file":"src/lib.rs","line":1,"title":"Synthetic finding","confidence":90,"severity":"major"}]}),
        );
        let synth = protocol(
            executor,
            json!({"findings":[{"file":"src/lib.rs","line":1,"title":"Merged finding","confidence":90,"severity":"major","merged_ids":["a-1","b-1","c-1"]}],"open_questions":[],"pre_existing":[]}),
        );
        let verify = protocol(
            executor,
            json!({"verdicts":[{"id":"a-1","verdict":"confirmed"}]}),
        );
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'opencode v2.0.21\\n'; exit 0; fi\ninput=$(/bin/cat)\ncase \"$input\" in\nFIND*) printf '%s' '{}' ;;\nSYNTHESIS*) printf '%s' '{}' ;;\nVERIFY*) printf '%s' '{}' ;;\n*) exit 90 ;;\nesac\n{}\n",
            find,
            synth,
            verify,
            if executor == "codex" {
                "printf 'model: observed-model\\ntokens used\\n5\\n' >&2"
            } else {
                "printf diagnostic >&2"
            }
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        overrides[executor] = json!({"executable":path});
    }
    let mut policy = json!({"runtime":{"stagger_delay":"0s"},"executors":overrides});
    revust_core::config::extensions::merge(&mut policy, &extra_policy);
    fs::write(config.join("extensions.json"),json!({"version":1,"policy":policy,"profiles":{"mixed":{"file":"extension-profiles/mixed.md"}}}).to_string()).unwrap();
    let tasks = temp.0.join("tasks");
    let round = tasks.join("test/01");
    fs::create_dir_all(round.join("input")).unwrap();
    fs::write(round.join("input/scope.md"), "synthetic scope").unwrap();
    let invocation = Invocation {
        config_dir: Some(temp.0.join("user")),
        workdir: Some(temp.0.clone()),
        task: "test".into(),
        run: "01".into(),
        overrides: vec![
            ("profile".into(), "mixed".into()),
            ("tasks-dir".into(), tasks.to_str().unwrap().into()),
        ],
    };
    let plan = PromptPlan::resolve(&temp.0, None, &invocation, &[])
        .unwrap()
        .into_execution()
        .unwrap();
    fs::write(config.join("profile.md"), "changed calibration").unwrap();
    let journal = Journal::create(
        &round,
        Identity {
            task: "test".into(),
            run: "01".into(),
            request: "synthetic".into(),
        },
    )
    .unwrap();
    (temp, plan, journal)
}
#[test]
fn all_three_adapters_complete_the_pipeline_and_archive_attempt_evidence() {
    for index in 0..3 {
        let (temp, plan, journal) = setup(index);
        let round = temp.0.join("tasks/test/01");
        let opencode = temp.0.join("opencode");
        let version = ["v2.0.21", "v2.7.91", "v2.19.0-rc.1+synthetic"][index];
        let script = fs::read_to_string(&opencode).unwrap();
        fs::write(&opencode, script.replace("v2.0.21", version)).unwrap();
        let claude = temp.0.join("claude");
        let script = fs::read_to_string(&claude).unwrap();
        let activity = json!({"type":"assistant","message":{"content":[{"type":"text","text":"synthetic activity"},{"type":"tool_use","name":"Read","input":{}}]}});
        fs::write(
            &claude,
            script.replace(
                "input=$(/bin/cat)",
                &format!("printf '%s\\n' '{}'\ninput=$(/bin/cat)", activity),
            ),
        )
        .unwrap();
        let control = RunControl::new(
            Limits::from_policy(&plan.definition().policy).unwrap(),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
        let observer = journal.subscribe().unwrap();
        let backend = Backend::prepare(
            plan,
            journal,
            Options::default(),
            |_, _| {},
            |_| Duration::ZERO,
        )
        .unwrap();
        assert_eq!(
            fs::read(round.join("prompts/input-profile.md")).unwrap(),
            b"frozen calibration"
        );
        assert_eq!(control.calls_spent(), 0);
        for name in ["a", "b", "c"] {
            assert!(round.join(format!("prompts/agents/{name}.md")).is_file());
        }
        let completed = backend.run_report(&control).unwrap();
        let events: Vec<Value> = fs::read_to_string(round.join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let observed: Vec<Value> = observer
            .try_iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect();
        assert_eq!(observed, events);
        assert!(matches!(
            observer.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(
            events
                .iter()
                .filter(|e| e["kind"] == "stage")
                .map(|e| e["stage"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["find", "synthesis", "verify"]
        );
        assert!(
            events
                .iter()
                .all(|e| e["at"].as_str().is_some_and(|at| at.ends_with('Z')))
        );
        assert!(
            events
                .iter()
                .any(|e| e["kind"] == "agent_state" && e["agent"] == "b")
        );
        for name in ["a", "b", "c", "synthesis", "verify src"] {
            let kinds: Vec<_> = events
                .iter()
                .filter(|e| e["agent"] == name)
                .map(|e| e["kind"].as_str().unwrap())
                .collect();
            assert_eq!(kinds.first(), Some(&"agent_started"), "{name}");
            assert_eq!(kinds.last(), Some(&"agent_done"), "{name}");
        }
        assert!(events.iter().any(|e| e["kind"] == "agent_activity"
            && e["agent"] == "a"
            && e["text"] == "synthetic activity"));
        assert!(
            events
                .iter()
                .any(|e| e["kind"] == "agent_progress" && e["agent"] == "a")
        );
        if index == 0 {
            assert!(
                events
                    .iter()
                    .any(|e| e["kind"] == "agent_state" && e["agent"] == "verify src")
            );
        }
        for (file, count, tokens, verdict, stages) in [
            ("1-found.json", 3, 15, "", 1),
            ("2-synthesized.json", 1, 20, "", 2),
            ("3-verified.json", 1, 25, "confirmed", 3),
        ] {
            let stage = revust_core::report::Report::decode(
                &fs::read_to_string(round.join("stages").join(file)).unwrap(),
            )
            .unwrap();
            assert_eq!(stage.findings.len(), count);
            assert_eq!(stage.findings[0].verdict, verdict);
            assert_eq!(stage.stats.tokens, tokens);
            assert_eq!(stage.stats.stages.len(), stages);
            assert_eq!(stage.sources.reported, 3);
            assert_eq!(stage.stats.started_at, "0001-01-01T00:00:00Z");
            assert_eq!(stage.stats.duration_ms, 0);
            assert!(stage.scope.task.is_empty());
        }
        assert_eq!(completed.report.stats.tokens, 25);
        assert_eq!(completed.report.sources.reported, 3);
        assert_eq!(
            completed
                .report
                .sources
                .agents
                .iter()
                .map(|a| a.tokens)
                .collect::<Vec<_>>(),
            [5, 5, 5]
        );
        assert_eq!(completed.report.scope.task, "test");
        assert_eq!(completed.report.scope.run, "01");
        assert_eq!(
            completed.report.scope.scope_path,
            round.join("input/scope.md").to_str().unwrap()
        );
        assert_eq!(completed.report.stats.stages.len(), 3);
        assert_eq!(
            completed.report.sources.agents[0].actual_model,
            "observed-model"
        );
        assert!(completed.report.sources.agents[2].actual_model.is_empty());
        assert_eq!(completed.exit_code(), 1);
        completed.report.json().unwrap();
        let report = completed.runtime;
        assert_eq!(report.state, State::Complete);
        assert_eq!(control.calls_spent(), 5);
        assert_eq!(report.categories.findings[0].verdict, "confirmed");
        assert_eq!(report.categories.findings[0].sources, ["a", "b", "c"]);
        let snapshot = journal::inspect(&round).unwrap();
        assert_eq!(snapshot.attempts.len(), 5);
        assert!(snapshot.report.is_some());
        for attempt in snapshot.attempts {
            let request: Value =
                serde_json::from_slice(&fs::read(attempt.directory.join("request.json")).unwrap())
                    .unwrap();
            let delivery: Value =
                serde_json::from_slice(&fs::read(attempt.directory.join("delivery.json")).unwrap())
                    .unwrap();
            assert!(!request["stdin"].as_str().unwrap().is_empty());
            let prompt = match attempt.id.stage {
                revust_core::runtime::Stage::Find => {
                    format!("prompts/agents/{}.md", attempt.id.unit)
                }
                revust_core::runtime::Stage::Synthesis => "prompts/stages/synthesis.md".into(),
                revust_core::runtime::Stage::Verify => {
                    format!("prompts/stages/verify-{}.md", attempt.id.unit)
                }
            };
            assert_eq!(
                fs::read_to_string(round.join(prompt)).unwrap(),
                request["stdin"].as_str().unwrap()
            );
            assert_eq!(delivery["tokens"], 5);
            if delivery["requested"]["executor"] == "opencode" {
                assert!(delivery["actual_model"].is_null());
            } else {
                assert_eq!(delivery["actual_model"], "observed-model");
            }
            assert!(
                !fs::read(attempt.directory.join("stdout"))
                    .unwrap()
                    .is_empty()
            );
            assert!(!attempt.directory.join("opencode").exists());
        }
        assert!(backend.run(&control).is_err());
        assert_eq!(control.calls_spent(), 5);
    }
}

#[test]
fn explicit_codex_home_is_the_child_home_and_missing_output_retries_separately() {
    let (temp, plan, journal) = setup(10);
    let home = temp.0.join("selected-codex");
    fs::create_dir(&home).unwrap();
    let executable = temp.0.join("codex");
    let original = fs::read_to_string(&executable).unwrap();
    // The first attempt emits incomplete evidence; its retry must be distinct.
    let guard = format!(
        "#!/bin/sh\nif [ \"$CODEX_HOME\" != '{}' ]; then exit 91; fi\nif [ ! -f \"$CODEX_HOME/attempted\" ]; then touch \"$CODEX_HOME/attempted\"; printf partial-answer; exit 1; fi\n",
        home.display()
    );
    fs::write(
        executable,
        guard + original.strip_prefix("#!/bin/sh\n").unwrap(),
    )
    .unwrap();
    let control = RunControl::new(
        Limits::from_policy(&plan.definition().policy).unwrap(),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    let backend = Backend::prepare(
        plan,
        journal,
        Options {
            codex_home: Some(home),
            ..Options::default()
        },
        |_, _| {},
        |_| Duration::ZERO,
    )
    .unwrap();
    let report = backend.run(&control).unwrap();
    assert!(report.successful_reviewers.contains(&"b".into()));
    let snapshot = journal::inspect(&temp.0.join("tasks/test/01")).unwrap();
    let attempts: Vec<_> = snapshot
        .attempts
        .iter()
        .filter(|a| a.id.unit == "b")
        .collect();
    assert_eq!(attempts.len(), 2);
    assert!(attempts[0].result.as_ref().unwrap().is_err());
    assert!(attempts[1].result.as_ref().unwrap().is_ok());
    assert_eq!(
        fs::read(attempts[0].directory.join("stdout")).unwrap(),
        b"partial-answer"
    );
    assert_ne!(attempts[0].directory, attempts[1].directory);
    let events: Vec<Value> = fs::read_to_string(temp.0.join("tasks/test/01/events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let lifecycle: Vec<_> = events
        .iter()
        .filter(|e| e["agent"] == "b")
        .filter_map(|e| e["kind"].as_str())
        .filter(|kind| {
            [
                "agent_started",
                "agent_retried",
                "findings",
                "agent_done",
                "agent_degraded",
            ]
            .contains(kind)
        })
        .collect();
    assert_eq!(
        lifecycle,
        ["agent_started", "agent_retried", "findings", "agent_done"]
    );
    let prompt = fs::read_to_string(temp.0.join("tasks/test/01/prompts/agents/b.md")).unwrap();
    for attempt in attempts {
        let request: Value =
            serde_json::from_slice(&fs::read(attempt.directory.join("request.json")).unwrap())
                .unwrap();
        assert_eq!(request["stdin"], prompt);
    }
}

#[test]
fn structured_nonzero_exit_is_delivered_but_invalid_schema_is_not_retried() {
    for (index, invalid) in [(11, false), (12, true)] {
        let (temp, plan, journal) = setup(index);
        let path = temp.0.join("claude");
        let original = fs::read_to_string(&path).unwrap();
        let script = if invalid {
            format!(
                "#!/bin/sh\n/bin/cat >/dev/null\nprintf '%s' '{}'\n",
                protocol("claude", json!({"findings":"invalid"}))
            )
        } else {
            original + "\nexit 7\n"
        };
        fs::write(path, script).unwrap();
        let control = RunControl::new(
            Limits::from_policy(&plan.definition().policy).unwrap(),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
        let backend = Backend::prepare(
            plan,
            journal,
            Options::default(),
            |_, _| {},
            |_| Duration::ZERO,
        )
        .unwrap();
        let report = backend.run(&control).unwrap();
        assert_eq!(report.successful_reviewers.contains(&"a".into()), !invalid);
        let events: Vec<Value> = fs::read_to_string(temp.0.join("tasks/test/01/events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            events
                .iter()
                .any(|e| e["agent"] == "a" && e["kind"] == "agent_degraded"),
            invalid
        );
        assert_eq!(
            events
                .iter()
                .any(|e| e["agent"] == "a" && e["kind"] == "agent_done"),
            !invalid
        );
        let snapshot = journal::inspect(&temp.0.join("tasks/test/01")).unwrap();
        let attempts: Vec<_> = snapshot
            .attempts
            .iter()
            .filter(|a| a.id.unit == "a")
            .collect();
        assert_eq!(attempts.len(), 1);
        let result = attempts[0].result.as_ref().unwrap();
        if invalid {
            assert_eq!(
                result.as_ref().unwrap_err().kind,
                revust_core::runtime::retry::Failure::InvalidOutput
            );
        } else {
            assert!(result.is_ok());
        }
    }
}

#[test]
fn conflicting_snapshot_or_round_is_rejected_before_provider_dispatch() {
    for index in [13, 14] {
        let (temp, plan, journal) = setup(index);
        let round = temp.0.join("tasks/test/01");
        let journal = if index == 13 {
            fs::create_dir(round.join("prompts")).unwrap();
            fs::write(round.join("prompts/input-profile.md"), "existing evidence").unwrap();
            journal
        } else {
            let other = temp.0.join("other-round");
            fs::create_dir(&other).unwrap();
            Journal::create(
                &other,
                Identity {
                    task: "test".into(),
                    run: "other".into(),
                    request: "other".into(),
                },
            )
            .unwrap()
        };
        let result = Backend::prepare(
            plan,
            journal,
            Options::default(),
            |_, _| {},
            |_| Duration::ZERO,
        );
        assert!(result.is_err());
        assert!(journal::inspect(&round).unwrap().attempts.is_empty());
        if index == 13 {
            assert_eq!(
                fs::read(round.join("prompts/input-profile.md")).unwrap(),
                b"existing evidence"
            );
        }
    }
}

#[test]
fn deadline_retains_delivered_payload_without_accepting_a_stopped_attempt() {
    let (temp, plan, journal) = setup(15);
    let path = temp.0.join("claude");
    let original = fs::read_to_string(&path).unwrap();
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Clock {
        base: MonotonicClock,
        delivered: AtomicBool,
    }
    impl revust_core::runtime::Clock for Clock {
        fn now(&self) -> Duration {
            self.base.now()
                + if self.delivered.load(Ordering::SeqCst) {
                    Duration::from_secs(61)
                } else {
                    Duration::ZERO
                }
        }
    }
    let clock = Arc::new(Clock {
        base: MonotonicClock::default(),
        delivered: AtomicBool::new(false),
    });
    let observed = clock.clone();
    let activity = json!({"type":"assistant","message":{"content":[{"type":"text","text":"delivery complete"}]}});
    fs::write(
        path,
        original + &format!("\nprintf '%s\\n' '{}'\nexec /bin/sleep 30\n", activity),
    )
    .unwrap();
    let mut limits = Limits::from_policy(&plan.definition().policy).unwrap();
    limits.elapsed = Some(Duration::from_secs(60));
    let backend = Backend::prepare(
        plan,
        journal,
        Options::default(),
        move |attempt,event| {if attempt.unit=="a" && matches!(event,revust_executors::events::Event::Activity(text) if text=="delivery complete") {observed.delivered.store(true,Ordering::SeqCst);}},
        |_| Duration::ZERO,
    )
    .unwrap();
    let control = RunControl::new(limits, clock).unwrap();
    let started = std::time::Instant::now();
    let completed = backend.run_report(&control).unwrap();
    assert_eq!(completed.exit_code(), 2);
    assert!(completed.report.sources.agents[0].degraded);
    assert_eq!(completed.report.sources.agents[0].tokens, 5);
    let report = completed.runtime;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        report.state,
        State::Stopped(revust_core::runtime::StopReason::Deadline)
    );
    assert!(!report.successful_reviewers.contains(&"a".into()));
    let snapshot = journal::inspect(&temp.0.join("tasks/test/01")).unwrap();
    let attempt = snapshot.attempts.iter().find(|a| a.id.unit == "a").unwrap();
    assert!(attempt.result.as_ref().unwrap().is_err());
    let delivery: Value =
        serde_json::from_slice(&fs::read(attempt.directory.join("delivery.json")).unwrap())
            .unwrap();
    assert_eq!(
        delivery["output"]["findings"][0]["title"],
        "Synthetic finding"
    );
    assert_eq!(delivery["process"]["termination"], "RunStopped(Deadline)");
}

#[test]
fn disabled_stages_write_no_snapshot_and_stage_storage_failure_stops_dispatch() {
    for failed_storage in [false, true] {
        let index = if failed_storage { 31 } else { 30 };
        let policy = if failed_storage {
            json!({})
        } else {
            json!({"stages":{"synthesis":{"enabled":false},"verify":{"enabled":false}}})
        };
        let (temp, plan, journal) = setup_with_policy(index, policy);
        let round = temp.0.join("tasks/test/01");
        let control = RunControl::new(
            Limits::from_policy(&plan.definition().policy).unwrap(),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
        if failed_storage {
            fs::create_dir(temp.0.join("outside")).unwrap();
            std::os::unix::fs::symlink(temp.0.join("outside"), round.join("stages")).unwrap();
        }
        let backend = Backend::prepare(
            plan,
            journal,
            Options::default(),
            |_, _| {},
            |_| Duration::ZERO,
        )
        .unwrap();
        if failed_storage {
            assert!(
                matches!(backend.run_report(&control), Err(error) if error.contains("stage snapshot"))
            );
            assert_eq!(fs::read_dir(temp.0.join("outside")).unwrap().count(), 0);
            assert!(journal::inspect(&round).unwrap().report.is_none());
            assert!(backend.run(&control).is_err());
        } else {
            let result = backend.run_report(&control).unwrap();
            assert_eq!(result.report.findings[0].verdict, "unverified");
            assert!(round.join("stages/1-found.json").is_file());
            assert!(!round.join("stages/2-synthesized.json").exists());
            assert!(!round.join("stages/3-verified.json").exists());
            let events = fs::read_to_string(round.join("events.jsonl")).unwrap();
            let stages: Vec<Value> = events
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .filter(|event| event["kind"] == "stage")
                .map(|event| event["stage"].clone())
                .collect();
            assert_eq!(stages, [json!("find")]);
        }
        assert_eq!(control.calls_spent(), 3);
    }
}

#[test]
fn prior_stage_is_durable_before_the_next_provider_starts() {
    let (temp, plan, journal) = setup(33);
    let round = temp.0.join("tasks/test/01");
    for executor in ["claude", "codex", "opencode"] {
        let executable = temp.0.join(executor);
        let script = fs::read_to_string(&executable).unwrap();
        let guard = format!(
            "case \"$input\" in\nSYNTHESIS*) [ -s '{}/stages/1-found.json' ] || exit 92 ;;\nVERIFY*) [ -s '{}/stages/2-synthesized.json' ] || exit 93 ;;\nesac\n",
            round.display(),
            round.display()
        );
        fs::write(
            executable,
            script.replace("case \"$input\" in\n", &(guard + "case \"$input\" in\n")),
        )
        .unwrap();
    }
    let control = RunControl::new(
        Limits::from_policy(&plan.definition().policy).unwrap(),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    let backend = Backend::prepare(
        plan,
        journal,
        Options::default(),
        |_, _| {},
        |_| Duration::ZERO,
    )
    .unwrap();
    let result = backend.run_report(&control).unwrap();
    assert_eq!(result.runtime.state, State::Complete);
    assert_eq!(result.report.findings[0].verdict, "confirmed");
    assert_eq!(control.calls_spent(), 5);
}

#[test]
fn saved_stage_snapshots_support_stats_and_replay_without_external_tools() {
    let (temp, plan, journal) = setup(32);
    let codex = temp.0.join("codex");
    let original = fs::read_to_string(&codex).unwrap();
    let marker = temp.0.join("codex-first-attempt");
    fs::write(
        &codex,
        format!(
            "#!/bin/sh\nif [ ! -f '{}' ]; then : > '{}'; printf retry-fixture >&2; exit 1; fi\n{}",
            marker.display(),
            marker.display(),
            original.strip_prefix("#!/bin/sh\n").unwrap()
        ),
    )
    .unwrap();
    let round = temp.0.join("tasks/test/01");
    let mut archive =
        revust_core::archive::Archive::claim(&temp.0.join("tasks"), "test", "01").unwrap();
    let control = RunControl::new(
        Limits::from_policy(&plan.definition().policy).unwrap(),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    let backend = Backend::prepare(
        plan,
        journal,
        Options::default(),
        |_, _| {},
        |_| Duration::ZERO,
    )
    .unwrap();
    let completed = backend.run_report(&control).unwrap();
    archive.save_report(&completed.report).unwrap();
    archive.finish(completed.manifest.as_bytes()).unwrap();
    drop(archive);
    let reopened = revust_core::archive::reader::load(&round).unwrap();
    assert_eq!(reopened.manifest.profile, "mixed");
    assert_eq!(reopened.roster.len(), 3);
    assert_eq!(reopened.roster[2].executor, "opencode");
    assert_eq!(reopened.manifest.tokens, 25);
    assert_eq!(reopened.manifest.prompts.len(), 23);
    assert!(reopened.manifest.prompts.iter().all(|p| p.hash.len() == 64));
    // Corpus inventory observes the released writer claim.
    let stats = serde_json::to_value(
        revust_core::archive::stats::collect(&temp.0.join("tasks"), "").unwrap(),
    )
    .unwrap();
    let totals = &stats["totals"];
    assert_eq!(totals["rounds"], 1);
    assert!(
        totals["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|note| note.as_str().unwrap().contains("unavailable"))
    );
    assert_eq!(totals["agents"], json!(["a", "b", "c"].map(|name| json!({
        "name":name,"raised":1,"survived":1,"corroborated":1,"degraded_rounds":0,"retries":if name == "b" { 1 } else { 0 },"tokens":5
    }))));
    assert_eq!(
        totals["lenses"],
        json!([{"name":"bugs","raised":3,"ambiguous":0,"verdicts":{"confirmed":1}}])
    );
    assert_eq!(
        totals["stages"],
        json!([{"name":"synthesis","in":3,"out":1},{"name":"verify","in":1,"out":1},{"name":"report","in":1,"out":1}])
    );
    assert_eq!(
        reopened.report.json().unwrap(),
        completed.report.json().unwrap()
    );
}

#[test]
fn failed_synthesis_does_not_publish_a_successful_stage_snapshot() {
    let (temp, plan, journal) = setup(34);
    let executable = temp.0.join("codex");
    let script = fs::read_to_string(&executable).unwrap();
    fs::write(&executable, script.replace("case \"$input\" in\n", "case \"$input\" in\nSYNTHESIS*) printf '%s' '{\"findings\":\"invalid\"}'; exit 0 ;;\nesac\ncase \"$input\" in\n")).unwrap();
    let control = RunControl::new(
        Limits::from_policy(&plan.definition().policy).unwrap(),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    let backend = Backend::prepare(
        plan,
        journal,
        Options::default(),
        |_, _| {},
        |_| Duration::ZERO,
    )
    .unwrap();
    let result = backend.run_report(&control).unwrap();
    assert_eq!(result.runtime.state, State::Failed);
    assert_eq!(control.calls_spent(), 4);
    let round = temp.0.join("tasks/test/01");
    assert!(round.join("stages/1-found.json").exists());
    assert!(!round.join("stages/2-synthesized.json").exists());
    assert!(!round.join("stages/3-verified.json").exists());
}

#[test]
fn preparation_archives_frozen_extension_profile_on_owned_sidecar() {
    let (temp, plan, journal) = setup(80);
    let snapshot = plan.extension_profile_snapshot().unwrap();
    let source = snapshot.source.clone();
    let content = snapshot.content.clone();
    fs::remove_file(&source).unwrap();
    let round = temp.0.join("tasks/test/01");
    fs::rename(round.join("revust"), round.join("saved-revust")).unwrap();
    fs::create_dir(round.join("revust")).unwrap();
    let _backend = Backend::prepare(
        plan,
        journal,
        Options::default(),
        |_, _| {},
        |_| Duration::ZERO,
    )
    .unwrap();
    let metadata: Value = serde_json::from_slice(
        &fs::read(round.join("saved-revust/extension-profile.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(metadata["version"], 1);
    assert_eq!(metadata["profile"], "mixed");
    assert_eq!(metadata["source"], source.to_str().unwrap());
    assert_eq!(metadata["content"], content);
    assert!(!round.join("revust/extension-profile.json").exists());
    assert!(journal::inspect(&round).is_err()); // Replacement is not the owned journal.
    assert_eq!(
        fs::read_dir(round.join("saved-revust/attempts"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn preparation_archives_frozen_policy_with_origins_but_no_environment_values() {
    let (temp, plan, journal) = setup_with_policy(
        81,
        json!({
            "budget":{"max_calls":7},
            "executors":{"claude":{"env":{"SYNTHETIC_TOKEN":"never-archive-this-value"}}}
        }),
    );
    fs::remove_file(temp.0.join(".revust/extensions.json")).unwrap();
    let round = temp.0.join("tasks/test/01");
    let _backend = Backend::prepare(
        plan,
        journal,
        Options::default(),
        |_, _| {},
        |_| Duration::ZERO,
    )
    .unwrap();
    let text = fs::read_to_string(round.join("revust/policy.json")).unwrap();
    assert!(!text.contains("never-archive-this-value"));
    let metadata: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(metadata["version"], 1);
    assert_eq!(metadata["profile"], "mixed");
    assert_eq!(metadata["policy"]["values"]["budget"]["max_calls"], 7);
    assert_eq!(
        metadata["policy"]["values"]["executors"]["claude"]["env"]["SYNTHETIC_TOKEN"],
        "<redacted>"
    );
    assert_eq!(
        metadata["policy"]["origins"]["/budget/max_calls"]["layer"],
        "project"
    );
}

#[test]
fn failed_and_truncated_terminal_payloads_are_saved_without_successful_delivery() {
    for (index, suffix) in [(91, "error"), (92, "partial"), (93, "duplicate")] {
        let (temp, plan, journal) = setup_with_policy(
            index,
            json!({"retries":{"find":{"count":0},"synthesis":{"count":0},"verify":{"count":0}}}),
        );
        let payload = json!({"findings":[{"title":"Evidence retained","file":"a.rs","line":1,"severity":"major","confidence":90}]});
        let first = if suffix == "error" {
            json!({"type":"result","is_error":true,"structured_output":payload}).to_string()
        } else {
            protocol("claude", payload)
        };
        let tail = match suffix {
            "partial" => "{\"unfinished\":".to_owned(),
            "duplicate" => json!({"type":"result","structured_output":null}).to_string(),
            _ => String::new(),
        };
        fs::write(
            temp.0.join("claude"),
            format!(
                "#!/bin/sh\n/bin/cat >/dev/null\nprintf '%s\\n%s' '{}' '{}'\n",
                first, tail
            ),
        )
        .unwrap();
        let control = RunControl::new(
            Limits::from_policy(&plan.definition().policy).unwrap(),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
        let backend = Backend::prepare(
            plan,
            journal,
            Options::default(),
            |_, _| {},
            |_| Duration::ZERO,
        )
        .unwrap();
        let result = backend.run(&control).unwrap();
        assert!(!result.successful_reviewers.contains(&"a".into()));
        let snapshot =
            revust_core::runtime::journal::inspect(&temp.0.join("tasks/test/01")).unwrap();
        let attempts: Vec<_> = snapshot
            .attempts
            .iter()
            .filter(|attempt| attempt.id.unit == "a")
            .collect();
        assert!(!attempts.is_empty());
        for attempt in attempts {
            assert!(attempt.result.as_ref().unwrap().is_err());
            let delivery: Value =
                serde_json::from_slice(&fs::read(attempt.directory.join("delivery.json")).unwrap())
                    .unwrap();
            assert_eq!(
                delivery["output"]["findings"][0]["title"],
                "Evidence retained"
            );
            assert!(delivery["protocol_error"].is_string());
            assert!(
                !fs::read(attempt.directory.join("stdout"))
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

#[test]
fn opencode_protocol_diagnostics_reach_saved_attempts_and_user_events() {
    use revust_core::runtime::retry::Failure;
    let long_stderr =
        "error: unknown flag: --standalone\n".to_owned() + &"synthetic diagnostic\n".repeat(512);
    let cases = [
        ("".to_owned(), long_stderr.as_str(), 2, "", Some(Failure::ProcessError), "CLI arguments", true),
        ("error: unexpected argument --format\n".into(), "", 2, "", Some(Failure::ProcessError), "CLI arguments", true),
        ("{\"type\":\"new_result\",\"answer\":{\"findings\":[]}}\n".into(), "", 0, "", Some(Failure::ProcessError), "text events", true),
        ("{\"type\":\"text\",\"part\":{\"content\":\"changed\"}}\n".into(), "", 0, "", Some(Failure::ProcessError), "JSONL", true),
        ("not-jsonl\n".into(), "", 0, "", Some(Failure::ProcessError), "JSONL", true),
        (json!({"type":"text","part":{"messageID":"m","text":"The model did not return JSON"}}).to_string(), "", 0, "", Some(Failure::MissingOutput), "structured result", false),
        (json!({"type":"text","part":{"messageID":"m","text":""}}).to_string(), "", 0, "", Some(Failure::MissingOutput), "structured result", false),
        (protocol("opencode", json!({"findings":[]})) + "{\"unfinished\":", "", 0, "", Some(Failure::ProcessError), "truncated OpenCode", false),
        (protocol("opencode", json!({"findings":[]})) + "{\"type\":\"text\",\"part\":{}}\n", "", 0, "", None, "", false),
        (protocol("opencode", json!({"findings":"invalid"})), "", 0, "", Some(Failure::InvalidOutput), "", false),
        (json!({"type":"error","error":{"type":"provider.auth","message":"Synthetic credential rejected"}}).to_string(), "", 1, "", Some(Failure::ProcessError), "Synthetic credential rejected", false),
        (json!({"type":"error","error":{"type":"provider.model","message":"Synthetic model unavailable"}}).to_string(), "", 1, "", Some(Failure::ProcessError), "Synthetic model unavailable", false),
        ("".into(), "provider: unknown option temperature", 1, "", Some(Failure::NonzeroExit), "exited unsuccessfully", false),
        ("".into(), "Synthetic network unavailable", 1, "", Some(Failure::NonzeroExit), "exited unsuccessfully", false),
        ("{\"type\":\"new_result\"}\n".into(), "", 0, "exec /bin/sleep 30", Some(Failure::HardTimeout), "hard deadline", false),
        ("{\"type\":\"future_event\"}\n".to_owned() + &protocol("opencode", json!({"findings":[]})), "diagnostic", 7, "", None, "", false),
    ];
    for (index, (stdout, stderr, exit, tail, failure, diagnostic, adapter_hint)) in
        cases.into_iter().enumerate()
    {
        let (temp, plan, journal) = setup_with_policy(
            110 + index,
            json!({
                "stages":{"synthesis":{"enabled":false},"verify":{"enabled":false}},
                "retries":{"find":{"count":0}},
                "runtime":{"hard_timeout":"1s"}
            }),
        );
        fs::write(temp.0.join("opencode"), format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'opencode v2.0.21\\n'; exit 0; fi\n/bin/cat >/dev/null\nprintf '%s' '{}'\nprintf '%s' '{}' >&2\n{}\nexit {}\n",
            stdout, stderr, tail, exit,
        )).unwrap();
        let control = RunControl::new(
            Limits::from_policy(&plan.definition().policy).unwrap(),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
        let backend = Backend::prepare(
            plan,
            journal,
            Options::default(),
            |_, _| {},
            |_| Duration::ZERO,
        )
        .unwrap();
        let result = backend.run(&control).unwrap();
        assert_eq!(
            result.successful_reviewers.contains(&"c".into()),
            failure.is_none(),
            "case {index}"
        );
        let round = temp.0.join("tasks/test/01");
        let snapshot = journal::inspect(&round).unwrap();
        let attempt = snapshot.attempts.iter().find(|a| a.id.unit == "c").unwrap();
        let outcome = attempt.result.as_ref().unwrap();
        if let Some(kind) = failure {
            let error = outcome.as_ref().unwrap_err();
            assert_eq!(error.kind, kind, "case {index}: {error:?}");
            assert!(
                error.diagnostic.contains(diagnostic),
                "case {index}: {error:?}"
            );
            assert_eq!(
                error.diagnostic.contains("adapter may need updating"),
                adapter_hint,
                "case {index}: {error:?}"
            );
            let events: Vec<Value> = fs::read_to_string(round.join("events.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert!(
                events.iter().any(|e| e["agent"] == "c"
                    && e["kind"] == "agent_degraded"
                    && e["text"] == error.diagnostic),
                "case {index}: {events:?}"
            );
        } else {
            assert!(outcome.is_ok());
        }
        for (stream, expected) in [("stdout", stdout.as_bytes()), ("stderr", stderr.as_bytes())] {
            let saved = fs::read(attempt.directory.join(stream)).unwrap_or_else(|error| {
                assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
                Vec::new() // Empty streams need not create an archive file.
            });
            assert_eq!(saved, expected, "case {index}: {stream}");
        }
        let delivery: Value =
            serde_json::from_slice(&fs::read(attempt.directory.join("delivery.json")).unwrap())
                .unwrap();
        assert_eq!(
            delivery["protocol_error"]
                .as_str()
                .is_some_and(|error| error.contains("adapter may need updating")),
            adapter_hint,
            "case {index}: {delivery}",
        );
    }
}
