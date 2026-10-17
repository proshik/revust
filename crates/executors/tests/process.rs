#![cfg(unix)]
use revust_executors::process::ProcessGroup;
use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn poll(group: &mut ProcessGroup) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = group.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "owned process failed to exit");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn normal_exit_preserves_status_and_closes_descendant_pipes() {
    let mut command = Command::new("/bin/sh");
    // The leader exits while its child still owns stdout. Killing only the
    // leader or waiting for EOF first would leave this test blocked for 30s.
    command
        .args(["-c", "/bin/sleep 30 & printf ready; exit 7"])
        .stdout(Stdio::piped());
    let started = Instant::now();
    let mut group = ProcessGroup::spawn(&mut command).unwrap();
    let mut output = group.take_stdout().unwrap();
    let status = poll(&mut group);
    assert_eq!(status.code(), Some(7));
    assert_eq!(group.stop().unwrap(), status);
    assert_eq!(group.try_wait().unwrap(), Some(status));
    let mut bytes = String::new();
    output.read_to_string(&mut bytes).unwrap();
    assert_eq!(bytes, "ready");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn explicit_stop_and_drop_terminate_the_owned_group() {
    for explicit in [false, true] {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "/bin/sleep 30 & printf ready; wait"])
            .stdout(Stdio::piped());
        let mut group = ProcessGroup::spawn(&mut command).unwrap();
        let mut output = group.take_stdout().unwrap();
        let mut ready = [0; 5];
        output.read_exact(&mut ready).unwrap();
        assert_eq!(&ready, b"ready");
        let started = Instant::now();
        if explicit {
            assert!(!group.stop().unwrap().success());
        }
        drop(group);
        let mut trailing = Vec::new();
        output.read_to_end(&mut trailing).unwrap();
        assert!(trailing.is_empty());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}

#[test]
fn stopping_one_owned_group_leaves_an_independent_owned_group_running() {
    fn start() -> ProcessGroup {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "printf ready; /bin/sleep 30 & wait"])
            .stdout(Stdio::piped());
        let mut group = ProcessGroup::spawn(&mut command).unwrap();
        let mut ready = [0; 5];
        group.take_stdout().unwrap().read_exact(&mut ready).unwrap();
        assert_eq!(&ready, b"ready");
        group
    }
    let mut first = start();
    let mut independent = start();
    first.stop().unwrap();
    assert_eq!(independent.try_wait().unwrap(), None);
    drop(first);
    assert_eq!(independent.try_wait().unwrap(), None);
    independent.stop().unwrap();
}
