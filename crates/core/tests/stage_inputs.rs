use revust_core::{
    prompts::Agent,
    runtime::{data::Finding, inputs, pipeline::Source},
};
use serde_json::Value;
fn agent(name: &str, lenses: &[&str]) -> Agent {
    Agent {
        name: name.into(),
        lenses: lenses.iter().map(|s| (*s).into()).collect(),
        runner: Default::default(),
        color: String::new(),
        color_name: String::new(),
    }
}
#[test]
fn stage_inputs_preserve_identity_evidence_and_literal_provider_text() {
    let first = Finding {
        id: "a-1".into(),
        body: "Do not expand {{SCOPE}}".into(),
        sources: vec!["a".into()],
        ..Finding::default()
    };
    let second = Finding {
        id: "b-1".into(),
        file: "src/next.rs".into(),
        line: 7,
        ..Finding::default()
    };
    let sources = [
        Source {
            agent: agent("a", &["bugs"]),
            findings: Some(vec![first.clone()]),
        },
        Source {
            agent: agent("b", &["tests"]),
            findings: Some(vec![second.clone()]),
        },
        Source {
            agent: agent("offline", &["bugs"]),
            findings: None,
        },
    ];
    let vars = inputs::synthesis(&sources).unwrap();
    let records: Vec<Finding> = serde_json::from_str(&vars["FINDINGS"]).unwrap();
    assert_eq!(records, vec![first.clone(), second.clone()]);
    assert!(vars["SOURCES"].contains("offline"));
    let inventory: Value = serde_json::from_str(&vars["SOURCES"]).unwrap();
    assert_eq!(inventory["degraded"], serde_json::json!(["offline"]));
    let vars = inputs::verification(&[second, first]).unwrap();
    let records: Value = serde_json::from_str(&vars["FINDINGS"]).unwrap();
    assert_eq!(records[0]["id"], "b-1");
    assert_eq!(records[1]["body"], "Do not expand {{SCOPE}}");
}
#[test]
fn empty_success_is_not_degraded_and_provider_text_stays_literal() {
    let sources = vec![
        Source {
            agent: agent("empty", &["one", "two"]),
            findings: Some(vec![]),
        },
        Source {
            agent: agent("failed", &["one"]),
            findings: None,
        },
    ];
    let vars = inputs::synthesis(&sources).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&vars["FINDINGS"]).unwrap(),
        serde_json::json!([])
    );
    let inventory: Value = serde_json::from_str(&vars["SOURCES"]).unwrap();
    assert_eq!(inventory["reported"], 1);
    assert_eq!(inventory["degraded"], serde_json::json!(["failed"]));
    let finding = Finding {
        body: "<>&\u{2028}\u{2029} {{FINDINGS}}".into(),
        ..Finding::default()
    };
    let json = &inputs::verification(std::slice::from_ref(&finding)).unwrap()["FINDINGS"];
    let data: Value = serde_json::from_str(json).unwrap();
    assert_eq!(data[0]["body"], finding.body);
}
