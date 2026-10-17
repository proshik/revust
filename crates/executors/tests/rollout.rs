use revust_executors::{
    events::Event,
    rollout::{self, Tail},
};
use serde_json::json;
use std::{
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
const ID: &str = "01234567-89ab-cdef-0123-456789abcdef";
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "revust-rollout-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn session_banner_requires_a_bounded_uuid_and_never_accepts_a_path() {
    assert_eq!(
        rollout::session_id(&format!("Session ID: {ID}")),
        Some(ID.into())
    );
    for text in [
        "session id: ../../file".to_owned(),
        format!("xsession id: {ID}"),
        format!("session id: {ID}f"),
    ] {
        assert!(rollout::session_id(&text).is_none());
    }
    assert!(Tail::new(std::path::Path::new("/synthetic"), "../../file").is_err());
}
#[test]
fn reasoning_is_not_duplicated_and_shell_progress_requires_an_actual_command() {
    let reasoning = |kind| {
        json!({"type":kind,"payload":{"type":"reasoning","summary":[{"text":"**Checking helper**\nLong detail."}]}}).to_string()
    };
    assert_eq!(
        rollout::event(reasoning("response_item").as_bytes()),
        Some(Event::Activity("Checking helper".into()))
    );
    assert!(rollout::event(reasoning("event_msg").as_bytes()).is_none());
    let tool = |name, args, input| {
        json!({"type":"response_item","payload":{"type":"function_call","name":name,"arguments":args,"input":input}}).to_string()
    };
    assert_eq!(
        rollout::event(tool("exec_command", "{\"cmd\":\"rg  x\\n src\"}", "").as_bytes()),
        Some(Event::Progress("rg x src".into()))
    );
    assert_eq!(
        rollout::event(tool("exec", "", "tools.exec_command({cmd: \"rg x src\"})").as_bytes()),
        Some(Event::Progress("rg x src".into()))
    );
    assert_eq!(
        rollout::event(
            tool(
                "exec",
                "",
                "const cmds = []; tools.exec_command({cmd: \"rg x\"})"
            )
            .as_bytes()
        ),
        Some(Event::Progress("rg x".into()))
    );
    assert!(rollout::event(tool("exec", "", "calls.map(run)").as_bytes()).is_none());
    assert_eq!(
        rollout::event(tool("read_file", "", "").as_bytes()),
        Some(Event::Progress("read_file".into()))
    );
}
#[test]
fn delayed_creation_partial_records_and_invisible_records_advance_without_replay() {
    let root = Root::new();
    let mut tail = Tail::new(&root.0, ID).unwrap();
    assert!(!tail.poll().unwrap().advanced);
    let directory = root.0.join("sessions/2026/10/10");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("rollout-synthetic-{ID}.jsonl"));
    let record=json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"**One**"}]}}).to_string();
    std::fs::write(&path, &record).unwrap();
    assert!(!tail.poll().unwrap().advanced);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"\n").unwrap();
    let next = tail.poll().unwrap();
    assert!(next.advanced);
    assert_eq!(next.events, vec![Event::Activity("One".into())]);
    assert!(!tail.poll().unwrap().advanced);
    file.write_all(b"{\"type\":\"unrendered\"}\n").unwrap();
    let next = tail.poll().unwrap();
    assert!(next.advanced);
    assert!(next.events.is_empty());
    std::fs::write(
        directory.join("rollout-other-ffffffff-ffff-ffff-ffff-ffffffffffff.jsonl"),
        b"private unrelated content",
    )
    .unwrap();
    assert!(!tail.poll().unwrap().advanced);
}

