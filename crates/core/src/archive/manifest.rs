//! Completion metadata joins frozen intent with saved execution observations.
use crate::{
    plan::ExecutionPlan,
    prompts::Agent,
    report::{Report, SourceStat, StageRun},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Serialize)]
struct Origin<'a> {
    path: &'a str,
    layer: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a Path>,
    hash: String,
}
#[derive(Serialize)]
struct Manifest<'a> {
    #[serde(skip_serializing_if = "<[Agent]>::is_empty")]
    roster: &'a [Agent],
    task: &'a str,
    run: &'a str,
    profile: &'a str,
    scope_path: &'a str,
    #[serde(serialize_with = "crate::report::serialize_time")]
    started_at: &'a str,
    #[serde(serialize_with = "crate::report::serialize_time")]
    finished_at: &'a str,
    duration_ms: i64,
    tokens: i64,
    agents: Vec<SourceStat>,
    degraded: Option<&'a [String]>,
    prompts: Vec<Origin<'a>>,
    stages: Option<&'a [StageRun]>,
}
pub(crate) fn encode(plan: &ExecutionPlan, report: &Report) -> Result<String, String> {
    let scope = plan.scope();
    if scope.task != report.scope.task
        || scope.run != report.scope.run
        || scope.scope_path != report.scope.scope_path
    {
        return Err("manifest report scope differs from frozen plan".into());
    }
    // Index observations once; repeated observations retain last-record precedence.
    let observed: std::collections::BTreeMap<_, _> = report
        .sources
        .agents
        .iter()
        .map(|source| (source.name.as_str(), source))
        .collect();
    let roster = &plan.definition().agents;
    let mut agents = Vec::with_capacity(roster.len());
    for reviewer in roster {
        let evidence = observed
            .get(reviewer.name.as_str())
            .copied()
            .cloned()
            .unwrap_or_default();
        agents.push(SourceStat {
            name: reviewer.name.clone(),
            lenses: Some(reviewer.lenses.clone()),
            executor: reviewer.runner.executor.clone(),
            requested_model: reviewer.runner.model.clone(),
            effort: reviewer.runner.effort.clone(),
            actual_model: evidence.actual_model,
            tokens: evidence.tokens,
            raised: evidence.raised,
            degraded: evidence.degraded,
        });
    }
    let manifest = Manifest {
        roster,
        task: &scope.task,
        run: &scope.run,
        profile: plan.profile(),
        scope_path: &scope.scope_path,
        started_at: &report.stats.started_at,
        finished_at: &report.stats.finished_at,
        duration_ms: report.stats.duration_ms,
        tokens: report.stats.tokens,
        agents,
        degraded: (!report.sources.degraded.is_empty())
            .then_some(report.sources.degraded.as_slice()),
        stages: (!report.stats.stages.is_empty()).then_some(report.stats.stages.as_slice()),
        prompts: plan
            .prompt_snapshots()
            .iter()
            .map(|snapshot| Origin {
                path: &snapshot.origin.path,
                layer: snapshot.origin.layer,
                source: snapshot.origin.source.as_deref(),
                hash: Sha256::digest(snapshot.content.as_bytes())
                    .as_slice()
                    .iter()
                    .fold(String::with_capacity(64), |mut text, byte| {
                        use std::fmt::Write;
                        write!(text, "{byte:02x}").unwrap();
                        text
                    }),
            })
            .collect(),
    };
    serde_json::to_string_pretty(&manifest)
        .map(|document| document + "\n")
        .map_err(|e| format!("encode completion metadata: {e}"))
}
