use crate::{config::paths::Layers, prompts::PromptSet};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Snapshot-based replacement, serialized with other revust writers by an
/// adjacent advisory lock. Editors that ignore that lock are checked immediately
/// before rename, but cannot be given a kernel-level CAS guarantee by std::fs.
pub struct Edit {
    path: PathBuf,
    expected: Option<Vec<u8>>,
}
impl Edit {
    pub fn read(path: &Path) -> Result<Self, String> {
        if path
            .file_name()
            .is_none_or(|name| name != "extensions.json")
        {
            return Err("extension edits may only target extensions.json".into());
        }
        Ok(Self {
            path: path.to_owned(),
            expected: read(path)?,
        })
    }
    pub fn replace(
        self,
        candidate: &str,
        layers: &Layers,
        prompts: &PromptSet,
    ) -> Result<(), String> {
        if self.path != layers.user.join("extensions.json")
            && layers
                .project
                .as_ref()
                .is_none_or(|p| self.path != p.join("extensions.json"))
        {
            return Err("edit target is not an active extension layer".into());
        }
        super::ExtensionSet::load_replacing(layers, prompts, Some((&self.path, candidate)))?;
        let parent = self.path.parent().ok_or("extension file has no parent")?;
        let lock_path = parent.join(".extensions.json.lock");
        let lock = open_private(&lock_path, false).map_err(|e| format!("open config lock: {e}"))?;
        lock.try_lock()
            .map_err(|e| format!("configuration is locked: {e}"))?;
        // Keep the inode stable: unlinking the advisory lock would let a second
        // writer lock a new inode while a waiting writer still owns the old one.
        if read(&self.path)? != self.expected {
            return Err("configuration changed since it was read".into());
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temporary = parent.join(format!(
            ".extensions-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = open_private(&temporary, true)
            .map_err(|e| format!("create config temporary file: {e}"))?;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }
        let _cleanup = Cleanup(temporary.clone());
        if let Ok(metadata) = fs::metadata(&self.path) {
            file.set_permissions(metadata.permissions())
                .map_err(|e| e.to_string())?;
        }
        file.write_all(candidate.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("write config temporary file: {e}"))?;
        if read(&self.path)? != self.expected {
            return Err("configuration changed before replacement".into());
        }
        fs::rename(&temporary, &self.path).map_err(|e| format!("replace config: {e}"))?;
        Ok(())
    }
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("stat {}: {error}", path.display())),
        Ok(metadata) if !metadata.is_file() => {
            return Err(
                "extension edit target must be a regular file, not a symlink or directory".into(),
            );
        }
        Ok(_) => (),
    }
    fs::read(path)
        .map(Some)
        .map_err(|e| format!("read {}: {e}", path.display()))
}
fn open_private(path: &Path, exclusive: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if exclusive {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
