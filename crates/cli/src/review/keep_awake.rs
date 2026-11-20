//! Holds an idle-sleep assertion while a review run executes providers.
use std::{
    io::{self, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
};

/// The macOS system helper. `-i` prevents idle sleep only; `-w` ends the
/// helper when this process exits, even if revust itself cannot clean up.
const HELPER: &str = "/usr/bin/caffeinate";

/// Release builds always use the system helper. Debug builds let tests
/// substitute it; this is not a configuration surface.
fn helper() -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("REVUST_KEEP_AWAKE_HELPER") {
        return path.into();
    }
    PathBuf::from(HELPER)
}

/// The helper stops when this value is dropped. A helper that cannot start
/// is reported once on stderr and the review continues without it.
pub(super) struct KeepAwake(Option<Child>);
impl KeepAwake {
    pub(super) fn start(enabled: bool) -> Self {
        if !enabled || !cfg!(target_os = "macos") {
            return Self(None);
        }
        let helper = helper();
        let child = Command::new(&helper)
            .args(["-i", "-w", &std::process::id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(child) => Self(Some(child)),
            Err(error) => {
                let _ = writeln!(
                    io::stderr().lock(),
                    "warning: keep awake: start {}: {error}; idle sleep is not prevented",
                    helper.display()
                );
                Self(None)
            }
        }
    }
}
impl Drop for KeepAwake {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
