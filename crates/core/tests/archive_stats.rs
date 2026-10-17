#![cfg(unix)]
use revust_core::archive::stats::collect;
use std::{fs, path::PathBuf};

#[test]
fn stage_survivors_and_raw_bytes_are_folded_independently_of_final_filter() {
    let root = std::env::temp_dir().join(format!("revust-stats-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    for task in ["a", "b"] {
        let round = root.join(task).join("01");
        fs::create_dir_all(round.join("stages")).unwrap();
        fs::write(
            round.join("manifest.json"),
            r#"{"finished_at":"2026-10-10T12:00:00Z"}"#,
        )
        .unwrap();
        fs::write(round.join("stages/1-found.json"), r#"{"sources":{"agents":[{"name":"alpha","lenses":["bugs","tests"],"tokens":13}]},"findings":[{"sources":["alpha"],"lenses":["tests","bugs"]}]}"#).unwrap();
        fs::write(round.join("stages/3-verified.json"), r#"{"pre_existing":[{"sources":["alpha","other"],"lenses":["bugs"],"verdict":"refined"}]}"#).unwrap();
        fs::write(round.join("findings.json"), "{}").unwrap();
        fs::write(round.join("events.jsonl"), b"{\"kind\":\"agent_retried\",\"agent\":\"alpha\"}\n{\"kind\":\"agent_retried\",\"agent\":\"verify\"}\n{\"kind\":\"agent_retried\"").unwrap();
        // Each rounds to zero MB, but together these sidecars round to 0.1 MB.
        fs::write(round.join("sidecar"), vec![b'x'; 30000]).unwrap();
    }
    let corpus = collect(&root, "").unwrap();
    assert_eq!(corpus.totals.rounds, 2);
    assert_eq!(corpus.totals.agents[0].raised, 2);
    assert_eq!(corpus.totals.agents[0].survived, 2);
    assert_eq!(corpus.totals.agents[0].corroborated, 2);
    assert_eq!(corpus.totals.agents[0].tokens, 26);
    assert_eq!(corpus.totals.agents[0].retries, 2);
    assert_eq!(corpus.totals.lenses[0].ambiguous, 2);
    assert_eq!(corpus.totals.lenses[0].verdicts["refined"], 2);
    assert_eq!(corpus.totals.stages[0].reclassified, 2);
    assert_eq!(corpus.totals.stages[0].refined, 2);
    assert_eq!(corpus.totals.stages[1].out, 0);
    assert_eq!(corpus.tasks[0].size_mb, 0.0);
    assert_eq!(corpus.tasks[1].size_mb, 0.0);
    assert_eq!(corpus.totals.size_mb, 0.1);
    assert!(
        corpus
            .totals
            .skipped
            .iter()
            .any(|message| message.contains("retry counts may be partial"))
    );
    assert!(
        corpus
            .totals
            .skipped
            .iter()
            .any(|message| message.contains("recorded usage"))
    );
    assert!(matches!(collect(&root, "missing"), Err(error) if error.contains("not found")));
}

#[test]
fn active_incomplete_and_escaping_rounds_are_qualified_without_exposing_bodies() {
    use revust_core::archive::{Archive, scaffold};
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join(format!("revust-stats-status-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    scaffold::create(&root.join("tasks"), "sample", "active").unwrap();
    scaffold::create(&root.join("tasks"), "sample", "prepared").unwrap();
    let active = Archive::claim(&root.join("tasks"), "sample", "active").unwrap();
    let outside = root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(
        outside.join("1-found.json"),
        r#"{"findings":[{"body":"PRIVATE BODY"}]}"#,
    )
    .unwrap();
    let escaped = scaffold::create(&root.join("tasks"), "sample", "escaped").unwrap();
    fs::write(
        escaped.round_dir.join("manifest.json"),
        r#"{"finished_at":"2026-10-10T10:00:00Z"}"#,
    )
    .unwrap();
    symlink(&outside, escaped.round_dir.join("stages")).unwrap();
    let stats = collect(&root.join("tasks"), "sample").unwrap();
    assert_eq!(stats.totals.rounds, 0);
    let text = serde_json::to_string(&stats).unwrap();
    assert!(text.contains("active: held"));
    assert!(text.contains("incomplete"));
    assert!(text.contains("escaped"));
    assert!(!text.contains("PRIVATE BODY"));
    drop(active);
    assert_eq!(
        fs::read(outside.join("1-found.json")).unwrap(),
        br#"{"findings":[{"body":"PRIVATE BODY"}]}"#
    );
}

#[test]
fn oversized_recorded_usage_is_an_error_instead_of_a_wrapped_total() {
    let root = std::env::temp_dir().join(format!("revust-stats-overflow-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    for id in ["a", "b"] {
        let round = root.join(id).join("01");
        fs::create_dir_all(round.join("stages")).unwrap();
        fs::write(
            round.join("manifest.json"),
            r#"{"finished_at":"2026-10-10T10:00:00Z"}"#,
        )
        .unwrap();
        fs::write(
            round.join("stages/1-found.json"),
            serde_json::json!({"sources":{"agents":[{"name":"observer","tokens":i64::MAX}]}})
                .to_string(),
        )
        .unwrap();
    }
    assert!(
        collect(&root, "")
            .err()
            .unwrap()
            .contains("token total exceeds")
    );
}
