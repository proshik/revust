//! Terminal selection stays in the console application, outside the engine.
use revust_tui::status::Status;
use std::fs::{File, OpenOptions};

pub fn tty(enabled: bool) -> Option<File> {
    enabled
        .then(|| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/tty")
                .ok()
        })
        .flatten()
}

/// A terminal receives inert text; redirected output keeps the saved bytes.
pub fn inert_on_terminal(text: String, terminal: bool) -> String {
    if terminal {
        revust_tui::wrap::text(&text)
    } else {
        text
    }
}

pub fn style(status: &mut Status) {
    status.colors =
        std::env::var_os("NO_COLOR").is_none() && std::env::var("TERM").as_deref() != Ok("dumb");
    status.dark = std::env::var("COLORFGBG")
        .ok()
        .and_then(|s| s.rsplit(';').next().and_then(|n| n.parse::<u8>().ok()))
        .is_none_or(|n| n < 7 || n == 8);
}
