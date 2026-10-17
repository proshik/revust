#![cfg(unix)]
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "revust-view-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn invoke(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_revust"))
            .args(args)
            .env_clear()
            .current_dir(&self.0)
            .output()
            .unwrap()
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn saved_reports_are_read_without_profiles_providers_or_writes() {
    let tree = Tree::new();
    let archive = tree.0.join("archive");
    fs::create_dir(&archive).unwrap();
    fs::create_dir(tree.0.join(".revust")).unwrap();
    fs::write(tree.0.join(".revust/config"), "broken config").unwrap();
    for findings in [
        serde_json::json!([]),
        serde_json::json!([{"id":"saved-1","title":"Saved issue","severity":"major","sources":["source-a"]}]),
    ] {
        let input =
            serde_json::json!({"scope":{"task":"synthetic","run":"01"},"findings":findings});
        fs::write(
            archive.join("manifest.json"),
            r#"{"task":"synthetic","run":"01","finished_at":"2026-10-10T10:00:02Z"}"#,
        )
        .unwrap();
        fs::write(archive.join("findings.json"), input.to_string()).unwrap();
        fs::write(archive.join("events.jsonl"), b"").unwrap();
        let before: Vec<_> = ["manifest.json", "findings.json", "events.jsonl"]
            .map(|n| fs::read(archive.join(n)).unwrap())
            .into();
        for (flag, field) in [
            (
                vec![
                    "--no-tui",
                    "view",
                    "--archive",
                    "archive",
                    "--profile=missing",
                    "--lenses=removed",
                ],
                "json",
            ),
            (
                vec!["view", "--archive=archive", "--no-tui", "--markdown"],
                "markdown",
            ),
        ] {
            let output = tree.invoke(&flag);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stderr.is_empty());
            if field == "json" {
                let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(result["scope"]["task"], "synthetic");
                assert_eq!(result["scope"]["run"], "01");
                assert_eq!(
                    result["findings"].as_array().unwrap().len(),
                    findings.as_array().unwrap().len()
                );
                if !findings.as_array().unwrap().is_empty() {
                    assert_eq!(result["findings"][0]["id"], "saved-1");
                    assert_eq!(
                        result["findings"][0]["sources"],
                        serde_json::json!(["source-a"])
                    );
                }
            } else {
                let text = String::from_utf8(output.stdout).unwrap();
                assert!(text.contains("synthetic") && text.contains("01"));
                if findings.as_array().unwrap().is_empty() {
                    assert!(text.contains("No active findings"));
                } else {
                    assert!(text.contains("Saved issue") && text.contains("saved-1"));
                }
            }
        }
        assert_eq!(fs::read_dir(&archive).unwrap().count(), 3);
        for (name, bytes) in ["manifest.json", "findings.json", "events.jsonl"]
            .into_iter()
            .zip(before)
        {
            assert_eq!(fs::read(archive.join(name)).unwrap(), bytes);
        }
    }
}
#[test]
fn missing_or_incomplete_archive_fails_without_creating_any_files() {
    let tree = Tree::new();
    let output = tree.invoke(&["--no-tui", "view", "--archive=missing"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read_dir(&tree.0).unwrap().count(), 0);
    fs::create_dir(tree.0.join("archive")).unwrap();
    fs::write(tree.0.join("archive/manifest.json"), "{}").unwrap();
    fs::write(tree.0.join("archive/findings.json"), "{}").unwrap();
    let output = tree.invoke(&["--no-tui", "view", "--archive=archive"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("completion timestamp"));
    assert_eq!(fs::read_dir(tree.0.join("archive")).unwrap().count(), 2);
}
