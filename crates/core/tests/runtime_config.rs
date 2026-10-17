use revust_core::config::runtime::{Invocation, RuntimeConfig};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "revust-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn put(&self, path: &str, contents: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn explicit_values_keep_layer_origin_and_caller_paths() {
    let tree = Tree::new();
    tree.put(
        "user/config",
        "max-parallel=7\nidle-timeout=90s\nprofile=caller\n",
    );
    tree.put(".revust/config", "max-parallel=0\nauto-exit=0s\n");
    fs::create_dir(tree.0.join("review")).unwrap();
    let invocation = Invocation {
        config_dir: Some(tree.0.join("user")),
        workdir: Some("review".into()),
        overrides: vec![("idle-timeout".into(), "1.5s".into())],
        ..Default::default()
    };
    let data =
        serde_json::to_value(RuntimeConfig::resolve(&tree.0, None, &invocation).unwrap()).unwrap();
    for (key, value, origin) in [
        ("max-parallel", serde_json::json!(0), "project"),
        ("idle-timeout", serde_json::json!("1.5s"), "flag"),
        ("profile", serde_json::json!("caller"), "user"),
    ] {
        let knob = data["knobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == key)
            .unwrap();
        assert_eq!(knob["value"], value);
        assert_eq!(knob["source"], origin);
    }
    assert_eq!(
        data["paths"]["workdir"],
        tree.0.join("review").to_str().unwrap()
    );
    assert_eq!(
        data["paths"]["project_dir"],
        tree.0.join(".revust").to_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(tree.0.join(".revust/config")).unwrap(),
        "max-parallel=0\nauto-exit=0s\n"
    );
}

#[test]
fn malformed_assignment_and_unknown_options_are_rejected_even_when_masked() {
    let tree = Tree::new();
    for contents in [
        "unknown-setting=4\n",
        "profile=sample\nmax-parallel\n",
        "idle-timeout=\"unterminated\n",
    ] {
        tree.put("user/config", contents);
        let invocation = Invocation {
            config_dir: Some(tree.0.join("user")),
            ..Default::default()
        };
        assert!(
            RuntimeConfig::resolve(&tree.0, None, &invocation).is_err(),
            "{contents}"
        );
        assert_eq!(
            fs::read_to_string(tree.0.join("user/config")).unwrap(),
            contents
        );
    }
}

#[test]
fn unreadable_config_is_an_error_and_workdir_failure_is_inspectable() {
    let tree = Tree::new();
    fs::create_dir_all(tree.0.join(".revust/config")).unwrap();
    let invocation = Invocation {
        config_dir: Some(tree.0.join("unused")),
        workdir: Some("missing".into()),
        ..Default::default()
    };
    assert!(
        RuntimeConfig::resolve(&tree.0, None, &invocation)
            .unwrap_err()
            .contains("config")
    );
    fs::remove_dir(tree.0.join(".revust/config")).unwrap();
    let result =
        serde_json::to_value(RuntimeConfig::resolve(&tree.0, None, &invocation).unwrap()).unwrap();
    assert_eq!(result["paths"]["workdir"], "missing");
    assert!(
        result["paths"]["workdir_error"]
            .as_str()
            .unwrap()
            .contains("missing")
    );
    assert!(!tree.0.join("unused").exists());
    assert!(!tree.0.join(Path::new(".revust/tasks")).exists());
}
