//! Codex answer and diagnostic observations. No requested setting is telemetry.
pub use crate::events::Event;
use crate::framing::Lines;
use serde_json::Value;

#[derive(Default, Debug)]
enum Footer {
    #[default]
    Absent,
    CountNext,
    Count(u64),
}
#[derive(Default, Debug)]
pub struct Stderr {
    pub actual_model: Option<String>,
    pub diagnostic: Option<String>,
    pub session_id: Option<String>,
    footer: Footer,
    headers: u8,
}
impl Stderr {
    pub fn line(&mut self, line: &str) -> Option<String> {
        if self.session_id.is_none() {
            self.session_id = crate::rollout::session_id(line);
        }
        let text = line.trim();
        if text.is_empty() {
            return None;
        }
        self.footer = if text.eq_ignore_ascii_case("tokens used") {
            Footer::CountNext
        } else if matches!(self.footer, Footer::CountNext) {
            text.replace(',', "")
                .parse::<i64>()
                .ok()
                .and_then(|count| u64::try_from(count).ok())
                .map_or(Footer::Absent, Footer::Count)
        } else {
            Footer::Absent
        };
        let (label, value) = text.split_once(':')?;
        let label = label.trim().to_ascii_lowercase();
        if label == "error" {
            self.diagnostic = Some(text.to_owned());
        }
        let mask = match label.as_str() {
            "model" => 1,
            "sandbox" => 2,
            "reasoning effort" => 4,
            _ => return None,
        };
        let value = value.trim();
        if value.is_empty() || self.headers & mask != 0 {
            return None;
        }
        self.headers |= mask;
        if mask == 1 {
            self.actual_model = Some(value.into());
        }
        Some(format!("{label}: {value}"))
    }
    pub fn tokens(&self) -> Option<u64> {
        if let Footer::Count(count) = self.footer {
            Some(count)
        } else {
            None
        }
    }
}

/// Locate complete outer objects before parsing. A nested object can never
/// substitute for an incomplete outer answer, including inside malformed prose.
pub fn extract(raw: &str) -> Option<Value> {
    let bytes = raw.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let start = cursor + bytes[cursor..].iter().position(|byte| *byte == b'{')?;
        let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
        let mut boundary = None;
        for (relative, byte) in bytes[start..].iter().copied().enumerate() {
            if quoted {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quoted = false;
                }
            } else {
                match byte {
                    b'"' => quoted = true,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            boundary = Some(start + relative + 1);
                            break;
                        }
                    }
                    _ => (),
                }
            }
        }
        let end = boundary?;
        match serde_json::from_slice::<Value>(&bytes[start..end]) {
            Ok(value) => return Some(value),
            Err(error) if error.is_eof() => return None,
            Err(_) => cursor = end,
        }
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
pub enum Classification {
    Delivered,
    Transient(String),
    RateLimit(String),
    Diagnostic(String),
    NoDiagnostic,
}

pub fn classify(
    raw: &str,
    diagnostic: Option<&str>,
    exit_success: bool,
    idle_timeout: bool,
) -> Classification {
    if exit_success {
        return Classification::Delivered;
    }
    let tail = &raw.as_bytes()[raw.len().saturating_sub(2048)..];
    let evidence = [
        String::from_utf8_lossy(tail).into_owned(),
        diagnostic.unwrap_or_default().to_owned(),
    ]
    .join("\n")
    .to_lowercase();
    // Fixed provider diagnostic signatures are protocol data, ordered by retry policy.
    let transient = [
        "api error: 529",
        "api error: 502",
        "api error: 503",
        "api error: 504",
    ]
    .into_iter()
    .find(|signature| evidence.contains(signature));
    let quota = [
        "rate limit exceeded",
        "rate limit reached",
        "429 too many requests",
        "quota exceeded",
        "insufficient_quota",
        "you've hit your usage limit",
    ]
    .into_iter()
    .find(|signature| evidence.contains(signature));
    match (
        idle_timeout,
        transient,
        quota,
        diagnostic.filter(|text| !text.is_empty()),
    ) {
        (false, Some(signature), _, _) => Classification::Transient(signature.into()),
        (_, _, Some(signature), _) => Classification::RateLimit(signature.into()),
        (false, _, _, Some(text)) => Classification::Diagnostic(text.into()),
        _ => Classification::NoDiagnostic,
    }
}

