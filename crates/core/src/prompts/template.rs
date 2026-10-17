//! Parse document boundaries and substitute only tokens in the authored text.
use std::{collections::BTreeMap, ops::Range};
pub const VARIABLES: &[&str] = &[
    "SCOPE", "GOAL", "PROFILE", "CONTEXT", "WORKDIR", "FINDINGS", "SOURCES",
];
const HISTORY_GUARD: &str = "Saved rounds provide leads, not conclusions. Inspect report.md for the human report and findings.json for structured records when relevant. Establish each current claim from current evidence: earlier agreement cannot confirm it, and earlier silence cannot rule it out.";
#[derive(Debug, PartialEq, Eq)]
pub struct Parts {
    pub meta: String,
    pub body: String,
}
pub fn split(input: &str) -> Result<Parts, String> {
    let normalized = input.replace("\r\n", "\n");
    let Some(payload) = normalized.strip_prefix("---\n") else {
        return Ok(Parts {
            meta: String::new(),
            body: normalized,
        });
    };
    let delimiter = payload
        .match_indices("---")
        .find_map(|(offset, _)| {
            let before = offset == 0 || payload.as_bytes()[offset - 1] == b'\n';
            let after =
                offset + 3 == payload.len() || payload.as_bytes().get(offset + 3) == Some(&b'\n');
            (before && after).then_some(offset)
        })
        .ok_or("front matter is opened but never closed")?;
    let remaining = &payload[delimiter + 3..];
    Ok(Parts {
        meta: payload[..delimiter].to_owned(),
        body: remaining.strip_prefix('\n').unwrap_or(remaining).to_owned(),
    })
}

/// An iterator refers only to the template, so inserted data cannot become code.
struct Tokens<'a> {
    text: &'a str,
    cursor: usize,
}
impl<'a> Iterator for Tokens<'a> {
    type Item = (Range<usize>, &'a str);
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(relative) = self.text[self.cursor..].find("{{") {
            let start = self.cursor + relative;
            self.cursor = start + 1; // Permit overlapping starts, e.g. triple braces.
            let after_open = &self.text[start + 2..];
            let length = after_open
                .bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
                .count();
            if length > 0 && after_open[length..].starts_with("}}") {
                self.cursor = start + length + 4;
                return Some((start..self.cursor, &after_open[..length]));
            }
        }
        None
    }
}
fn tokens(text: &str) -> Tokens<'_> {
    Tokens { text, cursor: 0 }
}
pub fn validate(body: &str) -> Result<(), String> {
    match tokens(body).find(|(_, name)| !VARIABLES.contains(name)) {
        Some((_, name)) => Err(format!("unknown variable {{{{{name}}}}}")),
        None => Ok(()),
    }
}
pub fn render(
    body: &str,
    vars: &BTreeMap<String, String>,
    history: &str,
) -> Result<String, String> {
    let mut pieces = Vec::new();
    let mut copied = 0;
    for (range, name) in tokens(body) {
        let value = vars
            .get(name)
            .ok_or_else(|| format!("unresolved variable {{{{{name}}}}}"))?;
        pieces.extend([&body[copied..range.start], value.as_str()]);
        copied = range.end;
    }
    pieces.push(&body[copied..]);
    if !history.is_empty() {
        pieces.extend(["\n\n", history.trim(), "\n\n", HISTORY_GUARD]);
    }
    Ok(pieces.concat())
}
