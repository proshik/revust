use std::process::Command;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "revust-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn incomplete_or_unknown_commands_fail_without_stdout() {
    let tree = Tree::new();
    for (args, diagnostic) in [
        (vec![], "--task is required"),
        (vec!["review"], "unsupported argument"),
        (
            vec![
                "config",
                "--runtime-only",
                "--config-dir",
                "/nonexistent",
                "--typo=1",
            ],
            "unknown option: typo",
        ),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_revust"))
            .current_dir(&tree.0)
            .args(args)
            .env_clear()
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(result.stdout.is_empty());
        assert!(
            String::from_utf8(result.stderr)
                .unwrap()
                .contains(diagnostic)
        );
    }
}

#[test]
fn explicit_config_root_and_split_flags_work_without_home_or_providers() {
    let tree = Tree::new();
    let result = Command::new(env!("CARGO_BIN_EXE_revust"))
        .current_dir(&tree.0)
        .args([
            "--config-dir",
            "/nonexistent",
            "config",
            "--runtime-only",
            "--auto-exit",
            "0s",
            "--max-parallel=2",
            "--max-parallel",
            "3",
        ])
        .env_clear()
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    for (name, expected) in [
        ("auto-exit", serde_json::json!("0s")),
        ("max-parallel", serde_json::json!(3)),
    ] {
        let knob = output["knobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["name"] == name)
            .unwrap();
        assert_eq!(knob["value"], expected);
        assert_eq!(knob["source"], "flag");
    }
    assert!(result.stderr.is_empty());
}

#[test]
fn help_explains_the_available_read_only_surface() {
    let tree = Tree::new();
    let result = Command::new(env!("CARGO_BIN_EXE_revust"))
        .current_dir(&tree.0)
        .arg("--help")
        .env_clear()
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("config --runtime-only")
    );
    assert!(result.stdout.is_empty());
}

#[test]
fn effective_inspection_applies_extensions_and_redacts_child_environment() {
    let tree = Tree::new();
    fs::create_dir(tree.0.join(".revust")).unwrap();
    fs::write(
        tree.0.join(".revust/extensions.json"),
        r#"{"version":1,"policy":{
        "budget":{"max_calls":4},"output":{"tui":false},
        "executors":{"opencode":{"env":{"TOKEN":"synthetic-secret"}}}}}"#,
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_revust"))
        .current_dir(&tree.0)
        .args(["config", "--effective", "--config-dir=.revust"])
        .env_clear()
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["policy"]["values"]["budget"]["max_calls"], 4);
    assert_eq!(result["policy"]["values"]["output"]["tui"], false);
    assert!(!result.to_string().contains("synthetic-secret"));
    assert!(!tree.0.join(".revust/tasks").exists());
}

#[test]
fn schema_export_does_not_require_home_or_a_configuration_tree() {
    let tree = Tree::new();
    let result = Command::new(env!("CARGO_BIN_EXE_revust"))
        .current_dir(&tree.0)
        .args(["config", "--extension-schema"])
        .env_clear()
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let schema: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(schema["properties"]["version"]["const"], 1);
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(fs::read_dir(&tree.0).unwrap().count(), 0);
}

#[test]
fn dry_run_applies_extension_executor_options_and_disabled_stages() {
    let tree = Tree::new();
    let config = tree.0.join(".revust");
    fs::create_dir_all(config.join("tasks/task/run/input")).unwrap();
    fs::write(config.join("tasks/task/run/input/scope.md"), "scope").unwrap();
    fs::write(config.join("extensions.json"),r#"{"version":1,"policy":{
        "budget":{"max_calls":4},"stages":{"verify":{"enabled":false}},
        "executors":{"codex":{"executable":"./custom/codex","options":{"sandbox":"workspace-write"}}}}}"#).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_revust"))
        .current_dir(&tree.0)
        .args([
            "dry-run",
            "--task=task",
            "--run=run",
            "--profile=codex-only",
            "--config-dir=.revust",
            "--no-tui",
            "--markdown",
        ])
        .env_clear()
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        result["effective_policy"]["values"]["budget"]["max_calls"],
        4
    );
    assert_eq!(
        result["find"][0]["request"]["program"],
        config.join("./custom/codex").to_str().unwrap()
    );
    assert_eq!(result["find"][0]["request"]["argv"][3], "workspace-write");
    assert_eq!(result["effective_policy"]["values"]["output"]["tui"], false);
    assert_eq!(
        result["effective_policy"]["values"]["output"]["format"],
        "markdown"
    );
    assert_eq!(result["stages"][1]["enabled"], false);
    assert!(result["stages"][1]["request_template"].is_null());
    assert!(!config.join("tasks/task/run/manifest.json").exists());
}

