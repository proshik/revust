use revust_core::runtime::{
    Attempt, Stage,
    data::{Categories, Finding},
    journal::{self, Identity, Journal, Stream},
    pipeline::{Report, State},
    retry::Failure,
    worker::{AttemptFailure, Completed, Event},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
struct Round(PathBuf);
impl Round {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "revust-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Round {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn identity() -> Identity {
    Identity {
        task: "test".into(),
        run: "1".into(),
        request: "request-1".into(),
    }
}
fn attempt(number: u32) -> Attempt {
    Attempt {
        stage: Stage::Find,
        unit: "../../reviewer/Ю".into(),
        number,
    }
}
fn report() -> Report {
    Report {
        stages: vec![],
        duration_ms: 0,
        state: State::Complete,
        categories: Categories::default(),
        successful_reviewers: vec!["reviewer".into()],
        units: vec![],
        policy_failures: vec![],
        verification_evidence: vec![],
        synthesis_succeeded: true,
    }
}
#[test]
fn retries_keep_streams_and_results_separate_and_reopening_never_recreates_run() {
    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    for number in 0..2 {
        let id = attempt(number);
        journal.record(Event::Started(&id)).unwrap();
        journal
            .append(&id, Stream::Stdout, format!("answer {number}").as_bytes())
            .unwrap();
        journal
            .append(
                &id,
                Stream::Stderr,
                format!("diagnostic {number}").as_bytes(),
            )
            .unwrap();
        let done = Completed {
            id: id.clone(),
            result: if number == 0 {
                Err(AttemptFailure {
                    kind: Failure::MissingOutput,
                    diagnostic: "no answer".into(),
                })
            } else {
                Ok(Categories {
                    findings: vec![Finding {
                        id: "reviewer-1".into(),
                        ..Finding::default()
                    }],
                    ..Categories::default()
                })
            },
        };
        journal.record(Event::Finished(&done)).unwrap();
        assert!(journal.append(&id, Stream::Stdout, b"late").is_err());
    }
    journal.finish(&report()).unwrap();
    drop(journal);
    let before = std::fs::read(round.0.join("revust/events.jsonl")).unwrap();
    let snapshot = journal::inspect(&round.0).unwrap();
    assert_eq!(snapshot.identity, identity());
    assert_eq!(snapshot.report.unwrap().state, State::Complete);
    assert!(!snapshot.truncated);
    assert_eq!(snapshot.attempts.len(), 2);
    for (n, entry) in snapshot.attempts.iter().enumerate() {
        assert!(entry.directory.starts_with(round.0.join("revust/attempts")));
        assert_eq!(
            std::fs::read_to_string(entry.directory.join("stdout")).unwrap(),
            format!("answer {n}")
        );
        assert_eq!(
            std::fs::read_to_string(entry.directory.join("stderr")).unwrap(),
            format!("diagnostic {n}")
        );
    }
    assert_eq!(
        snapshot.attempts[0]
            .result
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .diagnostic,
        "no answer"
    );
    assert!(Journal::create(&round.0, identity()).is_err());
    assert_eq!(
        std::fs::read(round.0.join("revust/events.jsonl")).unwrap(),
        before
    );
}
#[test]
fn interruption_and_torn_tail_are_observed_without_repair_or_clean_success() {
    use std::io::Write;
    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    journal.record(Event::Started(&attempt(0))).unwrap();
    journal
        .append(&attempt(0), Stream::Stderr, b"partial")
        .unwrap();
    drop(journal);
    let path = round.0.join("revust/events.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"unfinished\":")
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let snapshot = journal::inspect(&round.0).unwrap();
    assert!(snapshot.truncated);
    assert!(snapshot.report.is_none());
    assert_eq!(snapshot.attempts.len(), 1);
    assert!(snapshot.attempts[0].result.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(Journal::create(&round.0, identity()).is_err());
    std::fs::write(&path, b"not json\n").unwrap();
    assert!(journal::inspect(&round.0).is_err());
}
#[test]
fn concurrent_creators_have_one_owner_and_duplicate_attempts_are_rejected() {
    let round = Round::new();
    let wins = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|s| {
        for _ in 0..8 {
            let wins = wins.clone();
            let root = &round.0;
            s.spawn(move || {
                if Journal::create(root, identity()).is_ok() {
                    wins.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    });
    assert_eq!(wins.load(Ordering::SeqCst), 1);
    let round = Round::new();
    let j = Journal::create(&round.0, identity()).unwrap();
    j.record(Event::Started(&attempt(0))).unwrap();
    assert!(j.record(Event::Started(&attempt(0))).is_err());
    assert!(
        j.record(Event::Finished(&Completed {
            id: attempt(1),
            result: Ok(Categories::default())
        }))
        .is_err()
    );
    assert!(j.finish(&report()).is_err());
}

#[test]
fn terminal_journal_rejects_writes_and_damaged_or_unknown_records_do_not_claim_success() {
    use std::io::Write;
    let round = Round::new();
    let j = Journal::create(&round.0, identity()).unwrap();
    j.finish(&report()).unwrap();
    assert!(j.record(Event::Started(&attempt(0))).is_err());
    assert!(j.finish(&report()).is_err());
    let path = round.0.join("revust/events.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"torn")
        .unwrap();
    let view = journal::inspect(&round.0).unwrap();
    assert!(view.truncated);
    assert!(view.report.is_none());
    std::fs::write(&path, b"\n").unwrap();
    assert!(journal::inspect(&round.0).is_err());
    let path = round.0.join("revust/run.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["version"] = serde_json::json!(2);
    std::fs::write(path, value.to_string()).unwrap();
    assert!(journal::inspect(&round.0).is_err());
}

#[test]
fn stream_storage_failure_prevents_later_events_and_terminal_success() {
    let round = Round::new();
    let j = Journal::create(&round.0, identity()).unwrap();
    let observer = j.subscribe().unwrap();
    j.record(Event::Started(&attempt(0))).unwrap();
    // Controlled filesystem fault: a stream file cannot replace a directory.
    std::fs::create_dir(round.0.join("revust/attempts/0/stdout")).unwrap();
    assert!(j.append(&attempt(0), Stream::Stdout, b"answer").is_err());
    assert!(matches!(
        observer.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    ));
    assert!(
        j.record(Event::Finished(&Completed {
            id: attempt(0),
            result: Ok(Categories::default())
        }))
        .is_err()
    );
    assert!(j.finish(&report()).is_err());
    let snapshot = journal::inspect(&round.0).unwrap();
    assert!(snapshot.report.is_none());
    assert!(snapshot.attempts[0].result.is_none());
}

#[test]
fn request_and_delivery_artifacts_require_a_live_attempt_and_never_overwrite() {
    use journal::Artifact;
    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    let id = attempt(0);
    assert!(
        journal
            .save(&id, Artifact::Request, b"before start")
            .is_err()
    );
    journal.record(Event::Started(&id)).unwrap();
    journal
        .save(&id, Artifact::Request, b"synthetic request")
        .unwrap();
    journal
        .save(&id, Artifact::Delivery, b"synthetic delivery")
        .unwrap();
    let path = journal.attempt_directory(&id).unwrap();
    assert_eq!(path, round.0.join("revust/attempts/0"));
    assert_eq!(
        std::fs::read(path.join("request.json")).unwrap(),
        b"synthetic request"
    );
    assert!(
        journal
            .save(&id, Artifact::Request, b"replacement")
            .is_err()
    );
    assert_eq!(
        std::fs::read(path.join("request.json")).unwrap(),
        b"synthetic request"
    );
}

#[cfg(unix)]
#[test]
fn journal_keeps_ownership_when_round_attempt_or_stream_paths_are_replaced() {
    use journal::Artifact;
    use std::{fs, os::unix::fs::symlink};
    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    let id = attempt(0);
    journal.record(Event::Started(&id)).unwrap();
    journal.append(&id, Stream::Stdout, b"first ").unwrap();
    let dir = round.0.join("revust/attempts/0");
    fs::rename(dir.join("stdout"), dir.join("owned-stdout")).unwrap();
    let outside = round.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("stdout"), b"outside").unwrap();
    symlink(outside.join("stdout"), dir.join("stdout")).unwrap();
    journal.append(&id, Stream::Stdout, b"second").unwrap();
    assert_eq!(fs::read(dir.join("owned-stdout")).unwrap(), b"first second");
    assert_eq!(fs::read(outside.join("stdout")).unwrap(), b"outside");
    let owned = round.0.join("owned-attempt");
    fs::rename(&dir, &owned).unwrap();
    symlink(&outside, &dir).unwrap();
    journal
        .save(&id, Artifact::Delivery, b"owned delivery")
        .unwrap();
    assert_eq!(
        fs::read(owned.join("delivery.json")).unwrap(),
        b"owned delivery"
    );
    assert!(!outside.join("delivery.json").exists());
    let root = round.0.join("revust");
    let moved = round.0.join("owned-journal");
    fs::rename(&root, &moved).unwrap();
    symlink(&outside, &root).unwrap();
    journal.record(Event::Started(&attempt(1))).unwrap();
    assert!(moved.join("attempts/1").is_dir());
    assert!(!outside.join("attempts").exists());
}

#[test]
fn stage_snapshots_stay_on_owned_directory_and_cannot_be_replaced() {
    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    journal.save_stage(Stage::Find, b"first").unwrap();
    std::fs::rename(round.0.join("stages"), round.0.join("owned-stages")).unwrap();
    std::fs::create_dir(round.0.join("stages")).unwrap();
    journal.save_stage(Stage::Synthesis, b"second").unwrap();
    assert_eq!(
        std::fs::read(round.0.join("owned-stages/2-synthesized.json")).unwrap(),
        b"second"
    );
    assert_eq!(
        std::fs::read_dir(round.0.join("stages")).unwrap().count(),
        0
    );
    assert!(journal.save_stage(Stage::Find, b"overwrite").is_err());
    assert_eq!(
        std::fs::read(round.0.join("owned-stages/1-found.json")).unwrap(),
        b"first"
    );
    assert!(journal.save_stage(Stage::Verify, b"later").is_err());
    assert!(journal.finish(&report()).is_err());
}

#[test]
fn composed_prompts_are_owned_once_and_retries_cannot_replace_them() {
    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    journal.save_calibration(b"calibration").unwrap();
    std::fs::rename(round.0.join("prompts"), round.0.join("owned-prompts")).unwrap();
    std::fs::create_dir(round.0.join("prompts")).unwrap();
    journal
        .save_prompt(Stage::Find, "synthesis", b"finder")
        .unwrap();
    journal
        .save_prompt(Stage::Synthesis, "synthesis", b"merge")
        .unwrap();
    journal
        .save_prompt(Stage::Verify, "src-lib", b"verify")
        .unwrap();
    journal
        .save_prompt(Stage::Find, "synthesis", b"finder")
        .unwrap();
    assert_eq!(
        std::fs::read(round.0.join("owned-prompts/agents/synthesis.md")).unwrap(),
        b"finder"
    );
    assert_eq!(
        std::fs::read(round.0.join("owned-prompts/stages/synthesis.md")).unwrap(),
        b"merge"
    );
    assert_eq!(
        std::fs::read(round.0.join("owned-prompts/stages/verify-src-lib.md")).unwrap(),
        b"verify"
    );
    assert_eq!(
        std::fs::read_dir(round.0.join("prompts")).unwrap().count(),
        0
    );
    assert!(
        journal
            .save_prompt(Stage::Find, "synthesis", b"changed")
            .is_err()
    );
    assert!(journal.finish(&report()).is_err());
    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    assert!(
        journal
            .save_prompt(Stage::Find, "../escape", b"bad")
            .is_err()
    );
    assert!(!round.0.join("escape.md").exists());
}

#[test]
fn presentation_events_keep_owned_stream_and_fail_closed_on_invalid_encoding() {
    use revust_core::runtime::events::Event;
    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    let observer = journal.subscribe().unwrap();
    let path = round.0.join("events.jsonl");
    let owned = round.0.join("owned-events.jsonl");
    std::fs::rename(&path, &owned).unwrap();
    std::fs::write(&path, b"replacement").unwrap();
    let mut event = Event::stage("find");
    event.at = "2026-10-10T10:00:00+00:00".into();
    event.text = "literal <&>\nsecond line".into();
    journal.publish(event).unwrap();
    let bytes = std::fs::read_to_string(&owned).unwrap();
    assert_eq!(bytes.lines().count(), 1);
    let decoded: serde_json::Value = serde_json::from_str(&bytes).unwrap();
    assert_eq!(
        serde_json::to_value(observer.try_recv().unwrap()).unwrap(),
        decoded
    );
    assert!(journal.subscribe().is_err());
    assert_eq!(decoded["at"], "2026-10-10T10:00:00Z");
    assert_eq!(decoded["text"], "literal <&>\nsecond line");
    assert!(decoded.get("agent").is_none());
    assert!(decoded.get("findings").is_none());
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    let mut invalid = Event::stage("verify");
    invalid.at = "invalid".into();
    assert!(journal.publish(invalid).is_err());
    assert!(matches!(
        observer.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    ));
    assert!(journal.publish(Event::stage("synthesis")).is_err());
    assert!(journal.finish(&report()).is_err());
    assert_eq!(std::fs::read_to_string(&owned).unwrap(), bytes);
    let round = Round::new();
    std::fs::write(round.0.join("events.jsonl"), b"existing evidence").unwrap();
    assert!(Journal::create(&round.0, identity()).is_err());
    assert_eq!(
        std::fs::read(round.0.join("events.jsonl")).unwrap(),
        b"existing evidence"
    );
}

#[test]
fn extension_profile_preserves_raw_bytes_and_refuses_overwrite_or_late_write() {
    use revust_core::plan::ProfileSnapshot;
    use std::os::unix::fs::symlink;
    let snapshot = ProfileSnapshot {
        source: PathBuf::from("/synthetic/extension-profiles/example.md"),
        content: "abc".into(),
    };
    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    journal
        .save_extension_profile("example", &snapshot)
        .unwrap();
    let path = round.0.join("revust/extension-profile.json");
    let bytes = std::fs::read(&path).unwrap();
    let metadata: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(metadata["content"], "abc");
    assert_eq!(
        metadata["sha256"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(
        journal
            .save_extension_profile("example", &snapshot)
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(journal.record(Event::Started(&attempt(0))).is_err());
    assert!(journal.finish(&report()).is_err());

    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    let target = round.0.join("untouched");
    std::fs::write(&target, "keep").unwrap();
    symlink(&target, round.0.join("revust/extension-profile.json")).unwrap();
    assert!(
        journal
            .save_extension_profile("example", &snapshot)
            .is_err()
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    assert!(journal.record(Event::Started(&attempt(0))).is_err());

    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    journal.record(Event::Started(&attempt(0))).unwrap();
    assert!(
        journal
            .save_extension_profile("example", &snapshot)
            .is_err()
    );
    assert!(!round.0.join("revust/extension-profile.json").exists());
}

#[test]
fn legacy_agent_tees_keep_owned_files_and_refuse_collisions() {
    use std::{fs, os::unix::fs::symlink};
    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    let id = Attempt {
        stage: Stage::Find,
        unit: "reviewer".into(),
        number: 0,
    };
    journal.record(Event::Started(&id)).unwrap();
    journal.open_agent_stream(&id, "codex").unwrap();
    journal.append(&id, Stream::Stdout, b"first").unwrap();
    let agents = round.0.join("agents");
    fs::rename(&agents, round.0.join("owned-agents")).unwrap();
    fs::create_dir(&agents).unwrap();
    fs::write(agents.join("reviewer.log"), b"replacement").unwrap();
    journal.append(&id, Stream::Stdout, b" second").unwrap();
    journal.append(&id, Stream::Stderr, b"diagnostic").unwrap();
    assert_eq!(
        fs::read(round.0.join("owned-agents/reviewer.log")).unwrap(),
        b"first second"
    );
    assert_eq!(
        fs::read(agents.join("reviewer.log")).unwrap(),
        b"replacement"
    );
    // A retry cannot follow a pre-existing symlink, even inside the owned directory.
    let retry = Attempt { number: 1, ..id };
    journal.record(Event::Started(&retry)).unwrap();
    symlink(
        agents.join("reviewer.log"),
        round.0.join("owned-agents/reviewer.retry.log"),
    )
    .unwrap();
    assert!(journal.open_agent_stream(&retry, "codex").is_err());
    assert!(journal.finish(&report()).is_err());
    assert_eq!(
        fs::read(agents.join("reviewer.log")).unwrap(),
        b"replacement"
    );
}

#[test]
fn physical_prompt_destination_cannot_acquire_a_second_unit_owner() {
    let round = Round::new();
    let journal = Journal::create(&round.0, identity()).unwrap();
    journal
        .save_prompt(Stage::Synthesis, "first-unit", b"original evidence")
        .unwrap();
    assert!(
        journal
            .save_prompt(Stage::Synthesis, "second-unit", b"original evidence")
            .is_err()
    );
    assert_eq!(
        std::fs::read(round.0.join("prompts/stages/synthesis.md")).unwrap(),
        b"original evidence"
    );
    assert!(
        journal
            .save_prompt(Stage::Synthesis, "first-unit", b"later")
            .is_err()
    );
}
