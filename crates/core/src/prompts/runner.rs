//! Provider selection and partial override semantics for Markdown profiles.
use serde::Serialize;
pub const EXECUTORS: &[&str] = &["claude", "codex"];
pub const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
pub const STAGES: &[&str] = &["synthesis", "verify"];
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Runner {
    pub executor: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
}
impl Runner {
    pub fn parse(input: &str) -> Result<Self, String> {
        let selection = input.trim();
        let mut result = Self::default();
        if selection.is_empty() {
            return Ok(result);
        }
        let end = match selection.rfind(':') {
            Some(colon) => {
                let effort = &selection[colon + 1..];
                if !EFFORTS.contains(&effort) {
                    return Err(format!("unknown effort {effort:?}"));
                }
                result.effort = effort.to_owned();
                colon
            }
            None => selection.len(),
        };
        let provider_model = &selection[..end];
        let fields = provider_model.splitn(2, '/').collect::<Vec<_>>();
        let provider = fields[0];
        if let Some(model) = fields.get(1) {
            if model.is_empty() {
                return Err(format!("no model after the slash in {provider_model:?}"));
            }
            result.model = (*model).to_owned();
        }
        if !EXECUTORS.contains(&provider) {
            return Err(format!("unknown executor {provider:?}"));
        }
        result.executor = provider.to_owned();
        Ok(result)
    }
    pub fn inherit(&self, fallback: &Self) -> Self {
        match self.executor.as_str() {
            "" => fallback.clone(),
            _ => Self {
                executor: self.executor.clone(),
                model: match (self.model.is_empty(), self.executor == fallback.executor) {
                    (true, true) => fallback.model.clone(),
                    _ => self.model.clone(),
                },
                effort: if self.effort.is_empty() {
                    fallback.effort.clone()
                } else {
                    self.effort.clone()
                },
            },
        }
    }
    pub fn stage(&self, file: &Self, override_runner: Option<&Self>) -> Self {
        let refined =
            override_runner.map_or_else(|| file.clone(), |selected| selected.inherit(file));
        refined.inherit(self)
    }
}
