#![cfg(target_os = "macos")]
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

/// A review tree whose provider and idle-sleep helper are local scripts.
struct Tree(PathBuf);
impl Drop for Tree {
    fn drop(&mut self) {
        if let Some(pid) = self.helper_pid() {
            // Keep a failing regression from leaving its controlled process alive.
            let _ = kill(pid, Signal::SIGKILL);
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}
impl Tree {
    /// `provider` runs after stdin is consumed; `runtime` extends the runtime policy.
    fn new(id: usize, provider: &str, runtime: Value) -> Self {
        let root =
            std::env::temp_dir().join(format!("revust-keep-awake-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".revust/prompts/profiles")).unwrap();
        fs::create_dir_all(root.join("tasks/test/01/input")).unwrap();
        fs::write(root.join("tasks/test/01/input/scope.md"), "synthetic scope").unwrap();
        fs::write(
            root.join(".revust/prompts/profiles/test.md"),
            "---\nmodel: claude/sonnet\nagents:\n  - name: a\n    lenses: [bugs]\n---\nReview {{SCOPE}}\n",
        )
        .unwrap();
        let tree = Self(root);
        tree.script("provider", &format!("/bin/cat >/dev/null\n{provider}"));
        tree.script(
            "helper",
            &format!(
                "printf '%s\\n' \"$@\" > '{root}/helper.args.tmp'\n/bin/mv '{root}/helper.args.tmp' '{root}/helper.args'\nprintf '%s' $$ > '{root}/helper.pid.tmp'\n/bin/mv '{root}/helper.pid.tmp' '{root}/helper.pid'\nexec /bin/sleep 60\n",
                root = tree.0.display()
            ),
        );
        let mut runtime_policy = json!({"stagger_delay":"0s"});
        runtime_policy
            .as_object_mut()
            .unwrap()
            .extend(runtime.as_object().unwrap().clone());
        fs::write(
            tree.0.join(".revust/extensions.json"),
            json!({"version":1,"policy":{
                "executors":{"claude":{"executable":tree.0.join("provider")}},
                "runtime":runtime_policy,"retries":{"find":{"count":0}}}})
            .to_string(),
        )
        .unwrap();
        tree
    }
    fn script(&self, name: &str, body: &str) {
        let path = self.0.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    /// A plain command; the caller chooses the helper.
    fn revust(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_revust"));
        command
            .current_dir(&self.0)
            .env_clear()
            .env("HOME", &self.0);
        command
    }
    fn review(&self) -> Command {
        let mut command = self.revust();
        command.args(REVIEW);
        command
    }
    /// A review whose helper is the recording script.
    fn observed_review(&self) -> Command {
        let mut command = self.review();
        command.env("REVUST_KEEP_AWAKE_HELPER", self.0.join("helper"));
        command
    }
    /// A path that cannot be executed: any attempt to start it is reported.
    fn absent_helper(&self) -> PathBuf {
        self.0.join("absent-helper")
    }
    fn helper_pid(&self) -> Option<Pid> {
        let text = fs::read_to_string(self.0.join("helper.pid")).ok()?;
        Some(Pid::from_raw(text.parse().ok()?))
    }
    fn helper_args(&self) -> String {
        fs::read_to_string(self.0.join("helper.args")).unwrap()
    }
}
const REVIEW: [&str; 7] = [
    "--task=test",
    "--run=01",
    "--tasks-dir=tasks",
    "--profile=test",
    "--no-tui",
    "--no-synthesis",
    "--no-verify",
];
/// The provider waits for the helper, so the helper records itself before
/// revust can stop it.
fn after_helper(tree: &Tree, then: &str) -> String {
    format!(
        "i=0\nwhile [ ! -s '{}' ] && [ $i -lt 100 ]; do /bin/sleep 0.05; i=$((i+1)); done\n{then}",
        tree.0.join("helper.pid").display()
    )
}
fn finding() -> String {
    let payload = json!({"type":"result","structured_output":{"findings":[{"file":"a.rs","line":1,"title":"Synthetic","severity":"major","confidence":75}]}});
    format!("printf '%s\\n' '{payload}'\n")
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}
fn alive(pid: Pid) -> bool {
    kill(pid, None).is_ok()
}

#[test]
fn review_holds_helper_for_this_process_until_completion_or_failure() {
    for (id, then, code) in [(0, finding(), 1), (1, "exit 7\n".to_owned(), 2)] {
        let tree = Tree::new(id, "", json!({}));
        tree.script(
            "provider",
            &format!("/bin/cat >/dev/null\n{}", after_helper(&tree, &then)),
        );
        let child = tree
            .observed_review()
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(code), "{}", stderr(&out));
        assert_eq!(tree.helper_args(), format!("-i\n-w\n{pid}\n"));
        // The recording helper ignores -w, so only revust itself can stop it.
        assert!(!alive(tree.helper_pid().unwrap()), "helper survived review");
        assert!(!stderr(&out).contains("keep awake"), "{}", stderr(&out));
        if code == 1 {
            let report: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(report["findings"].as_array().unwrap().len(), 1);
        }
    }
}

#[test]
fn interrupted_review_stops_helper() {
    let tree = Tree::new(2, "", json!({}));
    let provider_pid = tree.0.join("provider.pid");
    tree.script(
        "provider",
        &format!(
            "/bin/cat >/dev/null\n{}",
            after_helper(
                &tree,
                &format!(
                    "printf '%s' $$ > '{0}.tmp'\n/bin/mv '{0}.tmp' '{0}'\nexec /bin/sleep 60\n",
                    provider_pid.display()
                )
            )
        ),
    );
    let mut child = tree
        .observed_review()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !provider_pid.is_file() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let revust = Pid::from_raw(child.id() as i32);
    if !provider_pid.is_file() {
        let _ = kill(revust, Signal::SIGKILL);
        let _ = child.wait();
        panic!("provider did not start");
    }
    let helper = tree.helper_pid().unwrap();
    assert!(alive(helper), "helper stopped before the review ended");
    kill(revust, Signal::SIGTERM).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(!alive(helper), "helper survived cancellation");
}

#[test]
fn disabled_policy_starts_no_helper_and_is_inspectable() {
    let tree = Tree::new(3, &finding(), json!({"keep_awake":false}));
    let out = tree
        .review()
        .env("REVUST_KEEP_AWAKE_HELPER", tree.absent_helper())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(!stderr(&out).contains("keep awake"), "{}", stderr(&out));
    let inspected = tree
        .revust()
        .args(["config", "--effective", "--task=test", "--run=01"])
        .output()
        .unwrap();
    assert_eq!(inspected.status.code(), Some(0), "{}", stderr(&inspected));
    let result: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(result["policy"]["values"]["runtime"]["keep_awake"], false);
    assert_eq!(
        result["policy"]["origins"]["/runtime/keep_awake"]["layer"],
        "project"
    );
}

#[test]
fn missing_helper_warns_once_and_review_completes() {
    let tree = Tree::new(4, &finding(), json!({}));
    let out = tree
        .review()
        .env("REVUST_KEEP_AWAKE_HELPER", tree.absent_helper())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let warnings: Vec<_> = stderr(&out)
        .lines()
        .filter(|line| line.starts_with("warning: keep awake: "))
        .map(str::to_owned)
        .collect();
    assert_eq!(warnings.len(), 1, "{}", stderr(&out));
    assert!(warnings[0].contains("absent-helper"), "{}", warnings[0]);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["findings"].as_array().unwrap().len(), 1);
    assert_eq!(
        fs::read(tree.0.join("tasks/test/01/findings.json")).unwrap(),
        out.stdout
    );
}

#[test]
fn inspection_and_saved_views_start_no_helper() {
    let tree = Tree::new(5, &finding(), json!({"keep_awake":false}));
    assert_eq!(tree.review().output().unwrap().status.code(), Some(1));
    let round = tree.0.join("tasks/test/01");
    let commands: [Vec<String>; 4] = [
        ["dry-run", "--task=test", "--run=01", "--tasks-dir=tasks"]
            .map(Into::into)
            .to_vec(),
        ["config", "--effective"].map(Into::into).to_vec(),
        ["stats", "--tasks-dir=tasks"].map(Into::into).to_vec(),
        vec![
            "view".into(),
            "--no-tui".into(),
            "--archive".into(),
            round.display().to_string(),
        ],
    ];
    for args in commands {
        let out = tree
            .revust()
            .args(&args)
            .env("REVUST_KEEP_AWAKE_HELPER", tree.absent_helper())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", stderr(&out));
        assert!(
            !stderr(&out).contains("keep awake"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
}

#[test]
fn system_helper_runs_as_child_during_review_and_ends_with_it() {
    let tree = Tree::new(6, "", json!({}));
    let snapshot = tree.0.join("processes");
    tree.script(
        "provider",
        &format!(
            "/bin/cat >/dev/null\n/bin/ps -A -o ppid=,args= > '{}'\n{}",
            snapshot.display(),
            finding()
        ),
    );
    let child = tree
        .review()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let helper = format!("/usr/bin/caffeinate -i -w {pid}");
    let during = fs::read_to_string(snapshot).unwrap();
    assert!(
        during.lines().any(|line| {
            let line = line.trim_start();
            line.split_once(' ')
                .is_some_and(|(parent, args)| parent == pid.to_string() && args.trim() == helper)
        }),
        "{during}"
    );
    let after = Command::new("/bin/ps")
        .args(["-A", "-o", "args="])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&after.stdout)
            .lines()
            .any(|line| line.trim() == helper)
    );
}
