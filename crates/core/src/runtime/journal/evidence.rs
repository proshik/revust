//! Artifact names are part of the saved-run format, separate from storage ownership.
use crate::runtime::{Attempt, Stage};
use std::io;
pub(super) fn unit_name(unit: &str) -> io::Result<()> {
    if unit.is_empty() || matches!(unit, "." | "..") || unit.contains(['/', '\\', '\0']) {
        Err(io::Error::other("invalid evidence unit name"))
    } else {
        Ok(())
    }
}
pub(super) fn checkpoint(stage: Stage) -> &'static str {
    match stage {
        Stage::Find => "1-found.json",
        Stage::Synthesis => "2-synthesized.json",
        Stage::Verify => "3-verified.json",
    }
}
pub(super) fn agent_stream(id: &Attempt, executor: &str) -> io::Result<String> {
    unit_name(&id.unit)?;
    let retry = match id.number {
        0 => String::new(),
        1 => ".retry".to_owned(),
        number => format!(".retry-{number}"),
    };
    let extension = if executor == "codex" { "log" } else { "jsonl" };
    Ok(format!("{}{retry}.{extension}", id.unit))
}
