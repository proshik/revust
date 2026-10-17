use super::{
    defaults,
    runner::{EFFORTS, EXECUTORS, Runner, STAGES},
    template,
};
use crate::config::paths::Layers;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

// Serialized field names are the reusable profile and inspection contract.

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ProfileMeta {
    description: Option<String>,
    model: Option<String>,
    agents: Option<Vec<AgentMeta>>,
    stages: Option<BTreeMap<String, Option<String>>>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AgentMeta {
    name: Option<String>,
    lenses: Option<Vec<String>>,
    model: Option<String>,
    color: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StageMeta {
    description: Option<String>,
    model: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LensMeta {
    description: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Agent {
    pub name: String,
    pub lenses: Vec<String>,
    #[serde(flatten)]
    pub runner: Runner,
    pub color: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub color_name: String,
}
struct Profile {
    description: String,
    body: String,
    runner: Runner,
    agents: Vec<Agent>,
    stages: BTreeMap<String, Runner>,
}
struct Document {
    description: String,
    body: String,
    runner: Runner,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileOrigin {
    pub path: String,
    pub layer: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
}
struct File {
    origin: FileOrigin,
    body: String,
}

pub struct PromptSet {
    profiles: BTreeMap<String, Profile>,
    stages: BTreeMap<String, Document>,
    lenses: BTreeMap<String, Document>,
    files: BTreeMap<String, File>,
}

#[derive(Serialize)]
pub struct Catalog {
    profiles: Vec<ProfileInfo>,
    lenses: Vec<LensInfo>,
    stages: Vec<StageInfo>,
    vocabulary: Vocabulary,
}
#[derive(Serialize)]
struct ProfileInfo {
    name: String,
    description: String,
    runner: Runner,
    roster: Vec<Agent>,
    stages: Vec<StageRunner>,
}
#[derive(Serialize)]
struct StageRunner {
    name: String,
    #[serde(flatten)]
    runner: Runner,
}
#[derive(Serialize)]
struct LensInfo {
    name: String,
    description: String,
}
#[derive(Serialize)]
struct StageInfo {
    name: String,
    description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    executor: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    effort: String,
}
#[derive(Serialize)]
struct Vocabulary {
    executors: &'static [&'static str],
    efforts: &'static [&'static str],
}

impl PromptSet {
    pub fn load(layers: &Layers) -> Result<Self, String> {
        let mut registry = Self {
            profiles: BTreeMap::new(),
            stages: BTreeMap::new(),
            lenses: BTreeMap::new(),
            files: BTreeMap::new(),
        };
        for &(name, text) in defaults::FILES {
            registry.files.insert(
                name.to_owned(),
                File {
                    body: text.to_owned(),
                    origin: FileOrigin {
                        path: name.to_owned(),
                        layer: "embedded",
                        source: None,
                    },
                },
            );
        }
        registry.overlay(&layers.user, "user")?;
        if let Some(directory) = &layers.project {
            registry.overlay(directory, "project")?;
        }
        let compiled = registry
            .files
            .iter()
            .map(|(name, file)| {
                compile(name, &file.body)
                    .map(|document| (name.clone(), document))
                    .map_err(|error| format!("{name}: {error}"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        for (path, document) in compiled {
            let key = path
                .rsplit('/')
                .next()
                .unwrap()
                .strip_suffix(".md")
                .expect("discovered Markdown file")
                .to_owned();
            match document {
                Compiled::Profile(profile) => {
                    registry.profiles.insert(key, profile);
                }
                Compiled::Stage(stage) => {
                    registry.stages.insert(key, stage);
                }
                Compiled::Lens(lens) => {
                    registry.lenses.insert(key, lens);
                }
            }
        }
        for (id, profile) in &registry.profiles {
            if profile.agents.is_empty() {
                return Err(format!("profile {id}: roster is empty"));
            }
            let mut unique = BTreeSet::new();
            for agent in &profile.agents {
                registry
                    .validate_agent(agent)
                    .map_err(|error| format!("profile {id}: {error}"))?;
                if !unique.insert(&agent.name) {
                    return Err(format!(
                        "profile {id}: duplicate agent name {:?}",
                        agent.name
                    ));
                }
            }
        }
        Ok(registry)
    }
    fn overlay(&mut self, root: &std::path::Path, layer: &'static str) -> Result<(), String> {
        let paths = ["prompts", "lenses", "prompts/profiles"]
            .into_iter()
            .flat_map(|directory| {
                fs::read_dir(root.join(directory))
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .filter_map(move |entry| {
                        entry
                            .file_name()
                            .to_str()
                            .filter(|name| name.ends_with(".md"))
                            .map(|name| format!("{directory}/{name}"))
                    })
            })
            .collect::<BTreeSet<_>>();
        for path in paths {
            let source = root.join(&path);
            let body = fs::read_to_string(&source).map_err(|error| {
                format!("read {} from {layer} layer: {error}", source.display())
            })?;
            let origin = FileOrigin {
                path: path.clone(),
                layer,
                source: Some(source),
            };
            self.files.insert(path, File { body, origin });
        }
        Ok(())
    }
    pub fn catalog(&self) -> Catalog {
        Catalog {
            profiles: self
                .profiles
                .iter()
                .map(|(name, profile)| ProfileInfo {
                    name: name.clone(),
                    description: profile.description.clone(),
                    runner: profile.runner.clone(),
                    roster: profile.agents.clone(),
                    stages: STAGES
                        .iter()
                        .filter_map(|name| {
                            self.stages.get(*name).map(|file| StageRunner {
                                name: (*name).into(),
                                runner: profile
                                    .runner
                                    .stage(&file.runner, profile.stages.get(*name)),
                            })
                        })
                        .collect(),
                })
                .collect(),
            lenses: self
                .lenses
                .iter()
                .map(|(name, doc)| LensInfo {
                    name: name.clone(),
                    description: doc.description.clone(),
                })
                .collect(),
            stages: STAGES
                .iter()
                .filter_map(|name| {
                    self.stages.get(*name).map(|doc| StageInfo {
                        name: (*name).into(),
                        description: doc.description.clone(),
                        executor: doc.runner.executor.clone(),
                        model: doc.runner.model.clone(),
                        effort: doc.runner.effort.clone(),
                    })
                })
                .collect(),
            vocabulary: Vocabulary {
                executors: EXECUTORS,
                efforts: EFFORTS,
            },
        }
    }

    pub fn provenance(&self) -> Vec<FileOrigin> {
        self.files
            .values()
            .map(|file| file.origin.clone())
            .collect()
    }
    pub fn content(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(|file| file.body.as_str())
    }
    pub fn profile_names(&self) -> impl Iterator<Item = &str> {
        self.profiles.keys().map(String::as_str)
    }
    fn profile(&self, name: &str) -> Result<&Profile, String> {
        self.profiles
            .get(name)
            .ok_or_else(|| format!("unknown profile {name:?}"))
    }
    pub fn roster(&self, profile: &str, lenses: &[String]) -> Result<Vec<Agent>, String> {
        let selected = self.profile(profile)?;
        if lenses.is_empty() {
            return Ok(selected.agents.clone());
        }
        let override_agent = Agent {
            name: "lenses".into(),
            runner: selected.runner.clone(),
            lenses: lenses.to_owned(),
            color: color("", 0)?,
            color_name: String::new(),
        };
        self.validate_agent(&override_agent)
            .map_err(|error| format!("lens override: {error}"))?;
        Ok(vec![override_agent])
    }
    pub(crate) fn validate_agent(&self, agent: &Agent) -> Result<(), String> {
        let id = &agent.name;
        let bad = if id.is_empty() {
            Some("roster entry has no name".to_owned())
        } else if std::path::Path::new(id).is_absolute() {
            Some(format!("agent {id:?} is an absolute path"))
        } else if id.contains(['/', '\\']) {
            Some(format!("agent {id:?} contains a path separator"))
        } else if id.contains("..") {
            Some(format!("agent {id:?} references a parent directory"))
        } else if id.starts_with('.') {
            Some(format!("agent {id:?} starts with a dot"))
        } else if agent.lenses.is_empty() {
            Some(format!("agent {id}: no lenses"))
        } else {
            None
        };
        if let Some(error) = bad {
            return Err(error);
        }
        match agent
            .lenses
            .iter()
            .find(|name| !self.lenses.contains_key(name.as_str()))
        {
            Some(name) => Err(format!("agent {id}: unknown lens {name:?}")),
            None => Ok(()),
        }
    }
    pub fn compose_agent(
        &self,
        profile: &str,
        agent: &Agent,
        vars: &BTreeMap<String, String>,
        history: &str,
    ) -> Result<String, String> {
        self.compose_body(&self.profile(profile)?.body, agent, vars, history)
    }
    pub fn compose_body(
        &self,
        body: &str,
        agent: &Agent,
        vars: &BTreeMap<String, String>,
        history: &str,
    ) -> Result<String, String> {
        self.validate_agent(agent)?;
        let sections = std::iter::once(body)
            .chain(
                agent
                    .lenses
                    .iter()
                    .map(|name| self.lenses[name].body.as_str()),
            )
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>();
        template::render(&sections.join("\n\n"), vars, history)
    }
    pub(crate) fn stage_body(&self, name: &str) -> Option<&str> {
        self.stages.get(name).map(|document| document.body.as_str())
    }
    pub fn stage_runner(
        &self,
        base: &Runner,
        stage: &str,
        override_runner: Option<&Runner>,
    ) -> Result<Runner, String> {
        let document = self
            .stages
            .get(stage)
            .ok_or_else(|| format!("unknown stage {stage:?}"))?;
        Ok(base.stage(&document.runner, override_runner))
    }
    pub fn compose_stage(
        &self,
        profile: &str,
        stage: &str,
        vars: &BTreeMap<String, String>,
        history: &str,
    ) -> Result<(Runner, String), String> {
        let selected = self.profile(profile)?;
        self.compose_stage_with_runner(
            &selected.runner,
            stage,
            selected.stages.get(stage),
            vars,
            history,
        )
    }
    pub fn compose_stage_with_runner(
        &self,
        base: &Runner,
        stage: &str,
        override_runner: Option<&Runner>,
        vars: &BTreeMap<String, String>,
        history: &str,
    ) -> Result<(Runner, String), String> {
        let selected = self.stage_runner(base, stage, override_runner)?;
        let rendered = template::render(
            self.stage_body(stage).expect("resolved stage"),
            vars,
            history,
        )?;
        Ok((selected, rendered))
    }
}

enum Compiled {
    Profile(Profile),
    Stage(Document),
    Lens(Document),
}
fn compile(path: &str, text: &str) -> Result<Compiled, String> {
    let parts = template::split(text)?;
    let body = parts.body.trim().to_owned();
    template::validate(&body)?;
    if path.starts_with("lenses/") {
        let LensMeta { description } = decode(&parts.meta)?;
        return Ok(Compiled::Lens(Document {
            description: description.unwrap_or_default(),
            body,
            runner: Runner::default(),
        }));
    }
    if !path.starts_with("prompts/profiles/") {
        let StageMeta { description, model } = decode(&parts.meta)?;
        return Ok(Compiled::Stage(Document {
            description: description.unwrap_or_default(),
            body,
            runner: Runner::parse(model.as_deref().unwrap_or(""))?,
        }));
    }
    let ProfileMeta {
        description,
        model,
        agents,
        stages,
    } = decode(&parts.meta)?;
    let authored = Runner::parse(model.as_deref().unwrap_or(""))?;
    let base = if authored.executor.is_empty() {
        Runner {
            executor: "claude".into(),
            ..authored
        }
    } else {
        authored
    };
    let agents = agents
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(position, meta)| {
            let color_name = meta.color.unwrap_or_default();
            Ok(Agent {
                name: meta.name.unwrap_or_default(),
                lenses: meta.lenses.unwrap_or_default(),
                runner: Runner::parse(meta.model.as_deref().unwrap_or(""))?.inherit(&base),
                color: color(&color_name, position)?,
                color_name,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let stages = stages
        .unwrap_or_default()
        .into_iter()
        .map(|(name, model)| {
            let selected = Runner::parse(model.as_deref().unwrap_or(""))?;
            if selected.executor.is_empty() {
                return Err(format!("stage {name}: empty runner"));
            }
            if !STAGES.contains(&name.as_str()) {
                return Err(format!("unknown stage {name:?}"));
            }
            Ok((name, selected))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(Compiled::Profile(Profile {
        description: description.unwrap_or_default(),
        body,
        runner: base,
        agents,
        stages,
    }))
}

pub(crate) fn decode<T: DeserializeOwned + Default>(text: &str) -> Result<T, String> {
    serde_saphyr::from_str::<Option<T>>(text)
        .map(Option::unwrap_or_default)
        .map_err(|error| match error.without_snippet() {
            serde_saphyr::Error::SerdeUnknownField { field, .. } => {
                format!("parse front matter: field {field} not found")
            }
            detail => format!("parse front matter: {detail}"),
        })
}
pub(crate) fn color(authored: &str, position: usize) -> Result<String, String> {
    const ORDER: [u8; 8] = [6, 5, 2, 3, 4, 1, 14, 13];
    let palette = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    if authored.is_empty() {
        return Ok(ORDER[position % ORDER.len()].to_string());
    }
    let (bright, base) = authored
        .strip_prefix("bright-")
        .map_or((false, authored), |base| (true, base));
    if let Some(index) = palette.iter().position(|candidate| *candidate == base) {
        return Ok((index + if bright { 8 } else { 0 }).to_string());
    }
    if authored
        .strip_prefix('#')
        .is_some_and(|hex| hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Ok(authored.to_owned());
    }
    Err(format!(
        "invalid color {authored:?}, want an ANSI-16 name or #RRGGBB"
    ))
}
