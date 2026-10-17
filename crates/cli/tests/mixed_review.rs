#![cfg(unix)]
use revust_core::runtime::{journal, pipeline::State};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

struct Tree(PathBuf);
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn wire(executor: &str, payload: &Value) -> String {
    match executor {
        "claude" => json!({"type":"result","structured_output":payload,"modelUsage":{"observed":{"outputTokens":3}},"usage":{"input_tokens":2,"output_tokens":3}}).to_string(),
        "codex" => payload.to_string(),
        _ => format!("{}\n{}",
            json!({"type":"text","part":{"id":"text","messageID":"message","text":payload.to_string()}}),
            json!({"type":"step_finish","part":{"messageID":"message","reason":"stop","tokens":{"input":2,"output":3,"cache":{"read":0,"write":0}}}})),
    }
}

#[test]
fn mixed_cli_cycles_use_each_executor_in_each_stage_and_reopen_without_profiles() {
    let executors = ["claude", "codex", "opencode"];
    for rotation in 0..3 {
        let tree = Tree(std::env::temp_dir().join(format!(
            "revust-mixed-cli-{}-{rotation}",
            std::process::id()
        )));
        let config = tree.0.join(".revust");
        let round = tree.0.join("tasks/test/01");
        fs::create_dir_all(config.join("extension-profiles")).unwrap();
        fs::create_dir(config.join("prompts")).unwrap();
        fs::create_dir_all(round.join("input")).unwrap();
        fs::write(round.join("input/scope.md"), "synthetic scope").unwrap();
        fs::write(
            config.join("prompts/synthesis.md"),
            "SYNTHESIS\n{{FINDINGS}}\n{{SOURCES}}\n",
        )
        .unwrap();
        fs::write(config.join("prompts/verify.md"), "VERIFY\n{{FINDINGS}}\n").unwrap();
        let runner = |executor| json!({"executor":executor,"model":"fixture/model"});
        let profile = json!({"runner":runner("claude"),"agents":executors.iter().enumerate().map(|(i,e)|json!({"name":format!("a{i}"),"lenses":["bugs"],"runner":runner(e)})).collect::<Vec<_>>(),
            "stages":{"synthesis":runner(executors[rotation]),"verify":runner(executors[(rotation+1)%3])}});
        fs::write(
            config.join("extension-profiles/mixed.md"),
            format!("---\n{profile}\n---\nFIND\n{{{{SCOPE}}}}\n"),
        )
        .unwrap();
        let finding = json!({"file":"src/a.rs","line":1,"title":"Synthetic","confidence":90,"severity":"major"});
        let mut merged = finding.clone();
        merged["merged_ids"] = json!(["a0-1", "a1-1", "a2-1"]);
        let find = json!({"findings":[finding]});
        let synthesis = json!({"findings":[merged],"open_questions":[],"pre_existing":[]});
        let verify = json!({"verdicts":[{"id":"a0-1","verdict":"refined","title":"Refined","severity":"minor","confidence":88}]});
        let mut overrides = json!({});
        for executor in executors {
            let path = tree.0.join(executor);
            let script = format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'opencode v2.0.21\\n'; exit 0; fi\ninput=$(/bin/cat)\ncase \"$input\" in\nFIND*) printf '%s\\n' '{}' ;;\nSYNTHESIS*) test -f '{}/stages/1-found.json' || exit 91; printf '%s\\n' '{}' ;;\nVERIFY*) test -f '{}/stages/2-synthesized.json' || exit 92; printf '%s\\n' '{}' ;;\n*) exit 90 ;;\nesac\n{}\n",
                wire(executor, &find),
                round.display(),
                wire(executor, &synthesis),
                round.display(),
                wire(executor, &verify),
                if executor == "codex" {
                    "printf 'model: observed\\ntokens used\\n5\\n' >&2"
                } else {
                    ""
                }
            );
            let script = script.replace(
                "input=$(/bin/cat)",
                &format!(
                    "printf '%s\\n' '{executor}' >> '{}'\ninput=$(/bin/cat)",
                    tree.0.join("provider-calls").display()
                ),
            );
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            overrides[executor] = json!({"executable":path});
        }
        let markdown = rotation == 1;
        fs::write(config.join("extensions.json"),json!({"version":1,"profiles":{"mixed":{"file":"extension-profiles/mixed.md"}},"policy":{"executors":overrides,"runtime":{"max_parallel":1,"stagger_delay":"0s"},"budget":{"max_calls":5},"verification":{"min_confidence":85},"output":{"tui":false,"format":if markdown {"markdown"} else {"json"}}}}).to_string()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_revust"));
        command
            .current_dir(&tree.0)
            .env_clear()
            .env("HOME", &tree.0)
            .args([
                "--task=test",
                "--run=01",
                "--tasks-dir=tasks",
                "--profile=mixed",
            ]);
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "rotation {rotation}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value =
            serde_json::from_slice(&fs::read(round.join("findings.json")).unwrap()).unwrap();
        assert_eq!(report["findings"].as_array().unwrap().len(), 1);
        assert_eq!(report["findings"][0]["title"], "Refined");
        assert_eq!(report["findings"][0]["sources"], json!(["a0", "a1", "a2"]));
        assert_eq!(report["stats"]["tokens"], 25);
        assert_eq!(
            report["stats"]["stages"][1]["executor"],
            executors[rotation]
        );
        assert_eq!(
            report["stats"]["stages"][2]["executor"],
            executors[(rotation + 1) % 3]
        );
        assert_eq!(report["sources"]["agents"][2]["actual_model"], "");
        assert_eq!(
            fs::read(round.join(if markdown {
                "report.md"
            } else {
                "findings.json"
            }))
            .unwrap(),
            output.stdout
        );
        let journal = journal::inspect(&round).unwrap();
        assert_eq!(journal.attempts.len(), 5);
        assert!(
            journal
                .attempts
                .iter()
                .all(|a| matches!(&a.result, Some(Ok(_))))
        );
        assert_eq!(journal.report.unwrap().state, State::Complete);
        assert!(!fs::read(round.join("manifest.json")).unwrap().is_empty());
        let calls = fs::read_to_string(tree.0.join("provider-calls")).unwrap();
        assert_eq!(calls.lines().count(), 5);
        for executor in executors {
            let expected = 1
                + usize::from(executor == executors[rotation])
                + usize::from(executor == executors[(rotation + 1) % 3]);
            assert_eq!(
                calls.lines().filter(|name| *name == executor).count(),
                expected
            );
        }
        fs::remove_dir_all(&config).unwrap();
        let mut view = Command::new(env!("CARGO_BIN_EXE_revust"));
        view.current_dir(&tree.0)
            .env_clear()
            .args(["view", "--no-tui", "--archive"])
            .arg(&round);
        if markdown {
            view.arg("--markdown");
        }
        let viewed = view.output().unwrap();
        assert!(
            viewed.status.success(),
            "{}",
            String::from_utf8_lossy(&viewed.stderr)
        );
        assert_eq!(viewed.stdout, output.stdout);
        assert_eq!(journal::inspect(&round).unwrap().attempts.len(), 5);
        assert_eq!(
            fs::read_to_string(tree.0.join("provider-calls")).unwrap(),
            calls
        );
    }
}
