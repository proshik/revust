//! Observation-only state folded from durable typed events.
//! Terminal I/O and rendering are separate from this model and from execution.
mod archive;
pub mod colors;
pub mod findings;
mod frame;
mod inputs;
mod logs;
pub mod markdown;
pub mod navigation;
mod presentation;
pub mod status;
mod tabs;
pub mod terminal;
mod terminal_input;
pub mod updates;
pub mod wrap;
use chrono::{DateTime, FixedOffset, TimeDelta};
use revust_core::{
    prompts::{Agent, runner::Runner},
    report::Report,
    runtime::{
        data::Finding,
        events::{Event, Kind},
    },
};
use std::{collections::VecDeque, time::Duration};

const RETAINED_ENTRIES: usize = 2000;
type At = DateTime<FixedOffset>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Waiting,
    Running,
    Retrying,
    Limited,
    Done,
    Degraded,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub at: At,
    pub agent: String,
    pub text: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub total: usize,
    pub critical: usize,
    pub major: usize,
    pub minor: usize,
}
impl Tally {
    fn add(&mut self, findings: &[Finding]) {
        for finding in findings {
            self.total += 1;
            match finding.severity.as_str() {
                "critical" => self.critical += 1,
                "major" => self.major += 1,
                "minor" => self.minor += 1,
                _ => (),
            }
        }
    }
}
pub struct AgentState {
    pub name: String,
    /// Frozen roster metadata; absent for derived synthesis/verification rows.
    pub roster: Option<Agent>,
    pub state: State,
    pub started: Option<At>,
    pub updated: Option<At>,
    pub last: String,
    pub lines: VecDeque<Entry>,
    beat: Option<At>,
    pending: String,
}
impl AgentState {
    fn new(name: String, roster: Option<Agent>) -> Self {
        let state = if roster.is_some() {
            State::Waiting
        } else {
            State::Running
        };
        Self {
            name,
            roster,
            state,
            started: None,
            updated: None,
            last: String::new(),
            lines: VecDeque::new(),
            beat: None,
            pending: String::new(),
        }
    }
    /// Whole observed seconds; terminal rows stop their clock at their last event.
    pub fn elapsed(&self, now: At) -> Option<TimeDelta> {
        let start = self.started?;
        let observed = self.updated?;
        let end = match self.state {
            State::Done | State::Degraded => observed,
            _ => now.max(observed),
        };
        let milliseconds = end.signed_duration_since(start).num_milliseconds();
        if milliseconds <= 0 {
            None
        } else {
            Some(TimeDelta::seconds(milliseconds.saturating_add(500) / 1000))
        }
    }
    fn track(&mut self, event: &Event, at: At) -> Option<Entry> {
        self.started.get_or_insert(at);
        self.updated = Some(self.updated.map_or(at, |previous| previous.max(at)));
        let next = match event.kind {
            Kind::AgentStarted | Kind::AgentActivity | Kind::AgentProgress | Kind::AgentState => {
                Some(State::Running)
            }
            Kind::AgentDone => Some(State::Done),
            Kind::AgentDegraded => Some(State::Degraded),
            Kind::AgentRetried => Some(State::Retrying),
            Kind::RateLimit => Some(State::Limited),
            _ => None,
        };
        if let Some(state) = next {
            self.state = state;
        }
        if matches!(event.kind, Kind::AgentStarted) {
            self.started = Some(at);
        }
        if matches!(event.kind, Kind::Findings) {
            self.last = format!("{} findings received", event.findings.len());
            return None;
        }
        let mut text = narration(event)?;
        if matches!(event.kind, Kind::AgentProgress) {
            self.last = text.clone();
            if !text.is_empty() {
                self.pending = text;
            }
            if self
                .beat
                .is_some_and(|previous| at.signed_duration_since(previous) < TimeDelta::seconds(10))
            {
                return None;
            }
            text = std::mem::take(&mut self.pending);
        } else if !matches!(event.kind, Kind::Dropped) {
            self.last = text.clone();
        }
        if text.is_empty() {
            return None;
        }
        let entry = Entry {
            at,
            agent: self.name.clone(),
            text,
        };
        retain(&mut self.lines, entry.clone());
        self.beat = Some(self.beat.map_or(at, |previous| previous.max(at)));
        Some(entry)
    }
}
fn retain(entries: &mut VecDeque<Entry>, entry: Entry) {
    if entries.len() == RETAINED_ENTRIES {
        entries.pop_front();
    }
    entries.push_back(entry);
}
pub struct Model {
    pub agents: Vec<AgentState>,
    pub combined: VecDeque<Entry>,
    pub stage: String,
    pub found: Tally,
    pub now: Option<At>,
    pub report: Option<Report>,
    pub findings: Option<findings::Findings>,
    synthesis_runner: Option<Runner>,
    verify_runner: Option<Runner>,
    view: navigation::View,
    inputs: Vec<navigation::InputDocument>,
    auto_exit: Duration,
    exit_in: Duration,
    archive: bool,
}
impl Model {
    pub fn new(roster: Vec<Agent>) -> Self {
        Self {
            agents: roster
                .into_iter()
                .map(|agent| AgentState::new(agent.name.clone(), Some(agent)))
                .collect(),
            combined: VecDeque::new(),
            stage: String::new(),
            found: Tally::default(),
            now: None,
            report: None,
            findings: None,
            synthesis_runner: None,
            verify_runner: None,
            view: navigation::View::default(),
            inputs: Vec::new(),
            auto_exit: Duration::ZERO,
            exit_in: Duration::ZERO,
            archive: false,
        }
    }
    /// Frozen requested settings, available before stage execution completes.
    pub fn with_stage_runners(mut self, synthesis: Runner, verify: Runner) -> Self {
        self.synthesis_runner = Some(synthesis);
        self.verify_runner = Some(verify);
        self
    }
    pub(crate) fn runner<'a>(&'a self, agent: &'a AgentState) -> Option<&'a Runner> {
        if let Some(roster) = &agent.roster {
            return Some(&roster.runner);
        }
        if agent.name == "synthesis" {
            self.synthesis_runner.as_ref()
        } else if agent.name.starts_with("verify ") {
            self.verify_runner.as_ref()
        } else {
            None
        }
    }
    /// Configures the delay for a future completion; archive viewers always wait.
    pub fn with_auto_exit(mut self, delay: Duration) -> Self {
        if !self.archive {
            self.auto_exit = delay;
        }
        self
    }
    pub fn exit_remaining(&self) -> Duration {
        self.exit_in
    }
    /// One scheduled second. A late tick after a key cancelled the countdown is inert.
    pub fn tick(&mut self) -> navigation::Action {
        if self.exit_in.is_zero() {
            return navigation::Action::None;
        }
        self.exit_in = self.exit_in.saturating_sub(Duration::from_secs(1));
        if self.exit_in.is_zero() {
            navigation::Action::Quit
        } else {
            navigation::Action::None
        }
    }
    fn observe(&mut self, name: &str, at: At) -> Option<usize> {
        self.now = Some(self.now.map_or(at, |previous| previous.max(at)));
        if name.is_empty() {
            return None;
        }
        let index = match self.agents.iter().position(|agent| agent.name == name) {
            Some(index) => index,
            None => {
                self.agents.push(AgentState::new(name.to_owned(), None));
                self.agents.len() - 1
            }
        };
        Some(index)
    }

