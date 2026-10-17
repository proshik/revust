//! Descriptor-owned storage for one claimed review round.
use nix::{
    fcntl::{AtFlags, Flock, FlockArg, OFlag, open, openat, readlinkat},
    sys::stat::{Mode, fstat, fstatat, mkdirat},
};
use std::{
    collections::VecDeque,
    ffi::OsString,
    fs::File,
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Component, Path},
};
pub struct Archive {
    directory: File,
    path: std::path::PathBuf,
    identity: (String, String),
    marker: Flock<File>,
    finished: bool,
}
impl Archive {
    pub fn claim(tasks: &Path, task: &str, run: &str) -> Result<Self, String> {
        check_name(task)?;
        check_name(run)?;
        if run == "task.md" {
            return Err("round name is reserved".into());
        }
        let path = std::path::absolute(tasks.join(task).join(run)).map_err(error)?;
        let identity = (task.to_owned(), run.to_owned());
        let root = File::from(
            open(
                tasks,
                OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(error)?,
        );
        let task = contained_directory(&root, Path::new(task))?;
        // Final round and input entries must be actual directories, not aliases.
        let directory = open_directory(&task, Path::new(run))?;
        let _input = open_directory(&directory, Path::new("input"))?;
        let flags = OFlag::O_WRONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK;
        let (file, existing) = match openat(
            &directory,
            "manifest.json",
            flags | OFlag::O_CREAT | OFlag::O_EXCL,
            Mode::from_bits_truncate(0o600),
        ) {
            Ok(fd) => (File::from(fd), false),
            Err(nix::errno::Errno::EEXIST) => (
                File::from(
                    openat(&directory, "manifest.json", flags, Mode::empty()).map_err(error)?,
                ),
                true,
            ),
            Err(e) => return Err(error(e)),
        };
        let marker = Flock::lock(file, FlockArg::LockExclusiveNonblock)
            .map_err(|(_, e)| format!("round is already claimed: {e}"))?;
        let metadata = marker.metadata().map_err(error)?;
        let entry =
            fstatat(&directory, "manifest.json", AtFlags::AT_SYMLINK_NOFOLLOW).map_err(error)?;
        let opened = fstat(&*marker).map_err(error)?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || opened.st_dev != entry.st_dev
            || opened.st_ino != entry.st_ino
        {
            return Err("manifest is not the exclusively owned regular file".into());
        }
        if existing {
            layout::preparable(&directory)?;
        }
        marker.sync_all().map_err(error)?;
        directory.sync_all().map_err(error)?;
        Ok(Self {
            path,
            identity,
            directory,
            marker,
            finished: false,
        })
    }
    pub fn journal(
        &self,
        identity: crate::runtime::journal::Identity,
    ) -> Result<crate::runtime::journal::Journal, String> {
        if self.finished {
            return Err("archive is finished".into());
        }
        if (&identity.task, &identity.run) != (&self.identity.0, &self.identity.1) {
            return Err("journal identity differs from claimed round".into());
        }
        crate::runtime::journal::Journal::create_in(&self.path, &self.directory, identity)
    }
    /// New artifacts never overwrite caller inputs or an earlier attempt's evidence.
    pub fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        if self.finished {
            return Err("archive is finished".into());
        }
        let components = artifact_components(path)?;
        let (file, parents) = components.split_last().ok_or("empty artifact name")?;
        let mut directory = self.directory.try_clone().map_err(error)?;
        for part in parents {
            let path = Path::new(part);
            match mkdirat(&directory, path, Mode::from_bits_truncate(0o750)) {
                Ok(()) => directory.sync_all().map_err(error)?,
                Err(nix::errno::Errno::EEXIST) => (),
                Err(e) => return Err(error(e)),
            }
            directory = open_directory(&directory, path)?;
        }
        let mut output = File::from(
            openat(
                &directory,
                Path::new(file),
                OFlag::O_WRONLY
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(error)?,
        );
        output
            .write_all(bytes)
            .and_then(|_| output.sync_all())
            .map_err(error)?;
        directory.sync_all().map_err(error)
    }
    /// Save exactly the final report shown to the caller. A failure seals this
    /// writer without completing its marker, so partial output cannot be blessed
    /// by a later finish. Full manifest assembly belongs to the caller.
    pub fn save_report(&mut self, report: &crate::report::Report) -> Result<(), String> {
        let result = (|| {
            if (&report.scope.task, &report.scope.run) != (&self.identity.0, &self.identity.1) {
                return Err("report identity differs from claimed round".into());
            }
            // Render both before writing: invalid timestamps must not leave a
            // plausible Markdown report beside an unencodable machine report.
            let json = report.json()?;
            let markdown = report.markdown();
            self.write(Path::new("report.md"), markdown.as_bytes())?;
            self.write(Path::new("findings.json"), json.as_bytes())
        })();
        if result.is_err() {
            self.finished = true;
        }
        result
    }
    /// The final manifest is written last through the locked file descriptor.
    pub fn finish(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.finished || bytes.is_empty() {
            return Err("manifest must be nonempty and written once".into());
        }
        self.finished = true;
        self.marker
            .write_all(bytes)
            .and_then(|_| self.marker.sync_all())
            .map_err(error)?;
        self.directory.sync_all().map_err(error)
    }
}
fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with('.')
        || name.contains("..")
        || name.contains(['/', '\\', '\0'])
    {
        return Err("invalid task or round name".into());
    }
    Ok(())
}
fn open_directory(parent: &File, path: &Path) -> Result<File, String> {
    openat(
        parent,
        path,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(error)
}
fn artifact_components(path: &Path) -> Result<Vec<OsString>, String> {
    let mut result = vec![];
    for component in path.components() {
        match component {
            Component::Normal(name) => result.push(name.to_owned()),
            Component::CurDir => (),
            Component::ParentDir => {
                if result.pop().is_none() {
                    return Err("artifact escapes round".into());
                }
            }
            _ => return Err("artifact must be relative".into()),
        }
    }
    if result
        .first()
        .is_none_or(|name| name == "input" || name == "manifest.json" || name == "revust")
    {
        return Err("artifact is empty or reserved".into());
    }
    Ok(result)
}
// Resolve a task's relative symlinks below the tasks root. Each directory stays
// open; '..' pops an owned handle and can never climb above the root.
fn contained_directory(root: &File, path: &Path) -> Result<File, String> {
    open_beneath(root, path, true)
}
fn open_beneath(root: &File, path: &Path, directory: bool) -> Result<File, String> {
    let mut stack = vec![root.try_clone().map_err(error)?];
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
                return Err("task alias escapes tasks root".into());
            }
            stack.pop();
            continue;
        }
        if Path::new(&part).is_absolute() {
            return Err("absolute task alias".into());
        }
        let parent = stack.last().unwrap();
        let flags = OFlag::O_RDONLY
            | OFlag::O_NOFOLLOW
            | OFlag::O_CLOEXEC
            | OFlag::O_NONBLOCK
            | if !pending.is_empty() || directory {
                OFlag::O_DIRECTORY
            } else {
                OFlag::empty()
            };
        match openat(parent, Path::new(&part), flags, Mode::empty())
            .map(File::from)
            .map_err(error)
        {
            Ok(file) if pending.is_empty() => return Ok(file),
            Ok(file) => stack.push(file),
            Err(open_error) => {
                let target = readlinkat(parent, Path::new(&part)).map_err(|_| open_error)?;
                links += 1;
                if links > 40 {
                    return Err("too many task symlinks".into());
                }
                let target = Path::new(&target);
                if target.is_absolute() {
                    return Err("absolute task alias".into());
                }
                let components: Vec<_> = target
                    .components()
                    .map(|c| c.as_os_str().to_owned())
                    .collect();
                for part in components.into_iter().rev() {
                    pending.push_front(part);
                }
            }
        }
    }
    Ok(stack.pop().unwrap())
}
fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub mod cleanup;
pub(crate) mod manifest;
pub mod reader;
pub mod scaffold;
pub mod stats;

mod layout;
