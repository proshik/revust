//! Portable provider request formatting; no process execution.
use crate::prompts::runner::Runner;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub const FINDER_SCHEMA: &str = include_str!("../schemas/finder.json");
pub const SYNTHESIS_SCHEMA: &str = include_str!("../schemas/synthesis.json");
pub const VERIFY_SCHEMA: &str = include_str!("../schemas/verify.json");

// Authored guidance; command flags below are pinned provider protocol data.
const NARRATION: &str = "\n\nProgress channel: briefly describe the evidence you are inspecting and report material discoveries or blockers as the investigation proceeds. Keep progress separate from the final structured result. Do not expose credentials or private environment values.\n";
const JSON_OUTPUT: &str = "\n\nDeliver the final result as one JSON object. Use the field types and allowed values in the following schema, without Markdown fences or surrounding explanation.\nOutput schema (JSON):\n";

#[derive(Clone, Debug, Serialize)]
pub struct Request {
    pub program: String,
    pub argv: Vec<String>,
    pub stdin: String,
    pub cwd: PathBuf,
}

#[derive(Clone, Copy)]
enum Provider {
    Claude,
    Codex,
    OpenCode,
}
impl FromStr for Provider {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, String> {
        match text {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::OpenCode),
            other => Err(format!("request formatter is unavailable for {other:?}")),
        }
    }
}
struct Arguments(Vec<String>);
impl Arguments {
    fn new(initial: &[&str]) -> Self {
        Self(initial.iter().map(|text| (*text).to_owned()).collect())
    }
    fn pair(&mut self, key: &str, value: &str) {
        self.0.extend([key.to_owned(), value.to_owned()]);
    }
}
impl Provider {
    fn arguments(self, sandbox: &str, runner: &Runner) -> Result<Arguments, String> {
        let mut args = match self {
            Self::Claude => {
                let mut args = Arguments::new(&["--print"]);
                for (key, value) in [
                    ("--output-format", "stream-json"),
                    ("--verbose", ""),
                    ("--permission-mode", "auto"),
                ] {
                    if value.is_empty() {
                        args.0.push(key.to_owned());
                    } else {
                        args.pair(key, value);
                    }
                }
                args.0.extend(
                    [
                        "--tools=Bash,Read,Grep,Glob,WebFetch,WebSearch",
                        "--strict-mcp-config",
                    ]
                    .map(str::to_owned),
                );
                args.pair("--setting-sources", "project");
                args.0.extend(
                    [
                        "--disable-slash-commands",
                        "--no-session-persistence",
                        "--include-partial-messages",
                    ]
                    .map(str::to_owned),
                );
                args
            }
            Self::Codex => {
                let sandbox = if sandbox.is_empty() {
                    "read-only"
                } else {
                    sandbox
                };
                if !["read-only", "workspace-write", "danger-full-access"].contains(&sandbox) {
                    return Err(format!("unsupported Codex sandbox {sandbox:?}"));
                }
                let mut args = Arguments::new(&["exec", "--skip-git-repo-check"]);
                args.pair("--sandbox", sandbox);
                args
            }
            Self::OpenCode => {
                validate_opencode(runner, sandbox)?;
                let mut args = Arguments::new(&["run", "--standalone"]);
                args.pair("--format", "json");
                args.pair("--title", "revust review");
                args.pair("--agent", "revust-review");
                args
            }
        };
        if !runner.model.is_empty() {
            args.pair(
                match self {
                    Self::Codex => "-m",
                    _ => "--model",
                },
                &runner.model,
            );
        }
        if !runner.effort.is_empty() {
            match self {
                Self::Claude => args.pair("--effort", &runner.effort),
                Self::Codex => {
                    args.pair("-c", &format!("model_reasoning_effort={}", runner.effort))
                }
                Self::OpenCode => unreachable!("effort validated before construction"),
            }
        }
        Ok(args)
    }
}
fn validate_opencode(runner: &Runner, sandbox: &str) -> Result<(), String> {
    let model = &runner.model;
    let mut segments = model.split('#');
    let base = segments.next().unwrap_or_default();
    let variant = segments.next();
    let provider_model = base
        .split_once('/')
        .is_some_and(|(provider, name)| !provider.is_empty() && !name.is_empty());
    let bad_variant = variant.is_some_and(str::is_empty) || segments.next().is_some();
    if !provider_model || bad_variant || model.chars().any(|c| c.is_whitespace() || c == '\0') {
        return Err("OpenCode requires an explicit provider/model[#variant]".into());
    }
    if !runner.effort.is_empty() {
        return Err(
            "OpenCode generic effort is unsupported; select an explicit model variant".into(),
        );
    }
    if !matches!(sandbox, "" | "read-only") {
        return Err("OpenCode supports only the qualified read-only tool policy".into());
    }
    Ok(())
}
pub fn compose(
    runner: &Runner,
    body: &str,
    schema: &str,
    cwd: &Path,
    sandbox: &str,
) -> Result<Request, String> {
    let provider: Provider = runner.executor.parse()?;
    let mut arguments = provider.arguments(sandbox, runner)?;
    let stdin = match (schema.is_empty(), provider) {
        (true, _) => body.to_owned(),
        (false, Provider::Claude) => {
            arguments.pair("--json-schema", schema);
            [body, NARRATION].concat()
        }
        (false, _) => [body, JSON_OUTPUT, schema, "\n"].concat(),
    };
    Ok(Request {
        program: runner.executor.clone(),
        argv: arguments.0,
        stdin,
        cwd: cwd.to_owned(),
    })
}
