//! CLI-only signal registration. Handlers never allocate, lock, or touch the runtime.
#![allow(unsafe_code)]
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use revust_core::runtime::RunControl;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
extern "C" fn interrupted(_: i32) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}
pub(super) fn received() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}
pub struct Guard {
    previous: Vec<(Signal, SigAction)>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Guard {
    pub fn install(control: RunControl) -> Result<Self, String> {
        INTERRUPTED.store(false, Ordering::Relaxed);
        let mut guard = Self {
            previous: vec![],
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        };
        let action = SigAction::new(
            SigHandler::Handler(interrupted),
            SaFlags::SA_RESTART,
            SigSet::empty(),
        );
        for signal in [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP] {
            // SAFETY: handler performs one lock-free atomic store only and has static lifetime.
            let previous = unsafe { sigaction(signal, &action) }.map_err(|e| e.to_string())?;
            guard.previous.push((signal, previous));
        }
        let stop = guard.stop.clone();
        guard.thread = Some(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if received() {
                    control.cancel();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }));
        Ok(guard)
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        for (signal, previous) in self.previous.drain(..).rev() {
            // SAFETY: restore the handler previously returned by sigaction for this signal.
            let _ = unsafe { sigaction(signal, &previous) };
        }
    }
}
