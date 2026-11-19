//! Resolve and freeze round inputs and fallback calibration without creating a round.
use super::{
    PlannedCopy,
    execution::{InputPaths, InputSnapshot},
};
use crate::{archive, config::runtime::RuntimePaths};
use nix::{errno::Errno, fcntl::AtFlags, sys::stat::fstatat};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File},
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};
pub(super) struct Resolved {
    pub variables: BTreeMap<String, String>,
    pub input_paths: InputPaths,
    pub profile_snapshot: Option<PlannedCopy>,
    pub snapshot_bytes: Option<Vec<u8>>,
    pub input_snapshots: Vec<InputSnapshot>,
    pub history: String,
}
// Context inventory ceiling, matching the input display walk; the shared archive
// file ceiling bounds each document and the combined frozen inputs.
const ENTRY_LIMIT: usize = 1024;
/// Round inputs read exactly once through the contained, nonblocking archive
/// reader. Providers, stages and replay use only these frozen bytes.
struct Freezer {
    root: File,
    used: u64,
    entries: usize,
    snapshots: Vec<InputSnapshot>,
}
impl Freezer {
    fn exists(&self, name: &Path) -> Result<bool, String> {
        match fstatat(&self.root, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(_) => Ok(true),
            Err(Errno::ENOENT) => Ok(false),
            Err(error) => Err(format!("stat {}: {error}", name.display())),
        }
    }
    /// Each read is capped by the remaining combined allowance, so planning
    /// never holds more than the documented total.
    fn read(&mut self, file: File, name: &Path) -> Result<Vec<u8>, String> {
        let label = name.to_string_lossy();
        let remaining = archive::FILE_LIMIT - self.used;
        let bytes = archive::reader::read_bounded(file, &label, remaining).map_err(|error| {
            if self.used > 0 && error.ends_with("exceeds size limit") {
                format!("review inputs exceed {} bytes", archive::FILE_LIMIT)
            } else {
                error
            }
        })?;
        self.used += bytes.len() as u64;
        Ok(bytes)
    }
    /// Empty and missing documents are both "not provided" to providers; an
    /// empty one is still frozen so saved views can tell the two apart.
    fn document(&mut self, name: &str) -> Result<bool, String> {
        let source = Path::new("input").join(name);
        if !self.exists(&source)? {
            return Ok(false);
        }
        let file = archive::open_beneath(&self.root, &source, false)
            .map_err(|error| format!("read {}: {error}", source.display()))?;
        let bytes = self.read(file, &source)?;
        let provided = !bytes.is_empty();
        self.snapshots.push(InputSnapshot {
            relative: PathBuf::from(name),
            content: Some(bytes),
        });
        Ok(provided)
    }
    fn context(&mut self) -> Result<bool, String> {
        let source = Path::new("input/context");
        if !self.exists(source)? {
            return Ok(false);
        }
        let directory = archive::open_beneath(&self.root, source, false)
            .map_err(|error| format!("read {}: {error}", source.display()))?;
        if !directory.metadata().is_ok_and(|info| info.is_dir()) {
            return Err(format!("{} is a file, want a directory", source.display()));
        }
        // An empty directory is frozen for saved views but not provided.
        self.snapshots.push(InputSnapshot {
            relative: PathBuf::from("context"),
            content: None,
        });
        let before = self.snapshots.len();
        self.walk(directory, Path::new("context"))?;
        Ok(self.snapshots.len() > before)
    }
    fn walk(&mut self, directory: File, relative: &Path) -> Result<(), String> {
        let label = Path::new("input").join(relative);
        let failure = |error: nix::Error| format!("read {}: {error}", label.display());
        let remaining = ENTRY_LIMIT - self.entries;
        let mut names = Vec::new();
        for entry in nix::dir::Dir::from_fd(directory.into())
            .map_err(failure)?
            .iter()
        {
            let name = entry.map_err(failure)?.file_name().to_bytes().to_owned();
            if name != b"." && name != b".." {
                names.push(OsString::from_vec(name));
                // Stop listing once the tree budget is exceeded.
                if names.len() > remaining {
                    return Err(format!("input/context exceeds {ENTRY_LIMIT} entries"));
                }
            }
        }
        names.sort();
        for name in names {
            self.entries += 1;
            if self.entries > ENTRY_LIMIT {
                return Err(format!("input/context exceeds {ENTRY_LIMIT} entries"));
            }
            let child = relative.join(&name);
            let source = Path::new("input").join(&child);
            // Each entry resolves from the round root, so relative links stay
            // contained in the round and link cycles end at the entry ceiling.
            let file = archive::open_beneath(&self.root, &source, false)
                .map_err(|error| format!("read {}: {error}", source.display()))?;
            if file.metadata().is_ok_and(|info| info.is_dir()) {
                self.snapshots.push(InputSnapshot {
                    relative: child.clone(),
                    content: None,
                });
                self.walk(file, &child)?;
            } else {
                let bytes = self.read(file, &source)?;
                self.snapshots.push(InputSnapshot {
                    relative: child,
                    content: Some(bytes),
                });
            }
        }
        Ok(())
    }
}
fn names(path: &Path) -> Result<&Path, String> {
    path.file_name()
        .map(Path::new)
        .ok_or_else(|| format!("{} has no name", path.display()))
}
fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    if !fs::metadata(path)
        .map_err(|error| format!("stat {}: {error}", path.display()))?
        .is_dir()
    {
        return Err(format!("{} is not a directory", path.display()));
    }
    path.canonicalize()
        .map_err(|error| format!("resolve {}: {error}", path.display()))
}
pub(super) fn resolve(paths: &RuntimePaths) -> Result<Resolved, String> {
    let round = paths
        .archive
        .as_ref()
        .ok_or("dry-run requires valid --task and --run names")?;
    let task = round.parent().ok_or("archive has no task parent")?;
    let real_task = canonical_directory(task)?;
    let real_root = canonical_directory(&paths.tasks_dir)?;
    if !real_task.starts_with(real_root) {
        return Err("task directory escapes tasks root".into());
    }
    if !canonical_directory(round)?.starts_with(&real_task) {
        return Err("round directory escapes task".into());
    }
    // Inputs are read below descriptors opened the way a claim opens the round:
    // contained task resolution and a round that is not itself a link.
    let root = archive::reader::open_root(&paths.tasks_dir)?;
    let task_directory = archive::contained_directory(&root, names(task)?)
        .map_err(|error| format!("open {}: {error}", task.display()))?;
    let mut freezer = Freezer {
        root: archive::open_directory(&task_directory, names(round)?)
            .map_err(|error| format!("open {}: {error}", round.display()))?,
        used: 0,
        entries: 0,
        snapshots: Vec::new(),
    };
    let frozen = round.join("prompts/input");
    if !freezer.document("scope.md")? {
        return Err(format!(
            "{} is required and must not be empty",
            round.join("input/scope.md").display()
        ));
    }
    let scope = frozen.join("scope.md");
    let goal = freezer.document("goal.md")?.then(|| frozen.join("goal.md"));
    let mut profile = freezer
        .document("profile.md")?
        .then(|| frozen.join("profile.md"));
    let mut profile_snapshot = None;
    let mut snapshot_bytes = None;
    if profile.is_none() {
        if let Some(error) = &paths.profile_fallback_error {
            return Err(error.clone());
        }
        if let Some(source) = &paths.profile_fallback {
            // Read exactly once at selection time, then retain these bytes. No
            // later planning or execution step reopens this calibration source.
            snapshot_bytes = Some(
                fs::read(source).map_err(|error| format!("read {}: {error}", source.display()))?,
            );
            let destination = round.join("prompts/input-profile.md");
            profile = Some(destination.clone());
            profile_snapshot = Some(PlannedCopy {
                source: source.clone(),
                destination,
            });
        }
    }
    let context = freezer.context()?.then(|| frozen.join("context"));
    if let Some(error) = &paths.workdir_error {
        return Err(format!("workdir: {error}"));
    }
    let input_paths = InputPaths {
        scope: scope.clone(),
        goal: goal.clone(),
        profile: profile.clone(),
        context: context.clone(),
        profile_is_snapshot: profile_snapshot.is_some(),
    };
    let mut variables = BTreeMap::new();
    for (name, path) in [
        ("SCOPE", Some(scope)),
        ("GOAL", goal),
        ("PROFILE", profile),
        ("CONTEXT", context),
        ("WORKDIR", Some(paths.workdir.clone())),
    ] {
        let text = match path {
            Some(path) => path
                .to_str()
                .ok_or("context path must be UTF-8")?
                .to_owned(),
            None => "none provided".to_owned(),
        };
        variables.insert(name.to_owned(), text);
    }
    Ok(Resolved {
        variables,
        input_paths,
        profile_snapshot,
        snapshot_bytes,
        input_snapshots: freezer.snapshots,
        history: crate::history::inventory(task)?,
    })
}
