use revust_core::{
    prompts::Agent,
    runtime::{
        data::{self, Finding},
        inputs,
        pipeline::Source,
    },
};
use serde_json::json;
#[test]
fn duplicate_original_id_cannot_silently_replace_source_evidence() {
    let originals = [
        Finding {
            id: "same".into(),
            sources: vec!["first".into()],
            ..Default::default()
        },
        Finding {
            id: "same".into(),
            sources: vec!["second".into()],
            ..Default::default()
        },
    ];
    assert!(data::synthesize(&json!({"findings":[{"merged_ids":["same"]}]}), &originals).is_err());
}
fn source(name: &str, findings: Option<Vec<Finding>>) -> Source {
    Source {
        agent: Agent {
            name: name.into(),
            lenses: vec!["bugs".into()],
            runner: Default::default(),
            color: String::new(),
            color_name: String::new(),
        },
        findings,
    }
}
#[test]
fn delivery_inventory_distinguishes_failed_from_successful_empty_and_checks_identity() {
    let values =
        inputs::synthesis(&[source("empty", Some(vec![])), source("failed", None)]).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&values["FINDINGS"]).unwrap(),
        json!([])
    );
    let inventory: serde_json::Value = serde_json::from_str(&values["SOURCES"]).unwrap();
    assert_eq!(inventory["reported"], 1);
    assert_eq!(inventory["degraded"], json!(["failed"]));
    assert_eq!(inventory["reviewers"][0]["status"], "delivered");
    assert_eq!(inventory["reviewers"][1]["status"], "failed");
    assert!(inputs::synthesis(&[source("same", Some(vec![])), source("same", None)]).is_err());
}
#[test]
fn consolidation_never_reassigns_or_invents_evidence_across_categories() {
    let originals = [Finding {
        id: "a-1".into(),
        sources: vec!["a".into()],
        lenses: vec!["bugs".into()],
        ..Default::default()
    }];
    for category in ["findings", "open_questions", "pre_existing"] {
        let mut value = json!({"findings":[]});
        value[category] = json!([{"merged_ids":["missing"]}]);
        assert!(data::synthesize(&value, &originals).is_err());
        if category != "findings" {
            value["findings"] = json!([{"merged_ids":["a-1"]}]);
            value[category] = json!([{"merged_ids":["a-1"]}]);
            assert!(data::synthesize(&value, &originals).is_err());
        }
    }
}
#[test]
fn assessment_preserves_owned_fields_and_originals_with_last_record_precedence() {
    let originals = [
        Finding {
            id: "a-1".into(),
            file: "owned.rs".into(),
            sources: vec!["a".into()],
            lenses: vec!["bugs".into()],
            line: 9,
            severity: "major".into(),
            body: "original".into(),
            ..Default::default()
        },
        Finding {
            id: "a-2".into(),
            ..Default::default()
        },
    ];
    let before = originals.clone();
    let result=data::verify(&json!({"verdicts":[{"id":"a-1","verdict":"rejected"},{"id":"foreign","verdict":"confirmed"},{"id":"a-1","verdict":"refined","file":[],"sources":"not-authority","lenses":false,"line":11,"body":"new"}]}),&originals).unwrap();
    assert_eq!(originals, before);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].file, "owned.rs");
    assert_eq!(result[0].sources, ["a"]);
    assert_eq!(result[0].lenses, ["bugs"]);
    assert_eq!(result[0].line, 11);
    assert_eq!(result[0].body, "new");
    assert_eq!(result[0].verdict, "refined");
    assert_eq!(result[1].verdict, "unverified");
}
