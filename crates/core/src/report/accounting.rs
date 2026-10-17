//! Fold executed-attempt observations into report totals without estimating usage.
use super::{Report, Scope, SourceStat, SourceStatus, Stats};
use crate::runtime::{
    Attempt, Stage,
    pipeline::{Definition, Report as RuntimeReport},
    worker::Outcome,
};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default, Clone)]
pub struct Usage {
    pub tokens: Option<i128>,
    pub actual_model: Option<String>,
}
#[derive(Default)]
struct Observed {
    tokens: i64,
    model: String,
    raised: i64,
}
fn add(total: &mut i64, value: Option<i128>) -> Result<(), String> {
    let value = i64::try_from(value.unwrap_or(0))
        .map_err(|_| "token observation overflows legacy report")?;
    *total = total
        .checked_add(value)
        .ok_or("token total overflows legacy report")?;
    Ok(())
}
pub fn assemble(
    scope: Scope,
    definition: &Definition,
    runtime: &RuntimeReport,
    usage: &BTreeMap<Attempt, Usage>,
    started_at: String,
    finished_at: String,
) -> Result<Report, String> {
    let mut seen = BTreeSet::new();
    let mut total = 0;
    let mut finders = BTreeMap::<&str, Observed>::new();
    for unit in &runtime.units {
        for attempt in &unit.attempts {
            if !seen.insert(&attempt.id) {
                return Err("duplicate attempt identity in report accounting".into());
            }
            let observation = usage.get(&attempt.id);
            add(&mut total, observation.and_then(|entry| entry.tokens))?;
            if attempt.id.stage != Stage::Find {
                continue;
            }
            let finder = finders.entry(&attempt.id.unit).or_default();
            if let Some(observation) = observation {
                add(&mut finder.tokens, observation.tokens)?;
                if let Some(model) = observation
                    .actual_model
                    .as_ref()
                    .filter(|model| !model.is_empty())
                {
                    finder.model.clone_from(model);
                }
            }
        }
        if unit.outcome == Outcome::Success
            && let Some(attempt) = unit
                .attempts
                .iter()
                .rev()
                .find(|attempt| attempt.id.stage == Stage::Find)
            && let Ok(output) = &attempt.result
        {
            finders.entry(&attempt.id.unit).or_default().raised =
                i64::try_from(output.findings.len()).map_err(|_| "finding count too large")?;
        }
    }
    let mut sources = SourceStatus {
        expected: i64::try_from(definition.agents.len()).map_err(|_| "roster too large")?,
        ..Default::default()
    };
    for agent in &definition.agents {
        let observed = finders.remove(agent.name.as_str()).unwrap_or_default();
        let degraded = !runtime.successful_reviewers.contains(&agent.name);
        if degraded {
            sources.degraded.push(agent.name.clone());
        } else {
            sources.reported += 1;
        }
        sources.agents.push(SourceStat {
            name: agent.name.clone(),
            lenses: Some(agent.lenses.clone()),
            executor: agent.runner.executor.clone(),
            requested_model: agent.runner.model.clone(),
            effort: agent.runner.effort.clone(),
            degraded,
            tokens: observed.tokens,
            actual_model: observed.model,
            raised: observed.raised,
        });
    }
    Ok(Report {
        outcome: None,
        scope,
        sources,
        findings: runtime.categories.findings.clone(),
        open_questions: runtime.categories.open_questions.clone(),
        pre_existing: runtime.categories.pre_existing.clone(),
        immaterial: runtime.categories.immaterial.clone(),
        stats: Stats {
            started_at,
            finished_at,
            duration_ms: runtime.duration_ms,
            tokens: total,
            stages: runtime.stages.clone(),
        },
    })
}
/// Wall-clock labels do not participate in monotonic admission or budget checks.
pub fn timestamp(time: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}
