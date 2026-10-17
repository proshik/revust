//! Publish unit outcomes while preserving evidence and treating storage errors as fatal.
use super::{Backend, Categories, Finding, RunControl, UnitResult};
use crate::runtime::{
    Stage,
    events::{Event, Kind},
    worker::Outcome,
};
pub(super) fn finish(
    backend: &impl Backend,
    control: &RunControl,
    stage: Stage,
    name: &str,
    inputs: &[Finding],
    mut unit: UnitResult<Categories>,
) -> UnitResult<Categories> {
    if matches!(unit.outcome, Outcome::JournalError(_)) {
        return unit;
    }
    let messages = outcome_events(stage, name, inputs, &unit);
    for event in messages {
        if let Err(error) = backend.publish(event) {
            control.cancel();
            unit.outcome = Outcome::JournalError(error);
            break;
        }
    }
    unit
}
fn outcome_events(
    stage: Stage,
    name: &str,
    inputs: &[Finding],
    unit: &UnitResult<Categories>,
) -> Vec<Event> {
    if unit.outcome != Outcome::Success {
        if stage == Stage::Synthesis {
            return Vec::new();
        }
        let reason = unit
            .attempts
            .last()
            .and_then(|attempt| attempt.result.as_ref().err())
            .map(|failure| failure.diagnostic.clone())
            .unwrap_or_else(|| format!("{:?}", unit.outcome));
        return vec![Event::agent(Kind::AgentDegraded, name, &reason)];
    }
    let Some(result) = unit
        .attempts
        .last()
        .and_then(|attempt| attempt.result.as_ref().ok())
    else {
        return vec![Event::agent(
            Kind::AgentDegraded,
            name,
            "No completed attempt evidence",
        )];
    };
    let mut events = Vec::new();
    if stage == Stage::Synthesis && !result.dropped.is_empty() {
        let urgent = result
            .dropped
            .iter()
            .filter(|candidate| matches!(candidate.severity.as_str(), "major" | "critical"))
            .count();
        let mut event = Event::agent(
            Kind::Dropped,
            name,
            &format!(
                "Consolidation omitted {} candidates; {} had major or critical impact",
                result.dropped.len(),
                urgent
            ),
        );
        event.findings = result.dropped.clone();
        events.push(event);
    }
    let completion = match stage {
        Stage::Verify => verification_summary(inputs, &result.findings),
        _ => {
            let mut event = Event::agent(Kind::Findings, name, "");
            event.findings = result.findings.clone();
            events.push(event);
            format!("Review output: {} candidates", result.findings.len())
        }
    };
    events.push(Event::agent(Kind::AgentDone, name, &completion));
    events
}
fn impact(severity: &str) -> u8 {
    match severity {
        "critical" => 3,
        "major" => 2,
        _ => 1,
    }
}
fn verification_summary(inputs: &[Finding], outputs: &[Finding]) -> String {
    let retained = outputs
        .iter()
        .filter(|finding| finding.verdict != "rejected")
        .collect::<Vec<_>>();
    let mut lower = 0;
    let mut higher = 0;
    let mut refined = 0;
    for finding in &retained {
        if finding.verdict == "refined" {
            refined += 1;
        }
        if let Some(original) = inputs
            .iter()
            .rev()
            .find(|original| original.id == finding.id)
        {
            match impact(&finding.severity).cmp(&impact(&original.severity)) {
                std::cmp::Ordering::Less => lower += 1,
                std::cmp::Ordering::Greater => higher += 1,
                _ => (),
            }
        }
    }
    format!(
        "Assessment: {} candidates, {} retained; {} refined, {} lower impact, {} higher impact",
        inputs.len(),
        retained.len(),
        refined,
        lower,
        higher
    )
}
