#!/usr/bin/env python3
"""Opt-in actual Codex CLI through revust and a loopback Responses peer; no models."""
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

ROOT = Path(__file__).resolve().parents[2]


def process_table():
    result = subprocess.run(["/bin/ps", "-axo", "pid=,ppid=,stat="],
                            capture_output=True, text=True, check=True, timeout=5)
    return [(int(pid), int(parent), state) for pid, parent, state in
            (line.split() for line in result.stdout.splitlines())]


def descendants(parent):
    table = process_table()
    own = {parent}
    while True:
        grown = own | {pid for pid, ppid, _ in table if ppid in own}
        if grown == own:
            return own
        own = grown


def input_text(body):
    # Codex inserts a separate user environment message before the actual stdin.
    messages = ["\n".join(part.get("text", "") for part in item.get("content", [])
                          if isinstance(part, dict))
                for item in body["input"] if item.get("role") == "user"]
    return messages[-1]


def reply(prompt):
    stage = prompt.splitlines()[0]
    if stage == "FIND":
        return {"findings": [{"file": "src/example.rs", "line": 1, "severity": "major",
                "confidence": 95, "title": "Synthetic Codex finding", "body": "Synthetic evidence.",
                "lenses": ["bugs"]}]}
    findings = json.loads(prompt.split("FINDINGS_BEGIN\n", 1)[1].split("\nFINDINGS_END", 1)[0])
    if stage == "SYNTHESIS":
        item = {k: findings[0][k] for k in ("file", "line", "severity", "confidence", "title", "body")}
        item["merged_ids"] = [f["id"] for f in findings]
        return {"findings": [item], "open_questions": [], "pre_existing": []}
    if stage == "VERIFY":
        return {"verdicts": [{"id": f["id"], "verdict": "confirmed"} for f in findings]}
    raise AssertionError("unexpected stage: " + stage)


