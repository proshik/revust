//! Read-only settings and task inspection with per-task diagnostics.
use super::{
    paths::Layers,
    runtime::{Invocation, Knob, RuntimeConfig, RuntimePaths},
};
use crate::prompts::{self, PromptSet, template};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

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
        let RuntimeConfig { knobs, paths } = RuntimeConfig::resolve(cwd, home, invocation)?;
        let prompts = PromptSet::load(&Layers {
            user: paths.config_dir.clone(),
            project: paths.project_dir.clone(),
        })?
        .catalog();
        let inventory = task_ids(&paths.tasks_dir).map(|ids| {
            ids.into_iter()
                .map(|id| inspect_task(&paths.tasks_dir, id))
                .collect()
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

fn inspect_task(root: &Path, id: String) -> Task {
    let directory = root.join(&id);
    let mut task = Task {
        id,
        ..Task::default()
    };
    match read_meta(&directory) {
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
    match rounds(&directory) {
        Ok(names) => task.rounds = names,
        Err(error) => task.rounds_error = Some(error),
    }
    task
}
fn read_meta(directory: &Path) -> Result<Meta, String> {
    let path = directory.join("task.md");
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Meta::default()),
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
pub(crate) fn rounds(task: &Path) -> Result<Vec<String>, String> {
    list(task, |entry| {
        entry.file_type().is_ok_and(|kind| kind.is_dir())
            && fs::symlink_metadata(entry.path().join("manifest.json"))
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
    })
}
