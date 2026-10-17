#![cfg(unix)]
use revust::archive_view;
use revust_tui::{
    State,
    navigation::{Action, Pane},
};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

struct Archive(PathBuf);
impl Archive {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "revust-view-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("input")).unwrap();
        fs::write(root.join("input/scope.md"), "saved scope").unwrap();
        fs::write(root.join("manifest.json"), json!({"task":"saved","run":"01","profile":"removed-profile","finished_at":"2026-10-10T12:00:03Z","roster":[{"name":"a","executor":"opencode","model":"saved/model","effort":"high","color":"\u{1b}[36m","lenses":["removed-lens"]}]}).to_string()).unwrap();
        fs::write(root.join("findings.json"), json!({"scope":{"task":"saved","run":"01"},"findings":[{"title":"saved finding","severity":"major","file":"src/main.rs"}]}).to_string()).unwrap();
        let events = [
            json!({"kind":"agent_started","agent":"a","at":"2026-10-10T12:00:00Z"}),
            json!({"kind":"agent_done","agent":"a","text":"finished","at":"2026-10-10T12:00:01Z"}),
            json!({"kind":"future_kind","agent":"future","text":"not a log","at":"2026-10-10T12:00:02Z"}),
            json!({"kind":"future_kind","agent":"a","text":"must not replace status","at":"2026-10-10T12:00:03Z"}),
        ];
        fs::write(
            root.join("events.jsonl"),
            events
                .iter()
                .map(|e| e.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        Self(root)
    }
    fn bytes(&self) -> Vec<Vec<u8>> {
        [
            "manifest.json",
            "findings.json",
            "events.jsonl",
            "input/scope.md",
        ]
        .iter()
        .map(|p| fs::read(self.0.join(p)).unwrap())
        .collect()
    }
}
impl Drop for Archive {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn restores_saved_observation_and_future_events_without_countdown_or_archive_writes() {
    let archive = Archive::new();
    let before = archive.bytes();
    let view = archive_view::load(&archive.0).unwrap();
    assert_eq!(
        (
            view.status.task.as_str(),
            view.status.run.as_str(),
            view.status.profile.as_str()
        ),
        ("saved", "01", "removed-profile")
    );
    let mut model = view.model.with_auto_exit(Duration::from_secs(1));
    assert_eq!(model.stage, "archive");
    assert_eq!(model.pane(), Pane::Combined);
    assert_eq!(model.found.major, 1);
    assert_eq!(model.inputs()[0].content, "saved scope");
    let agent = &model.agents[0];
    assert_eq!(agent.roster.as_ref().unwrap().runner.executor, "opencode");
    assert_eq!(agent.roster.as_ref().unwrap().runner.model, "saved/model");
    assert_eq!(agent.state, State::Done);
    assert!(agent.last.contains("done") && agent.last.contains("finished"));
    assert_eq!(
        agent.updated.unwrap().to_rfc3339(),
        "2026-10-10T12:00:03+00:00"
    );
    assert_eq!(model.agents[1].name, "future");
    assert_eq!(model.agents[1].state, State::Running);
    assert!(model.agents[1].lines.is_empty());
    assert_eq!(model.combined.len(), 2);
    assert_eq!(model.tick(), Action::None);
    assert_eq!(model.exit_remaining(), Duration::ZERO);
    assert_eq!(archive.bytes(), before);
}

#[test]
fn invalid_archive_fails_before_constructing_a_view() {
    let archive = Archive::new();
    fs::write(archive.0.join("findings.json"), "{broken").unwrap();
    let before = archive.bytes();
    assert!(
        archive_view::load(&archive.0)
            .err()
            .unwrap()
            .contains("findings.json")
    );
    assert_eq!(archive.bytes(), before);
}

#[test]
fn saved_manifest_stage_settings_restore_without_archive_writes() {
    let archive = Archive::new();
    let path = archive.0.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["stages"] = json!([
        {"name":"synthesis","executor":"codex","model":"saved/synth","effort":"high"},
        {"name":"verify","executor":"opencode","model":"saved/verify"}
    ]);
    fs::write(path, manifest.to_string()).unwrap();
    let path = archive.0.join("events.jsonl");
    let mut events = fs::read_to_string(&path).unwrap();
    for name in ["synthesis", "verify root"] {
        events.push_str(
            &(json!({"kind":"agent_started","agent":name,"at":"2026-10-10T12:00:03Z"}).to_string()
                + "\n"),
        );
    }
    fs::write(path, events).unwrap();
    let before = archive.bytes();
    let view = archive_view::load(&archive.0).unwrap();
    let stages = &view.model.report.as_ref().unwrap().stats.stages;
    assert_eq!(stages.len(), 2);
    assert_eq!(
        (
            stages[0].executor.as_str(),
            stages[0].model.as_str(),
            stages[0].effort.as_str()
        ),
        ("codex", "saved/synth", "high")
    );
    assert_eq!(
        (stages[1].executor.as_str(), stages[1].model.as_str()),
        ("opencode", "saved/verify")
    );
    assert_eq!(archive.bytes(), before);
}

#[test]
fn older_archive_restores_roster_from_observed_source_identity() {
    let archive = Archive::new();
    let path = archive.0.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest.as_object_mut().unwrap().remove("roster");
    manifest["agents"] = json!([{"name":"a","executor":"codex","actual_model":"observed-model","requested_model":"different-request","lenses":["saved-lens"]}]);
    fs::write(path, manifest.to_string()).unwrap();
    let view = archive_view::load(&archive.0).unwrap();
    let agent = view.model.agents[0].roster.as_ref().unwrap();
    assert_eq!(agent.runner.executor, "codex");
    assert_eq!(agent.runner.model, "observed-model");
    assert_eq!(agent.lenses, ["saved-lens"]);
    assert!(!agent.color.is_empty());
}
