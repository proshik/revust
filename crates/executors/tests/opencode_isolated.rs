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
fn capture_script(root: &std::path::Path, body: &str) -> String {
    script(
        root,
        &format!("printf '%s' \"$OPENCODE_CONFIG_CONTENT\" > \"$REVUST_CAPTURE\"; {body}"),
    )
}
fn response() -> &'static str {
    "printf '%s\\n' '{\"type\":\"text\",\"part\":{\"messageID\":\"m\",\"text\":\"{\\\"findings\\\":[]}\"}}'"
}
fn captured_config(capture: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(capture).unwrap()).unwrap()
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
fn providers_file_supplies_native_providers_and_env_overrides_by_key() {
    for (name, document) in [
        (
            "wrapper",
            json!({"providers":{
                "file-only":{"package":"@opencode/ai/providers/openai-compatible","name":"from file","models":{"m":{}}},
                "shared":{"package":"@opencode/ai/providers/openai-compatible","name":"from file"}}}),
        ),
        (
            "bare",
            json!({
                "file-only":{"package":"@opencode/ai/providers/openai-compatible","name":"from file","models":{"m":{}}},
                "shared":{"package":"@opencode/ai/providers/openai-compatible","name":"from file"}}),
        ),
    ] {
        let tmp = Temp::new();
        let capture = tmp.0.join("captured");
        let bin = capture_script(&tmp.0, response());
        let file = tmp.0.join(format!("providers-{name}.json"));
        fs::write(&file, document.to_string()).unwrap();
        let mut settings = settings(bin);
        settings
            .environment
            .insert("REVUST_CAPTURE".into(), capture.to_str().unwrap().into());
        settings.environment.insert(
            "OPENCODE_CONFIG_CONTENT".into(),
            json!({"providers":{"shared":{"name":"from env"}}}).to_string(),
        );
        let root = tmp.0.join("private");
        let session = Session::prepare(
            &root,
            &settings,
            &json!({"session_mode":"isolated","providers_file": file.to_str().unwrap()}),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        let result = session
            .run(request(&tmp.0), &control(), |_, _| Ok(()), |_| {})
            .unwrap();
        assert!(result.process.status.unwrap().success(), "{name}");
        let config = captured_config(&capture);
        assert_eq!(
            config["providers"]["file-only"],
            json!({"package":"@opencode/ai/providers/openai-compatible","name":"from file","models":{"m":{}}}),
            "{name}"
        );
        // Env content overrides a file provider by its whole entry.
        assert_eq!(
            config["providers"]["shared"],
            json!({"name":"from env"}),
            "{name}"
        );
    }
}

#[test]
fn providers_file_converts_opencode_jsonc_with_comments_and_trailing_commas() {
    let tmp = Temp::new();
    let capture = tmp.0.join("captured");
    let bin = capture_script(&tmp.0, response());
    let file = tmp.0.join("opencode.jsonc");
    fs::write(
        &file,
        r#"{
        // user opencode.jsonc; comment markers "inside" strings must survive
        "provider": {
            "ecom_glm": { /* native mapping of @ai-sdk/openai-compatible */
                "npm": "@ai-sdk/openai-compatible",
                "name": "GLM Ecom",
                "options": {"baseURL": "https://llm.example.invalid/v1", "apiKey": "{file:~/.secrets/glm}"},
                "models": {"glm-5.3-flash": {"options": {"maxTokens": 8}}, /* tail */},
            },
            "disabled": {"npm": "@ai-sdk/openai-compatible", "options": {}}, // filtered
        },
        "enabled_providers": ["ecom_glm",], /* done */
    }"#,
    )
    .unwrap();
    let mut settings = settings(bin);
    settings
        .environment
        .insert("REVUST_CAPTURE".into(), capture.to_str().unwrap().into());
    let root = tmp.0.join("private");
    let session = Session::prepare(
        &root,
        &settings,
        &json!({"providers_file": file.to_str().unwrap()}),
    )
    .unwrap();
    let result = session
        .run(request(&tmp.0), &control(), |_, _| Ok(()), |_| {})
        .unwrap();
    assert!(result.process.status.unwrap().success());
    let config = captured_config(&capture);
    let home = std::env::var("HOME").unwrap_or_default();
    assert_eq!(
        config["providers"]["ecom_glm"],
        json!({
            "name":"GLM Ecom",
            "package":"@opencode/ai/providers/openai-compatible",
            "settings":{"baseURL":"https://llm.example.invalid/v1","apiKey":format!("{{file:{home}/.secrets/glm}}")},
            "models":{"glm-5.3-flash":{"settings":{"maxTokens":8}}}
        })
    );
    assert!(config["providers"].get("disabled").is_none());
}

