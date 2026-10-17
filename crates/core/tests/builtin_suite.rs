use revust_core::{config::paths::Layers, prompts::PromptSet, request, runtime::data};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

#[test]
fn every_builtin_profile_composes_complete_stage_inputs() {
    let set = PromptSet::load(&Layers {
        user: PathBuf::from("/absent-fixture-user"),
        project: None,
    })
    .unwrap();
    let vars = BTreeMap::from([
        ("SCOPE".into(), "/fixture/scope.md".into()),
        ("GOAL".into(), "/fixture/goal.md".into()),
        ("PROFILE".into(), "/fixture/profile.md".into()),
        ("CONTEXT".into(), "/fixture/context".into()),
        ("WORKDIR".into(), "/fixture/work".into()),
        (
            "FINDINGS".into(),
            r#"[{"id":"a-1","body":"literal {{SCOPE}}"}]"#.into(),
        ),
        (
            "SOURCES".into(),
            "a delivered one candidate; b failed".into(),
        ),
    ]);
    for profile in [
        "claude-only",
        "codex-only",
        "comprehensive",
        "expert",
        "final",
        "focused",
        "grill-me",
        "triage",
    ] {
        let roster = set.roster(profile, &[]).unwrap();
        assert!(!roster.is_empty());
        for agent in roster {
            let body = set.compose_agent(profile, &agent, &vars, "").unwrap();
            for input in ["SCOPE", "GOAL", "PROFILE", "CONTEXT", "WORKDIR"] {
                assert!(body.contains(&vars[input]), "{profile} missing {input}");
                assert!(!body.contains(&format!("{{{{{input}}}}}")));
            }
        }
        for stage in ["synthesis", "verify"] {
            let (_, body) = set.compose_stage(profile, stage, &vars, "").unwrap();
            assert!(body.contains(&vars["FINDINGS"]));
            assert_eq!(body.matches("literal {{SCOPE}}").count(), 1);
            if stage == "synthesis" {
                assert!(body.contains(&vars["SOURCES"]));
            }
        }
    }
}

#[test]
fn unresolved_verification_is_representable_without_a_false_decision() {
    let schema: Value = serde_json::from_str(request::VERIFY_SCHEMA).unwrap();
    let allowed = schema["properties"]["verdicts"]["items"]["properties"]["verdict"]["enum"]
        .as_array()
        .unwrap();
    assert!(allowed.contains(&json!("unverified")));
    let original = data::Finding {
        id: "a-1".into(),
        title: "candidate".into(),
        ..Default::default()
    };
    let result = data::verify(&json!({"verdicts":[{"id":"a-1","verdict":"unverified","reason":"Missing a required deployment fact"}]}), std::slice::from_ref(&original)).unwrap();
    assert_eq!(result[0].verdict, "unverified");
    assert_eq!(result[0].title, original.title);
    assert_eq!(result[0].id, original.id);
}
