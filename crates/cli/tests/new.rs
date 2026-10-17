#![cfg(unix)]
use revust_core::archive::Archive;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "revust-new-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn invoke(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_revust"))
            .args(args)
            .env_clear()
            .env("HOME", &self.0)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn prepared_round_feeds_dry_run_and_archive_claim_without_rewriting_inputs() {
    let tree = Tree::new();
    let args = [
        "new",
        "--task=synthetic",
        "--run=01",
        "--no-tui",
        "--markdown",
        "--lenses=ignored",
    ];
    let output = tree.invoke(&args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let paths: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        paths["created"],
        serde_json::json!(["task_dir", "task_file", "round_dir", "input_dir"])
    );
    let round = PathBuf::from(paths["round_dir"].as_str().unwrap());
    assert_eq!(fs::read_dir(&round).unwrap().count(), 1);
    assert_eq!(fs::read_dir(round.join("input")).unwrap().count(), 0);
    fs::write(
        round.join("input/scope.md"),
        "Inspect synthetic local changes.\n",
    )
    .unwrap();
    let prepared = tree.invoke(&["dry-run", "--task=synthetic", "--run=01"]);
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let mut archive = Archive::claim(&tree.0.join(".revust/tasks"), "synthetic", "01").unwrap();
    let repeated = tree.invoke(&args);
    assert!(repeated.status.success()); // Preparation is observation of the existing layout, not execution.
    assert_eq!(
        serde_json::from_slice::<Value>(&repeated.stdout).unwrap()["created"],
        serde_json::json!([])
    );
    assert!(Archive::claim(&tree.0.join(".revust/tasks"), "synthetic", "01").is_err());
    archive.finish(b"{}\n").unwrap();
    let completed = tree.invoke(&args);
    assert_eq!(completed.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&completed.stderr).contains("already run"));
    assert_eq!(
        fs::read(round.join("input/scope.md")).unwrap(),
        b"Inspect synthetic local changes.\n"
    );
}

#[test]
fn invalid_or_empty_names_and_root_fail_before_creating_directories() {
    for args in [
        vec!["new", "--task=../escape", "--run=01"],
        vec!["new", "--task=synthetic"],
        vec!["new", "--task=synthetic", "--run=task.md"],
        vec!["new", "--task=synthetic", "--run=01", "--tasks-dir="],
    ] {
        let tree = Tree::new();
        let output = tree.invoke(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(fs::read_dir(&tree.0).unwrap().count(), 0);
    }
}
