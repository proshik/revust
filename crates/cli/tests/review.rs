#![cfg(unix)]
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};
struct Tree(PathBuf);
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
impl Tree {
    fn new(id: usize) -> Self {
        let root = std::env::temp_dir().join(format!("revust-review-{}-{id}", std::process::id()));
        fs::create_dir_all(root.join(".revust/prompts/profiles")).unwrap();
        fs::create_dir_all(root.join("tasks/test/01/input")).unwrap();
        fs::write(root.join("tasks/test/01/input/scope.md"), "synthetic scope").unwrap();
        fs::write(
            root.join(".revust/prompts/profiles/test.md"),
            "---\nmodel: claude/sonnet\nagents:\n  - name: a\n    lenses: [bugs]\n---\nReview {{SCOPE}}\n",
        )
        .unwrap();
        let provider = root.join("provider");
        let payload = json!({"type":"result","structured_output":{"findings":[{"file":"a.rs","line":1,"title":"Synthetic","severity":"major","confidence":75}]}});
        fs::write(
            &provider,
            format!(
                "#!/bin/sh\n/bin/cat >/dev/null\nprintf '%s\\n' '{}'\n",
                payload
            ),
        )
        .unwrap();
        fs::set_permissions(&provider, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(root.join(".revust/extensions.json"),json!({"version":1,"policy":{"executors":{"claude":{"executable":provider}},"runtime":{"stagger_delay":"0s"}}}).to_string()).unwrap();
        Self(root)
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_revust"));
        c.current_dir(&self.0)
            .env_clear()
            .env("HOME", &self.0)
            .args([
                "--task=test",
                "--run=01",
                "--tasks-dir=tasks",
                "--profile=test",
                "--no-tui",
                "--no-synthesis",
                "--no-verify",
            ]);
        c
    }
}
#[test]
fn headless_execution_archives_filtered_report_and_refuses_reexecution() {
    for (id, threshold, exit, count) in [
        (0, "0", 1, 1),
        (1, "80", 0, 0),
        (3, "-1", 1, 1),
        (4, "101", 0, 0),
    ] {
        let tree = Tree::new(id);
        let out = tree
            .command()
            .arg(format!("--min-confidence={threshold}"))
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(exit),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let progress = String::from_utf8(out.stderr.clone()).unwrap();
        assert!(progress.contains("find"), "{progress}");
        assert!(progress.contains("complete"), "{progress}");
        assert!(
            progress.contains("sources 1/1; degraded: none"),
            "{progress}"
        );
        assert!(
            progress.contains(if count == 0 {
                "no findings"
            } else {
                "1 findings: 1 major"
            }),
            "{progress}"
        );
        assert!(!out.stdout.contains(&0x1b));
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["findings"].as_array().unwrap().len(), count);
        let round = tree.0.join("tasks/test/01");
        assert_eq!(fs::read(round.join("findings.json")).unwrap(), out.stdout);
        assert!(!fs::read(round.join("manifest.json")).unwrap().is_empty());
        let policy: Value =
            serde_json::from_slice(&fs::read(round.join("revust/policy.json")).unwrap()).unwrap();
        assert_eq!(
            policy["policy"]["values"]["verification"]["min_confidence"],
            threshold.parse::<i64>().unwrap()
        );
        assert_eq!(
            policy["policy"]["origins"]["/verification/min_confidence"]["kind"],
            "legacy"
        );
        let stages: Value =
            serde_json::from_slice(&fs::read(round.join("stages/1-found.json")).unwrap()).unwrap();
        assert_eq!(stages["findings"].as_array().unwrap().len(), 1);
        assert!(!round.join("stages/2-synthesized.json").exists());
        let again = tree.command().output().unwrap();
        assert_eq!(again.status.code(), Some(2));
        assert!(again.stdout.is_empty());
    }
}

