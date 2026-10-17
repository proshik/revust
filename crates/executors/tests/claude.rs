use revust_executors::claude::{Decoder, Event};
use serde_json::json;
#[test]
fn chunked_stream_keeps_progress_separate_and_uses_observed_telemetry() {
    let mut d = Decoder::default();
    let mut events = vec![];
    for bytes in include_bytes!("fixtures/claude-v2-stream.jsonl").chunks(7) {
        events.extend(d.feed(bytes));
    }
    events.extend(d.finish());
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0], Event::Activity(text) if text.contains("unchanged helper")));
    assert!(
        matches!(&events[1], Event::Progress(text) if text.contains("Read") && text.contains("helper.rs"))
    );
    assert!(matches!(&events[2], Event::Info(text) if text.contains("Bash")));
    assert_eq!(d.result.output, Some(json!({"findings":[]})));
    assert_eq!(d.result.actual_model.as_deref(), Some("synthetic-a"));
    assert_eq!(d.result.tokens, Some(19));
    assert_eq!(d.result.first_token_ms, Some(37));
    assert!(!d.result.rate_limited);
    assert_eq!(d.result.malformed_lines, 0);
    assert!(!d.result.truncated);
}
#[test]
fn malformed_and_truncated_streams_keep_prior_evidence_without_inventing_a_result() {
    let mut d = Decoder::default();
    d.feed(b"not-json\n{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":5}]}}\n");
    d.feed(b"{\"type\":\"result\",\"structured_output\":{");
    d.finish();
    assert!(d.result.output.is_none());
    assert!(d.result.truncated);
    assert_eq!(d.result.malformed_lines, 3);
    let mut d = Decoder::default();
    d.feed(b"{\"type\":\"result\",\"structured_output\":null}");
    d.finish();
    assert_eq!(d.result.output, Some(json!(null)));
    assert!(d.result.tokens.is_none());
    assert!(d.result.actual_model.is_none());
    assert!(!d.result.truncated);
}
#[test]
fn rate_warning_is_not_rejection_and_text_clipping_preserves_unicode() {
    let mut d = Decoder::default();
    let text = "я".repeat(2001);
    let line = json!({"type":"assistant","message":{"content":[{"type":"text","text":text}]}})
        .to_string()
        + "\n";
    let events = d.feed(line.as_bytes());
    assert!(
        matches!(&events[0], Event::Activity(text) if text.starts_with(&"я".repeat(2000)) && text.chars().count() <= 2003)
    );
    assert!(matches!(
        d.feed(b"{\"type\":\"rate_limit_event\",\"rate_limit_info\":{\"status\":\"rejected\"}}\n")
            .as_slice(),
        [Event::RateLimit(_)]
    ));
    assert!(d.result.rate_limited);
    d.feed(
        b"{\"type\":\"rate_limit_event\",\"rate_limit_info\":{\"status\":\"allowed_warning\"}}\n",
    );
    assert!(!d.result.rate_limited);
}