def response_events(output, model):
    text = json.dumps(output)
    item = {"id": "msg_fixture", "type": "message", "role": "assistant", "status": "completed",
            "content": [{"type": "output_text", "text": text, "annotations": []}]}
    response = {"id": "resp_fixture", "object": "response", "created_at": 1,
                "status": "completed", "model": model, "output": [item],
                "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}}
    return [
        {"type": "response.created", "response": dict(response, status="in_progress", output=[])},
        {"type": "response.output_item.added", "output_index": 0,
         "item": dict(item, status="in_progress", content=[])},
        {"type": "response.output_text.delta", "item_id": item["id"], "output_index": 0,
         "content_index": 0, "delta": text},
        {"type": "response.output_item.done", "output_index": 0, "item": item},
        {"type": "response.completed", "response": response},
    ]


def tool_events(command, model, call_id):
    item = {"id": "fc_" + call_id, "type": "function_call", "status": "completed",
            "call_id": call_id, "name": "exec_command",
            "arguments": json.dumps({"cmd": command, "yield_time_ms": 1000, "max_output_tokens": 1000})}
    response = {"id": "resp_" + call_id, "object": "response", "created_at": 1,
                "status": "completed", "model": model, "output": [item],
                "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}}
    return [
        {"type": "response.created", "response": dict(response, status="in_progress", output=[])},
        {"type": "response.output_item.added", "output_index": 0,
         "item": dict(item, status="in_progress", arguments="")},
        {"type": "response.function_call_arguments.delta", "item_id": item["id"],
         "output_index": 0, "delta": item["arguments"]},
        {"type": "response.output_item.done", "output_index": 0, "item": item},
        {"type": "response.completed", "response": response},
    ]


@unittest.skipUnless(os.environ.get("REVUST_CODEX"), "set REVUST_CODEX to the pinned CLI; no model calls")
class CodexCLI(unittest.TestCase):
    def test_actual_cli_completes_all_stages_and_saved_view_is_read_only(self):
        self.observe(cancel=False)

    def test_cancel_stops_actual_cli_and_does_not_restart_the_request(self):
        self.observe(cancel=True)

    def test_actual_verifier_reads_unchanged_source_and_refuses_checkout_write(self):
        self.observe(cancel=False, tools=True)

    def observe(self, cancel, tools=False):
        codex = Path(os.environ["REVUST_CODEX"]).resolve(strict=True)
        identity = next(item for item in json.loads((ROOT / "docs/executor-protocols.json").read_text())["executors"]
                        if item["name"] == "codex")
        self.assertEqual(hashlib.sha256(codex.read_bytes()).hexdigest(), identity["binary_sha256"],
                         "requalify changed Codex before execution")
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
                    if self.path != "/v1/responses":
                        raise AssertionError("unexpected endpoint: " + self.path)
                    if self.headers.get("Authorization") != "Bearer not-a-credential":
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
                    output = reply(prompt)
                    if tools and prompt.splitlines()[0] == "VERIFY" and len(requests) <= 4:
                        available = [t.get("name") for t in body.get("tools", [])]
                        if "exec_command" not in available:
                            raise AssertionError("expected exec_command tool; available=" + str(available))
                        command = ("cat src/helper.rs" if len(requests) == 3 else
                                   "printf synthetic > src/write-probe.txt")
                        events = tool_events(command, body["model"], "fixture_" + str(len(requests)))
                    else:
                        events = response_events(output, body["model"])
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Connection", "close")
                    self.end_headers()
                    for sequence, event in enumerate(events):
                        event["sequence_number"] = sequence
                        self.wfile.write(("event: " + event["type"] + "\ndata: " +
                                          json.dumps(event) + "\n\n").encode())
                    self.wfile.flush()
                except Exception as error:
                    errors.append(str(error) + "; input shapes=" + json.dumps([
                        {"role": item.get("role"), "type": item.get("type"),
                         "content_type": type(item.get("content")).__name__,
                         "content_hint": str(item.get("content"))[:160]}
                        for item in body.get("input", [])], ensure_ascii=False)[:2500])
                    arrived.set()

            def do_GET(self):
                errors.append("unexpected GET " + self.path)
                self.send_error(404)

        with tempfile.TemporaryDirectory(prefix="revust-codex-cli-") as directory:
            root = Path(directory).resolve()
            project, home, codex_home = root / "project", root / "home", root / "codex"
            for p in (project, home, codex_home):
                p.mkdir()
            (project / "src").mkdir()
            (project / "src/example.rs").write_text("// Synthetic unchanged context.\n")
            if tools:
                (project / "src/helper.rs").write_text("// UNCHANGED_HELPER_EVIDENCE_314159\n")
            config = project / ".revust"
            (config / "prompts/profiles").mkdir(parents=True)
            (config / "prompts/profiles/fixture.md").write_text(
                "---\nmodel: codex/gpt-5.4:high\nagents:\n  - name: observer\n    lenses: [bugs]\n"
                "stages:\n  synthesis: codex/gpt-5.4:low\n  verify: codex/gpt-5.4:medium\n---\nFIND\n{{SCOPE}}\n")
            (config / "prompts/synthesis.md").write_text("SYNTHESIS\nFINDINGS_BEGIN\n{{FINDINGS}}\nFINDINGS_END\n")
            (config / "prompts/verify.md").write_text("VERIFY\nFINDINGS_BEGIN\n{{FINDINGS}}\nFINDINGS_END\n")
            (config / "extensions.json").write_text(json.dumps({"version": 1, "policy": {
                "retries": {name: {"count": 0} for name in ("find", "synthesis", "verify")},
                "budget": {"max_calls": 3, "elapsed": "20s"},
                "executors": {"codex": {"executable": str(codex)}}}}))
            archive = root / "tasks/fixture/01"
            (archive / "input").mkdir(parents=True)
            (archive / "input/scope.md").write_text("Synthetic scope: тест.\n")
            watched = [project, archive / "input"]
            snapshot = lambda p: {str(f.relative_to(p)): f.read_bytes() for f in p.rglob("*") if f.is_file()}
            before = [snapshot(p) for p in watched]
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Peer)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            (codex_home / "config.toml").write_text(
                'model_provider = "revust-fixture"\nweb_search = "disabled"\n'
                'cli_auth_credentials_store = "file"\ncheck_for_update_on_startup = false\n'
                '[feedback]\nenabled = false\n[otel]\nexporter = "none"\n'
                'trace_exporter = "none"\nmetrics_exporter = "none"\n'
                '[model_providers.revust-fixture]\nname = "Synthetic fixture"\n'
                f'base_url = "http://127.0.0.1:{server.server_port}/v1"\n'
                'wire_api = "responses"\nenv_key = "REVUST_SYNTHETIC_KEY"\n'
                'requires_openai_auth = false\nsupports_websockets = false\n'
                'request_max_retries = 0\nstream_max_retries = 0\nstream_idle_timeout_ms = 10000\n')
            env = {"HOME": str(home), "CODEX_HOME": str(codex_home), "PATH": "/usr/bin:/bin",
                   "TMPDIR": str(root), "LANG": "C", "TERM": "dumb",
                   "REVUST_SYNTHETIC_KEY": "not-a-credential",
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
                    self.fail("actual Codex never reached the loopback peer: " + stderr.decode(errors="replace"))
                self.assertEqual(errors, [])
                own.update(descendants(child.pid))
                if cancel:
                    self.assertGreater(len(own), 1, "actual Codex descendant not observable")
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
                    self.assertEqual(report["sources"]["agents"][0]["actual_model"], "gpt-5.4")
                    self.assertEqual(report["findings"][0]["verdict"], "confirmed")
                    self.assertEqual(len(requests), 5 if tools else 3)
                    initial_requests = requests[:3] if tools else requests
                    self.assertEqual([input_text(r).splitlines()[0] for r in initial_requests], ["FIND", "SYNTHESIS", "VERIFY"])
                    self.assertEqual([r["reasoning"]["effort"] for r in initial_requests], ["high", "low", "medium"])
                    if tools:
                        outputs = {item["call_id"]: str(item["output"])
                                   for body in requests for item in body["input"]
                                   if item.get("type") == "function_call_output"}
                        self.assertIn("Process exited with code 0", outputs["fixture_3"])
                        self.assertIn("UNCHANGED_HELPER_EVIDENCE_314159", outputs["fixture_3"])
                        self.assertIn("Process exited with code 1", outputs["fixture_4"])
                        self.assertIn("operation not permitted", outputs["fixture_4"].lower())
                        self.assertEqual([r["reasoning"]["effort"] for r in requests],
                                         ["high", "low", "medium", "medium", "medium"])
                        self.assertFalse((project / "src/write-probe.txt").exists())
                    archived_prompts = [p.read_text() for p in (archive / "prompts").rglob("*.md")]
                    self.assertEqual(sorted(input_text(r) for r in initial_requests), sorted(archived_prompts))
                    if tools:
                        for prompt in archived_prompts:
                            self.assertNotIn("UNCHANGED_HELPER_EVIDENCE_314159", prompt)
                    saved = snapshot(archive)
                    view = subprocess.run([str(binary), "view", "--archive", str(archive), "--no-tui"],
                                          cwd=root, env={"HOME": str(home), "PATH": "/nonexistent"},
                                          capture_output=True, timeout=10)
                    self.assertEqual(view.returncode, 0)
                    self.assertEqual(json.loads(view.stdout), report)
                    self.assertEqual(snapshot(archive), saved)
                    self.assertEqual(len(requests), 5 if tools else 3)
                for body in requests:
                    self.assertEqual(body["model"], "gpt-5.4")
                    self.assertTrue(body["stream"])
                self.assertTrue(input_text(requests[0]).startswith("FIND\n"))
                self.assertIn(b"sandbox: read-only", stderr)
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
