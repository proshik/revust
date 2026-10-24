#![cfg(unix)]
use revust_core::{
    config::{
        extensions::EffectivePolicy,
        runtime::{Invocation, RuntimeConfig},
    },
    request::Request,
    runtime::{Limits, MonotonicClock, RunControl, Stage},
};
use revust_executors::{
    configured::Settings,
    supervision::{Stream, Termination},
};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
fn policy(patch: serde_json::Value, overrides: Vec<(String, String)>) -> EffectivePolicy {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "revust-stage-settings-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&root).unwrap();
    let config = RuntimeConfig::resolve(
        &root,
        None,
        &Invocation {
            config_dir: Some(root.join("user")),
            overrides,
            ..Invocation::default()
        },
    )
    .unwrap();
    let result = EffectivePolicy::resolve(&config, &[], "comprehensive", &patch).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    result
}
#[test]
fn resolved_limits_keep_defaults_stage_overrides_and_nonpositive_legacy_disabling() {
    let p = policy(json!({}), vec![]);
    for stage in [Stage::Find, Stage::Synthesis, Stage::Verify] {
        let settings = Settings::resolve(&p, stage, "codex").unwrap();
        assert_eq!(settings.limits.idle, Some(Duration::from_secs(120)));
        assert_eq!(settings.limits.hard, Some(Duration::from_secs(1200)));
        assert_eq!(settings.program, "codex");
    }
    let p = policy(
        json!({"stages":{"synthesis":{"idle_timeout":"17ms","hard_timeout":"23ms"},"verify":{"idle_timeout":"0s"}}}),
        vec![("hard-timeout".into(), "-1s".into())],
    );
    let s = Settings::resolve(&p, Stage::Synthesis, "claude").unwrap();
    assert_eq!(s.limits.idle, Some(Duration::from_millis(17)));
    assert_eq!(s.limits.hard, Some(Duration::from_millis(23)));
    let s = Settings::resolve(&p, Stage::Verify, "opencode").unwrap();
    assert!(s.limits.idle.is_none());
    assert!(s.limits.hard.is_none());
    assert!(Settings::resolve(&p, Stage::Find, "unknown").is_err());
}
#[test]
fn child_uses_selected_executable_environment_and_stage_watchdog() {
    let before = std::env::var_os("REVUST_CONFIGURED_VALUE");
    let p = policy(
        json!({"stages":{"find":{"idle_timeout":"40ms","hard_timeout":"2s"},"verify":{"idle_timeout":"0s","hard_timeout":"80ms"}},"executors":{"claude":{"executable":"/bin/sh","env":{"REVUST_CONFIGURED_VALUE":"child-only"}}}}),
        vec![],
    );
    for (stage, expected) in [
        (Stage::Find, Termination::IdleTimeout),
        (Stage::Verify, Termination::HardTimeout),
    ] {
        let settings = Settings::resolve(&p, stage, "claude").unwrap();
        let control = RunControl::new(
            Limits::from_policy(&p).unwrap(),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
        let mut output = vec![];
        let result = settings
            .run(
                Request {
                    program: "must-be-overridden".into(),
                    argv: vec![
                        "-c".into(),
                        "printf '%s' \"$REVUST_CONFIGURED_VALUE\"; /bin/sleep 5".into(),
                    ],
                    stdin: String::new(),
                    cwd: std::env::temp_dir(),
                },
                &control,
                |stream, bytes| {
                    if stream == Stream::Stdout {
                        output.extend_from_slice(bytes);
                    }
                    Ok(false)
                },
            )
            .unwrap();
        assert_eq!(result.termination, expected);
        assert_eq!(output, b"child-only");
        assert_eq!(std::env::var_os("REVUST_CONFIGURED_VALUE"), before);
    }
}

#[test]
fn provider_environment_filters_inherited_values_with_explicit_preservation() {
    if std::env::var("REVUST_ENV_PROBE").as_deref() != Ok("1") {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider_environment_filters_inherited_values_with_explicit_preservation",
                "--nocapture",
            ])
            .env("REVUST_ENV_PROBE", "1")
            .env("CLAUDECODE", "synthetic-parent")
            .env("ANTHROPIC_API_KEY", "synthetic-key-not-credential")
            .env("REVUST_KEEP", "keep")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        return;
    }
    let p = policy(
        json!({"executors":{"claude":{"executable":"/bin/sh"}}}),
        vec![],
    );
    for preserve in [false, true] {
        let mut settings = Settings::resolve(&p, Stage::Find, "claude").unwrap();
        settings.preserve_anthropic_api_key = preserve;
        let control = RunControl::new(
            Limits::from_policy(&p).unwrap(),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
        let mut bytes = vec![];
        settings.run(Request{program:"unused".into(),argv:vec!["-c".into(),"printf '%s/%s/%s' \"${CLAUDECODE-unset}\" \"${ANTHROPIC_API_KEY-unset}\" \"$REVUST_KEEP\"".into()],stdin:String::new(),cwd:std::env::temp_dir()},&control,|stream,data|{if stream==Stream::Stdout{bytes.extend_from_slice(data);}Ok(true)}).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            if preserve {
                "unset/synthetic-key-not-credential/keep"
            } else {
                "unset/unset/keep"
            }
        );
        assert_eq!(std::env::var("CLAUDECODE").unwrap(), "synthetic-parent");
        assert_eq!(
            std::env::var("ANTHROPIC_API_KEY").unwrap(),
            "synthetic-key-not-credential"
        );
    }
}

