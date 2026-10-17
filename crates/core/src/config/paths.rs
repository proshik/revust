//! Caller-relative discovery. Looking up a configuration never creates it.
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq)]
pub struct Layers {
    pub user: PathBuf,
    pub project: Option<PathBuf>,
}
impl Layers {
    pub fn resolve(cwd: &Path, home: Option<&Path>, config_dir: Option<&Path>) -> io::Result<Self> {
        let explicit = config_dir.filter(|p| !p.as_os_str().is_empty());
        let user = if let Some(directory) = explicit {
            absolute(cwd, directory)?
        } else {
            let home = home.filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "locate home directory: HOME is not set",
                )
            })?;
            absolute(cwd, &home.join(".config/revust"))?
        };
        let candidate = absolute(cwd, Path::new(".revust"))?;
        let project = match fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_dir() && !directory_identity(&candidate, &user) => {
                Some(candidate)
            }
            _ => None,
        };
        Ok(Self { user, project })
    }
}
fn directory_identity(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}
/// Normalize path components lexically; symlink resolution is deliberately separate.
pub fn absolute(cwd: &Path, path: &Path) -> io::Result<PathBuf> {
    if !cwd.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cwd must be absolute",
        ));
    }
    let joined = if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    };
    let mut components = Vec::new();
    for piece in joined.components() {
        match piece {
            Component::Normal(name) => components.push(name.to_owned()),
            Component::ParentDir => {
                components.pop();
            }
            Component::RootDir | Component::CurDir => (),
            Component::Prefix(_) => unreachable!("Unix paths have no prefix"),
        }
    }
    Ok(std::iter::once(PathBuf::from("/"))
        .chain(components.into_iter().map(PathBuf::from))
        .collect())
}
pub fn work_dir(cwd: &Path, requested: Option<&Path>) -> io::Result<PathBuf> {
    match requested.filter(|p| !p.as_os_str().is_empty()) {
        None => absolute(cwd, cwd),
        Some(path) => {
            let directory = absolute(cwd, path)?;
            if fs::metadata(&directory)?.is_dir() {
                Ok(directory)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    format!("workdir {} is not a directory", directory.display()),
                ))
            }
        }
    }
}
pub fn profile_fallback(cwd: &Path) -> io::Result<Option<PathBuf>> {
    let location = absolute(cwd, Path::new(".revust/profile.md"))?;
    match fs::metadata(&location) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
        Ok(metadata) => {
            if metadata.is_dir() {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is a directory, want a file", location.display()),
                ))
            } else {
                Ok((metadata.len() > 0).then_some(location))
            }
        }
    }
}
