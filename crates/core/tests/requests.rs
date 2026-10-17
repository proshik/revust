use revust_core::{prompts::runner::Runner, request};
use serde_json::Value;
use std::path::Path;

#[test]
fn structured_requests_transport_authored_schema_and_literal_user_input() {
    for executor in ["claude", "codex"] {
        for schema in [
            request::FINDER_SCHEMA,
            request::SYNTHESIS_SCHEMA,
            request::VERIFY_SCHEMA,
        ] {
            let runner = Runner::parse(&format!("{executor}/fixture-model:high")).unwrap();
            let body = "Review input: {{literal}} / non-ASCII λ";
            let req = request::compose(
                &runner,
                body,
                schema,
                Path::new("/fixture/work"),
                "read-only",
            )
            .unwrap();
            assert_eq!(req.cwd, Path::new("/fixture/work"));
            assert_eq!(req.program, executor);
            assert!(req.stdin.starts_with(body));
            if executor == "claude" {
                let i = req.argv.iter().position(|a| a == "--json-schema").unwrap();
                let delivered: Value = serde_json::from_str(&req.argv[i + 1]).unwrap();
                assert_eq!(delivered, serde_json::from_str::<Value>(schema).unwrap());
                assert!(!req.stdin.contains(schema));
            } else {
                let (_, delivered) = req.stdin.rsplit_once("Output schema (JSON):\n").unwrap();
                let delivered: Value = serde_json::from_str(delivered).unwrap();
                assert_eq!(delivered, serde_json::from_str::<Value>(schema).unwrap());
                assert!(!req.argv.iter().any(|a| a == "--json-schema"));
            }
            assert!(req.argv.iter().any(|a| a == "fixture-model"));
            assert!(
                req.argv
                    .iter()
                    .any(|a| a == "high" || a == "model_reasoning_effort=high")
            );
        }
    }
}

#[test]
fn omitted_model_effort_and_schema_do_not_invent_provider_defaults() {
    for executor in ["claude", "codex"] {
        let runner = Runner::parse(executor).unwrap();
        let request =
            request::compose(&runner, "literal {{message}}", "", Path::new("/work"), "").unwrap();
        assert_eq!(request.stdin, "literal {{message}}");
        assert!(!request.argv.iter().any(|s| ["--model", "-m", "--effort", "-c", "--json-schema"].contains(&s.as_str())));
        if executor == "codex" {
            assert_eq!(
                request.argv,
                ["exec", "--skip-git-repo-check", "--sandbox", "read-only"]
            );
        }
    }
    let runner = Runner::parse("codex").unwrap();
    assert!(request::compose(&runner, "", "", Path::new("/work"), "invalid").is_err());
}

#[test]
fn opencode_composes_explicit_model_and_isolated_json_for_each_stage() {
    let runner = Runner {
        executor: "opencode".into(),
        model: "fixture/model#variant".into(),
        effort: String::new(),
    };
    for schema in [
        request::FINDER_SCHEMA,
        request::SYNTHESIS_SCHEMA,
        request::VERIFY_SCHEMA,
    ] {
        let req = request::compose(
            &runner,
            "Review unchanged context: тест",
            schema,
            Path::new("/synthetic/project"),
            "read-only",
        )
        .unwrap();
        assert_eq!(
            req.argv,
            [
                "run",
                "--standalone",
                "--format",
                "json",
                "--title",
                "revust review",
                "--agent",
                "revust-review",
                "--model",
                "fixture/model#variant"
            ]
        );
        assert!(req.stdin.starts_with("Review unchanged context: тест\n"));
        assert!(req.stdin.contains(schema));
        assert_eq!(req.cwd, Path::new("/synthetic/project"));
    }
    for (model, effort, sandbox) in [
        ("", "", ""),
        ("model", "", ""),
        ("p/m#", "", ""),
        ("p/#v", "", ""),
        ("p/m\0", "", ""),
        ("p/m", "high", ""),
        ("p/m", "", "workspace-write"),
    ] {
        assert!(
            request::compose(
                &Runner {
                    executor: "opencode".into(),
                    model: model.into(),
                    effort: effort.into()
                },
                "body",
                "",
                Path::new("."),
                sandbox
            )
            .is_err()
        );
    }
}
