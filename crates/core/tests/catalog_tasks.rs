use revust_core::config::{catalog::Catalog, runtime::Invocation};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn put(root: &Path, name: &str, value: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

#[test]
fn inventory_retains_task_metadata_and_lists_only_recorded_real_rounds() {
    let root = std::env::temp_dir().join(format!("revust-task-catalog-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup<'a>(&'a Path);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            fs::remove_dir_all(self.0).unwrap();
        }
    }
    let _cleanup = Cleanup(&root);
    let tasks = root.join(".revust/tasks");
    put(
        &tasks,
        "alpha/task.md",
        "---\ndescription: Synthetic task\nbranch: feature/test\n---\n",
    );
    put(&tasks, "alpha/01/manifest.json", "{}");
    put(&tasks, "alpha/02/manifest.json", "");
    put(&tasks, "alpha/03/input/scope.md", "Prepared only");
    put(&tasks, "broken/task.md", "---\ntypo: rejected\n---\n");
    put(&tasks, "broken/01/manifest.json", "{}");
    put(&tasks, "outside.txt", "not a task");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink("alpha", tasks.join("alias")).unwrap();
        symlink(
            root.join(".revust/tasks/alpha"),
            tasks.join("absolute-alias"),
        )
        .unwrap();
        symlink("01", tasks.join("alpha/round-alias")).unwrap();
        fs::create_dir(tasks.join("alpha/04")).unwrap();
        symlink("../01/manifest.json", tasks.join("alpha/04/manifest.json")).unwrap();
    }
    let before = fs::read_to_string(tasks.join("alpha/task.md")).unwrap();
    let invocation = Invocation {
        config_dir: Some(root.join("unused")),
        ..Default::default()
    };
    let data: Value =
        serde_json::to_value(Catalog::resolve(&root, None, &invocation).unwrap()).unwrap();
    let all = data["paths"]["tasks"].as_array().unwrap();
    let alpha = all.iter().find(|v| v["id"] == "alpha").unwrap();
    assert_eq!(
        *alpha,
        json!({"id":"alpha", "description":"Synthetic task", "branch":"feature/test", "url":"", "base":"", "rounds":["01"]})
    );
    let broken = all.iter().find(|v| v["id"] == "broken").unwrap();
    assert!(
        broken["meta_error"]
            .as_str()
            .unwrap()
            .contains("field typo not found")
    );
    assert_eq!(broken["rounds"], json!(["01"]));
    assert!(
        !all.iter()
            .any(|v| v["id"] == "absolute-alias" || v["id"] == "outside.txt")
    );
    #[cfg(unix)]
    assert!(
        all.iter()
            .any(|v| v["id"] == "alias" && v["rounds"] == json!(["01"]))
    );
    assert_eq!(
        fs::read_to_string(tasks.join("alpha/task.md")).unwrap(),
        before
    );
    assert!(!root.join("unused").exists());
}

#[cfg(unix)]
#[test]
fn rounds_whose_names_are_not_utf8_are_reported_instead_of_renamed() {
    use std::os::unix::ffi::OsStrExt;
    let root = std::env::temp_dir().join(format!("revust-task-bytes-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup<'a>(&'a Path);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            fs::remove_dir_all(self.0).unwrap();
        }
    }
    let _cleanup = Cleanup(&root);
    let tasks = root.join(".revust/tasks");
    put(&tasks, "alpha/01/manifest.json", "{}");
    // A lossy name would print, and later reopen, "r\u{fffd}" instead.
    let raw = tasks
        .join("alpha")
        .join(std::ffi::OsStr::from_bytes(b"r\xff"));
    match fs::create_dir(&raw) {
        // APFS and HFS+ refuse such names, so the case cannot arise there.
        Err(error) if error.raw_os_error() == Some(nix::libc::EILSEQ) => return,
        result => result.unwrap(),
    }
    fs::write(raw.join("manifest.json"), "{}").unwrap();
    put(&tasks, "alpha/r\u{fffd}/manifest.json", "{}");
    let invocation = Invocation {
        config_dir: Some(root.join("unused")),
        ..Default::default()
    };
    let data: Value =
        serde_json::to_value(Catalog::resolve(&root, None, &invocation).unwrap()).unwrap();
    let alpha = &data["paths"]["tasks"][0];
    assert_eq!(alpha["rounds"], json!(["01", "r\u{fffd}"]));
    assert!(
        alpha["rounds_error"]
            .as_str()
            .unwrap()
            .contains("not UTF-8"),
        "{alpha}"
    );
}
