use revust_core::config::{
    extensions::{Document, EffectivePolicy, Layer, LayerKind},
    runtime::{Invocation, RuntimeConfig},
};
use serde_json::json;
use std::{fs, path::PathBuf, time::SystemTime};
struct Tree(PathBuf);
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn setup() -> Tree {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let unique = format!("{timestamp}-{sequence}");
    let tree = Tree(
        std::env::temp_dir().join(format!("revust-extension-{}-{unique}", std::process::id())),
    );
    fs::create_dir_all(tree.0.join(".revust")).unwrap();
    fs::create_dir_all(tree.0.join("user")).unwrap();
    tree
}
fn layer(tree: &Tree, kind: LayerKind, text: serde_json::Value) -> Layer {
    Layer {
        directory: tree.0.join(if kind == LayerKind::User {
            "user"
        } else {
            ".revust"
        }),
        kind,
        document: Document::parse(&text.to_string()).unwrap(),
    }
}

#[test]
fn later_legacy_globals_override_earlier_stage_policy_even_with_legacy_alias_origins() {
    let tree = setup();
    fs::write(
        tree.0.join(".revust/config"),
        "HardTimeout=12s\nMaxParallel=3\n",
    )
    .unwrap();
    let invocation = Invocation {
        config_dir: Some(tree.0.join("user")),
        ..Invocation::default()
    };
    let runtime = RuntimeConfig::resolve(&tree.0, None, &invocation).unwrap();
    let layers = [layer(
        &tree,
        LayerKind::User,
        json!({"version":1,"policy":{
        "stages":{"verify":{"hard_timeout":"1s","parallelism":2}},
        "budget":{"max_calls":5},"output":{"tui":false}}}),
    )];
    let result = EffectivePolicy::resolve(&runtime, &layers, "comprehensive", &json!({}))
        .unwrap()
        .inspection();
    assert_eq!(result["values"]["stages"]["verify"]["hard_timeout"], "12s");
    assert_eq!(result["values"]["stages"]["verify"]["parallelism"], 3);
    assert_eq!(result["values"]["budget"]["max_calls"], 5);
    assert_eq!(result["values"]["output"]["tui"], false);
    assert_eq!(
        result["origins"]["/stages/verify/hard_timeout"]["layer"],
        "project"
    );
    assert_eq!(
        result["origins"]["/stages/verify/hard_timeout"]["kind"],
        "legacy"
    );
}

#[test]
fn profile_overrides_within_layer_and_cli_win_without_exposing_environment_values() {
    let tree = setup();
    let invocation = Invocation {
        config_dir: Some(tree.0.join("user")),
        overrides: vec![("hard-timeout".into(), "19s".into())],
        ..Invocation::default()
    };
    let runtime = RuntimeConfig::resolve(&tree.0, None, &invocation).unwrap();
    let layers = [
        layer(
            &tree,
            LayerKind::Project,
            json!({"version":1,"policy":{"budget":{"max_calls":null},
            "stages":{"verify":{"hard_timeout":"2s"}},"completeness":{"required_reviewers":["b"]}},
            "profiles":{"focused":{"policy":{"stages":{"verify":{"enabled":false}},
                "retries":{"verify":{"count":0}},"executors":{"opencode":{"env":{"TOKEN":"synthetic-secret"}}}}}}}),
        ),
        layer(
            &tree,
            LayerKind::User,
            json!({"version":1,"policy":{"budget":{"max_calls":2},"completeness":{"required_reviewers":["a"]}}}),
        ),
    ];
    let effective = EffectivePolicy::resolve(
        &runtime,
        &layers,
        "focused",
        &json!({"verification":{"min_confidence":0}}),
    )
    .unwrap();
    let result = effective.inspection();
    assert_eq!(result["values"]["stages"]["verify"]["hard_timeout"], "19s");
    assert_eq!(result["values"]["stages"]["verify"]["enabled"], false);
    assert_eq!(result["values"]["retries"]["verify"]["count"], 0);
    assert_eq!(
        result["values"]["budget"]["max_calls"],
        serde_json::Value::Null
    );
    assert_eq!(
        result["values"]["completeness"]["required_reviewers"],
        json!(["b"])
    );
    assert_eq!(result["origins"]["/budget/max_calls"]["layer"], "project");
    assert_eq!(
        result["origins"]["/stages/verify/enabled"]["profile"],
        "focused"
    );
    assert_eq!(
        result["origins"]["/verification/min_confidence"]["layer"],
        "flag"
    );
    assert!(!result.to_string().contains("synthetic-secret"));
    assert_eq!(
        result["values"]["executors"]["opencode"]["env"]["TOKEN"],
        "<redacted>"
    );
}

#[test]
fn runtime_admission_uses_resolved_limits_and_preserves_legacy_unlimited_parallelism() {
    use revust_core::runtime::Limits;
    let tree = setup();
    let invocation = Invocation {
        config_dir: Some(tree.0.join("user")),
        overrides: vec![("max-parallel".into(), "0".into())],
        ..Invocation::default()
    };
    let runtime = RuntimeConfig::resolve(&tree.0, None, &invocation).unwrap();
    let policy = EffectivePolicy::resolve(
        &runtime,
        &[],
        "comprehensive",
        &json!({
            "budget":{"max_calls":7,"elapsed":"1m"}, "stages":{"verify":{"parallelism":2}}
        }),
    )
    .unwrap();
    let limits = Limits::from_policy(&policy).unwrap();
    assert_eq!(limits.max_calls, Some(7));
    assert_eq!(limits.elapsed, Some(std::time::Duration::from_secs(60)));
    assert_eq!(limits.max_parallel, usize::MAX);
    assert_eq!(limits.stage_parallel, [usize::MAX, 1, 2]);
}
