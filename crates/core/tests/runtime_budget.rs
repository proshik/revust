use revust_core::runtime::{Attempt, Clock, DispatchError, Limits, RunControl, Stage, StopReason};
use std::{
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
struct ClockStub(AtomicU64);
impl Clock for ClockStub {
    fn now(&self) -> Duration {
        Duration::from_secs(self.0.load(Ordering::SeqCst))
    }
}
fn limits() -> Limits {
    Limits {
        max_calls: Some(3),
        elapsed: Some(Duration::from_secs(10)),
        max_parallel: 2,
        stage_parallel: [1, 1, 3],
    }
}
fn attempt(stage: Stage, unit: &str, number: u32) -> Attempt {
    Attempt {
        stage,
        unit: unit.into(),
        number,
    }
}

#[test]
fn concurrent_last_call_is_reserved_once_and_never_refunded() {
    let mut limits = limits();
    limits.max_calls = Some(1);
    limits.max_parallel = 16;
    limits.stage_parallel = [16; 3];
    let run = RunControl::new(limits, Arc::new(ClockStub::default())).unwrap();
    let start = Arc::new(Barrier::new(16));
    let successes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|n| {
                let run = run.clone();
                let start = start.clone();
                scope.spawn(move || {
                    start.wait();
                    run.try_dispatch(attempt(Stage::Find, &n.to_string(), 0))
                        .is_ok()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum::<usize>()
    });
    assert_eq!(successes, 1);
    assert_eq!(run.calls_spent(), 1);
    assert_eq!(
        run.try_dispatch(attempt(Stage::Verify, "later", 0)).err(),
        Some(DispatchError::Stopped(StopReason::CallBudget))
    );
}

#[test]
fn busy_slots_do_not_spend_calls_and_global_ceiling_caps_stages() {
    let run = RunControl::new(limits(), Arc::new(ClockStub::default())).unwrap();
    let first = run.try_dispatch(attempt(Stage::Find, "a", 0)).unwrap();
    assert_eq!(
        run.try_dispatch(attempt(Stage::Find, "b", 0)).err(),
        Some(DispatchError::Busy)
    );
    let second = run.try_dispatch(attempt(Stage::Verify, "one", 0)).unwrap();
    assert_eq!(
        run.try_dispatch(attempt(Stage::Verify, "two", 0)).err(),
        Some(DispatchError::Busy)
    );
    assert_eq!(run.calls_spent(), 2);
    drop(first);
    let third = run.try_dispatch(attempt(Stage::Verify, "two", 0)).unwrap();
    assert_eq!(run.calls_spent(), 3);
    assert_eq!(second.check_running(), None);
    assert_eq!(third.check_running(), None);
}

#[test]
fn observer_clones_and_transport_replays_cannot_restart_an_attempt() {
    let run = RunControl::new(limits(), Arc::new(ClockStub::default())).unwrap();
    let observer = run.clone();
    drop(
        run.try_dispatch(attempt(Stage::Find, "reviewer", 0))
            .unwrap(),
    );
    assert_eq!(
        observer
            .try_dispatch(attempt(Stage::Find, "reviewer", 0))
            .err(),
        Some(DispatchError::Duplicate)
    );
    drop(
        observer
            .try_dispatch(attempt(Stage::Find, "reviewer", 1))
            .unwrap(),
    );
    assert_eq!(run.calls_spent(), 2);
}

#[test]
fn deadline_includes_queue_and_backoff_and_stops_running_attempts() {
    let clock = Arc::new(ClockStub::default());
    let run = RunControl::new(limits(), clock.clone()).unwrap();
    let active = run.try_dispatch(attempt(Stage::Find, "a", 0)).unwrap();
    clock.0.store(9, Ordering::SeqCst);
    assert_eq!(active.check_running(), None);
    clock.0.store(10, Ordering::SeqCst);
    assert_eq!(active.check_running(), Some(StopReason::Deadline));
    assert_eq!(
        run.try_dispatch(attempt(Stage::Find, "b", 0)).err(),
        Some(DispatchError::Stopped(StopReason::Deadline))
    );
    assert_eq!(run.calls_spent(), 1);
    // Advancing time without any active process still consumes the whole-run deadline.
    let queued = RunControl::new(limits(), clock.clone()).unwrap();
    clock.0.store(21, Ordering::SeqCst);
    assert_eq!(
        queued.try_dispatch(attempt(Stage::Find, "first", 0)).err(),
        Some(DispatchError::Stopped(StopReason::Deadline))
    );
    assert_eq!(queued.calls_spent(), 0);
}

#[test]
fn cancellation_is_shared_and_invalid_limits_fail_before_dispatch() {
    let run = RunControl::new(limits(), Arc::new(ClockStub::default())).unwrap();
    let active = run.try_dispatch(attempt(Stage::Find, "a", 0)).unwrap();
    run.clone().cancel();
    assert_eq!(active.check_running(), Some(StopReason::Cancelled));
    assert_eq!(
        run.try_dispatch(attempt(Stage::Verify, "v", 0)).err(),
        Some(DispatchError::Stopped(StopReason::Cancelled))
    );
    for invalid in [
        Limits {
            max_calls: Some(0),
            ..limits()
        },
        Limits {
            elapsed: Some(Duration::ZERO),
            ..limits()
        },
        Limits {
            max_parallel: 0,
            ..limits()
        },
        Limits {
            stage_parallel: [1, 0, 1],
            ..limits()
        },
    ] {
        assert!(RunControl::new(invalid, Arc::new(ClockStub::default())).is_err());
    }
}
