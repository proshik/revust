//! A single activity latch with a monotonic fallback; admission stays in RunControl.
use super::{RunControl, StopReason};
use std::{sync::OnceLock, time::Duration};
pub(super) struct Gate {
    activity: OnceLock<()>,
    deadline: Duration,
}
impl Gate {
    pub fn new(run: &RunControl, value: &serde_json::Value) -> Result<Self, String> {
        let nanos = value
            .as_str()
            .ok_or("missing stagger delay")?
            .parse::<crate::config::LegacyDuration>()?
            .0;
        let gate = Self {
            activity: OnceLock::new(),
            deadline: run
                .now()
                .saturating_add(Duration::from_nanos(nanos.max(0) as u64)),
        };
        if nanos <= 0 {
            gate.release();
        }
        Ok(gate)
    }
    pub fn release(&self) {
        let _ = self.activity.set(());
    }
    pub fn wait(&self, run: &RunControl, mut wait: impl FnMut(Duration)) -> Result<(), StopReason> {
        loop {
            if self.activity.get().is_some() {
                return Ok(());
            }
            if let Some(reason) = run.check_dispatch() {
                return Err(reason);
            }
            let now = run.now();
            if now >= self.deadline {
                self.release();
                return Ok(());
            }
            wait((self.deadline - now).min(Duration::from_millis(10)));
        }
    }
}
