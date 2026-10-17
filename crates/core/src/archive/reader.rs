//! Read-only completed archive loading. It owns no execution or repair capability.
use crate::{
    report::{ExecutionOutcome, Report, SourceStat, StageRun},
    runtime::data::Finding,
};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, path::Path};
const FILE_LIMIT: u64 = 64 << 20;
const EVENT_LIMIT: usize = 4 << 20;
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Agent {
    pub name: String,
    pub lenses: Vec<String>,
    pub executor: String,
    pub model: String,
    pub effort: String,
    pub color: String,
    pub color_name: String,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub outcome: Option<ExecutionOutcome>,
    pub roster: Vec<Agent>,
    pub task: String,
    pub run: String,
    pub profile: String,
    pub scope_path: String,
    #[serde(default = "zero_time")]
    pub started_at: String,
    #[serde(default = "zero_time")]
    pub finished_at: String,
    pub duration_ms: i64,
    pub tokens: i64,
    pub agents: Vec<SourceStat>,
    pub degraded: Vec<String>,
    pub prompts: Vec<Prompt>,
    pub stages: Vec<StageRun>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Prompt {
    pub path: String,
    pub layer: String,
    pub source: String,
    pub hash: String,
}
#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Event {
    pub kind: String,
    pub agent: String,
    pub stage: String,
    pub text: String,
    pub findings: Vec<Finding>,
    #[serde(default = "zero_time")]
    pub at: String,
}
pub struct ArchivedRun {
    pub manifest: Manifest,
    pub report: Report,
    pub roster: Vec<Agent>,
    pub events: Vec<Event>,
}
pub fn load(path: &Path) -> Result<ArchivedRun, String> {
    let root = open_root(path)?;
    // Keep a shared claim for the entire observation, using the actual marker
    // descriptor rather than reopening its path after checking ownership.
    let marker = marker(&root)?;
    let raw = read_file(
        marker.try_clone().map_err(|e| e.to_string())?,
        "manifest.json",
    )?;
    let mut report = Report::decode(&String::from_utf8_lossy(&read(&root, "findings.json")?))
        .map_err(|e| format!("archive findings.json: {e}"))?;
    let manifest: Manifest = if raw.is_empty() {
        legacy_terminal(&root, &mut report)?
    } else {
        decode(&raw).map_err(|e| format!("archive manifest.json: {e}"))?
    };
    crate::report::parsed_time(&manifest.started_at)?;
    if !completed(&manifest.finished_at)? {
        return Err("archive completion timestamp is missing".into());
    }
    if manifest.outcome != report.outcome {
        return Err("archive manifest and report outcomes do not match".into());
    }
    match (manifest.task.as_str(), manifest.run.as_str()) {
        ("", _) | (_, "") => return Err("archive identity is incomplete".into()),
        (task, run) if task != report.scope.task || run != report.scope.run => {
            return Err("archive manifest and report identities do not match".into());
        }
        _ => (),
    }
    let roster = restored_roster(&manifest);
    let mut events = Vec::new();
    for (index, bytes) in read(&root, "events.jsonl")?
        .split_inclusive(|byte| *byte == b'\n')
        .enumerate()
    {
        if bytes.len() >= EVENT_LIMIT {
            return Err(format!("archive event {} exceeds size limit", index + 1));
        }
        let event: Event =
            decode(bytes).map_err(|e| format!("archive event {}: {e}", index + 1))?;
        if event.kind.is_empty() || !completed(&event.at)? {
            return Err(format!("archive event {} is incomplete", index + 1));
        }
        events.push(event);
    }
    Ok(ArchivedRun {
        roster,
        report,
        manifest,
        events,
    })
}

fn marker(root: &File) -> Result<nix::fcntl::Flock<File>, String> {
    use nix::{
        fcntl::{AtFlags, Flock, FlockArg, OFlag, openat},
        sys::stat::{Mode, fstat, fstatat},
    };
    let file = File::from(
        openat(
            root,
            "manifest.json",
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK,
            Mode::empty(),
        )
        .map_err(|e| format!("archive completion marker: {e}"))?,
    );
    let guard = Flock::lock(file, FlockArg::LockSharedNonblock)
        .map_err(|_| "archive is active or its ownership cannot be established".to_owned())?;
    let opened = fstat(&*guard).map_err(|e| e.to_string())?;
    let entry =
        fstatat(root, "manifest.json", AtFlags::AT_SYMLINK_NOFOLLOW).map_err(|e| e.to_string())?;
    if opened.st_mode & nix::libc::S_IFMT != nix::libc::S_IFREG
        || opened.st_nlink != 1
        || opened.st_dev != entry.st_dev
        || opened.st_ino != entry.st_ino
    {
        return Err("archive completion marker is not an owned regular file".into());
    }
    Ok(guard)
}

