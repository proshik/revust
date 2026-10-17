//! Directory inventory and preparation rules shared by archive owners.
use super::error;
use nix::{
    fcntl::{AtFlags, OFlag},
    sys::stat::{Mode, fstatat},
};
use std::{ffi::OsString, fs::File, os::unix::ffi::OsStrExt};

pub(super) fn names(root: &File) -> Result<Vec<OsString>, String> {
    let mut cursor = nix::dir::Dir::openat(
        root,
        ".",
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(error)?;
    let mut result = Vec::new();
    for entry in cursor.iter() {
        let entry = entry.map_err(error)?;
        let bytes = entry.file_name().to_bytes();
        if bytes != b"." && bytes != b".." {
            result.push(std::ffi::OsStr::from_bytes(bytes).to_owned());
        }
    }
    result.sort();
    Ok(result)
}

enum Preparation {
    Unclaimed,
    EmptyClaim,
    Completed,
    Interrupted,
    InvalidMarker,
}

fn preparation(round: &File) -> Result<Preparation, String> {
    let entries = names(round)?;
    if !entries.iter().any(|name| name == "manifest.json") {
        return Ok(Preparation::Unclaimed);
    }
    let marker = fstatat(round, "manifest.json", AtFlags::AT_SYMLINK_NOFOLLOW).map_err(error)?;
    let regular = marker.st_mode & nix::libc::S_IFMT == nix::libc::S_IFREG;
    let artifacts = entries
        .iter()
        .any(|name| name != "input" && name != "manifest.json");
    Ok(match (regular, marker.st_size, artifacts) {
        (false, _, _) => Preparation::InvalidMarker,
        (true, 0, false) => Preparation::EmptyClaim,
        (true, 0, true) => Preparation::Interrupted,
        (true, _, _) => Preparation::Completed,
    })
}

/// Preparation observes a claim but never acquires or completes it.
/// Only an owner holding the marker lock may use this to authorize execution.
pub(super) fn preparable(round: &File) -> Result<(), String> {
    match preparation(round)? {
        Preparation::Unclaimed | Preparation::EmptyClaim => Ok(()),
        Preparation::Completed => Err("round has already run".into()),
        Preparation::Interrupted => {
            Err("interrupted round retains artifacts; prepare a new round".into())
        }
        Preparation::InvalidMarker => Err("manifest.json is not a regular file".into()),
    }
}
