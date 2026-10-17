#![cfg(unix)]
use revust_core::{
    prompts::runner::Runner,
    request,
    runtime::{Limits, MonotonicClock, RunControl},
};
use revust_executors::{
    configured::Settings,
    opencode::isolated::Session,
    supervision::{self, Stream, Termination},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "revust-isolated-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn settings(program: String) -> Settings {
    Settings {
        program,
        environment: BTreeMap::new(),
        preserve_anthropic_api_key: false,
        limits: supervision::Limits {
            idle: Some(Duration::from_secs(1)),
            hard: Some(Duration::from_secs(2)),
        },
    }
}
fn control() -> RunControl {
    RunControl::new(
        Limits {
            max_calls: None,
            elapsed: None,
            max_parallel: 1,
            stage_parallel: [1; 3],
        },
        Arc::new(MonotonicClock::default()),
    )
    .unwrap()
}
fn request(root: &std::path::Path) -> request::Request {
    request::compose(
        &Runner {
            executor: "opencode".into(),
            model: "fixture/model".into(),
            effort: String::new(),
        },
        "synthetic prompt",
        request::FINDER_SCHEMA,
        root,
        "read-only",
    )
    .unwrap()
}
fn script(root: &std::path::Path, body: &str) -> String {
    let p = root.join("executor");
    fs::write(&p, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'opencode v2.0.21\\n'; exit 0; fi\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
    p.to_str().unwrap().into()
}
#[test]
fn isolated_transport_preserves_input_raw_output_and_private_child_config() {
    let tmp = Temp::new();
    let capture = tmp.0.join("captured");
    let bin = script(
        &tmp.0,
        "/bin/cat > \"$REVUST_CAPTURE.input\"; printf '%s' \"$OPENCODE_CONFIG_CONTENT\" > \"$REVUST_CAPTURE.config\"; printf '%s' \"$HOME\" > \"$REVUST_CAPTURE.home\"; printf diagnostic >&2; /bin/cat \"$REVUST_FIXTURE\"",
    );
    let mut settings = settings(bin);
    settings
        .environment
        .insert("REVUST_CAPTURE".into(), capture.to_str().unwrap().into());
    settings.environment.insert(
        "REVUST_FIXTURE".into(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/opencode-v2021-tools.jsonl")
            .to_str()
            .unwrap()
            .into(),
    );
    settings.environment.insert(
        "OPENCODE_CONFIG_CONTENT".into(),
        json!({"providers":{"fixture":{"models":{"model":{}}}}}).to_string(),
    );
    let root = tmp.0.join("private");
    let session = Session::prepare(&root, &settings, &json!({"session_mode":"isolated"})).unwrap();
    assert_eq!(
        fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(Session::prepare(&root, &settings, &json!({})).is_err());
    let req = request(&tmp.0);
    let expected = req.stdin.clone();
    let mut raw = vec![];
    let mut err = vec![];
    let attempt_root = tmp.0.join("attempt-private");
    let attempt_session = session.for_attempt(&attempt_root).unwrap();
    let result = attempt_session
        .run(
            req,
            &control(),
            |stream, bytes| {
                if stream == Stream::Stdout {
                    raw.extend_from_slice(bytes)
                } else {
                    err.extend_from_slice(bytes)
                }
                Ok(())
            },
            |_| {},
        )
        .unwrap();
    assert!(result.process.status.unwrap().success());
    assert_eq!(result.protocol.output, Some(json!({"findings":[]})));
    assert_eq!(raw, include_bytes!("fixtures/opencode-v2021-tools.jsonl"));
    assert_eq!(err, b"diagnostic");
    assert_eq!(
        fs::read_to_string(capture.with_extension("input")).unwrap(),
        expected
    );
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(capture.with_extension("config")).unwrap())
            .unwrap();
    assert_eq!(
        config["providers"]["fixture"]["models"],
        json!({"model":{}})
    );
    assert_eq!(
        config["agents"]["revust-review"]["permissions"][0],
        json!({"action":"*","resource":"*","effect":"deny"})
    );
    assert!(
        PathBuf::from(fs::read_to_string(capture.with_extension("home")).unwrap())
            .starts_with(fs::canonicalize(&attempt_root).unwrap())
    );
    assert!(
        config["agents"]["revust-review"]["permissions"]
            .as_array()
            .unwrap()
            .contains(&json!({"action":"external_directory","resource":"*","effect":"allow"}))
    );
    drop(attempt_session);
    assert!(!attempt_root.exists());
    assert!(root.exists());
    drop(session);
    assert!(!root.exists());
}
#[test]
fn preflight_refuses_environment_and_agent_escape_without_creating_session() {
    let tmp = Temp::new();
    let root = tmp.0.join("private");
    for (key, value) in [
        ("HOME", "/synthetic"),
        ("OPENCODE_CONFIG", "/synthetic/config"),
        ("OPENCODE_CONFIG_PROJECT_DISABLE", "0"),
        ("OPENCODE_CONFIG_CONTENT", "{\"plugins\":[\"unsafe\"]}"),
        (
            "OPENCODE_CONFIG_CONTENT",
            "{\"providers\":{\"x\":{\"package\":\"some-npm-package\"}}}",
        ),
    ] {
        let mut s = settings("/bin/sh".into());
        s.environment.insert(key.into(), value.into());
        assert!(Session::prepare(&root, &s, &json!({})).is_err());
        assert!(!root.exists());
    }
    for options in [json!({"agent":"build"}), json!({"session_mode":"shared"})] {
        assert!(Session::prepare(&root, &settings("/bin/sh".into()), &options).is_err());
        assert!(!root.exists());
    }
}
#[test]
fn isolated_transport_obeys_hard_timeout_and_refuses_server_attachment() {
    let tmp = Temp::new();
    let bin = script(&tmp.0, "/bin/sleep 30 & wait");
    let mut s = settings(bin);
    s.limits.hard = Some(Duration::from_millis(100));
    let session = Session::prepare(&tmp.0.join("private"), &s, &json!({})).unwrap();
    let mut bad = request(&tmp.0);
    bad.argv
        .extend(["--server".into(), "http://127.0.0.1:1".into()]);
    assert!(session.run(bad, &control(), |_, _| Ok(()), |_| {}).is_err());
    let result = session
        .run(request(&tmp.0), &control(), |_, _| Ok(()), |_| {})
        .unwrap();
    assert_eq!(result.process.termination, Termination::HardTimeout);
    assert!(result.protocol.output.is_none());
}

#[test]
fn two_x_versions_dispatch_supported_protocol_in_owned_sessions() {
    for version in [
        "opencode v2.0.0",
        "opencode v2.1.73",
        "opencode v2.999.1234",
        "opencode v2.3.0-rc.1+build.42",
        "2.8.9",
        "v2.8.9+local",
    ] {
        let tmp = Temp::new();
        let program = script(
            &tmp.0,
            "printf '%s\\n' '{\"type\":\"text\",\"part\":{\"messageID\":\"m\",\"text\":\"{\\\"findings\\\":[]}\"}}'",
        );
        let content = fs::read_to_string(&program)
            .unwrap()
            .replace("opencode v2.0.21", version);
        fs::write(&program, content).unwrap();
        let root = tmp.0.join("private");
        let session = Session::prepare(&root, &settings(program), &json!({}))
            .unwrap_or_else(|error| panic!("{version}: {error}"));
        let execution = session
            .run(request(&tmp.0), &control(), |_, _| Ok(()), |_| {})
            .unwrap();
        assert_eq!(
            execution.protocol.output,
            Some(json!({"findings":[]})),
            "{version}"
        );
        drop(session);
        assert!(!root.exists());
    }
}

#[test]
fn unsupported_or_failed_version_probes_refuse_dispatch_and_clean_private_state() {
    for probe in [
        "printf 'opencode v1.0.0\\n'",
        "printf 'opencode v3.0.0\\n'",
        "printf 'opencode v20.0.0\\n'",
        "printf 'opencode v2.bad.1\\n'",
        "printf 'opencode v2.1\\n'",
        "printf 'opencode v2.1.2.3\\n'",
        "printf 'opencode v2.1.2-\\n'",
        "printf 'opencode v2.1.2+\\n'",
        "printf 'opencode v2.1.2-rc..1\\n'",
        "printf 'opencode v2.1.2\\nprivate-probe-sentinel\\n'",
        "printf 'opencode v2.0.21\\n'; exit 7",
        "printf '\\377'",
        "i=0; while [ $i -lt 4097 ]; do printf x; i=$((i+1)); done",
        "exec /bin/sleep 30",
    ] {
        let tmp = Temp::new();
        let program = script(
            &tmp.0,
            &format!("touch '{}'", tmp.0.join("dispatched").display()),
        );
        let content = fs::read_to_string(&program).unwrap().replace(
            "printf 'opencode v2.0.21\\n'; exit 0",
            &format!("{probe}; exit 0"),
        );
        fs::write(&program, content).unwrap();
        let root = tmp.0.join("private");
        let error = Session::prepare(&root, &settings(program), &json!({}))
            .err()
            .expect("probe must fail")
            .to_string();
        assert!(
            error.contains("OpenCode") && error.contains("2.x"),
            "{probe}: {error}"
        );
        assert!(!error.contains("private-probe-sentinel"));
        assert!(error.len() < 1000);
        assert!(!tmp.0.join("dispatched").exists());
        assert!(!root.exists());
    }
}

#[test]
fn temporary_attempts_have_distinct_owned_homes_removed_on_drop() {
    let temp = Temp::new();
    let capture = temp.0.join("home");
    let program = script(
        &temp.0,
        "/bin/cat >/dev/null; printf '%s' \"$HOME\" > \"$REVUST_CAPTURE\"",
    );
    let mut settings = settings(program);
    settings
        .environment
        .insert("REVUST_CAPTURE".into(), capture.to_str().unwrap().into());
    let seed = Session::temporary(&settings, &json!({})).unwrap();
    let first = seed.temporary_attempt().unwrap();
    let second = seed.temporary_attempt().unwrap();
    first
        .run(request(&temp.0), &control(), |_, _| Ok(()), |_| {})
        .unwrap();
    let first_home = PathBuf::from(fs::read_to_string(&capture).unwrap());
    second
        .run(request(&temp.0), &control(), |_, _| Ok(()), |_| {})
        .unwrap();
    let second_home = PathBuf::from(fs::read_to_string(&capture).unwrap());
    assert_ne!(first_home, second_home);
    assert!(first_home.is_dir());
    assert!(second_home.is_dir());
    drop(first);
    assert!(!first_home.parent().unwrap().exists());
    assert!(second_home.is_dir());
    drop(second);
    assert!(!second_home.parent().unwrap().exists());
}
