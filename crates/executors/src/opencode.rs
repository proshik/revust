//! OpenCode V2 JSONL evidence; no default model or invocation is inferred.
//! Protocol observations and independent Rust implementation: docs/executor-protocols.md.
pub use crate::events::Event;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
const MAX_BYTES: usize = 8 << 20;
#[derive(Default, Debug)]
pub struct Result {
    pub output: Option<Value>,
    /// A final model response was understood, independently of its JSON validity.
    pub has_text: bool,
    pub tokens: Option<u64>,
    pub actual_model: Option<String>,
    pub error: Option<Error>,
    pub malformed_lines: usize,
    pub truncated: bool,
}
#[derive(Debug)]
pub struct Error {
    pub kind: String,
    pub message: String,
}
#[derive(Default)]
pub struct Decoder {
    pub result: Result,
    pending: Vec<u8>,
    stopped: bool,
    finished: bool,
    message: String,
    text: String,
    seen: BTreeSet<String>,
    steps: BTreeMap<String, Option<u64>>,
}
impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut events = vec![];
        if self.stopped || self.finished {
            return events;
        }
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            let size = self.pending.len().saturating_add(part.len());
            if size > MAX_BYTES || (size == MAX_BYTES && !part.ends_with(b"\n")) {
                self.result.truncated = true;
                self.stopped = true;
                self.pending.clear();
                break;
            }
            self.pending.extend_from_slice(part);
            if part.ends_with(b"\n") {
                let line = std::mem::take(&mut self.pending);
                events.extend(self.line(&line, false));
                if self.stopped {
                    break;
                }
            }
        }
        events
    }
    pub fn finish(&mut self) -> Vec<Event> {
        if self.finished {
            return vec![];
        }
        self.finished = true;
        let line = std::mem::take(&mut self.pending);
        let events = if self.stopped {
            vec![]
        } else {
            self.line(&line, true)
        };
        self.result.output = crate::codex::extract(&self.text);
        // A missing final step_finish was observed in the installed CLI. Partial
        // per-step totals are not presented as total run usage.
        if !self.steps.is_empty() && !self.result.truncated && self.result.malformed_lines == 0 {
            self.result.tokens = self
                .steps
                .values()
                .try_fold(0u64, |sum, n| sum.checked_add((*n)?));
        }
        events
    }
    fn line(&mut self, line: &[u8], tail: bool) -> Vec<Event> {
        let mut events = vec![];
        let wire: Value = match serde_json::from_slice(line) {
            Ok(wire) => wire,
            Err(e) => {
                if line.iter().all(u8::is_ascii_whitespace) {
                    return events;
                }
                self.result.malformed_lines += 1;
                self.result.truncated |= tail && e.is_eof();
                return events;
            }
        };
        let kind = wire["type"].as_str().unwrap_or_default();
        if kind == "error" {
            if let (Some(kind), Some(message)) = (
                wire["error"]["type"].as_str(),
                wire["error"]["message"].as_str(),
            ) {
                self.result.error = Some(Error {
                    kind: kind.into(),
                    message: message.into(),
                });
                events.push(Event::Info(label(message)));
            } else {
                self.result.malformed_lines += 1;
            }
            return events;
        }
        if !["step_start", "step_finish", "text", "tool_use"].contains(&kind) {
            return events;
        }
        let part = &wire["part"];
        let Some(message) = part["messageID"].as_str().filter(|s| !s.is_empty()) else {
            self.result.malformed_lines += 1;
            return events;
        };
        self.steps.entry(message.into()).or_default();
        match kind {
            "step_start" => {
                if self.message != message {
                    self.message = message.into();
                    self.text.clear();
                    self.result.has_text = false;
                }
            }
            "text" => {
                let Some(text) = part["text"].as_str() else {
                    self.result.malformed_lines += 1;
                    return events;
                };
                if let Some(id) = part["id"].as_str()
                    && !self.seen.insert(id.into())
                {
                    return events;
                }
                if self.message != message {
                    self.message = message.into();
                    self.text.clear();
                }
                self.result.has_text = true;
                if self.text.len().saturating_add(text.len()) > MAX_BYTES {
                    self.result.truncated = true;
                    self.stopped = true;
                    return events;
                }
                self.text.push_str(text);
                if !text.trim().is_empty() {
                    events.push(Event::Activity(label(text)));
                }
            }
            "step_finish" => {
                self.steps.insert(message.into(), usage(&part["tokens"]));
                if part["reason"] == "tool-calls" && self.message == message {
                    self.text.clear();
                    self.result.has_text = false;
                }
            }
            "tool_use" => {
                if self.message != message {
                    self.message = message.into();
                    self.text.clear();
                    self.result.has_text = false;
                }
                let tool = part["tool"].as_str().unwrap_or("tool");
                let state = &part["state"];
                let input = &state["input"];
                let arg = ["path", "pattern", "command"]
                    .into_iter()
                    .find_map(|key| input[key].as_str());
                events.push(Event::Progress(label(
                    &arg.map_or_else(|| tool.into(), |arg| format!("{tool} {arg}")),
                )));
                if let Some(error) = state["error"].as_str() {
                    events.push(Event::Info(label(&format!("{tool}: {error}"))));
                }
            }
            _ => (),
        }
        events
    }
}
fn usage(tokens: &Value) -> Option<u64> {
    [
        tokens["input"].as_u64(),
        tokens["output"].as_u64(),
        tokens["cache"]["read"].as_u64(),
        tokens["cache"]["write"].as_u64(),
    ]
    .into_iter()
    .try_fold(0u64, |sum, n| sum.checked_add(n?))
}
fn label(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(2000)
        .collect()
}

#[cfg(unix)]
pub mod isolated;
