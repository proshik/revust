//! Explicit prompt extraction; all writes stay relative to an owned root.
use crate::config::{paths::Layers, runtime::Invocation};
use nix::{
    errno::Errno,
    fcntl::{AtFlags, OFlag, openat, readlinkat},
    sys::stat::{Mode, fstatat, mkdirat},
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    fs::{DirBuilder, File, OpenOptions},
    io::{Read, Seek, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Serialize)]
pub struct InitPaths {
    pub dir: PathBuf,
    pub config: PathBuf,
    pub files: Vec<InitFile>,
}
#[derive(Serialize)]
pub struct InitFile {
    pub path: PathBuf,
    pub layer: &'static str,
    pub created: bool,
}

/// Resolve all content before opening a writer. Initialization is tied to the
/// caller project, independently of the review directory and selected profile.
pub fn initialize(
    cwd: &Path,
    home: Option<&Path>,
    invocation: &Invocation,
) -> Result<InitPaths, String> {
    let layers =
        Layers::resolve(cwd, home, invocation.config_dir.as_deref()).map_err(|e| e.to_string())?;
    let resolved = super::PromptSet::load(&layers)?;
    let entries = resolved
        .provenance()
        .into_iter()
        .map(|origin| {
            let content = resolved
                .content(&origin.path)
                .ok_or_else(|| format!("resolved prompt {} has no content", origin.path))?;
            Ok((origin.path, origin.layer, content.as_bytes().to_vec()))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let destination = cwd.join(".revust");
    let writer = TreeWriter::open(&destination)?;
    let config = writer.read_config()?;
    match config.as_ref() {
        Some(snapshot) if has_assignment(&snapshot.bytes) => (),
        expected => writer.replace_config(expected, include_bytes!("../../templates/config"))?,
    }
    let mut files = Vec::with_capacity(entries.len());
    for (relative, layer, bytes) in entries {
        let created = writer.write(Path::new(&relative), &bytes)?;
        files.push(InitFile {
            path: destination.join(relative),
            layer,
            created,
        });
    }
    Ok(InitPaths {
        config: destination.join("config"),
        dir: destination,
        files,
    })
}

fn has_assignment(bytes: &[u8]) -> bool {
    for line in String::from_utf8_lossy(bytes).lines() {
        let trimmed = line.trim();
        match trimmed.chars().next() {
            None | Some('#' | ';' | '[') => continue,
            _ if trimmed.split_once('=').is_some() => return true,
            _ => (),
        }
    }
    false
}

struct ConfigSnapshot {
    bytes: Vec<u8>,
    identity: (u64, u64),
}

/// Export the shipped suite without loading or modifying caller profiles.
pub fn dump_defaults(destination: &Path, diagnostics: &mut dyn Write) -> Result<(), String> {
    let writer = TreeWriter::open(destination)?;
    for (relative, content) in super::defaults::FILES.iter().copied() {
        let outcome = if writer.write(Path::new(relative), content.as_bytes())? {
            "created"
        } else {
            "preserved existing entry"
        };
        writeln!(
            diagnostics,
            "{outcome}: {}",
            writer.path.join(relative).display()
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

struct TreeWriter {
    root: File,
    path: PathBuf,
}
impl TreeWriter {
    fn read_config(&self) -> Result<Option<ConfigSnapshot>, String> {
        let path = Path::new("config");
        match fstatat(&self.root, path, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Err(Errno::ENOENT) => return Ok(None),
            Err(e) => return Err(e.to_string()),
            Ok(stat) if stat.st_mode & nix::libc::S_IFMT != nix::libc::S_IFREG => {
                return Err("config: not a regular file".into());
            }
            Ok(_) => (),
        }
        let mut file = File::from(
            openat(
                &self.root,
                path,
                OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| format!("read config: {e}"))?,
        );
        read_snapshot(&mut file).map(Some)
    }

    fn replace_config(
        &self,
        expected: Option<&ConfigSnapshot>,
        bytes: &[u8],
    ) -> Result<(), String> {
        let Some(expected) = expected else {
            if !self.write(Path::new("config"), bytes)? {
                return Err("config changed since it was read".into());
            }
            return Ok(());
        };
        // No truncation until both identity and bytes have been rechecked.
        // Noncooperating editors cannot be given kernel-level compare-and-swap.
        let mut file = File::from(
            openat(
                &self.root,
                Path::new("config"),
                OFlag::O_RDWR | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| format!("open config for replacement: {e}"))?,
        );
        file.try_lock().map_err(|e| format!("lock config: {e}"))?;
        let current = read_snapshot(&mut file)?;
        if current.identity != expected.identity || current.bytes != expected.bytes {
            return Err("config changed since it was read".into());
        }
        file.rewind()
            .and_then(|_| file.set_len(0))
            .and_then(|_| file.write_all(bytes))
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("write config: {e}"))
    }

    fn open(path: &Path) -> Result<Self, String> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o750)
            .create(path)
            .map_err(|e| format!("create {}: {e}", path.display()))?;
        let root = OpenOptions::new()
            .read(true)
            .custom_flags(OFlag::O_DIRECTORY.bits() | OFlag::O_CLOEXEC.bits())
            .open(path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        Ok(Self {
            root,
            path: path.to_owned(),
        })
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<bool, String> {
        let error = |e: String| format!("write {}: {e}", self.path.join(path).display());
        let name = path
            .file_name()
            .ok_or_else(|| error("missing file name".into()))?;
        let parent = self
            .parent(path.parent().unwrap_or(Path::new("")))
            .map_err(error)?;
        match fstatat(&parent, Path::new(name), AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(_) => return Ok(false),
            Err(Errno::ENOENT) => (),
            Err(e) => return Err(error(e.to_string())),
        }
        let mut file = File::from(
            openat(
                &parent,
                Path::new(name),
                OFlag::O_WRONLY
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(|e| error(e.to_string()))?,
        );
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| error(e.to_string()))?;
        parent.sync_all().map_err(|e| error(e.to_string()))?;
        Ok(true)
    }

    // Keep every traversed directory open. A contained relative symlink may
    // pop this stack, but never reach above the selected destination root.
    fn parent(&self, path: &Path) -> Result<File, String> {
        let mut stack = vec![self.root.try_clone().map_err(|e| e.to_string())?];
        let mut pending: VecDeque<_> = path
            .components()
            .map(|c| c.as_os_str().to_owned())
            .collect();
        let mut links = 0;
        while let Some(part) = pending.pop_front() {
            if part == "." {
                continue;
            }
            if part == ".." {
                if stack.len() == 1 {
                    return Err("path escapes destination root".into());
                }
                stack.pop();
                continue;
            }
            let name = Path::new(&part);
            if name.is_absolute() {
                return Err("absolute symlink escapes destination root".into());
            }
            let parent = stack.last().unwrap();
            let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
            let opened = openat(parent, name, flags, Mode::empty());
            match opened {
                Ok(fd) => stack.push(File::from(fd)),
                Err(open_error) => match readlinkat(parent, name) {
                    Ok(target) => {
                        links += 1;
                        if links > 40 {
                            return Err("too many symlinks".into());
                        }
                        let target = Path::new(&target);
                        if target.is_absolute() {
                            return Err("absolute symlink escapes destination root".into());
                        }
                        for c in target.components().rev() {
                            pending.push_front(c.as_os_str().to_owned());
                        }
                    }
                    Err(Errno::ENOENT) if open_error == Errno::ENOENT => {
                        match mkdirat(parent, name, Mode::from_bits_truncate(0o750)) {
                            Ok(()) | Err(Errno::EEXIST) => (),
                            Err(e) => return Err(e.to_string()),
                        }
                        stack.push(File::from(
                            openat(parent, name, flags, Mode::empty())
                                .map_err(|e| e.to_string())?,
                        ));
                    }
                    Err(_) => return Err(open_error.to_string()),
                },
            }
        }
        Ok(stack.pop().unwrap())
    }
}

fn read_snapshot(file: &mut File) -> Result<ConfigSnapshot, String> {
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("config: not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(ConfigSnapshot {
        bytes,
        identity: (metadata.dev(), metadata.ino()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    // Exercise the read/write boundary deterministically; a timing race in a
    // CLI test would not reliably prove preservation of an intervening edit.
    #[test]
    fn config_replacement_refuses_intervening_edits_and_path_substitution() {
        let dir = std::env::temp_dir().join(format!("revust-init-edit-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let tree = TreeWriter::open(&dir).unwrap();
        let config = dir.join("config");
        fs::write(&config, "# old template\n").unwrap();
        let snapshot = tree.read_config().unwrap();
        fs::write(&config, "auto-exit = 7s\n").unwrap();
        assert!(
            tree.replace_config(snapshot.as_ref(), b"# new template\n")
                .unwrap_err()
                .contains("changed")
        );
        assert_eq!(fs::read(&config).unwrap(), b"auto-exit = 7s\n");

        let snapshot = tree.read_config().unwrap();
        fs::rename(&config, dir.join("original")).unwrap();
        symlink("original", &config).unwrap();
        assert!(
            tree.replace_config(snapshot.as_ref(), b"# new template\n")
                .is_err()
        );
        assert_eq!(fs::read(dir.join("original")).unwrap(), b"auto-exit = 7s\n");

        fs::remove_file(&config).unwrap();
        fs::write(&config, "auto-exit = 7s\n").unwrap();
        assert!(
            tree.replace_config(snapshot.as_ref(), b"# new template\n")
                .unwrap_err()
                .contains("changed")
        );
        assert_eq!(fs::read(&config).unwrap(), b"auto-exit = 7s\n");

        fs::remove_file(&config).unwrap();
        let missing = tree.read_config().unwrap();
        assert!(missing.is_none());
        fs::write(&config, "max-parallel = 2\n").unwrap();
        assert!(
            tree.replace_config(missing.as_ref(), b"# new template\n")
                .unwrap_err()
                .contains("changed")
        );
        assert_eq!(fs::read(&config).unwrap(), b"max-parallel = 2\n");
    }
}
