//! Prompt evidence is owned by physical destination and the original stage unit.
use super::super::Stage;
use nix::{
    fcntl::{OFlag, openat},
    sys::stat::{Mode, mkdirat},
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Write},
};
struct Evidence {
    stage: Stage,
    unit: String,
    bytes: Vec<u8>,
}
#[derive(Default)]
pub(super) struct Storage {
    root: Option<File>,
    directories: BTreeMap<&'static str, File>,
    artifacts: BTreeMap<(&'static str, String), Evidence>,
}
impl Storage {
    fn root(&mut self, round: &File) -> io::Result<&File> {
        if self.root.is_none() {
            self.root = Some(directory(round, "prompts")?);
        }
        Ok(self.root.as_ref().expect("opened prompt root"))
    }
    pub(super) fn calibration(&mut self, round: &File, bytes: &[u8]) -> io::Result<()> {
        persist(self.root(round)?, "input-profile.md", bytes)
    }
    pub(super) fn save(
        &mut self,
        round: &File,
        stage: Stage,
        unit: &str,
        bytes: &[u8],
    ) -> io::Result<()> {
        super::evidence::unit_name(unit)?;
        let (parent, name) = match stage {
            Stage::Find => ("agents", format!("{unit}.md")),
            Stage::Synthesis => ("stages", "synthesis.md".to_owned()),
            Stage::Verify => ("stages", format!("verify-{unit}.md")),
        };
        let destination = (parent, name);
        match self.artifacts.get(&destination) {
            Some(saved) if saved.stage != stage || saved.unit != unit => {
                return Err(io::Error::other(
                    "prompt destination already belongs to another unit",
                ));
            }
            Some(saved) if saved.bytes != bytes => {
                return Err(io::Error::other(
                    "retry prompt differs from archived prompt",
                ));
            }
            Some(_) => return Ok(()),
            None => (),
        }
        if !self.directories.contains_key(parent) {
            let handle = directory(self.root(round)?, parent)?;
            self.directories.insert(parent, handle);
        }
        persist(&self.directories[parent], &destination.1, bytes)?;
        self.artifacts.insert(
            destination,
            Evidence {
                stage,
                unit: unit.to_owned(),
                bytes: bytes.to_owned(),
            },
        );
        Ok(())
    }
}
// Retained original descriptor-relative directory primitive; archive ownership is
// qualified separately from the prompt retention policy above.
pub(super) fn directory(parent: &File, name: &str) -> io::Result<File> {
    match mkdirat(parent, name, Mode::from_bits_truncate(0o700)) {
        Ok(()) => parent.sync_all()?,
        Err(nix::errno::Errno::EEXIST) => (),
        Err(error) => return Err(error.into()),
    }
    Ok(File::from(openat(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?))
}

fn persist(parent: &File, name: &str, bytes: &[u8]) -> io::Result<()> {
    let mut destination = super::private_file(parent, name)?;
    destination
        .write_all(bytes)
        .and_then(|_| destination.sync_all())
        .and_then(|_| parent.sync_all())
}
