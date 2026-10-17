mod review;
use revust_core::config::catalog::Catalog;
use revust_core::config::runtime::{Invocation, RuntimeConfig};
use revust_core::plan::PromptPlan;
use std::io::{self, Write};

const HELP: &str = "revust --task NAME --run NAME [--no-tui] [OPTIONS]\nrevust config [OPTIONS]\nrevust config --runtime-only [OPTIONS]\nrevust config --effective [OPTIONS]\nrevust config --extension-schema\nrevust dry-run --task NAME --run NAME [OPTIONS]\nrevust view --archive PATH [--no-tui] [--markdown]\nrevust --dump-defaults PATH\nrevust init [--config-dir PATH]\nrevust new --task NAME --run NAME [--tasks-dir PATH]\nrevust stats [--tasks-dir PATH] [--task NAME]\nrevust cleanup --task NAME [--tasks-dir PATH]\n\nconfig prints the legacy catalogue; --effective includes extension policy and\nnew profiles. --runtime-only inspects INI settings and paths without prompts.\n--extension-schema exports the version 1 JSON schema.\ndry-run prepares finder requests and deferred stage templates, including\ncontext and prior-round history. view reads saved results without current profiles.\nConfiguration inspection, dry-run and view are read-only.\n--dump-defaults exports embedded prompts, preserving existing entries.\ninit (--init) writes resolved prompts and a config template into ./.revust/.\n\nPaths: --config-dir PATH, --workdir PATH, --task NAME, --run NAME\nInformation: --help (-h), --version\nSettings: --idle-timeout DURATION, --hard-timeout DURATION,\n          --stagger-delay DURATION, --auto-exit DURATION,\n          --max-parallel N, --verify-groups N, --verify-group-by dir|source,\n          --tasks-dir PATH, --profile NAME, --codex-sandbox MODE\nreview/dry-run: --lenses NAME,NAME, --no-synthesis, --no-verify,\n                --min-confidence INTEGER, --preserve-anthropic-api-key\n";

