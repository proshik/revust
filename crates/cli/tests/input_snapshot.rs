#![cfg(unix)]
use revust::inputs::{Snapshot, Sources};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "revust-input-snapshot-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn snapshot_normalizes_text_and_refuses_links_unsafe_or_nonregular_inputs() {
    let tree = Tree::new();
    let mut snapshot = Snapshot::default();
    let original = b"# Scope\r\n\twide \xe7\x95\x8c\n";
    let path = tree.file("scope.MD", original);
    let doc = snapshot.file("scope", "input/scope.md", &path);
    assert_eq!(doc.content, "# Scope\n\twide 界\n");
    assert!(doc.markdown);
    assert!(doc.notice.is_empty());
    assert_eq!(doc.label, "scope");
    assert_eq!(doc.path, "input/scope.md");
    assert_eq!(fs::read(&path).unwrap(), original);
    let link = tree.0.join("link.txt");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let doc = snapshot.file("link", "input/link.txt", &link);
    assert!(!doc.markdown);
    assert!(doc.content.is_empty());
    assert!(doc.notice.starts_with("Snapshot unavailable [read]:"));
    for (index, bytes) in [b"bad\x1b[31m".as_slice(), b"bad\0", b"bad\xff", b"bad\r"]
        .iter()
        .enumerate()
    {
        let p = tree.file(&format!("bad-{index}"), bytes);
        let doc = snapshot.file("bad", "bad", &p);
        assert!(doc.content.is_empty());
        assert_eq!(
            doc.notice,
            format!(
                "Snapshot withheld [text validation; observed bytes={}]",
                bytes.len()
            )
        );
    }
    assert_eq!(
        snapshot.file("dir", "dir", &tree.0).notice,
        "Snapshot unavailable [file type]"
    );
    let fifo = tree.0.join("pipe");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR).unwrap();
    assert_eq!(
        snapshot.file("pipe", "pipe", &fifo).notice,
        "Snapshot unavailable [file type]"
    );
    let missing = snapshot.file("missing", "missing", &tree.0.join("absent"));
    assert!(missing.content.is_empty());
    assert!(missing.notice.starts_with("Snapshot unavailable [read]:"));
}