    pub fn apply(&mut self, event: Event) -> Result<(), String> {
        let at = DateTime::parse_from_rfc3339(&event.at)
            .map_err(|e| format!("invalid event time: {e}"))?;
        // The durable completion snapshot is the final observation boundary.
        if self.report.is_some() {
            return Ok(());
        }
        let row = self.observe(&event.agent, at);
        if matches!(event.kind, Kind::Stage) {
            self.stage = event.stage;
            retain(
                &mut self.combined,
                Entry {
                    at,
                    agent: String::new(),
                    text: self.stage.clone(),
                },
            );
        } else if let Some(index) = row {
            if matches!(event.kind, Kind::Findings) {
                if self.agents[index].roster.is_none() {
                    self.found = Tally::default();
                }
                self.found.add(&event.findings);
            }
            if let Some(entry) = self.agents[index].track(&event, at) {
                retain(&mut self.combined, entry);
            }
        }
        Ok(())
    }
    /// A channel ending, or all agents finishing, does not replace this message.
    pub fn complete(&mut self, report: Report) {
        if self.report.is_some() {
            return;
        }
        for stage in &report.stats.stages {
            let slot = match stage.name.as_str() {
                "synthesis" => &mut self.synthesis_runner,
                "verify" => &mut self.verify_runner,
                _ => continue,
            };
            if slot.is_none() {
                *slot = Some(Runner {
                    executor: stage.executor.clone(),
                    model: stage.model.clone(),
                    effort: stage.effort.clone(),
                });
            }
        }
        self.found = Tally::default();
        self.found.add(&report.findings);
        self.findings = Some(findings::Findings::new(&report));
        self.report = Some(report);
        self.exit_in = self.auto_exit;
        self.view.complete(self.agents.len() + 1);
    }
    /// Restores observation only. No executor, filesystem or runtime handle exists here.
    pub fn from_archive(
        roster: Vec<Agent>,
        events: impl IntoIterator<Item = Event>,
        report: Report,
    ) -> Result<Self, String> {
        let mut model = Self::new(roster);
        model.archive = true;
        for event in events {
            model.apply(event)?;
        }
        model.complete(report);
        model.stage = "archive".into();
        model.view.archive();
        Ok(model)
    }
}

/// Human-readable event meaning shared by terminal and plain observers.
/// The returned string is data; presentation must make control characters inert.
pub fn narration(event: &Event) -> Option<String> {
    let label = match event.kind {
        Kind::Stage => return Some(event.stage.clone()),
        Kind::Findings => return None,
        Kind::AgentStarted => "started",
        Kind::AgentDone => "done",
        Kind::AgentRetried => "retrying",
        Kind::AgentDegraded => "degraded",
        Kind::RateLimit => "rate limited",
        _ => return Some(event.text.clone()),
    };
    Some(if event.text.is_empty() {
        label.to_owned()
    } else {
        format!("{label}: {}", event.text)
    })
}
