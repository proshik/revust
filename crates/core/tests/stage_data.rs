use revust_core::runtime::data::{Finding, findings, synthesize, verify};
use serde_json::json;
fn finding(id: &str, source: &str) -> Finding {
    Finding {
        id: id.into(),
        sources: vec![source.into()],
        lenses: vec!["bugs".into()],
        title: "original".into(),
        confidence: 80,
        severity: "major".into(),
        line: 10,
        ..Finding::default()
    }
}

#[test]
fn finder_attribution_comes_from_the_executing_agent() {
    let result=findings(&json!({"findings":[{"id":"invented","sources":["a","b"],"lenses":["unknown","bugs"]},{"lenses":["unknown"]}]}),"reviewer",&["bugs".into(),"tests".into()]).unwrap();
    assert_eq!(
        result.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        vec!["reviewer-1", "reviewer-2"]
    );
    assert_eq!(result[0].sources, vec!["reviewer"]);
    assert_eq!(result[0].lenses, vec!["bugs"]);
    assert_eq!(result[1].lenses, vec!["bugs", "tests"]);
    assert!(findings(&json!({"wrong":[]}), "r", &[]).is_err());
    assert!(findings(&json!({"findings":[{"line":"ten"}]}), "r", &[]).is_err());
    assert!(
        findings(&json!({"findings":null}), "r", &[])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn synthesis_attributes_ids_across_categories_and_keeps_dropped_evidence() {
    let inputs = vec![
        finding("a-1", "a"),
        finding("b-1", "b"),
        finding("a-2", "a"),
    ];
    let result=synthesize(&json!({"findings":[],"open_questions":[{"merged_ids":["a-1","a-1","b-1"],"sources":["invented"],"lenses":[],"title":"question"}]}),&inputs).unwrap();
    assert_eq!(result.open_questions[0].sources, vec!["a", "b"]);
    assert_eq!(result.open_questions[0].lenses, vec!["bugs"]);
    assert_eq!(result.open_questions[0].id, "a-1");
    assert_eq!(result.dropped, vec![inputs[2].clone()]);
    for value in [
        json!({"findings":[{"merged_ids":[]}]}),
        json!({"findings":[{"merged_ids":["unknown"]}]}),
        json!({"findings":[{"merged_ids":["a-1"]}],"pre_existing":[{"merged_ids":["a-1"]}]}),
    ] {
        assert!(synthesize(&value, &inputs).is_err());
    }
}

#[test]
fn verifier_keeps_unknowns_unverified_and_applies_only_qualified_refinements() {
    let inputs = vec![
        finding("a", "one"),
        finding("b", "one"),
        finding("c", "one"),
    ];
    let result=verify(&json!({"verdicts":[{"id":"a","verdict":"refined","confidence":0,"line":0,"severity":"invented","title":"revised"},{"id":"b","verdict":"invented"},{"id":"c","verdict":"rejected"},{"id":"unknown","verdict":"confirmed"}]}),&inputs).unwrap();
    assert_eq!(result[0].title, "revised");
    assert_eq!(result[0].confidence, 80);
    assert_eq!(result[0].line, 10);
    assert_eq!(result[0].severity, "major");
    assert_eq!(result[0].sources, inputs[0].sources);
    assert_eq!(result[1].verdict, "unverified");
    // Keep the rejected decision as evidence; the report router removes it.
    assert_eq!(result[2].verdict, "rejected");
    assert!(verify(&json!({"wrong":[]}), &inputs).is_err());
    assert!(
        verify(&json!({"verdicts":null}), &inputs)
            .unwrap()
            .iter()
            .all(|f| f.verdict == "unverified")
    );
}
