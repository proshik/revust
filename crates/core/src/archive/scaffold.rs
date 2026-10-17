//! Prepare input directories without claiming execution or replacing authored files.
use super::{check_name, contained_directory, error, open_directory};
use nix::{
    errno::Errno,
    fcntl::{AtFlags, OFlag, openat},
    sys::stat::{Mode, fstat, fstatat, mkdirat},
};
use serde::Serialize;
use std::{
    fs::{DirBuilder, File},
    io::Write,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
pub struct Paths {
    pub task_dir: PathBuf,
    pub task_file: PathBuf,
    pub round_dir: PathBuf,
    pub input_dir: PathBuf,
    pub scope: PathBuf,
    pub goal: PathBuf,
    pub profile: PathBuf,
    pub context: PathBuf,
    pub created: Vec<&'static str>,
}

pub fn create(tasks: &Path, task: &str, run: &str) -> Result<Paths, String> {
    if tasks.as_os_str().is_empty() {
        return Err("tasks directory is empty".into());
    }
    check_name(task).map_err(|e| format!("--task: {e}"))?;
    check_name(run).map_err(|e| format!("--run: {e}"))?;
    if run == "task.md" {
        return Err("--run task.md is reserved".into());
    }
    let root_path = crate::config::paths::absolute(&std::env::current_dir().map_err(error)?, tasks)
        .map_err(error)?;
    DirBuilder::new()
        .recursive(true)
        .mode(0o750)
        .create(&root_path)
        .map_err(error)?;
    let root = super::reader::open_root(&root_path)?;
    let mut created = Vec::new();
    let mut record = |label, made| {
        if made {
            created.push(label);
        }
    };
    record("task_dir", mkdir(&root, task)?);
    let task_root = contained_directory(&root, Path::new(task))?;
    record("task_file", write_meta(&task_root)?);
    record("round_dir", mkdir(&task_root, run)?);
    let round = real_directory(&task_root, run)?;
    super::layout::preparable(&round)?;
    record("input_dir", mkdir(&round, "input")?);
    real_directory(&round, "input")?;
    let task_dir = root_path.join(task);
    let round_dir = task_dir.join(run);
    let input_dir = round_dir.join("input");
    Ok(Paths {
        task_file: task_dir.join("task.md"),
        task_dir,
        round_dir,
        scope: input_dir.join("scope.md"),
        goal: input_dir.join("goal.md"),
        profile: input_dir.join("profile.md"),
        context: input_dir.join("context"),
        input_dir,
        created,
    })
}

fn mkdir(parent: &File, name: &str) -> Result<bool, String> {
    match mkdirat(parent, name, Mode::from_bits_truncate(0o750)) {
        Ok(()) => Ok(true),
        Err(Errno::EEXIST) => Ok(false),
        Err(e) => Err(format!("create {name}: {e}")),
    }
}

fn real_directory(parent: &File, name: &str) -> Result<File, String> {
    let entry = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(error)?;
    match entry.st_mode & nix::libc::S_IFMT {
        nix::libc::S_IFLNK => return Err(format!("{name} is a symlink")),
        nix::libc::S_IFDIR => (),
        _ => return Err(format!("{name} is not a directory")),
    }
    let directory = open_directory(parent, Path::new(name))?;
    let entry = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(error)?;
    let opened = fstat(&directory).map_err(error)?;
    if entry.st_dev != opened.st_dev || entry.st_ino != opened.st_ino {
        return Err(format!("{name} was replaced while it was being opened"));
    }
    Ok(directory)
}

fn write_meta(task: &File) -> Result<bool, String> {
    let flags =
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let opened = openat(task, "task.md", flags, Mode::from_bits_truncate(0o600));
    let mut file = match opened {
        Ok(descriptor) => File::from(descriptor),
        Err(Errno::EEXIST) => {
            let entry = fstatat(task, "task.md", AtFlags::AT_SYMLINK_NOFOLLOW).map_err(error)?;
            return match entry.st_mode & nix::libc::S_IFMT {
                nix::libc::S_IFLNK => Err("task.md is a symlink".into()),
                nix::libc::S_IFDIR => Err("task.md is a directory".into()),
                _ => Ok(false),
            };
        }
        Err(e) => return Err(error(e)),
    };
    file.write_all(include_bytes!("../../templates/task.md"))
        .and_then(|_| file.sync_all())
        .map_err(error)?;
    task.sync_all().map_err(error)?;
    Ok(true)
}