#[test]
fn providers_file_moves_model_level_variants_into_settings() {
    let tmp = Temp::new();
    let capture = tmp.0.join("captured");
    let bin = capture_script(&tmp.0, response());
    let file = tmp.0.join("opencode-variants.jsonc");
    fs::write(
        &file,
        r#"{
        "provider": {
            "ecom": {
                "npm": "@ai-sdk/openai-compatible",
                "name": "Ecom",
                "options": {"baseURL": "https://llm.example.invalid/v1"},
                "models": {
                    "deepseek-v4.1-flash": {
                        "name": "DeepSeek Flash",
                        "limit": {"context": 384000, "output": 32768},
                        "options": {"maxTokens": 32768, "temperature": 0.2},
                        "variants": {
                            "high": {"reasoningEffort": "high"},
                            "low": {"reasoningEffort": "low"}
                        },
                    },
                    "glm-5.3-flash": {
                        "options": {
                            "maxTokens": 8,
                            "variants": {
                                "fast": {"temperature": 0.1},
                                "medium": {"temperature": 0.6}
                            }
                        },
                        "variants": {"fast": {"temperature": 0.4}, "slow": {"temperature": 0.9}},
                    },
                    "variants-only": {
                        "variants": {"fast": {"temperature": 0.3}},
                    },
                    "scalar-options": {
                        "options": "broken",
                        "variants": {"fast": {"temperature": 0.5}},
                    },
                },
            },
        },
    }"#,
    )
    .unwrap();
    let mut settings = settings(bin);
    settings
        .environment
        .insert("REVUST_CAPTURE".into(), capture.to_str().unwrap().into());
    let root = tmp.0.join("private");
    let session = Session::prepare(
        &root,
        &settings,
        &json!({"providers_file": file.to_str().unwrap()}),
    )
    .unwrap();
    let result = session
        .run(request(&tmp.0), &control(), |_, _| Ok(()), |_| {})
        .unwrap();
    assert!(result.process.status.unwrap().success());
    let config = captured_config(&capture);
    let ecom = &config["providers"]["ecom"];
    // Model-level variants land inside settings; limit, name and the
    // converted options stay on the model untouched.
    let flash = &ecom["models"]["deepseek-v4.1-flash"];
    assert_eq!(
        flash,
        &json!({
            "name": "DeepSeek Flash",
            "limit": {"context": 384000, "output": 32768},
            "settings": {
                "maxTokens": 32768,
                "temperature": 0.2,
                "variants": {
                    "high": {"reasoningEffort": "high"},
                    "low": {"reasoningEffort": "low"}
                }
            }
        })
    );
    assert!(flash.get("variants").is_none());
    // Variants arriving through options merge with model-level ones
    // without losing keys; the model-level entry wins on a conflict.
    let glm = &ecom["models"]["glm-5.3-flash"];
    assert_eq!(
        glm,
        &json!({
            "settings": {
                "maxTokens": 8,
                "variants": {
                    "fast": {"temperature": 0.4},
                    "medium": {"temperature": 0.6},
                    "slow": {"temperature": 0.9}
                }
            }
        })
    );
    assert!(glm.get("variants").is_none());
    // Without options the model gains a freshly created settings object
    // holding exactly the model-level variants.
    assert_eq!(
        ecom["models"]["variants-only"],
        json!({"settings": {"variants": {"fast": {"temperature": 0.3}}}})
    );
    // A non-object settings value is replaced instead of dropping the
    // variant definitions.
    assert_eq!(
        ecom["models"]["scalar-options"],
        json!({"settings": {"variants": {"fast": {"temperature": 0.5}}}})
    );
}

