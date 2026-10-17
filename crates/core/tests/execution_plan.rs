use revust_core::{
    config::runtime::Invocation,
    plan::PromptPlan,
    runtime::{
        Attempt, Stage,
        data::Finding,
        pipeline::{Call, Source},
    },
};
use std::{fs, path::PathBuf};
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn frozen_templates_render_actual_results_once_and_snapshot_calibration() {
    let root = std::env::temp_dir().join(format!(
        "revust-execution-plan-{}-{{{{FINDINGS}}}}",
        std::process::id()
    ));
    fs::create_dir(&root).unwrap();
    let temp = Temp(root);
    let config = temp.0.join(".revust");
    fs::create_dir_all(config.join("prompts/profiles")).unwrap();
    fs::write(config.join("prompts/profiles/synthetic.md"),"---\nmodel: codex/synthetic\nagents:\n  - name: observer\n    lenses: [bugs]\n---\nFIND {{SCOPE}}\n").unwrap();
    fs::write(
        config.join("prompts/synthesis.md"),
        " \n\tSYNTH {{SCOPE}}\n{{FINDINGS}}\n{{SOURCES}}\n \t",
    )
    .unwrap();
    fs::write(
        config.join("prompts/verify.md"),
        " \n\tVERIFY {{SCOPE}}\n{{FINDINGS}}\n \t",
    )
    .unwrap();
    fs::write(config.join("profile.md"), "original calibration").unwrap();
    let tasks = temp.0.join("tasks");
    let input = tasks.join("test/01/input");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("scope.md"), "synthetic scope").unwrap();
    let invocation = Invocation {
        config_dir: Some(temp.0.join("user")),
        workdir: Some(temp.0.clone()),
        overrides: vec![
            ("profile".into(), "synthetic".into()),
            ("tasks-dir".into(), tasks.to_str().unwrap().into()),
        ],
        task: "test".into(),
        run: "01".into(),
    };
    let plan = PromptPlan::resolve(&temp.0, None, &invocation, &[]).unwrap();
    fs::write(
        config.join("prompts/synthesis.md"),
        "CHANGED AFTER RESOLUTION",
    )
    .unwrap();
    fs::write(config.join("profile.md"), "changed calibration").unwrap();
    let prepared = plan.into_execution().unwrap();
    assert_eq!(prepared.snapshots()[0].content, b"original calibration");
    assert!(!prepared.snapshots()[0].destination.exists());
    let agent = &prepared.definition().agents[0];
    let finder = prepared
        .request(&Call {
            display_name: "observer",
            attempt: &Attempt {
                stage: Stage::Find,
                unit: agent.name.clone(),
                number: 0,
            },
            runner: &agent.runner,
            agent: Some(agent),
            findings: &[],
            sources: &[],
            activity: &|| {},
        })
        .unwrap();
    assert!(finder.stdin.starts_with("FIND "));
    assert!(finder.stdin.contains("{{FINDINGS}}"));
    let finding = Finding {
        id: "observer-1".into(),
        body: "Literal {{SCOPE}} survives.".into(),
        ..Finding::default()
    };
    let sources = [Source {
        agent: agent.clone(),
        findings: Some(vec![finding.clone()]),
    }];
    for stage in [Stage::Synthesis, Stage::Verify] {
        let runner = if stage == Stage::Synthesis {
            &prepared.definition().synthesis
        } else {
            &prepared.definition().verify
        };
        let req = prepared
            .request(&Call {
                display_name: "stage",
                attempt: &Attempt {
                    stage,
                    unit: "group".into(),
                    number: 0,
                },
                runner,
                agent: None,
                findings: std::slice::from_ref(&finding),
                sources: &sources,
                activity: &|| {},
            })
            .unwrap();
        let (body, _) = req
            .stdin
            .split_once("\n\nDeliver the final result")
            .unwrap();
        assert!(body.starts_with(if stage == Stage::Synthesis {
            "SYNTH "
        } else {
            "VERIFY "
        }));
        assert!(body.ends_with(if stage == Stage::Synthesis { "}" } else { "]" }));
        assert!(!req.stdin.contains("CHANGED AFTER RESOLUTION"));
        assert!(req.stdin.contains("Literal {{SCOPE}} survives."));
        assert!(req.stdin.contains(input.join("scope.md").to_str().unwrap()));
        if stage == Stage::Synthesis {
            let inventory: serde_json::Value =
                serde_json::from_str(&body[body.rfind("\n{").unwrap() + 1..]).unwrap();
            assert_eq!(inventory["reported"], 1);
            assert_eq!(
                inventory["reviewers"][0]["finding_ids"],
                serde_json::json!(["observer-1"])
            );
        }
    }
}

