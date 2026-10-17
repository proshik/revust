use super::{Document, Layer, LayerKind};
use crate::{
    config::paths::Layers,
    prompts::{Agent, PromptSet, runner::Runner},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Serialize)]
pub struct ExtendedProfile {
    pub name: String,
    pub description: String,
    pub runner: Runner,
    pub agents: Vec<Agent>,
    pub stages: BTreeMap<String, Runner>,
    pub source: PathBuf,
    #[serde(skip)]
    pub body: String,
    #[serde(skip)]
    pub(crate) source_text: String,
}
pub struct ExtensionSet {
    pub layers: Vec<Layer>,
    pub profiles: BTreeMap<String, ExtendedProfile>,
}
impl ExtensionSet {
    pub fn load(layers: &Layers, prompts: &PromptSet) -> Result<Self, String> {
        Self::load_replacing(layers, prompts, None)
    }
    pub(crate) fn load_replacing(
        layers: &Layers,
        prompts: &PromptSet,
        replacement: Option<(&Path, &str)>,
    ) -> Result<Self, String> {
        let mut result = Self {
            layers: Vec::new(),
            profiles: BTreeMap::new(),
        };
        let legacy: BTreeSet<_> = prompts.profile_names().collect();
        for (directory, kind) in [
            (Some(&layers.user), LayerKind::User),
            (layers.project.as_ref(), LayerKind::Project),
        ] {
            let Some(directory) = directory else {
                continue;
            };
            let path = directory.join("extensions.json");
            let read = match replacement {
                Some((target, text)) if target == path => Ok(text.to_owned()),
                _ => fs::read_to_string(&path),
            };
            let text = match read {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("read {}: {error}", path.display())),
            };
            let mut document =
                Document::parse(&text).map_err(|error| format!("{}: {error}", path.display()))?;
            normalize_executables(&mut document.value, directory);
            if let Some(entries) = document
                .value
                .get("profiles")
                .and_then(serde_json::Value::as_object)
            {
                for (name, entry) in entries {
                    if name.starts_with('.') || name.contains("..") || name.contains(['/', '\\']) {
                        return Err(format!("{}: invalid profile ID {name:?}", path.display()));
                    }
                    if let Some(file) = entry.get("file").and_then(serde_json::Value::as_str) {
                        if legacy.contains(name.as_str()) {
                            return Err(format!(
                                "extension profile {name:?} shadows a legacy profile"
                            ));
                        }
                        let relative = Path::new(file);
                        if relative.is_absolute()
                            || !relative.starts_with("extension-profiles")
                            || relative
                                .components()
                                .any(|c| c == std::path::Component::ParentDir)
                        {
                            return Err(format!(
                                "profile {name}: file must stay inside extension-profiles"
                            ));
                        }
                        let source = directory.join(relative);
                        let base = directory
                            .join("extension-profiles")
                            .canonicalize()
                            .map_err(|e| format!("profile {name}: {e}"))?;
                        let real = source
                            .canonicalize()
                            .map_err(|e| format!("read {}: {e}", source.display()))?;
                        if !real.starts_with(base) {
                            return Err(format!("profile {name}: file escapes extension-profiles"));
                        }
                        let text = fs::read_to_string(&source)
                            .map_err(|e| format!("read {}: {e}", source.display()))?;
                        let profile = parse_profile(name, &text, source, prompts)?;
                        result.profiles.insert(name.clone(), profile);
                    }
                }
            }
            result.layers.push(Layer {
                directory: directory.clone(),
                kind,
                document,
            });
        }
        for layer in &result.layers {
            if let Some(entries) = layer
                .document
                .value
                .get("profiles")
                .and_then(serde_json::Value::as_object)
            {
                for name in entries.keys() {
                    if !legacy.contains(name.as_str()) && !result.profiles.contains_key(name) {
                        return Err(format!(
                            "extension policy references unknown profile {name:?}"
                        ));
                    }
                }
            }
        }
        Ok(result)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileMeta {
    #[serde(default)]
    description: String,
    runner: RunnerMeta,
    agents: Vec<AgentMeta>,
    #[serde(default)]
    stages: BTreeMap<String, RunnerMeta>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentMeta {
    name: String,
    lenses: Vec<String>,
    runner: Option<RunnerMeta>,
    #[serde(default)]
    color: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerMeta {
    executor: String,
    model: Option<String>,
    effort: Option<String>,
}
impl RunnerMeta {
    fn resolve(self) -> Result<Runner, String> {
        if !["claude", "codex", "opencode"].contains(&self.executor.as_str()) {
            return Err("unknown extension executor".into());
        }
        if self
            .model
            .as_ref()
            .is_some_and(|m| m.is_empty() || m.contains('\0'))
        {
            return Err("extension model must not be empty or contain NUL".into());
        }
        if self
            .effort
            .as_ref()
            .is_some_and(|e| !crate::prompts::runner::EFFORTS.contains(&e.as_str()))
        {
            return Err("unsupported generic effort".into());
        }
        Ok(Runner {
            executor: self.executor,
            model: self.model.unwrap_or_default(),
            effort: self.effort.unwrap_or_default(),
        })
    }
}
fn parse_profile(
    name: &str,
    text: &str,
    source: PathBuf,
    prompts: &PromptSet,
) -> Result<ExtendedProfile, String> {
    let parts = crate::prompts::template::split(text)?;
    let metadata: ProfileMeta =
        serde_saphyr::from_str(&parts.meta).map_err(|e| format!("profile {name}: {e}"))?;
    let runner = metadata.runner.resolve()?;
    if metadata.agents.is_empty() {
        return Err(format!("profile {name}: empty roster"));
    }
    crate::prompts::template::validate(&parts.body)?;
    let mut agents = Vec::new();
    let mut names = BTreeSet::new();
    for (index, agent) in metadata.agents.into_iter().enumerate() {
        let agent = Agent {
            name: agent.name,
            lenses: agent.lenses,
            runner: match agent.runner {
                Some(r) => r.resolve()?.inherit(&runner),
                None => runner.clone(),
            },
            color: crate::prompts::color(&agent.color, index)?,
            color_name: agent.color,
        };
        prompts.validate_agent(&agent)?;
        if !names.insert(agent.name.clone()) {
            return Err(format!("profile {name}: duplicate agent name"));
        }
        agents.push(agent);
    }
    let mut stages = BTreeMap::new();
    for (stage, spec) in metadata.stages {
        if !crate::prompts::runner::STAGES.contains(&stage.as_str()) {
            return Err(format!("profile {name}: unknown stage {stage}"));
        }
        stages.insert(stage, spec.resolve()?);
    }
    Ok(ExtendedProfile {
        source_text: text.into(),
        name: name.into(),
        description: metadata.description,
        runner,
        agents,
        stages,
        source,
        body: parts.body.trim().into(),
    })
}
fn normalize_executables(document: &mut serde_json::Value, directory: &Path) {
    fn policy(policy: &mut serde_json::Value, directory: &Path) {
        if let Some(executors) = policy
            .get_mut("executors")
            .and_then(serde_json::Value::as_object_mut)
        {
            for executor in executors.values_mut() {
                if let Some(path) = executor.get_mut("executable") {
                    let name = path.as_str().expect("validated executable");
                    if name.contains('/') && !Path::new(name).is_absolute() {
                        *path = serde_json::json!(directory.join(name));
                    }
                }
            }
        }
    }
    if let Some(value) = document.get_mut("policy") {
        policy(value, directory);
    }
    if let Some(profiles) = document
        .get_mut("profiles")
        .and_then(serde_json::Value::as_object_mut)
    {
        for entry in profiles.values_mut() {
            if let Some(value) = entry.get_mut("policy") {
                policy(value, directory);
            }
        }
    }
}
