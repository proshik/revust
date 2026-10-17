//! Provider progress presented independently of raw transport and final answers.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", content = "text", rename_all = "snake_case")]
pub enum Event {
    Activity(String),
    Progress(String),
    Info(String),
    RateLimit(String),
}

/// Human-readable progress is bounded independently of the raw saved stream.
pub(crate) fn summary(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let mut visible: String = chars.by_ref().take(2000).collect();
    if chars.next().is_some() {
        visible.push('…');
    }
    visible
}
