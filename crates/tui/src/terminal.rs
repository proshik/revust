//! Owns terminal presentation modes, independently of review execution.
use crate::{
    Model,
    navigation::{Action, Pane},
    status::Status,
};
use crossterm::{
    ExecutableCommand,
    cursor::{Hide, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    style::{Attribute, ResetColor, SetAttribute},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend, text::Line};
use std::cell::Cell;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// Browses an already-built observational model. It never starts or resumes a run.
/// The returned action lets the CLI distinguish ordinary exit from interruption.
pub fn browse<W: Write>(
    output: W,
    status: &Status,
    model: &mut Model,
    mut pane: impl FnMut(&Model, Pane) -> Vec<Line<'static>>,
) -> io::Result<Action> {
    drive(output, status, model, &mut pane, None, || false)
}

/// Observes a live producer through channels; never owns its worker or cancellation.
pub fn observe<W: Write>(
    output: W,
    status: &Status,
    model: &mut Model,
    mut pane: impl FnMut(&Model, Pane) -> Vec<Line<'static>>,
    updates: &mut crate::updates::Updates,
    stopped: impl Fn() -> bool,
) -> io::Result<Action> {
    drive(output, status, model, &mut pane, Some(updates), stopped)
}

fn drive<W: Write>(
    output: W,
    status: &Status,
    model: &mut Model,
    mut pane: impl FnMut(&Model, Pane) -> Vec<Line<'static>>,
    mut updates: Option<&mut crate::updates::Updates>,
    stopped: impl Fn() -> bool,
) -> io::Result<Action> {
    run(output, |output| {
        // Install input/resize listeners before the first visible frame.
        let _ = crossterm::event::poll(Duration::ZERO)?;
        let attached = Cell::new(true);
        let mut terminal = Drawing {
            terminal: Terminal::new(CrosstermBackend::new(DrawingOutput {
                output,
                attached: &attached,
            }))?,
            attached: &attached,
        };
        let mut next_tick = Instant::now() + Duration::from_secs(1);
        let mut redraw = true;
        loop {
            if stopped() {
                return Ok(Action::Interrupt);
            }
            if let Some(updates) = &mut updates {
                let completed = updates.complete();
                redraw |= updates.poll(model).map_err(io::Error::other)?;
                if !completed && updates.complete() {
                    // The first countdown second starts at completion, not at UI startup.
                    next_tick = Instant::now() + Duration::from_secs(1);
                }
            }
            if redraw {
                terminal
                    .terminal
                    .draw(|frame| status.frame(model, frame.buffer_mut(), &mut pane))?;
                redraw = false;
            }
            let mut timeout = next_tick.saturating_duration_since(Instant::now());
            if updates.as_ref().is_some_and(|u| !u.complete()) {
                timeout = timeout.min(Duration::from_millis(20));
            }
            if crossterm::event::poll(timeout)? {
                let action =
                    model.terminal_event(crossterm::event::read()?, |m, p| pane(m, p).len());
                if action != Action::None {
                    return Ok(action);
                }
                redraw = true;
            }
            if Instant::now() >= next_tick {
                redraw |= !model.exit_remaining().is_zero();
                let action = model.tick();
                if action != Action::None {
                    return Ok(action);
                }
                next_tick = Instant::now() + Duration::from_secs(1);
            }
        }
    })
}

// Session owns cursor restoration and records any failure. Ratatui's Drop also
// shows the cursor, then prints an error to stderr if that fails. On a destroyed
// PTY that diagnostic can panic. Detach its writer before Drop; Session still
// attempts every restoration operation on the actual output, including Show.
struct Drawing<'a, W: Write> {
    terminal: Terminal<CrosstermBackend<DrawingOutput<'a, W>>>,
    attached: &'a Cell<bool>,
}
impl<W: Write> Drop for Drawing<'_, W> {
    fn drop(&mut self) {
        self.attached.set(false);
    }
}
struct DrawingOutput<'a, W: Write> {
    output: W,
    attached: &'a Cell<bool>,
}
impl<W: Write> Write for DrawingOutput<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.attached.get() {
            self.output.write(bytes)
        } else {
            Ok(bytes.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.attached.get() {
            self.output.flush()
        } else {
            Ok(())
        }
    }
}

/// Runs a single presentation session on the caller-selected terminal writer.
/// Restores modes after return, setup/body errors and unwinding panics. The caller
/// owns review cancellation; leaving this scope never claims review success.
pub fn run<W: Write, T>(output: W, body: impl FnOnce(&mut W) -> io::Result<T>) -> io::Result<T> {
    let owns_raw = !terminal::is_raw_mode_enabled()?;
    if owns_raw {
        terminal::enable_raw_mode()?;
    }
    let mut session = Session {
        output,
        owns_raw,
        active: true,
    };
    let result = session.enter().and_then(|_| body(&mut session.output));
    let restored = session.restore();
    match (result, restored) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(restore)) => Err(io::Error::new(
            error.kind(),
            format!("{error}; restore terminal: {restore}"),
        )),
    }
}

struct Session<W: Write> {
    output: W,
    owns_raw: bool,
    active: bool,
}
impl<W: Write> Session<W> {
    fn enter(&mut self) -> io::Result<()> {
        self.output.execute(EnterAlternateScreen)?;
        self.output.execute(EnableBracketedPaste)?;
        self.output.execute(Hide)?;
        Ok(())
    }
    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        // Try every operation even when an earlier write fails. In particular,
        // terminal output failure must not leave keyboard input in raw mode.
        let mut first = None;
        let mut attempt = |result: io::Result<()>| {
            if let Err(error) = result {
                first.get_or_insert(error);
            }
        };
        attempt(self.output.execute(DisableBracketedPaste).map(|_| ()));
        attempt(
            self.output
                .execute(SetAttribute(Attribute::Reset))
                .map(|_| ()),
        );
        attempt(self.output.execute(ResetColor).map(|_| ()));
        attempt(self.output.execute(Show).map(|_| ()));
        attempt(self.output.execute(LeaveAlternateScreen).map(|_| ()));
        if self.owns_raw {
            attempt(terminal::disable_raw_mode());
        }
        first.map_or(Ok(()), Err)
    }
}
impl<W: Write> Drop for Session<W> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
