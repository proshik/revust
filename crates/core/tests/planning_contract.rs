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
