#![cfg(unix)]
use revust_core::archive::{Archive, cleanup, scaffold};
use std::{fs, path::PathBuf};

#[test]
fn cleanup_respects_archive_owner_then_removes_abandoned_sidecars_only_in_selected_task() {
    let root = std::env::temp_dir().join(format!("revust-cleanup-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let tasks = root.join("tasks");
    let paths = scaffold::create(&tasks, "remove", "01").unwrap();
    fs::write(&paths.scope, "Caller scope.\n").unwrap();
    let sibling = scaffold::create(&tasks, "keep", "01").unwrap();
    fs::write(&sibling.scope, "Keep this scope.\n").unwrap();
    let archive = Archive::claim(&tasks, "remove", "01").unwrap();
    archive
        .write("agents/partial.jsonl".as_ref(), b"partial evidence\n")
        .unwrap();
    assert!(
        matches!(cleanup::remove(&tasks, "remove"), Err(e) if e.contains("held by a running review"))
    );
    assert_eq!(fs::read(&paths.scope).unwrap(), b"Caller scope.\n");
    assert_eq!(
        fs::read(paths.round_dir.join("agents/partial.jsonl")).unwrap(),
        b"partial evidence\n"
    );
    drop(archive);
    let result = cleanup::remove(&tasks, "remove").unwrap();
    assert_eq!(result.removed.len(), 1);
    assert_eq!(result.removed[0].id, "remove");
    assert_eq!(result.removed[0].rounds, 0); // Empty claimed marker is not a completed round.
    assert!(!paths.task_dir.exists());
    assert_eq!(fs::read(sibling.scope).unwrap(), b"Keep this scope.\n");
    assert!(matches!(cleanup::remove(&tasks, "remove"), Err(e) if e.contains("not found")));
}

#[test]
fn cleanup_unlinks_child_aliases_and_refuses_ambiguous_marker_ownership() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join(format!("revust-cleanup-links-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let paths = scaffold::create(&root.join("tasks"), "remove", "01").unwrap();
    let outside = root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "external evidence").unwrap();
    symlink(&outside, paths.round_dir.join("alias")).unwrap();
    let marker = root.join("marker");
    fs::write(&marker, []).unwrap();
    fs::hard_link(&marker, paths.round_dir.join("manifest.json")).unwrap();
    assert!(cleanup::remove(&root.join("tasks"), "remove").is_err());
    assert!(paths.round_dir.exists());
    fs::remove_file(paths.round_dir.join("manifest.json")).unwrap();
    cleanup::remove(&root.join("tasks"), "remove").unwrap();
    assert_eq!(
        fs::read(outside.join("keep")).unwrap(),
        b"external evidence"
    );
    assert_eq!(fs::read(marker).unwrap(), b"");
}
