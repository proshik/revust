#![cfg(unix)]
use revust_core::archive::reader;
use serde_json::json;
use std::{
    fs,
    os::unix::fs::symlink,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "revust-read-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::write(p.join("manifest.json"),json!({"task":"test","run":"01","profile":"removed","finished_at":"2026-10-10T10:00:02Z","agents":[{"name":"a","executor":"codex","requested_model":"requested","actual_model":"observed","lenses":["bugs"]}]}).to_string()).unwrap();
        fs::write(
            p.join("findings.json"),
            json!({"scope":{"task":"test","run":"01"}}).to_string(),
        )
        .unwrap();
        fs::write(
            p.join("events.jsonl"),
            b"{\"kind\":\"future_kind\",\"at\":\"2026-10-10T10:00:01Z\",\"text\":\"saved text\"}",
        )
        .unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn saved_roster_wins_and_old_manifest_uses_observed_source_identity() {
    let t = Temp::new();
    let old = reader::load(&t.0).unwrap();
    assert_eq!(old.roster[0].executor, "codex");
    assert_eq!(old.roster[0].model, "observed");
    assert_eq!(old.roster[0].lenses, ["bugs"]);
    assert!(!old.roster[0].color.is_empty());
    assert_eq!(old.events[0].text, "saved text");
    assert_eq!(old.events[0].kind, "future_kind");
    let path = t.0.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["roster"] = json!([{"name":"saved","executor":"opencode","model":"fixture/model","color":"#123456","lenses":["removed-lens"]}]);
    fs::write(path, manifest.to_string()).unwrap();
    let saved = reader::load(&t.0).unwrap();
    assert_eq!(saved.roster.len(), 1);
    assert_eq!(saved.roster[0].name, "saved");
    assert_eq!(saved.roster[0].executor, "opencode");
    assert_eq!(saved.roster[0].model, "fixture/model");
    assert_eq!(saved.roster[0].color, "#123456");
}
#[test]
fn contained_file_aliases_are_read_but_fifo_and_large_event_records_fail() {
    let t = Temp::new();
    fs::create_dir(t.0.join("inside")).unwrap();
    fs::rename(t.0.join("findings.json"), t.0.join("inside/report.json")).unwrap();
    symlink("inside/report.json", t.0.join("findings.json")).unwrap();
    assert!(reader::load(&t.0).is_ok());
    fs::remove_file(t.0.join("findings.json")).unwrap();
    nix::unistd::mkfifo(
        &t.0.join("findings.json"),
        nix::sys::stat::Mode::from_bits_truncate(0o600),
    )
    .unwrap();
    assert!(reader::load(&t.0).err().unwrap().contains("regular file"));
    fs::remove_file(t.0.join("findings.json")).unwrap();
    symlink("inside/report.json", t.0.join("findings.json")).unwrap();
    fs::write(t.0.join("events.jsonl"), vec![b' '; 4 << 20]).unwrap();
    assert!(reader::load(&t.0).err().unwrap().contains("size limit"));
}

#[test]
fn saved_view_rejects_missing_corrupt_and_escaping_evidence_without_mutation() {
    let t = Temp::new();
    let original = fs::read(t.0.join("manifest.json")).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(&original).unwrap();
    manifest["agents"][0]["actual_model"] = json!("");
    fs::write(t.0.join("manifest.json"), manifest.to_string()).unwrap();
    assert_eq!(reader::load(&t.0).unwrap().roster[0].model, "");
    let names = ["manifest.json", "findings.json", "events.jsonl"];
    let before: Vec<_> = names
        .iter()
        .map(|name| fs::read(t.0.join(name)).unwrap())
        .collect();
    reader::load(&t.0).unwrap();
    for (name, bytes) in names.iter().zip(before) {
        assert_eq!(fs::read(t.0.join(name)).unwrap(), bytes);
    }
    fs::rename(t.0.join("events.jsonl"), t.0.join("saved-events")).unwrap();
    assert!(reader::load(&t.0).is_err());
    assert!(!t.0.join("events.jsonl").exists());
    symlink("../not-owned", t.0.join("events.jsonl")).unwrap();
    assert!(reader::load(&t.0).is_err());
    fs::remove_file(t.0.join("events.jsonl")).unwrap();
    fs::write(t.0.join("events.jsonl"), b"{\"kind\":\"partial\"}\n").unwrap();
    assert!(reader::load(&t.0).err().unwrap().contains("incomplete"));
    fs::write(t.0.join("events.jsonl"), b"{\"kind\":").unwrap();
    assert!(reader::load(&t.0).is_err());
    fs::write(t.0.join("manifest.json"), b"{}").unwrap();
    assert!(reader::load(&t.0).is_err());
}

#[test]
fn saved_nullable_lists_restore_declared_fields_without_consulting_current_settings() {
    let t = Temp::new();
    fs::write(
        t.0.join("manifest.json"),
        json!({
            "task":"test", "run":"01", "finished_at":"2026-10-14T08:00:00Z",
            "roster":[null, {"name":"saved", "lenses":[null,"bugs"], "model":"chosen"}],
            "prompts":[null], "stages":[null], "agents":[null],
            "future_extension":{"nested":null}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        t.0.join("events.jsonl"),
        json!({
            "kind":"findings", "at":"2026-10-14T07:59:59Z",
            "findings":[null,{"sources":[null,"saved"]}]
        })
        .to_string(),
    )
    .unwrap();
    let saved = reader::load(&t.0).unwrap();
    assert_eq!(saved.roster.len(), 2);
    assert_eq!(saved.roster[1].lenses, ["", "bugs"]);
    assert_eq!(saved.roster[1].model, "chosen");
    assert_eq!(saved.manifest.prompts.len(), 1);
    assert_eq!(saved.events[0].findings[1].sources, ["", "saved"]);
    fs::write(
        t.0.join("events.jsonl"),
        r#"{"kind":"findings","at":"2026-10-14T08:00:00Z","findings":[{"sources":[9]}]}"#,
    )
    .unwrap();
    assert!(reader::load(&t.0).is_err());
}

fn legacy_terminal(state: serde_json::Value) -> Temp {
    use revust_core::runtime::{
        journal::{Identity, Journal},
        pipeline,
    };
    let t = Temp::new();
    fs::write(t.0.join("manifest.json"), b"").unwrap();
    fs::write(t.0.join("findings.json"), json!({
        "scope":{"task":"test","run":"01"},
        "sources":{"expected":1,"reported":0,"degraded":["a"],"agents":[{"name":"a","executor":"codex","degraded":true}]},
        "stats":{"started_at":"2026-10-17T10:00:00Z","finished_at":"2026-10-17T10:00:01Z"}
    }).to_string()).unwrap();
    fs::write(t.0.join("report.md"), "Synthetic saved report\n").unwrap();
    fs::remove_file(t.0.join("events.jsonl")).unwrap();
    let journal = Journal::create(
        &t.0,
        Identity {
            task: "test".into(),
            run: "01".into(),
            request: "synthetic".into(),
        },
    )
    .unwrap();
    let runtime: pipeline::Report = serde_json::from_value(json!({
        "state":state,"categories":{"findings":[],"open_questions":[],"pre_existing":[],"immaterial":[],"dropped":[]},"successful_reviewers":[],"units":[],
        "policy_failures":["synthetic delivery failure"],"verification_evidence":[],"synthesis_succeeded":false
    })).unwrap();
    journal.finish(&runtime).unwrap();
    t
}

#[test]
fn legacy_terminal_failures_restore_read_only_without_filling_the_empty_marker() {
    for state in [json!("failed"), json!({"stopped":"cancelled"})] {
        let t = legacy_terminal(state.clone());
        let names = [
            "manifest.json",
            "findings.json",
            "report.md",
            "events.jsonl",
            "revust/run.json",
            "revust/events.jsonl",
        ];
        let before = names.map(|name| fs::read(t.0.join(name)).unwrap());
        let restored = reader::load(&t.0).unwrap();
        assert_eq!(restored.report.exit_code(), 2);
        assert_eq!(restored.manifest.task, "test");
        assert_eq!(restored.roster[0].name, "a");
        let value: serde_json::Value =
            serde_json::from_str(&restored.report.json().unwrap()).unwrap();
        assert_eq!(value["outcome"]["state"], state);
        assert!(
            restored
                .report
                .markdown()
                .contains("synthetic delivery failure")
        );
        for (name, bytes) in names.into_iter().zip(before) {
            assert_eq!(fs::read(t.0.join(name)).unwrap(), bytes);
        }
    }
}

#[test]
fn legacy_empty_markers_require_terminal_matching_contained_and_inactive_evidence() {
    use nix::fcntl::{Flock, FlockArg};
    for case in [
        "active",
        "unfinished",
        "truncated",
        "after_terminal",
        "corrupt_journal",
        "identity",
        "empty_identity",
        "header_version",
        "success",
        "report_time",
        "report_time_order",
        "explicit_outcome",
        "report_json",
        "missing_markdown",
        "corrupt_marker",
        "journal_escape",
        "marker_alias",
        "marker_hardlink",
    ] {
        let t = legacy_terminal(json!("failed"));
        let journal = t.0.join("revust/events.jsonl");
        let mut guard = None;
        match case {
            "active" => {
                guard = Some(
                    Flock::lock(
                        fs::File::open(t.0.join("manifest.json")).unwrap(),
                        FlockArg::LockExclusiveNonblock,
                    )
                    .unwrap(),
                )
            }
            "unfinished" => fs::write(&journal, b"").unwrap(),
            "truncated" => {
                let mut bytes = fs::read(&journal).unwrap();
                bytes.pop();
                fs::write(&journal, bytes).unwrap();
            }
            "after_terminal" => {
                let mut bytes = fs::read(&journal).unwrap();
                bytes.extend_from_slice(b"{\"type\":");
                fs::write(&journal, bytes).unwrap();
            }
            "corrupt_journal" => fs::write(&journal, b"not JSON\n").unwrap(),
            "identity" | "empty_identity" | "header_version" => {
                let p = t.0.join("revust/run.json");
                let mut v: serde_json::Value =
                    serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                if case == "header_version" {
                    v["version"] = json!(99);
                } else {
                    v["identity"]["run"] = json!(if case == "identity" { "other" } else { "" });
                }
                fs::write(p, v.to_string()).unwrap();
            }
            "success" => {
                let bytes = fs::read_to_string(&journal).unwrap();
                fs::write(&journal, bytes.replace("\"failed\"", "\"complete\"")).unwrap();
            }
            "report_time" => {
                let p = t.0.join("findings.json");
                let bytes = fs::read_to_string(&p).unwrap();
                fs::write(
                    p,
                    bytes.replace("2026-10-17T10:00:01Z", "0001-01-01T00:00:00Z"),
                )
                .unwrap();
            }
            "explicit_outcome" => {
                let p = t.0.join("findings.json");
                let mut v: serde_json::Value =
                    serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                v["outcome"] = json!({"state":"failed","policy_failures":["synthetic delivery failure"],"diagnostics":[]});
                fs::write(p, v.to_string()).unwrap();
            }
            "report_time_order" => {
                let p = t.0.join("findings.json");
                let bytes = fs::read_to_string(&p).unwrap();
                fs::write(
                    p,
                    bytes.replace("2026-10-17T10:00:01Z", "2026-10-17T09:00:00Z"),
                )
                .unwrap();
            }
            "report_json" => fs::write(t.0.join("findings.json"), b"{").unwrap(),
            "missing_markdown" => fs::remove_file(t.0.join("report.md")).unwrap(),
            "corrupt_marker" => fs::write(t.0.join("manifest.json"), b"{").unwrap(),
            "journal_escape" => {
                fs::rename(t.0.join("revust"), t.0.join("saved-journal")).unwrap();
                symlink("../outside-journal", t.0.join("revust")).unwrap();
            }
            "marker_alias" => {
                fs::rename(t.0.join("manifest.json"), t.0.join("saved-marker")).unwrap();
                symlink("saved-marker", t.0.join("manifest.json")).unwrap();
            }
            "marker_hardlink" => {
                fs::hard_link(t.0.join("manifest.json"), t.0.join("second-marker")).unwrap()
            }
            _ => unreachable!(),
        }
        assert!(reader::load(&t.0).is_err(), "accepted {case}");
        drop(guard);
    }
}

#[test]
fn explicit_saved_outcomes_must_be_typed_and_match_the_report() {
    let t = Temp::new();
    let marker = t.0.join("manifest.json");
    let findings = t.0.join("findings.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    let mut report: serde_json::Value =
        serde_json::from_slice(&fs::read(&findings).unwrap()).unwrap();
    let outcome = json!({"state":"failed","policy_failures":["synthetic incomplete review"],"diagnostics":[]});
    manifest["outcome"] = outcome.clone();
    report["outcome"] = outcome;
    fs::write(&marker, manifest.to_string()).unwrap();
    fs::write(&findings, report.to_string()).unwrap();
    assert_eq!(reader::load(&t.0).unwrap().report.exit_code(), 2);
    for corruption in [
        json!(null),
        json!({"state":"complete"}),
        json!({"state":42}),
    ] {
        report["outcome"] = corruption;
        fs::write(&findings, report.to_string()).unwrap();
        assert!(reader::load(&t.0).is_err());
    }
}