#[test]
fn dry_run_preserves_context_paths_and_plans_calibration_without_copying() {
    let tree = Tree::new();
    let input = tree.0.join("tasks/demo/first/input");
    let config = tree.0.join(".revust");
    fs::create_dir_all(&input).unwrap();
    fs::create_dir_all(config.join("prompts/profiles")).unwrap();
    fs::write(
        input.join("scope.md"),
        "DO NOT paste the scope into the prompt",
    )
    .unwrap();
    fs::write(input.join("goal.md"), "").unwrap();
    fs::create_dir(input.join("context")).unwrap();
    fs::write(config.join("profile.md"), "project calibration").unwrap();
    fs::write(config.join("prompts/profiles/simple.md"),
        "---\nmodel: claude/test:high\nagents:\n  - name: reviewer\n    lenses: [bugs]\n---\n{{SCOPE}}\n{{GOAL}}\n{{PROFILE}}\n{{CONTEXT}}\n{{WORKDIR}}\n").unwrap();
    // Placeholder-looking paths must survive one-pass substitution unchanged.
    let workdir = tree.0.join("{{FINDINGS}}");
    fs::create_dir(&workdir).unwrap();
    fs::write(
        config.join("prompts/synthesis.md"),
        "{{WORKDIR}} {{FINDINGS}}",
    )
    .unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_revust"))
            .current_dir(&tree.0)
            .args([
                "dry-run",
                "--task=demo",
                "--run=first",
                "--tasks-dir=tasks",
                "--config-dir=.revust",
                "--profile=simple",
            ])
            .arg("--workdir")
            .arg(&workdir)
            .env_clear()
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let body = plan["find"][0]["body"].as_str().unwrap();
    assert!(body.starts_with(input.join("scope.md").to_str().unwrap()));
    assert!(!body.contains("DO NOT paste"));
    assert_eq!(plan["variables"]["GOAL"], "none provided");
    assert_eq!(plan["variables"]["CONTEXT"], "none provided");
    assert_eq!(
        plan["stages"][0]["body_template"],
        format!("{} {{{{FINDINGS}}}}", workdir.display())
    );
    assert_eq!(
        plan["stages"][0]["deferred"],
        serde_json::json!(["FINDINGS"])
    );
    assert!(!input.parent().unwrap().join("prompts").exists());
    assert!(!input.parent().unwrap().join("manifest.json").exists());
    // A round-specific calibration masks even an invalid project fallback.
    fs::remove_file(config.join("profile.md")).unwrap();
    fs::create_dir(config.join("profile.md")).unwrap();
    fs::write(input.join("profile.md"), "round calibration").unwrap();
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(plan["profile_snapshot"].is_null());
    assert_eq!(
        plan["variables"]["PROFILE"],
        input.join("profile.md").to_str().unwrap()
    );
}

#[test]
fn effective_inspection_applies_review_flags_without_a_round() {
    let tree = Tree::new();
    let out = Command::new(env!("CARGO_BIN_EXE_revust"))
        .current_dir(&tree.0)
        .env_clear()
        .env("HOME", &tree.0)
        .args([
            "config",
            "--effective",
            "--min-confidence=-1",
            "--no-verify",
            "--no-tui",
            "--markdown",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let inspection: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(inspection["policy"]["values"]["output"]["tui"], false);
    assert_eq!(
        inspection["policy"]["values"]["output"]["format"],
        "markdown"
    );
    assert_eq!(
        inspection["policy"]["values"]["verification"]["min_confidence"],
        -1
    );
    assert_eq!(
        inspection["policy"]["values"]["stages"]["verify"]["enabled"],
        false
    );
    assert_eq!(
        inspection["policy"]["origins"]["/verification/min_confidence"]["kind"],
        "legacy"
    );
    assert_eq!(fs::read_dir(&tree.0).unwrap().count(), 0);
}
