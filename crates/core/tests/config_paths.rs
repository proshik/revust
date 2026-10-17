use revust_core::config::paths::{Layers, profile_fallback, work_dir};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "revust-paths-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        for dir in ["home", "project", "review"] {
            fs::create_dir(root.join(dir)).unwrap();
        }
        Self(root)
    }
    fn path(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

// These protect legacy path semantics, not the private representation of a layer.
#[test]
fn roots_are_resolved_from_caller_cwd_without_creating_them() {
    let tree = Tree::new();
    let cwd = tree.path("project");
    let home = tree.path("home");
    let layers = Layers::resolve(&cwd, Some(&home), None).unwrap();
    assert_eq!(layers.user, home.join(".config/revust"));
    assert_eq!(layers.project, None);
    assert!(!layers.user.exists());
    assert!(!cwd.join(".revust").exists());
    fs::create_dir(cwd.join(".revust")).unwrap();
    fs::create_dir(tree.path("review/.revust")).unwrap();
    let layers = Layers::resolve(&cwd, None, Some(Path::new("../alternate/./config/.."))).unwrap();
    assert_eq!(layers.user, tree.path("alternate"));
    assert_eq!(layers.project, Some(cwd.join(".revust")));
    assert_eq!(
        work_dir(&cwd, Some(Path::new("../review"))).unwrap(),
        tree.path("review")
    );
    assert_eq!(work_dir(&cwd, None).unwrap(), cwd);
    assert!(!layers.user.exists());
}

#[test]
fn missing_home_requires_an_explicit_config_directory() {
    let tree = Tree::new();
    assert!(Layers::resolve(&tree.path("project"), None, None).is_err());
    assert!(Layers::resolve(Path::new("relative"), Some(&tree.path("home")), None).is_err());
    fs::write(tree.path("project/.revust"), "not a directory").unwrap();
    let layers = Layers::resolve(&tree.path("project"), Some(&tree.path("home")), None).unwrap();
    assert_eq!(layers.project, None);
    assert!(work_dir(&tree.path("project"), Some(Path::new(".revust"))).is_err());
    assert!(work_dir(&tree.path("project"), Some(Path::new("missing"))).is_err());
}

#[test]
fn project_calibration_is_optional_but_a_directory_is_an_error() {
    let tree = Tree::new();
    let cwd = tree.path("project");
    fs::create_dir(cwd.join(".revust")).unwrap();
    let profile = cwd.join(".revust/profile.md");
    assert_eq!(profile_fallback(&cwd).unwrap(), None);
    fs::write(&profile, "").unwrap();
    assert_eq!(profile_fallback(&cwd).unwrap(), None);
    fs::write(&profile, " \n").unwrap();
    assert_eq!(profile_fallback(&cwd).unwrap(), Some(profile.clone()));
    fs::remove_file(&profile).unwrap();
    fs::create_dir(&profile).unwrap();
    assert!(profile_fallback(&cwd).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_equivalent_roots_collapse_without_losing_project_calibration() {
    use std::os::unix::fs::symlink;
    let tree = Tree::new();
    let cwd = tree.path("project");
    fs::create_dir(cwd.join(".revust")).unwrap();
    fs::write(cwd.join(".revust/profile.md"), "Synthetic calibration\n").unwrap();
    symlink(cwd.join(".revust"), tree.path("alias")).unwrap();
    let layers = Layers::resolve(&cwd, None, Some(&tree.path("alias"))).unwrap();
    assert_eq!(layers.user, tree.path("alias"));
    assert_eq!(layers.project, None);
    assert_eq!(
        profile_fallback(&cwd).unwrap(),
        Some(cwd.join(".revust/profile.md"))
    );
    assert!(tree.path("alias").is_symlink());
}

#[test]
fn discovery_ignores_unrelated_trees_and_explicit_directories_remain_arbitrary() {
    let tree = Tree::new();
    let cwd = tree.path("project");
    let home = tree.path("home");
    fs::create_dir_all(home.join(".config/unrelated-tool")).unwrap();
    fs::create_dir(cwd.join("authored-config")).unwrap();
    fs::write(
        cwd.join("authored-config/profile.md"),
        "Synthetic unrelated calibration",
    )
    .unwrap();
    let discovered = Layers::resolve(&cwd, Some(&home), None).unwrap();
    assert_eq!(discovered.user, home.join(".config/revust"));
    assert_eq!(discovered.project, None);
    assert_eq!(profile_fallback(&cwd).unwrap(), None);
    let selected = Layers::resolve(&cwd, None, Some(Path::new("authored-config"))).unwrap();
    assert_eq!(selected.user, cwd.join("authored-config"));
}
