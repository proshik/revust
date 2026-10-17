//! Saved report transport and observation-oriented rendering.
use crate::runtime::data::Finding;
use serde::{Deserialize, Serialize};
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ExecutionOutcome>,
    pub scope: Scope,
    pub sources: SourceStatus,
    pub findings: Vec<Finding>,
    pub open_questions: Vec<Finding>,
    pub pre_existing: Vec<Finding>,
    pub immaterial: Vec<Finding>,
    pub stats: Stats,
}
/// Durable execution outcome; absence retains older successful-archive semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionOutcome {
    pub state: crate::runtime::pipeline::State,
    #[serde(default)]
    pub policy_failures: Vec<String>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}
impl ExecutionOutcome {
    pub fn from_runtime(runtime: &crate::runtime::pipeline::Report) -> Self {
        Self {
            state: runtime.state.clone(),
            policy_failures: runtime.policy_failures.clone(),
            diagnostics: runtime
                .units
                .iter()
                .filter_map(|unit| {
                    unit.attempts
                        .last()?
                        .result
                        .as_ref()
                        .err()
                        .map(|error| error.diagnostic.clone())
                })
                .collect(),
        }
    }
    pub fn incomplete(&self) -> bool {
        !self.policy_failures.is_empty()
            || !matches!(
                self.state,
                crate::runtime::pipeline::State::Complete
                    | crate::runtime::pipeline::State::Partial
            )
    }
    pub fn diagnostic(&self) -> String {
        let mut text = format!(
            "review incomplete: {:?}; policy failures: {:?}",
            self.state, self.policy_failures
        );
        for diagnostic in &self.diagnostics {
            text.push('\n');
            text.push_str(diagnostic);
        }
        text
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Scope {
    pub task: String,
    pub run: String,
    pub scope_path: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceStatus {
    pub expected: i64,
    pub reported: i64,
    pub degraded: Vec<String>,
    pub agents: Vec<SourceStat>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceStat {
    pub name: String,
    pub lenses: Option<Vec<String>>,
    pub executor: String,
    pub requested_model: String,
    pub actual_model: String,
    pub effort: String,
    pub tokens: i64,
    pub raised: i64,
    pub degraded: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    #[serde(serialize_with = "serialize_time")]
    pub started_at: String,
    #[serde(serialize_with = "serialize_time")]
    pub finished_at: String,
    pub duration_ms: i64,
    pub tokens: i64,
    pub stages: Vec<StageRun>,
}
impl Default for Stats {
    fn default() -> Self {
        Self {
            started_at: "0001-01-01T00:00:00Z".into(),
            finished_at: "0001-01-01T00:00:00Z".into(),
            duration_ms: 0,
            tokens: 0,
            stages: vec![],
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StageRun {
    pub name: String,
    pub duration_ms: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub executor: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
}
impl Report {
    /// Missing and null optional values retain the saved format's defaults.
    pub fn decode(text: &str) -> Result<Self, String> {
        let mut document = serde_json::from_str(text).map_err(|e| format!("report JSON: {e}"))?;
        saved::project(&mut document);
        let report: Self =
            serde_json::from_value(document).map_err(|e| format!("report fields: {e}"))?;
        for timestamp in [&report.stats.started_at, &report.stats.finished_at] {
            parsed_time(timestamp)?;
        }
        Ok(report)
    }

    pub fn json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self)
            .map(|document| document + "\n")
            .map_err(|e| format!("encode report: {e}"))
    }

    pub fn above(&self, min_confidence: i64) -> Self {
        Self {
            findings: self
                .findings
                .iter()
                .filter(|item| item.confidence >= min_confidence)
                .cloned()
                .collect(),
            ..self.clone()
        }
    }

    pub fn exit_code(&self) -> i32 {
        if self
            .outcome
            .as_ref()
            .is_some_and(ExecutionOutcome::incomplete)
        {
            2
        } else {
            i32::from(!self.findings.is_empty())
        }
    }

    pub fn markdown(&self) -> String {
        use std::fmt::Write;
        let mut text = String::from("# Review results\n\n");
        if let Some(outcome) = &self.outcome
            && outcome.incomplete()
        {
            writeln!(text, "{}\n", outcome.diagnostic()).unwrap();
        }
        if !self.scope.task.is_empty() || !self.scope.run.is_empty() {
            writeln!(
                text,
                "Task: `{}` · Round: `{}`\n",
                self.scope.task, self.scope.run
            )
            .unwrap();
        }
        if !self.scope.scope_path.is_empty() {
            writeln!(text, "Scope: `{}`\n", self.scope.scope_path).unwrap();
        }
        if self.sources.expected == 0 && self.sources.agents.is_empty() {
            text.push_str("Source delivery: unavailable in saved metadata.\n\n");
        } else {
            writeln!(
                text,
                "Source delivery: {} / {} reported.\n",
                self.sources.reported, self.sources.expected
            )
            .unwrap();
        }
        let failed = self.sources.degraded_names();
        if !failed.is_empty() {
            writeln!(
                text,
                "Incomplete sources: {}. Results may be partial.\n",
                failed.join(", ")
            )
            .unwrap();
        }
        let mut active: Vec<_> = self.findings.iter().collect();
        active.sort_by_key(|item| match item.severity.as_str() {
            "critical" => 0,
            "major" => 1,
            "minor" => 2,
            _ => 3,
        });
        text.push_str("## Active findings\n\n");
        if active.is_empty() {
            text.push_str("No active findings in the saved result.\n\n");
        }
        for item in active {
            render_finding(&mut text, item);
        }
        for (category, items) in [
            ("Open questions", &self.open_questions),
            ("Pre-existing", &self.pre_existing),
            ("Immaterial", &self.immaterial),
        ] {
            if items.is_empty() {
                continue;
            }
            writeln!(text, "## {category}\n").unwrap();
            for item in items {
                render_finding(&mut text, item);
            }
        }
        if !self.sources.agents.is_empty() {
            text.push_str("## Source observations\n\n");
            text.push_str("| Source | Executor | Requested model | Observed model | Effort | Recorded tokens | Raised | Delivery |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n");
            for source in &self.sources.agents {
                let tokens = if source.tokens == 0 {
                    "zero or unavailable".into()
                } else {
                    source.tokens.to_string()
                };
                let cells = [
                    source.name.as_str(),
                    source.executor.as_str(),
                    source.requested_model.as_str(),
                    source.actual_model.as_str(),
                    source.effort.as_str(),
                ];
                let cells: Vec<_> = cells.into_iter().map(table_cell).collect();
                writeln!(
                    text,
                    "| {} | {tokens} | {} | {} |",
                    cells.join(" | "),
                    source.raised,
                    if source.degraded || failed.contains(&source.name) {
                        "failed or partial"
                    } else {
                        "reported"
                    }
                )
                .unwrap();
            }
            text.push_str("\nToken fields record available observations only. Zero cannot distinguish measured zero from unavailable usage; totals do not establish complete usage. Failed attempts may contribute recorded usage.\n");
        }
        text
    }
}

impl SourceStatus {
    pub fn degraded_names(&self) -> Vec<String> {
        let mut names = self.degraded.clone();
        for source in self.agents.iter().filter(|source| source.degraded) {
            if !names.contains(&source.name) {
                names.push(source.name.clone());
            }
        }
        names
    }
}

pub fn heading(severity: &str) -> String {
    let mut letters = severity.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + letters.as_str())
        .unwrap_or_else(|| "Unspecified".into())
}

pub fn location(finding: &Finding) -> String {
    match (finding.file.as_str(), finding.line, finding.end_line) {
        ("", _, _) => "(no file)".into(),
        (file, line, _) if line <= 0 => file.into(),
        (file, line, end) if end > line => format!("{file}:{line}-{end}"),
        (file, line, _) => format!("{file}:{line}"),
    }
}

fn table_cell(value: &str) -> String {
    if value.is_empty() {
        return "unknown".into();
    }
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn render_finding(text: &mut String, item: &Finding) {
    use std::fmt::Write;
    writeln!(text, "### {} — {}\n", heading(&item.severity), item.title).unwrap();
    writeln!(
        text,
        "Identity: `{}` · Location: `{}` · Confidence: {}\n",
        item.id,
        location(item),
        item.confidence
    )
    .unwrap();
    if !item.body.is_empty() {
        writeln!(text, "{}\n", item.body).unwrap();
    }
    if !item.fix.is_empty() {
        writeln!(text, "Suggested change: {}\n", item.fix).unwrap();
    }
    writeln!(
        text,
        "Sources: {} · Lenses: {} · Verdict: {}\n",
        item.sources.join(", "),
        item.lenses.join(", "),
        if item.verdict.is_empty() {
            "unverified"
        } else {
            &item.verdict
        }
    )
    .unwrap();
}

pub(crate) mod saved;

fn canonical_time(text: &str) -> Result<String, String> {
    // Validate saved timestamps through the shared reader, but only emit RFC3339.
    parsed_time(text)?;
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
        .map_err(|e| format!("timestamp cannot be serialized as RFC3339: {e}"))
}

pub(crate) fn serialize_time<S: serde::Serializer>(
    text: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&canonical_time(text).map_err(serde::ser::Error::custom)?)
}

pub mod accounting;

pub(crate) fn parsed_time(text: &str) -> Result<chrono::DateTime<chrono::Utc>, String> {
    use chrono::Timelike;
    let date = crate::history::parse_date(text).ok_or("invalid report timestamp")?;
    if date.nanosecond() >= 1_000_000_000 {
        return Err("invalid report leap second".into());
    }
    Ok(date)
}
