#![cfg(unix)]
use revust::execution::Worker;
use revust_core::{
    archive::Archive,
    report::Report,
    runtime::{Limits, MonotonicClock, RunControl, StopReason, pipeline},
};
use revust_executors::backend::ReviewResult;
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Duration,
};

struct Tree(PathBuf);
impl Tree {
    fn new(id: &str) -> Self {
        let root = std::env::temp_dir().join(format!("revust-worker-{}-{id}", std::process::id()));
        fs::create_dir_all(root.join("test/01/input")).unwrap();
        Self(root)
    }
    fn claim(&self) -> Archive {
        Archive::claim(&self.0, "test", "01").unwrap()
    }
    fn read(&self, name: &str) -> Vec<u8> {
        fs::read(self.0.join("test/01").join(name)).unwrap()
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn control() -> RunControl {
    RunControl::new(
        Limits {
            max_calls: None,
            elapsed: None,
            max_parallel: 1,
            stage_parallel: [1; 3],
        },
        Arc::new(MonotonicClock::default()),
    )
    .unwrap()
}
fn result(state: pipeline::State) -> ReviewResult {
    let mut report = Report::default();
    report.scope.task = "test".into();
    report.scope.run = "01".into();
    report.findings = serde_json::from_str(
        r#"[{"title":"kept","confidence":90},{"title":"filtered","confidence":30}]"#,
    )
    .unwrap();
    report.stats.started_at = "2026-10-17T10:00:00Z".into();
    report.stats.finished_at = "2026-10-17T10:00:01Z".into();
    ReviewResult {
        runtime: pipeline::Report {
            stages: vec![],
            duration_ms: 0,
            state,
            categories: Default::default(),
            successful_reviewers: vec![],
            units: vec![],
            policy_failures: vec![],
            verification_evidence: vec![],
            synthesis_succeeded: false,
        },
        report,
        manifest: r#"{"task":"test","run":"01","finished_at":"2026-10-17T10:00:01Z"}"#.into(),
    }
}

#[test]
fn completion_follows_filtered_durable_report_and_final_marker() {
    let tree = Tree::new("complete");
    let (worker, done) = Worker::start(control(), tree.claim(), 80, |_| {
        Ok(result(pipeline::State::Complete))
    })
    .unwrap();
    let report = done.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].title, "kept");
    assert_eq!(
        tree.read("findings.json"),
        report.json().unwrap().as_bytes()
    );
    assert_eq!(tree.read("report.md"), report.markdown().as_bytes());
    assert!(!tree.read("manifest.json").is_empty());
    let result = worker.join().unwrap();
    assert_eq!(result.exit_code(), 1);
    assert_eq!(tree.read("manifest.json"), result.manifest.as_bytes());
}

#[test]
fn persistence_failure_never_announces_durable_completion() {
    let tree = Tree::new("storage-error");
    fs::write(tree.0.join("test/01/findings.json"), "existing evidence").unwrap();
    let (worker, done) = Worker::start(control(), tree.claim(), 0, |_| {
        Ok(result(pipeline::State::Complete))
    })
    .unwrap();
    assert!(done.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
    assert!(worker.join().is_err());
    assert_eq!(tree.read("findings.json"), b"existing evidence");
    assert!(tree.read("manifest.json").is_empty());
}

#[test]
fn dropping_observation_owner_cancels_and_joins_before_releasing_archive() {
    let tree = Tree::new("cancel");
    let (stopped, observed) = mpsc::channel();
    let (worker, done) = Worker::start(control(), tree.claim(), 0, move |run| {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while run.check_running().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(run.check_running(), Some(StopReason::Cancelled));
        stopped.send(()).unwrap();
        Ok(result(pipeline::State::Stopped(StopReason::Cancelled)))
    })
    .unwrap();
    drop(worker);
    observed.try_recv().unwrap();
    let report = done.try_recv().unwrap().unwrap();
    assert_eq!(report.exit_code(), 2);
    assert!(!tree.read("findings.json").is_empty());
    assert!(!tree.read("manifest.json").is_empty());
}

#[test]
fn execution_error_and_panic_are_reported_to_both_observer_and_owner() {
    for panic in [false, true] {
        let tree = Tree::new(if panic { "panic" } else { "error" });
        let (worker, done) = Worker::start(control(), tree.claim(), 0, move |_| {
            if panic {
                panic!("controlled worker panic");
            }
            Err("controlled execution error".into())
        })
        .unwrap();
        let error = done
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(
            error,
            if panic {
                "review worker panicked"
            } else {
                "controlled execution error"
            }
        );
        assert_eq!(worker.join().err().unwrap(), error);
        assert!(tree.read("manifest.json").is_empty());
    }
}

#[test]
fn incomplete_runs_preserve_live_countdown_and_durable_browsable_outcomes() {
    for (id, state, expected) in [
        (
            "failed",
            pipeline::State::Failed,
            serde_json::json!("failed"),
        ),
        (
            "policy-failed",
            pipeline::State::Complete,
            serde_json::json!("complete"),
        ),
        (
            "stopped",
            pipeline::State::Stopped(StopReason::Deadline),
            serde_json::json!({"stopped":"deadline"}),
        ),
    ] {
        let tree = Tree::new(id);
        let archive = tree.claim();
        archive.write("events.jsonl".as_ref(), b"").unwrap();
        let (worker, done) = Worker::start(control(), archive, 80, move |_| {
            let mut result = result(state);
            if id == "policy-failed" {
                result
                    .runtime
                    .policy_failures
                    .push("synthetic strict policy failure".into());
            }
            Ok(result)
        })
        .unwrap();
        assert_eq!(worker.join().unwrap().exit_code(), 2);
        let (_events, rx) = mpsc::channel();
        let mut updates = revust_tui::updates::Updates::new(rx, done);
        let mut model = revust_tui::Model::new(vec![]).with_auto_exit(Duration::from_secs(1));
        updates.poll(&mut model).unwrap();
        assert!(updates.complete());
        assert_eq!(model.report.as_ref().unwrap().exit_code(), 2);
        assert_eq!(model.exit_remaining(), Duration::from_secs(1));
        assert_eq!(model.tick(), revust_tui::navigation::Action::Quit);
        let restored = revust::archive_view::load(&tree.0.join("test/01")).unwrap();
        assert_eq!(restored.model.report.as_ref().unwrap().exit_code(), 2);
        assert!(restored.model.exit_remaining().is_zero());
        let manifest: serde_json::Value =
            serde_json::from_slice(&tree.read("manifest.json")).unwrap();
        assert_eq!(manifest["outcome"]["state"], expected);
        let report: serde_json::Value =
            serde_json::from_slice(&tree.read("findings.json")).unwrap();
        assert_eq!(report["outcome"], manifest["outcome"]);
        assert_eq!(report["findings"].as_array().unwrap().len(), 1);
        assert!(
            String::from_utf8(tree.read("report.md"))
                .unwrap()
                .contains("review incomplete")
        );
    }
}
