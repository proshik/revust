#![cfg(unix)]
use revust_core::archive::Archive;
use std::{
    fs,
    os::unix::fs::symlink,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "revust-archive-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(p.join("tasks/test/01/input")).unwrap();
        fs::write(p.join("tasks/test/01/input/scope.md"), "preserved scope").unwrap();
        Self(p)
    }
    fn tasks(&self) -> PathBuf {
        self.0.join("tasks")
    }
    fn round(&self) -> PathBuf {
        self.tasks().join("test/01")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn claim_is_exclusive_completed_and_partial_rounds_are_not_reused() {
    let t = Temp::new();
    let mut archive = Archive::claim(&t.tasks(), "test", "01").unwrap();
    assert!(Archive::claim(&t.tasks(), "test", "01").is_err());
    archive
        .write("stages/1-found.json".as_ref(), b"{}\n")
        .unwrap();
    assert!(
        archive
            .write("stages/1-found.json".as_ref(), b"overwrite")
            .is_err()
    );
    archive
        .finish(b"{\"task\":\"test\",\"run\":\"01\"}\n")
        .unwrap();
    assert!(archive.write("late".as_ref(), b"late").is_err());
    drop(archive);
    assert!(Archive::claim(&t.tasks(), "test", "01").is_err());
    assert_eq!(
        fs::read(t.round().join("input/scope.md")).unwrap(),
        b"preserved scope"
    );
    let t = Temp::new();
    let archive = Archive::claim(&t.tasks(), "test", "01").unwrap();
    drop(archive);
    let archive = Archive::claim(&t.tasks(), "test", "01").unwrap();
    archive
        .write("events.jsonl".as_ref(), b"partial\n")
        .unwrap();
    drop(archive);
    assert!(Archive::claim(&t.tasks(), "test", "01").is_err());
}
#[test]
fn contained_task_alias_is_allowed_but_round_input_and_escape_aliases_are_refused() {
    let t = Temp::new();
    fs::create_dir(t.tasks().join("nested")).unwrap();
    symlink("nested/../test", t.tasks().join("alias")).unwrap();
    drop(Archive::claim(&t.tasks(), "alias", "01").unwrap());
    symlink("01", t.tasks().join("test/02")).unwrap();
    assert!(Archive::claim(&t.tasks(), "test", "02").is_err());
    fs::create_dir_all(t.0.join("outside/01/input")).unwrap();
    symlink("../outside", t.tasks().join("escape")).unwrap();
    assert!(Archive::claim(&t.tasks(), "escape", "01").is_err());
    assert!(!t.0.join("outside/01/manifest.json").exists());
    fs::rename(t.round().join("input"), t.round().join("saved-input")).unwrap();
    symlink("saved-input", t.round().join("input")).unwrap();
    assert!(Archive::claim(&t.tasks(), "test", "01").is_err());
}
#[test]
fn writes_use_owned_directory_and_cannot_follow_artifact_escapes() {
    let t = Temp::new();
    let archive = Archive::claim(&t.tasks(), "test", "01").unwrap();
    fs::create_dir(t.0.join("outside")).unwrap();
    symlink(t.0.join("outside"), t.round().join("escape")).unwrap();
    for path in [
        "../outside/file",
        "/tmp/unowned",
        "escape/file",
        "input/scope.md",
    ] {
        assert!(archive.write(path.as_ref(), b"bad").is_err(), "{path}");
    }
    let moved = t.0.join("owned");
    fs::rename(t.round(), &moved).unwrap();
    symlink(t.0.join("outside"), t.round()).unwrap();
    archive
        .write("report.md".as_ref(), b"owned output")
        .unwrap();
    assert_eq!(fs::read(moved.join("report.md")).unwrap(), b"owned output");
    assert!(!t.0.join("outside/report.md").exists());
}

#[test]
fn manifest_aliases_and_special_files_are_refused_without_writing_the_target() {
    use nix::{sys::stat::Mode, unistd::mkfifo};
    for kind in ["symlink", "hardlink", "fifo", "directory"] {
        let t = Temp::new();
        let marker = t.round().join("manifest.json");
        let outside = t.0.join("outside-marker");
        fs::write(&outside, []).unwrap();
        match kind {
            "symlink" => symlink(&outside, &marker).unwrap(),
            "hardlink" => fs::hard_link(&outside, &marker).unwrap(),
            "fifo" => mkfifo(&marker, Mode::from_bits_truncate(0o600)).unwrap(),
            _ => fs::create_dir(&marker).unwrap(),
        }
        assert!(Archive::claim(&t.tasks(), "test", "01").is_err(), "{kind}");
        assert_eq!(fs::read(outside).unwrap(), b"");
        assert_eq!(
            fs::read(t.round().join("input/scope.md")).unwrap(),
            b"preserved scope"
        );
    }
}

#[test]
fn saved_reader_roundtrips_owned_report_without_external_profiles_or_writes() {
    use revust_core::{archive::reader, report::Report};
    let t = Temp::new();
    let report = Report::decode(r#"{"scope":{"task":"test","run":"01"},"findings":[{"id":"saved-1","title":"Durable observation","severity":"major","sources":["observer"],"body":"Saved & <literal> content"}],"open_questions":[{"id":"question-1"}],"stats":{"finished_at":"2026-10-10T10:00:00Z"}}"#).unwrap();
    let mut archive = Archive::claim(&t.tasks(), "test", "01").unwrap();
    archive.save_report(&report).unwrap();
    archive.write("events.jsonl".as_ref(), b"").unwrap();
    archive
        .finish(br#"{"task":"test","run":"01","finished_at":"2026-10-10T10:00:00Z"}"#)
        .unwrap();
    drop(archive);
    let names = [
        "input/scope.md",
        "manifest.json",
        "findings.json",
        "report.md",
        "events.jsonl",
    ];
    let before: Vec<_> = names
        .iter()
        .map(|name| fs::read(t.round().join(name)).unwrap())
        .collect();
    let restored = reader::load(&t.round()).unwrap();
    assert_eq!(restored.report.findings[0].id, "saved-1");
    assert_eq!(restored.report.findings[0].sources, ["observer"]);
    assert_eq!(
        restored.report.findings[0].body,
        "Saved & <literal> content"
    );
    assert_eq!(restored.report.open_questions[0].id, "question-1");
    for (name, bytes) in names.iter().zip(before) {
        assert_eq!(fs::read(t.round().join(name)).unwrap(), bytes);
    }
}

#[test]
fn journal_uses_the_archive_claim_even_after_round_path_replacement() {
    use revust_core::runtime::{
        Attempt, Stage,
        journal::{Artifact, Identity, Stream},
        worker::Event,
    };
    let t = Temp::new();
    let archive = Archive::claim(&t.tasks(), "test", "01").unwrap();
    let identity = Identity {
        task: "test".into(),
        run: "01".into(),
        request: "synthetic".into(),
    };
    assert!(
        archive
            .journal(Identity {
                task: "wrong".into(),
                ..identity.clone()
            })
            .is_err()
    );
    assert!(!t.round().join("revust").exists());
    let owned = t.0.join("owned");
    fs::rename(t.round(), &owned).unwrap();
    let outside = t.0.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, t.round()).unwrap();
    let journal = archive.journal(identity).unwrap();
    journal.save_calibration(b"frozen calibration").unwrap();
    assert_eq!(
        fs::read(owned.join("prompts/input-profile.md")).unwrap(),
        b"frozen calibration"
    );
    assert!(!outside.join("prompts").exists());
    let attempt = Attempt {
        stage: Stage::Find,
        unit: "a".into(),
        number: 0,
    };
    journal.record(Event::Started(&attempt)).unwrap();
    journal
        .append(&attempt, Stream::Stdout, b"owned stream")
        .unwrap();
    journal
        .save(&attempt, Artifact::Request, b"owned request")
        .unwrap();
    assert_eq!(
        fs::read(owned.join("revust/attempts/0/stdout")).unwrap(),
        b"owned stream"
    );
    assert_eq!(
        fs::read(owned.join("revust/attempts/0/request.json")).unwrap(),
        b"owned request"
    );
    assert!(!outside.join("revust").exists());
}

fn final_report() -> revust_core::report::Report {
    let mut report = revust_core::report::Report::decode(
        r#"{
        "findings":[{"id":"one-1","title":"Owned artifact","sources":["one"]}],
        "open_questions":[{"id":"two-1","body":"Unresolved observation"}],
        "pre_existing":[{"id":"three-1"}],"immaterial":[{"id":"four-1"}],
        "stats":{"started_at":"2026-10-14T10:00:00Z","finished_at":"2026-10-14T10:00:01Z"}
    }"#,
    )
    .unwrap();
    report.scope.task = "test".into();
    report.scope.run = "01".into();
    report
}

#[test]
fn final_report_is_viewable_and_stays_with_the_claimed_directory() {
    let t = Temp::new();
    let mut archive = Archive::claim(&t.tasks(), "test", "01").unwrap();
    let owned = t.0.join("owned");
    fs::rename(t.round(), &owned).unwrap();
    fs::create_dir(t.round()).unwrap();
    let report = final_report();
    archive.write("events.jsonl".as_ref(), b"").unwrap();
    archive.save_report(&report).unwrap();
    assert!(fs::read(owned.join("manifest.json")).unwrap().is_empty());
    assert!(revust_core::archive::reader::load(&owned).is_err());
    // Synthetic metadata isolates report persistence from manifest assembly.
    let manifest =
        serde_json::json!({"task":"test","run":"01","finished_at":report.stats.finished_at});
    archive.finish(manifest.to_string().as_bytes()).unwrap();
    assert!(
        revust_core::archive::reader::load(&owned)
            .err()
            .unwrap()
            .contains("active")
    );
    drop(archive);
    let reopened = revust_core::archive::reader::load(&owned).unwrap();
    assert_eq!(reopened.report.json().unwrap(), report.json().unwrap());
    assert_eq!(
        fs::read_to_string(owned.join("report.md")).unwrap(),
        report.markdown()
    );
    assert_eq!(fs::read_dir(t.round()).unwrap().count(), 0);
}

#[test]
fn final_report_failure_prevents_a_completion_marker() {
    for failure in ["identity", "encoding", "markdown", "json"] {
        let t = Temp::new();
        let mut archive = Archive::claim(&t.tasks(), "test", "01").unwrap();
        let mut report = final_report();
        match failure {
            "identity" => report.scope.run = "other".into(),
            "encoding" => report.stats.finished_at = "invalid".into(),
            "markdown" => fs::write(t.round().join("report.md"), b"existing").unwrap(),
            "json" => fs::write(t.round().join("findings.json"), b"existing").unwrap(),
            _ => unreachable!(),
        }
        assert!(archive.save_report(&report).is_err(), "{failure}");
        assert!(archive.finish(b"finished").is_err(), "{failure}");
        assert!(
            fs::read(t.round().join("manifest.json"))
                .unwrap()
                .is_empty()
        );
        if failure == "json" || failure == "markdown" {
            let name = if failure == "json" {
                "findings.json"
            } else {
                "report.md"
            };
            assert_eq!(fs::read(t.round().join(name)).unwrap(), b"existing");
        } else {
            assert!(!t.round().join("report.md").exists());
            assert!(!t.round().join("findings.json").exists());
        }
    }
}

#[test]
fn concurrent_claims_admit_one_owner_and_preserve_unmarked_outputs() {
    use std::sync::{Arc, Barrier};
    let t = Temp::new();
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let barrier = barrier.clone();
            let tasks = t.tasks();
            std::thread::spawn(move || {
                barrier.wait();
                Archive::claim(&tasks, "test", "01").ok()
            })
        })
        .collect();
    let claims: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    drop(claims);
    let t = Temp::new();
    fs::write(t.round().join("report.md"), "unmarked evidence").unwrap();
    let archive = Archive::claim(&t.tasks(), "test", "01").unwrap();
    assert!(archive.write("report.md".as_ref(), b"replacement").is_err());
    assert_eq!(
        fs::read(t.round().join("report.md")).unwrap(),
        b"unmarked evidence"
    );
    assert_eq!(fs::read(t.round().join("manifest.json")).unwrap(), b"");
}
