//! Legacy prompt loading and composition, independent of providers and terminals.
pub mod colors;
mod defaults;
#[cfg(unix)]
pub mod materialize;
pub mod runner;
pub mod template;
mod tree;
pub(crate) use tree::color;
pub(crate) use tree::decode;
pub use tree::{Agent, Catalog, FileOrigin, PromptSet};
