#!/usr/bin/env python3
"""Opt-in actual Claude CLI through revust and a loopback Messages peer; no models."""
import hashlib
import http.server
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import threading
import time
import unittest

from codex_cli_checks import descendants, process_table

ROOT = Path(__file__).resolve().parents[2]


def input_text(body):
    for message in body["messages"]:
        if message["role"] != "user":
            continue
        content = message["content"]
        texts = [content] if isinstance(content, str) else [part["text"] for part in content
                                                          if part.get("type") == "text"]
        for text in texts:
            if text.splitlines() and text.splitlines()[0] in ("FIND", "SYNTHESIS", "VERIFY"):
                return text
    raise AssertionError("stage prompt missing")


def reply(prompt):
    stage = prompt.splitlines()[0]
    if stage == "FIND":
        return {"findings": [{"file": "src/example.rs", "line": 1, "severity": "major",
                "confidence": 95, "title": "Synthetic Claude finding", "body": "Synthetic evidence.",
                "lenses": ["bugs"]}]}
    findings = json.loads(prompt.split("FINDINGS_BEGIN\n", 1)[1].split("\nFINDINGS_END", 1)[0])
    if stage == "SYNTHESIS":
        item = {k: findings[0][k] for k in ("file", "line", "severity", "confidence", "title", "body")}
        item["merged_ids"] = [f["id"] for f in findings]
        return {"findings": [item], "open_questions": [], "pre_existing": []}
    if stage == "VERIFY":
        return {"verdicts": [{"id": f["id"], "verdict": "confirmed"} for f in findings]}
    raise AssertionError("unexpected stage: " + stage)


def message_events(output, model, tool="StructuredOutput", tool_id="toolu_fixture"):
    return [
        {"type": "message_start", "message": {"id": "msg_fixture", "type": "message",
         "role": "assistant", "content": [], "model": model, "stop_reason": None,
         "stop_sequence": None, "usage": {"input_tokens": 10, "output_tokens": 0}}},
        {"type": "content_block_start", "index": 0, "content_block": {
         "type": "tool_use", "id": tool_id, "name": tool, "input": {}}},
        {"type": "content_block_delta", "index": 0, "delta": {
         "type": "input_json_delta", "partial_json": json.dumps(output)}},
        {"type": "content_block_stop", "index": 0},
        {"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": None},
         "usage": {"output_tokens": 5}},
        {"type": "message_stop"},
    ]


