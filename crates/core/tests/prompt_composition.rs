use revust_core::prompts::{runner::Runner, template};
use std::collections::BTreeMap;

#[test]
fn executor_changes_never_inherit_another_providers_model() {
    let base = Runner::parse(" claude/synthetic-base:high ").unwrap();
    let switched = Runner::parse("codex").unwrap().inherit(&base);
    assert_eq!(switched.executor, "codex");
    assert_eq!(switched.model, "");
    assert_eq!(switched.effort, "high");
    let same = Runner::parse("claude:low").unwrap().inherit(&base);
    assert_eq!(same.model, "synthetic-base");
    assert_eq!(same.effort, "low");
    assert_eq!(Runner::parse("").unwrap().inherit(&base), base);
    assert_eq!(
        Runner::parse("codex/provider/model:max").unwrap().model,
        "provider/model"
    );
    for (value, error) in [
        ("claude/", "no model after the slash"),
        ("opencode/model", "unknown executor"),
        ("claude:typo", "unknown effort"),
    ] {
        assert!(Runner::parse(value).unwrap_err().contains(error));
    }
}

#[test]
fn stage_overrides_apply_before_the_profile_fallback() {
    let base = Runner::parse("claude/synthetic-base:high").unwrap();
    let file = Runner::parse("codex/synthetic-stage:max").unwrap();
    let over = Runner::parse("claude").unwrap();
    let resolved = base.stage(&file, Some(&over));
    assert_eq!(resolved.executor, "claude");
    assert_eq!(resolved.model, "synthetic-base");
    assert_eq!(resolved.effort, "max");
    assert_eq!(base.stage(&file, None), file);
}

#[test]
fn frontmatter_handles_crlf_empty_blocks_and_exact_delimiters() {
    let doc = template::split("---\r\ndescription: Synthetic\r\n---\r\nBody\r\n").unwrap();
    assert_eq!(doc.meta, "description: Synthetic\n");
    assert_eq!(doc.body, "Body\n");
    assert_eq!(template::split("---\n---\nbody").unwrap().body, "body");
    assert_eq!(template::split("---\n---").unwrap().meta, "");
    assert_eq!(
        template::split("---\nvalue: |\n  ---extra\n---\nbody")
            .unwrap()
            .body,
        "body"
    );
    assert_eq!(template::split(" ---\nbody").unwrap().body, " ---\nbody");
    assert!(
        template::split("---\nvalue: x\n---extra\nbody")
            .unwrap_err()
            .contains("never closed")
    );
}

#[test]
fn substitution_is_one_pass_and_history_is_not_a_template() {
    let vars = BTreeMap::from([
        ("SCOPE".into(), "/synthetic/scope.md".into()),
        ("FINDINGS".into(), "literal {{message}}".into()),
    ]);
    let body = "{{SCOPE}} / {{FINDINGS}} / {{ ignored syntax }}";
    assert_eq!(
        template::render(body, &vars, "").unwrap(),
        "/synthetic/scope.md / literal {{message}} / {{ ignored syntax }}"
    );
    assert!(
        template::validate("{{MISSPELLED}}")
            .unwrap_err()
            .contains("unknown variable {{MISSPELLED}}")
    );
    assert!(
        template::render("{{GOAL}}", &vars, "")
            .unwrap_err()
            .contains("unresolved variable {{GOAL}}")
    );
    assert!(
        template::validate(
            "{{SCOPE}} {{GOAL}} {{PROFILE}} {{CONTEXT}} {{WORKDIR}} {{FINDINGS}} {{SOURCES}}"
        )
        .is_ok()
    );
    let result = template::render("body", &vars, "\nHistorical {{raw}}\n").unwrap();
    assert!(result.starts_with("body\n\nHistorical {{raw}}\n\n"));
    assert!(result.contains("Saved rounds provide leads, not conclusions."));
    assert_eq!(result.matches("Historical {{raw}}").count(), 1);
    assert_eq!(
        template::render("{{{SCOPE}}}", &vars, "").unwrap(),
        "{/synthetic/scope.md}"
    );
}