fn main() {
    if let Err(error) = run() {
        // The terminal itself may have disappeared; a diagnostic write failure
        // must not replace the tool's error status with a panic.
        let _ = writeln!(io::stderr().lock(), "error: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "arguments must be UTF-8".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut invocation = Invocation::default();
    let mut command = None;
    let mut init = false;
    let mut version = false;
    let mut archive: Option<std::path::PathBuf> = None;
    let mut dump_defaults: Option<std::path::PathBuf> = None;
    let mut policy = serde_json::json!({});
    let mut min_confidence = None;
    let mut execution_options = revust_executors::backend::Options::default();
    let mut no_tui = false;
    let mut markdown = false;
    let mut runtime_only = false;
    let mut effective = false;
    let mut extension_schema = false;
    let mut lenses: Option<Vec<String>> = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                return io::stderr()
                    .lock()
                    .write_all(HELP.as_bytes())
                    .map_err(|e| e.to_string());
            }
            "--version" => version = true,
            "--" => break,
            "config" | "dry-run" | "view" | "init" | "new" | "stats" | "cleanup"
                if command.is_none() =>
            {
                command = Some(arg.as_str())
            }
            "--init" => init = true,
            "--preserve-anthropic-api-key" => execution_options.preserve_anthropic_api_key = true,
            "--no-tui" => {
                no_tui = true;
                policy["output"]["tui"] = false.into();
            }
            "--no-synthesis" => policy["stages"]["synthesis"]["enabled"] = false.into(),
            "--no-verify" => policy["stages"]["verify"]["enabled"] = false.into(),
            "--markdown" => {
                markdown = true;
                policy["output"]["format"] = "markdown".into();
            }
            "--runtime-only" if !runtime_only => runtime_only = true,
            "--effective" if !effective => effective = true,
            "--extension-schema" if !extension_schema => extension_schema = true,
            arg if arg.starts_with("--") => {
                let (key, value) = if let Some((key, value)) = arg[2..].split_once('=') {
                    (key, value)
                } else {
                    (
                        &arg[2..],
                        args.next()
                            .ok_or_else(|| format!("missing value for {arg}"))?
                            .as_str(),
                    )
                };
                match key {
                    "min-confidence" => {
                        min_confidence = Some(
                            revust_core::config::runtime::parse_integer(value)
                                .map_err(|e| format!("invalid minimum confidence: {e}"))?,
                        );
                    }
                    "archive" if command == Some("view") => archive = Some(value.into()),
                    "dump-defaults" => dump_defaults = Some(value.into()),
                    "config-dir" => invocation.config_dir = Some(value.into()),
                    "workdir" => invocation.workdir = Some(value.into()),
                    "task" => invocation.task = value.into(),
                    "run" => invocation.run = value.into(),
                    "lenses" => lenses.get_or_insert_with(Vec::new).extend(
                        value
                            .split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned),
                    ),
                    _ => {
                        revust_core::config::runtime::validate_option(key, value)?;
                        invocation.overrides.push((key.into(), value.into()));
                    }
                }
            }
            other if other.starts_with('-') => return Err(format!("unknown option: {other}")),
            _ if command.is_some() => (),
            other => return Err(format!("unsupported argument {other:?}; use revust --help")),
        }
    }
    init |= command == Some("init");
    dump_defaults = dump_defaults.filter(|path| !path.as_os_str().is_empty());
    if command.is_none()
        && dump_defaults.is_none()
        && !init
        && !version
        && invocation.task.is_empty()
    {
        return Err("--task is required; use revust --help".into());
    }
    if runtime_only && command != Some("config") {
        return Err("--runtime-only requires config".into());
    }
    if (effective || extension_schema) && command != Some("config") {
        return Err("--effective and --extension-schema require config".into());
    }
    if [effective, extension_schema, runtime_only]
        .into_iter()
        .filter(|v| *v)
        .count()
        > 1
    {
        return Err("config inspection modes are mutually exclusive".into());
    }
    if command == Some("view") {
        return view_archive(&archive.ok_or("view requires --archive")?, no_tui, markdown);
    }
    if !init
        && command.is_some()
        && ![Some("new"), Some("stats"), Some("cleanup")].contains(&command)
        && dump_defaults.is_none()
        && archive.is_some()
    {
        return Err("--archive requires view".into());
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if version {
        RuntimeConfig::resolve(&cwd, home.as_deref(), &invocation)?;
        return writeln!(io::stdout().lock(), "revust {}", env!("CARGO_PKG_VERSION"))
            .map_err(|e| e.to_string());
    }
    if init {
        RuntimeConfig::resolve(&cwd, home.as_deref(), &invocation)?;
        return initialize(&cwd, home.as_deref(), &invocation);
    }
    if let Some(destination) = dump_defaults {
        // The reference validates runtime INI/flags but does not load prompts.
        RuntimeConfig::resolve(&cwd, home.as_deref(), &invocation)?;
        return export_defaults(&destination);
    }
    if command == Some("new") {
        let runtime = RuntimeConfig::resolve(&cwd, home.as_deref(), &invocation)?;
        return prepare_round(&runtime, &invocation);
    }
    if command == Some("stats") {
        let runtime = RuntimeConfig::resolve(&cwd, home.as_deref(), &invocation)?;
        return print_stats(&runtime, &invocation);
    }
    if command == Some("cleanup") {
        let runtime = RuntimeConfig::resolve(&cwd, home.as_deref(), &invocation)?;
        return cleanup(&runtime, &invocation);
    }
    if command.is_none() {
        if archive.is_some() {
            return Err("--archive requires view".into());
        }
        let plan = PromptPlan::resolve_with_policy(
            &cwd,
            home.as_deref(),
            &invocation,
            lenses.as_deref().unwrap_or_default(),
            &policy,
        )?
        .with_min_confidence(min_confidence)
        .into_execution()?;
        let code = review::run(plan, markdown, execution_options)?;
        std::process::exit(code);
    }
    let data = if extension_schema {
        serde_json::to_vec_pretty(&revust_core::config::extensions::json_schema())
    } else if effective {
        serde_json::to_vec_pretty(&revust_core::config::extensions::inspect_with_policy(
            &cwd,
            home.as_deref(),
            &invocation,
            &policy,
            min_confidence,
        )?)
    } else if command == Some("dry-run") {
        serde_json::to_vec_pretty(
            &PromptPlan::resolve_with_policy(
                &cwd,
                home.as_deref(),
                &invocation,
                lenses.as_deref().unwrap_or_default(),
                &policy,
            )?
            .with_min_confidence(min_confidence),
        )
    } else if runtime_only {
        serde_json::to_vec_pretty(&RuntimeConfig::resolve(&cwd, home.as_deref(), &invocation)?)
    } else {
        serde_json::to_vec_pretty(&Catalog::resolve(&cwd, home.as_deref(), &invocation)?)
    }
    .map_err(|e| e.to_string())?;
    let mut output = io::stdout().lock();
    output.write_all(&data).map_err(|e| e.to_string())?;
    writeln!(output).map_err(|e| e.to_string())
}

