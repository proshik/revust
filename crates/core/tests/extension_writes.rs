use revust_core::{
    config::{extensions::Edit, paths::Layers},
    prompts::PromptSet,
};
use std::{fs, path::PathBuf, time::SystemTime};
struct Tree(PathBuf);
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn setup() -> (Tree, Layers, PromptSet) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let unique = format!("{timestamp}-{sequence}");
    let tree =
        Tree(std::env::temp_dir().join(format!("revust-write-{}-{unique}", std::process::id())));
    fs::create_dir(&tree.0).unwrap();
    let layers = Layers {
        user: tree.0.clone(),
        project: None,
    };
    let prompts = PromptSet::load(&layers).unwrap();
    (tree, layers, prompts)
}

#[test]
fn stale_or_invalid_edits_preserve_original_bytes_and_validate_profile_references() {
    let (tree, layers, prompts) = setup();
    let path = tree.0.join("extensions.json");
    let original = "{\"version\":1}\n";
    fs::write(&path, original).unwrap();
    for candidate in [
        r#"{"version":9}"#,
        r#"{"version":1,"policy":{"budget":{"max_calls":0}}}"#,
        r#"{"version":1,"profiles":{"missing":{"file":"extension-profiles/missing.md"}}}"#,
    ] {
        let edit = Edit::read(&path).unwrap();
        assert!(edit.replace(candidate, &layers, &prompts).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert_eq!(fs::read_dir(&tree.0).unwrap().count(), 1);
    }
    let edit = Edit::read(&path).unwrap();
    let external = r#"{"version":1,"policy":{"budget":{"max_calls":2}}}"#;
    fs::write(&path, external).unwrap();
    let error = edit.replace(original, &layers, &prompts).unwrap_err();
    assert!(error.contains("changed"));
    assert_eq!(fs::read_to_string(&path).unwrap(), external);
}

#[test]
fn two_snapshots_cannot_both_replace_the_same_version() {
    let (tree, layers, prompts) = setup();
    let path = tree.0.join("extensions.json");
    let first = Edit::read(&path).unwrap();
    let second = Edit::read(&path).unwrap();
    let chosen = r#"{"version":1,"policy":{"output":{"tui":false}}}"#;
    first.replace(chosen, &layers, &prompts).unwrap();
    assert!(
        second
            .replace(r#"{"version":1}"#, &layers, &prompts)
            .is_err()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), chosen);
    assert!(
        fs::read_dir(&tree.0).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp"))
    );
}

#[test]
fn concurrent_writers_cannot_both_commit_the_same_snapshot() {
    let (tree, layers, prompts) = setup();
    let path = tree.0.join("extensions.json");
    fs::write(&path, r#"{"version":1}"#).unwrap();
    let edits = [Edit::read(&path).unwrap(), Edit::read(&path).unwrap()];
    let gate = std::sync::Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = edits
            .into_iter()
            .enumerate()
            .map(|(index, edit)| {
                let gate = &gate;
                let layers = &layers;
                let prompts = &prompts;
                scope.spawn(move || {
                    gate.wait();
                    let candidate = format!(
                        "{{\"version\":1,\"policy\":{{\"budget\":{{\"max_calls\":{}}}}}}}",
                        index + 1
                    );
                    edit.replace(&candidate, layers, prompts).map(|_| candidate)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    let winner = outcomes.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), winner);
}