#[test]
fn interrupt_cancels_provider_and_preserves_incomplete_evidence() {
    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    for signal in [Signal::SIGTERM, Signal::SIGHUP] {
        let tree = Tree::new(2);
        let pid_path = tree.0.join("provider.pid");
        fs::write(
        tree.0.join("provider"),
        format!(
            "#!/bin/sh\npid_file='{}'\nprintf '%s' $$ > \"$pid_file.tmp\"\n/bin/mv \"$pid_file.tmp\" \"$pid_file\"\nexec /bin/sleep 60\n",
            pid_path.display()
        ),
    )
    .unwrap();
        let mut child = tree
            .command()
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        while !pid_path.is_file() && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !pid_path.is_file() {
            let _ = kill(Pid::from_raw(child.id() as i32), Signal::SIGKILL);
            let _ = child.wait();
            panic!("provider did not start");
        }
        let provider = Pid::from_raw(fs::read_to_string(pid_path).unwrap().parse().unwrap());
        kill(Pid::from_raw(child.id() as i32), signal).unwrap();
        let out = child.wait_with_output().unwrap();
        let survived = kill(provider, None).is_ok();
        if survived {
            // Keep a failing regression from leaving its controlled process alive.
            let _ = kill(provider, Signal::SIGKILL);
        }
        assert!(!String::from_utf8_lossy(&out.stderr).contains("── complete ──"));
        assert_eq!(
            out.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!survived, "provider survived cancellation");
        let round = tree.0.join("tasks/test/01");
        let manifest: Value =
            serde_json::from_slice(&fs::read(round.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["outcome"]["state"], json!({"stopped":"cancelled"}));
        let viewed = Command::new(env!("CARGO_BIN_EXE_revust"))
            .env_clear()
            .args(["view", "--no-tui", "--archive"])
            .arg(&round)
            .output()
            .unwrap();
        assert_eq!(viewed.status.code(), Some(2));
        let report: Value = serde_json::from_slice(&viewed.stdout).unwrap();
        assert_eq!(report["outcome"]["state"], json!({"stopped":"cancelled"}));
        assert!(
            fs::read_to_string(round.join("revust/events.jsonl"))
                .unwrap()
                .contains("cancelled")
        );
    }
}

#[test]
fn anthropic_key_is_stripped_unless_explicitly_preserved() {
    for (index, executor) in ["claude", "codex", "opencode"].iter().enumerate() {
        for preserve in [false, true] {
            let tree = Tree::new(10 + index * 2 + usize::from(preserve));
            let provider = tree.0.join("provider");
            fs::create_dir(tree.0.join(".revust/extension-profiles")).unwrap();
            fs::write(tree.0.join(".revust/extension-profiles/keys.md"),format!("---\nrunner:\n  executor: {executor}\n  model: fixture/model\nagents:\n  - name: a\n    lenses: [bugs]\n---\nReview {{{{SCOPE}}}}\n")).unwrap();
            fs::write(tree.0.join(".revust/extensions.json"),json!({"version":1,"profiles":{"keys":{"file":"extension-profiles/keys.md"}},"policy":{"executors":{*executor:{"executable":provider}},"runtime":{"stagger_delay":"0s"}}}).to_string()).unwrap();
            let payload = json!({"findings":[{"file":"a.rs","line":1,"title":"Synthetic","severity":"major","confidence":75}]});
            let answer = match *executor {
                "claude" => json!({"type":"result","structured_output":payload}).to_string(),
                "codex" => payload.to_string(),
                _ => format!(
                    "{}\n{}",
                    json!({"type":"text","part":{"id":"text","messageID":"message","text":payload.to_string()}}),
                    json!({"type":"step_finish","part":{"messageID":"message","reason":"stop"}})
                ),
            };
            let script = format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'opencode v2.0.21\\n'; exit 0; fi\nif [ \"${{ANTHROPIC_API_KEY-}}\" = synthetic-only ]; then printf present; else printf absent; fi > '{}'\n/bin/cat >/dev/null\nprintf '%s\\n' '{}'\n",
                tree.0.join("key-status").display(),
                answer
            );
            fs::write(&provider, script).unwrap();
            let mut command = tree.command();
            command
                .arg("--profile=keys")
                .env("ANTHROPIC_API_KEY", "synthetic-only");
            if preserve {
                command.arg("--preserve-anthropic-api-key");
            }
            let out = command.output().unwrap();
            assert_eq!(
                out.status.code(),
                Some(1),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                fs::read_to_string(tree.0.join("key-status")).unwrap(),
                if preserve { "present" } else { "absent" }
            );
            assert!(!String::from_utf8_lossy(&out.stdout).contains("synthetic-only"));
        }
    }
}

#[test]
fn final_archive_write_failure_does_not_print_completion() {
    let tree = Tree::new(9);
    fs::write(tree.0.join("tasks/test/01/report.md"), "keep existing").unwrap();
    let out = tree.command().output().unwrap();
    let progress = String::from_utf8(out.stderr).unwrap();
    assert_eq!(out.status.code(), Some(2), "{progress}");
    let events = fs::read_to_string(tree.0.join("tasks/test/01/events.jsonl")).unwrap();
    let events: Vec<serde_json::Value> = events
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "agent_done" && event["agent"] == "a"),
        "{progress}"
    );
    assert!(
        !events
            .iter()
            .any(|event| event["kind"] == "stage" && event["stage"] == "complete"),
        "{progress}"
    );
    assert!(
        fs::read(tree.0.join("tasks/test/01/manifest.json"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read_to_string(tree.0.join("tasks/test/01/report.md")).unwrap(),
        "keep existing"
    );
}

#[test]
fn failed_providers_save_explicit_outcomes_and_reopen_without_dispatch() {
    for (index, executor) in ["claude", "codex", "opencode"].into_iter().enumerate() {
        let tree = Tree::new(30 + index);
        let profile = format!(
            "---\nrunner:\n  executor: {executor}\n  model: fixture/model\nagents:\n  - name: a\n    lenses: [bugs]\n---\nSynthetic review {{{{SCOPE}}}}\n"
        );
        fs::create_dir(tree.0.join(".revust/extension-profiles")).unwrap();
        fs::write(
            tree.0.join(".revust/extension-profiles/failure.md"),
            profile,
        )
        .unwrap();
        let config = tree.0.join(".revust/extensions.json");
        fs::write(config, json!({"version":1,"profiles":{"failure":{"file":"extension-profiles/failure.md"}},"policy":{"executors":{executor:{"executable":tree.0.join("provider")}},"runtime":{"stagger_delay":"0s"},"retries":{"find":{"count":0}}}}).to_string()).unwrap();
        fs::write(tree.0.join("provider"), "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'opencode v2.0.21\\n'; exit 0; fi\n/bin/cat >/dev/null\nprintf synthetic-failure >&2\nexit 7\n").unwrap();
        let out = tree.command().arg("--profile=failure").output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("review incomplete"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let round = tree.0.join("tasks/test/01");
        let manifest: Value =
            serde_json::from_slice(&fs::read(round.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["outcome"]["state"], "failed");
        // Any accidental provider dispatch during viewing now fails immediately.
        fs::remove_file(tree.0.join("provider")).unwrap();
        let before = fs::read(round.join("manifest.json")).unwrap();
        let viewed = Command::new(env!("CARGO_BIN_EXE_revust"))
            .env_clear()
            .args(["view", "--no-tui", "--archive"])
            .arg(&round)
            .output()
            .unwrap();
        assert_eq!(viewed.status.code(), Some(2));
        let report: Value = serde_json::from_slice(&viewed.stdout).unwrap();
        assert_eq!(report["outcome"]["state"], "failed");
        assert!(String::from_utf8_lossy(&viewed.stderr).contains("exited unsuccessfully"));
        assert_eq!(fs::read(round.join("manifest.json")).unwrap(), before);
    }
}
