//! Read-only preparation. No process, archive writer, terminal or environment access.
use crate::{
    config::{
        extensions::{EffectivePolicy, ExtensionSet},
        paths::Layers,
        runtime::{Invocation, RuntimeConfig, Value},
    },
    prompts::{FileOrigin, PromptSet, runner::Runner},
    request::{self, Request},
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Serialize)]
pub struct PromptPlan {
    format: &'static str,
    reference_revision: &'static str,
    profile: String,
    configuration: RuntimeConfig,
    effective_policy: serde_json::Value,
    variables: BTreeMap<String, String>,
    profile_snapshot: Option<PlannedCopy>,
    history: String,
    prompt_files: Vec<FileOrigin>,
    find: Vec<Finder>,
    stages: Vec<Stage>,
    #[serde(skip)]
    policy: EffectivePolicy,
    #[serde(skip)]
    roster: Vec<crate::prompts::Agent>,
    #[serde(skip)]
    authored_stages: BTreeMap<String, String>,
    #[serde(skip)]
    snapshot_bytes: Option<Vec<u8>>,
    #[serde(skip)]
    scope: crate::report::Scope,
    #[serde(skip)]
    prompt_snapshots: Vec<execution::PromptSnapshot>,
    #[serde(skip)]
    extension_profile_snapshot: Option<execution::ProfileSnapshot>,
    #[serde(skip)]
    input_paths: execution::InputPaths,
}

#[derive(Serialize)]
struct PlannedCopy {
    source: PathBuf,
    destination: PathBuf,
}
#[derive(Serialize)]
struct Finder {
    name: String,
    lenses: Vec<String>,
    #[serde(flatten)]
    runner: Runner,
    body: String,
    request: Option<Request>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_unavailable: Option<String>,
}
#[derive(Serialize)]
struct Stage {
    name: &'static str,
    #[serde(flatten)]
    runner: Runner,
    body_template: String,
    deferred: Vec<&'static str>,
    enabled: bool,
    request_template: Option<Request>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_unavailable: Option<String>,
}