fn legacy_terminal(root: &File, report: &mut Report) -> Result<Manifest, String> {
    use crate::runtime::{journal, pipeline::State};
    // New explicit-outcome reports require their own final marker. The fallback
    // is only for the historical writer that omitted failed/stopped markers.
    if report.outcome.is_some() {
        return Err("archive explicit outcome is missing its completion marker".into());
    }
    if read(root, "report.md")?.is_empty() {
        return Err("archive legacy saved report is empty".into());
    }
    if !completed(&report.stats.started_at)? || !completed(&report.stats.finished_at)? {
        return Err("archive legacy report completion timestamp is missing".into());
    }
    if crate::report::parsed_time(&report.stats.finished_at)?
        < crate::report::parsed_time(&report.stats.started_at)?
    {
        return Err("archive legacy report timestamps are inconsistent".into());
    }
    let snapshot = journal::decode_snapshot(
        Path::new("revust"),
        &read(root, "revust/run.json")?,
        &read(root, "revust/events.jsonl")?,
    )
    .map_err(|e| format!("archive legacy journal: {e}"))?;
    let runtime = snapshot
        .report
        .ok_or("archive legacy journal is truncated or not terminal")?;
    if snapshot.truncated || !matches!(runtime.state, State::Failed | State::Stopped(_)) {
        return Err("archive empty marker has no verified failed/stopped terminal outcome".into());
    }
    if snapshot.identity.task != report.scope.task || snapshot.identity.run != report.scope.run {
        return Err("archive journal and report identities do not match".into());
    }
    let outcome = ExecutionOutcome::from_runtime(&runtime);
    report.outcome = Some(outcome);
    Ok(Manifest {
        outcome: report.outcome.clone(),
        task: report.scope.task.clone(),
        run: report.scope.run.clone(),
        scope_path: report.scope.scope_path.clone(),
        started_at: report.stats.started_at.clone(),
        finished_at: report.stats.finished_at.clone(),
        duration_ms: report.stats.duration_ms,
        tokens: report.stats.tokens,
        agents: report.sources.agents.clone(),
        degraded: report.sources.degraded.clone(),
        stages: report.stats.stages.clone(),
        ..Default::default()
    })
}

fn restored_roster(manifest: &Manifest) -> Vec<Agent> {
    if !manifest.roster.is_empty() {
        return manifest.roster.clone();
    }
    manifest
        .agents
        .iter()
        .map(|source| Agent {
            name: source.name.clone(),
            lenses: source.lenses.clone().unwrap_or_default(),
            executor: source.executor.clone(),
            // Requested settings are not observations of the model that ran.
            model: source.actual_model.clone(),
            effort: source.effort.clone(),
            color: crate::prompts::colors::derived(&source.name),
            ..Agent::default()
        })
        .collect()
}

pub(super) fn open_root(path: &Path) -> Result<File, String> {
    use nix::{
        fcntl::{OFlag, open},
        sys::stat::Mode,
    };
    open(
        path,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|e| format!("open archive {}: {e}", path.display()))
}

pub(super) fn read(root: &File, name: &str) -> Result<Vec<u8>, String> {
    let file = super::open_beneath(root, Path::new(name), false)
        .map_err(|e| format!("read archive {name}: {e}"))?;
    read_file(file, name)
}

fn read_file(file: File, name: &str) -> Result<Vec<u8>, String> {
    let metadata = file
        .metadata()
        .map_err(|e| format!("stat archive {name}: {e}"))?;
    if !metadata.is_file() {
        return Err(format!("archive {name}: expected regular file"));
    }
    let mut bytes = Vec::new();
    if metadata.len() <= FILE_LIMIT {
        file.take(FILE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("read archive {name}: {e}"))?;
        if bytes.len() as u64 <= FILE_LIMIT {
            return Ok(bytes);
        }
    }
    Err(format!("archive {name} exceeds size limit"))
}

pub(super) fn optional(root: &File, name: &str) -> Result<Option<Vec<u8>>, String> {
    use nix::{errno::Errno, fcntl::AtFlags, sys::stat::fstatat};
    match fstatat(root, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Err(Errno::ENOENT) => Ok(None),
        // Reads always use the contained resolver, including for existing aliases.
        _ => read(root, name).map(Some),
    }
}

pub(super) fn completed(timestamp: &str) -> Result<bool, String> {
    Ok(crate::report::parsed_time(timestamp)? != crate::report::parsed_time(&zero_time())?)
}

pub(super) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let mut document = serde_json::from_slice(bytes).map_err(|e| format!("saved JSON: {e}"))?;
    crate::report::saved::project(&mut document);
    serde_json::from_value(document).map_err(|e| format!("saved fields: {e}"))
}

fn zero_time() -> String {
    "0001-01-01T00:00:00Z".into()
}
