use revust_core::{
    archive::Archive,
    plan::ExecutionPlan,
    runtime::{Limits, MonotonicClock, RunControl, journal::Identity},
};
use revust_executors::backend::{Backend, Options};
use std::{
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
mod progress;
mod signals;

pub fn run(plan: ExecutionPlan, markdown: bool, options: Options) -> Result<i32, String> {
    let policy = plan.definition().policy.execution_values();
    let tty = revust::presentation::tty(policy["output"]["tui"] == true);
    let mut view = tty
        .as_ref()
        .map(|_| {
            let delay = policy["output"]["auto_exit"]
                .as_str()
                .unwrap_or("0s")
                .parse::<revust_core::config::LegacyDuration>()?;
            let mut status = revust_tui::status::Status {
                task: plan.scope().task.clone(),
                run: plan.scope().run.clone(),
                profile: plan.profile().to_owned(),
                ..Default::default()
            };
            revust::presentation::style(&mut status);
            Ok::<_, String>((
                revust_tui::Model::new(plan.definition().agents.clone())
                    .with_stage_runners(
                        plan.definition().synthesis.clone(),
                        plan.definition().verify.clone(),
                    )
                    .with_auto_exit(Duration::from_nanos(delay.0.max(0) as u64)),
                status,
                revust::inputs::Sources::review(&plan),
            ))
        })
        .transpose()?;
    let markdown = markdown || policy["output"]["format"] == "markdown";
    let confidence = policy["verification"]["min_confidence"]
        .as_i64()
        .ok_or("missing confidence threshold")?;
    let control = RunControl::new(
        Limits::from_policy(&plan.definition().policy)?,
        Arc::new(MonotonicClock::default()),
    )?;
    let _signals = signals::Guard::install(control.clone())?;
    let mut seed = [0; 8];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut seed))
        .map_err(|e| e.to_string())?;
    let random = AtomicU64::new(u64::from_ne_bytes(seed).max(1));
    let scope = plan.scope();
    let archive = Archive::claim(plan.tasks_dir(), &scope.task, &scope.run)?;
    let journal = archive.journal(Identity {
        task: scope.task.clone(),
        run: scope.run.clone(),
        request: "review".into(),
    })?;
    let mut progress = progress::Progress::new(
        plan.definition()
            .agents
            .iter()
            .map(|agent| (agent.name.as_str(), agent.color.as_str())),
        progress::columns(std::env::var("COLUMNS").ok().as_deref()),
    );
    let events = journal.subscribe()?;
    let backend = Backend::prepare(
        plan,
        journal,
        options,
        |_, _| {},
        move |bound| {
            if bound.is_zero() {
                return Duration::ZERO;
            }
            let old = random
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |mut n| {
                    n ^= n << 13;
                    n ^= n >> 7;
                    n ^= n << 17;
                    Some(n)
                })
                .unwrap();
            let nanos = u128::from(old) % bound.as_nanos();
            Duration::new(
                (nanos / 1_000_000_000) as u64,
                (nanos % 1_000_000_000) as u32,
            )
        },
    )?;
    if let Some((model, _, sources)) = view.as_mut() {
        let inputs = revust::inputs::Snapshot::default().load(sources);
        let old = std::mem::replace(model, revust_tui::Model::new(vec![]));
        *model = old.with_inputs(inputs);
    }
    let (worker, completion) =
        revust::execution::Worker::start(control.clone(), archive, confidence, move |control| {
            backend.run_report(control)
        })?;
    let mut progress = if let (Some(tty), Some((mut model, status, _))) = (tty, view) {
        let mut updates = revust_tui::updates::Updates::new(events, completion);
        let action = revust_tui::terminal::observe(
            tty,
            &status,
            &mut model,
            |model, pane| status.pane(model, pane),
            &mut updates,
            // Runtime deadlines stop execution, but its saved outcome still
            // belongs in the observer. Only an external signal closes it here.
            signals::received,
        );
        match action {
            Ok(revust_tui::navigation::Action::Interrupt) => control.cancel(),
            Err(error) => {
                drop(worker);
                return Err(error.to_string());
            }
            _ => (),
        }
        None
    } else {
        let observer = std::thread::Builder::new()
            .name("review-progress".into())
            .spawn(move || {
                let mut stderr = io::stderr().lock();
                for event in events {
                    progress.refresh_columns();
                    if let Some(line) = progress.line(event) {
                        let _ = writeln!(stderr, "{line}");
                    }
                }
                progress
            })
            .map_err(|e| format!("start progress renderer: {e}"))?;
        Some(observer.join().map_err(|_| "progress renderer panicked")?)
    };
    let result = worker.join()?;
    let code = result.exit_code();
    if matches!(
        result.runtime.state,
        revust_core::runtime::pipeline::State::Complete
            | revust_core::runtime::pipeline::State::Partial
    ) && code != 2
        && let Some(progress) = &mut progress
    {
        progress.refresh_columns();
        let mut stderr = io::stderr().lock();
        for line in progress.finish(&result.report) {
            let _ = writeln!(stderr, "{line}");
        }
    }
    if code == 2 {
        let _ = writeln!(
            io::stderr().lock(),
            "review incomplete: {:?}; policy failures: {:?}",
            result.runtime.state,
            result.runtime.policy_failures
        );
        return Ok(code);
    }
    let output = if markdown {
        result.report.markdown()
    } else {
        result.report.json()?
    };
    io::stdout()
        .lock()
        .write_all(output.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(code)
}
