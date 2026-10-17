//! Versioned sidecar journal. Creating a run is exclusive; inspection never resumes it.
use super::{
    Attempt,
    data::Categories,
    pipeline::Report,
    worker::{AttemptFailure, Event},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub task: String,
    pub run: String,
    pub request: String,
}
pub enum Artifact {
    Request,
    Delivery,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stream {
    Stdout,
    Stderr,
    ProviderEvents,
}
pub struct AttemptRecord {
    pub id: Attempt,
    pub directory: PathBuf,
    pub result: Option<Result<Categories, AttemptFailure>>,
}
pub struct Snapshot {
    pub identity: Identity,
    pub attempts: Vec<AttemptRecord>,
    pub report: Option<Report>,
    pub truncated: bool,
}
#[derive(Serialize, Deserialize)]
struct Header {
    version: u32,
    identity: Identity,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Record<R> {
    Started {
        attempt: Attempt,
    },
    Finished {
        attempt: Attempt,
        result: Result<Categories, AttemptFailure>,
    },
    Terminal {
        report: R,
    },
}
struct LiveAttempt {
    sequence: usize,
    finished: bool,
    directory: Option<std::fs::File>,
    streams: std::collections::BTreeMap<Stream, std::fs::File>,
    legacy_stdout: Option<std::fs::File>,
}
mod evidence;
mod profile;
mod prompts;
struct State {
    observers: Vec<std::sync::mpsc::Sender<super::events::Event>>,
    presentation_started: bool,
    legacy_events: Option<std::fs::File>,
    prompts: prompts::Storage,
    file: std::fs::File,
    stages_directory: Option<std::fs::File>,
    agents_directory: Option<std::fs::File>,
    attempts: std::collections::BTreeMap<Attempt, LiveAttempt>,
    sealed: bool,
    failed: bool,
}
pub struct Journal {
    root: PathBuf,
    directory: std::fs::File,
    attempts_directory: std::fs::File,
    round_directory: std::fs::File,
    state: std::sync::Mutex<State>,
    executing: std::sync::atomic::AtomicBool,
}
impl Journal {
    /// Observe a new run without owning execution or its budget. Subscribe before
    /// the first event; completed/ongoing archives use the read-only replay path.
    /// Delivery is ordered after durable writes. Consumers must drain the queue;
    /// a disconnected consumer does not block or cancel provider execution.
    pub fn subscribe(&self) -> Result<std::sync::mpsc::Receiver<super::events::Event>, String> {
        let mut state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        if state.presentation_started {
            return Err("live observation must subscribe before the first event".into());
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        state.observers.push(sender);
        Ok(receiver)
    }
    /// Persist presentation events separately from the versioned attempt journal.
    pub fn publish(&self, event: super::events::Event) -> Result<(), String> {
        use std::io::Write;
        let mut state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        let event = event.stamped();
        let result = (|| {
            let bytes = event.clone().json()?;
            let file = state.legacy_events.as_mut().ok_or("event stream closed")?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            state.fail();
            return Err(format!("persist presentation event: {error}"));
        }
        state.presentation_started = true;
        state
            .observers
            .retain(|sender| sender.send(event.clone()).is_ok());
        Ok(())
    }
    /// Legacy stage artifacts use a cached directory descriptor, never a
    /// reopened round path. Each snapshot is exclusive and a failure is sticky.
    pub fn save_stage(&self, stage: super::Stage, bytes: &[u8]) -> Result<(), String> {
        use nix::{
            fcntl::{OFlag, openat},
            sys::stat::{Mode, mkdirat},
        };
        use std::io::Write;
        let mut state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        let result: std::io::Result<()> = (|| {
            if state.stages_directory.is_none() {
                match mkdirat(
                    &self.round_directory,
                    "stages",
                    Mode::from_bits_truncate(0o750),
                ) {
                    Ok(()) => self.round_directory.sync_all()?,
                    Err(nix::errno::Errno::EEXIST) => (),
                    Err(e) => return Err(e.into()),
                }
                state.stages_directory = Some(std::fs::File::from(openat(
                    &self.round_directory,
                    "stages",
                    OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                    Mode::empty(),
                )?));
            }
            let directory = state.stages_directory.as_ref().unwrap();
            let name = evidence::checkpoint(stage);
            let mut file = private_file(directory, name)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            directory.sync_all()
        })();
        if let Err(e) = result {
            state.fail();
            return Err(format!("persist {stage:?} stage snapshot: {e}"));
        }
        Ok(())
    }
    /// Paths come from durable numeric sequences, never provider-authored unit names.
    pub fn attempt_directory(&self, id: &Attempt) -> Result<PathBuf, String> {
        let state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        let attempt = state.attempts.get(id).ok_or("attempt was not started")?;
        if attempt.finished {
            return Err("attempt already finished".into());
        }
        Ok(self
            .root
            .join("attempts")
            .join(attempt.sequence.to_string()))
    }
    pub fn round(&self) -> &Path {
        self.root.parent().expect("journal has round parent")
    }
    /// Frozen calibration is a legacy artifact, owned by the same round handle.
    pub fn save_calibration(&self, bytes: &[u8]) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        if !state.attempts.is_empty() {
            return Err("calibration must precede dispatch".into());
        }
        let result = state.prompts.calibration(&self.round_directory, bytes);
        if let Err(error) = result {
            state.fail();
            return Err(format!("persist calibration: {error}"));
        }
        Ok(())
    }
    /// Legacy prompts are written once; identical retry requests reuse their artifact.
    pub fn save_prompt(&self, stage: super::Stage, unit: &str, bytes: &[u8]) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        if let Err(error) = state
            .prompts
            .save(&self.round_directory, stage, unit, bytes)
        {
            state.fail();
            return Err(format!("persist composed prompt: {error}"));
        }
        Ok(())
    }
    pub fn save(&self, id: &Attempt, artifact: Artifact, bytes: &[u8]) -> Result<(), String> {
        use std::io::Write;
        let mut state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        let attempt = state.attempts.get(id).ok_or("attempt was not started")?;
        if attempt.finished {
            return Err("attempt already finished".into());
        }
        let directory = attempt.directory.as_ref().expect("live attempt directory");
        let name = match artifact {
            Artifact::Request => "request.json",
            Artifact::Delivery => "delivery.json",
        };
        let result = (|| {
            let mut file = private_file(directory, name)?;
            file.write_all(bytes)?;
            file.sync_all()
        })()
        .and_then(|_| directory.sync_all());
        if let Err(error) = result {
            state.fail();
            return Err(format!("persist attempt artifact: {error}"));
        }
        Ok(())
    }
    /// Existing sidecars, even interrupted ones, are never opened for execution.
    pub fn create(round: &Path, identity: Identity) -> Result<Self, String> {
        use nix::{
            fcntl::{OFlag, open},
            sys::stat::Mode,
        };
        let directory = std::fs::File::from(
            open(
                round,
                OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| e.to_string())?,
        );
        Self::create_in(round, &directory, identity)
    }
    pub(crate) fn create_in(
        round: &Path,
        parent: &std::fs::File,
        identity: Identity,
    ) -> Result<Self, String> {
        if [&identity.task, &identity.run, &identity.request]
            .iter()
            .any(|s| s.is_empty())
        {
            return Err("task, run and request identities must be nonempty".into());
        }
        let directory = private_directory(parent, "revust")
            .map_err(|e| format!("claim run (existing runs are observation-only): {e}"))?;
        let attempts_directory =
            private_directory(&directory, "attempts").map_err(|e| e.to_string())?;
        let mut header = private_file(&directory, "run.json").map_err(|e| e.to_string())?;
        use std::io::Write;
        header
            .write_all(
                &serde_json::to_vec(&Header {
                    version: 1,
                    identity,
                })
                .map_err(|e| e.to_string())?,
            )
            .and_then(|_| header.sync_all())
            .map_err(|e| e.to_string())?;
        let file = private_file(&directory, "events.jsonl").map_err(|e| e.to_string())?;
        let legacy_events = private_file(parent, "events.jsonl")
            .map_err(|e| format!("create presentation events: {e}"))?;
        legacy_events
            .sync_all()
            .and_then(|_| parent.sync_all())
            .map_err(|e| e.to_string())?;
        file.sync_all()
            .and_then(|_| directory.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(Self {
            root: round.join("revust"),
            directory,
            attempts_directory,
            round_directory: parent.try_clone().map_err(|e| e.to_string())?,
            executing: std::sync::atomic::AtomicBool::new(false),
            state: std::sync::Mutex::new(State {
                observers: vec![],
                presentation_started: false,
                legacy_events: Some(legacy_events),
                prompts: Default::default(),
                file,
                stages_directory: None,
                agents_directory: None,
                attempts: Default::default(),
                sealed: false,
                failed: false,
            }),
        })
    }
    pub fn record(&self, event: Event<'_, Categories>) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        match event {
            Event::Retrying(_) => (), // Presentation decision; attempts keep the version-1 shape.
            Event::Started(id) => {
                if state.attempts.contains_key(id) {
                    return Err("attempt already recorded".into());
                }
                let sequence = state.attempts.len();
                let directory =
                    match private_directory(&self.attempts_directory, &sequence.to_string()) {
                        Ok(directory) => directory,
                        Err(error) => {
                            state.fail();
                            return Err(error.to_string());
                        }
                    };
                state.persist(&Record::<&Report>::Started {
                    attempt: id.clone(),
                })?;
                state.attempts.insert(
                    id.clone(),
                    LiveAttempt {
                        sequence,
                        finished: false,
                        directory: Some(directory),
                        streams: Default::default(),
                        legacy_stdout: None,
                    },
                );
            }
            Event::Finished(done) => {
                let attempt = state
                    .attempts
                    .get(&done.id)
                    .ok_or("attempt was not started")?;
                if attempt.finished {
                    return Err("attempt already finished".into());
                }
                state.persist(&Record::<&Report>::Finished {
                    attempt: done.id.clone(),
                    result: done.result.clone(),
                })?;
                let attempt = state.attempts.get_mut(&done.id).expect("recorded attempt");
                attempt.finished = true;
                attempt.streams.clear();
                attempt.legacy_stdout.take();
                attempt.directory.take();
            }
        }
        Ok(())
    }
    /// Preserve the reference finder stdout tee beside the detailed sidecar streams.
    pub fn open_agent_stream(&self, id: &Attempt, executor: &str) -> Result<(), String> {
        if id.stage != super::Stage::Find {
            return Ok(());
        }
        let mut state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        let attempt = state.attempts.get(id).ok_or("attempt was not started")?;
        if attempt.finished || attempt.legacy_stdout.is_some() {
            return Err("agent stream already opened or finished".into());
        }
        let result: std::io::Result<std::fs::File> = (|| {
            let name = evidence::agent_stream(id, executor)?;
            if state.agents_directory.is_none() {
                state.agents_directory = Some(prompts::directory(&self.round_directory, "agents")?);
            }
            let directory = state.agents_directory.as_ref().unwrap();
            let file = private_file(directory, &name)?;
            file.sync_all()?;
            directory.sync_all()?;
            Ok(file)
        })();
        match result {
            Ok(file) => {
                state.attempts.get_mut(id).unwrap().legacy_stdout = Some(file);
                Ok(())
            }
            Err(error) => {
                state.fail();
                Err(format!("create agent stream: {error}"))
            }
        }
    }
    pub fn append(&self, id: &Attempt, stream: Stream, bytes: &[u8]) -> Result<(), String> {
        use std::io::Write;
        let mut state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        let attempt = state
            .attempts
            .get_mut(id)
            .ok_or("attempt was not started")?;
        if attempt.finished {
            return Err("attempt already finished".into());
        }
        let result: std::io::Result<()> = (|| {
            if let std::collections::btree_map::Entry::Vacant(entry) = attempt.streams.entry(stream)
            {
                let name = match stream {
                    Stream::Stdout => "stdout",
                    Stream::Stderr => "stderr",
                    Stream::ProviderEvents => "provider-events.jsonl",
                };
                let directory = attempt.directory.as_ref().expect("live attempt directory");
                let file = private_file(directory, name)?;
                directory.sync_all()?;
                entry.insert(file);
            }
            let file = attempt.streams.get_mut(&stream).expect("opened stream");
            file.write_all(bytes)?;
            file.sync_all()?;
            if stream == Stream::Stdout
                && let Some(file) = &mut attempt.legacy_stdout
            {
                file.write_all(bytes)?;
                file.sync_all()?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            state.fail();
            return Err(format!("persist attempt stream: {error}"));
        }
        Ok(())
    }
    pub fn run(
        &self,
        definition: &super::pipeline::Definition,
        control: &super::RunControl,
        backend: &impl super::pipeline::Backend,
    ) -> Result<Report, String> {
        use std::sync::atomic::Ordering;
        self.executing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "this run has already been executed; reopen for observation only")?;
        {
            let state = self.state.lock().map_err(|_| "journal lock poisoned")?;
            state.writable()?;
            if !state.attempts.is_empty() {
                return Err("cannot execute a journal with existing attempts".into());
            }
        }
        let report = super::pipeline::run(
            definition,
            control,
            &Recording {
                journal: self,
                inner: backend,
            },
        )?;
        if let Err(error) = self.finish(&report) {
            control.cancel();
            return Err(error);
        }
        Ok(report)
    }
    pub fn finish(&self, report: &Report) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        if matches!(
            report.state,
            super::pipeline::State::Complete | super::pipeline::State::Partial
        ) && state.attempts.values().any(|attempt| !attempt.finished)
        {
            return Err("cannot finish a successful run with unfinished attempts".into());
        }
        state.persist(&Record::Terminal { report })?;
        state.sealed = true;
        state.observers.clear();
        state.legacy_events.take();
        state.stages_directory.take();
        state.agents_directory.take();
        state.prompts = Default::default();
        for attempt in state.attempts.values_mut() {
            attempt.streams.clear();
            attempt.legacy_stdout.take();
            attempt.directory.take();
        }
        Ok(())
    }
}
impl State {
    fn fail(&mut self) {
        self.failed = true;
        self.observers.clear();
    }
    fn writable(&self) -> Result<(), String> {
        if self.failed {
            Err("journal previously failed; no further writes allowed".into())
        } else if self.sealed {
            Err("journal is already terminal".into())
        } else {
            Ok(())
        }
    }
    fn persist(&mut self, record: &impl Serialize) -> Result<(), String> {
        use std::io::Write;
        let mut bytes = serde_json::to_vec(record).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if let Err(e) = self
            .file
            .write_all(&bytes)
            .and_then(|_| self.file.sync_all())
        {
            self.fail();
            return Err(format!("persist journal event: {e}"));
        }
        Ok(())
    }
}
/// Read-only snapshot. No writer handle, lock, repair, dispatch or resume path.
pub fn inspect(round: &Path) -> Result<Snapshot, String> {
    let root = round.join("revust");
    decode_snapshot(
        &root,
        &std::fs::read(root.join("run.json")).map_err(|e| e.to_string())?,
        &std::fs::read(root.join("events.jsonl")).map_err(|e| e.to_string())?,
    )
}

/// Reuse journal validation with bytes read through the archive's contained handles.
pub(crate) fn decode_snapshot(
    root: &Path,
    header: &[u8],
    bytes: &[u8],
) -> Result<Snapshot, String> {
    let header: Header = serde_json::from_slice(header).map_err(|e| e.to_string())?;
    if header.version != 1 {
        return Err(format!("unsupported journal version {}", header.version));
    }
    if [
        &header.identity.task,
        &header.identity.run,
        &header.identity.request,
    ]
    .iter()
    .any(|value| value.is_empty())
    {
        return Err("journal identity is incomplete".into());
    }
    let truncated = !bytes.is_empty() && !bytes.ends_with(b"\n");
    let complete = if truncated {
        bytes
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(&[][..], |i| &bytes[..=i])
    } else {
        bytes
    };
    let mut snapshot = Snapshot {
        identity: header.identity,
        attempts: vec![],
        report: None,
        truncated,
    };
    for line in complete
        .split_inclusive(|b| *b == b'\n')
        .map(|line| &line[..line.len() - 1])
    {
        if snapshot.report.is_some() {
            return Err("journal contains events after terminal report".into());
        }
        let record: Record<Report> =
            serde_json::from_slice(line).map_err(|e| format!("corrupt journal event: {e}"))?;
        match record {
            Record::Started { attempt } => {
                if snapshot.attempts.iter().any(|a| a.id == attempt) {
                    return Err("duplicate attempt start in journal".into());
                }
                snapshot.attempts.push(AttemptRecord {
                    id: attempt,
                    directory: root
                        .join("attempts")
                        .join(snapshot.attempts.len().to_string()),
                    result: None,
                });
            }
            Record::Finished { attempt, result } => {
                let entry = snapshot
                    .attempts
                    .iter_mut()
                    .find(|a| a.id == attempt)
                    .ok_or("finish without start in journal")?;
                if entry.result.is_some() {
                    return Err("duplicate attempt finish in journal".into());
                }
                entry.result = Some(result);
            }
            Record::Terminal { report } => {
                if matches!(
                    report.state,
                    super::pipeline::State::Complete | super::pipeline::State::Partial
                ) && snapshot.attempts.iter().any(|a| a.result.is_none())
                {
                    return Err("successful journal has unfinished attempts".into());
                }
                snapshot.report = Some(report);
            }
        }
    }
    // Any uncommitted suffix makes terminality unproven, even after a terminal line.
    if truncated {
        snapshot.report = None;
    }
    Ok(snapshot)
}
fn private_directory(parent: &std::fs::File, name: &str) -> std::io::Result<std::fs::File> {
    use nix::{
        fcntl::{OFlag, openat},
        sys::stat::{Mode, mkdirat},
    };
    mkdirat(parent, name, Mode::from_bits_truncate(0o700))?;
    let directory = std::fs::File::from(openat(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?);
    parent.sync_all()?;
    Ok(directory)
}
fn private_file(parent: &std::fs::File, name: &str) -> std::io::Result<std::fs::File> {
    use nix::{
        fcntl::{OFlag, openat},
        sys::stat::Mode,
    };
    Ok(std::fs::File::from(openat(
        parent,
        name,
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )?))
}

struct Recording<'a, B> {
    journal: &'a Journal,
    inner: &'a B,
}
impl<B: super::pipeline::Backend> super::pipeline::Backend for Recording<'_, B> {
    fn publish(&self, event: super::events::Event) -> Result<(), String> {
        self.journal.publish(event.clone())?;
        self.inner.publish(event)
    }
    fn checkpoint(&self, stage: super::Stage, report: &Report) -> Result<(), String> {
        self.inner.checkpoint(stage, report)
    }
    fn execute(
        &self,
        call: super::pipeline::Call<'_>,
        run: &super::RunControl,
    ) -> Result<serde_json::Value, AttemptFailure> {
        self.inner.execute(call, run)
    }
    fn record(&self, event: Event<'_, Categories>) -> Result<(), String> {
        // The durable start must succeed before the worker can dispatch.
        match event {
            Event::Retrying(done) => self.inner.record(Event::Retrying(done)),
            Event::Started(id) => {
                self.journal.record(Event::Started(id))?;
                self.inner.record(Event::Started(id))
            }
            Event::Finished(done) => {
                self.journal.record(Event::Finished(done))?;
                self.inner.record(Event::Finished(done))
            }
        }
    }
    fn wait(&self, duration: std::time::Duration) {
        self.inner.wait(duration);
    }
    fn jitter(&self, bound: std::time::Duration) -> std::time::Duration {
        self.inner.jitter(bound)
    }
}
