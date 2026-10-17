//! Stage result projection, identity ownership and immutable verification patches.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Finding {
    pub id: String,
    pub file: String,
    pub line: i64,
    pub end_line: i64,
    pub severity: String,
    pub confidence: i64,
    pub title: String,
    pub body: String,
    pub fix: String,
    pub sources: Vec<String>,
    pub lenses: Vec<String>,
    pub verdict: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Categories {
    pub findings: Vec<Finding>,
    pub open_questions: Vec<Finding>,
    pub pre_existing: Vec<Finding>,
    pub immaterial: Vec<Finding>,
    pub dropped: Vec<Finding>,
}

fn array<'a>(value: &'a Value, key: &str, required: bool) -> Result<&'a [Value], String> {
    let fields = value.as_object().ok_or("stage result must be an object")?;
    match fields.get(key) {
        Some(Value::Array(items)) => Ok(items),
        Some(Value::Null) => Ok(&[]),
        None if !required => Ok(&[]),
        _ => Err(format!("stage result requires {key} array")),
    }
}
fn field<T: DeserializeOwned + Default>(value: &Value, key: &str) -> Result<T, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(T::default()),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid finding field {key}: {error}")),
    }
}
fn strings(value: &Value, key: &str) -> Result<Vec<String>, String> {
    let items: Vec<Option<String>> = field(value, key)?;
    Ok(items.into_iter().map(Option::unwrap_or_default).collect())
}
fn project(value: &Value) -> Result<Finding, String> {
    if !value.is_null() && !value.is_object() {
        return Err("finding must be an object".into());
    }
    Ok(Finding {
        id: field(value, "id")?,
        file: field(value, "file")?,
        line: field(value, "line")?,
        end_line: field(value, "end_line")?,
        severity: field(value, "severity")?,
        confidence: field(value, "confidence")?,
        title: field(value, "title")?,
        body: field(value, "body")?,
        fix: field(value, "fix")?,
        sources: strings(value, "sources")?,
        lenses: strings(value, "lenses")?,
        verdict: field(value, "verdict")?,
    })
}
pub fn findings(value: &Value, agent: &str, lenses: &[String]) -> Result<Vec<Finding>, String> {
    let mut output = Vec::new();
    for item in array(value, "findings", true)? {
        let mut candidate = project(item)?;
        candidate.id = format!("{agent}-{}", output.len() + 1);
        candidate.sources = vec![agent.to_owned()];
        let assigned = candidate
            .lenses
            .into_iter()
            .filter(|id| lenses.contains(id))
            .collect::<Vec<_>>();
        candidate.lenses = if assigned.is_empty() {
            lenses.to_owned()
        } else {
            assigned
        };
        output.push(candidate);
    }
    Ok(output)
}
struct Ownership<'a> {
    originals: BTreeMap<&'a str, &'a Finding>,
    claimed: BTreeSet<&'a str>,
}
impl<'a> Ownership<'a> {
    fn new(inputs: &'a [Finding]) -> Result<Self, String> {
        let mut originals = BTreeMap::new();
        for finding in inputs {
            if originals.insert(finding.id.as_str(), finding).is_some() {
                return Err(format!("duplicate synthesis input ID {}", finding.id));
            }
        }
        Ok(Self {
            originals,
            claimed: BTreeSet::new(),
        })
    }
    fn consolidate(&mut self, item: &Value) -> Result<Finding, String> {
        let ids: Vec<String> =
            field(item, "merged_ids").map_err(|error| format!("invalid merged IDs: {error}"))?;
        let first = ids.first().ok_or("synthesis merged no input IDs")?;
        let mut candidate = project(item)?;
        candidate.id = first.to_owned();
        candidate.sources.clear();
        candidate.lenses.clear();
        let mut local = BTreeSet::new();
        for id in &ids {
            if !local.insert(id) {
                continue;
            }
            let original = self
                .originals
                .get(id.as_str())
                .ok_or_else(|| format!("synthesis merged unknown ID {id}"))?;
            if !self.claimed.insert(original.id.as_str()) {
                return Err(format!("synthesis reused input ID {id}"));
            }
            for (target, values) in [
                (&mut candidate.sources, &original.sources),
                (&mut candidate.lenses, &original.lenses),
            ] {
                for value in values {
                    if !target.contains(value) {
                        target.push(value.clone());
                    }
                }
            }
        }
        Ok(candidate)
    }
}
pub fn synthesize(value: &Value, inputs: &[Finding]) -> Result<Categories, String> {
    let mut ownership = Ownership::new(inputs)?;
    let mut result = Categories::default();
    for (key, required, target) in [
        ("findings", true, &mut result.findings),
        ("open_questions", false, &mut result.open_questions),
        ("pre_existing", false, &mut result.pre_existing),
    ] {
        for item in array(value, key, required)? {
            target.push(ownership.consolidate(item)?);
        }
    }
    result.dropped = ownership
        .originals
        .into_iter()
        .filter(|(id, _)| !ownership.claimed.contains(id))
        .map(|(_, original)| original.clone())
        .collect();
    Ok(result)
}
fn assessment(item: &Value) -> Result<Finding, String> {
    if !item.is_null() && !item.is_object() {
        return Err("finding must be an object".into());
    }
    // Identity/location ownership comes from input, never these ignored fields.
    let mut fields = item.as_object().cloned().unwrap_or_default();
    for key in ["file", "sources", "lenses"] {
        fields.remove(key);
    }
    project(&Value::Object(fields))
}
impl Finding {
    fn assessed(&self, patch: Option<&Finding>) -> Self {
        let mut result = Self {
            verdict: "unverified".into(),
            ..self.clone()
        };
        let Some(patch) = patch.filter(|patch| {
            matches!(
                patch.verdict.as_str(),
                "confirmed" | "refined" | "rejected" | "immaterial" | "pre_existing"
            )
        }) else {
            return result;
        };
        result.verdict = patch.verdict.clone();
        if patch.verdict != "refined" {
            return result;
        }
        for (target, value) in [
            (&mut result.line, patch.line),
            (&mut result.end_line, patch.end_line),
            (&mut result.confidence, patch.confidence),
        ] {
            if value > 0 {
                *target = value;
            }
        }
        if matches!(patch.severity.as_str(), "critical" | "major" | "minor") {
            result.severity = patch.severity.clone();
        }
        for (target, value) in [
            (&mut result.title, &patch.title),
            (&mut result.body, &patch.body),
            (&mut result.fix, &patch.fix),
        ] {
            if !value.is_empty() {
                target.clone_from(value);
            }
        }
        result
    }
}
pub fn verify(value: &Value, inputs: &[Finding]) -> Result<Vec<Finding>, String> {
    let mut patches = BTreeMap::new();
    for item in array(value, "verdicts", true)? {
        let patch = assessment(item)?;
        patches.insert(patch.id.clone(), patch);
    }
    Ok(inputs
        .iter()
        .map(|original| original.assessed(patches.get(&original.id)))
        .collect())
}
