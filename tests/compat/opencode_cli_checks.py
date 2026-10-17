#!/usr/bin/env python3
"""Opt-in actual OpenCode through public revust and a local scripted API."""
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

from claude_cli_checks import reply
from codex_cli_checks import descendants, process_table

ROOT = Path(__file__).resolve().parents[2]


def input_text(body):
    for message in body["messages"]:
        if message["role"] == "user":
            content = message["content"]
            texts = [content] if isinstance(content, str) else [p["text"] for p in content
                                                               if p.get("type") == "text"]
            for text in texts:
                if text.splitlines() and text.splitlines()[0] in ("FIND", "SYNTHESIS", "VERIFY"):
                    return text
    raise AssertionError("missing stage prompt")


@unittest.skipUnless(os.environ.get("REVUST_OPENCODE"), "set REVUST_OPENCODE; no model calls")
class OpenCodeCLI(unittest.TestCase):
    def test_actual_cli_completes_all_stages_and_saved_view_is_read_only(self):
        self.observe(cancel=False)

    def test_cancel_stops_actual_cli_without_restarting_the_request(self):
        self.observe(cancel=True)

    def observe(self, cancel):
        executable = Path(os.environ["REVUST_OPENCODE"]).resolve(strict=True)
        identity = next(x for x in json.loads((ROOT / "docs/executor-protocols.json").read_text())["executors"]
                        if x["name"] == "opencode")
        self.assertEqual(hashlib.sha256(executable.read_bytes()).hexdigest(), identity["binary_sha256"],
                         "requalify changed OpenCode before execution")
        binary = Path(os.environ.get("REVUST_BIN", ROOT / "target/debug/revust")).resolve(strict=True)
        requests, errors, owned = [], [], set()
        arrived, disconnected = threading.Event(), threading.Event()
        child = None

        class Peer(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                try:
                    if self.path != "/v1/chat/completions":
                        raise AssertionError("unexpected endpoint: " + self.path)
                    if self.headers.get("authorization") != "Bearer not-a-credential":
                        raise AssertionError("expected synthetic authorization")
                    body = json.loads(self.rfile.read(int(self.headers["content-length"])))
                    owned.update(descendants(child.pid))
                    requests.append(body)
                    arrived.set()
                    if cancel:
                        self.connection.settimeout(8)
                        try:
                            if self.connection.recv(1) != b"":
                                raise AssertionError("unexpected pending-request data")
                        except ConnectionResetError:
                            pass
                        disconnected.set()
                        return
                    output = json.dumps(reply(input_text(body)))
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Connection", "close")
                    self.end_headers()
                    for delta, finish in [({"role": "assistant", "content": output}, None), ({}, "stop")]:
                        chunk = {"id": "synthetic-response", "object": "chat.completion.chunk",
                                 "created": 1, "model": "synthetic-model", "choices": [
                                     {"index": 0, "delta": delta, "finish_reason": finish}]}
                        if finish:
                            chunk["usage"] = {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
                        self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
                    self.wfile.write(b"data: [DONE]\n\n")
                    self.wfile.flush()
                except Exception as error:
                    errors.append(str(error))
                    arrived.set()

            def do_GET(self):
                errors.append("unexpected GET: " + self.path)
                self.send_error(404)

        with tempfile.TemporaryDirectory(prefix="revust-opencode-cli-") as directory:
            root = Path(directory).resolve()
            project, home = root / "project", root / "home"
            project.mkdir()
            home.mkdir()
            (project / "src").mkdir()
            (project / "src/example.rs").write_text("// Synthetic unchanged context.\n")
            config = project / ".revust"
            (config / "extension-profiles").mkdir(parents=True)
            (config / "prompts").mkdir()
            (config / "extension-profiles/fixture.md").write_text(
                "---\nrunner:\n  executor: opencode\n  model: revust-stub/synthetic-model\n"
                "agents:\n  - name: observer\n    lenses: [bugs]\n---\nFIND\n{{SCOPE}}\n")
            (config / "prompts/synthesis.md").write_text("SYNTHESIS\nFINDINGS_BEGIN\n{{FINDINGS}}\nFINDINGS_END\n")
            (config / "prompts/verify.md").write_text("VERIFY\nFINDINGS_BEGIN\n{{FINDINGS}}\nFINDINGS_END\n")
            archive = root / "tasks/fixture/01"
            (archive / "input").mkdir(parents=True)
            (archive / "input/scope.md").write_text("Synthetic scope: тест.\n")
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Peer)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            providers = {"revust-stub": {"name": "Synthetic fixture", "env": ["REVUST_SYNTHETIC_KEY"],
                "package": "@opencode/ai/providers/openai-compatible",
                "settings": {"baseURL": f"http://127.0.0.1:{server.server_port}/v1"},
                "models": {"synthetic-model": {}}}}
            (config / "extensions.json").write_text(json.dumps({"version": 1,
                "profiles": {"fixture": {"file": "extension-profiles/fixture.md"}}, "policy": {
                "retries": {s: {"count": 0} for s in ("find", "synthesis", "verify")},
                "budget": {"max_calls": 3, "elapsed": "20s"},
                "executors": {"opencode": {"executable": str(executable), "env": {
                    "REVUST_SYNTHETIC_KEY": "not-a-credential",
                    "OPENCODE_CONFIG_CONTENT": json.dumps({"providers": providers})}}}}}))
            snapshot = lambda p: {str(f.relative_to(p)): f.read_bytes() for f in p.rglob("*") if f.is_file()}
            watched = [project, archive / "input"]
            before = [snapshot(p) for p in watched]
            env = {"HOME": str(home), "PATH": "/usr/bin:/bin", "TMPDIR": str(root), "LANG": "C",
                   "TERM": "dumb", "HTTP_PROXY": "http://127.0.0.1:9",
                   "HTTPS_PROXY": "http://127.0.0.1:9", "NO_PROXY": "127.0.0.1,localhost"}
            command = [str(binary), "--task=fixture", "--run=01", "--profile=fixture",
                       "--tasks-dir=" + str(root / "tasks"), "--no-tui", "--max-parallel=1",
                       "--stagger-delay=0s", "--idle-timeout=10s", "--hard-timeout=15s"]
            child = subprocess.Popen(command, cwd=project, env=env, start_new_session=True,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            owned.add(child.pid)
            try:
                if not arrived.wait(10):
                    if child.poll() is None:
                        child.send_signal(signal.SIGTERM)
                    stdout, stderr = child.communicate(timeout=5)
                    self.fail("OpenCode did not reach local peer: " + stderr.decode(errors="replace"))
                self.assertEqual(errors, [])
                owned.update(descendants(child.pid))
                if cancel:
                    self.assertGreater(len(owned), 1)
                    child.send_signal(signal.SIGTERM)
                stdout, stderr = child.communicate(timeout=20)
                self.assertEqual(errors, [])
                self.assertEqual([snapshot(p) for p in watched], before)
                if cancel:
                    self.assertEqual(child.returncode, 2, stderr.decode(errors="replace"))
                    self.assertEqual(stdout, b"")
                    self.assertTrue(disconnected.wait(3))
                    self.assertEqual(len(requests), 1)
                    self.assertEqual((archive / "manifest.json").read_bytes(), b"")
                else:
                    self.assertEqual(child.returncode, 1, stderr.decode(errors="replace"))
                    report = json.loads(stdout)
                    self.assertEqual(report["sources"]["reported"], 1)
                    self.assertEqual(report["sources"]["degraded"], [])
                    self.assertEqual(report["findings"][0]["verdict"], "confirmed")
                    self.assertEqual(len(requests), 3)
                    self.assertEqual([input_text(r).splitlines()[0] for r in requests], ["FIND", "SYNTHESIS", "VERIFY"])
                    prompts = [p.read_text() for p in (archive / "prompts").rglob("*.md")]
                    self.assertEqual(sorted(input_text(r) for r in requests), sorted(prompts))
                    saved = snapshot(archive)
                    view = subprocess.run([str(binary), "view", "--archive", str(archive), "--no-tui"],
                                          cwd=root, env={"HOME": str(home), "PATH": "/nonexistent"},
                                          capture_output=True, timeout=10)
                    self.assertEqual(view.returncode, 0, view.stderr)
                    self.assertEqual(json.loads(view.stdout), report)
                    self.assertEqual(snapshot(archive), saved)
                    self.assertEqual(len(requests), 3)
                for body in requests:
                    self.assertEqual(body["model"], "synthetic-model")
                    self.assertTrue(body["stream"])
                    self.assertEqual({t["function"]["name"] for t in body["tools"]}, {"read", "glob", "grep"})
                deadline = time.monotonic() + 3
                while any(pid in owned and not state.startswith("Z") for pid, _, state in process_table()):
                    self.assertLess(time.monotonic(), deadline, "owned process survived")
                    time.sleep(0.02)
            finally:
                for pid in owned:
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
