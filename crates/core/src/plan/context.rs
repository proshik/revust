//! Resolve input locations and freeze fallback calibration without creating a round.
use super::{PlannedCopy, execution::InputPaths};
use crate::config::runtime::RuntimePaths;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};
pub(super) struct Resolved {
    pub variables: BTreeMap<String, String>,
    pub input_paths: InputPaths,
    pub profile_snapshot: Option<PlannedCopy>,
    pub snapshot_bytes: Option<Vec<u8>>,
    pub history: String,
}
enum Expected {
    File,
    Directory,
}
fn optional(path: &Path, expected: Expected) -> Result<Option<PathBuf>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("stat {}: {error}", path.display())),
    };
    let present = match expected {
        Expected::File => {
            if metadata.is_dir() {
                return Err(format!("{} is a directory, want a file", path.display()));
            }
            metadata.len() > 0
        }
        Expected::Directory => {
            if !metadata.is_dir() {
                return Err(format!("{} is a file, want a directory", path.display()));
            }
            fs::read_dir(path)
                .map_err(|error| format!("read {}: {error}", path.display()))?
                .next()
                .transpose()
                .map_err(|error| format!("read {}: {error}", path.display()))?
                .is_some()
        }
    };
    Ok(present.then(|| path.to_owned()))
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
    let input = round.join("input");
    let scope_path = input.join("scope.md");
    let scope = optional(&scope_path, Expected::File)?
        .ok_or_else(|| format!("{} is required and must not be empty", scope_path.display()))?;
    let goal = optional(&input.join("goal.md"), Expected::File)?;
    let mut profile = optional(&input.join("profile.md"), Expected::File)?;
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
    let context = optional(&input.join("context"), Expected::Directory)?;
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
        history: crate::history::inventory(task)?,
    })
}
