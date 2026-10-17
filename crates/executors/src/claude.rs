//! Claude JSONL observations, kept separate from structured stage acceptance.
pub use crate::events::Event;
use crate::{events::summary, framing::Lines};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default, Debug)]
pub struct Result {
    pub output: Option<Value>,
    pub actual_model: Option<String>,
    pub tokens: Option<i64>,
    pub first_token_ms: Option<i64>,
    pub rate_limited: bool,
    pub malformed_lines: usize,
    pub truncated: bool,
    pub error: Option<String>,
}
#[derive(Default)]
pub struct Decoder {
    pub result: Result,
    lines: Lines,
    terminal_seen: bool,
}
impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        for record in self.lines.push(bytes) {
            events.extend(self.record(&record, false));
        }
        self.result.truncated |= self.lines.overflow;
        events
    }
    pub fn finish(&mut self) -> Vec<Event> {
        match self.lines.finish() {
            Some(record) => self.record(&record, true),
            None => Vec::new(),
        }
    }
    fn record(&mut self, bytes: &[u8], tail: bool) -> Vec<Event> {
        // Saved raw bytes remain authoritative; progress tolerates replacement
        // characters in a provider string without splitting a UTF-8 chunk.
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        let parsed = serde_json::from_str::<Value>(text);
        let value = match parsed {
            Ok(value) => value,
            Err(error) => {
                self.result.truncated |= tail && error.is_eof();
                self.result.malformed_lines += 1;
                return Vec::new();
            }
        };
        let result = match value.get("type").and_then(Value::as_str) {
            Some("assistant") => serde_json::from_value::<AssistantEnvelope>(value)
                .map(|record| assistant_events(record.message)),
            Some("rate_limit_event") => {
                serde_json::from_value::<RateEnvelope>(value).map(|record| {
                    self.result.rate_limited = record
                        .rate_limit_info
                        .and_then(|info| info.status)
                        .as_deref()
                        == Some("rejected");
                    if self.result.rate_limited {
                        vec![Event::RateLimit("Provider rejected a request".into())]
                    } else {
                        Vec::new()
                    }
                })
            }
            Some("result") => match serde_json::from_value::<Terminal>(value) {
                Ok(record) => Ok(self.terminal(record)),
                Err(error) => {
                    self.terminal_seen = true;
                    self.result.error = Some("Invalid terminal result fields".into());
                    Err(error)
                }
            },
            _ => return Vec::new(),
        };
        result.unwrap_or_else(|_| {
            self.result.malformed_lines += 1;
            Vec::new()
        })
    }
    fn terminal(&mut self, record: Terminal) -> Vec<Event> {
        if self.terminal_seen {
            self.result.error = Some("Multiple terminal result records".into());
            return Vec::new();
        }
        self.terminal_seen = true;
        let mut events = Vec::new();
        let denied = record.permission_denials.unwrap_or_default();
        if !denied.is_empty() {
            let mut tools: Vec<_> = denied
                .into_iter()
                .flatten()
                .filter_map(|denial| denial.tool_name)
                .collect();
            tools.sort();
            tools.dedup();
            events.push(Event::Info(summary(&format!(
                "Permission restrictions affected tools: {}",
                tools.join(", ")
            ))));
        }
        self.result.error = (record.is_error == Some(true)
            || record
                .subtype
                .as_deref()
                .is_some_and(|kind| kind.starts_with("error")))
        .then(|| "Provider reported an unsuccessful terminal result".into());
        self.result.output = record.structured_output;
        self.result.first_token_ms = record.ttft_ms.filter(|value| *value >= 0);
        self.result.tokens = record.usage.and_then(Usage::total);
        let mut candidates: Vec<_> = record
            .model_usage
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(model, observation)| {
                let count = observation?.output_tokens?;
                (!model.is_empty() && count >= 0).then_some((model, count))
            })
            .collect();
        candidates.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
        self.result.actual_model = candidates.into_iter().next().map(|(model, _)| model);
        events
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct AssistantEnvelope {
    message: Option<Message>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Message {
    content: Option<Vec<Option<Block>>>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Block {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    name: Option<String>,
    input: Value,
}
fn assistant_events(message: Option<Message>) -> Vec<Event> {
    let blocks: Vec<_> = message
        .and_then(|message| message.content)
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect();
    let narration = blocks
        .iter()
        .filter(|block| block.kind.as_deref() == Some("text"))
        .filter_map(|block| block.text.as_deref())
        .map(summary)
        .find(|text| !text.is_empty());
    let operation = blocks.iter().find(|block| {
        block.kind.as_deref() == Some("tool_use")
            && block
                .name
                .as_deref()
                .is_some_and(|name| !name.is_empty() && name != "StructuredOutput")
    });
    let mut events = Vec::new();
    if let Some(text) = narration {
        events.push(Event::Activity(text));
    }
    if let Some(operation) = operation {
        let name = operation.name.as_deref().unwrap();
        let argument = [
            "file_path",
            "path",
            "pattern",
            "command",
            "query",
            "description",
            "url",
        ]
        .into_iter()
        .filter_map(|key| operation.input.get(key).and_then(Value::as_str))
        .find(|text| !text.trim().is_empty());
        let text =
            argument.map_or_else(|| name.to_owned(), |argument| format!("{name}: {argument}"));
        events.push(Event::Progress(summary(&text)));
    }
    events
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Terminal {
    #[serde(deserialize_with = "present")]
    structured_output: Option<Value>,
    #[serde(rename = "modelUsage")]
    model_usage: Option<BTreeMap<String, Option<ModelUsage>>>,
    usage: Option<Usage>,
    ttft_ms: Option<i64>,
    is_error: Option<bool>,
    subtype: Option<String>,
    permission_denials: Option<Vec<Option<Denial>>>,
}
fn present<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct ModelUsage {
    #[serde(rename = "outputTokens")]
    output_tokens: Option<i64>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Usage {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
}
impl Usage {
    fn total(self) -> Option<i64> {
        let observations = [
            self.input_tokens?,
            self.output_tokens?,
            self.cache_read_input_tokens.unwrap_or(0),
            self.cache_creation_input_tokens.unwrap_or(0),
        ];
        observations.into_iter().try_fold(0i64, |sum, value| {
            if value < 0 {
                None
            } else {
                sum.checked_add(value)
            }
        })
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Denial {
    tool_name: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct RateEnvelope {
    rate_limit_info: Option<RateInfo>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct RateInfo {
    status: Option<String>,
}

/// Stream transport and protocol evidence; the pipeline validates the stage schema.
#[cfg(unix)]
pub struct Execution {
    pub protocol: Result,
    pub process: crate::supervision::Outcome,
}
#[cfg(unix)]
pub fn run(
    settings: &crate::configured::Settings,
    request: revust_core::request::Request,
    control: &revust_core::runtime::RunControl,
    mut raw: impl FnMut(crate::supervision::Stream, &[u8]) -> std::io::Result<()>,
    mut emit: impl FnMut(Event),
) -> std::io::Result<Execution> {
    let mut decoder = Decoder::default();
    let process = settings.run(request, control, |stream, bytes| {
        raw(stream, bytes)?;
        if stream == crate::supervision::Stream::Stdout {
            for event in decoder.feed(bytes) {
                emit(event);
            }
            // Any stdout delivery is transport activity, including partial records.
            Ok(!bytes.is_empty())
        } else {
            // A complete diagnostic line is transport activity.
            Ok(bytes.contains(&b'\n'))
        }
    })?;
    for event in decoder.finish() {
        emit(event);
    }
    Ok(Execution {
        protocol: decoder.result,
        process,
    })
}
