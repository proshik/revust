//! Owned process group. Adapted ownership strategy from pinned Guild; see
//! third_party/guild/NOTICE. The leader remains unreaped until group cleanup.
use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::{
    io,
    os::unix::process::CommandExt,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus},
};

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod platform;
#[cfg(not(target_os = "macos"))]
mod platform {
    use nix::{
        errno::Errno,
        sys::wait::{Id, WaitPidFlag, WaitStatus, waitid},
        unistd::Pid,
    };
    use std::io;
    pub(super) fn exited(pid: Pid) -> io::Result<bool> {
        loop {
            match waitid(
                Id::Pid(pid),
                WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
            ) {
                Ok(status) => return Ok(status != WaitStatus::StillAlive),
                Err(Errno::EINTR) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
    pub(super) fn only_exited_leader(_: Pid) -> io::Result<bool> {
        Ok(false)
    }
}

pub struct ProcessGroup {
    child: Child,
    status: Option<ExitStatus>,
}
impl ProcessGroup {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        command.process_group(0);
        Ok(Self {
            child: command.spawn()?,
            status: None,
        })
    }
    pub fn id(&self) -> u32 {
        self.child.id()
    }
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }
    fn pid(&self) -> Pid {
        Pid::from_raw(self.child.id() as i32)
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        if platform::exited(self.pid())? {
            self.stop().map(Some)
        } else {
            Ok(None)
        }
    }
    /// Kill every member still in the owned group, then reap the leader once.
    /// A cached terminal status prevents signaling an ID after it can be reused.
    pub fn stop(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        loop {
            match killpg(self.pid(), Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => break,
                Err(Errno::EINTR) => continue,
                Err(Errno::EPERM) if platform::only_exited_leader(self.pid())? => break,
                Err(error) => return Err(error.into()),
            }
        }
        let status = self.child.wait()?;
        self.status = Some(status);
        Ok(status)
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // Explicit callers receive errors; Drop remains a final cleanup attempt.
        let _ = self.stop();
    }
}
