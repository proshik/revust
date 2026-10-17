//! Controlled terminal acceptance probe; no engine or providers are started.
use crossterm::event::{self, Event, KeyCode};
use std::io::{self, Write};

struct Output {
    stderr: io::Stderr,
    fail_next: bool,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if std::mem::take(&mut self.fail_next) {
            return Err(io::Error::other("controlled write error"));
        }
        self.stderr.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stderr.flush()
    }
}

fn main() -> io::Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let result = std::panic::catch_unwind(|| {
        if mode == "live" || mode == "live-error" {
            use revust_core::{
                report::Report,
                runtime::events::{Event, Kind},
            };
            use revust_tui::{Model, navigation::Action, status::Status, updates::Updates};
            use std::{sync::mpsc, time::Duration};
            let (events, rx) = mpsc::channel();
            let (finished, done) = mpsc::channel();
            let (ready, start) = mpsc::channel();
            let mut ready = Some(ready);
            let failed = mode == "live-error";
            let worker = std::thread::spawn(move || {
                start.recv().unwrap();
                std::thread::sleep(Duration::from_millis(100));
                events
                    .send(Event {
                        at: "2026-10-10T12:00:00Z".into(),
                        ..Event::agent(Kind::AgentActivity, "a", "saved activity")
                    })
                    .unwrap();
                std::thread::sleep(Duration::from_millis(100));
                finished
                    .send(if failed {
                        Err("controlled producer error".into())
                    } else {
                        Ok(Report::default())
                    })
                    .unwrap();
            });
            let mut model = Model::new(vec![]).with_auto_exit(Duration::from_secs(1));
            let mut completed_at = None;
            let result = revust_tui::terminal::observe(
                io::stderr(),
                &Status::default(),
                &mut model,
                |m, _| {
                    if let Some(ready) = ready.take() {
                        ready.send(()).unwrap();
                    }
                    vec![ratatui::text::Line::raw(if m.report.is_some() {
                        completed_at.get_or_insert_with(std::time::Instant::now);
                        "COMPLETED"
                    } else {
                        "OBSERVING"
                    })]
                },
                &mut Updates::new(rx, done),
                || false,
            );
            worker.join().unwrap();
            return match result {
                Ok(Action::Quit)
                    if !failed
                        && completed_at
                            .is_some_and(|at| at.elapsed() >= Duration::from_millis(900)) =>
                {
                    Ok(())
                }
                Err(error) if failed && error.to_string() == "controlled producer error" => Ok(()),
                other => Err(io::Error::other(format!(
                    "unexpected observation result: {other:?}"
                ))),
            };
        }
        if ["browse", "interrupt", "countdown"].contains(&mode.as_str()) {
            use revust_tui::{
                Model,
                navigation::{Action, InputDocument, Pane},
                status::Status,
            };
            let mut model = Model::from_archive(vec![], [], revust_core::report::Report::default())
                .unwrap()
                .with_inputs(vec![InputDocument::default()]);
            if mode == "countdown" {
                model = Model::new(vec![]).with_auto_exit(std::time::Duration::from_secs(1));
                model.complete(revust_core::report::Report::default());
            }
            let action = revust_tui::terminal::browse(
                io::stderr(),
                &Status::default(),
                &mut model,
                |m, p| {
                    let name = match p {
                        Pane::Input(_) => "BBBBBBBB",
                        _ => "PANE:combined",
                    };
                    vec![ratatui::text::Line::raw(format!(
                        "{name} width{}",
                        m.width()
                    ))]
                },
            )?;
            let expected = if mode == "interrupt" {
                Action::Interrupt
            } else {
                Action::Quit
            };
            if action != expected {
                return Err(io::Error::other("unexpected browser exit"));
            }
            return Ok(());
        }
        revust_tui::terminal::run(
            Output {
                stderr: io::stderr(),
                fail_next: mode == "setup-error",
            },
            |out| {
                // Register the event source before the parent sends SIGWINCH.
                let _ = event::poll(std::time::Duration::ZERO)?;
                writeln!(out, "READY")?;
                out.flush()?;
                if mode == "error" {
                    return Err(io::Error::other("controlled body error"));
                }
                if mode == "panic" {
                    panic!("controlled unwind");
                }
                if mode == "restore-error" {
                    out.fail_next = true;
                    return Ok(());
                }
                loop {
                    if event::poll(std::time::Duration::from_secs(5))? {
                        match event::read()? {
                            Event::Resize(w, h) => {
                                writeln!(out, "SIZE {w} {h}")?;
                                out.flush()?;
                            }
                            Event::Key(k) if k.code == KeyCode::Char('q') => return Ok(()),
                            _ => (),
                        }
                    } else {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "probe input timeout",
                        ));
                    }
                }
            },
        )
    });
    match (mode.as_str(), result) {
        ("panic", Err(_)) => (),
        ("error", Ok(Err(e))) if e.to_string() == "controlled body error" => (),
        ("setup-error" | "restore-error", Ok(Err(e)))
            if e.to_string() == "controlled write error" => {}
        ("" | "browse" | "interrupt" | "countdown" | "live" | "live-error", Ok(Ok(()))) => (),
        (_, other) => {
            return Err(io::Error::other(format!(
                "unexpected probe result: {other:?}"
            )));
        }
    }
    println!("RESTORED");
    io::stdout().flush()?;
    // Keep the controlling terminal alive while the parent verifies termios.
    let mut acknowledgement = String::new();
    io::stdin().read_line(&mut acknowledgement)?;
    if acknowledgement != "ack\n" {
        return Err(io::Error::other("missing parent acknowledgement"));
    }
    Ok(())
}
