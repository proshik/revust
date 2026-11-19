//! Read-only settings and task inspection with per-task diagnostics.
use super::{
    paths::Layers,
    runtime::{Invocation, Knob, RuntimeConfig, RuntimePaths},
};
use crate::prompts::{self, PromptSet, template};
use serde::{Deserialize, Serialize};
use std::{fs, fs::File, io, path::Path};

#[derive(Serialize)]
pub struct Catalog {
    knobs: Vec<Knob>,
    #[serde(flatten)]
    prompts: crate::prompts::Catalog,
    paths: CatalogPaths,
}
#[derive(Serialize)]
struct CatalogPaths {
    #[serde(flatten)]
    runtime: RuntimePaths,
    tasks: Vec<Task>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tasks_error: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Meta {
    description: Option<String>,
    url: Option<String>,
    branch: Option<String>,
    base: Option<String>,
}
#[derive(Default, Serialize)]
struct Task {
    id: String,
    description: String,
    url: String,
    branch: String,
    base: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta_error: Option<String>,
    rounds: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rounds_error: Option<String>,
}

impl Catalog {
    pub fn resolve(
        cwd: &Path,
        home: Option<&Path>,
        invocation: &Invocation,
    ) -> Result<Self, String> {
        let RuntimeConfig { knobs, paths, .. } = RuntimeConfig::resolve(cwd, home, invocation)?;
        let prompts = PromptSet::load(&Layers {
            user: paths.config_dir.clone(),
            project: paths.project_dir.clone(),
        })?
        .catalog();
        let inventory = task_ids(&paths.tasks_dir).and_then(|ids| {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let root = crate::archive::reader::open_root(&paths.tasks_dir)
                .map_err(|error| format!("list tasks: {error}"))?;
            Ok(ids
                .into_iter()
                .map(|id| inspect_task(&root, &paths.tasks_dir, id))
                .collect())
        });
        let (tasks, tasks_error) = match inventory {
            Ok(tasks) => (tasks, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        Ok(Self {
            knobs,
            prompts,
            paths: CatalogPaths {
                runtime: paths,
                tasks,
                tasks_error,
            },
        })
    }
}

/// Metadata and rounds come from one task directory handle, so a task replaced
/// during inspection cannot contribute rounds of another directory.
fn inspect_task(root: &File, root_path: &Path, id: String) -> Task {
    let directory = root_path.join(&id);
    let mut task = Task {
        id,
        ..Task::default()
    };
    let handle = match crate::archive::contained_directory(root, Path::new(&task.id)) {
        Ok(handle) => handle,
        Err(error) => {
            let error = format!("open {}: {error}", directory.display());
            task.meta_error = Some(error.clone());
            task.rounds_error = Some(error);
            return task;
        }
    };
    match read_meta(&handle, &directory) {
        Ok(Meta {
            description,
            url,
            branch,
            base,
        }) => {
            task.description = description.unwrap_or_default();
            task.url = url.unwrap_or_default();
            task.branch = branch.unwrap_or_default();
            task.base = base.unwrap_or_default();
        }
        Err(error) => task.meta_error = Some(error),
    }
    match rounds(&handle, &directory) {
        Ok((names, skipped)) => {
            task.rounds = names;
            task.rounds_error = (!skipped.is_empty()).then(|| skipped.join("; "));
        }
        Err(error) => task.rounds_error = Some(error),
    }
    task
}
fn read_meta(task: &File, directory: &Path) -> Result<Meta, String> {
    let path = directory.join("task.md");
    let source = match crate::archive::reader::optional(task, "task.md") {
        Ok(Some(bytes)) => {
            String::from_utf8(bytes).map_err(|error| format!("read {}: {error}", path.display()))?
        }
        Ok(None) => return Ok(Meta::default()),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    template::split(&source)
        .and_then(|parts| prompts::decode(&parts.meta))
        .map_err(|error| format!("parse {}: {error}", path.display()))
}

/// Discovery accepts contained relative task aliases; archive round identities do
/// not accept symlinks. This distinction is independent of metadata parsing.
fn list(directory: &Path, accept: impl Fn(&fs::DirEntry) -> bool) -> Result<Vec<String>, String> {
    let mut names = fs::read_dir(directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?
        .map(|entry| entry.map_err(|error| format!("read {}: {error}", directory.display())))
        .collect::<Result<Vec<_>, _>>()?;
    names.retain(accept);
    let mut names = names
        .into_iter()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}
pub(crate) fn task_ids(root: &Path) -> Result<Vec<String>, String> {
    let canonical = match root.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("list tasks: {}: {error}", root.display())),
    };
    list(root, |entry| {
        let path = entry.path();
        let relative_alias = !fs::read_link(&path).is_ok_and(|target| target.is_absolute());
        relative_alias
            && path.is_dir()
            && path
                .canonicalize()
                .is_ok_and(|target| target.starts_with(&canonical))
    })
    .map_err(|error| format!("list tasks: {error}"))
}
/// Recorded rounds below a held task directory, plus diagnostics for rounds
/// skipped because their names are not UTF-8: a lossy name could reopen a
/// different round. Neither a round nor its marker may be a symlink.
pub(crate) fn rounds(task: &File, path: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    use nix::{fcntl::AtFlags, libc::S_IFMT, sys::stat::fstatat};
    let kind = |parent: &File, name: &Path| {
        fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)
            .map(|stat| (stat.st_mode & S_IFMT, stat.st_size))
    };
    let mut names = Vec::new();
    let mut skipped = Vec::new();
    let entries =
        crate::archive::names(task).map_err(|error| format!("read {}: {error}", path.display()))?;
    for name in entries {
        let recorded = kind(task, Path::new(&name))
            .is_ok_and(|(mode, _)| mode == nix::libc::S_IFDIR)
            && crate::archive::open_directory(task, Path::new(&name)).is_ok_and(|round| {
                kind(&round, Path::new("manifest.json"))
                    .is_ok_and(|(mode, size)| mode == nix::libc::S_IFREG && size > 0)
            });
        if recorded {
            match name.into_string() {
                Ok(name) => names.push(name),
                Err(name) => skipped.push(format!(
                    "skipped round {:?} in {}: name is not UTF-8",
                    name.to_string_lossy(),
                    path.display()
                )),
            }
        }
    }
    Ok((names, skipped))
}
