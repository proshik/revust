#![cfg(unix)]
use revust_core::prompts::materialize::dump_defaults;
use std::{
    fs,
    io::{self, Write},
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
};

struct SwapDestination {
    root: PathBuf,
    swapped: bool,
}
impl Write for SwapDestination {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.swapped {
            fs::rename(self.root.join("exported"), self.root.join("owned"))?;
            symlink("outside", self.root.join("exported"))?;
            self.swapped = true;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for SwapDestination {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn replacing_destination_during_export_keeps_writes_on_owned_root() {
    let mut diagnostics = SwapDestination {
        root: std::env::temp_dir().join(format!("revust-export-{}", std::process::id())),
        swapped: false,
    };
    fs::create_dir(&diagnostics.root).unwrap();
    fs::create_dir(diagnostics.root.join("outside")).unwrap();
    dump_defaults(&diagnostics.root.join("exported"), &mut diagnostics).unwrap();
    assert!(diagnostics.swapped);
    assert_eq!(
        fs::read_dir(diagnostics.root.join("outside"))
            .unwrap()
            .count(),
        0
    );
    let owned = diagnostics.root.join("owned");
    assert_eq!(
        fs::read(owned.join("prompts/verify.md")).unwrap(),
        include_bytes!("../defaults/prompts/verify.md")
    );
    assert_eq!(fs::read_dir(owned.join("lenses")).unwrap().count(), 13);
    assert_eq!(
        fs::read_dir(owned.join("prompts/profiles"))
            .unwrap()
            .count(),
        8
    );
    assert!(!owned.join("config").exists());
    // Existing umask may remove permissions, but the exporter must not add
    // group/other access to files or world access to created directories.
    assert_eq!(
        fs::metadata(owned.join("prompts/verify.md"))
            .unwrap()
            .permissions()
            .mode()
            & 0o077,
        0
    );
    assert_eq!(fs::metadata(owned).unwrap().permissions().mode() & 0o007, 0);
}

#[test]
fn initialization_preserves_assignments_and_raw_profile_bytes_before_export() {
    use revust_core::{config::runtime::Invocation, prompts::materialize::initialize};
    let root = std::env::temp_dir().join(format!("revust-materialize-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let project = root.join("project");
    let home = root.join("home");
    fs::create_dir_all(project.join(".revust/lenses")).unwrap();
    fs::create_dir_all(home.join(".config/revust/lenses")).unwrap();
    let raw = b"# Authored lens\r\nKeep original whitespace unchanged.\r\n";
    fs::write(home.join(".config/revust/lenses/bugs.md"), raw).unwrap();
    let config = project.join(".revust/config");
    fs::write(&config, b"max-parallel = invalid-on-purpose\n").unwrap();
    fs::write(
        project.join(".revust/lenses/docs.md"),
        b"Authored project document\n",
    )
    .unwrap();
    let paths = initialize(&project, Some(&home), &Invocation::default()).unwrap();
    assert_eq!(
        fs::read(&config).unwrap(),
        b"max-parallel = invalid-on-purpose\n"
    );
    assert_eq!(
        fs::read(project.join(".revust/lenses/bugs.md")).unwrap(),
        raw
    );
    assert_eq!(
        fs::read(project.join(".revust/lenses/docs.md")).unwrap(),
        b"Authored project document\n"
    );
    assert!(
        paths
            .files
            .iter()
            .any(|file| file.layer == "user" && file.created)
    );
    assert!(
        initialize(&project, Some(&home), &Invocation::default())
            .unwrap()
            .files
            .iter()
            .all(|file| !file.created)
    );
    fs::write(&config, b"# template = comment\n; comment\n[section]\n").unwrap();
    initialize(&project, Some(&home), &Invocation::default()).unwrap();
    assert_eq!(
        fs::read(&config).unwrap(),
        include_bytes!("../templates/config")
    );
}

#[test]
fn invalid_profile_fails_before_materialization_and_existing_leaf_alias_survives_export() {
    use revust_core::{config::runtime::Invocation, prompts::materialize::initialize};
    let root =
        std::env::temp_dir().join(format!("revust-export-validation-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let home = root.join("home");
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    let profiles = home.join(".config/revust/prompts/profiles");
    fs::create_dir_all(&profiles).unwrap();
    fs::write(profiles.join("invalid.md"), "---\nagents: [\n---\n").unwrap();
    assert!(initialize(&project, Some(&home), &Invocation::default()).is_err());
    assert!(!project.join(".revust").exists());
    let export = root.join("export");
    fs::create_dir_all(export.join("lenses")).unwrap();
    symlink("missing", export.join("lenses/bugs.md")).unwrap();
    dump_defaults(&export, &mut Vec::new()).unwrap();
    assert_eq!(
        fs::read_link(export.join("lenses/bugs.md")).unwrap(),
        PathBuf::from("missing")
    );
}
