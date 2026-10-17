//! Explicit removal of one named task, never implicit bulk reclamation.
use super::{
    check_name, error,
    layout::names,
    open_directory,
    stats::{megabytes, owned_size},
};
use nix::{
    errno::Errno,
    fcntl::{AtFlags, Flock, FlockArg, OFlag, openat},
    sys::stat::{FileStat, Mode, fstat, fstatat},
    unistd::{UnlinkatFlags, unlinkat},
};
use serde::Serialize;
use std::{
    fs::File,
    path::{Path, PathBuf},
};

#[derive(Serialize)]
pub struct CleanupResult {
    pub tasks_dir: PathBuf,
    pub removed: Vec<Removal>,
    pub total_mb_after: f64,
}
#[derive(Serialize)]
pub struct Removal {
    pub id: String,
    pub rounds: usize,
    pub size_mb: f64,
}

/// Check current marker owners, then remove only the named task. The idle check
/// cannot prevent a new review starting after it, and deletion is not atomic.
pub fn remove(root_path: &Path, task: &str) -> Result<CleanupResult, String> {
    check_name(task).map_err(|e| format!("select task: {e}; pass --task explicitly"))?;
    let root = super::reader::open_root(root_path)?;
    let entry = fstatat(&root, task, AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|e| format!("task {task} not found or unreadable: {e}"))?;
    if entry.st_mode & nix::libc::S_IFMT == nix::libc::S_IFLNK {
        return Err(format!(
            "task {task} is a symlink and will not be removed under this name"
        ));
    }
    let directory = open_directory(&root, Path::new(task))?;
    let identity = fstat(&directory).map_err(error)?;
    same_entry(&root, Path::new(task), &identity)?;
    // Hold all observed marker claims until removal finishes. This cannot lock
    // marker files that another process creates after this inventory.
    let claims = idle_claims(&directory)?;
    let rounds = claims
        .iter()
        .filter(|claim| claim.metadata().is_ok_and(|meta| meta.len() > 0))
        .count();
    let (bytes, _) = owned_size(&directory);
    same_entry(&root, Path::new(task), &identity)?;
    remove_open_directory(&root, Path::new(task), &directory)
        .map_err(|e| format!("remove task {task}: {e}; removal may be partial"))?;
    let (remaining, _) = owned_size(&root);
    Ok(CleanupResult {
        tasks_dir: root_path.into(),
        removed: vec![Removal {
            id: task.into(),
            rounds,
            size_mb: megabytes(bytes),
        }],
        total_mb_after: megabytes(remaining),
    })
}

fn idle_claims(task: &File) -> Result<Vec<Flock<File>>, String> {
    let mut claims = Vec::new();
    for name in names(task)? {
        let Ok(round) = open_directory(task, Path::new(&name)) else {
            continue;
        };
        let stat = match fstatat(&round, "manifest.json", AtFlags::AT_SYMLINK_NOFOLLOW) {
            Err(Errno::ENOENT) => continue,
            Err(e) => return Err(error(e)),
            Ok(stat) => stat,
        };
        if stat.st_mode & nix::libc::S_IFMT != nix::libc::S_IFREG {
            return Err("cannot establish ownership of nonregular manifest.json".into());
        }
        let file = File::from(
            openat(
                &round,
                "manifest.json",
                OFlag::O_WRONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(error)?,
        );
        let opened = fstat(&file).map_err(error)?;
        if opened.st_dev != stat.st_dev || opened.st_ino != stat.st_ino || opened.st_nlink != 1 {
            return Err("completion marker changed or is multiply linked".into());
        }
        claims.push(Flock::lock(file, FlockArg::LockExclusiveNonblock)
            .map_err(|(_, e)| format!("round {} is held by a running review or cannot be locked: {e}; nothing was removed", name.to_string_lossy()))?);
    }
    Ok(claims)
}

fn same_entry(parent: &File, name: &Path, identity: &FileStat) -> Result<(), String> {
    let entry = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(error)?;
    if entry.st_dev != identity.st_dev || entry.st_ino != identity.st_ino {
        return Err(format!("{} was replaced during cleanup", name.display()));
    }
    Ok(())
}

fn remove_open_directory(parent: &File, name: &Path, dir: &File) -> Result<(), String> {
    let identity = fstat(dir).map_err(error)?;
    same_entry(parent, name, &identity)?;
    for child in names(dir)? {
        let child = Path::new(&child);
        let entry = match fstatat(dir, child, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(entry) => entry,
            Err(Errno::ENOENT) => continue,
            Err(e) => return Err(error(e)),
        };
        if entry.st_mode & nix::libc::S_IFMT == nix::libc::S_IFDIR {
            let opened = open_directory(dir, child)?;
            remove_open_directory(dir, child, &opened)?;
        } else {
            // This unlinks the directory entry itself, including symlinks,
            // rather than following a link into another task or outside root.
            match unlinkat(dir, child, UnlinkatFlags::NoRemoveDir) {
                Ok(()) | Err(Errno::ENOENT) => (),
                Err(e) => return Err(error(e)),
            }
        }
    }
    same_entry(parent, name, &identity)?;
    unlinkat(parent, name, UnlinkatFlags::RemoveDir).map_err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn removal_stays_on_owned_root_and_refuses_a_substituted_task() {
        let temp =
            std::env::temp_dir().join(format!("revust-cleanup-owned-{}", std::process::id()));
        fs::create_dir(&temp).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(temp.clone());
        fs::create_dir_all(temp.join("tasks/selected/nested")).unwrap();
        fs::write(temp.join("tasks/selected/nested/file"), "owned").unwrap();
        fs::create_dir_all(temp.join("outside/selected")).unwrap();
        fs::write(temp.join("outside/selected/file"), "outside").unwrap();
        let root = File::open(temp.join("tasks")).unwrap();
        let task = open_directory(&root, Path::new("selected")).unwrap();
        fs::rename(temp.join("tasks"), temp.join("owned")).unwrap();
        symlink("outside", temp.join("tasks")).unwrap();
        remove_open_directory(&root, Path::new("selected"), &task).unwrap();
        assert!(!temp.join("owned/selected").exists());
        assert_eq!(
            fs::read(temp.join("outside/selected/file")).unwrap(),
            b"outside"
        );

        fs::create_dir(temp.join("owned/selected")).unwrap();
        fs::write(temp.join("owned/selected/file"), "original").unwrap();
        let task = open_directory(&root, Path::new("selected")).unwrap();
        fs::rename(temp.join("owned/selected"), temp.join("owned/renamed")).unwrap();
        fs::create_dir(temp.join("owned/selected")).unwrap();
        fs::write(temp.join("owned/selected/file"), "replacement").unwrap();
        assert!(
            remove_open_directory(&root, Path::new("selected"), &task)
                .unwrap_err()
                .contains("replaced")
        );
        assert_eq!(
            fs::read(temp.join("owned/renamed/file")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(temp.join("owned/selected/file")).unwrap(),
            b"replacement"
        );
    }
}
