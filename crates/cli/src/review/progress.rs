//! Plain observer of durable events; never dispatches execution.
mod terminal;
use chrono::{DateTime, FixedOffset};
use revust_core::{
    report::Report,
    runtime::events::{Event, Kind},
};
use revust_tui::{colors, wrap};
use std::collections::BTreeMap;

pub fn columns(value: Option<&str>) -> usize {
    value
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|width| *width >= 40)
        .or_else(|| terminal::stderr_columns().filter(|width| *width >= 40))
        .unwrap_or(120)
}
#[derive(Default)]
struct Activity {
    emitted: Option<DateTime<FixedOffset>>,
    pending: String,
}
pub struct Progress {
    palette: BTreeMap<String, String>,
    activity: BTreeMap<String, Activity>,
    columns: usize,
}
impl Progress {
    pub fn new<'a>(agents: impl Iterator<Item = (&'a str, &'a str)>, columns: usize) -> Self {
        Self {
            palette: agents
                .map(|(name, color)| (name.to_owned(), color.to_owned()))
                .collect(),
            activity: BTreeMap::new(),
            columns,
        }
    }
    pub fn refresh_columns(&mut self) {
        self.columns = columns(std::env::var("COLUMNS").ok().as_deref());
    }
    fn render(&self, clock: &str, agent: &str, text: &str) -> String {
        let agent = wrap::text(agent).replace('\n', " ");
        let head = if agent.is_empty() {
            format!("{clock} ")
        } else {
            format!("{clock} {agent}: ")
        };
        let mut rows = wrap::wrap(&head, text, self.columns);
        if !agent.is_empty()
            && let Some(first) = rows.first_mut()
        {
            let color = self
                .palette
                .get(&agent)
                .cloned()
                .unwrap_or_else(|| colors::derived(&agent));
            let painted = colors::paint(&color, &agent);
            *first = format!(
                "{clock} {painted}: {}",
                first.strip_prefix(&head).unwrap_or(first)
            );
        }
        rows.join("\n")
    }
    pub fn line(&mut self, event: Event) -> Option<String> {
        let at = DateTime::parse_from_rfc3339(&event.at).ok()?;
        let mut message = revust_tui::narration(&event)?;
        let activity = self.activity.entry(event.agent.clone()).or_default();
        if matches!(event.kind, Kind::AgentProgress) {
            if !message.is_empty() {
                activity.pending = message;
            }
            if activity.emitted.is_some_and(|previous| {
                at.signed_duration_since(previous) < chrono::TimeDelta::seconds(10)
            }) {
                return None;
            }
            message = std::mem::take(&mut activity.pending);
        }
        if message.is_empty() {
            return None;
        }
        activity.emitted = Some(activity.emitted.map_or(at, |previous| previous.max(at)));
        Some(
            self.render(
                &at.with_timezone(&chrono::Local)
                    .format("%H:%M:%S")
                    .to_string(),
                &event.agent,
                &message,
            ),
        )
    }
    /// The caller publishes this only after report persistence succeeds.
    pub fn finish(&self, report: &Report) -> Vec<String> {
        let clock = DateTime::parse_from_rfc3339(&report.stats.finished_at)
            .map(|at| {
                at.with_timezone(&chrono::Local)
                    .format("%H:%M:%S")
                    .to_string()
            })
            .unwrap_or_else(|_| "time unknown".into());
        let mut counts = BTreeMap::new();
        for finding in &report.findings {
            *counts
                .entry(if finding.severity.is_empty() {
                    "unlabeled"
                } else {
                    &finding.severity
                })
                .or_insert(0usize) += 1;
        }
        let degraded = report.sources.degraded_names();
        let messages = [
            "complete".to_owned(),
            format!(
                "sources {}/{}; degraded: {}",
                report.sources.reported,
                report.sources.expected,
                if degraded.is_empty() {
                    "none".into()
                } else {
                    degraded.join(", ")
                }
            ),
            if counts.is_empty() {
                "no findings".into()
            } else {
                format!(
                    "{} findings: {}",
                    report.findings.len(),
                    counts
                        .into_iter()
                        .map(|(severity, count)| format!("{count} {severity}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        ];
        messages
            .iter()
            .map(|message| self.render(&clock, "", message))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(kind: Kind, name: &str, text: &str, second: u32) -> Event {
        let mut event = Event::agent(kind, name, text);
        event.at = format!("2026-10-10T12:00:{second:02}Z");
        event
    }
    #[test]
    fn independent_sources_coalesce_newest_progress_without_inventing_completion() {
        let mut progress = Progress::new(std::iter::empty(), 120);
        assert!(
            progress
                .line(event(Kind::AgentStarted, "a", "fixture", 0))
                .unwrap()
                .contains("started: fixture")
        );
        for (second, text) in [(1, "old"), (2, "new")] {
            assert!(
                progress
                    .line(event(Kind::AgentProgress, "a", text, second))
                    .is_none()
            );
        }
        assert!(
            progress
                .line(event(Kind::AgentProgress, "b", "other", 3))
                .unwrap()
                .contains("other")
        );
        assert!(
            progress
                .line(event(Kind::AgentProgress, "a", "", 10))
                .unwrap()
                .contains("new")
        );
        assert!(
            progress
                .line(event(Kind::AgentProgress, "a", "", 20))
                .is_none()
        );
        assert!(
            progress
                .line(event(Kind::Findings, "a", "hidden", 21))
                .is_none()
        );
        assert!(
            progress
                .line(event(Kind::AgentDegraded, "a", "partial", 22))
                .unwrap()
                .contains("degraded: partial")
        );
    }
    #[test]
    fn hostile_text_cannot_supply_control_codes_and_unknown_summary_stays_visible() {
        let mut progress = Progress::new([("a", "")].into_iter(), 120);
        let line = progress
            .line(event(
                Kind::AgentActivity,
                "a",
                "bad\x1b]52;c;secret\x07",
                0,
            ))
            .unwrap();
        assert!(!line.chars().any(|c| c.is_control()));
        let mut report = Report::decode(r#"{"sources":{"expected":2,"reported":1,"agents":[{"name":"b","degraded":true}]},"findings":[{"severity":"unknown"},{"severity":"major"},{"severity":""}]}"#).unwrap();
        report.stats.finished_at = "invalid".into();
        let summary = progress.finish(&report).join("\n");
        for expected in [
            "time unknown",
            "complete",
            "sources 1/2",
            "degraded: b",
            "1 unknown",
            "1 unlabeled",
            "1 major",
        ] {
            assert!(summary.contains(expected), "{summary}");
        }
    }
}
