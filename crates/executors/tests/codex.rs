use revust_executors::codex::{Classification, Stderr, classify, extract};
use serde_json::json;
#[test]
fn json_extraction_skips_prose_but_never_salvages_nested_truncated_answers() {
    assert_eq!(
        extract("prose {not json}\n```json\n{\"findings\":[]}\n```"),
        Some(json!({"findings":[]}))
    );
    assert_eq!(extract("{\"findings\":[{\"id\":\"nested\"}"), None);
    assert_eq!(
        extract("prefix {\"findings\":[]} trailing {\"another\":1}"),
        Some(json!({"findings":[]}))
    );
    assert_eq!(extract("no answer"), None);
}
#[test]
fn stderr_model_headers_are_once_only_and_tokens_must_be_the_real_footer() {
    let mut s = Stderr::default();
    assert_eq!(s.line("model: observed"), Some("model: observed".into()));
    assert_eq!(s.line("model: ignored"), None);
    assert_eq!(s.actual_model.as_deref(), Some("observed"));
    s.line("tokens used");
    s.line("1,234");
    assert_eq!(s.tokens(), Some(1234));
    s.line("echoed prompt continues");
    assert_eq!(s.tokens(), None);
    s.line("ERROR: first");
    s.line("error: last");
    assert_eq!(s.diagnostic.as_deref(), Some("error: last"));
    s.line("tokens used");
    s.line("0");
    s.line(" ");
    assert_eq!(s.tokens(), Some(0));
}
#[test]
fn rate_patterns_only_apply_to_failed_tail_and_transient_tier_wins() {
    assert_eq!(
        classify("rate limit exceeded", None, true, false),
        Classification::Delivered
    );
    assert_eq!(
        classify(
            "",
            Some("error: API error: 503; quota exceeded"),
            false,
            false
        ),
        Classification::Transient("api error: 503".into())
    );
    assert_eq!(
        classify("", Some("error: quota exceeded"), false, true),
        Classification::RateLimit("quota exceeded".into())
    );
    assert_eq!(
        classify(
            &("rate limit exceeded".to_owned() + &"x".repeat(3000)),
            None,
            false,
            false
        ),
        Classification::NoDiagnostic
    );
    assert_eq!(
        classify("", Some("error: API error: 500"), false, false),
        Classification::Diagnostic("error: API error: 500".into())
    );
    assert_eq!(
        classify("", Some("error: API error: 503"), false, true),
        Classification::NoDiagnostic
    );
}

#[test]
fn byte_chunks_preserve_headers_and_do_not_treat_banner_as_activity() {
    use revust_executors::codex::{Decoder, Event};
    let mut d = Decoder::default();
    let mut events = vec![];
    for bytes in include_bytes!("fixtures/codex-v0162-stderr.txt").chunks(3) {
        events.extend(d.stderr(bytes));
    }
    assert!(events.iter().all(|e| matches!(e, Event::Info(_))));
    assert!(matches!(
        d.stdout(b"{\"find").as_slice(),
        [Event::Progress(_)]
    ));
    assert!(d.stdout(b"ings\":[]}").is_empty());
    let (result, extra) = d.finish(true, false);
    assert!(extra.is_empty());
    assert_eq!(result.output, Some(json!({"findings":[]})));
    assert_eq!(result.tokens, Some(1024));
    assert_eq!(result.actual_model.as_deref(), Some("synthetic-observed"));
}
#[cfg(unix)]
#[test]
fn controlled_transport_preserves_diagnostics_and_rate_limit_evidence() {
    use revust_core::{
        request::Request,
        runtime::{Limits, MonotonicClock, RunControl},
    };
    use revust_executors::{
        codex,
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
    let mut err = vec![];
    let mut events = vec![];
    let result = codex::run(
        &settings,
        Request {
            program: "unused".into(),
            argv: vec![
                "-c".into(),
                "printf 'model: synthetic\nerror: quota exceeded\n' >&2; exit 7".into(),
            ],
            stdin: String::new(),
            cwd: std::env::temp_dir(),
        },
        &control,
        |stream, bytes| {
            if stream == Stream::Stderr {
                err.extend_from_slice(bytes);
            }
            Ok(())
        },
        |event| events.push(event),
    )
    .unwrap();
    assert_eq!(err, b"model: synthetic\nerror: quota exceeded\n");
    assert_eq!(result.process.status.unwrap().code(), Some(7));
    assert!(result.protocol.output.is_none());
    assert_eq!(
        result.protocol.classification,
        Classification::RateLimit("quota exceeded".into())
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, codex::Event::RateLimit(_)))
    );
}

#[test]
fn extraction_tracks_string_escapes_and_refuses_an_unclosed_outer_record() {
    let payload = json!({"findings":[{"body":"quoted brace } then { and \\\"","id":"stable"}]});
    assert_eq!(
        extract(&format!("prefix {{bad}}\n{payload}\ntrailer")),
        Some(payload)
    );
    assert_eq!(extract("{broken outer {\"findings\":[]}"), None);
}

#[test]
fn oversized_transport_preserves_payload_as_incomplete_evidence() {
    use revust_executors::codex::Decoder;
    let mut decoder = Decoder::default();
    decoder.stdout(b"{\"findings\":[]}\n");
    let chunk = vec![b'x'; 1 << 20];
    for _ in 0..65 {
        decoder.stdout(&chunk);
    }
    let (result, _) = decoder.finish(true, false);
    assert!(result.truncated);
    assert_eq!(result.output, Some(json!({"findings":[]})));
    let mut decoder = Decoder::default();
    decoder.stderr(b"tokens used\n3\n");
    for _ in 0..8 {
        decoder.stderr(&chunk);
    }
    let (result, _) = decoder.finish(false, false);
    assert!(result.truncated);
    assert!(result.tokens.is_none());
}
