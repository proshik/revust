//! Read-only corpus summaries from bounded, contained saved evidence.
use super::{layout::names, reader};
use crate::{report::Report, runtime::data::Finding};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::Path,
};
#[derive(Default, Serialize)]
pub struct Corpus {
    pub tasks: Vec<TaskStats>,
    pub totals: TaskStats,
}
#[derive(Default, Serialize)]
pub struct TaskStats {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub size_mb: f64,
    #[serde(skip)]
    size_bytes: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_run: String,
    pub rounds: i64,
    pub skipped: Vec<String>,
    pub agents: Vec<AgentStats>,
    pub lenses: Vec<LensStats>,
    pub stages: Vec<StageFlow>,
}
#[derive(Default, Serialize)]
pub struct AgentStats {
    pub name: String,
    pub raised: i64,
    pub survived: i64,
    pub corroborated: i64,
    pub degraded_rounds: i64,
    pub retries: i64,
    pub tokens: i64,
}
#[derive(Default, Serialize)]
pub struct LensStats {
    pub name: String,
    pub raised: i64,
    pub ambiguous: i64,
    pub verdicts: BTreeMap<String, i64>,
}
#[derive(Default, Serialize)]
pub struct StageFlow {
    pub name: String,
    #[serde(rename = "in")]
    pub input: i64,
    pub out: i64,
    #[serde(skip_serializing_if = "zero")]
    pub reclassified: i64,
    #[serde(skip_serializing_if = "zero")]
    pub refined: i64,
}
fn zero(value: &i64) -> bool {
    *value == 0
}

pub fn collect(root: &Path, selected: &str) -> Result<Corpus, String> {
    let mut ids = crate::config::catalog::task_ids(root)?;
    if !selected.is_empty() {
        if !ids.iter().any(|id| id == selected) {
            return Err(format!(
                "task {selected} not found under {}",
                root.display()
            ));
        }
        ids.retain(|id| id == selected);
    }
    let mut result = Corpus::default();
    if ids.is_empty() {
        return Ok(result);
    }
    let root = reader::open_root(root)?;
    for id in ids {
        let task = super::contained_directory(&root, Path::new(&id))?;
        let mut summary = TaskStats {
            id,
            description: description(&task),
            ..Default::default()
        };
        let mut newest = None;
        for name in names(&task)? {
            let Ok(round) = super::open_directory(&task, Path::new(&name)) else {
                continue;
            };
            match round_summary(&round) {
                Ok((round_stats, finished)) => {
                    merge(&mut summary, &round_stats)?;
                    if let Some((time, text)) = finished
                        && newest.as_ref().is_none_or(|(previous, _)| time > *previous)
                    {
                        newest = Some((time, text));
                    }
                }
                Err(error) => summary
                    .skipped
                    .push(format!("round {}: {error}", name.to_string_lossy())),
            }
        }
        summary.last_run = newest
            .map(|(_, text)| text.chars().take(10).collect())
            .unwrap_or_default();
        let (size, warnings) = owned_size(&task);
        summary.size_bytes = size;
        summary.size_mb = megabytes(size);
        summary.skipped.extend(warnings);
        merge(&mut result.totals, &summary)?;
        result.tasks.push(summary);
    }
    Ok(result)
}

fn description(task: &File) -> String {
    #[derive(Default, Deserialize)]
    struct Metadata {
        description: Option<String>,
    }
    let parse = || -> Option<String> {
        let raw = reader::optional(task, "task.md").ok()??;
        let text = std::str::from_utf8(&raw).ok()?;
        let parts = crate::prompts::template::split(text).ok()?;
        crate::prompts::decode::<Metadata>(&parts.meta)
            .ok()?
            .description
    };
    parse().unwrap_or_default()
}

fn merge_rows<T: Default>(
    dest: &mut Vec<T>,
    incoming: &[T],
    key: impl Fn(&T) -> &str,
    add: impl Fn(&mut T, &T),
) {
    let mut indexed: BTreeMap<String, T> = std::mem::take(dest)
        .into_iter()
        .map(|row| (key(&row).to_owned(), row))
        .collect();
    for row in incoming {
        add(indexed.entry(key(row).to_owned()).or_default(), row);
    }
    *dest = indexed.into_values().collect();
}

