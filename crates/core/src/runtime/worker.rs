//! One schedulable unit and its separate attempts. Its slot is held through
//! retries, while the shared run control owns call/deadline accounting.
use super::{
    Attempt, DispatchError, RunControl, StopReason,
    retry::{Failure, RetryPolicy},
};
use std::time::Duration;
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AttemptFailure {
    pub kind: Failure,
    pub diagnostic: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Completed<T> {
    pub id: Attempt,
    pub result: Result<T, AttemptFailure>,
}
pub enum Event<'a, T> {
    Started(&'a Attempt),
    Finished(&'a Completed<T>),
    /// Retry decision before backoff; the next attempt is not yet admitted.
    Retrying(&'a Completed<T>),
}
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Success,
    Failed,
    Stopped(StopReason),
    Duplicate,
    JournalError(String),
    Invalid(String),
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct UnitResult<T> {
    pub outcome: Outcome,
    pub attempts: Vec<Completed<T>>,
}

/// `wait` sleeps or advances the same injected clock used by RunControl. The
/// journal callback persists synchronously before dispatch and after completion;
/// an error cancels the run. Executors must observe the supplied run control.
pub fn run_unit<T>(
    run: &RunControl,
    mut id: Attempt,
    retries: &RetryPolicy,
    mut wait: impl FnMut(Duration),
    mut jitter: impl FnMut(Duration) -> Duration,
    mut execute: impl FnMut(&Attempt, &RunControl) -> Result<T, AttemptFailure>,
    mut record: impl FnMut(Event<'_, T>) -> Result<(), String>,
) -> UnitResult<T> {
    let mut attempts = Vec::new();
    if id.number != 0 {
        return UnitResult {
            outcome: Outcome::Invalid("new units start with attempt zero".into()),
            attempts,
        };
    }
    let permit = loop {
        match run.try_dispatch(id.clone()) {
            Ok(permit) => break permit,
            Err(DispatchError::Busy) => wait(Duration::from_millis(10)),
            Err(error) => {
                return UnitResult {
                    outcome: dispatch_error(error),
                    attempts,
                };
            }
        }
    };
    let outcome = loop {
        if let Err(error) = record(Event::Started(&id)) {
            run.cancel();
            break Outcome::JournalError(error);
        }
        if let Some(reason) = run.check_running() {
            break Outcome::Stopped(reason);
        }
        let result = execute(&id, run);
        attempts.push(Completed {
            id: id.clone(),
            result,
        });
        let completed = attempts.last().expect("just completed attempt");
        if let Err(error) = record(Event::Finished(completed)) {
            run.cancel();
            break Outcome::JournalError(error);
        }
        // Preserve even a late successful answer, but never call the stopped
        // run successful merely because its executor returned some data.
        if let Some(reason) = run.check_running() {
            break Outcome::Stopped(reason);
        }
        let failure = match &completed.result {
            Ok(_) => break Outcome::Success,
            Err(failure) => failure.kind,
        };
        let Some(number) = id.number.checked_add(1) else {
            break Outcome::Failed;
        };
        match retries.delay(number, failure, Duration::ZERO) {
            Ok(None) => break Outcome::Failed,
            Err(error) => break Outcome::Invalid(error),
            Ok(Some(_)) => (),
        }
        let delay = match retries.delay(number, failure, jitter(retries.jitter_bound())) {
            Ok(Some(delay)) => delay,
            Ok(None) => break Outcome::Failed,
            Err(error) => break Outcome::Invalid(error),
        };
        if let Err(error) = record(Event::Retrying(completed)) {
            run.cancel();
            break Outcome::JournalError(error);
        }
        if let Err(reason) = run.wait_for(delay, &mut wait) {
            break Outcome::Stopped(reason);
        }
        id.number = number;
        if let Err(error) = permit.reserve_retry(id.clone()) {
            break dispatch_error(error);
        }
    };
    UnitResult { outcome, attempts }
}
fn dispatch_error(error: DispatchError) -> Outcome {
    match error {
        DispatchError::Stopped(reason) => Outcome::Stopped(reason),
        DispatchError::Duplicate => Outcome::Duplicate,
        DispatchError::Busy => unreachable!("busy admission is handled by the queue"),
    }
}
