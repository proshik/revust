//! Restore a read-only observation from saved evidence, without runtime handles.
use crate::inputs::{Snapshot, Sources};
use revust_core::{
    archive::reader,
    prompts::{Agent, runner::Runner},
};
use revust_tui::{Model, status::Status};
use std::path::Path;
pub struct View {
    pub model: Model,
    pub status: Status,
}
fn roster(saved: reader::Agent) -> Agent {
    Agent {
        name: saved.name,
        lenses: saved.lenses,
        color: saved.color,
        color_name: saved.color_name,
        runner: Runner {
            executor: saved.executor,
            model: saved.model,
            effort: saved.effort,
        },
    }
}
pub fn load(path: &Path) -> Result<View, String> {
    let reader::ArchivedRun {
        manifest,
        mut report,
        roster: saved_agents,
        events,
    } = reader::load(path)?;
    // Old archives sometimes carry requested stage settings only in the manifest.
    // Existing report settings remain authoritative when present.
    if report.stats.stages.is_empty() {
        report.stats.stages.extend(manifest.stages);
    }
    let inputs = Sources::archive(path).map_err(|error| format!("resolve archive: {error}"))?;
    let documents = Snapshot::default().load(&inputs);
    let model = Model::from_saved_events(
        saved_agents.into_iter().map(roster).collect(),
        events,
        report,
    )?
    .with_inputs(documents);
    Ok(View {
        model,
        status: Status {
            task: manifest.task,
            run: manifest.run,
            profile: manifest.profile,
            ..Default::default()
        },
    })
}