impl PromptPlan {
    /// Confidence filtering does not change composed provider requests.
    pub fn with_min_confidence(mut self, value: Option<i64>) -> Self {
        if let Some(value) = value {
            self.policy.set_legacy_min_confidence(value);
            self.effective_policy = self.policy.inspection();
        }
        self
    }
    pub fn resolve(
        cwd: &Path,
        home: Option<&Path>,
        invocation: &Invocation,
        lenses: &[String],
    ) -> Result<Self, String> {
        Self::resolve_with_policy(cwd, home, invocation, lenses, &serde_json::json!({}))
    }
    pub fn resolve_with_policy(
        cwd: &Path,
        home: Option<&Path>,
        invocation: &Invocation,
        lenses: &[String],
        cli: &serde_json::Value,
    ) -> Result<Self, String> {
        let configuration = RuntimeConfig::resolve(cwd, home, invocation)?;
        let paths = &configuration.paths;
        let context::Resolved {
            variables,
            input_paths,
            profile_snapshot,
            snapshot_bytes,
            history,
        } = context::resolve(paths)?;
        let profile = configuration
            .knobs
            .iter()
            .find_map(|knob| match (&knob.name, &knob.value) {
                (&"profile", Value::Text(value)) => Some(value.clone()),
                _ => None,
            })
            .expect("runtime resolver provides profile");
        let layers = Layers {
            user: paths.config_dir.clone(),
            project: paths.project_dir.clone(),
        };
        let prompts = PromptSet::load(&layers)?;
        let extensions = ExtensionSet::load(&layers, &prompts)?;
        let effective =
            EffectivePolicy::resolve(&configuration, &extensions.layers, &profile, cli)?;
        let extended = extensions.profiles.get(&profile);
        let roster = match extended {
            Some(profile) if lenses.is_empty() => profile.agents.clone(),
            Some(profile) => vec![crate::prompts::Agent {
                name: "lenses".into(),
                lenses: lenses.to_vec(),
                runner: profile.runner.clone(),
                color: "6".into(),
                color_name: String::new(),
            }],
            None => prompts.roster(&profile, lenses)?,
        };
        let mut find = Vec::new();
        for agent in roster.clone() {
            let body = match extended {
                Some(profile) => {
                    prompts.compose_body(&profile.body, &agent, &variables, &history)?
                }
                None => prompts.compose_agent(&profile, &agent, &variables, &history)?,
            };
            let (request, request_unavailable) = planned_request(
                &agent.runner,
                &body,
                request::FINDER_SCHEMA,
                &paths.workdir,
                &effective,
            )?;
            find.push(Finder {
                name: agent.name,
                lenses: agent.lenses,
                runner: agent.runner,
                body,
                request,
                request_unavailable,
            });
        }
        // Keep runtime values explicit. This is a display template, never an
        // executable request to be interpolated a second time: inserted paths
        // may themselves contain placeholder-like text.
        let mut stage_vars = variables.clone();
        stage_vars.insert("FINDINGS".into(), "{{FINDINGS}}".into());
        stage_vars.insert("SOURCES".into(), "{{SOURCES}}".into());
        let mut stages = Vec::new();
        let mut authored_stages = BTreeMap::new();
        for name in ["synthesis", "verify"] {
            let (runner, body_template) = match extended {
                Some(profile) => prompts.compose_stage_with_runner(
                    &profile.runner,
                    name,
                    profile.stages.get(name),
                    &stage_vars,
                    &history,
                )?,
                None => prompts.compose_stage(&profile, name, &stage_vars, &history)?,
            };
            let authored = prompts.stage_body(name).expect("composed stage exists");
            authored_stages.insert(name.to_owned(), authored.to_owned());
            let deferred = ["FINDINGS", "SOURCES"]
                .into_iter()
                .filter(|key| authored.contains(&format!("{{{{{key}}}}}")))
                .collect();
            let schema = if name == "synthesis" {
                request::SYNTHESIS_SCHEMA
            } else {
                request::VERIFY_SCHEMA
            };
            let enabled = effective.execution_values()["stages"][name]["enabled"]
                .as_bool()
                .unwrap_or(true);
            let (request_template, request_unavailable) = if enabled {
                planned_request(&runner, &body_template, schema, &paths.workdir, &effective)?
            } else {
                (None, None)
            };
            stages.push(Stage {
                name,
                runner,
                body_template,
                deferred,
                request_template,
                enabled,
                request_unavailable,
            });
        }
        let prompt_snapshots = prompts
            .provenance()
            .into_iter()
            .map(|origin| {
                let content = prompts
                    .content(&origin.path)
                    .expect("loaded prompt origin")
                    .to_owned();
                execution::PromptSnapshot { origin, content }
            })
            .collect();
        let extension_profile_snapshot = extended.map(|profile| execution::ProfileSnapshot {
            source: profile.source.clone(),
            content: profile.source_text.clone(),
        });
        Ok(Self {
            input_paths,
            prompt_snapshots,
            extension_profile_snapshot,
            scope: crate::report::Scope {
                task: invocation.task.clone(),
                run: invocation.run.clone(),
                scope_path: variables["SCOPE"].clone(),
            },
            format: "revust.prompt-plan.v1",
            reference_revision: "revust-contract-v1",
            profile,
            configuration,
            effective_policy: effective.inspection(),
            variables,
            profile_snapshot,
            history,
            prompt_files: prompts.provenance(),
            find,
            stages,
            policy: effective,
            roster,
            authored_stages,
            snapshot_bytes,
        })
    }
}

// Locally authored extension-policy application; provider command construction
// belongs to request.rs, and input resolution belongs to context.rs.
fn planned_request(
    runner: &Runner,
    body: &str,
    schema: &str,
    workdir: &Path,
    policy: &EffectivePolicy,
) -> Result<(Option<Request>, Option<String>), String> {
    let values = policy.execution_values();
    let sandbox = if runner.executor == "codex" {
        values["executors"]["codex"]["options"]["sandbox"]
            .as_str()
            .unwrap_or("read-only")
    } else {
        "read-only"
    };
    if runner.executor == "opencode"
        && values["executors"]["opencode"]["options"]["agent"]
            .as_str()
            .is_some_and(|agent| agent != "revust-review")
    {
        return Ok((
            None,
            Some("OpenCode supports only the owned revust-review agent".into()),
        ));
    }
    let mut request = match request::compose(runner, body, schema, workdir, sandbox) {
        Ok(request) => request,
        Err(error) if runner.executor == "opencode" => return Ok((None, Some(error))),
        Err(error) => return Err(error),
    };
    let options = &values["executors"][&runner.executor];
    if let Some(program) = options["executable"].as_str() {
        request.program = program.into();
    }
    if runner.executor == "claude"
        && let Some(sources) = options["options"]["setting_sources"].as_array()
    {
        let index = request
            .argv
            .iter()
            .position(|arg| arg == "--setting-sources")
            .expect("Claude setting source flag");
        request.argv[index + 1] = sources
            .iter()
            .map(|s| s.as_str().expect("validated setting source"))
            .collect::<Vec<_>>()
            .join(",");
    }
    Ok((Some(request), None))
}

mod context;

mod execution;
pub use execution::{ExecutionPlan, InputPaths, ProfileSnapshot, PromptSnapshot};
