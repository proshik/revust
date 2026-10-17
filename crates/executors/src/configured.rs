//! Resolve a stage's child settings once, before reserving provider calls.
use crate::supervision::{self, Limits, Outcome, Stream};
use revust_core::{
    config::extensions::EffectivePolicy,
    request::Request,
    runtime::{RunControl, Stage},
};
use std::{collections::BTreeMap, io};
pub struct Settings {
    pub program: String,
    pub environment: BTreeMap<String, String>,
    pub limits: Limits,
    /// Explicit legacy --preserve-anthropic-api-key opt-in.
    pub preserve_anthropic_api_key: bool,
}
impl Settings {
    pub fn resolve(policy: &EffectivePolicy, stage: Stage, executor: &str) -> Result<Self, String> {
        use revust_core::config::LegacyDuration;
        use std::time::Duration;
        let values = policy.execution_values();
        let name = match stage {
            Stage::Find => "find",
            Stage::Synthesis => "synthesis",
            Stage::Verify => "verify",
        };
        let duration = |field| -> Result<Option<Duration>, String> {
            let text = values["stages"][name][field]
                .as_str()
                .ok_or_else(|| format!("missing {name} {field}"))?;
            let nanos = text.parse::<LegacyDuration>()?.0;
            Ok((nanos > 0).then(|| Duration::from_nanos(nanos as u64)))
        };
        let options = values["executors"]
            .get(executor)
            .ok_or_else(|| format!("unknown executor {executor}"))?;
        let program = options["executable"]
            .as_str()
            .ok_or("missing executor program")?
            .to_owned();
        let mut environment = BTreeMap::new();
        for (key, value) in options["env"]
            .as_object()
            .ok_or("missing child environment")?
        {
            environment.insert(
                key.clone(),
                value
                    .as_str()
                    .ok_or("invalid child environment value")?
                    .to_owned(),
            );
        }
        Ok(Self {
            program,
            environment,
            preserve_anthropic_api_key: false,
            limits: Limits {
                idle: duration("idle_timeout")?,
                hard: duration("hard_timeout")?,
            },
        })
    }
    pub fn run(
        &self,
        request: Request,
        control: &RunControl,
        emit: impl FnMut(Stream, &[u8]) -> io::Result<bool>,
    ) -> io::Result<Outcome> {
        self.run_observed(request, control, emit, || Ok(false))
    }
    pub fn run_observed(
        &self,
        mut request: Request,
        control: &RunControl,
        emit: impl FnMut(Stream, &[u8]) -> io::Result<bool>,
        poll: impl FnMut() -> io::Result<bool>,
    ) -> io::Result<Outcome> {
        request.program = self.program.clone();
        supervision::run_with_environment(
            &request,
            &self.environment,
            if self.preserve_anthropic_api_key {
                &["CLAUDECODE"]
            } else {
                &["CLAUDECODE", "ANTHROPIC_API_KEY"]
            },
            Limits {
                idle: self.limits.idle,
                hard: self.limits.hard,
            },
            || control.check_running(),
            emit,
            poll,
        )
    }
}
