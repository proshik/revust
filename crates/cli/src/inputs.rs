//! Bounded immutable display observations, captured through owned descriptors.
use nix::{
    fcntl::{OFlag, open, openat},
    sys::stat::Mode,
};
use revust_tui::navigation::InputDocument;
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read},
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
};
const FILE_LIMIT: u64 = 1 << 20;
const TOTAL_LIMIT: u64 = 8 << 20;
const CONTEXT_LIMIT: usize = 128;
const ENTRY_LIMIT: usize = 1024;
#[derive(Default)]
pub struct Sources {
    pub scope: PathBuf,
    pub input_dir: Option<PathBuf>,
    pub goal: Option<PathBuf>,
    pub profile: Option<PathBuf>,
    pub profile_is_snapshot: bool,
    pub context: Option<PathBuf>,
}
impl Sources {
    pub fn review(plan: &revust_core::plan::ExecutionPlan) -> Self {
        let selected = plan.input_paths();
        // The explicitly selected round is the trust anchor, matching the archive
        // reader. Resolve aliases of that root once; never resolve its descendants.
        let root = fs::canonicalize(plan.round()).unwrap_or_else(|_| plan.round().to_owned());
        let anchored = |path: &Path| {
            path.strip_prefix(plan.round())
                .map_or_else(|_| path.to_owned(), |suffix| root.join(suffix))
        };
        // Empty optional documents carry useful display evidence even though
        // the provider plan intentionally omits their contents.
        let empty_document = |name: &str| {
            let path = root.join("input").join(name);
            fs::symlink_metadata(&path)
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() == 0)
                .then_some(path)
        };
        Self {
            scope: anchored(&selected.scope),
            goal: selected
                .goal
                .as_deref()
                .map(anchored)
                .or_else(|| empty_document("goal.md")),
            profile: selected
                .profile
                .as_deref()
                .map(anchored)
                .or_else(|| empty_document("profile.md")),
            profile_is_snapshot: selected.profile_is_snapshot,
            context: selected.context.as_deref().map(anchored),
            // Review selection is already frozen. Do not rediscover optional
            // nonempty files which appeared after planning; empty files remain
            // display-only observations. Archive selection is separate.
            input_dir: None,
        }
    }
    pub fn archive(path: &Path) -> io::Result<Self> {
        let root = fs::canonicalize(path).or_else(|_| std::path::absolute(path))?;
        let profile = root.join("prompts/input-profile.md");
        let saved = fs::symlink_metadata(&profile).is_ok();
        Ok(Self {
            scope: root.join("input/scope.md"),
            goal: None,
            profile: saved.then_some(profile),
            profile_is_snapshot: saved,
            context: Some(root.join("input/context")),
            input_dir: Some(root.join("input")),
        })
    }
    fn optional(&self, name: &str, selected: &Option<PathBuf>) -> Option<PathBuf> {
        match &self.input_dir {
            None => selected.clone(),
            Some(input) => {
                let path = input.join(name);
                match fs::symlink_metadata(&path) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    _ => Some(path),
                }
            }
        }
    }
}
fn failure(label: &str, path: &str, notice: impl Into<String>) -> InputDocument {
    InputDocument {
        label: label.into(),
        path: path.into(),
        notice: notice.into(),
        ..Default::default()
    }
}
fn flags(directory: bool) -> OFlag {
    OFlag::O_RDONLY
        | OFlag::O_CLOEXEC
        | OFlag::O_NOFOLLOW
        | OFlag::O_NONBLOCK
        | if directory {
            OFlag::O_DIRECTORY
        } else {
            OFlag::empty()
        }
}
fn beneath(parent: &File, name: &Path, directory: bool) -> io::Result<File> {
    openat(parent, name, flags(directory), Mode::empty())
        .map(File::from)
        .map_err(io::Error::from)
}
fn directory(path: &Path) -> io::Result<File> {
    let absolute = std::path::absolute(path)?;
    let mut current = File::from(open(Path::new("/"), flags(true), Mode::empty())?);
    let mut prefix = PathBuf::from("/");
    for component in absolute.components() {
        match component {
            Component::Normal(name) => {
                prefix.push(name);
                // macOS exposes system temporary roots through these root links.
                // Only these system anchors may resolve aliases; descendants never do.
                current = if cfg!(target_os = "macos")
                    && (prefix == Path::new("/var") || prefix == Path::new("/tmp"))
                {
                    File::from(open(
                        &prefix,
                        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
                        Mode::empty(),
                    )?)
                } else {
                    beneath(&current, Path::new(name), true)?
                };
            }
            Component::ParentDir => current = beneath(&current, Path::new(".."), true)?,
            _ => {}
        }
    }
    Ok(current)
}
#[derive(Default)]
pub struct Snapshot {
    used: u64,
}
impl Snapshot {
    pub fn load(&mut self, sources: &Sources) -> Vec<InputDocument> {
        let mut selected = vec![("scope", "input/scope.md", Some(sources.scope.clone()))];
        selected.push((
            "goal",
            "input/goal.md",
            sources.optional("goal.md", &sources.goal),
        ));
        selected.push(if sources.profile_is_snapshot {
            (
                "profile",
                "prompts/input-profile.md",
                sources.profile.clone(),
            )
        } else {
            (
                "profile",
                "input/profile.md",
                sources.optional("profile.md", &sources.profile),
            )
        });
        let mut documents = selected
            .into_iter()
            .filter_map(|(label, relative, path)| {
                path.map(|path| self.file(label, relative, &path))
            })
            .collect::<Vec<_>>();
        if let Some(context) = &sources.context {
            documents.extend(self.context(context));
        }
        documents
    }
    pub fn file(&mut self, label: &str, relative: &str, path: &Path) -> InputDocument {
        let opened = (|| {
            let parent = directory(
                path.parent()
                    .ok_or_else(|| io::Error::other("missing parent"))?,
            )?;
            beneath(
                &parent,
                Path::new(
                    path.file_name()
                        .ok_or_else(|| io::Error::other("missing file name"))?,
                ),
                false,
            )
        })();
        self.capture(label, relative, path, opened)
    }
    fn capture(
        &mut self,
        label: &str,
        relative: &str,
        path: &Path,
        opened: io::Result<File>,
    ) -> InputDocument {
        let mut doc = failure(label, relative, "");
        doc.markdown = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
        let mut handle = match opened {
            Ok(handle) => handle,
            Err(error) => {
                doc.notice = format!("Snapshot unavailable [read]: {error}");
                return doc;
            }
        };
        let metadata = match handle.metadata() {
            Ok(value) => value,
            Err(error) => {
                doc.notice = format!("Snapshot unavailable [read]: {error}");
                return doc;
            }
        };
        if !metadata.is_file() {
            doc.notice = "Snapshot unavailable [file type]".into();
            return doc;
        }
        if metadata.len() == 0 {
            return doc;
        }
        let allowance = FILE_LIMIT.min(TOTAL_LIMIT.saturating_sub(self.used));
        if allowance == 0 {
            doc.notice = format!("Snapshot omitted [byte budget={TOTAL_LIMIT}]");
            return doc;
        }
        let mut bytes = Vec::new();
        let stopped = handle
            .by_ref()
            .take(allowance + 1)
            .read_to_end(&mut bytes)
            .err();
        let truncated = bytes.len() as u64 > allowance || metadata.len() > allowance;
        bytes.truncate(allowance as usize);
        if truncated {
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            if let Err(error) = std::str::from_utf8(&bytes)
                && error.error_len().is_none()
            {
                bytes.truncate(error.valid_up_to());
            }
        }
        let captured = bytes.len();
        self.used = self.used.saturating_add(captured as u64);
        match String::from_utf8(bytes) {
            Ok(value) => {
                let value = value.replace("\r\n", "\n");
                if value
                    .chars()
                    .all(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
                {
                    doc.content = value;
                } else {
                    doc.notice = format!(
                        "Snapshot withheld [text validation; observed bytes={}]",
                        metadata.len()
                    );
                    return doc;
                }
            }
            Err(_) => {
                doc.notice = format!(
                    "Snapshot withheld [text validation; observed bytes={}]",
                    metadata.len()
                );
                return doc;
            }
        }
        if truncated {
            doc.notice = format!(
                "Snapshot partial [captured={captured}; observed={}]",
                metadata.len()
            );
        }
        if let Some(error) = stopped {
            if !doc.notice.is_empty() {
                doc.notice.push_str("; ");
            }
            doc.notice
                .push_str(&format!("Snapshot read error: {error}"));
        }
        doc
    }
    fn context(&mut self, path: &Path) -> Vec<InputDocument> {
        let root = match directory(path) {
            Ok(file) => file,
            Err(error) => {
                return vec![failure(
                    "context",
                    "input/context",
                    format!("Context unavailable [directory]: {error}"),
                )];
            }
        };
        let mut state = Walk::default();
        self.walk(&root, Path::new(""), &mut state);
        if state.limited {
            state.docs.push(failure(
                "more",
                "input/context",
                format!("Context partial [document cap={CONTEXT_LIMIT}; entry cap={ENTRY_LIMIT}]"),
            ));
        }
        state.docs
    }
    fn walk(&mut self, handle: &File, relative: &Path, state: &mut Walk) {
        let remaining = ENTRY_LIMIT - state.entries;
        let listing = (|| -> io::Result<Vec<OsString>> {
            let mut reader = nix::dir::Dir::from_fd(handle.try_clone()?.into())?;
            let mut names = Vec::new();
            for item in reader.iter() {
                let item = item?;
                let name = item.file_name().to_bytes();
                if name == b"." || name == b".." {
                    continue;
                }
                names.push(OsString::from(std::ffi::OsStr::from_bytes(name)));
                if names.len() > remaining {
                    break;
                }
            }
            names.sort();
            Ok(names)
        })();
        let names = match listing {
            Ok(names) => names,
            Err(error) => {
                state.docs.push(failure(
                    &relative.to_string_lossy(),
                    &format!("input/context/{}", relative.display()),
                    format!("Snapshot unavailable [read]: {error}"),
                ));
                return;
            }
        };
        for name in names {
            if state.entries == ENTRY_LIMIT || state.docs.len() == CONTEXT_LIMIT {
                state.limited = true;
                return;
            }
            state.entries += 1;
            let child = relative.join(&name);
            let display = child.to_string_lossy();
            match beneath(handle, Path::new(&name), false) {
                Ok(file) if file.metadata().is_ok_and(|info| info.is_dir()) => {
                    self.walk(&file, &child, state)
                }
                result => state.docs.push(self.capture(
                    &display,
                    &format!("input/context/{display}"),
                    &child,
                    result,
                )),
            }
        }
    }
}
#[derive(Default)]
struct Walk {
    docs: Vec<InputDocument>,
    entries: usize,
    limited: bool,
}
