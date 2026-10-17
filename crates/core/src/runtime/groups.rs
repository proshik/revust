//! Deterministic verification batches with complete bucket ownership and safe labels.
use super::data::Finding;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Debug, Default)]
pub struct Group {
    pub name: String,
    pub shown: String,
    pub keys: Vec<String>,
    pub findings: Vec<Finding>,
}
fn joined(groups: impl IntoIterator<Item = Group>) -> Group {
    let mut result = Group::default();
    for group in groups {
        result.keys.extend(group.keys);
        result.findings.extend(group.findings);
    }
    result
}
fn safe(text: &str) -> String {
    let words = text
        .split(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '_' | '.' | '-'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let label = words.join("-");
    let label = label.trim_matches(['.', '-']);
    if label.is_empty() {
        "root".into()
    } else {
        label.to_owned()
    }
}
fn label(keys: &[String]) -> String {
    let mut text = keys
        .iter()
        .take(3)
        .map(|key| safe(key))
        .collect::<Vec<_>>()
        .join("+");
    if keys.len() > 3 {
        text.push_str(&format!("+{}-more", keys.len() - 3));
    }
    if text.is_empty() { "root".into() } else { text }
}
fn distinct(base: String, used: &mut BTreeSet<String>) -> String {
    (1..)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|name| used.insert(name.clone()))
        .unwrap()
}
fn parent(file: &str) -> String {
    let Some((directory, _)) = file.rsplit_once('/') else {
        return ".".into();
    };
    let absolute = file.starts_with('/');
    let mut pieces = Vec::new();
    for part in directory
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
    {
        if part == ".." && pieces.last().is_some_and(|last| *last != "..") {
            pieces.pop();
        } else if part != ".." || !absolute {
            pieces.push(part);
        }
    }
    let normalized = pieces.join("/");
    match (absolute, normalized.is_empty()) {
        (true, _) => format!("/{normalized}"),
        (false, true) => ".".into(),
        _ => normalized,
    }
}
pub fn group(
    inputs: &[Finding],
    by_source: bool,
    min_directory_size: u64,
    limit: Option<usize>,
) -> Vec<Group> {
    let mut buckets = BTreeMap::<String, Vec<Finding>>::new();
    for finding in inputs {
        let key = if by_source {
            finding
                .sources
                .first()
                .cloned()
                .unwrap_or_else(|| ".".into())
        } else {
            parent(&finding.file)
        };
        buckets.entry(key).or_default().push(finding.clone());
    }
    let (mut full, small): (Vec<_>, Vec<_>) = buckets
        .into_iter()
        .map(|(key, findings)| Group {
            keys: vec![key],
            findings,
            ..Default::default()
        })
        .partition(|group| by_source || group.findings.len() as u64 >= min_directory_size);
    if !small.is_empty() {
        full.push(joined(small));
    }
    if let Some(cap) = limit.filter(|cap| *cap > 0 && full.len() > *cap) {
        full.sort_by_key(|group| group.findings.len());
        let remaining = full.split_off(full.len() - cap + 1);
        let mut capped = vec![joined(full)];
        capped.extend(remaining);
        full = capped;
        full.sort_by_cached_key(|group| label(&group.keys));
    }
    let mut names = BTreeSet::new();
    let mut displayed = BTreeSet::new();
    for batch in &mut full {
        batch.name = distinct(label(&batch.keys), &mut names);
        let short = batch
            .keys
            .first()
            .map(|key| key.trim_matches('/').rsplit('/').next().unwrap_or_default())
            .unwrap_or("");
        batch.shown = distinct(safe(short), &mut displayed);
    }
    full
}
