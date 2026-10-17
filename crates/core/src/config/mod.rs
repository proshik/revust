//! Layered revust configuration.
pub mod catalog;
pub(crate) mod duration;
pub mod extensions;
mod ini;
pub mod paths;
pub mod runtime;

pub use duration::LegacyDuration;
