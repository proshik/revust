use revust_core::report::Report;
use serde_json::Value;

#[test]
fn authored_report_preserves_categories_identity_and_observation_uncertainty() {
    let report = Report::decode(r#"{"scope":{"task":"sample","run":"r2","scope_path":"src/<scope>"},"sources":{"expected":2,"reported":1,"degraded":["missing"],"agents":[{"name":"delivered","requested_model":"chosen","raised":1}]},"findings":[{"id":"F1","title":"Active issue","severity":"major","file":"src/lib.rs","line":8,"sources":["delivered"],"body":"Body & <literal>"}],"open_questions":[{"title":"Question"}],"pre_existing":[{"title":"Prior issue"}],"immaterial":[{"title":"Low impact"}]}"#).unwrap();
    let text = report.markdown();
    for expected in [
        "sample",
        "r2",
        "src/<scope>",
        "F1",
        "Major",
        "src/lib.rs:8",
        "delivered",
        "missing",
        "chosen",
        "unknown",
        "Question",
        "Prior issue",
        "Low impact",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(text.contains("zero or unavailable"));
    let json = report.json().unwrap();
    assert!(json.contains("Body & <literal>"));
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["sources"]["agents"][0]["actual_model"], "");
    assert_eq!(Report::decode(&json).unwrap().json().unwrap(), json);
}

#[test]
fn optional_nulls_are_defaults_but_wrong_nonnull_types_fail() {
    let report = Report::decode(r#"{"scope":null,"findings":[null,{"sources":[null,"a"],"lenses":null}],"stats":{"stages":[null]},"sources":{"degraded":[null]}}"#).unwrap();
    assert_eq!(report.findings.len(), 2);
    assert_eq!(report.findings[1].sources, ["", "a"]);
    assert_eq!(report.sources.degraded, [""]);
    assert_eq!(report.stats.stages.len(), 1);
}
#[test]
fn filtering_changes_only_findings_and_exit_code() {
    let report=Report::decode(r#"{"findings":[{"title":"below","confidence":69},{"title":"at","confidence":70}],"open_questions":[{"confidence":1}],"pre_existing":[{}],"immaterial":[{}]}"#).unwrap();
    let filtered = report.above(70);
    assert_eq!(filtered.findings.len(), 1);
    assert_eq!(filtered.findings[0].title, "at");
    assert_eq!(filtered.exit_code(), 1);
    let empty = report.above(101);
    assert_eq!(empty.exit_code(), 0);
    assert_eq!(
        (
            empty.open_questions.len(),
            empty.pre_existing.len(),
            empty.immaterial.len()
        ),
        (1, 1, 1)
    );
    assert_eq!(report.findings.len(), 2);
}

#[test]
fn invalid_report_types_and_dates_are_not_silently_replaced() {
    for input in [
        r#"{"findings":{}}"#,
        r#"{"findings":[{"line":"4"}]}"#,
        r#"{"sources":{"expected":1.5}}"#,
        r#"{"stats":{"started_at":"yesterday"}}"#,
        r#"{"stats":{"started_at":"2026-10-10T23:59:60Z"}}"#,
    ] {
        assert!(Report::decode(input).is_err(), "{input}");
    }
    let mut report = Report::default();
    report.stats.started_at = "invalid".into();
    assert!(report.json().is_err());
}

#[test]
fn legacy_timezone_boundary_can_be_viewed_in_markdown_but_not_encoded_as_json() {
    let report = Report::decode(r#"{"stats":{"started_at":"2026-10-10T10:00:00+24:00"}}"#).unwrap();
    assert!(report.markdown().contains("No active findings"));
    assert!(report.json().is_err());
}

#[test]
fn timestamp_output_keeps_offset_and_instant_using_rfc3339() {
    let report = Report::decode(r#"{"stats":{"started_at":"2026-10-10T12:30:01.123456789+03:00","finished_at":"2026-10-10T09:30:02.1Z"}}"#).unwrap();
    let output: Value = serde_json::from_str(&report.json().unwrap()).unwrap();
    for name in ["started_at", "finished_at"] {
        let written =
            chrono::DateTime::parse_from_rfc3339(output["stats"][name].as_str().unwrap()).unwrap();
        let original = if name == "started_at" {
            &report.stats.started_at
        } else {
            &report.stats.finished_at
        };
        let original = chrono::DateTime::parse_from_rfc3339(original).unwrap();
        assert_eq!(written, original);
        assert_eq!(written.offset(), original.offset());
    }
}
