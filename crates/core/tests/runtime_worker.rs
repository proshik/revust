use revust_core::{
    config::{
        extensions::EffectivePolicy,
        runtime::{Invocation, RuntimeConfig},
    },
    runtime::{
        Attempt, Clock, DispatchError, Limits, RunControl, Stage, StopReason,
        retry::{Failure, RetryPolicy},
        worker::{AttemptFailure, Event, Outcome, run_unit},
    },
};
use serde_json::json;
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
#[derive(Default)]
struct FakeClock(AtomicU64);
impl Clock for FakeClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.0.load(Ordering::SeqCst))
    }
}
impl FakeClock {
    fn advance(&self, by: Duration) {
        self.0.fetch_add(by.as_nanos() as u64, Ordering::SeqCst);
    }
}
fn control(clock: Arc<FakeClock>, calls: u64, seconds: u64) -> RunControl {
    RunControl::new(
        Limits {
            max_calls: Some(calls),
            elapsed: Some(Duration::from_secs(seconds)),
            max_parallel: 1,
            stage_parallel: [1; 3],
        },
        clock,
    )
    .unwrap()
}
fn attempt(unit: &str) -> Attempt {
    Attempt {
        stage: Stage::Find,
        unit: unit.into(),
        number: 0,
    }
}
fn retry() -> RetryPolicy {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "revust-worker-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let runtime = RuntimeConfig::resolve(
        &root,
        None,
        &Invocation {
            config_dir: Some(root.join("user")),
            ..Invocation::default()
        },
    )
    .unwrap();
    let policy = EffectivePolicy::resolve(&runtime, &[], "comprehensive", &json!({})).unwrap();
    fs::remove_dir_all(root).unwrap();
    RetryPolicy::from_policy(&policy, Stage::Find).unwrap()
}
fn transient() -> AttemptFailure {
    AttemptFailure {
        kind: Failure::MissingOutput,
        diagnostic: "synthetic partial result".into(),
    }
}

#[test]
fn retry_keeps_its_scheduler_slot_and_records_each_attempt_before_dispatch() {
    let clock = Arc::new(FakeClock::default());
    let run = control(clock.clone(), 3, 20);
    let mut recorded = Vec::new();
    let retry_announced = std::cell::Cell::new(false);
    let mut calls = 0;
    let result = run_unit(
        &run,
        attempt("first"),
        &retry(),
        |duration| {
            assert!(retry_announced.get(), "retry decision precedes backoff");
            assert_eq!(
                run.try_dispatch(attempt("queued")).err(),
                Some(DispatchError::Busy)
            );
            clock.advance(duration);
        },
        |_| Duration::ZERO,
        |id, _| {
            calls += 1;
            assert_eq!(id.number, calls - 1);
            if calls == 1 {
                Err(transient())
            } else {
                Ok("answer")
            }
        },
        |event| {
            recorded.push(match event {
                Event::Started(id) => (id.number, "start"),
                Event::Finished(record) => (record.id.number, "finish"),
                Event::Retrying(record) => {
                    retry_announced.set(true);
                    (record.id.number, "retry")
                }
            });
            Ok(())
        },
    );
    assert_eq!(result.outcome, Outcome::Success);
    assert_eq!(
        recorded,
        vec![
            (0, "start"),
            (0, "finish"),
            (0, "retry"),
            (1, "start"),
            (1, "finish")
        ]
    );
    assert_eq!(run.calls_spent(), 2);
    assert_eq!(
        result.attempts[0].result.as_ref().unwrap_err().diagnostic,
        "synthetic partial result"
    );
    assert_eq!(result.attempts[1].result.as_ref().unwrap(), &"answer");
    assert_eq!(clock.now(), Duration::from_secs(5));
    assert!(run.try_dispatch(attempt("queued")).is_ok());
}

#[test]
fn deadline_and_cancellation_during_backoff_do_not_dispatch_or_record_a_retry() {
    for cancel in [false, true] {
        let clock = Arc::new(FakeClock::default());
        let run = control(clock.clone(), 3, 2);
        let result = run_unit::<()>(
            &run,
            attempt("first"),
            &retry(),
            |duration| {
                clock.advance(duration);
                if cancel {
                    run.cancel();
                }
            },
            |_| Duration::ZERO,
            |_, _| Err(transient()),
            |_| Ok(()),
        );
        assert_eq!(
            result.outcome,
            Outcome::Stopped(if cancel {
                StopReason::Cancelled
            } else {
                StopReason::Deadline
            })
        );
        assert_eq!(result.attempts.len(), 1);
        assert_eq!(run.calls_spent(), 1);
    }
}

#[test]
fn exhausted_budget_skips_backoff_and_failed_journal_prevents_execution() {
    let clock = Arc::new(FakeClock::default());
    let run = control(clock.clone(), 1, 10);
    let result = run_unit::<()>(
        &run,
        attempt("first"),
        &retry(),
        |_| panic!("no pointless backoff"),
        |_| Duration::ZERO,
        |_, _| Err(transient()),
        |_| Ok(()),
    );
    assert_eq!(result.outcome, Outcome::Stopped(StopReason::CallBudget));
    assert_eq!(run.calls_spent(), 1);
    let run = control(clock, 3, 10);
    let result = run_unit::<()>(
        &run,
        attempt("first"),
        &retry(),
        |_| {},
        |_| Duration::ZERO,
        |_, _| panic!("journal failed before provider launch"),
        |_| Err("disk full".into()),
    );
    assert_eq!(result.outcome, Outcome::JournalError("disk full".into()));
    assert_eq!(run.check_running(), Some(StopReason::Cancelled));
    assert!(result.attempts.is_empty());
}

#[test]
fn queue_time_consumes_deadline_and_late_success_is_retained_as_partial() {
    let clock = Arc::new(FakeClock::default());
    let run = control(clock.clone(), 3, 2);
    let slot = run.try_dispatch(attempt("occupier")).unwrap();
    let result = run_unit::<()>(
        &run,
        attempt("queued"),
        &retry(),
        |duration| clock.advance(duration),
        |_| Duration::ZERO,
        |_, _| panic!("queued past deadline"),
        |_| Ok(()),
    );
    assert_eq!(result.outcome, Outcome::Stopped(StopReason::Deadline));
    assert_eq!(run.calls_spent(), 1);
    drop(slot);
    let run = control(clock.clone(), 3, 2);
    let result = run_unit(
        &run,
        attempt("late"),
        &retry(),
        |_| {},
        |_| Duration::ZERO,
        |_, _| {
            clock.advance(Duration::from_secs(3));
            Ok("partial answer")
        },
        |_| Ok(()),
    );
    assert_eq!(result.outcome, Outcome::Stopped(StopReason::Deadline));
    assert_eq!(
        result.attempts[0].result.as_ref().unwrap(),
        &"partial answer"
    );
}