#[test]
fn truncation_keeps_utf8_and_crlf_boundaries_and_reports_original_byte_sizes() {
    const LIMIT: usize = 1 << 20;
    for suffix in ["界", "\r\n"] {
        let tree = Tree::new();
        let mut bytes = vec![b'x'; LIMIT - 1];
        bytes.extend_from_slice(suffix.as_bytes());
        let path = tree.file("large.md", &bytes);
        let doc = Snapshot::default().file("large", "large.md", &path);
        assert_eq!(doc.content.len(), LIMIT - 1);
        assert!(doc.content.bytes().all(|b| b == b'x'));
        assert_eq!(
            doc.notice,
            format!(
                "Snapshot partial [captured={}; observed={}]",
                LIMIT - 1,
                bytes.len()
            )
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn rejected_bytes_consume_total_budget_but_empty_files_still_have_empty_documents() {
    const LIMIT: usize = 1 << 20;
    let tree = Tree::new();
    let unsafe_file = tree.file("binary", &vec![0; LIMIT]);
    let mut snapshot = Snapshot::default();
    for _ in 0..8 {
        assert!(
            snapshot
                .file("binary", "binary", &unsafe_file)
                .notice
                .starts_with("Snapshot withheld [text validation;")
        );
    }
    let text = tree.file("text", b"text");
    let doc = snapshot.file("text", "text", &text);
    assert_eq!(doc.notice, "Snapshot omitted [byte budget=8388608]");
    assert!(doc.content.is_empty());
    let empty = tree.file("empty", b"");
    let doc = snapshot.file("empty", "empty", &empty);
    assert!(doc.notice.is_empty());
    assert!(doc.content.is_empty());
}

#[test]
fn collection_preserves_order_empty_optional_files_and_profile_snapshot_origin() {
    let tree = Tree::new();
    let scope = tree.file("scope.md", b"scope");
    tree.file("goal.md", b"");
    let profile = tree.file("input-profile.md", b"project calibration");
    let context = tree.0.join("context");
    fs::create_dir_all(context.join("nested")).unwrap();
    fs::write(context.join("a.md"), "alpha").unwrap();
    fs::write(context.join("nested/b.txt"), "beta").unwrap();
    let mut sources = Sources {
        scope,
        input_dir: Some(tree.0.clone()),
        profile: Some(profile),
        profile_is_snapshot: true,
        context: Some(context.clone()),
        ..Sources::default()
    };
    let docs = Snapshot::default().load(&sources);
    assert_eq!(
        docs.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(),
        ["scope", "goal", "profile", "a.md", "nested/b.txt"]
    );
    assert!(docs[1].content.is_empty() && docs[1].notice.is_empty());
    assert_eq!(docs[2].path, "prompts/input-profile.md");
    assert_eq!(docs[2].content, "project calibration");
    assert_eq!(docs[4].path, "input/context/nested/b.txt");
    assert_eq!(docs[4].content, "beta");
    sources.profile_is_snapshot = false;
    sources.context = None;
    assert_eq!(Snapshot::default().load(&sources).len(), 2); // missing round profile is absent
    fs::remove_file(tree.0.join("goal.md")).unwrap();
    assert_eq!(Snapshot::default().load(&sources).len(), 1);
    sources.input_dir = None;
    sources.goal = Some(tree.file("external-goal.md", b"external"));
    sources.profile = None;
    assert_eq!(Snapshot::default().load(&sources)[1].content, "external");
}

#[test]
fn context_links_are_displayed_without_traversing_linked_directories() {
    let tree = Tree::new();
    let scope = tree.file("scope.md", b"scope");
    let context = tree.0.join("context");
    fs::create_dir(&context).unwrap();
    let target = tree.file("target.txt", b"linked content");
    std::os::unix::fs::symlink(&target, context.join("a.txt")).unwrap();
    std::os::unix::fs::symlink(&tree.0, context.join("b")).unwrap(); // would cycle if followed
    std::os::unix::fs::symlink(tree.0.join("absent"), context.join("c")).unwrap();
    std::os::unix::fs::symlink(tree.0.join("absent-goal"), tree.0.join("goal.md")).unwrap();
    let mut sources = Sources {
        scope,
        input_dir: Some(tree.0.clone()),
        context: Some(context),
        ..Sources::default()
    };
    let docs = Snapshot::default().load(&sources);
    assert_eq!(
        docs.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(),
        ["scope", "goal", "a.txt", "b", "c"]
    );
    assert!(docs[1].notice.starts_with("Snapshot unavailable [read]:"));
    assert!(docs[2].content.is_empty());
    assert!(docs[2].notice.starts_with("Snapshot unavailable [read]:"));
    assert!(docs[3].notice.starts_with("Snapshot unavailable [read]:"));
    assert!(docs[4].notice.starts_with("Snapshot unavailable [read]:"));
    let link = tree.0.join("root-link");
    std::os::unix::fs::symlink(&tree.0, &link).unwrap();
    sources.context = Some(link);
    let docs = Snapshot::default().load(&sources);
    assert_eq!(docs.len(), 3);
    assert_eq!(docs[2].label, "context");
    assert!(
        docs[2]
            .notice
            .starts_with("Context unavailable [directory]:")
    );
    let empty = tree.0.join("empty");
    fs::create_dir(&empty).unwrap();
    sources.context = Some(empty);
    assert_eq!(Snapshot::default().load(&sources).len(), 2);
}

#[test]
fn context_file_and_entry_limits_distinguish_exact_limits_from_omitted_work() {
    let tree = Tree::new();
    let scope = tree.file("scope.md", b"scope");
    let context = tree.0.join("context");
    fs::create_dir(&context).unwrap();
    let sources = Sources {
        scope,
        context: Some(context.clone()),
        ..Sources::default()
    };
    for i in 0..128 {
        fs::write(context.join(format!("{i:03}.txt")), "text").unwrap();
    }
    assert_eq!(Snapshot::default().load(&sources).len(), 129); // scope + exactly 128 files, no "more"
    fs::write(context.join("128.txt"), "text").unwrap();
    let docs = Snapshot::default().load(&sources);
    assert_eq!(docs.len(), 130);
    assert_eq!(docs.last().unwrap().label, "more");
    assert_eq!(
        docs.last().unwrap().notice,
        "Context partial [document cap=128; entry cap=1024]"
    );
    fs::remove_dir_all(&context).unwrap();
    fs::create_dir(&context).unwrap();
    for i in 0..1024 {
        fs::create_dir(context.join(format!("{i:04}"))).unwrap();
    }
    assert_eq!(Snapshot::default().load(&sources).len(), 1); // empty dirs still consume traversal budget
    fs::create_dir(context.join("1024")).unwrap();
    let docs = Snapshot::default().load(&sources);
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[1].label, "more");
}

#[test]
fn archive_sources_preserve_snapshot_priority_and_missing_input_diagnostics() {
    let tree = Tree::new();
    fs::create_dir(tree.0.join("input")).unwrap();
    fs::create_dir(tree.0.join("prompts")).unwrap();
    tree.file("input/scope.md", b"archived scope");
    tree.file("input/profile.md", b"round calibration");
    // Snapshot presence wins, including empty and dangling snapshots.
    let snapshot = tree.file("prompts/input-profile.md", b"saved project calibration");
    let documents = || Snapshot::default().load(&Sources::archive(&tree.0).unwrap());
    let docs = documents();
    assert_eq!(docs[0].content, "archived scope");
    assert_eq!(docs[1].content, "saved project calibration");
    assert_eq!(docs[1].path, "prompts/input-profile.md");
    // The reference supplies context even when absent, yielding a diagnostic.
    assert_eq!(docs[2].label, "context");
    assert!(
        docs[2]
            .notice
            .starts_with("Context unavailable [directory]:")
    );
    fs::write(&snapshot, b"").unwrap();
    assert!(documents()[1].content.is_empty());
    fs::remove_file(&snapshot).unwrap();
    std::os::unix::fs::symlink(tree.0.join("missing"), &snapshot).unwrap();
    assert!(
        documents()[1]
            .notice
            .starts_with("Snapshot unavailable [read]:")
    );
    fs::remove_file(&snapshot).unwrap();
    assert_eq!(documents()[1].content, "round calibration");
    fs::remove_file(tree.0.join("input/profile.md")).unwrap();
    assert_eq!(documents().len(), 2);
    assert!(!snapshot.exists());
}

#[test]
fn review_documents_follow_frozen_paths_and_selected_calibration_snapshot() {
    use revust_core::{config::runtime::Invocation, plan::PromptPlan};
    let tree = Tree::new();
    let input = tree.0.join("tasks/test/01/input");
    fs::create_dir_all(&input).unwrap();
    fs::create_dir(tree.0.join(".revust")).unwrap();
    tree.file(".revust/profile.md", b"selected calibration");
    fs::write(input.join("scope.md"), b"scope").unwrap();
    fs::write(input.join("goal.md"), b"").unwrap();
    let invocation = Invocation {
        config_dir: Some(tree.0.join("user")),
        workdir: Some(tree.0.clone()),
        task: "test".into(),
        run: "01".into(),
        overrides: vec![(
            "tasks-dir".into(),
            tree.0.join("tasks").to_str().unwrap().into(),
        )],
    };
    let plan = PromptPlan::resolve(&tree.0, None, &invocation, &[])
        .unwrap()
        .into_execution()
        .unwrap();
    tree.file(".revust/profile.md", b"changed after resolution");
    fs::create_dir(input.join("context")).unwrap();
    fs::write(input.join("context/later.txt"), "added later").unwrap();
    for snapshot in plan.snapshots() {
        fs::create_dir_all(snapshot.destination.parent().unwrap()).unwrap();
        fs::write(&snapshot.destination, &snapshot.content).unwrap();
    }
    let sources = Sources::review(&plan);
    assert!(sources.context.is_none()); // absent at resolution, never used by providers
    let docs = Snapshot::default().load(&sources);
    assert_eq!(
        docs.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(),
        ["scope", "goal", "profile"]
    );
    assert!(docs[1].content.is_empty());
    assert_eq!(docs[2].content, "selected calibration");
    assert_eq!(docs[2].path, "prompts/input-profile.md");
    fs::write(input.join("profile.md"), "round calibration").unwrap();
    let plan = PromptPlan::resolve(&tree.0, None, &invocation, &[])
        .unwrap()
        .into_execution()
        .unwrap();
    let sources = Sources::review(&plan);
    assert!(!sources.profile_is_snapshot);
    let docs = Snapshot::default().load(&sources);
    assert_eq!(docs[2].content, "round calibration");
    assert_eq!(docs[3].content, "added later");
    assert_eq!(docs[2].path, "input/profile.md");
}

#[test]
fn snapshot_refuses_an_aliased_parent_and_retains_captured_observation() {
    let tree = Tree::new();
    let outside = tree.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("scope.md"), "outside content").unwrap();
    std::os::unix::fs::symlink(&outside, tree.0.join("alias")).unwrap();
    let mut capture = Snapshot::default();
    let blocked = capture.file("scope", "input/scope.md", &tree.0.join("alias/scope.md"));
    assert!(blocked.content.is_empty());
    assert!(blocked.notice.starts_with("Snapshot unavailable [read]:"));
    let selected = capture.file("scope", "input/scope.md", &outside.join("scope.md"));
    fs::write(outside.join("scope.md"), "later edit").unwrap();
    assert_eq!(selected.content, "outside content");
    assert_eq!(
        fs::read_to_string(outside.join("scope.md")).unwrap(),
        "later edit"
    );
}

#[test]
fn explicitly_selected_archive_root_alias_keeps_its_read_only_observations() {
    let tree = Tree::new();
    fs::create_dir(tree.0.join("input")).unwrap();
    fs::write(tree.0.join("input/scope.md"), "saved scope").unwrap();
    let alias = tree.0.join("archive-alias");
    std::os::unix::fs::symlink(&tree.0, &alias).unwrap();
    let direct = Snapshot::default().load(&Sources::archive(&tree.0).unwrap());
    let linked = Snapshot::default().load(&Sources::archive(&alias).unwrap());
    assert_eq!(direct[0].content, linked[0].content);
    assert_eq!(linked[0].content, "saved scope");
    assert_eq!(linked[0].path, "input/scope.md");
    assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
}

#[test]
fn review_does_not_substitute_new_optional_contents_after_resolution() {
    use revust_core::{config::runtime::Invocation, plan::PromptPlan};
    let tree = Tree::new();
    let input = tree.0.join("tasks/test/01/input");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("scope.md"), "selected scope").unwrap();
    let invocation = Invocation {
        config_dir: Some(tree.0.join("user")),
        workdir: Some(tree.0.clone()),
        task: "test".into(),
        run: "01".into(),
        overrides: vec![(
            "tasks-dir".into(),
            tree.0.join("tasks").to_str().unwrap().into(),
        )],
    };
    let plan = PromptPlan::resolve(&tree.0, None, &invocation, &[])
        .unwrap()
        .into_execution()
        .unwrap();
    fs::write(input.join("goal.md"), "not selected").unwrap();
    fs::write(input.join("profile.md"), "not selected").unwrap();
    let documents = Snapshot::default().load(&Sources::review(&plan));
    assert_eq!(
        documents
            .iter()
            .map(|doc| doc.label.as_str())
            .collect::<Vec<_>>(),
        ["scope"]
    );
    assert_eq!(documents[0].content, "selected scope");
    assert_eq!(
        fs::read_to_string(input.join("profile.md")).unwrap(),
        "not selected"
    );
}