@unittest.skipUnless(os.environ.get("REVUST_CLAUDE"), "set REVUST_CLAUDE to the pinned CLI; no model calls")
class ClaudeCLI(unittest.TestCase):
    def test_actual_cli_completes_all_stages_and_saved_view_is_read_only(self):
        self.observe(cancel=False)

    def test_cancel_stops_actual_cli_and_does_not_restart_the_request(self):
        self.observe(cancel=True)

    def test_actual_verifier_reads_unchanged_source_outside_prompt_packet(self):
        self.observe(cancel=False, read_source=True)

    def observe(self, cancel, read_source=False):
        claude = Path(os.environ["REVUST_CLAUDE"]).resolve(strict=True)
        identity = next(item for item in json.loads((ROOT / "docs/executor-protocols.json").read_text())["executors"]
                        if item["name"] == "claude")
        self.assertEqual(hashlib.sha256(claude.read_bytes()).hexdigest(), identity["binary_sha256"],
                         "requalify changed Claude before execution")
        binary = Path(os.environ.get("REVUST_BIN", ROOT / "target/debug/revust")).resolve(strict=True)
        requests, errors = [], []
        arrived, disconnected = threading.Event(), threading.Event()
        own = set()
        child = None

        class Peer(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                body = {}
                try:
                    if self.path.split("?", 1)[0] != "/v1/messages":
                        raise AssertionError("unexpected endpoint: " + self.path)
                    if self.headers.get("x-api-key") != "not-a-credential":
                        raise AssertionError("expected synthetic authorization")
                    body = json.loads(self.rfile.read(int(self.headers["content-length"])))
                    if child is not None:
                        own.update(descendants(child.pid))
                    requests.append(body)
                    arrived.set()
                    if cancel:
                        self.connection.settimeout(8)
                        try:
                            if self.connection.recv(1) != b"":
                                raise AssertionError("unexpected data after request")
                        except ConnectionResetError:
                            pass
                        disconnected.set()
                        return
                    prompt = input_text(body)
                    if "StructuredOutput" not in [t.get("name") for t in body.get("tools", [])]:
                        raise AssertionError("missing StructuredOutput tool; request keys=" + str(list(body)))
                    if read_source and prompt.splitlines()[0] == "VERIFY" and len(requests) == 3:
                        if "Read" not in [t.get("name") for t in body["tools"]]:
                            raise AssertionError("missing Read tool")
                        events = message_events({"file_path": str(project / "src/helper.rs")},
                                                body["model"], "Read", "toolu_fixture_read")
                    else:
                        events = message_events(reply(prompt), body["model"])
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Connection", "close")
                    self.end_headers()
                    for event in events:
                        self.wfile.write(("event: " + event["type"] + "\ndata: " +
                                          json.dumps(event) + "\n\n").encode())
                    self.wfile.flush()
                except Exception as error:
                    errors.append(str(error) + "; request shape=" + json.dumps({
                        "keys": list(body), "tools": [t.get("name") for t in body.get("tools", [])],
                        "messages": [{"role": m.get("role"), "content_type": type(m.get("content")).__name__,
                                      "hint": str(m.get("content"))[:160]} for m in body.get("messages", [])]
                    }, ensure_ascii=False)[:2500])
                    arrived.set()

            def do_GET(self):
                errors.append("unexpected GET " + self.path)
                self.send_error(404)

        with tempfile.TemporaryDirectory(prefix="revust-claude-cli-") as directory:
            root = Path(directory).resolve()
            project, home, claude_home = root / "project", root / "home", root / "claude"
            for p in (project, home, claude_home):
                p.mkdir()
            (project / "src").mkdir()
            (project / "src/example.rs").write_text("// Synthetic unchanged context.\n")
            if read_source:
                (project / "src/helper.rs").write_text("// CLAUDE_UNCHANGED_HELPER_271828\n")
            config = project / ".revust"
            (config / "prompts/profiles").mkdir(parents=True)
            (config / "prompts/profiles/fixture.md").write_text(
                "---\nmodel: claude/sonnet:high\nagents:\n  - name: observer\n    lenses: [bugs]\n"
                "stages:\n  synthesis: claude/sonnet:low\n  verify: claude/sonnet:medium\n---\nFIND\n{{SCOPE}}\n")
            (config / "prompts/synthesis.md").write_text("SYNTHESIS\nFINDINGS_BEGIN\n{{FINDINGS}}\nFINDINGS_END\n")
            (config / "prompts/verify.md").write_text("VERIFY\nFINDINGS_BEGIN\n{{FINDINGS}}\nFINDINGS_END\n")
            archive = root / "tasks/fixture/01"
            (archive / "input").mkdir(parents=True)
            (archive / "input/scope.md").write_text("Synthetic scope: тест.\n")
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Peer)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            (config / "extensions.json").write_text(json.dumps({"version": 1, "policy": {
                "retries": {name: {"count": 0} for name in ("find", "synthesis", "verify")},
                "budget": {"max_calls": 3, "elapsed": "20s"},
                "executors": {"claude": {"executable": str(claude), "env": {
                    "ANTHROPIC_API_KEY": "not-a-credential",
                    "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{server.server_port}",
                }}}}}))
            watched = [project, archive / "input"]
            snapshot = lambda p: {str(f.relative_to(p)): f.read_bytes() for f in p.rglob("*") if f.is_file()}
            before = [snapshot(p) for p in watched]
            env = {"HOME": str(home), "CLAUDE_CONFIG_DIR": str(claude_home), "PATH": "/usr/bin:/bin",
                   "TMPDIR": str(root), "LANG": "C", "TERM": "dumb",
                   "CLAUDE_CODE_SIMPLE": "1", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
                   "HTTP_PROXY": "http://127.0.0.1:9", "HTTPS_PROXY": "http://127.0.0.1:9",
                   "NO_PROXY": "127.0.0.1,localhost"}
            command = [str(binary), "--task=fixture", "--run=01", "--profile=fixture",
                       "--tasks-dir=" + str(root / "tasks"), "--no-tui", "--max-parallel=1",
                       "--stagger-delay=0s", "--idle-timeout=10s", "--hard-timeout=15s"]
            child = subprocess.Popen(command, cwd=project, env=env, start_new_session=True,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            own.add(child.pid)
            try:
                if not arrived.wait(10):
                    child.send_signal(signal.SIGTERM) if child.poll() is None else None
                    stdout, stderr = child.communicate(timeout=5)
                    self.fail("actual Claude never reached the loopback peer: " + stderr.decode(errors="replace"))
                self.assertEqual(errors, [])
                own.update(descendants(child.pid))
                if cancel:
                    self.assertGreater(len(own), 1, "actual Claude descendant not observable")
                    child.send_signal(signal.SIGTERM)
                stdout, stderr = child.communicate(timeout=20)
                self.assertEqual(errors, [])
                self.assertEqual([snapshot(p) for p in watched], before)
                if cancel:
                    self.assertEqual(child.returncode, 2, stderr.decode(errors="replace"))
                    self.assertEqual(stdout, b"")
                    self.assertTrue(disconnected.wait(3), "cancelled provider connection remained open")
                    self.assertEqual(len(requests), 1)
                    self.assertEqual((archive / "manifest.json").read_bytes(), b"")
                else:
                    self.assertEqual(child.returncode, 1, stderr.decode(errors="replace"))
                    report = json.loads(stdout)
                    self.assertEqual(report["sources"]["reported"], 1)
                    self.assertEqual(report["sources"]["degraded"], [])
                    self.assertEqual(report["sources"]["agents"][0]["actual_model"], requests[0]["model"])
                    self.assertEqual(report["findings"][0]["verdict"], "confirmed")
                    finder_events = [json.loads(line) for line in (archive / "agents/observer.jsonl").read_text().splitlines()]
                    init = next(e for e in finder_events if e.get("type") == "system" and e.get("subtype") == "init")
                    self.assertEqual(init["permissionMode"], "auto")
                    final = next(e for e in finder_events if e.get("type") == "result")
                    self.assertEqual(final["structured_output"], reply(input_text(requests[0])))
                    self.assertFalse(final["is_error"])
                    self.assertEqual(len(requests), 4 if read_source else 3)
                    initial_requests = requests[:3]
                    self.assertEqual([input_text(r).splitlines()[0] for r in initial_requests], ["FIND", "SYNTHESIS", "VERIFY"])
                    self.assertEqual([r["output_config"]["effort"] for r in initial_requests], ["high", "low", "medium"])
                    if read_source:
                        results = [part for message in requests[3]["messages"]
                                   if isinstance(message["content"], list) for part in message["content"]
                                   if part.get("type") == "tool_result"
                                   and part.get("tool_use_id") == "toolu_fixture_read"]
                        self.assertEqual(len(results), 1)
                        self.assertFalse(results[0].get("is_error", False))
                        self.assertIn("CLAUDE_UNCHANGED_HELPER_271828", str(results[0]["content"]))
                        self.assertEqual(requests[3]["output_config"]["effort"], "medium")
                    for body, stage in zip(initial_requests, ("finder", "synthesis", "verify")):
                        tool = next(t for t in body["tools"] if t.get("name") == "StructuredOutput")
                        schema = json.loads((ROOT / f"crates/core/schemas/{stage}.json").read_text())
                        self.assertEqual(tool["input_schema"], schema)
                    archived_prompts = [p.read_text() for p in (archive / "prompts").rglob("*.md")]
                    self.assertEqual(sorted(input_text(r) for r in initial_requests), sorted(archived_prompts))
                    if read_source:
                        for prompt in archived_prompts:
                            self.assertNotIn("CLAUDE_UNCHANGED_HELPER_271828", prompt)
                    saved = snapshot(archive)
                    view = subprocess.run([str(binary), "view", "--archive", str(archive), "--no-tui"],
                                          cwd=root, env={"HOME": str(home), "PATH": "/nonexistent"},
                                          capture_output=True, timeout=10)
                    self.assertEqual(view.returncode, 0)
                    self.assertEqual(json.loads(view.stdout), report)
                    self.assertEqual(snapshot(archive), saved)
                    self.assertEqual(len(requests), 4 if read_source else 3)
                for body in requests:
                    # The pinned CLI resolves the legacy sonnet alias locally.
                    self.assertEqual(body["model"], "claude-sonnet-5-5")
                    self.assertTrue(body["stream"])
                deadline = time.monotonic() + 3
                while any(pid in own and not state.startswith("Z") for pid, _, state in process_table()):
                    self.assertLess(time.monotonic(), deadline, "owned CLI/descendant survived")
                    time.sleep(0.02)
            finally:
                if child.poll() is None:
                    own.update(descendants(child.pid))
                    child.send_signal(signal.SIGTERM)
                    try:
                        child.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        pass
                for pid in own:
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                child.wait(timeout=3)
                server.shutdown()
                server.server_close()
                thread.join(timeout=3)


if __name__ == "__main__":
    unittest.main(verbosity=2)
