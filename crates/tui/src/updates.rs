//! Observation transport only; producers own execution and durable completion.
use crate::Model;
use revust_core::{report::Report, runtime::events::Event};
use std::sync::mpsc::{Receiver, TryRecvError};

pub struct Updates {
    events: Receiver<Event>,
    completion: Receiver<Result<Report, String>>,
    complete: bool,
    pending: Option<Result<Report, String>>,
}
impl Updates {
    pub fn new(events: Receiver<Event>, completion: Receiver<Result<Report, String>>) -> Self {
        Self {
            events,
            completion,
            complete: false,
            pending: None,
        }
    }
    pub fn complete(&self) -> bool {
        self.complete
    }
    /// Drains a bounded batch so continuous output cannot starve keyboard input.
    /// Completion is authoritative and follows all already-queued events.
    pub fn poll(&mut self, model: &mut Model) -> Result<bool, String> {
        if self.complete {
            return Ok(false);
        }
        let mut changed = false;
        for _ in 0..256 {
            match self.events.try_recv() {
                Ok(event) => {
                    model.apply(event)?;
                    changed = true;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => {
                    if let Some(result) = self.pending.take() {
                        model.complete(result?);
                        self.complete = true;
                        return Ok(true);
                    }
                    match self.completion.try_recv() {
                        Ok(result) => self.pending = Some(result),
                        Err(TryRecvError::Empty) => return Ok(changed),
                        Err(TryRecvError::Disconnected) => {
                            return Err("review observation ended without completion".into());
                        }
                    }
                    // Recheck events after receiving completion: the producer may
                    // have queued its final event between the two channel reads.
                }
            }
        }
        Ok(changed)
    }
}
