use revust_core::history::inventory;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime},
};
struct Tree(PathBuf);
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn history_lists_recorded_rounds_and_excludes_private_finding_prose() {
    let t =
        Tree(std::env::temp_dir().join(format!("revust-history-contract-{}", std::process::id())));
    fs::create_dir(&t.0).unwrap();
    assert_eq!(inventory(&t.0.join("absent")).unwrap(), "");
    for (name, record) in [
        ("newer",json!({"stats":{"started_at":"2026-04-01T13:00:00+02:00"},"sources":{"reported":1,"expected":2,"degraded":["offline"]},"findings":[{"severity":"major","body":"DO NOT INCLUDE PRIVATE PROSE"}]}).to_string()),
        ("damaged","{bad".into()),
        ("older",json!({"stats":{"started_at":"2026-04-01T09:00:00Z"},"findings":[null,{"severity":"minor"}]}).to_string()),
    ] {
        let dir=t.0.join(name);fs::create_dir(&dir).unwrap();fs::write(dir.join("manifest.json"),"{}").unwrap();fs::write(dir.join("findings.json"),record).unwrap();
        fs::File::open(&dir).unwrap().set_times(fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH+Duration::from_secs(1_800_000_000))).unwrap();
    }
    fs::create_dir(t.0.join("prepared-only")).unwrap();
    fs::write(t.0.join("prepared-only/manifest.json"), "").unwrap();
    let before = fs::read(t.0.join("newer/findings.json")).unwrap();
    let text = inventory(&t.0).unwrap();
    assert!(!text.contains("PRIVATE PROSE"));
    let data: Value = serde_json::from_str(&text).unwrap();
    let rounds = data["rounds"].as_array().unwrap();
    assert_eq!(
        rounds
            .iter()
            .map(|r| r["round"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["older", "newer", "damaged"]
    );
    assert_eq!(rounds[0]["finding_counts"]["total"], 2);
    assert_eq!(rounds[1]["finding_counts"]["major"], 1);
    assert_eq!(rounds[1]["recorded_at"], "2026-04-01T11:00Z");
    assert_eq!(rounds[1]["sources"]["degraded"], json!(["offline"]));
    assert_eq!(rounds[2]["report_status"], "unreadable");
    assert!(rounds[2]["finding_counts"].is_null());
    assert_eq!(fs::read(t.0.join("newer/findings.json")).unwrap(), before);
}

#[test]
fn report_timestamps_have_explicit_unknown_and_calendar_boundaries() {
    let t = Tree(std::env::temp_dir().join(format!("revust-history-dates-{}", std::process::id())));
    fs::create_dir(&t.0).unwrap();
    for (stamp, expected, status) in [
        (
            "2026-04-01T1:02:03Z",
            Some("2026-04-01T01:02Z"),
            "available",
        ),
        (
            "2026-04-01T01:02:03,5-01:30",
            Some("2026-04-01T02:32Z"),
            "available",
        ),
        (
            "2026-04-01T01:00:00+24:60",
            Some("2026-03-31T00:00Z"),
            "available",
        ),
        ("0001-01-01T00:00:00Z", None, "available"),
        ("2026-02-30T01:00:00Z", None, "unreadable"),
        ("2026-04-01t01:00:00Z", None, "unreadable"),
        ("2026-04-01T01:00:60Z", None, "unreadable"),
        ("2026-04-01T01:00:00+25:00", None, "unreadable"),
    ] {
        let dir = t.0.join("01");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("manifest.json"), "{}").unwrap();
        fs::write(
            dir.join("findings.json"),
            json!({"stats":{"started_at":stamp},"findings":[]}).to_string(),
        )
        .unwrap();
        let actual: Value = serde_json::from_str(&inventory(&t.0).unwrap()).unwrap();
        assert_eq!(
            actual["rounds"][0]["recorded_at"],
            json!(expected),
            "{stamp}"
        );
        assert_eq!(actual["rounds"][0]["report_status"], status, "{stamp}");
    }
}

#[test]
fn history_marks_nonregular_and_escaping_reports_unreadable() {
    use std::os::unix::fs::symlink;
    let t = Tree(std::env::temp_dir().join(format!("revust-history-reads-{}", std::process::id())));
    let task = t.0.join("task");
    fs::create_dir_all(&task).unwrap();
    let outside = t.0.join("outside.json");
    fs::write(
        &outside,
        json!({"findings":[{"severity":"critical"}],"sources":{"expected":9}}).to_string(),
    )
    .unwrap();
    for name in ["fifo", "relative-escape", "absolute-link"] {
        let dir = task.join(name);
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("manifest.json"), "{}").unwrap();
    }
    nix::unistd::mkfifo(
        &task.join("fifo/findings.json"),
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    symlink(
        "../../outside.json",
        task.join("relative-escape/findings.json"),
    )
    .unwrap();
    symlink(&outside, task.join("absolute-link/findings.json")).unwrap();
    // A blocking read of the FIFO would wait for a writer forever.
    let (sender, receiver) = std::sync::mpsc::channel();
    let root = task.clone();
    std::thread::spawn(move || sender.send(inventory(&root)));
    let text = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("history inventory must not block on a FIFO report")
        .unwrap();
    let data: Value = serde_json::from_str(&text).unwrap();
    let rounds = data["rounds"].as_array().unwrap();
    assert_eq!(rounds.len(), 3);
    for round in rounds {
        assert_eq!(round["report_status"], "unreadable", "{round}");
        assert!(round["finding_counts"].is_null(), "{round}");
        assert!(round["sources"].is_null(), "{round}");
    }
}