#[derive(Debug)]
pub struct Result {
    pub output: Option<Value>,
    pub actual_model: Option<String>,
    pub tokens: Option<u64>,
    pub classification: Classification,
    pub truncated: bool,
}
#[derive(Default)]
pub struct Decoder {
    answer: Vec<u8>,
    diagnostics: Lines,
    stderr: Stderr,
    oversized: bool,
    announced: bool,
}
impl Decoder {
    pub fn stdout(&mut self, bytes: &[u8]) -> Vec<Event> {
        const ANSWER_LIMIT: usize = 64 << 20;
        let available = ANSWER_LIMIT - self.answer.len();
        self.answer
            .extend_from_slice(&bytes[..bytes.len().min(available)]);
        self.oversized |= bytes.len() > available;
        if bytes.is_empty() || self.announced {
            Vec::new()
        } else {
            self.announced = true;
            vec![Event::Progress("Receiving provider response".into())]
        }
    }
    pub fn stderr(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        for record in self.diagnostics.push(bytes) {
            if let Some(info) = self.stderr.line(&String::from_utf8_lossy(&record)) {
                events.push(Event::Info(info));
            }
        }
        events
    }
    pub fn finish(mut self, exit_success: bool, idle_timeout: bool) -> (Result, Vec<Event>) {
        let mut events = Vec::new();
        if let Some(record) = self.diagnostics.finish()
            && let Some(info) = self.stderr.line(&String::from_utf8_lossy(&record))
        {
            events.push(Event::Info(info));
        }
        let raw = String::from_utf8_lossy(&self.answer);
        let classification = classify(
            &raw,
            self.stderr.diagnostic.as_deref(),
            exit_success,
            idle_timeout,
        );
        if let Classification::RateLimit(signature) = &classification {
            events.push(Event::RateLimit(signature.clone()));
        }
        let result = Result {
            output: extract(&raw),
            actual_model: self.stderr.actual_model.clone(),
            tokens: if self.diagnostics.overflow {
                None
            } else {
                self.stderr.tokens()
            },
            classification,
            truncated: self.oversized || self.diagnostics.overflow,
        };
        (result, events)
    }
}
#[cfg(unix)]
pub struct Execution {
    pub protocol: Result,
    pub process: crate::supervision::Outcome,
}
/// Pipe-only entry point for callers without a qualified session directory.
#[cfg(unix)]
pub fn run(
    settings: &crate::configured::Settings,
    request: revust_core::request::Request,
    control: &revust_core::runtime::RunControl,
    raw: impl FnMut(crate::supervision::Stream, &[u8]) -> std::io::Result<()>,
    emit: impl FnMut(Event),
) -> std::io::Result<Execution> {
    run_with_rollout(settings, request, control, None, raw, emit)
}
/// The caller supplies the selected Codex home. Only the UUID reported by this
/// child may select a rollout; no other session's contents are opened.
#[cfg(unix)]
pub fn run_with_rollout(
    settings: &crate::configured::Settings,
    request: revust_core::request::Request,
    control: &revust_core::runtime::RunControl,
    home: Option<&std::path::Path>,
    mut raw: impl FnMut(crate::supervision::Stream, &[u8]) -> std::io::Result<()>,
    emit: impl FnMut(Event),
) -> std::io::Result<Execution> {
    use crate::supervision::{Stream, Termination};
    use std::{
        cell::RefCell,
        time::{Duration, Instant},
    };
    struct Observation<'a> {
        home: Option<&'a std::path::Path>,
        tail: Option<crate::rollout::Tail>,
        due: Instant,
    }
    impl Observation<'_> {
        fn poll(
            &mut self,
            id: Option<&str>,
            final_pass: bool,
            emit: &mut impl FnMut(Event),
        ) -> bool {
            let Some(home) = self.home else {
                return false;
            };
            if !final_pass && Instant::now() < self.due {
                return false;
            }
            self.due = Instant::now() + Duration::from_millis(500);
            if self.tail.is_none() {
                self.tail = id.and_then(|id| crate::rollout::Tail::new(home, id).ok());
            }
            let Some(tail) = self.tail.as_mut() else {
                return false;
            };
            let mut activity = false;
            // Terminal observation is bounded even if an escaped writer keeps
            // appending. It cannot extend the already-ended idle watchdog.
            for _ in 0..if final_pass { 8 } else { 1 } {
                let Ok(update) = tail.poll() else {
                    break;
                };
                for event in update.events {
                    emit(event);
                }
                activity |= update.advanced;
                if !update.advanced {
                    break;
                }
            }
            activity && !final_pass
        }
    }
    let decoder = RefCell::new(Decoder::default());
    let events = RefCell::new(emit);
    let observer = RefCell::new(Observation {
        home,
        tail: None,
        due: Instant::now(),
    });
    let observe = |final_pass| {
        observer.borrow_mut().poll(
            decoder.borrow().stderr.session_id.as_deref(),
            final_pass,
            &mut *events.borrow_mut(),
        )
    };
    let process = settings.run_observed(
        request,
        control,
        |stream, bytes| {
            raw(stream, bytes)?;
            let decoded = match stream {
                Stream::Stdout => decoder.borrow_mut().stdout(bytes),
                Stream::Stderr => decoder.borrow_mut().stderr(bytes),
            };
            for event in decoded {
                (events.borrow_mut())(event);
            }
            Ok(match stream {
                Stream::Stdout => !bytes.is_empty(),
                Stream::Stderr => bytes.contains(&b'\n'),
            })
        },
        || Ok(observe(false)),
    )?;
    observe(true);
    let (protocol, trailing) = decoder.into_inner().finish(
        process.status.is_some_and(|status| status.success()),
        process.termination == Termination::IdleTimeout,
    );
    for event in trailing {
        (events.borrow_mut())(event);
    }
    Ok(Execution { protocol, process })
}