#[cfg(unix)]
fn view_archive(path: &std::path::Path, no_tui: bool, markdown: bool) -> Result<(), String> {
    if let Some(tty) = revust::presentation::tty(!no_tui) {
        let mut view = revust::archive_view::load(path)?;
        revust::presentation::style(&mut view.status);
        revust_tui::terminal::browse(tty, &view.status, &mut view.model, |model, pane| {
            view.status.pane(model, pane)
        })
        .map_err(|e| e.to_string())?;
        let report = view.model.report.as_ref().ok_or("missing saved report")?;
        use std::io::IsTerminal;
        if !io::stdout().is_terminal() {
            let output = if markdown {
                report.markdown()
            } else {
                report.json()?
            };
            io::stdout()
                .lock()
                .write_all(output.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        return saved_outcome(report);
    }
    let saved = revust_core::archive::reader::load(path)?;
    let output = if markdown {
        saved.report.markdown()
    } else {
        saved.report.json()?
    };
    io::stdout()
        .lock()
        .write_all(output.as_bytes())
        .map_err(|e| e.to_string())?;
    saved_outcome(&saved.report)
}
#[cfg(unix)]
fn saved_outcome(report: &revust_core::report::Report) -> Result<(), String> {
    if let Some(outcome) = &report.outcome
        && outcome.incomplete()
    {
        return Err(outcome.diagnostic());
    }
    Ok(())
}
#[cfg(not(unix))]
fn view_archive(_: &std::path::Path, _: bool, _: bool) -> Result<(), String> {
    Err("archive viewing requires a qualified Unix platform".into())
}

#[cfg(unix)]
fn export_defaults(path: &std::path::Path) -> Result<(), String> {
    revust_core::prompts::materialize::dump_defaults(path, &mut io::stderr().lock())
}
#[cfg(not(unix))]
fn export_defaults(_: &std::path::Path) -> Result<(), String> {
    Err("prompt export requires a qualified Unix platform".into())
}

#[cfg(unix)]
fn initialize(
    cwd: &std::path::Path,
    home: Option<&std::path::Path>,
    invocation: &Invocation,
) -> Result<(), String> {
    let paths = revust_core::prompts::materialize::initialize(cwd, home, invocation)?;
    let data = serde_json::to_vec_pretty(&paths).map_err(|e| e.to_string())?;
    let mut output = io::stdout().lock();
    output
        .write_all(&data)
        .and_then(|_| writeln!(output))
        .map_err(|e| e.to_string())
}
#[cfg(not(unix))]
fn initialize(
    _: &std::path::Path,
    _: Option<&std::path::Path>,
    _: &Invocation,
) -> Result<(), String> {
    Err("project initialization requires a qualified Unix platform".into())
}

#[cfg(unix)]
fn prepare_round(runtime: &RuntimeConfig, invocation: &Invocation) -> Result<(), String> {
    if runtime.knobs.iter().any(|knob| knob.name == "tasks-dir" && matches!(&knob.value, revust_core::config::runtime::Value::Text(value) if value.is_empty())) {
        return Err("tasks directory is empty".into());
    }
    let paths = revust_core::archive::scaffold::create(
        &runtime.paths.tasks_dir,
        &invocation.task,
        &invocation.run,
    )?;
    let data = serde_json::to_vec_pretty(&paths).map_err(|e| e.to_string())?;
    let mut output = io::stdout().lock();
    output
        .write_all(&data)
        .and_then(|_| writeln!(output))
        .map_err(|e| e.to_string())
}
#[cfg(not(unix))]
fn prepare_round(_: &RuntimeConfig, _: &Invocation) -> Result<(), String> {
    Err("round preparation requires a qualified Unix platform".into())
}

#[cfg(unix)]
fn print_stats(runtime: &RuntimeConfig, invocation: &Invocation) -> Result<(), String> {
    let tasks = runtime
        .knobs
        .iter()
        .find_map(|knob| match (&knob.value, knob.name) {
            (revust_core::config::runtime::Value::Text(value), "tasks-dir") => Some(value),
            _ => None,
        })
        .expect("tasks-dir definition");
    let stats =
        revust_core::archive::stats::collect(std::path::Path::new(tasks), &invocation.task)?;
    let data = serde_json::to_vec_pretty(&stats).map_err(|e| e.to_string())?;
    let mut output = io::stdout().lock();
    output
        .write_all(&data)
        .and_then(|_| writeln!(output))
        .map_err(|e| e.to_string())
}
#[cfg(not(unix))]
fn print_stats(_: &RuntimeConfig, _: &Invocation) -> Result<(), String> {
    Err("archive statistics require a qualified Unix platform".into())
}

#[cfg(unix)]
fn cleanup(runtime: &RuntimeConfig, invocation: &Invocation) -> Result<(), String> {
    let tasks = runtime
        .knobs
        .iter()
        .find_map(|knob| match (&knob.value, knob.name) {
            (revust_core::config::runtime::Value::Text(value), "tasks-dir") => Some(value),
            _ => None,
        })
        .expect("tasks-dir definition");
    let result =
        revust_core::archive::cleanup::remove(std::path::Path::new(tasks), &invocation.task)?;
    let data = serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?;
    let mut output = io::stdout().lock();
    output
        .write_all(&data)
        .and_then(|_| writeln!(output))
        .map_err(|e| e.to_string())
}
#[cfg(not(unix))]
fn cleanup(_: &RuntimeConfig, _: &Invocation) -> Result<(), String> {
    Err("task cleanup requires a qualified Unix platform".into())
}
