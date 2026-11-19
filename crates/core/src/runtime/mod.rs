//! Shared run admission. Presentation and transport observers clone this handle;
//! they never create a new budget for an existing run.
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};

/// A monotonic clock. Queueing and backoff never modify the run's start time.
pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Find,
    Synthesis,
    Verify,
}
impl Stage {
    fn index(self) -> usize {
        match self {
            Self::Find => 0,
            Self::Synthesis => 1,
            Self::Verify => 2,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Attempt {
    pub stage: Stage,
    pub unit: String,
    /// Zero for the initial dispatch, incremented for each retry.
    pub number: u32,
}
pub struct Limits {
    pub max_calls: Option<u64>,
    pub elapsed: Option<Duration>,
    pub max_parallel: usize,
    /// Find, synthesis, verify. Each is also capped by max_parallel.
    pub stage_parallel: [usize; 3],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    CallBudget,
    Deadline,
    Cancelled,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchError {
    Busy,
    Duplicate,
    Stopped(StopReason),
}
#[derive(Clone)]
pub struct RunControl(Arc<Shared>);
struct Shared {
    limits: Limits,
    deadline: Option<Duration>,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    calls: u64,
    active: [usize; 3],
    dispatched: BTreeSet<Attempt>,
    stopped: Option<StopReason>,
}
/// Owns a scheduler slot, not a refundable call. Drop releases the slot only.
pub struct Permit {
    run: RunControl,
    stage: Stage,
}
impl RunControl {
    pub fn new(limits: Limits, clock: Arc<dyn Clock>) -> Result<Self, String> {
        if limits.max_calls == Some(0)
            || limits.elapsed == Some(Duration::ZERO)
            || limits.max_parallel == 0
            || limits.stage_parallel.contains(&0)
        {
            return Err("runtime limits must be positive when enabled".into());
        }
        let deadline = limits
            .elapsed
            .map(|elapsed| {
                clock
                    .now()
                    .checked_add(elapsed)
                    .ok_or("whole-run deadline overflows the monotonic clock")
            })
            .transpose()?;
        Ok(Self(Arc::new(Shared {
            limits,
            deadline,
            clock,
            state: Mutex::new(State::default()),
        })))
    }

    /// Reserve immediately before dispatch. Busy/duplicate/stopped requests
    /// consume no call. Once granted, even a spawn failure spends the call.
    pub fn try_dispatch(&self, attempt: Attempt) -> Result<Permit, DispatchError> {
        let mut state = self.0.state.lock().expect("run admission lock poisoned");
        if let Some(reason) = self.stopped(&mut state) {
            return Err(DispatchError::Stopped(reason));
        }
        if state.dispatched.contains(&attempt) {
            return Err(DispatchError::Duplicate);
        }
        if self
            .0
            .limits
            .max_calls
            .is_some_and(|max| state.calls >= max)
            || state.calls == u64::MAX
        {
            return Err(DispatchError::Stopped(StopReason::CallBudget));
        }
        let stage = attempt.stage;
        if state.active.iter().sum::<usize>() >= self.0.limits.max_parallel
            || state.active[stage.index()] >= self.0.limits.stage_parallel[stage.index()]
        {
            return Err(DispatchError::Busy);
        }
        state.dispatched.insert(attempt);
        state.calls += 1;
        state.active[stage.index()] += 1;
        Ok(Permit {
            run: self.clone(),
            stage,
        })
    }
    pub(crate) fn now(&self) -> Duration {
        self.0.clock.now()
    }
    /// Upper bound on units of `stage` that could hold a slot at once: the
    /// stage and global ceilings and the calls left. At least one, so queued
    /// units can still report a stopped outcome; exactly one once stopped.
    pub(crate) fn dispatch_width(&self, stage: Stage) -> usize {
        let mut state = self.0.state.lock().expect("run admission lock poisoned");
        if self.stopped(&mut state).is_some() {
            return 1;
        }
        let calls = self.0.limits.max_calls.map_or(usize::MAX, |max| {
            usize::try_from(max.saturating_sub(state.calls)).unwrap_or(usize::MAX)
        });
        self.0
            .limits
            .max_parallel
            .min(self.0.limits.stage_parallel[stage.index()])
            .min(calls)
            .max(1)
    }
    pub fn calls_spent(&self) -> u64 {
        self.0
            .state
            .lock()
            .expect("run admission lock poisoned")
            .calls
    }
    pub fn cancel(&self) {
        let mut state = self.0.state.lock().expect("run admission lock poisoned");
        if self.stopped(&mut state).is_none() {
            state.stopped = Some(StopReason::Cancelled);
        }
    }
    /// Call exhaustion prevents further dispatch but does not kill a call
    /// already reserved. Deadlines and explicit cancellation do stop it.
    pub fn check_running(&self) -> Option<StopReason> {
        self.stopped(&mut self.0.state.lock().expect("run admission lock poisoned"))
    }
    fn check_dispatch(&self) -> Option<StopReason> {
        let mut state = self.0.state.lock().expect("run admission lock poisoned");
        self.stopped(&mut state).or_else(|| {
            (self
                .0
                .limits
                .max_calls
                .is_some_and(|max| state.calls >= max)
                || state.calls == u64::MAX)
                .then_some(StopReason::CallBudget)
        })
    }
    fn wait_for(&self, delay: Duration, mut wait: impl FnMut(Duration)) -> Result<(), StopReason> {
        let until = self.0.clock.now().saturating_add(delay);
        loop {
            if let Some(reason) = self.check_dispatch() {
                return Err(reason);
            }
            let remaining = until.saturating_sub(self.0.clock.now());
            if remaining.is_zero() {
                return Ok(());
            }
            wait(remaining.min(Duration::from_millis(10)));
        }
    }
    fn stopped(&self, state: &mut State) -> Option<StopReason> {
        if state.stopped.is_none() && self.0.deadline.is_some_and(|d| self.0.clock.now() >= d) {
            state.stopped = Some(StopReason::Deadline);
        }
        state.stopped
    }
}
impl Permit {
    pub fn check_running(&self) -> Option<StopReason> {
        self.run.check_running()
    }
    /// Reserve a retry without releasing the worker's scheduler slot. The
    /// qualified finder holds its slot throughout backoff and retries.
    fn reserve_retry(&self, attempt: Attempt) -> Result<(), DispatchError> {
        debug_assert_eq!(attempt.stage, self.stage);
        let mut state = self
            .run
            .0
            .state
            .lock()
            .expect("run admission lock poisoned");
        if let Some(reason) = self.run.stopped(&mut state) {
            return Err(DispatchError::Stopped(reason));
        }
        if state.dispatched.contains(&attempt) {
            return Err(DispatchError::Duplicate);
        }
        if self
            .run
            .0
            .limits
            .max_calls
            .is_some_and(|max| state.calls >= max)
            || state.calls == u64::MAX
        {
            return Err(DispatchError::Stopped(StopReason::CallBudget));
        }
        state.dispatched.insert(attempt);
        state.calls += 1;
        Ok(())
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.run
            .0
            .state
            .lock()
            .expect("run admission lock poisoned")
            .active[self.stage.index()] -= 1;
    }
}

impl Limits {
    pub fn from_policy(
        policy: &crate::config::extensions::EffectivePolicy,
    ) -> Result<Self, String> {
        let values = policy.execution_values();
        fn parallel(value: &serde_json::Value) -> Result<usize, String> {
            // The legacy CLI permits zero/negative values meaning uncapped.
            // Extension-authored zero has already been rejected by its schema.
            if value.as_i64().is_some_and(|n| n <= 0) {
                return Ok(usize::MAX);
            }
            usize::try_from(value.as_u64().ok_or("missing parallelism")?)
                .map_err(|_| "parallelism exceeds platform capacity".into())
        }
        let elapsed = values["budget"]["elapsed"]
            .as_str()
            .map(|text| {
                let duration = text.parse::<crate::config::duration::LegacyDuration>()?;
                u64::try_from(duration.0)
                    .map(Duration::from_nanos)
                    .map_err(|_| "negative whole-run deadline".to_owned())
            })
            .transpose()?;
        Ok(Self {
            max_calls: values["budget"]["max_calls"].as_u64(),
            elapsed,
            max_parallel: parallel(&values["runtime"]["max_parallel"])?,
            stage_parallel: [
                parallel(&values["stages"]["find"]["parallelism"])?,
                parallel(&values["stages"]["synthesis"]["parallelism"])?,
                parallel(&values["stages"]["verify"]["parallelism"])?,
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Attempt, Limits, MonotonicClock, RunControl, Stage};
    use std::sync::Arc;
    fn control(max_calls: Option<u64>, max_parallel: usize, verify: usize) -> RunControl {
        let limits = Limits {
            max_calls,
            elapsed: None,
            max_parallel,
            stage_parallel: [8, 1, verify],
        };
        RunControl::new(limits, Arc::new(MonotonicClock::default())).unwrap()
    }
    fn spend(run: &RunControl, calls: usize) {
        for n in 0..calls {
            let attempt = Attempt {
                stage: Stage::Find,
                unit: n.to_string(),
                number: 0,
            };
            drop(run.try_dispatch(attempt).unwrap());
        }
    }
    #[test]
    fn dispatch_width_takes_the_lowest_of_both_ceilings_and_calls_left() {
        assert_eq!(control(None, 1, 6).dispatch_width(Stage::Verify), 1);
        assert_eq!(control(None, 8, 3).dispatch_width(Stage::Verify), 3);
        assert_eq!(
            control(None, usize::MAX, usize::MAX).dispatch_width(Stage::Verify),
            usize::MAX
        );
        let run = control(Some(5), 8, 6);
        spend(&run, 3);
        assert_eq!(run.dispatch_width(Stage::Verify), 2);
    }
    #[test]
    fn dispatch_width_is_one_without_calls_or_after_a_stop() {
        let run = control(Some(2), 8, 6);
        spend(&run, 2);
        assert_eq!(run.dispatch_width(Stage::Verify), 1);
        let run = control(None, 8, 6);
        run.cancel();
        assert_eq!(run.dispatch_width(Stage::Verify), 1);
    }
}

pub mod retry;

pub mod worker;

/// Clock shared by production run admission and retry scheduling.
pub struct MonotonicClock(std::time::Instant);
impl Default for MonotonicClock {
    fn default() -> Self {
        Self(std::time::Instant::now())
    }
}
impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

pub mod data;

pub mod groups;

pub mod pipeline;

mod stagger;

#[cfg(unix)]
pub mod journal;

pub mod events;
pub mod inputs;
