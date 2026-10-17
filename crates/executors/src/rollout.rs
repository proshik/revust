//! Bounded, read-only activity from the session identified by the selected child.
use crate::{
    events::{Event, summary},
    framing::Lines,
};
use serde_json::Value;
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

fn uuid(text: &str) -> bool {
    let parts: Vec<_> = text.split('-').collect();
    parts.len() == 5
        && parts.iter().zip([8, 4, 4, 4, 12]).all(|(part, size)| {
            part.len() == size && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}
fn word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
pub fn session_id(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    for start in 0..bytes.len().saturating_sub(10) {
        if !bytes[start..]
            .get(..11)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"session id:"))
        {
            continue;
        }
        if start > 0 && word(bytes[start - 1]) {
            continue;
        }
        let rest = line[start + 11..].trim_start_matches(|ch: char| ch.is_ascii_whitespace());
        let size = rest
            .bytes()
            .take_while(|byte| byte.is_ascii_hexdigit() || *byte == b'-')
            .count();
        let id = &rest[..size];
        if uuid(id) && rest.as_bytes().get(size).is_none_or(|byte| !word(*byte)) {
            return Some(id.into());
        }
    }
    None
}

pub fn event(line: &[u8]) -> Option<Event> {
    let record: Value = serde_json::from_slice(line).ok()?;
    if record.get("type")?.as_str()? != "response_item" {
        return None;
    }
    let item = record.get("payload")?;
    match item.get("type")?.as_str()? {
        "reasoning" => item
            .get("summary")?
            .as_array()?
            .iter()
            .filter_map(|entry| entry.get("text")?.as_str()?.trim().lines().next())
            .map(|text| summary(text.trim().trim_matches('*')))
            .find(|text| !text.is_empty())
            .map(Event::Activity),
        "function_call" | "custom_tool_call" => {
            let name = item.get("name")?.as_str().filter(|text| !text.is_empty())?;
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|text| serde_json::from_str::<Value>(text).ok());
            let command = arguments
                .as_ref()
                .and_then(|args| {
                    ["cmd", "command"]
                        .into_iter()
                        .filter_map(|key| args.get(key).and_then(Value::as_str))
                        .find(|text| !text.trim().is_empty())
                        .map(str::to_owned)
                })
                .or_else(|| {
                    item.get("input")
                        .and_then(Value::as_str)
                        .and_then(command_field)
                });
            match command
                .map(|text| summary(&text))
                .filter(|text| !text.is_empty())
            {
                Some(text) => Some(Event::Progress(text)),
                None if matches!(name, "exec" | "exec_command" | "shell") => None,
                None => Some(Event::Progress(summary(name))),
            }
        }
        _ => None,
    }
}

// Read only JSON string literals after a cmd property; never evaluate a snippet.
fn command_field(text: &str) -> Option<String> {
    for (colon, _) in text.match_indices(':') {
        let prefix = text[..colon].trim_end();
        let key = prefix
            .strip_suffix("\"cmd\"")
            .or_else(|| prefix.strip_suffix("cmd"));
        let Some(before) = key else {
            continue;
        };
        if before.as_bytes().last().is_some_and(|byte| word(*byte)) {
            continue;
        }
        let mut values = serde_json::Deserializer::from_str(text[colon + 1..].trim_start())
            .into_iter::<String>();
        if let Some(Ok(value)) = values.next() {
            return Some(value);
        }
    }
    None
}

