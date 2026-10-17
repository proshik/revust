#![cfg(unix)]
//! Opt-in real CLI qualification, using a loopback scripted provider, never a model.
use revust_core::{
    prompts::runner::Runner,
    request,
    runtime::{Limits, MonotonicClock, RunControl, StopReason},
};
use revust_executors::{
    configured::Settings,
    opencode::isolated::Session,
    supervision::{self, Termination},
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn processes() -> Vec<(u32, u32, String)> {
    let result = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,stat="])
        .output()
        .unwrap();
    assert!(result.status.success(), "process inspection unavailable");
    String::from_utf8(result.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
                fields.next()?.into(),
            ))
        })
        .collect()
}
fn descendants() -> BTreeSet<u32> {
    let all = processes();
    let mut own = BTreeSet::from([std::process::id()]);
    loop {
        let n = own.len();
        for (pid, parent, _) in &all {
            if own.contains(parent) {
                own.insert(*pid);
            }
        }
        if own.len() == n {
            break;
        }
    }
    own.remove(&std::process::id());
    own
}
fn read_request(socket: &mut TcpStream) -> io::Result<serde_json::Value> {
    // macOS may inherit the listener's nonblocking mode on accept.
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut bytes = Vec::new();
    let mut byte = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if bytes.len() > 65536 {
            return Err(io::Error::other("oversized fixture headers"));
        }
    }
    let header = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
    let length = header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .ok_or_else(|| io::Error::other("missing content length"))?
        .1
        .trim()
        .parse::<usize>()
        .map_err(io::Error::other)?;
    if length > 2 * 1024 * 1024 {
        return Err(io::Error::other("oversized fixture request"));
    }
    let mut body = vec![0; length];
    socket.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(io::Error::other)
}
#[test]
#[ignore = "requires REVUST_OPENCODE and local sockets/process inspection; no model calls"]
fn real_cli_success_and_cancellation_through_rust_transport() {
    let binary = std::env::var("REVUST_OPENCODE").expect("set REVUST_OPENCODE to qualified binary");
    for cancel in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "revust-real-opencode-{}-{cancel}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let tmp = Temp(root);
        let project = tmp.0.join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("context.txt"), "synthetic unchanged context\n").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let settings=Settings { program:binary.clone(),preserve_anthropic_api_key:false,
            environment:BTreeMap::from([
                ("REVUST_SYNTHETIC_KEY".into(),"not-a-credential".into()),
                ("OPENCODE_CONFIG_CONTENT".into(),json!({"providers":{"revust-stub":{"name":"Synthetic fixture","env":["REVUST_SYNTHETIC_KEY"],"package":"@opencode/ai/providers/openai-compatible","settings":{"baseURL":format!("http://127.0.0.1:{port}/v1")},"models":{"synthetic-model":{}}}}}).to_string())]),
            limits:supervision::Limits {idle:Some(Duration::from_secs(3)),hard:Some(Duration::from_secs(6))},
        };
        let session = Session::prepare(&tmp.0.join("private"), &settings, &json!({})).unwrap();
        let control = Arc::new(
            RunControl::new(
                Limits {
                    max_calls: None,
                    elapsed: None,
                    max_parallel: 1,
                    stage_parallel: [1; 3],
                },
                Arc::new(MonotonicClock::default()),
            )
            .unwrap(),
        );
        let server_control = control.clone();
        let server = std::thread::spawn(move || -> io::Result<BTreeSet<u32>> {
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => return Err(e),
                }
            };
            let input = read_request(&mut socket)?;
            assert_eq!(input["model"], "synthetic-model");
            assert!(
                input["messages"]
                    .to_string()
                    .contains("SYNTHETIC RUST STDIN")
            );
            let owned = descendants();
            if cancel {
                server_control.cancel();
                let mut byte = [0];
                match socket.read(&mut byte) {
                    Ok(0) => (),
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => (),
                    other => {
                        return Err(io::Error::other(format!(
                            "cancelled provider connection result: {other:?}; byte={}",
                            byte[0]
                        )));
                    }
                }
            } else {
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")?;
                for (delta, reason) in [
                    (
                        json!({"role":"assistant","content":"{\"findings\":[]}"}),
                        serde_json::Value::Null,
                    ),
                    (json!({}), json!("stop")),
                ] {
                    let chunk = json!({"id":"synthetic-response","object":"chat.completion.chunk","created":1,"model":"synthetic-model","choices":[{"index":0,"delta":delta,"finish_reason":reason}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}});
                    write!(socket, "data: {chunk}\n\n")?;
                }
                socket.write_all(b"data: [DONE]\n\n")?;
            }
            Ok(owned)
        });
        let req = request::compose(
            &Runner {
                executor: "opencode".into(),
                model: "revust-stub/synthetic-model".into(),
                effort: String::new(),
            },
            "SYNTHETIC RUST STDIN",
            request::FINDER_SCHEMA,
            &project,
            "read-only",
        )
        .unwrap();
        let started = Instant::now();
        let result = session.run(req, &control, |_, _| Ok(()), |_| {});
        let owned = server.join().unwrap().unwrap();
        let result = result.unwrap();
        assert!(started.elapsed() < Duration::from_secs(7));
        assert!(result.process.output_complete, "owned pipe remained open");
        if cancel {
            assert_eq!(
                result.process.termination,
                Termination::RunStopped(StopReason::Cancelled)
            );
            assert!(result.protocol.output.is_none());
        } else {
            assert_eq!(result.process.termination, Termination::Exited);
            assert!(result.process.status.unwrap().success());
            assert_eq!(result.protocol.output, Some(json!({"findings":[]})));
        }
        // Include the known leader even if the process snapshot was unusually sparse.
        assert!(owned.contains(&result.process.pid.unwrap()));
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let alive = processes()
                .into_iter()
                .any(|(pid, _, state)| owned.contains(&pid) && !state.starts_with('Z'));
            if !alive {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "owned OpenCode descendant survived completion"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(session);
        assert!(!tmp.0.join("private").exists());
        assert_eq!(
            fs::read_to_string(project.join("context.txt")).unwrap(),
            "synthetic unchanged context\n"
        );
    }
}