#[test]
fn oversize_line_stops_decoding_but_preserves_preceding_result() {
    let mut d = Decoder::default();
    d.feed(b"{\"type\":\"result\",\"structured_output\":{\"findings\":[]}}\n");
    for _ in 0..129 {
        d.feed(&vec![b'x'; 65536]);
    }
    d.feed(b"\n{\"type\":\"result\",\"structured_output\":null}\n");
    d.finish();
    assert!(d.result.truncated);
    assert_eq!(d.result.output, Some(json!({"findings":[]})));
}
#[cfg(unix)]
#[test]
fn controlled_process_keeps_valid_nonzero_result_and_archives_bytes_before_parsing() {
    use revust_core::{
        request::Request,
        runtime::{Limits, MonotonicClock, RunControl},
    };
    use revust_executors::{
        claude,
        configured::Settings,
        supervision::{self, Stream},
    };
    use std::{collections::BTreeMap, sync::Arc, time::Duration};
    let control = RunControl::new(
        Limits {
            max_calls: None,
            elapsed: None,
            max_parallel: 1,
            stage_parallel: [1; 3],
        },
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    let settings = Settings {
        preserve_anthropic_api_key: false,
        program: "/bin/sh".into(),
        environment: BTreeMap::new(),
        limits: supervision::Limits {
            idle: Some(Duration::from_secs(1)),
            hard: Some(Duration::from_secs(2)),
        },
    };
    let input = include_str!("fixtures/claude-v2-stream.jsonl");
    let mut stdout = vec![];
    let mut stderr = vec![];
    let mut events = vec![];
    let execution = claude::run(
        &settings,
        Request {
            program: "unused".into(),
            argv: vec![
                "-c".into(),
                "/bin/cat; printf diagnostic >&2; exit 7".into(),
            ],
            stdin: input.into(),
            cwd: std::env::temp_dir(),
        },
        &control,
        |stream, bytes| {
            match stream {
                Stream::Stdout => stdout.extend_from_slice(bytes),
                Stream::Stderr => stderr.extend_from_slice(bytes),
            }
            Ok(())
        },
        |event| events.push(event),
    )
    .unwrap();
    assert_eq!(stdout, input.as_bytes());
    assert_eq!(stderr, b"diagnostic");
    assert_eq!(execution.process.status.unwrap().code(), Some(7));
    assert_eq!(execution.protocol.output, Some(json!({"findings":[]})));
    assert_eq!(execution.protocol.tokens, Some(19));
    assert_eq!(events.len(), 3);
}

#[test]
fn reference_whitespace_and_invalid_utf8_inside_json_strings_are_tolerated() {
    let mut d = Decoder::default();
    let input = "\u{a0}{\"type\":\"result\",\"structured_output\":{\"title\":\"".as_bytes();
    let mut bytes = input.to_vec();
    bytes.push(0xff);
    bytes.extend_from_slice(b"\"}}\n");
    d.feed(&bytes);
    assert_eq!(d.result.output, Some(json!({"title":"\u{fffd}"})));
}

#[test]
fn terminal_errors_and_incomplete_usage_remain_distinct_from_complete_delivery() {
    let mut decoder = Decoder::default();
    decoder.feed(br#"{"type":"system","model":"requested-only"}
{"type":"result","subtype":"error_during_execution","is_error":true,"structured_output":{"findings":[]},"usage":{},"modelUsage":{"unobserved":{}}}
"#);
    decoder.finish();
    assert!(decoder.result.error.is_some());
    assert_eq!(decoder.result.output, Some(json!({"findings":[]})));
    assert_eq!(decoder.result.tokens, None);
    assert_eq!(decoder.result.actual_model, None);
    assert!(decoder.feed(b"{}\n").is_empty());
    assert!(decoder.finish().is_empty());
    for usage in [
        json!({"input_tokens":-1,"output_tokens":5}),
        json!({"input_tokens":i64::MAX,"output_tokens":1}),
        json!({"input_tokens":7}),
    ] {
        let mut decoder = Decoder::default();
        decoder.feed((json!({"type":"result","usage":usage}).to_string() + "\n").as_bytes());
        assert_eq!(decoder.result.tokens, None);
    }
}

#[test]
fn every_chunk_boundary_keeps_multibyte_records_and_explicit_zero() {
    let data = (json!({"type":"assistant","message":{"content":[{"type":"text","text":"Inspect λ context"}]}}).to_string()+"\n"+
        &json!({"type":"result","structured_output":{"findings":[]},"usage":{"input_tokens":0,"output_tokens":0},"modelUsage":{"seen":{"outputTokens":0}}}).to_string()).into_bytes();
    for cut in 0..=data.len() {
        let mut decoder = Decoder::default();
        let mut events = decoder.feed(&data[..cut]);
        events.extend(decoder.feed(&data[cut..]));
        events.extend(decoder.finish());
        assert_eq!(decoder.result.tokens, Some(0));
        assert_eq!(decoder.result.actual_model.as_deref(), Some("seen"));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Activity(_)))
                .count(),
            1
        );
        assert!(!decoder.result.truncated);
    }
}

#[test]
fn terminal_payload_is_not_replaced_by_another_result_record() {
    let mut decoder = Decoder::default();
    decoder.feed(b"{\"type\":\"result\",\"structured_output\":{\"id\":\"first\"}}\n");
    decoder.feed(b"{\"type\":\"result\",\"structured_output\":{\"id\":\"second\"}}\n");
    assert_eq!(decoder.result.output, Some(json!({"id":"first"})));
    assert!(decoder.result.error.is_some());
}

#[test]
fn invalid_terminal_fields_cannot_bless_an_earlier_payload() {
    let mut decoder = Decoder::default();
    decoder.feed(b"{\"type\":\"result\",\"structured_output\":{\"id\":\"saved\"}}\n");
    decoder.feed(b"{\"type\":\"result\",\"usage\":false}\n");
    assert_eq!(decoder.result.output, Some(json!({"id":"saved"})));
    assert!(decoder.result.error.is_some());
    assert_eq!(decoder.result.malformed_lines, 1);
}