pub struct Tail {
    root: PathBuf,
    id: String,
    selected: Option<Observation>,
}
struct Observation {
    file: File,
    read: u64,
    lines: Lines,
    ready: VecDeque<Vec<u8>>,
}
pub struct Update {
    pub events: Vec<Event>,
    pub advanced: bool,
}
impl Tail {
    pub fn new(root: &Path, id: &str) -> Result<Self, String> {
        if !uuid(id) {
            return Err("rollout identity must be a UUID".into());
        }
        Ok(Self {
            root: root.into(),
            id: id.into(),
            selected: None,
        })
    }
    pub fn poll(&mut self) -> io::Result<Update> {
        if self.selected.is_none() {
            self.selected = discover(&self.root, &self.id)?.map(|file| Observation {
                file,
                read: 0,
                lines: Lines::default(),
                ready: VecDeque::new(),
            });
        }
        let mut update = Update {
            events: Vec::new(),
            advanced: false,
        };
        let Some(observation) = self.selected.as_mut() else {
            return Ok(update);
        };
        if observation.file.metadata()?.len() < observation.read {
            return Err(io::Error::other("selected rollout was truncated"));
        }
        if observation.lines.overflow {
            return Err(io::Error::other("rollout record exceeds size limit"));
        }
        let (mut bytes, mut records) = (0, 0);
        let mut buffer = [0u8; 8192];
        loop {
            if records == 1024 {
                break;
            }
            if let Some(record) = observation.ready.pop_front() {
                update.advanced = true;
                records += 1;
                if let Some(event) = event(&record) {
                    update.events.push(event);
                }
                continue;
            }
            if bytes >= 1 << 20 {
                break;
            }
            let count = observation.file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes += count;
            observation.read += count as u64;
            observation
                .ready
                .extend(observation.lines.push(&buffer[..count]));
            if observation.lines.overflow {
                return Err(io::Error::other("rollout record exceeds size limit"));
            }
        }
        Ok(update)
    }
}

fn entries(root: &File) -> io::Result<Vec<std::ffi::OsString>> {
    use nix::{fcntl::OFlag, sys::stat::Mode};
    use std::os::unix::ffi::OsStrExt;
    let mut directory = nix::dir::Dir::openat(
        root,
        ".",
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?;
    let mut names = Vec::new();
    for entry in directory.iter() {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if bytes != b"." && bytes != b".." {
            if names.len() == 16_384 {
                return Err(io::Error::other(
                    "rollout directory exceeds observation limit",
                ));
            }
            names.push(std::ffi::OsStr::from_bytes(bytes).to_owned());
        }
    }
    names.sort();
    Ok(names)
}
fn directory(root: &File, name: &Path) -> io::Result<File> {
    use nix::{
        fcntl::{OFlag, openat},
        sys::stat::Mode,
    };
    Ok(File::from(openat(
        root,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?))
}
fn discover(root: &Path, id: &str) -> io::Result<Option<File>> {
    use nix::{
        fcntl::{OFlag, open},
        sys::stat::Mode,
    };
    let root = match open(
        root,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => File::from(fd),
        Err(nix::errno::Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Ok(sessions) = directory(&root, Path::new("sessions")) else {
        return Ok(None);
    };
    fn visit(
        parent: &File,
        path: &Path,
        depth: usize,
        suffix: &str,
        budget: &mut usize,
        candidate: &mut Option<(PathBuf, File)>,
    ) -> io::Result<()> {
        use nix::{
            fcntl::{OFlag, openat},
            sys::stat::Mode,
        };
        for name in entries(parent)? {
            *budget = budget
                .checked_sub(1)
                .ok_or_else(|| io::Error::other("rollout inventory exceeds observation limit"))?;
            if depth < 3 {
                if let Ok(child) = directory(parent, Path::new(&name)) {
                    visit(
                        &child,
                        &path.join(name),
                        depth + 1,
                        suffix,
                        budget,
                        candidate,
                    )?;
                }
                continue;
            }
            let Some(text) = name.to_str() else {
                continue;
            };
            if !text.starts_with("rollout-") || !text.ends_with(suffix) {
                continue;
            }
            let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
            if let Ok(fd) = openat(parent, Path::new(&name), flags, Mode::empty()) {
                let file = File::from(fd);
                let path = path.join(name);
                if file.metadata()?.is_file()
                    && candidate
                        .as_ref()
                        .is_none_or(|(current, _)| path < *current)
                {
                    *candidate = Some((path, file));
                }
            }
        }
        Ok(())
    }
    let mut candidate = None;
    visit(
        &sessions,
        Path::new(""),
        0,
        &format!("-{id}.jsonl"),
        &mut 16_384,
        &mut candidate,
    )?;
    Ok(candidate.map(|(_, file)| file))
}