#[test]
fn archive_provenance_retains_raw_winning_files_after_sources_change() {
    let temp =
        Temp(std::env::temp_dir().join(format!("revust-plan-provenance-{}", std::process::id())));
    let config = temp.0.join(".revust");
    let user = temp.0.join("user");
    for root in [&config, &user] {
        fs::create_dir_all(root.join("lenses")).unwrap();
    }
    fs::create_dir_all(config.join("extension-profiles")).unwrap();
    fs::create_dir_all(temp.0.join("tasks/test/01/input")).unwrap();
    fs::write(temp.0.join("tasks/test/01/input/scope.md"), "scope").unwrap();
    fs::write(user.join("lenses/bugs.md"), "user loses\n").unwrap();
    let raw_lens = "---\ndescription: Frozen lens\n---\n  authored whitespace\n\n";
    fs::write(config.join("lenses/bugs.md"), raw_lens).unwrap();
    let raw_profile = "---\nrunner:\n  executor: opencode\n  model: fixture/model\nagents:\n  - name: observer\n    lenses: [bugs]\n---\n  FIND {{SCOPE}}\n\n";
    let profile_path = config.join("extension-profiles/custom.md");
    fs::write(&profile_path, raw_profile).unwrap();
    fs::write(
        config.join("extensions.json"),
        r#"{"version":1,"profiles":{"custom":{"file":"extension-profiles/custom.md"}}}"#,
    )
    .unwrap();
    let invocation = Invocation {
        config_dir: Some(user),
        workdir: Some(temp.0.clone()),
        task: "test".into(),
        run: "01".into(),
        overrides: vec![
            ("profile".into(), "custom".into()),
            (
                "tasks-dir".into(),
                temp.0.join("tasks").to_str().unwrap().into(),
            ),
        ],
    };
    let plan = PromptPlan::resolve(&temp.0, None, &invocation, &[]).unwrap();
    let inspection = serde_json::to_value(&plan).unwrap();
    assert!(inspection.get("prompt_snapshots").is_none());
    fs::remove_dir_all(&config).unwrap();
    let execution = plan.into_execution().unwrap();
    assert_eq!(execution.profile(), "custom");
    let snapshots = execution.prompt_snapshots();
    assert_eq!(snapshots.len(), 23);
    assert!(
        snapshots
            .windows(2)
            .all(|pair| pair[0].origin.path < pair[1].origin.path)
    );
    let lens = snapshots
        .iter()
        .find(|s| s.origin.path == "lenses/bugs.md")
        .unwrap();
    assert_eq!(lens.origin.layer, "project");
    assert_eq!(
        lens.origin.source.as_ref(),
        Some(&config.join("lenses/bugs.md"))
    );
    assert_eq!(lens.content, raw_lens);
    let embedded = snapshots
        .iter()
        .find(|s| s.origin.path == "prompts/verify.md")
        .unwrap();
    assert_eq!(embedded.origin.layer, "embedded");
    assert!(embedded.origin.source.is_none());
    assert!(!embedded.content.is_empty());
    let extension = execution.extension_profile_snapshot().unwrap();
    assert_eq!(extension.source, profile_path);
    assert_eq!(extension.content, raw_profile);
    let mut report = revust_core::report::Report {
        scope: execution.scope().clone(),
        ..Default::default()
    };
    report.stats.finished_at = "2026-10-10T10:00:00+00:00".into();
    report.stats.tokens = 12;
    report.sources.agents.push(revust_core::report::SourceStat {
        name: "observer".into(),
        actual_model: "observed".into(),
        tokens: 12,
        raised: 2,
        degraded: true,
        ..Default::default()
    });
    report.sources.degraded.push("observer".into());
    let manifest: serde_json::Value =
        serde_json::from_str(&execution.manifest(&report).unwrap()).unwrap();
    assert_eq!(manifest["profile"], "custom");
    assert_eq!(manifest["roster"][0]["executor"], "opencode");
    assert_eq!(manifest["agents"][0]["requested_model"], "fixture/model");
    assert_eq!(manifest["agents"][0]["actual_model"], "observed");
    assert_eq!(manifest["agents"][0]["tokens"], 12);
    assert_eq!(manifest["agents"][0]["raised"], 2);
    assert_eq!(manifest["agents"][0]["degraded"], true);
    assert_eq!(manifest["finished_at"], "2026-10-10T10:00:00Z");
    let origin = manifest["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["path"] == "lenses/bugs.md")
        .unwrap();
    assert_eq!(
        origin["hash"],
        "557a99f08c2a4249c4250d68be76aa3666b550f474bdbc42e1f2bb79e219ccc0"
    );
    for origin in manifest["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["layer"] == "embedded")
    {
        use sha2::{Digest, Sha256};
        let snapshot = execution
            .prompt_snapshots()
            .iter()
            .find(|p| p.origin.path == origin["path"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            origin["hash"],
            Sha256::digest(snapshot.content.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        assert!(origin.get("source").is_none());
    }
    report.sources.agents.clear();
    report.sources.degraded.clear();
    let absent: serde_json::Value =
        serde_json::from_str(&execution.manifest(&report).unwrap()).unwrap();
    assert_eq!(absent["agents"][0]["name"], "observer");
    assert_eq!(absent["agents"][0]["requested_model"], "fixture/model");
    assert_eq!(absent["agents"][0]["actual_model"], "");
    assert_eq!(absent["agents"][0]["tokens"], 0);
    assert!(absent["degraded"].is_null());
    report.stats.finished_at = "invalid".into();
    assert!(execution.manifest(&report).is_err());
    report.stats.finished_at = "2026-10-10T10:00:00Z".into();
    report.scope.run = "other".into();
    assert!(execution.manifest(&report).is_err());
}
