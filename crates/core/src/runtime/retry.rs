//! Retry eligibility and bounded delay. Execution supplies failure classification
//! and a sampled jitter interval; this module neither sleeps nor dispatches.
use crate::{
    config::{duration::LegacyDuration, extensions::EffectivePolicy},
    runtime::Stage,
};
use std::time::Duration;
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    ProcessError,
    IdleTimeout,
    HardTimeout,
    RateLimit,
    NonzeroExit,
    MissingOutput,
    InvalidOutput,
}
impl Failure {
    fn name(self) -> &'static str {
        match self {
            Self::ProcessError => "process_error",
            Self::IdleTimeout => "idle_timeout",
            Self::HardTimeout => "hard_timeout",
            Self::RateLimit => "rate_limit",
            Self::NonzeroExit => "nonzero_exit",
            Self::MissingOutput => "missing_output",
            Self::InvalidOutput => "invalid_output",
        }
    }
}
pub struct RetryPolicy {
    count: u32,
    conditions: Option<Vec<String>>,
    base: Duration,
    factor: f64,
    maximum: Option<Duration>,
    jitter: Duration,
}
impl RetryPolicy {
    pub fn jitter_bound(&self) -> Duration {
        self.jitter
    }
    pub fn from_policy(policy: &EffectivePolicy, stage: Stage) -> Result<Self, String> {
        let name = match stage {
            Stage::Find => "find",
            Stage::Synthesis => "synthesis",
            Stage::Verify => "verify",
        };
        let value = &policy.execution_values()["retries"][name];
        let duration = |value: &serde_json::Value| -> Result<Duration, String> {
            let parsed = value
                .as_str()
                .ok_or("missing retry duration")?
                .parse::<LegacyDuration>()?;
            Ok(Duration::from_nanos(
                u64::try_from(parsed.0).map_err(|_| "negative retry duration")?,
            ))
        };
        Ok(Self {
            count: u32::try_from(value["count"].as_u64().ok_or("missing retry count")?)
                .map_err(|_| "retry count too large")?,
            conditions: value["conditions"].as_array().map(|items| {
                items
                    .iter()
                    .map(|v| v.as_str().expect("validated retry condition").to_owned())
                    .collect()
            }),
            base: duration(&value["delay"]["base"])?,
            factor: value["delay"]["factor"]
                .as_f64()
                .ok_or("missing retry factor")?,
            maximum: if value["delay"]["max"].is_null() {
                None
            } else {
                Some(duration(&value["delay"]["max"])?)
            },
            jitter: duration(&value["delay"]["jitter"])?,
        })
    }
    /// Retry numbers start at one. Jitter is a caller-provided sample in [0,jitter).
    /// The optional cap applies to the complete delay, including jitter.
    pub fn delay(
        &self,
        number: u32,
        failure: Failure,
        jitter: Duration,
    ) -> Result<Option<Duration>, String> {
        if number == 0 {
            return Err("retry number starts at one".into());
        }
        if number > self.count {
            return Ok(None);
        }
        let eligible = match &self.conditions {
            Some(conditions) => conditions.iter().any(|s| s == failure.name()),
            // Legacy find/synthesis retry a failed delivery, never a delivered
            // but invalid answer. Verify has count=0 unless explicitly changed.
            None => failure != Failure::InvalidOutput,
        };
        if !eligible {
            return Ok(None);
        }
        if (!self.jitter.is_zero() && jitter >= self.jitter)
            || (self.jitter.is_zero() && !jitter.is_zero())
        {
            return Err("jitter sample outside configured interval".into());
        }
        let grown = if self.base.is_zero() {
            Duration::ZERO
        } else if number == 1 || self.factor == 1.0 {
            self.base
        } else {
            Duration::try_from_secs_f64(
                self.base.as_secs_f64() * self.factor.powf(f64::from(number - 1)),
            )
            .unwrap_or(Duration::MAX)
        };
        let delay = grown.saturating_add(jitter);
        Ok(Some(
            self.maximum.map_or(delay, |maximum| delay.min(maximum)),
        ))
    }
}
