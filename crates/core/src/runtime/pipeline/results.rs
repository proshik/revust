//! Bridge attempt evidence to owned stage conclusions without mutating earlier data.
use super::{Categories, Finding, Report, Source};
use crate::{
    prompts::Agent,
    runtime::{
        Stage,
        retry::Failure,
        worker::{AttemptFailure, Outcome, UnitResult},
    },
};
use serde_json::Value;
use std::collections::BTreeMap;
pub(super) fn output(unit: &UnitResult<Categories>) -> Option<&Categories> {
    unit.attempts
        .iter()
        .rev()
        .find_map(|attempt| attempt.result.as_ref().ok())
}
pub(super) fn collect_sources(
    agents: &[Agent],
    units: Vec<UnitResult<Categories>>,
    report: &mut Report,
) -> Vec<Source> {
    agents
        .iter()
        .zip(units)
        .map(|(agent, unit)| {
            let delivered = output(&unit);
            let successful = unit.outcome == Outcome::Success;
            let source = Source {
                agent: agent.clone(),
                findings: if successful {
                    delivered.map(|data| data.findings.clone())
                } else {
                    None
                },
            };
            if successful {
                report.successful_reviewers.push(agent.name.clone());
            }
            if let Some(data) = delivered {
                report.categories.findings.extend_from_slice(&data.findings);
            }
            report.accept_unit(unit);
            source
        })
        .collect()
}
pub(super) fn decode(
    stage: Stage,
    value: &Value,
    unit: &str,
    agent: Option<&Agent>,
    inputs: &[Finding],
) -> Result<Categories, AttemptFailure> {
    use crate::runtime::data;
    let result =
        match stage {
            Stage::Synthesis => data::synthesize(value, inputs),
            Stage::Find => data::findings(value, unit, &agent.expect("finder identity").lenses)
                .map(|findings| Categories {
                    findings,
                    ..Default::default()
                }),
            Stage::Verify => data::verify(value, inputs).map(|findings| Categories {
                findings,
                ..Default::default()
            }),
        };
    result.map_err(|diagnostic| AttemptFailure {
        kind: Failure::InvalidOutput,
        diagnostic,
    })
}
impl Report {
    pub(super) fn apply_verdicts(&mut self) {
        let mut patches = BTreeMap::new();
        for assessment in &self.verification_evidence {
            patches.entry(assessment.id.as_str()).or_insert(assessment);
        }
        let replace = |finding: &mut Finding| {
            if let Some(assessment) = patches.get(finding.id.as_str()) {
                finding.clone_from(assessment);
            }
        };
        self.categories
            .open_questions
            .iter_mut()
            .chain(self.categories.pre_existing.iter_mut())
            .for_each(replace);
        let assessed = std::mem::take(&mut self.categories.findings)
            .into_iter()
            .map(|mut finding| {
                replace(&mut finding);
                finding
            });
        for finding in assessed {
            match finding.verdict.as_str() {
                "rejected" => (),
                "immaterial" => self.categories.immaterial.push(finding),
                "pre_existing" => self.categories.pre_existing.push(finding),
                _ => self.categories.findings.push(finding),
            }
        }
    }
}
