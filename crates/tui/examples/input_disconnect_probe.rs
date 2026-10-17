//! Input-loss probe without drawing, signals, executors or model calls.
use std::{
    io::{self, Write},
    time::Duration,
};
fn main() -> io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let _ = crossterm::event::poll(Duration::ZERO)?;
    println!("READY");
    io::stdout().flush()?;
    loop {
        match crossterm::event::poll(Duration::from_millis(100)) {
            Err(error) => {
                println!("DISCONNECTED: {error}");
                return Ok(());
            }
            Ok(true) => {
                let _ = crossterm::event::read()?;
            }
            Ok(false) => (),
        }
    }
}
