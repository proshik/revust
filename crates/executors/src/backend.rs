//! In-process review execution: provider transports and durable attempt evidence.
use crate::{
    configured::Settings,
    events::Event,
    opencode::isolated::Session,
    supervision::{Outcome, Stream, Termination},
};
use revust_core::{
    plan::ExecutionPlan,
    runtime::{
        Attempt, RunControl, Stage,
        data::Categories,
        journal::{Artifact, Journal, Stream as SavedStream},
        pipeline::{self, Call, Report},
        retry::Failure,
        worker::{self, AttemptFailure},
    },
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io,
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct Options {
    pub preserve_anthropic_api_key: bool,
    /// Explicit caller-selected Codex home; otherwise follows the child environment.
    pub codex_home: Option<PathBuf>,
}
pub struct ReviewResult {
    pub runtime: Report,
    pub report: revust_core::report::Report,
    pub manifest: String,
}
impl ReviewResult {
    pub fn exit_code(&self) -> i32 {
        if self.runtime.policy_satisfied() {
            self.report.exit_code()
        } else {
            2
        }
    }
}
pub struct Backend<P, J> {
    usage: std::sync::Mutex<BTreeMap<Attempt, revust_core::report::accounting::Usage>>,
    plan: ExecutionPlan,
    journal: Journal,
    settings: BTreeMap<(Stage, String), Settings>,
    opencode: BTreeMap<Stage, Session>,
    options: Options,
    progress: P,
    jitter: J,
}
impl<P: Fn(&Attempt, &Event) + Sync, J: Fn(Duration) -> Duration + Sync> Backend<P, J> {
    pub fn prepare(
        plan: ExecutionPlan,
        journal: Journal,
        options: Options,
        progress: P,
        jitter: J,
    ) -> Result<Self, String> {
        if plan.round() != journal.round() {
            return Err("execution plan and journal have different rounds".into());
        }
        if let Some(snapshot) = plan.extension_profile_snapshot() {
            journal.save_extension_profile(plan.profile(), snapshot)?;
        }
        journal.save_policy(plan.profile(), &plan.definition().policy)?;
        let definition = plan.definition();
        let values = definition.policy.execution_values();
        let mut selected: Vec<_> = definition
            .agents
            .iter()
            .map(|a| (Stage::Find, a.runner.executor.clone()))
            .collect();
        for (stage, name, runner) in [
            (Stage::Synthesis, "synthesis", &definition.synthesis),
            (Stage::Verify, "verify", &definition.verify),
        ] {
            if values["stages"][name]["enabled"] == true {
                selected.push((stage, runner.executor.clone()));
            }
        }
        selected.sort();
        selected.dedup();
        let mut settings = BTreeMap::new();
        let mut opencode = BTreeMap::new();
        for (stage, executor) in selected {
            let mut setting = Settings::resolve(&definition.policy, stage, &executor)?;
            setting.preserve_anthropic_api_key = options.preserve_anthropic_api_key;
            if executor == "codex"
                && let Some(home) = &options.codex_home
            {
                setting.environment.insert(
                    "CODEX_HOME".into(),
                    home.to_str().ok_or("Codex home is not UTF-8")?.into(),
                );
            }
            if executor == "opencode" {
                // OpenCode clears the inherited environment. Copy only the
                // explicitly opted-in key; configured child values still win.
                if options.preserve_anthropic_api_key
                    && !setting.environment.contains_key("ANTHROPIC_API_KEY")
                    && let Ok(value) = std::env::var("ANTHROPIC_API_KEY")
                {
                    setting
                        .environment
                        .insert("ANTHROPIC_API_KEY".into(), value);
                }
                let session =
                    Session::temporary(&setting, &values["executors"]["opencode"]["options"])
                        .map_err(|e| e.to_string())?;
                opencode.insert(stage, session);
            }
            settings.insert((stage, executor), setting);
        }
        for snapshot in plan.snapshots() {
            let parent = journal.round().join("prompts");
            if snapshot.destination != parent.join("input-profile.md") {
                return Err("unexpected calibration snapshot destination".into());
            }
            journal.save_calibration(&snapshot.content)?;
        }
        for (name, request) in plan.finder_requests() {
            journal.save_prompt(Stage::Find, name, request.stdin.as_bytes())?;
        }
        Ok(Self {
            usage: Default::default(),
            plan,
            journal,
            settings,
            opencode,
            options,
            progress,
            jitter,
        })
    }
    pub fn run(&self, control: &RunControl) -> Result<Report, String> {
        self.journal.run(self.plan.definition(), control, self)
    }
    pub fn run_report(&self, control: &RunControl) -> Result<ReviewResult, String> {
        use revust_core::report::accounting;
        let started = accounting::timestamp(std::time::SystemTime::now());
        let runtime = self.run(control)?;
        let finished = accounting::timestamp(std::time::SystemTime::now());
        let report = accounting::assemble(
            self.plan.scope().clone(),
            self.plan.definition(),
            &runtime,
            &*self.usage.lock().map_err(|_| "usage lock poisoned")?,
            started,
            finished,
        )?;
        let manifest = self.plan.manifest(&report)?;
        Ok(ReviewResult {
            runtime,
            report,
            manifest,
        })
    }
    fn execute_attempt(&self, call: Call<'_>, run: &RunControl) -> Result<Value, AttemptFailure> {
        let request = self.plan.request(&call).map_err(process_error)?;
        self.journal
            .save_prompt(
                call.attempt.stage,
                &call.attempt.unit,
                request.stdin.as_bytes(),
            )
            .map_err(|error| {
                run.cancel();
                process_error(error)
            })?;
        let save = |artifact, bytes: &[u8]| {
            self.journal
                .save(call.attempt, artifact, bytes)
                .map_err(|e| {
                    run.cancel();
                    process_error(e)
                })
        };
        save(
            Artifact::Request,
            &serde_json::to_vec(&request).map_err(|e| process_error(e.to_string()))?,
        )?;
        let settings = self
            .settings
            .get(&(call.attempt.stage, call.runner.executor.clone()))
            .ok_or_else(|| process_error("missing prepared executor".into()))?;
        self.journal
            .open_agent_stream(call.attempt, &call.runner.executor)
            .map_err(|error| {
                run.cancel();
                process_error(error)
            })?;
        let storage_error = RefCell::new(None);
        let raw = |stream, bytes: &[u8]| {
            self.journal
                .append(
                    call.attempt,
                    match stream {
                        Stream::Stdout => SavedStream::Stdout,
                        Stream::Stderr => SavedStream::Stderr,
                    },
                    bytes,
                )
                .map_err(|e| {
                    run.cancel();
                    io::Error::other(e)
                })
        };
        let emit = |event: Event| {
            let mut bytes = serde_json::to_vec(&event).expect("event serialization");
            bytes.push(b'\n');
            if let Err(e) = self
                .journal
                .append(call.attempt, SavedStream::ProviderEvents, &bytes)
            {
                run.cancel();
                *storage_error.borrow_mut() = Some(e);
                return;
            }
            use revust_core::runtime::events::{Event as ArchivedEvent, Kind};
            let (kind, text) = match &event {
                Event::Activity(text) => (Kind::AgentActivity, text),
                Event::Progress(text) => (Kind::AgentProgress, text),
                Event::Info(text) => (Kind::AgentState, text),
                Event::RateLimit(text) => (Kind::RateLimit, text),
            };
            if let Err(error) =
                self.journal
                    .publish(ArchivedEvent::agent(kind, call.display_name, text))
            {
                run.cancel();
                *storage_error.borrow_mut() = Some(error);
                return;
            }
            if matches!(event, Event::Activity(_) | Event::Progress(_)) {
                (call.activity)();
            }
            (self.progress)(call.attempt, &event);
        };
        let started = Instant::now();
        let result: io::Result<Delivery> = (|| match call.runner.executor.as_str() {
            "claude" => {
                let e = crate::claude::run(settings, request, run, raw, emit)?;
                let p = e.protocol;
                Ok(Delivery {
                    output: p.output,
                    model: p.actual_model,
                    tokens: json!(p.tokens),
                    first_token: json!(p.first_token_ms),
                    rate: p.rate_limited,
                    error: p.error.or_else(|| {
                        p.truncated
                            .then(|| "incomplete Claude protocol output".into())
                    }),
                    details: json!({"malformed_lines":p.malformed_lines,"truncated":p.truncated}),
                    process: e.process,
                })
            }
            "codex" => {
                let home = self
                    .options
                    .codex_home
                    .clone()
                    .or_else(|| settings.environment.get("CODEX_HOME").map(PathBuf::from))
                    .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
                    .or_else(|| {
                        settings
                            .environment
                            .get("HOME")
                            .map(PathBuf::from)
                            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                            .map(|p| p.join(".codex"))
                    });
                let e = crate::codex::run_with_rollout(
                    settings,
                    request,
                    run,
                    home.as_deref(),
                    raw,
                    emit,
                )?;
                let p = e.protocol;
                let rate = matches!(p.classification, crate::codex::Classification::RateLimit(_));
                let error = match &p.classification {
                    crate::codex::Classification::Transient(s)
                    | crate::codex::Classification::Diagnostic(s) => Some(s.clone()),
                    _ => None,
                };
                Ok(Delivery {
                    output: p.output,
                    model: p.actual_model,
                    tokens: json!(p.tokens),
                    first_token: Value::Null,
                    rate,
                    error: error.or_else(|| {
                        p.truncated
                            .then(|| "incomplete Codex protocol output".into())
                    }),
                    details: json!({"classification":format!("{:?}",p.classification),"truncated":p.truncated}),
                    process: e.process,
                })
            }
            "opencode" => {
                let session = self
                    .opencode
                    .get(&call.attempt.stage)
                    .ok_or_else(|| io::Error::other("missing qualified OpenCode session"))?
                    .temporary_attempt()?;
                let mut diagnostics = [Vec::new(), Vec::new()];
                let e = session.run(
                    request,
                    run,
                    |stream, bytes| {
                        raw(stream, bytes)?;
                        let buffer = &mut diagnostics[match stream {
                            Stream::Stdout => 0,
                            Stream::Stderr => 1,
                        }];
                        let remaining = 4096usize.saturating_sub(buffer.len());
                        buffer.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
                        Ok(())
                    },
                    emit,
                )?;
                let p = e.protocol;
                let error = opencode_error(&p, &e.process, &diagnostics);
                Ok(Delivery {
                    output: p.output,
                    model: p.actual_model,
                    tokens: json!(p.tokens),
                    first_token: Value::Null,
                    rate: false,
                    error,
                    details: json!({"malformed_lines":p.malformed_lines,"truncated":p.truncated}),
                    process: e.process,
                })
            }
            _ => Err(io::Error::other("unsupported executor")),
        })();
        if let Some(error) = storage_error.into_inner() {
            return Err(process_error(error));
        }
        let delivery = match result {
            Ok(delivery) => delivery,
            Err(e) => {
                save(Artifact::Delivery,&serde_json::to_vec(&json!({"requested":call.runner,"transport_error":e.to_string(),"elapsed_ms":started.elapsed().as_millis()})).expect("delivery serialization"))?;
                return Err(process_error(e.to_string()));
            }
        };
        let evidence = json!({"requested":call.runner,"output":delivery.output,"actual_model":delivery.model,"tokens":delivery.tokens,"first_token_ms":delivery.first_token,"protocol":delivery.details,"protocol_error":delivery.error,"elapsed_ms":started.elapsed().as_millis(),"process":{"pid":delivery.process.pid,"exit_code":delivery.process.status.and_then(|s|s.code()),"signal":delivery.process.status.and_then(|s|s.signal()),"termination":format!("{:?}",delivery.process.termination),"output_complete":delivery.process.output_complete}});
        save(
            Artifact::Delivery,
            &serde_json::to_vec(&evidence).expect("delivery serialization"),
        )?;
        let tokens = delivery
            .tokens
            .as_i64()
            .map(i128::from)
            .or_else(|| delivery.tokens.as_u64().map(i128::from));
        self.usage
            .lock()
            .map_err(|_| process_error("usage lock poisoned".into()))?
            .insert(
                call.attempt.clone(),
                revust_core::report::accounting::Usage {
                    tokens,
                    actual_model: delivery.model.clone(),
                },
            );
        delivery.accept()
    }
}
impl<P: Fn(&Attempt, &Event) + Sync, J: Fn(Duration) -> Duration + Sync> pipeline::Backend
    for Backend<P, J>
{
    fn execute(&self, call: Call<'_>, run: &RunControl) -> Result<Value, AttemptFailure> {
        self.execute_attempt(call, run)
    }
    fn record(&self, _: worker::Event<'_, Categories>) -> Result<(), String> {
        Ok(())
    }
    fn checkpoint(&self, stage: Stage, runtime: &Report) -> Result<(), String> {
        let snapshot = revust_core::report::accounting::assemble(
            revust_core::report::Scope::default(),
            self.plan.definition(),
            runtime,
            &*self.usage.lock().map_err(|_| "usage lock poisoned")?,
            "0001-01-01T00:00:00Z".into(),
            "0001-01-01T00:00:00Z".into(),
        )?;
        self.journal.save_stage(stage, snapshot.json()?.as_bytes())
    }
    fn wait(&self, duration: Duration) {
        std::thread::sleep(duration)
    }
    fn jitter(&self, bound: Duration) -> Duration {
        (self.jitter)(bound)
    }
}

fn opencode_error(
    protocol: &crate::opencode::Result,
    process: &Outcome,
    diagnostics: &[Vec<u8>],
) -> Option<String> {
    if let Some(error) = &protocol.error {
        return Some(format!("{}: {}", error.kind, error.message));
    }
    if protocol.truncated {
        return Some("truncated OpenCode output".into());
    }
    // A stopped, incomplete or unsuccessful provider request is not evidence
    // of an adapter mismatch. Only explicit CLI argument rejection overrides it.
    if process.termination != Termination::Exited || !process.output_complete {
        return None;
    }
    let rejected_arguments = [
        "unknown flag",
        "unknown option",
        "unrecognized option",
        "unexpected argument",
        "unknown argument",
        "flag provided but not defined",
    ];
    let rejected_arguments = diagnostics.iter().any(|bytes| {
        let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
        text.lines().any(|line| {
            rejected_arguments
                .iter()
                .any(|phrase| line.contains(phrase))
                && ["--standalone", "--format", "--title", "--agent", "--model"]
                    .iter()
                    .any(|flag| line.contains(flag))
        })
    });
    let reason = if !protocol.has_text && rejected_arguments {
        "OpenCode rejected the adapter's CLI arguments".to_owned()
    } else if !process.status.is_some_and(|status| status.success()) {
        return None;
    } else if protocol.malformed_lines > 0 && protocol.output.is_none() {
        format!(
            "OpenCode returned {} JSONL records the adapter could not decode",
            protocol.malformed_lines
        )
    } else if !protocol.has_text {
        "OpenCode finished without supported text events for a final response".into()
    } else {
        // Model text without valid review JSON belongs to normal result/schema
        // validation, not CLI protocol compatibility.
        return None;
    };
    Some(format!(
        "{reason}; the revust OpenCode adapter may need updating. Check the archived stdout/stderr when reporting unexpected CLI output."
    ))
}

fn process_error(diagnostic: String) -> AttemptFailure {
    AttemptFailure {
        kind: Failure::ProcessError,
        diagnostic,
    }
}
struct Delivery {
    output: Option<Value>,
    model: Option<String>,
    tokens: Value,
    first_token: Value,
    rate: bool,
    error: Option<String>,
    details: Value,
    process: Outcome,
}
impl Delivery {
    fn accept(self) -> Result<Value, AttemptFailure> {
        let complete_transport = self.process.output_complete
            && matches!(
                self.process.termination,
                Termination::Exited | Termination::IdleTimeout
            );
        if complete_transport
            && self.error.is_none()
            && let Some(output) = self.output
        {
            // A complete result is evidence independently of the exit code.
            // Core schema and acceptance policy validation still follow.
            return Ok(output);
        }
        let (kind, diagnostic) = match (
            self.process.output_complete,
            self.process.termination,
            self.error,
        ) {
            (false, _, _) => (
                Failure::ProcessError,
                "Provider transport ended with open output pipes".into(),
            ),
            (_, Termination::HardTimeout, _) => (
                Failure::HardTimeout,
                "Provider exceeded its hard deadline".into(),
            ),
            (_, Termination::RunStopped(reason), _) => (
                Failure::ProcessError,
                format!("Provider attempt stopped: {reason:?}"),
            ),
            (_, _, Some(error)) => (Failure::ProcessError, error),
            (_, Termination::IdleTimeout, _) => (
                Failure::IdleTimeout,
                "Provider became idle without a complete result".into(),
            ),
            _ if self.rate => (
                Failure::RateLimit,
                "Provider request was rate limited without a result".into(),
            ),
            _ if !self.process.status.is_some_and(|status| status.success()) => (
                Failure::NonzeroExit,
                "Provider exited unsuccessfully without a result".into(),
            ),
            _ => (
                Failure::MissingOutput,
                "Provider finished without a structured result".into(),
            ),
        };
        Err(AttemptFailure { kind, diagnostic })
    }
}