#[test]
fn explicit_child_values_win_after_inherited_filtering_without_parent_mutation() {
    let p = policy(
        json!({"executors":{"claude":{"executable":"/bin/sh","env":{"CLAUDECODE":"synthetic-child","ANTHROPIC_API_KEY":"synthetic-explicit"}}}}),
        vec![],
    );
    for preserve in [false, true] {
        let mut settings = Settings::resolve(&p, Stage::Find, "claude").unwrap();
        settings.preserve_anthropic_api_key = preserve;
        let control = RunControl::new(
            Limits::from_policy(&p).unwrap(),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
        let mut output = Vec::new();
        let outcome = settings.run(Request { program:"unused".into(), argv:vec!["-c".into(), "test \"$CLAUDECODE\" = synthetic-child && test \"$ANTHROPIC_API_KEY\" = synthetic-explicit && printf valid".into()], stdin:String::new(), cwd:std::env::temp_dir() }, &control, |stream, bytes| { if stream == Stream::Stdout { output.extend_from_slice(bytes); } Ok(true) }).unwrap();
        assert!(outcome.status.unwrap().success());
        assert_eq!(output, b"valid");
    }
}

#[test]
fn inherited_proxy_values_and_explicit_overrides_reach_codex_and_claude() {
    const KEYS: [&str; 8] = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ];
    if std::env::var("REVUST_PROXY_PROBE").as_deref() != Ok("1") {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "inherited_proxy_values_and_explicit_overrides_reach_codex_and_claude",
            ])
            .env("REVUST_PROXY_PROBE", "1");
        for (index, key) in KEYS.iter().enumerate() {
            child.env(
                key,
                if index == 0 {
                    String::new()
                } else {
                    format!("synthetic-{index}")
                },
            );
        }
        assert!(
            child.output().unwrap().status.success(),
            "inherited proxy policy failed"
        );
        return;
    }
    let p = policy(
        json!({"executors":{"claude":{"executable":"/bin/sh"},"codex":{"executable":"/bin/sh"}}}),
        vec![],
    );
    for executor in ["codex", "claude"] {
        for explicit in [false, true] {
            let mut settings = Settings::resolve(&p, Stage::Find, executor).unwrap();
            if explicit {
                settings
                    .environment
                    .insert("HTTP_PROXY".into(), "override".into());
            }
            let mut body = String::new();
            for (index, key) in KEYS.iter().enumerate() {
                let expected = if explicit && index == 0 {
                    "override".into()
                } else {
                    std::env::var(key).unwrap()
                };
                body.push_str(&format!(
                    "test \"${{{key}-missing}}\" = '{expected}' || exit 9;\n"
                ));
            }
            let control = RunControl::new(
                Limits::from_policy(&p).unwrap(),
                Arc::new(MonotonicClock::default()),
            )
            .unwrap();
            let result = settings
                .run(
                    Request {
                        program: "unused".into(),
                        argv: vec!["-c".into(), body],
                        stdin: String::new(),
                        cwd: std::env::temp_dir(),
                    },
                    &control,
                    |_, _| Ok(true),
                )
                .unwrap();
            assert!(
                result.status.unwrap().success(),
                "proxy child policy mismatch"
            );
            assert_eq!(std::env::var("HTTP_PROXY").unwrap(), "");
        }
    }
}
