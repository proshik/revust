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

pub fn style(status: &mut Status) {
    status.colors =
        std::env::var_os("NO_COLOR").is_none() && std::env::var("TERM").as_deref() != Ok("dumb");
    status.dark = std::env::var("COLORFGBG")
        .ok()
        .and_then(|s| s.rsplit(';').next().and_then(|n| n.parse::<u8>().ok()))
        .is_none_or(|n| n < 7 || n == 8);
}
