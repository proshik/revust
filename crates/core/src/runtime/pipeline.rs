//! Headless stage coordination. Provider protocols and archive storage are supplied by the caller.
use super::{
    Attempt, RunControl,
    data::{Categories, Finding},
    worker::{AttemptFailure, Event, UnitResult},
};
use crate::{
    config::extensions::EffectivePolicy,
    prompts::{Agent, runner::Runner},
};
use serde_json::Value;
use std::time::Duration;
mod lifecycle;
mod results;
use results::output;

pub struct Definition {
    pub agents: Vec<Agent>,
    pub synthesis: Runner,
    pub verify: Runner,
    pub policy: EffectivePolicy,
}
/// Final finder status, including a successful empty answer versus failed delivery.
pub struct Source {
    pub agent: Agent,
    pub findings: Option<Vec<Finding>>,
}
pub struct Call<'a> {
    pub attempt: &'a Attempt,
    /// Presentation identity; verification filenames retain their separate unit ID.
    pub display_name: &'a str,
    pub runner: &'a Runner,
    pub agent: Option<&'a Agent>,
    pub findings: &'a [Finding],
    pub sources: &'a [Source],
    /// Adapters call this only for meaningful provider activity, not raw bytes.
    pub activity: &'a (dyn Fn() + Sync),
}
/// Implementations must observe cancellation, keep attempts separate, and persist
/// records synchronously. Waiting and jitter use the run's injected time source.
pub trait Backend: Sync {
    fn publish(&self, _event: super::events::Event) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, call: Call<'_>, run: &RunControl) -> Result<Value, AttemptFailure>;
    fn record(&self, event: Event<'_, Categories>) -> Result<(), String>;
    /// Persist the state left by this stage before any subsequent dispatch.
    /// Pure in-memory backends may omit storage; production adapters must save it.
    fn checkpoint(&self, _stage: super::Stage, _report: &Report) -> Result<(), String> {
        Ok(())
    }
    fn wait(&self, duration: Duration);
    fn jitter(&self, bound: Duration) -> Duration;
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Complete,
    Partial,
    Failed,
    Stopped(super::StopReason),
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Report {
    #[serde(default)]
    pub stages: Vec<crate::report::StageRun>,
    #[serde(default)]
    pub duration_ms: i64,
    pub state: State,
    pub categories: Categories,
    pub successful_reviewers: Vec<String>,
    pub units: Vec<UnitResult<Categories>>,
    pub policy_failures: Vec<String>,
    /// Includes rejected verdicts which the compatibility report omits.
    pub verification_evidence: Vec<Finding>,
    pub synthesis_succeeded: bool,
}
impl Report {
    pub fn policy_satisfied(&self) -> bool {
        self.policy_failures.is_empty() && matches!(self.state, State::Complete | State::Partial)
    }
}
/// Roster rules shared by inspection, planning and the run, so a bad roster is
/// refused before a round is claimed. Names fold case as prompt filenames do;
/// required reviewers name finders exactly. Returns the required reviewers.
pub fn check_roster(agents: &[Agent], policy: &EffectivePolicy) -> Result<Vec<String>, String> {
    let required: Vec<String> = serde_json::from_value(
        policy.execution_values()["completeness"]["required_reviewers"].clone(),
    )
    .map_err(|e| e.to_string())?;
    if agents.iter().any(|agent| agent.name.is_empty()) {
        return Err("finder names must be nonempty and unique".into());
    }
    crate::prompts::distinct_agent_names(agents)?;
    if agents.is_empty() {
        return Err("review requires at least one finder".into());
    }
    if let Some(name) = required
        .iter()
        .find(|name| !agents.iter().any(|agent| agent.name == **name))
    {
        return Err(format!("required reviewer {name:?} is absent from roster"));
    }
    Ok(required)
}
pub fn run(
    definition: &Definition,
    control: &RunControl,
    backend: &impl Backend,
) -> Result<Report, String> {
    let policy = definition.policy.execution_values();
    let required = check_roster(&definition.agents, &definition.policy)?;
    // Parse all policies before the first attempt spends a call.
    let retries = [
        super::Stage::Find,
        super::Stage::Synthesis,
        super::Stage::Verify,
    ]
    .map(|stage| super::retry::RetryPolicy::from_policy(&definition.policy, stage))
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
    let started = control.now();
    let find_started = control.now();
    let mut report = Report {
        stages: vec![],
        duration_ms: 0,
        state: State::Complete,
        categories: Categories::default(),
        successful_reviewers: vec![],
        units: vec![],
        policy_failures: vec![],
        verification_evidence: vec![],
        synthesis_succeeded: false,
    };
    stage_event(backend, control, "find")?;
    let stagger = super::stagger::Gate::new(control, &policy["runtime"]["stagger_delay"])?;
    // Join in roster order: completion order must not alter synthesis inputs.
    let found = std::thread::scope(|scope| {
        let handles: Vec<_> = definition
            .agents
            .iter()
            .enumerate()
            .map(|(index, agent)| {
                let retry = &retries[0];
                let stagger = &stagger;
                scope.spawn(move || {
                    if index != 0
                        && let Err(reason) = stagger.wait(control, |d| backend.wait(d))
                    {
                        return lifecycle::finish(
                            backend,
                            control,
                            super::Stage::Find,
                            &agent.name,
                            &[],
                            UnitResult {
                                outcome: super::worker::Outcome::Stopped(reason),
                                attempts: vec![],
                            },
                        );
                    }
                    execute(
                        control,
                        backend,
                        retry,
                        super::Stage::Find,
                        &agent.name,
                        &agent.name,
                        &agent.lenses.join(", "),
                        &agent.runner,
                        Some(agent),
                        &[],
                        &[],
                        &|| {
                            if index == 0 {
                                stagger.release();
                            }
                        },
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("finder worker panicked"))
            .collect::<Vec<_>>()
    });
    let sources = results::collect_sources(&definition.agents, found, &mut report);
    report
        .stages
        .push(stage_timing("find", find_started, control, None));
    if report.successful_reviewers.is_empty() && !matches!(report.state, State::Stopped(_)) {
        report.state = State::Failed;
    }
    checkpoint(backend, control, super::Stage::Find, &report)?;
    if report.can_continue() && policy["stages"]["synthesis"]["enabled"] == true {
        stage_event(backend, control, "synthesis")?;
        let stage_started = control.now();
        let unit = execute(
            control,
            backend,
            &retries[1],
            super::Stage::Synthesis,
            "synthesis",
            "synthesis",
            &format!(
                "Consolidating {} candidates",
                report.categories.findings.len()
            ),
            &definition.synthesis,
            None,
            &report.categories.findings,
            &sources,
            &|| {},
        );
        report.stages.push(stage_timing(
            "synthesis",
            stage_started,
            control,
            Some(&definition.synthesis),
        ));
        report.synthesis_succeeded = unit.outcome == super::worker::Outcome::Success;
        if let Some(output) = output(&unit) {
            report.categories = output.clone();
        }
        let failed = !report.synthesis_succeeded;
        report.accept_unit(unit);
        if failed
            && report.can_continue()
            && policy["completeness"]["synthesis_failure"] != "passthrough"
        {
            report.state = State::Failed;
        }
        if report.synthesis_succeeded {
            checkpoint(backend, control, super::Stage::Synthesis, &report)?;
        }
    }
    let strict = policy["completeness"]["require_all_verified"] == true;
    let mut candidates = report.categories.findings.clone();
    if strict {
        candidates.extend(report.categories.open_questions.clone());
        candidates.extend(report.categories.pre_existing.clone());
    }
    // Neither model-authored verdicts nor disabled stages count as verification.
    for item in &mut report.categories.findings {
        item.verdict = "unverified".into();
    }
    if strict {
        for item in report
            .categories
            .open_questions
            .iter_mut()
            .chain(report.categories.pre_existing.iter_mut())
        {
            item.verdict = "unverified".into();
        }
    }
    if report.can_continue() && policy["stages"]["verify"]["enabled"] == true {
        stage_event(backend, control, "verify")?;
        let stage_started = control.now();
        let groups = super::groups::group(
            &candidates,
            policy["runtime"]["verify_group_by"] == "source",
            policy["verification"]["min_directory_group_size"]
                .as_u64()
                .ok_or("missing group size")?,
            policy["runtime"]["verify_groups"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok()),
        );
        let primary = std::mem::take(&mut report.categories.findings);
        report.categories.findings = groups
            .iter()
            .flat_map(|group| &group.findings)
            .filter_map(|candidate| primary.iter().find(|f| f.id == candidate.id).cloned())
            .collect();
        let verified = pool(
            control.dispatch_width(super::Stage::Verify),
            &groups,
            |group| {
                execute(
                    control,
                    backend,
                    &retries[2],
                    super::Stage::Verify,
                    &group.name,
                    &format!("verify {}", group.shown),
                    &group.keys.join(", "),
                    &definition.verify,
                    None,
                    &group.findings,
                    &[],
                    &|| {},
                )
            },
            std::thread::Builder::new,
            || control.check_running().is_none(),
        );
        for unit in verified {
            if let Some(output) = output(&unit) {
                report.verification_evidence.extend(output.findings.clone());
            }
            report.accept_unit(unit);
        }
        report.apply_verdicts();
        report.stages.push(stage_timing(
            "verify",
            stage_started,
            control,
            (!groups.is_empty()).then_some(&definition.verify),
        ));
        checkpoint(backend, control, super::Stage::Verify, &report)?;
    }
    for name in required {
        if !report.successful_reviewers.contains(&name) {
            report
                .policy_failures
                .push(format!("required reviewer {name} did not succeed"));
        }
    }
    if (report.successful_reviewers.len() as u64)
        < policy["completeness"]["min_successful_reviewers"]
            .as_u64()
            .ok_or("missing minimum successful reviewers")?
    {
        report
            .policy_failures
            .push("too few successful reviewers".into());
    }
    if policy["completeness"]["require_synthesis_success"] == true && !report.synthesis_succeeded {
        report
            .policy_failures
            .push("synthesis did not succeed".into());
    }
    // Inputs synthesis left unclaimed are already forwarded in findings. All
    // three reported synthesis categories participate, including rejected
    // verdicts removed from the compatibility findings list.
    if strict
        && candidates.iter().any(|candidate| {
            !report.verification_evidence.iter().any(|f| {
                f.id == candidate.id
                    && matches!(
                        f.verdict.as_str(),
                        "confirmed" | "refined" | "rejected" | "immaterial" | "pre_existing"
                    )
            })
        })
    {
        report
            .policy_failures
            .push("not all reported candidates were verified".into());
    }
    if let Some(reason) = control.check_running() {
        report.state = State::Stopped(reason);
    }
    report.duration_ms = milliseconds(control.now().saturating_sub(started));
    Ok(report)
}
fn checkpoint(
    backend: &impl Backend,
    control: &RunControl,
    stage: super::Stage,
    report: &Report,
) -> Result<(), String> {
    backend
        .checkpoint(stage, report)
        .inspect_err(|_| control.cancel())
}
fn stage_event(backend: &impl Backend, control: &RunControl, name: &str) -> Result<(), String> {
    backend
        .publish(super::events::Event::stage(name))
        .inspect_err(|_| control.cancel())
}
impl Report {
    fn can_continue(&self) -> bool {
        matches!(self.state, State::Complete | State::Partial)
    }
    fn accept_unit(&mut self, unit: UnitResult<Categories>) {
        use super::worker::Outcome;
        match &unit.outcome {
            Outcome::Success => (),
            Outcome::Stopped(reason) => {
                if !matches!(self.state, State::Stopped(_)) {
                    self.state = State::Stopped(*reason);
                }
            }
            Outcome::Failed if self.state == State::Complete => self.state = State::Partial,
            Outcome::Failed => (),
            Outcome::JournalError(error) | Outcome::Invalid(error) => {
                self.policy_failures.push(error.clone());
                self.state = State::Failed;
            }
            Outcome::Duplicate => {
                self.policy_failures
                    .push("attempt was already dispatched".into());
                self.state = State::Failed;
            }
        }
        self.units.push(unit);
    }
}
#[allow(clippy::too_many_arguments)]
fn execute(
    control: &RunControl,
    backend: &impl Backend,
    retries: &super::retry::RetryPolicy,
    stage: super::Stage,
    unit: &str,
    display_name: &str,
    start_text: &str,
    runner: &Runner,
    agent: Option<&Agent>,
    inputs: &[Finding],
    sources: &[Source],
    activity: &(dyn Fn() + Sync),
) -> UnitResult<Categories> {
    let result = super::worker::run_unit(
        control,
        Attempt {
            stage,
            unit: unit.into(),
            number: 0,
        },
        retries,
        |d| backend.wait(d),
        |bound| backend.jitter(bound),
        |attempt, run| {
            let value = backend.execute(
                Call {
                    attempt,
                    display_name,
                    runner,
                    agent,
                    findings: inputs,
                    sources,
                    activity,
                },
                run,
            )?;
            results::decode(stage, &value, unit, agent, inputs)
        },
        |event| {
            use super::events::{Event as Presentation, Kind};
            let presentation = match &event {
                Event::Started(attempt) if attempt.number == 0 => Some(Presentation::agent(
                    Kind::AgentStarted,
                    display_name,
                    start_text,
                )),
                Event::Retrying(done) => done.result.as_ref().err().map(|failure| {
                    Presentation::agent(Kind::AgentRetried, display_name, &failure.diagnostic)
                }),
                _ => None,
            };
            backend.record(event)?;
            if let Some(event) = presentation {
                backend.publish(event)?;
            }
            Ok(())
        },
    );
    lifecycle::finish(backend, control, stage, display_name, inputs, result)
}

/// Run `work` over a shared queue with at most `width` native threads, the
/// caller included, and return results in item order. A refused spawn, or a
/// run that stops while helpers start, only narrows the pool: the remaining
/// workers drain the queue.
fn pool<T: Sync, R: Send>(
    width: usize,
    items: &[T],
    work: impl Fn(&T) -> R + Sync,
    builder: impl Fn() -> std::thread::Builder,
    running: impl Fn() -> bool,
) -> Vec<R> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let drain = || {
        let mut done = Vec::new();
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(item) = items.get(index) else {
                break done;
            };
            done.push((index, work(item)));
        }
    };
    let mut finished = std::thread::scope(|scope| {
        let helpers: Vec<_> = (1..width.min(items.len()))
            .map_while(|_| {
                running()
                    .then(|| builder().spawn_scoped(scope, drain).ok())
                    .flatten()
            })
            .collect();
        let mut finished = drain();
        for helper in helpers {
            finished.extend(helper.join().expect("verification worker panicked"));
        }
        finished
    });
    finished.sort_unstable_by_key(|(index, _)| *index);
    finished.into_iter().map(|(_, result)| result).collect()
}
fn milliseconds(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}
fn stage_timing(
    name: &str,
    started: Duration,
    control: &RunControl,
    runner: Option<&Runner>,
) -> crate::report::StageRun {
    crate::report::StageRun {
        name: name.to_owned(),
        duration_ms: milliseconds(control.now().saturating_sub(started)),
        executor: runner
            .map(|runner| runner.executor.clone())
            .unwrap_or_default(),
        model: runner
            .map(|runner| runner.model.clone())
            .unwrap_or_default(),
        effort: runner
            .map(|runner| runner.effort.clone())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::pool;
    use std::{
        collections::BTreeSet,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        thread::Builder,
    };
    /// Spawns refused by the OS: no host can map this stack.
    fn refused() -> Builder {
        Builder::new().stack_size(usize::MAX / 2)
    }
    fn run(width: usize, items: usize, admit: usize) -> (Vec<usize>, usize, usize) {
        let attempts = AtomicUsize::new(0);
        let threads = Mutex::new(BTreeSet::new());
        let items = (0..items).collect::<Vec<_>>();
        let results = pool(
            width,
            &items,
            |n| {
                threads
                    .lock()
                    .unwrap()
                    .insert(format!("{:?}", std::thread::current().id()));
                n * 10
            },
            || {
                if attempts.fetch_add(1, Ordering::SeqCst) < admit {
                    Builder::new()
                } else {
                    refused()
                }
            },
            || true,
        );
        let threads = threads.lock().unwrap().len();
        (results, attempts.into_inner(), threads)
    }
    #[test]
    fn helpers_are_bounded_by_width_and_items() {
        for (width, items, helpers) in [(1, 9, 0), (3, 9, 2), (5, 2, 1), (4, 0, 0)] {
            let (results, attempts, threads) = run(width, items, usize::MAX);
            assert_eq!(results, (0..items).map(|n| n * 10).collect::<Vec<_>>());
            assert_eq!(attempts, helpers, "width {width}, items {items}");
            assert!(threads <= helpers + 1);
        }
    }
    #[test]
    fn refused_helpers_narrow_the_pool_and_every_item_still_runs_in_order() {
        assert!(refused().spawn(|| ()).is_err(), "the host must refuse");
        for admit in [0, 1] {
            let (results, attempts, threads) = run(4, 9, admit);
            assert_eq!(results, (0..9).map(|n| n * 10).collect::<Vec<_>>());
            assert_eq!(attempts, admit + 1, "spawning stops at the first refusal");
            assert!(threads <= admit + 1);
        }
    }
    #[test]
    fn a_run_stopped_while_helpers_start_creates_no_further_helpers() {
        // The first helper's item stops the run; the next spawn check sees it.
        let stopped = std::sync::atomic::AtomicBool::new(false);
        let checks = AtomicUsize::new(0);
        let attempts = AtomicUsize::new(0);
        let items = (0..9).collect::<Vec<_>>();
        let results = pool(
            4,
            &items,
            |n| {
                stopped.store(true, Ordering::SeqCst);
                n * 10
            },
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Builder::new()
            },
            || {
                if checks.fetch_add(1, Ordering::SeqCst) > 0 {
                    let started = std::time::Instant::now();
                    while !stopped.load(Ordering::SeqCst) {
                        assert!(started.elapsed().as_secs() < 10, "helper never ran");
                        std::thread::yield_now();
                    }
                }
                !stopped.load(Ordering::SeqCst)
            },
        );
        assert_eq!(results, (0..9).map(|n| n * 10).collect::<Vec<_>>());
        assert_eq!(attempts.into_inner(), 1, "no helper after the stop");
        assert_eq!(checks.into_inner(), 2, "spawning stops at the first stop");
    }
}
