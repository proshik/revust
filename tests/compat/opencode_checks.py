#!/usr/bin/env python3
"""Opt-in, credential-free OpenCode protocol observation; never calls a model."""
import hashlib
import http.server
import threading
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
METADATA = ROOT / "docs/executor-protocols.json"
FIXTURE = ROOT / "crates/executors/tests/fixtures/opencode-v2021-unavailable.json"


@unittest.skipUnless(os.environ.get("REVUST_OPENCODE"), "set REVUST_OPENCODE to the pinned binary")
class OpenCodeProtocol(unittest.TestCase):
    def test_standalone_unavailable_model_is_json_error(self):
        binary = Path(os.environ["REVUST_OPENCODE"]).resolve(strict=True)
        expected = next(x for x in json.loads(METADATA.read_text())["executors"]
                        if x["name"] == "opencode")
        self.assertEqual(hashlib.sha256(binary.read_bytes()).hexdigest(),
                         expected["binary_sha256"], "requalify changed OpenCode before running")
        with tempfile.TemporaryDirectory(prefix="revust-opencode-") as directory:
            root = Path(directory)
            # Do not inherit credentials, server URLs, provider or project settings.
            env = {"PATH": "/usr/bin:/bin", "HOME": directory, "TMPDIR": directory,
                   "OPENCODE_TEST_HOME": directory,
                   "OPENCODE_DISABLE_AUTOUPDATE": "1",
                   "OPENCODE_DISABLE_MODELS_FETCH": "1",
                   "OPENCODE_CONFIG_PROJECT_DISABLE": "1"}
            for key in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"):
                env[key] = str(root / key)
            env["OPENCODE_CONFIG_CONTENT"] = json.dumps({
                "update": "disable",
                "agents": {"revust-probe": {"mode": "primary", "description": "Synthetic probe",
                    "permissions": [{"action": "*", "resource": "*", "effect": "deny"}]}}})
            # This provider is deliberately absent. No inference endpoint exists.
            command = [str(binary), "run", "--standalone", "--format", "json",
                       "--agent", "revust-probe", "--model", "revust-nonexistent-provider/no-model",
                       "Synthetic protocol probe; must fail before inference."]
            child = subprocess.Popen(command, cwd=root, env=env, start_new_session=True,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                stdout, stderr = child.communicate(timeout=15)
            finally:
                # Own group only; never discover or stop a user background service.
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait(timeout=3)
            self.assertEqual(child.returncode, 1)
            self.assertEqual(stderr, b"")
            records = [json.loads(line) for line in stdout.splitlines()]
            self.assertEqual(len(records), 1)
            record = records[0]
            self.assertIsInstance(record.pop("timestamp"), int)
            self.assertTrue(record.pop("sessionID").startswith("ses_"))
            self.assertEqual(record, json.loads(FIXTURE.read_text()))

    def test_read_only_tools_against_local_provider(self):
        binary = Path(os.environ["REVUST_OPENCODE"]).resolve(strict=True)
        expected = next(x for x in json.loads(METADATA.read_text())["executors"]
                        if x["name"] == "opencode")
        self.assertEqual(hashlib.sha256(binary.read_bytes()).hexdigest(), expected["binary_sha256"])
        attempts = [("read", {"path": "context.txt"}),
                    ("read", {"path": "../dependency.txt"}),
                    ("write", {"path": "../dependency.txt", "content": "MUTATED"}),
                    ("write", {"path": "context.txt", "content": "MUTATED"}),
                    ("edit", {"path": "context.txt", "oldString": "CONTEXT", "newString": "MUTATED"}),
                    ("patch", {"patch": "*** Begin Patch\n*** Delete File: context.txt\n*** End Patch"}),
                    ("shell", {"command": "printf MUTATED > context.txt"}),
                    ("subagent", {"agent": "general", "prompt": "Synthetic rejected request"}),
                    ("execute", {"code": "throw new Error('must not run')"})]
        requests = []

        class Stub(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers["content-length"])))
                requests.append(request)
                index = len(requests) - 1
                if index < len(attempts):
                    name, arguments = attempts[index]
                    delta = {"role": "assistant", "tool_calls": [{"index": 0,
                        "id": f"synthetic-tool-{index}", "type": "function",
                        "function": {"name": name, "arguments": json.dumps(arguments)}}]}
                    finish = "tool_calls"
                else:
                    delta = {"role": "assistant", "content": '{"findings":[]}'}
                    finish = "stop"
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                for part, reason in [(delta, None), ({}, finish)]:
                    chunk = {"id": "synthetic-response", "object": "chat.completion.chunk",
                             "created": 1, "model": "synthetic-model",
                             "choices": [{"index": 0, "delta": part, "finish_reason": reason}]}
                    if reason:
                        chunk["usage"] = {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
                    self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
                self.wfile.write(b"data: [DONE]\n\n")
                self.wfile.flush()

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Stub)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory(prefix="revust-opencode-tools-") as directory:
                root = Path(directory)
                project = root / "project"
                project.mkdir()
                dependency = root / "dependency.txt"
                dependency.write_text("SYNTHETIC EXTERNAL DEPENDENCY\n")
                (project / "context.txt").write_text("SYNTHETIC UNCHANGED CONTEXT\n")
                # Negative control: project discovery must not replace the owned agent.
                (project / "opencode.json").write_text(json.dumps({"agents": {"revust-probe": {
                    "system": "UNEXPECTED PROJECT CONFIG", "permissions": [
                        {"action": "*", "resource": "*", "effect": "allow"}]}}}))
                before = {p.relative_to(project): p.read_bytes() for p in project.rglob("*") if p.is_file()}
                env = {"PATH": "/usr/bin:/bin", "HOME": directory, "TMPDIR": directory,
                       "OPENCODE_TEST_HOME": directory, "OPENCODE_DISABLE_AUTOUPDATE": "1",
                       "OPENCODE_DISABLE_MODELS_FETCH": "1", "OPENCODE_CONFIG_PROJECT_DISABLE": "1",
                       "REVUST_SYNTHETIC_KEY": "not-a-credential"}
                for key in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"):
                    env[key] = str(root / key)
                permissions = [{"action": "*", "resource": "*", "effect": "deny"}]
                permissions += [{"action": a, "resource": "*", "effect": "allow"}
                                for a in ("read", "glob", "grep", "external_directory")]
                env["OPENCODE_CONFIG_CONTENT"] = json.dumps({"update": "disable", "agents": {
                    "revust-probe": {"mode": "primary", "description": "Synthetic probe",
                                     "permissions": permissions}}, "providers": {"revust-stub": {
                    "name": "Local synthetic fixture", "env": ["REVUST_SYNTHETIC_KEY"],
                    "package": "@opencode/ai/providers/openai-compatible",
                    "settings": {"baseURL": f"http://127.0.0.1:{server.server_port}/v1"},
                    "models": {"synthetic-model": {"name": "Synthetic model"}}}}})
                command = [str(binary), "run", "--standalone", "--format", "json",
                           "--title", "Synthetic review", "--agent", "revust-probe",
                           "--model", "revust-stub/synthetic-model"]
                child = subprocess.Popen(command, cwd=project, env=env, start_new_session=True,
                                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                try:
                    stdout, stderr = child.communicate(input="Return a JSON review.\nSYNTHETIC STDIN: тест\n".encode(), timeout=20)
                finally:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    child.wait(timeout=3)
                self.assertEqual(child.returncode, 0, stderr.decode(errors="replace"))
                self.assertEqual(stderr, b"")
                self.assertEqual(len(requests), len(attempts) + 1)
                self.assertIn("SYNTHETIC STDIN: тест", json.dumps(requests[0]["messages"], ensure_ascii=False))
                for request in requests:
                    self.assertEqual(request["model"], "synthetic-model")
                    self.assertEqual({t["function"]["name"] for t in request["tools"]}, {"read", "glob", "grep"})
                    self.assertNotIn("UNEXPECTED PROJECT CONFIG", json.dumps(request))
                events = [json.loads(line) for line in stdout.splitlines()]
                tools = [e["part"] for e in events if e["type"] == "tool_use"]
                self.assertEqual([p["tool"] for p in tools], [name for name, _ in attempts])
                self.assertEqual(tools[0]["state"]["status"], "completed")
                self.assertIn("SYNTHETIC UNCHANGED CONTEXT", tools[0]["state"]["output"])
                self.assertEqual(tools[1]["state"]["status"], "completed")
                self.assertIn("SYNTHETIC EXTERNAL DEPENDENCY", tools[1]["state"]["output"])
                self.assertEqual(dependency.read_text(), "SYNTHETIC EXTERNAL DEPENDENCY\n")
                for tool in tools[2:]:
                    self.assertEqual(tool["state"]["status"], "error")
                    self.assertIn("No tool named", tool["state"]["error"])
                texts = [e["part"]["text"] for e in events if e["type"] == "text"]
                self.assertEqual(texts, ['{"findings":[]}'])
                after = {p.relative_to(project): p.read_bytes() for p in project.rglob("*") if p.is_file()}
                self.assertEqual(after, before)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=3)


if __name__ == "__main__":
    unittest.main()
