//! macOS APIs not wrapped by nix 0.31.3. No raw handles escape this module.
//! Adapted from Guild's process ownership module; third_party/guild/NOTICE.
use nix::{libc, unistd::Pid};
use std::{
    io,
    time::{Duration, Instant},
};

pub(super) fn exited(pid: Pid) -> io::Result<bool> {
    // SAFETY: zeroed siginfo is valid output storage; caller exclusively owns
    // this child and WNOWAIT prevents releasing its PID before group cleanup.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    loop {
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid.as_raw() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == 0 {
            return Ok(unsafe { info.si_pid() } != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

pub(super) fn only_exited_leader(pid: Pid) -> io::Result<bool> {
    // Darwin can report EPERM for a group consisting solely of its zombie
    // leader. Never suppress EPERM for a live/foreign group member.
    let until = Instant::now() + Duration::from_millis(100);
    while !exited(pid)? {
        if Instant::now() >= until {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut members = [0 as libc::pid_t; 2];
    // SAFETY: buffer contains two pid_t slots; exact byte size is passed.
    // PROC_PGRP_ONLY=2; two slots distinguish the sole leader from more members.
    let bytes = unsafe {
        libc::proc_listpids(
            2,
            pid.as_raw() as u32,
            members.as_mut_ptr().cast(),
            std::mem::size_of_val(&members) as i32,
        )
    };
    Ok(bytes == std::mem::size_of::<libc::pid_t>() as i32 && members[0] == pid.as_raw())
}
