use revust_core::{
    config::{extensions::ExtensionSet, paths::Layers},
    prompts::PromptSet,
};
use std::{fs, path::PathBuf, time::SystemTime};
struct Tree(PathBuf);
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn setup() -> (Tree, Layers) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let unique = format!("{timestamp}-{sequence}");
    let tree =
        Tree(std::env::temp_dir().join(format!("revust-profiles-{}-{unique}", std::process::id())));
    fs::create_dir_all(tree.0.join("extension-profiles")).unwrap();
    let layers = Layers {
        user: tree.0.clone(),
        project: None,
    };
    (tree, layers)
}

#[test]
fn mixed_extension_roster_uses_structured_models_without_changing_legacy_catalogue() {
    let (tree, layers) = setup();
    fs::write(
        tree.0.join("extensions.json"),
        r#"{"version":1,"profiles":{"mixed":{"file":"extension-profiles/mixed.md"}}}"#,
    )
    .unwrap();
    fs::write(tree.0.join("extension-profiles/mixed.md"), "---\ndescription: Mixed review\nrunner:\n  executor: opencode\n  model: provider/model:revision\n  effort: high\nagents:\n  - name: open\n    lenses: [bugs]\n  - name: claude\n    lenses: [tests]\n    runner:\n      executor: claude\nstages:\n  verify:\n    executor: opencode\n    model: provider/verify\n---\nReview {{SCOPE}}\n").unwrap();
    let legacy = PromptSet::load(&layers).unwrap();
    let before = serde_json::to_value(legacy.catalog()).unwrap();
    let set = ExtensionSet::load(&layers, &legacy).unwrap();
    let mixed = &set.profiles["mixed"];
    assert_eq!(mixed.runner.model, "provider/model:revision");
    assert_eq!(mixed.agents[0].runner.executor, "opencode");
    assert_eq!(mixed.agents[1].runner.executor, "claude");
    assert!(mixed.agents[1].runner.model.is_empty());
    assert_eq!(mixed.agents[1].runner.effort, "high");
    assert_eq!(mixed.stages["verify"].model, "provider/verify");
    assert!(!legacy.profile_names().any(|name| name == "mixed"));
    assert_eq!(
        before,
        serde_json::to_value(PromptSet::load(&layers).unwrap().catalog()).unwrap()
    );
    let vars = [
        ("SCOPE".to_owned(), "/scope.md".to_owned()),
        ("WORKDIR".to_owned(), "/work".to_owned()),
    ]
    .into_iter()
    .collect();
    let composed = legacy
        .compose_body(&mixed.body, &mixed.agents[0], &vars, "")
        .unwrap();
    assert!(composed.starts_with("Review /scope.md\n\n"));
}

#[test]
fn missing_unknown_and_shadowing_profile_references_fail_during_load() {
    let (tree, layers) = setup();
    let legacy = PromptSet::load(&layers).unwrap();
    for text in [
        r#"{"version":1,"profiles":{"comprehensive":{"file":"extension-profiles/other.md"}}}"#,
        r#"{"version":1,"profiles":{"unknown":{"policy":{}}}}"#,
        r#"{"version":1,"profiles":{"missing":{"file":"extension-profiles/missing.md"}}}"#,
        r#"{"version":1,"profiles":{"outside":{"file":"../private.md"}}}"#,
    ] {
        fs::write(tree.0.join("extensions.json"), text).unwrap();
        assert!(ExtensionSet::load(&layers, &legacy).is_err());
    }
}
