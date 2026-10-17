//! Replay saved observations without access to current configuration or execution.
use crate::Model;
use chrono::DateTime;
use revust_core::{
    archive::reader,
    prompts::Agent,
    report::Report,
    runtime::events::{Event, Kind},
};
fn saved_kind(name: &str) -> Option<Kind> {
    Some(match name {
        "stage" => Kind::Stage,
        "findings" => Kind::Findings,
        "dropped" => Kind::Dropped,
        "rate_limit" => Kind::RateLimit,
        "agent_started" => Kind::AgentStarted,
        "agent_activity" => Kind::AgentActivity,
        "agent_progress" => Kind::AgentProgress,
        "agent_state" => Kind::AgentState,
        "agent_done" => Kind::AgentDone,
        "agent_retried" => Kind::AgentRetried,
        "agent_degraded" => Kind::AgentDegraded,
        _ => return None,
    })
}
impl Model {
    pub fn from_saved_events(
        roster: Vec<Agent>,
        events: impl IntoIterator<Item = reader::Event>,
        report: Report,
    ) -> Result<Self, String> {
        let mut observation = Self::new(roster);
        observation.archive = true;
        for saved in events {
            if let Some(kind) = saved_kind(&saved.kind) {
                observation.apply(Event {
                    kind,
                    at: saved.at,
                    agent: saved.agent,
                    stage: saved.stage,
                    text: saved.text,
                    findings: saved.findings,
                })?;
            } else {
                let timestamp = DateTime::parse_from_rfc3339(&saved.at)
                    .map_err(|error| format!("invalid event time: {error}"))?;
                if let Some(index) = observation.observe(&saved.agent, timestamp) {
                    let row = &mut observation.agents[index];
                    row.started.get_or_insert(timestamp);
                    row.updated = Some(
                        row.updated
                            .map_or(timestamp, |previous| previous.max(timestamp)),
                    );
                }
            }
        }
        observation.complete(report);
        observation.stage = "archive".into();
        observation.view.archive();
        Ok(observation)
    }
}
