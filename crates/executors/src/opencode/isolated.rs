//! Owned, private OpenCode process configuration. No global settings are copied.
use crate::{
    configured::Settings,
    opencode,
    supervision::{self, Outcome, Stream},
};
use revust_core::{
    prompts::runner::Runner,
    request::{self, Request},
    runtime::RunControl,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs, io,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

pub struct Session {
    root: PathBuf,
    program: String,
    environment: BTreeMap<String, String>,
    limits: supervision::Limits,
}
pub struct Execution {
    pub protocol: opencode::Result,
    pub process: Outcome,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
impl Session {
    /// Provider-local state lives outside the archive's replaceable display path.
    pub fn temporary(settings: &Settings, options: &Value) -> io::Result<Self> {
        Self::prepare(&temporary_path(), settings, options)
    }
    pub fn temporary_attempt(&self) -> io::Result<Self> {
        self.for_attempt(&temporary_path())
    }
    /// Fresh home/database for each attempt, using already-qualified capabilities.
    pub fn for_attempt(&self, root: &Path) -> io::Result<Self> {
        Self::allocate(
            root,
            self.program.clone(),
            self.environment.clone(),
            supervision::Limits {
                idle: self.limits.idle,
                hard: self.limits.hard,
            },
        )
    }
    fn allocate(
        root: &Path,
        program: String,
        environment: BTreeMap<String, String>,
        limits: supervision::Limits,
    ) -> io::Result<Self> {
        // Exclusive creation prevents adopting or deleting a pre-existing directory.
        fs::DirBuilder::new().mode(0o700).create(root)?;
        let root = fs::canonicalize(root)?;
        let mut session = Self {
            root,
            program,
            environment,
            limits,
        };
        for (key, name) in [
            ("HOME", "home"),
            ("TMPDIR", "tmp"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
        ] {
            let path = session.root.join(name);
            fs::DirBuilder::new().mode(0o700).create(&path)?;
            session.environment.insert(
                key.into(),
                path.to_str()
                    .ok_or_else(|| invalid("OpenCode isolation path is not UTF-8"))?
                    .into(),
            );
        }
        session.environment.insert(
            "OPENCODE_TEST_HOME".into(),
            session.environment["HOME"].clone(),
        );
        for key in [
            "OPENCODE_DISABLE_AUTOUPDATE",
            "OPENCODE_DISABLE_MODELS_FETCH",
            "OPENCODE_CONFIG_PROJECT_DISABLE",
        ] {
            session.environment.insert(key.into(), "1".into());
        }
        Ok(session)
    }
    /// Exclusive per-attempt root, removed on drop. Explicit credentials stay in
    /// child environment; OPENCODE_CONFIG_CONTENT may supply only provider data.
    pub fn prepare(root: &Path, settings: &Settings, options: &Value) -> io::Result<Self> {
        let options = options
            .as_object()
            .ok_or_else(|| invalid("OpenCode options must be an object"))?;
        if options
            .keys()
            .any(|k| !["agent", "providers_file", "session_mode"].contains(&k.as_str()))
            || options.get("agent").is_some_and(|v| v != "revust-review")
            || options.get("session_mode").is_some_and(|v| v != "isolated")
        {
            return Err(invalid(
                "OpenCode supports the owned revust-review agent in isolated mode only",
            ));
        }
        let mut environment = BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]);
        // Preserve transport settings narrowly; explicit executor values below win.
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ] {
            if let Ok(value) = std::env::var(key) {
                environment.insert(key.into(), value);
            }
        }
        // File providers form the base layer; env content overrides by ID.
        let mut providers = json!({});
        if let Some(file) = options.get("providers_file") {
            let text = file
                .as_str()
                .ok_or_else(|| invalid("OpenCode providers_file must be a string"))?;
            if !text.is_empty() {
                let path = opencode::providers::resolve_path(text)?;
                let loaded = opencode::providers::load(&path)?;
                opencode::providers::validate(&loaded)?;
                providers = loaded;
            }
        }
        for (key, value) in &settings.environment {
            if key == "OPENCODE_CONFIG_CONTENT" {
                let config: Value = serde_json::from_str(value)
                    .map_err(|_| invalid("invalid OpenCode provider configuration"))?;
                let config = config
                    .as_object()
                    .ok_or_else(|| invalid("OpenCode provider configuration must be an object"))?;
                if config.keys().any(|k| k != "providers") {
                    return Err(invalid(
                        "isolated OPENCODE_CONFIG_CONTENT accepts only providers",
                    ));
                }
                let content = config
                    .get("providers")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                opencode::providers::validate(&content)?;
                if let Some(map) = content.as_object() {
                    let merged = providers
                        .as_object_mut()
                        .ok_or_else(|| invalid("OpenCode providers must be an object"))?;
                    for (id, provider) in map {
                        merged.insert(id.clone(), provider.clone());
                    }
                }
            } else if key == "HOME"
                || key == "TMPDIR"
                || key.starts_with("XDG_")
                || (key.starts_with("OPENCODE_") && key != "OPENCODE_API_KEY")
                || key == "CLAUDECODE"
            {
                return Err(invalid(
                    "child environment attempts to override OpenCode isolation",
                ));
            } else {
                environment.insert(key.clone(), value.clone());
            }
        }
        let program = executable(&settings.program)?;
        let mut session = Self::allocate(
            root,
            program,
            environment,
            supervision::Limits {
                idle: settings.limits.idle,
                hard: settings.limits.hard,
            },
        )?;
        let mut permissions = vec![json!({"action":"*","resource":"*","effect":"deny"})];
        for action in ["read", "glob", "grep", "external_directory"] {
            permissions.push(json!({"action":action,"resource":"*","effect":"allow"}));
        }
        let config = json!({"update":"disable","providers":providers,"agents":{"revust-review":{"mode":"primary","description":"Review source without edits or delegation","permissions":permissions}}});
        session
            .environment
            .insert("OPENCODE_CONFIG_CONTENT".into(), config.to_string());
        session.verify_version()?;
        Ok(session)
    }
    fn verify_version(&self) -> io::Result<()> {
        let mut output = Vec::new();
        let outcome = supervision::run_isolated(
            &Request {
                program: self.program.clone(),
                argv: vec!["--version".into()],
                stdin: String::new(),
                cwd: self.root.clone(),
            },
            &self.environment,
            supervision::Limits {
                idle: Some(std::time::Duration::from_secs(2)),
                hard: Some(std::time::Duration::from_secs(5)),
            },
            || None,
            |stream, bytes| {
                if stream == Stream::Stdout {
                    if output.len().saturating_add(bytes.len()) > 4096 {
                        return Err(invalid(
                            "OpenCode --version exceeded 4096 bytes; expected OpenCode 2.x",
                        ));
                    }
                    output.extend_from_slice(bytes);
                }
                Ok(!bytes.is_empty())
            },
        )
        .map_err(|error| {
            invalid(&format!(
                "OpenCode --version probe failed ({}); select a working OpenCode 2.x executable",
                error.kind()
            ))
        })?;
        if outcome.termination != supervision::Termination::Exited
            || !outcome.output_complete
            || !outcome.status.is_some_and(|status| status.success())
            || !std::str::from_utf8(&output).is_ok_and(supported_version)
        {
            return Err(invalid(&format!(
                "OpenCode --version must report OpenCode 2.x (2.minor.patch, optional prerelease/build suffix); check the configured executable (status {:?}, termination {:?}, output complete {}, version bytes {})",
                outcome.status,
                outcome.termination,
                outcome.output_complete,
                output.len()
            )));
        }
        Ok(())
    }
    /// Consume a request from the core formatter. Archive callbacks see bytes
    /// before decoding, including failed and interrupted attempts.
    pub fn run(
        &self,
        mut request: Request,
        control: &RunControl,
        mut raw: impl FnMut(Stream, &[u8]) -> io::Result<()>,
        mut emit: impl FnMut(opencode::Event),
    ) -> io::Result<Execution> {
        let runner = Runner {
            executor: "opencode".into(),
            model: request.argv.last().cloned().unwrap_or_default(),
            effort: String::new(),
        };
        let expected = request::compose(&runner, "", "", &request.cwd, "read-only")
            .map_err(|_| invalid("unqualified OpenCode request"))?;
        if request.argv != expected.argv {
            return Err(invalid("unqualified OpenCode request flags"));
        }
        request.program = self.program.clone();
        let mut decoder = opencode::Decoder::default();
        let process = supervision::run_isolated(
            &request,
            &self.environment,
            supervision::Limits {
                idle: self.limits.idle,
                hard: self.limits.hard,
            },
            || control.check_running(),
            |stream, bytes| {
                raw(stream, bytes)?;
                if stream == Stream::Stdout {
                    for event in decoder.feed(bytes) {
                        emit(event);
                    }
                }
                Ok(!bytes.is_empty())
            },
        )?;
        for event in decoder.finish() {
            emit(event);
        }
        Ok(Execution {
            protocol: decoder.result,
            process,
        })
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

// Accept the major-version contract without pinning an observed minor/patch.
fn supported_version(output: &str) -> bool {
    let version = output.trim();
    let version = version.strip_prefix("opencode ").unwrap_or(version);
    let version = version.strip_prefix('v').unwrap_or(version);
    let identifiers = |text: &str| {
        text.split('.').all(|part| {
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    };
    let core = if let Some((core, build)) = version.split_once('+') {
        if !identifiers(build) {
            return false;
        }
        core
    } else {
        version
    };
    let core = if let Some((core, prerelease)) = core.split_once('-') {
        if !identifiers(prerelease) {
            return false;
        }
        core
    } else {
        core
    };
    let parts: Vec<_> = core.split('.').collect();
    parts.len() == 3
        && parts[0] == "2"
        && parts[1..]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn executable(program: &str) -> io::Result<String> {
    let path = if program.contains('/') {
        PathBuf::from(program)
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|p| p.join(program))
            .find(|p| p.is_file())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "OpenCode executable not found")
            })?
    };
    fs::canonicalize(path)?
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("OpenCode executable path is not UTF-8"))
}

fn temporary_path() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "revust-opencode-{}-{time}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
