//! Owns execution and persistence independently of terminal observation.
use revust_core::{
    archive::Archive,
    report::{ExecutionOutcome, Report},
    runtime::RunControl,
};
use revust_executors::backend::ReviewResult;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::mpsc::{self, Receiver},
    thread::JoinHandle,
};

pub struct Worker {
    control: RunControl,
    thread: Option<JoinHandle<Result<ReviewResult, String>>>,
}
impl Worker {
    /// `execute` must honor cancellation and join its owned descendants before returning.
    /// Completion is sent after attempting report persistence and the final archive marker.
    /// Failed/stopped execution is a saved outcome; persistence failure is an error
    /// for both the observer and `join`, never durable completion.
    pub fn start(
        control: RunControl,
        mut archive: Archive,
        confidence: i64,
        execute: impl FnOnce(&RunControl) -> Result<ReviewResult, String> + Send + 'static,
    ) -> Result<(Self, Receiver<Result<Report, String>>), String> {
        let (finished, done) = mpsc::channel();
        let run = control.clone();
        let thread = std::thread::Builder::new()
            .name("review-execution".into())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    let mut result = execute(&run)?;
                    result.report = result.report.above(confidence);
                    result.report.outcome = Some(ExecutionOutcome::from_runtime(&result.runtime));
                    let persisted = archive.save_report(&result.report).and_then(|()| {
                        let mut manifest: serde_json::Value =
                            serde_json::from_str(&result.manifest)
                                .map_err(|e| format!("encode archive outcome: {e}"))?;
                        let fields = manifest
                            .as_object_mut()
                            .ok_or("archive manifest must be an object")?;
                        fields.insert(
                            "outcome".into(),
                            serde_json::to_value(&result.report.outcome)
                                .map_err(|e| e.to_string())?,
                        );
                        result.manifest =
                            serde_json::to_string(&manifest).map_err(|e| e.to_string())?;
                        archive.finish(result.manifest.as_bytes())
                    });
                    let notification = persisted
                        .as_ref()
                        .map(|()| result.report.clone())
                        .map_err(Clone::clone);
                    Ok::<_, String>((persisted.map(|()| result), notification))
                }))
                .unwrap_or_else(|_| Err("review worker panicked".into()));
                let (result, notification) = match outcome {
                    Ok(pair) => pair,
                    Err(error) => (Err(error.clone()), Err(error)),
                };
                // Observation can close without changing the already persisted outcome.
                let _ = finished.send(notification);
                result
            })
            .map_err(|e| format!("start review execution: {e}"))?;
        Ok((
            Self {
                control,
                thread: Some(thread),
            },
            done,
        ))
    }

    pub fn join(mut self) -> Result<ReviewResult, String> {
        self.thread
            .take()
            .unwrap()
            .join()
            .map_err(|_| "review worker panicked".to_owned())?
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.control.cancel();
            let _ = thread.join();
        }
    }
}
