//! Structured stage context. Provider-authored strings are serialized as data.
use super::{data::Finding, pipeline::Source};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Serialize)]
struct Reviewer<'a> {
    reviewer: &'a str,
    lenses: &'a [String],
    status: &'static str,
    finding_ids: Vec<&'a str>,
}
#[derive(Serialize)]
struct Inventory<'a> {
    expected: usize,
    reported: usize,
    degraded: Vec<&'a str>,
    reviewers: Vec<Reviewer<'a>>,
}
fn json(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|error| error.to_string())
}
pub fn synthesis(sources: &[Source]) -> Result<BTreeMap<String, String>, String> {
    let mut unique_sources = BTreeSet::new();
    let mut unique_ids = BTreeSet::new();
    let mut findings = Vec::new();
    let mut inventory = Inventory {
        expected: sources.len(),
        reported: 0,
        degraded: Vec::new(),
        reviewers: Vec::new(),
    };
    for source in sources {
        let name = source.agent.name.as_str();
        if !unique_sources.insert(name) {
            return Err(format!("duplicate stage source {name}"));
        }
        let mut reviewer = Reviewer {
            reviewer: name,
            lenses: &source.agent.lenses,
            status: "failed",
            finding_ids: Vec::new(),
        };
        match &source.findings {
            None => inventory.degraded.push(name),
            Some(items) => {
                reviewer.status = "delivered";
                inventory.reported += 1;
                for finding in items {
                    if !unique_ids.insert(finding.id.as_str()) {
                        return Err(format!("duplicate stage finding ID {}", finding.id));
                    }
                    reviewer.finding_ids.push(&finding.id);
                    findings.push(finding);
                }
            }
        }
        inventory.reviewers.push(reviewer);
    }
    Ok(BTreeMap::from([
        ("FINDINGS".into(), json(&findings)?),
        ("SOURCES".into(), json(&inventory)?),
    ]))
}
pub fn verification(findings: &[Finding]) -> Result<BTreeMap<String, String>, String> {
    Ok(BTreeMap::from([("FINDINGS".into(), json(&findings)?)]))
}