#[cfg(unix)]
#[test]
fn silent_child_stays_live_from_unrendered_rollout_but_hard_deadline_still_wins() {
    use revust_core::{
        request::Request,
        runtime::{Limits, MonotonicClock, RunControl},
    };
    use revust_executors::{
        codex,
        configured::Settings,
        supervision::{self, Termination},
    };
    use std::{collections::BTreeMap, sync::Arc, time::Duration};
    for hard in [false, true] {
        let root = Root::new();
        let directory = root.0.join("sessions/2026/10/10");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("rollout-synthetic-{ID}.jsonl"));
        let control = RunControl::new(
            Limits {
                max_calls: None,
                elapsed: None,
                max_parallel: 1,
                stage_parallel: [1; 3],
            },
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
        let settings = Settings {
            preserve_anthropic_api_key: false,
            program: "/bin/sh".into(),
            environment: BTreeMap::from([("REVUST_ROLLOUT".into(), path.to_str().unwrap().into())]),
            limits: supervision::Limits {
                idle: Some(Duration::from_millis(700)),
                hard: Some(Duration::from_millis(if hard { 1100 } else { 3000 })),
            },
        };
        let script = r#"printf 'session id: 01234567-89ab-cdef-0123-456789abcdef\n' >&2
n=0
while [ "$n" -lt 4 ]; do
 printf '%s\n' '{"type":"event_msg","payload":{}}' >> "$REVUST_ROLLOUT"
 /bin/sleep 0.35
 n=$((n+1))
done
printf '%s\n' '{"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"**Final check**"}]}}' >> "$REVUST_ROLLOUT"
printf '%s' '{"findings":[]}'
"#;
        let mut events = vec![];
        let result = codex::run_with_rollout(
            &settings,
            Request {
                program: "unused".into(),
                argv: vec!["-c".into(), script.into()],
                stdin: String::new(),
                cwd: root.0.clone(),
            },
            &control,
            Some(&root.0),
            |_, _| Ok(()),
            |event| events.push(event),
        )
        .unwrap();
        assert_eq!(
            result.process.termination,
            if hard {
                Termination::HardTimeout
            } else {
                Termination::Exited
            }
        );
        if !hard {
            assert_eq!(result.protocol.output, Some(json!({"findings":[]})));
            assert!(events.contains(&Event::Activity("Final check".into())));
        }
    }
}

#[test]
fn aliases_cannot_select_outside_evidence_and_replaced_files_do_not_replay() {
    use std::os::unix::fs::symlink;
    let root = Root::new();
    let outside = root.0.join("outside");
    std::fs::create_dir_all(outside.join("2026/10/10")).unwrap();
    let filename = format!("rollout-owned-{ID}.jsonl");
    let record = json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"External"}]}}).to_string()+"\n";
    std::fs::write(outside.join("2026/10/10").join(&filename), &record).unwrap();
    symlink("outside", root.0.join("sessions")).unwrap();
    let mut tail = Tail::new(&root.0, ID).unwrap();
    assert!(!tail.poll().unwrap().advanced);
    std::fs::remove_file(root.0.join("sessions")).unwrap();
    let directory = root.0.join("sessions/2026/10/10");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(filename);
    std::fs::write(&path, &record).unwrap();
    assert!(tail.poll().unwrap().advanced);
    std::fs::rename(&path, directory.join("old")).unwrap();
    std::fs::write(&path, &record).unwrap();
    assert!(!tail.poll().unwrap().advanced);
}

#[test]
fn unterminated_large_records_yield_and_cannot_refresh_idle() {
    let root = Root::new();
    let directory = root.0.join("sessions/2026/10/10");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("rollout-bounded-{ID}.jsonl"));
    std::fs::write(path, vec![b'x'; 9 << 20]).unwrap();
    let mut tail = Tail::new(&root.0, ID).unwrap();
    let started = std::time::Instant::now();
    assert!(!tail.poll().unwrap().advanced);
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    let mut refused = false;
    for _ in 0..12 {
        if tail.poll().is_err() {
            refused = true;
            break;
        }
    }
    assert!(refused);
}

#[test]
fn bounded_record_batches_keep_the_cursor_and_refuse_truncation() {
    let root = Root::new();
    let directory = root.0.join("sessions/2026/10/10");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("rollout-batches-{ID}.jsonl"));
    let records: String = (0..1100).map(|index| json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"text":index.to_string()}]}}).to_string()+"\n").collect();
    std::fs::write(&path, records).unwrap();
    let mut tail = Tail::new(&root.0, ID).unwrap();
    let first = tail.poll().unwrap();
    let second = tail.poll().unwrap();
    assert_eq!(first.events.len(), 1024);
    assert_eq!(second.events.len(), 76);
    assert_eq!(second.events[0], Event::Activity("1024".into()));
    assert!(!tail.poll().unwrap().advanced);
    std::fs::write(path, b"short").unwrap();
    assert!(tail.poll().is_err());
}
