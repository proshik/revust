use revust_core::{config::runtime::Invocation, plan::PromptPlan};
use std::{fs, path::PathBuf};
struct Tree(PathBuf);
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
#[cfg(unix)]
fn planning_refuses_a_round_alias_that_escapes_the_task() {
    use std::os::unix::fs::symlink;
    let root =
        Tree(std::env::temp_dir().join(format!("revust-plan-containment-{}", std::process::id())));
    fs::create_dir_all(root.0.join("tasks/task")).unwrap();
    fs::create_dir_all(root.0.join("outside/input")).unwrap();
    fs::write(
        root.0.join("outside/input/scope.md"),
        "owned synthetic input",
    )
    .unwrap();
    symlink(root.0.join("outside"), root.0.join("tasks/task/01")).unwrap();
    let inv = Invocation {
        config_dir: Some(root.0.join("user")),
        task: "task".into(),
        run: "01".into(),
        overrides: vec![(
            "tasks-dir".into(),
            root.0.join("tasks").to_str().unwrap().into(),
        )],
        ..Default::default()
    };
    let result = PromptPlan::resolve(&root.0, None, &inv, &[]);
    assert!(matches!(result,Err(error) if error.contains("round directory escapes task")));
    assert!(!root.0.join("outside/manifest.json").exists());
}

#[test]
#[cfg(unix)]
fn planning_freezes_round_inputs_once_with_contained_bounded_reads() {
    use std::os::unix::fs::symlink;
    let root =
        Tree(std::env::temp_dir().join(format!("revust-plan-freeze-{}", std::process::id())));
    let input = root.0.join("tasks/task/01/input");
    fs::create_dir_all(input.join("context/nested")).unwrap();
    fs::write(root.0.join("outside.md"), "outside the round").unwrap();
    fs::write(input.join("scope.md"), "scope").unwrap();
    fs::write(input.join("goal.md"), "").unwrap();
    fs::write(input.join("context/nested/a.md"), "a").unwrap();
    symlink("nested/a.md", input.join("context/alias.md")).unwrap();
    let resolve = || {
        let (sender, receiver) = std::sync::mpsc::channel();
        let root = root.0.clone();
        std::thread::spawn(move || {
            let inv = Invocation {
                config_dir: Some(root.join("user")),
                task: "task".into(),
                run: "01".into(),
                overrides: vec![(
                    "tasks-dir".into(),
                    root.join("tasks").to_str().unwrap().into(),
                )],
                ..Default::default()
            };
            let _ = sender.send(
                PromptPlan::resolve(&root, None, &inv, &[]).and_then(|plan| plan.into_execution()),
            );
        });
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("input freezing must not block on a FIFO")
    };
    let plan = resolve().unwrap();
    let frozen: Vec<_> = plan
        .input_snapshots()
        .iter()
        .map(|s| {
            (
                s.relative.to_str().unwrap().to_owned(),
                s.content
                    .as_deref()
                    .map(|b| String::from_utf8_lossy(b).into_owned()),
            )
        })
        .collect();
    let file = |path: &str, text: &str| (path.to_owned(), Some(text.to_owned()));
    let directory = |path: &str| (path.to_owned(), None);
    assert_eq!(
        frozen,
        [
            file("scope.md", "scope"),
            file("goal.md", ""),
            directory("context"),
            file("context/alias.md", "a"),
            directory("context/nested"),
            file("context/nested/a.md", "a"),
        ]
    );
    // Empty documents are frozen for saved views but not provided to providers.
    assert!(plan.input_paths().goal.is_none());
    assert!(!root.0.join("tasks/task/01/prompts").exists());
    // A non-UTF-8 context name (where the filesystem permits one) is frozen
    // natively and the dry-run plan still serializes.
    let native = input.join(
        <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"context/bad-\xff.md"),
    );
    if fs::write(&native, "native").is_ok() {
        let inv = Invocation {
            config_dir: Some(root.0.join("user")),
            task: "task".into(),
            run: "01".into(),
            overrides: vec![(
                "tasks-dir".into(),
                root.0.join("tasks").to_str().unwrap().into(),
            )],
            ..Default::default()
        };
        let plan = PromptPlan::resolve(&root.0, None, &inv, &[]).unwrap();
        serde_json::to_string(&plan).unwrap();
        fs::remove_file(&native).unwrap();
    }
    // An existing empty context is frozen but not provided.
    let empty = root.0.join("tasks/task/02/input/context");
    fs::create_dir_all(&empty).unwrap();
    fs::write(empty.parent().unwrap().join("scope.md"), "scope").unwrap();
    let inv = Invocation {
        config_dir: Some(root.0.join("user")),
        task: "task".into(),
        run: "02".into(),
        overrides: vec![(
            "tasks-dir".into(),
            root.0.join("tasks").to_str().unwrap().into(),
        )],
        ..Default::default()
    };
    let plan = PromptPlan::resolve(&root.0, None, &inv, &[])
        .unwrap()
        .into_execution()
        .unwrap();
    assert!(plan.input_paths().context.is_none());
    assert!(
        plan.input_snapshots()
            .iter()
            .any(|s| s.relative == std::path::Path::new("context") && s.content.is_none())
    );
    fs::remove_dir_all(root.0.join("tasks/task/02")).unwrap();
    // Links leaving the round, non-regular inputs and oversized inventories fail
    // planning explicitly instead of being read or blocking.
    symlink("../../../../outside.md", input.join("context/escape.md")).unwrap();
    assert!(resolve().is_err_and(|e| e.contains("input/context/escape.md")));
    fs::remove_file(input.join("context/escape.md")).unwrap();
    nix::unistd::mkfifo(
        &input.join("goal.md.fifo"),
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    fs::remove_file(input.join("goal.md")).unwrap();
    fs::rename(input.join("goal.md.fifo"), input.join("goal.md")).unwrap();
    assert!(resolve().is_err_and(|e| e.contains("expected regular file")));
    fs::remove_file(input.join("goal.md")).unwrap();
    for index in 0..1024 {
        fs::write(input.join(format!("context/nested/{index}")), "").unwrap();
    }
    assert!(resolve().is_err_and(|e| e.contains("exceeds 1024 entries")));
}
