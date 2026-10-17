//! Provider-independent process ownership; no terminal host dependency.
#[cfg(unix)]
pub mod process;

#[cfg(unix)]
pub mod supervision;

#[cfg(unix)]
pub mod configured;

pub mod claude;

pub mod codex;

pub mod events;

pub mod rollout;

pub mod opencode;

#[cfg(unix)]
pub mod backend;

mod framing;
