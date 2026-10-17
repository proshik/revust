//! Console application support; filesystem access stays outside the TUI model.
#[cfg(unix)]
pub mod archive_view;
#[cfg(unix)]
pub mod execution;
#[cfg(unix)]
pub mod inputs;

#[cfg(unix)]
pub mod presentation;
