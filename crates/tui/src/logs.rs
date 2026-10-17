//! Plain journal rows, independent of native styled pane rendering.
use crate::{At, Model, navigation::Pane, status::Status, wrap};
impl Status {
    pub fn log_rows(&self, model: &Model, pane: Pane, clock: impl Fn(At) -> String) -> Vec<String> {
        let entries = match pane {
            Pane::Combined => &model.combined,
            Pane::Agent(index) => match model.agents.get(index) {
                Some(agent) => &agent.lines,
                None => return vec!["no such agent".into()],
            },
            _ => return vec![],
        };
        if entries.is_empty() {
            return vec!["waiting for activity...".into()];
        }
        entries
            .iter()
            .flat_map(|entry| {
                let identity = if matches!(pane, Pane::Combined) && !entry.agent.is_empty() {
                    format!("{}: ", entry.agent)
                } else {
                    String::new()
                };
                wrap::wrap(
                    &format!("{} {identity}", clock(entry.at)),
                    &entry.text,
                    model.width(),
                )
            })
            .collect()
    }
}
