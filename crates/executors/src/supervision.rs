//! One owned attempt, streamed through nonblocking pipes. Adapters decide which
//! output is meaningful activity; callbacks must return promptly.
use crate::process::ProcessGroup;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use revust_core::{request::Request, runtime::StopReason};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    os::fd::AsFd,
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Termination {
    Exited,
    IdleTimeout,
    HardTimeout,
    RunStopped(StopReason),
}
pub struct Limits {
    pub idle: Option<Duration>,
    pub hard: Option<Duration>,
}
pub struct Outcome {
    pub pid: Option<u32>,
    pub status: Option<ExitStatus>,
    pub termination: Termination,
    /// False if a pipe remained open beyond cleanup (for example an escaped
    /// descendant). Never interpret incomplete transport as a complete answer.
    pub output_complete: bool,
}
pub fn run(
    request: &Request,
    environment: &BTreeMap<String, String>,
    limits: Limits,
    stopped: impl Fn() -> Option<StopReason>,
    emit: impl FnMut(Stream, &[u8]) -> io::Result<bool>,
) -> io::Result<Outcome> {
    run_observed(request, environment, limits, stopped, emit, || Ok(false))
}
/// Poll an additional liveness source while the process is running. The final
/// observation belongs to the adapter and must not rearm the watchdog.
pub fn run_observed(
    request: &Request,
    environment: &BTreeMap<String, String>,
    limits: Limits,
    stopped: impl Fn() -> Option<StopReason>,
    emit: impl FnMut(Stream, &[u8]) -> io::Result<bool>,
    poll: impl FnMut() -> io::Result<bool>,
) -> io::Result<Outcome> {
    run_with_environment(request, environment, &[], limits, stopped, emit, poll)
}
/// Remove inherited variables before applying explicit child-only overrides.
pub fn run_with_environment(
    request: &Request,
    environment: &BTreeMap<String, String>,
    remove: &[&str],
    limits: Limits,
    stopped: impl Fn() -> Option<StopReason>,
    emit: impl FnMut(Stream, &[u8]) -> io::Result<bool>,
    poll: impl FnMut() -> io::Result<bool>,
) -> io::Result<Outcome> {
    run_with_policy(
        request,
        environment,
        Environment::Inherited(remove),
        limits,
        stopped,
        emit,
        poll,
    )
}
enum Environment<'a> {
    Inherited(&'a [&'a str]),
    Isolated,
}
fn run_with_policy(
    request: &Request,
    environment: &BTreeMap<String, String>,
    policy: Environment<'_>,
    limits: Limits,
    stopped: impl Fn() -> Option<StopReason>,
    mut emit: impl FnMut(Stream, &[u8]) -> io::Result<bool>,
    mut poll: impl FnMut() -> io::Result<bool>,
) -> io::Result<Outcome> {
    if let Some(reason) = stopped() {
        return Ok(Outcome {
            pid: None,
            status: None,
            termination: Termination::RunStopped(reason),
            output_complete: true,
        });
    }
    let started = Instant::now();
    let mut command = Command::new(&request.program);
    match policy {
        Environment::Inherited(remove) => {
            for name in remove {
                command.env_remove(name);
            }
        }
        Environment::Isolated => {
            command.env_clear();
        }
    }
    command
        .args(&request.argv)
        .current_dir(&request.cwd)
        .envs(environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ProcessGroup::spawn(&mut command)?;
    let pid = Some(child.id());
    let mut stdin = child.take_stdin();
    let mut stdout = child.take_stdout().expect("stdout requested");
    let mut stderr = child.take_stderr().expect("stderr requested");
    nonblocking(stdin.as_ref().expect("stdin requested"))?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let mut offset = 0;
    let mut activity = started;
    let (mut out_eof, mut err_eof) = (false, false);
    let mut terminal: Option<(ExitStatus, Termination, Instant)> = None;
    loop {
        let out = drain(&mut stdout, Stream::Stdout, &mut emit)?;
        let err = drain(&mut stderr, Stream::Stderr, &mut emit)?;
        out_eof |= out.eof;
        err_eof |= err.eof;
        let mut progress = out.progress || err.progress;
        let observed = terminal.is_none() && poll()?;
        if out.activity || err.activity || observed {
            activity = Instant::now();
        }
        if let Some((status, termination, since)) = terminal {
            if (out_eof && err_eof) || since.elapsed() >= Duration::from_millis(100) {
                return Ok(Outcome {
                    pid,
                    status: Some(status),
                    termination,
                    output_complete: out_eof && err_eof,
                });
            }
        } else {
            if let Some(input) = &mut stdin {
                let remaining = &request.stdin.as_bytes()[offset..];
                if !remaining.is_empty() {
                    match input.write(&remaining[..remaining.len().min(64 * 1024)]) {
                        Ok(n) => {
                            offset += n;
                            progress |= n > 0;
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                            ) => {}
                        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                            stdin = None;
                        }
                        Err(error) => return Err(error),
                    }
                }
                if offset == request.stdin.len() {
                    stdin = None;
                }
            }
            let reason = if let Some(reason) = stopped() {
                Some(Termination::RunStopped(reason))
            } else if expired(limits.hard, started.elapsed()) {
                Some(Termination::HardTimeout)
            } else if expired(limits.idle, activity.elapsed()) {
                Some(Termination::IdleTimeout)
            } else {
                None
            };
            let finished = if let Some(reason) = reason {
                Some((child.stop()?, reason))
            } else {
                child
                    .try_wait()?
                    .map(|status| (status, Termination::Exited))
            };
            if let Some((status, reason)) = finished {
                stdin = None;
                terminal = Some((status, reason, Instant::now()));
                continue;
            }
        }
        if !progress {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
fn expired(limit: Option<Duration>, elapsed: Duration) -> bool {
    limit.is_some_and(|limit| !limit.is_zero() && elapsed >= limit)
}
fn nonblocking(pipe: &impl AsFd) -> io::Result<()> {
    let flags = OFlag::from_bits_truncate(fcntl(pipe, FcntlArg::F_GETFL)?);
    fcntl(pipe, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
    Ok(())
}
#[derive(Default)]
struct Drained {
    eof: bool,
    progress: bool,
    activity: bool,
}
fn drain(
    reader: &mut impl Read,
    stream: Stream,
    emit: &mut impl FnMut(Stream, &[u8]) -> io::Result<bool>,
) -> io::Result<Drained> {
    let mut result = Drained::default();
    let mut buffer = [0; 16 * 1024];
    // A chatty pipe must not starve cancellation, the other pipe or deadlines.
    for _ in 0..32 {
        match reader.read(&mut buffer) {
            Ok(0) => {
                result.eof = true;
                break;
            }
            Ok(n) => {
                result.progress = true;
                result.activity |= emit(stream, &buffer[..n])?;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(result)
}

/// Launch with only the explicitly supplied child environment.
pub fn run_isolated(
    request: &Request,
    environment: &BTreeMap<String, String>,
    limits: Limits,
    stopped: impl Fn() -> Option<StopReason>,
    emit: impl FnMut(Stream, &[u8]) -> io::Result<bool>,
) -> io::Result<Outcome> {
    run_with_policy(
        request,
        environment,
        Environment::Isolated,
        limits,
        stopped,
        emit,
        || Ok(false),
    )
}
