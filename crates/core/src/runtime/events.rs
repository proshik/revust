//! Durable presentation event vocabulary; text is authored independently of wire keys.
use super::data::Finding;
use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    AgentStarted,
    AgentActivity,
    AgentProgress,
    AgentState,
    AgentDone,
    AgentRetried,
    AgentDegraded,
    Findings,
    Dropped,
    Stage,
    RateLimit,
}
#[derive(Clone, Serialize)]
pub struct Event {
    pub kind: Kind,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stage: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
    #[serde(serialize_with = "crate::report::serialize_time")]
    pub at: String,
}
impl Event {
    pub fn agent(kind: Kind, name: &str, text: &str) -> Self {
        Self {
            kind,
            agent: name.into(),
            stage: String::new(),
            text: text.into(),
            findings: vec![],
            at: String::new(),
        }
    }
    pub fn stage(name: &str) -> Self {
        Self {
            kind: Kind::Stage,
            agent: String::new(),
            stage: name.into(),
            text: String::new(),
            findings: vec![],
            at: String::new(),
        }
    }
    pub(crate) fn stamped(mut self) -> Self {
        if self.at.is_empty() {
            self.at = crate::report::accounting::timestamp(std::time::SystemTime::now());
        }
        self
    }
    pub(crate) fn json(self) -> Result<Vec<u8>, String> {
        let mut bytes = serde_json::to_vec(&self.stamped()).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}
