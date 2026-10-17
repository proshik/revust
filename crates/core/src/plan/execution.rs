//! Frozen requests and authored templates for an execution. No filesystem writes.
use super::{PromptPlan, planned_request};
use crate::{
    prompts::template,
    request::{self, Request},
    runtime::{
        Stage, inputs,
        pipeline::{Call, Definition},
    },
};
use std::{collections::BTreeMap, path::PathBuf};
pub struct Snapshot {
    pub destination: PathBuf,
    pub content: Vec<u8>,
}
/// Raw winning prompt text, including front matter and original whitespace.
pub struct PromptSnapshot {
    pub origin: crate::prompts::FileOrigin,
    pub content: String,
}
/// Extension-only profile evidence, kept separate from the legacy prompt tree.
pub struct ProfileSnapshot {
    pub source: PathBuf,
    pub content: String,
}
/// Input paths selected during resolution, before prompt placeholder formatting.
/// Optional absence stays distinct from any later filesystem changes.
pub struct InputPaths {
    pub scope: PathBuf,
    pub goal: Option<PathBuf>,
    pub profile: Option<PathBuf>,
    pub context: Option<PathBuf>,
    pub profile_is_snapshot: bool,
}
pub struct ExecutionPlan {
    input_paths: InputPaths,
    profile: String,
    prompt_snapshots: Vec<PromptSnapshot>,
    extension_profile_snapshot: Option<ProfileSnapshot>,
    definition: Definition,
    snapshots: Vec<Snapshot>,
    find: BTreeMap<String, Request>,
    authored_stages: BTreeMap<String, String>,
    variables: BTreeMap<String, String>,
    history: String,
    workdir: PathBuf,
    round: PathBuf,
    tasks_dir: PathBuf,
    scope: crate::report::Scope,
}
impl PromptPlan {
    pub fn into_execution(self) -> Result<ExecutionPlan, String> {
        let mut find = BTreeMap::new();
        for finder in self.find {
            let request = finder.request.ok_or_else(|| {
                finder
                    .request_unavailable
                    .unwrap_or_else(|| "finder request is unavailable".into())
            })?;
            find.insert(finder.name, request);
        }
        for stage in &self.stages {
            if !stage.enabled {
                continue;
            }
            if stage.request_template.is_none() {
                return Err(stage
                    .request_unavailable
                    .clone()
                    .unwrap_or_else(|| "stage request is unavailable".into()));
            }
            let mut vars = self.variables.clone();
            vars.insert("FINDINGS".into(), String::new());
            if stage.name == "synthesis" {
                vars.insert("SOURCES".into(), String::new());
            }
            template::render(&self.authored_stages[stage.name], &vars, &self.history)?;
        }
        let synthesis = self
            .stages
            .iter()
            .find(|s| s.name == "synthesis")
            .ok_or("synthesis stage missing")?
            .runner
            .clone();
        let verify = self
            .stages
            .iter()
            .find(|s| s.name == "verify")
            .ok_or("verify stage missing")?
            .runner
            .clone();
        let snapshots = match (self.profile_snapshot, self.snapshot_bytes) {
            (Some(copy), Some(content)) => vec![Snapshot {
                destination: copy.destination,
                content,
            }],
            (None, None) => vec![],
            _ => return Err("incomplete calibration snapshot".into()),
        };
        Ok(ExecutionPlan {
            input_paths: self.input_paths,
            profile: self.profile,
            prompt_snapshots: self.prompt_snapshots,
            extension_profile_snapshot: self.extension_profile_snapshot,
            scope: self.scope,
            definition: Definition {
                agents: self.roster,
                synthesis,
                verify,
                policy: self.policy,
            },
            snapshots,
            find,
            authored_stages: self.authored_stages,
            variables: self.variables,
            history: self.history,
            round: self
                .configuration
                .paths
                .archive
                .ok_or("execution requires a round")?,
            workdir: self.configuration.paths.workdir,
            tasks_dir: self.configuration.paths.tasks_dir,
        })
    }
}
impl ExecutionPlan {
    pub fn input_paths(&self) -> &InputPaths {
        &self.input_paths
    }
    pub fn manifest(&self, report: &crate::report::Report) -> Result<String, String> {
        crate::archive::manifest::encode(self, report)
    }
    pub fn profile(&self) -> &str {
        &self.profile
    }
    pub fn prompt_snapshots(&self) -> &[PromptSnapshot] {
        &self.prompt_snapshots
    }
    pub fn extension_profile_snapshot(&self) -> Option<&ProfileSnapshot> {
        self.extension_profile_snapshot.as_ref()
    }
    pub fn tasks_dir(&self) -> &std::path::Path {
        &self.tasks_dir
    }
    pub fn round(&self) -> &std::path::Path {
        &self.round
    }
    pub fn scope(&self) -> &crate::report::Scope {
        &self.scope
    }
    pub fn definition(&self) -> &Definition {
        &self.definition
    }
    pub fn snapshots(&self) -> &[Snapshot] {
        &self.snapshots
    }
    pub fn finder_requests(&self) -> impl Iterator<Item = (&str, &Request)> {
        self.find
            .iter()
            .map(|(name, request)| (name.as_str(), request))
    }
    pub fn request(&self, call: &Call<'_>) -> Result<Request, String> {
        if call.attempt.stage == Stage::Find {
            let agent = self
                .definition
                .agents
                .iter()
                .find(|a| a.name == call.attempt.unit)
                .ok_or("unknown finder")?;
            if &agent.runner != call.runner {
                return Err("finder runner differs from frozen plan".into());
            }
            return self
                .find
                .get(&agent.name)
                .cloned()
                .ok_or_else(|| "finder request unavailable".into());
        }
        let (name, runner, schema, inputs) = match call.attempt.stage {
            Stage::Synthesis => (
                "synthesis",
                &self.definition.synthesis,
                request::SYNTHESIS_SCHEMA,
                inputs::synthesis(call.sources)?,
            ),
            Stage::Verify => (
                "verify",
                &self.definition.verify,
                request::VERIFY_SCHEMA,
                inputs::verification(call.findings)?,
            ),
            Stage::Find => unreachable!(),
        };
        if self.definition.policy.execution_values()["stages"][name]["enabled"] != true {
            return Err(format!("{name} stage is disabled"));
        }
        if runner != call.runner {
            return Err("stage runner differs from frozen plan".into());
        }
        let mut vars = self.variables.clone();
        vars.extend(inputs);
        // Render authored text exactly once: neither inserted paths nor findings
        // are scanned for placeholders, and history is appended only here.
        let body = template::render(&self.authored_stages[name], &vars, &self.history)?;
        let (request, error) = planned_request(
            runner,
            &body,
            schema,
            &self.workdir,
            &self.definition.policy,
        )?;
        request.ok_or_else(|| error.unwrap_or_else(|| "stage request unavailable".into()))
    }
}