fn merge(dest: &mut TaskStats, source: &TaskStats) -> Result<(), String> {
    for incoming in &source.agents {
        let existing = dest
            .agents
            .iter()
            .find(|row| row.name == incoming.name)
            .map_or(0, |row| row.tokens);
        existing
            .checked_add(incoming.tokens)
            .ok_or("recorded token total exceeds supported range")?;
    }
    dest.rounds += source.rounds;
    dest.size_bytes = dest.size_bytes.saturating_add(source.size_bytes);
    dest.size_mb = megabytes(dest.size_bytes);
    if source.last_run > dest.last_run {
        dest.last_run.clone_from(&source.last_run);
    }
    dest.skipped.extend(source.skipped.iter().cloned());
    merge_rows(
        &mut dest.agents,
        &source.agents,
        |row| &row.name,
        |a, b| {
            a.name.clone_from(&b.name);
            a.raised += b.raised;
            a.survived += b.survived;
            a.corroborated += b.corroborated;
            a.degraded_rounds += b.degraded_rounds;
            a.retries += b.retries;
            a.tokens += b.tokens;
        },
    );
    merge_rows(
        &mut dest.lenses,
        &source.lenses,
        |row| &row.name,
        |a, b| {
            a.name.clone_from(&b.name);
            a.raised += b.raised;
            a.ambiguous += b.ambiguous;
            for (verdict, count) in &b.verdicts {
                *a.verdicts.entry(verdict.clone()).or_default() += count;
            }
        },
    );
    // Stage order is pipeline order, not lexical order.
    for row in &source.stages {
        if let Some(existing) = dest.stages.iter_mut().find(|stage| stage.name == row.name) {
            existing.input += row.input;
            existing.out += row.out;
            existing.reclassified += row.reclassified;
            existing.refined += row.refined;
        } else {
            dest.stages.push(StageFlow {
                name: row.name.clone(),
                input: row.input,
                out: row.out,
                reclassified: row.reclassified,
                refined: row.refined,
            });
        }
    }
    Ok(())
}

fn records(report: &Report) -> impl Iterator<Item = &Finding> {
    [
        &report.findings,
        &report.open_questions,
        &report.pre_existing,
        &report.immaterial,
    ]
    .into_iter()
    .flatten()
}
fn unique(values: &[String]) -> BTreeSet<&str> {
    values.iter().map(String::as_str).collect()
}

type Finished = Option<(chrono::DateTime<chrono::Utc>, String)>;
fn round_summary(round: &File) -> Result<(TaskStats, Finished), String> {
    use nix::fcntl::{Flock, FlockArg};
    let marker = super::open_beneath(round, Path::new("manifest.json"), false)
        .map_err(|_| "incomplete: no readable completion marker".to_owned())?;
    let _guard = Flock::lock(marker, FlockArg::LockSharedNonblock)
        .map_err(|_| "active: held by a running review".to_owned())?;
    let raw = reader::read(round, "manifest.json")?;
    if raw.is_empty() {
        return Err("incomplete: empty completion marker".into());
    }
    let manifest: reader::Manifest = reader::decode(&raw)?;
    let finished = if reader::completed(&manifest.finished_at)? {
        Some((
            crate::report::parsed_time(&manifest.finished_at)?,
            manifest.finished_at,
        ))
    } else {
        None
    };
    let mut sequence = Vec::new();
    for (stage, filename) in [
        ("find", "stages/1-found.json"),
        ("synthesis", "stages/2-synthesized.json"),
        ("verify", "stages/3-verified.json"),
        ("report", "findings.json"),
    ] {
        if let Some(bytes) = reader::optional(round, filename)? {
            sequence.push((stage, Report::decode(&String::from_utf8_lossy(&bytes))?));
        }
    }
    let Some(("find", found)) = sequence.first() else {
        return Err("incomplete: no stages/1-found.json".into());
    };
    let final_stage = &sequence
        .iter()
        .rev()
        .find(|(stage, _)| *stage != "report")
        .unwrap()
        .1;
    let mut agents: BTreeMap<String, AgentStats> = BTreeMap::new();
    let mut lenses: BTreeMap<String, LensStats> = BTreeMap::new();
    let mut assignments = BTreeMap::new();
    for source in &found.sources.agents {
        let row = agents.entry(source.name.clone()).or_default();
        if source.tokens < 0 {
            return Err("negative recorded token observation".into());
        }
        row.tokens = row
            .tokens
            .checked_add(source.tokens)
            .ok_or("recorded source token total exceeds supported range")?;
        let assigned = source.lenses.clone().unwrap_or_default();
        for lens in &assigned {
            lenses.entry(lens.clone()).or_default();
        }
        assignments.insert(source.name.as_str(), assigned);
    }
    for source in found.sources.degraded_names() {
        agents.entry(source).or_default().degraded_rounds = 1;
    }
    for item in records(found) {
        for source in unique(&item.sources) {
            agents.entry(source.into()).or_default().raised += 1;
        }
        let item_lenses = unique(&item.lenses);
        let ambiguous = item
            .sources
            .iter()
            .filter_map(|source| assignments.get(source.as_str()))
            .any(|assigned| {
                let assigned = unique(assigned);
                assigned.len() > 1 && assigned == item_lenses
            });
        for lens in item_lenses {
            let row = lenses.entry(lens.into()).or_default();
            row.raised += 1;
            row.ambiguous += i64::from(ambiguous);
        }
    }
    for item in records(final_stage) {
        let sources = unique(&item.sources);
        for source in &sources {
            let row = agents.entry((*source).into()).or_default();
            row.survived += 1;
            row.corroborated += i64::from(sources.len() > 1);
        }
        let verdict = if item.verdict.is_empty() {
            "unverified"
        } else {
            &item.verdict
        };
        for lens in unique(&item.lenses) {
            *lenses
                .entry(lens.into())
                .or_default()
                .verdicts
                .entry(verdict.into())
                .or_default() += 1;
        }
    }
    let mut result = TaskStats {
        rounds: 1,
        ..Default::default()
    };
    if finished.is_none() || sequence.last().is_none_or(|(stage, _)| *stage != "report") {
        result
            .skipped
            .push("partial stage evidence: completion time or final report is unavailable".into());
    }
    result
        .skipped
        .push("token totals include recorded usage only; zero may mean unavailable".into());
    for (name, mut row) in agents {
        row.name = name;
        result.agents.push(row);
    }
    for (name, mut row) in lenses {
        row.name = name;
        result.lenses.push(row);
    }
    for adjacent in sequence.windows(2) {
        let (before, (stage, after)) = (&adjacent[0].1, &adjacent[1]);
        let classified =
            |report: &Report| (report.pre_existing.len() + report.immaterial.len()) as i64;
        let refined = |report: &Report| {
            records(report)
                .filter(|item| item.verdict == "refined")
                .count() as i64
        };
        result.stages.push(StageFlow {
            name: (*stage).into(),
            input: records(before).count() as i64,
            out: records(after).count() as i64,
            reclassified: (classified(after) - classified(before)).max(0),
            refined: (refined(after) - refined(before)).max(0),
        });
    }
    count_retries(round, &mut result)?;
    Ok((result, finished))
}

