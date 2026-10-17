use revust_executors::opencode::{Decoder, Event};
use serde_json::json;

#[test]
fn observed_read_and_denied_write_preserve_final_answer_without_inventing_usage() {
    let fixture = include_bytes!("fixtures/opencode-v2021-tools.jsonl");
    for size in [1, 7, 4096] {
        let mut decoder = Decoder::default();
        let mut events = vec![];
        for chunk in fixture.chunks(size) {
            events.extend(decoder.feed(chunk));
        }
        events.extend(decoder.finish());
        assert_eq!(decoder.result.output, Some(json!({"findings":[]})));
        assert_eq!(decoder.result.tokens, None); // Final step_finish absent in observed CLI output.
        assert_eq!(decoder.result.actual_model, None);
        assert!(events.contains(&Event::Progress("read context.txt".into())));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Info(s) if s.contains("No tool named")))
        );
    }
}

#[test]
fn all_stage_payloads_and_usage_are_decoded_across_byte_boundaries() {
    for output in [
        json!({"findings":[]}),
        json!({"findings":[],"open_questions":[],"pre_existing":[]}),
        json!({"verdicts":[]}),
    ] {
        let lines = [
            json!({"type":"step_start","part":{"messageID":"a","type":"step-start"}}),
            json!({"type":"text","part":{"id":"t","messageID":"a","type":"text","text":output.to_string()}}),
            json!({"type":"step_finish","part":{"messageID":"a","type":"step-finish","reason":"stop","tokens":{"input":10,"output":5,"reasoning":2,"cache":{"read":3,"write":4}}}}),
        ];
        let mut d = Decoder::default();
        for line in lines {
            for byte in (line.to_string() + "\n").as_bytes() {
                d.feed(&[*byte]);
            }
        }
        d.finish();
        assert_eq!(d.result.output, Some(output));
        assert_eq!(d.result.tokens, Some(22)); // Reasoning is reported separately, not added twice.
        assert_eq!(d.result.actual_model, None);
    }
}

#[test]
fn last_message_wins_and_repeated_parts_do_not_duplicate_text_or_usage() {
    let mut d = Decoder::default();
    let line = json!({"type":"text","part":{"id":"old","messageID":"a","text":"{\"findings\":[{\"title\":\"not final\"}]}"}});
    d.feed((line.to_string() + "\n").as_bytes());
    d.feed(
        b"{\"type\":\"step_finish\",\"part\":{\"messageID\":\"a\",\"reason\":\"tool-calls\"}}\n",
    );
    let line = json!({"type":"text","part":{"id":"new","messageID":"b","text":"{\"findings\":[],\"note\":\"тест\"}"}});
    for _ in 0..2 {
        d.feed((line.to_string() + "\n").as_bytes());
    }
    d.finish();
    assert_eq!(d.result.output, Some(json!({"findings":[],"note":"тест"})));
    assert_eq!(d.result.tokens, None);
}

#[test]
fn errors_and_damaged_streams_are_not_empty_success() {
    let mut d = Decoder::default();
    d.feed(include_bytes!("fixtures/opencode-v2021-unavailable.json")); // Multiline JSON is not JSONL.
    d.finish();
    assert!(d.result.malformed_lines > 0);
    assert!(d.result.output.is_none());
    let mut d = Decoder::default();
    let error: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/opencode-v2021-unavailable.json")).unwrap();
    d.feed((error.to_string() + "\n").as_bytes());
    d.feed(b"{\"type\":\"text\"");
    d.finish();
    assert_eq!(d.result.error.unwrap().kind, "provider.no-route");
    assert!(d.result.truncated);
    assert!(d.result.output.is_none());
    let mut d = Decoder::default();
    d.feed(&vec![b'x'; 8 << 20]);
    d.finish();
    assert!(d.result.truncated);
    assert!(d.result.output.is_none());
}

#[test]
fn unfinished_later_tool_step_cannot_reuse_an_earlier_answer() {
    let mut d = Decoder::default();
    for line in [
        json!({"type":"text","part":{"id":"a","messageID":"old","text":"{\"findings\":[]}"}}),
        json!({"type":"tool_use","part":{"messageID":"new","tool":"read","state":{"status":"completed","input":{"path":"src/lib.rs"}}}}),
    ] {
        d.feed((line.to_string() + "\n").as_bytes());
    }
    d.finish();
    assert!(d.result.output.is_none());
}