#[test]
fn providers_file_refuses_missing_files_unknown_npm_and_unqualified_packages() {
    for (name, file, expected) in [
        ("missing", None, "providers file"),
        (
            "unknown npm",
            Some(r#"{"provider":{"x":{"npm":"@ai-sdk/anthropic","options":{}}}}"#),
            "@ai-sdk/anthropic",
        ),
        (
            "unqualified package",
            Some(r#"{"x":{"package":"some-npm-package"}}"#),
            "unqualified OpenCode provider package",
        ),
        (
            "unterminated comment",
            Some(r#"{"x":{"package":"@opencode/ai/providers/openai-compatible"}} /* unfinished"#),
            "block comment is not terminated",
        ),
    ] {
        let tmp = Temp::new();
        let root = tmp.0.join("private");
        let path = match file {
            Some(content) => {
                let path = tmp.0.join("providers-file");
                fs::write(&path, content).unwrap();
                path
            }
            None => tmp.0.join("missing.json"),
        };
        let options = json!({"providers_file": path.to_str().unwrap()});
        let error = Session::prepare(&root, &settings("/bin/sh".into()), &options)
            .err()
            .unwrap_or_else(|| panic!("{name}: must fail"))
            .to_string();
        assert!(error.contains(expected), "{name}: {error}");
        assert!(!root.exists(), "{name}");
    }
}

#[test]
fn empty_providers_file_disables_loading_without_touching_the_filesystem() {
    let tmp = Temp::new();
    let capture = tmp.0.join("captured");
    let bin = capture_script(&tmp.0, response());
    let mut settings = settings(bin);
    settings
        .environment
        .insert("REVUST_CAPTURE".into(), capture.to_str().unwrap().into());
    let root = tmp.0.join("private");
    let session = Session::prepare(&root, &settings, &json!({"providers_file": ""})).unwrap();
    session
        .run(request(&tmp.0), &control(), |_, _| Ok(()), |_| {})
        .unwrap();
    assert_eq!(captured_config(&capture)["providers"], json!({}));
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

#[test]
fn inherited_proxies_survive_isolation_and_explicit_values_win() {
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
        for mixed in [false, true] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "inherited_proxies_survive_isolation_and_explicit_values_win",
                ])
                .env("REVUST_PROXY_PROBE", "1");
            for (index, key) in KEYS.iter().enumerate() {
                child.env_remove(key);
                if !mixed || index % 3 != 0 {
                    child.env(
                        key,
                        if mixed {
                            String::new()
                        } else {
                            format!("synthetic-{index}")
                        },
                    );
                }
            }
            assert!(
                child.output().unwrap().status.success(),
                "isolated proxy preservation failed"
            );
        }
        return;
    }
    let tmp = Temp::new();
    for explicit in [false, true] {
        let mut body = String::new();
        for (index, key) in KEYS.iter().enumerate() {
            let expected = if explicit && index == 0 {
                Some("override".into())
            } else {
                std::env::var(key).ok()
            };
            body.push_str(&match expected {
                Some(value) => format!("test \"${{{key}-missing}}\" = '{value}' || exit 9;\n"),
                None => format!("test \"${{{key}+present}}\" = '' || exit 9;\n"),
            });
        }
        body.push_str(response());
        let mut configured = settings(script(&tmp.0, &body));
        if explicit {
            configured
                .environment
                .insert("HTTP_PROXY".into(), "override".into());
        }
        let session = Session::prepare(
            &tmp.0.join(if explicit { "explicit" } else { "inherited" }),
            &configured,
            &json!({}),
        )
        .unwrap();
        let execution = session
            .run(request(&tmp.0), &control(), |_, _| Ok(()), |_| {})
            .unwrap();
        assert!(
            execution.process.status.unwrap().success(),
            "proxy environment mismatch"
        );
    }
}

#[test]
fn path_lookup_skips_non_executable_files_and_directories() {
    const NAME: &str = "path_lookup_skips_non_executable_files_and_directories";
    let Some(probe) = std::env::var_os("REVUST_LOOKUP_PROBE") else {
        let tmp = Temp::new();
        let plain = tmp.0.join("plain");
        let directory = tmp.0.join("directory");
        let stub = tmp.0.join("stub");
        for dir in [&plain, &directory, &stub] {
            fs::create_dir(dir).unwrap();
        }
        // The readable non-executable file would pass the version probe if run.
        let version = "#!/bin/sh\nprintf 'opencode v2.0.21\\n'\n";
        fs::write(plain.join("opencode"), version).unwrap();
        fs::set_permissions(plain.join("opencode"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::create_dir(directory.join("opencode")).unwrap();
        fs::rename(script(&stub, response()), stub.join("opencode")).unwrap();
        // An executable FIFO is not a regular file either.
        let fifo = tmp.0.join("fifo");
        fs::create_dir(&fifo).unwrap();
        nix::unistd::mkfifo(&fifo.join("opencode"), nix::sys::stat::Mode::S_IRWXU).unwrap();
        // Like `which`, an empty or unset PATH finds nothing, not the
        // launch directory's executable.
        for (mode, path) in [
            ("selects", Some(vec![&plain, &directory, &fifo, &stub])),
            ("missing", Some(vec![&plain, &directory, &fifo])),
            ("missing", Some(vec![])),
            ("missing", None),
        ] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", NAME])
                .current_dir(&stub)
                .env("REVUST_LOOKUP_PROBE", &tmp.0)
                .env("REVUST_LOOKUP_MODE", mode);
            match path {
                Some(path) => command.env("PATH", std::env::join_paths(path).unwrap()),
                None => command.env_remove("PATH"),
            };
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        return;
    };
    let probe = PathBuf::from(probe);
    let root = probe.join("private");
    let prepared = Session::prepare(&root, &settings("opencode".into()), &json!({}));
    match std::env::var("REVUST_LOOKUP_MODE").unwrap().as_str() {
        "selects" => {
            let execution = prepared
                .unwrap()
                .run(request(&probe), &control(), |_, _| Ok(()), |_| {})
                .unwrap();
            assert_eq!(execution.protocol.output, Some(json!({"findings":[]})));
        }
        "missing" => {
            let error = prepared.err().expect("lookup must fail");
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
            assert_eq!(error.to_string(), "OpenCode executable not found");
        }
        mode => panic!("unknown lookup mode {mode}"),
    }
    assert!(!root.exists());
}
