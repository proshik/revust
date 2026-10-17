//! CLI-only terminal geometry. No engine or executor dependency on terminal I/O.
#![allow(unsafe_code)]
use nix::libc;
use std::os::fd::AsRawFd;

pub fn stderr_columns() -> Option<usize> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes exactly one initialized winsize through this
    // valid exclusive pointer. The stderr handle remains alive during the call.
    // A pipe, closed descriptor or unsupported device reports failure.
    let stderr = std::io::stderr();
    let result = unsafe { libc::ioctl(stderr.as_raw_fd(), libc::TIOCGWINSZ, &mut size) };
    (result == 0).then_some(usize::from(size.ws_col))
}