fn count_retries(round: &File, result: &mut TaskStats) -> Result<(), String> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Retry {
        kind: String,
        agent: String,
    }
    let Some(bytes) = reader::optional(round, "events.jsonl")? else {
        result
            .skipped
            .push("retry observations unavailable: no events.jsonl".into());
        return Ok(());
    };
    for (index, line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        if line.len() >= 4 << 20 {
            return Err("event exceeds size limit".into());
        }
        match reader::decode::<Retry>(line) {
            Ok(event) if event.kind == "agent_retried" => {
                if let Some(source) = result.agents.iter_mut().find(|row| row.name == event.agent) {
                    source.retries += 1;
                }
            }
            Ok(_) => (),
            Err(_) => result.skipped.push(format!(
                "unreadable event record {}; retry counts may be partial",
                index + 1
            )),
        }
    }
    Ok(())
}

pub(super) fn megabytes(bytes: i64) -> f64 {
    (bytes as f64 / 1_048_576.0 * 10.0).round() / 10.0
}

pub(super) fn owned_size(root: &File) -> (i64, Vec<String>) {
    fn walk(
        root: &File,
        relative: &Path,
        bytes: &mut i64,
        warnings: &mut Vec<String>,
    ) -> Result<(), String> {
        use nix::{fcntl::AtFlags, sys::stat::fstatat};
        for name in names(root)? {
            let path = relative.join(&name);
            let stat = fstatat(root, Path::new(&name), AtFlags::AT_SYMLINK_NOFOLLOW)
                .map_err(super::error)?;
            match stat.st_mode & nix::libc::S_IFMT {
                nix::libc::S_IFREG => *bytes = bytes.saturating_add(stat.st_size.max(0)),
                nix::libc::S_IFDIR => {
                    match super::open_directory(root, Path::new(&name))
                        .and_then(|child| walk(&child, &path, bytes, warnings))
                    {
                        Ok(()) => (),
                        Err(error) => {
                            warnings.push(format!("{}: {error}; size is a floor", path.display()))
                        }
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }
    let (mut bytes, mut warnings) = (0, Vec::new());
    if let Err(error) = walk(root, Path::new(""), &mut bytes, &mut warnings) {
        warnings.push(format!("{error}; size is a floor"));
    }
    (bytes, warnings)
}
