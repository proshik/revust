#!/usr/bin/env python3
"""Model-free real-revust launch chains; native Guild is an explicit opt-in."""
import contextlib
import io
import http.client
import http.server
import json
import os
from pathlib import Path
import subprocess
import shutil
import sys
import tempfile
import threading
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
BINARY = Path(os.environ.get('REVUST_TEST_BINARY', ROOT / 'target/debug/revust')).resolve()
KEYS = ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY',
        'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')


def prepare(root, executor, values, url=None):
    profiles = root / '.revust/extension-profiles'
    profiles.mkdir(parents=True)
    (profiles / 'proxy.md').write_text('---\nrunner:\n  executor: ' + executor + '\n  model: fixture/model\nagents:\n  - name: proxy\n    lenses: [bugs]\n---\nReview {{SCOPE}}\n')
    expected = root / 'expected.json'
    expected.write_text(json.dumps(values))
    expected.chmod(0o600)
    provider = root / 'provider'
    provider.write_text('#!' + sys.executable + '\n' + '''import json, os, pathlib, sys, urllib.request
if sys.argv[1:] == ['--version']:
    print('opencode v2.0.21')
    raise SystemExit(0)
root = pathlib.Path(''' + repr(str(root)) + ''')
keys = ''' + repr(KEYS) + '''
expected = json.loads((root / 'expected.json').read_text())
valid = {k: os.environ[k] for k in keys if k in os.environ} == expected
if not valid:
    print('synthetic transport mismatch', file=sys.stderr)
    raise SystemExit(9)
url = ''' + repr(url) + '''
if url:
    try:
        with urllib.request.urlopen(url, timeout=3) as reply:
            valid = reply.read() == b'origin-ok'
    except Exception:
        valid = False
(root / 'provider-pass.json').write_text(json.dumps({'environment': True, 'network': valid}))
if not valid:
    raise SystemExit(9)
sys.stdin.read()
executor = ''' + repr(executor) + '''
answer = {'findings': []}
if executor == 'claude':
    print(json.dumps({'type': 'result', 'structured_output': answer}))
elif executor == 'opencode':
    print(json.dumps({'type': 'text', 'part': {'messageID': 'm', 'text': json.dumps(answer)}}))
else:
    print(json.dumps(answer))
''')
    provider.chmod(0o700)
    (root / '.revust/extensions.json').write_text(json.dumps({'version': 1,
        'profiles': {'proxy': {'file': 'extension-profiles/proxy.md'}}, 'policy': {
        'executors': {executor: {'executable': str(provider)}}, 'runtime': {'stagger_delay': '0s'}}}))
    source = root / 'tasks/proxy/01/input'
    source.mkdir(parents=True)
    (source / 'scope.md').write_text('Synthetic proxy forwarding fixture.\n')
    return ['--task=proxy', '--run=01', '--tasks-dir=' + str(root / 'tasks'),
            '--profile=proxy', '--no-synthesis', '--no-verify', '--auto-exit=1ms']


class RealChain(unittest.TestCase):
    def test_real_binary_all_executor_environment_policies(self):
        from host_launcher_checks import HostContract
        harness = HostContract()
        self.addCleanup(harness.doCleanups)
        for host in ('guild', 'agterm'):
            for executor in ('claude', 'codex', 'opencode'):
                for values in ({}, {k: 'private-synthetic-' + k for k in KEYS},
                               {'HTTP_PROXY': '', 'https_proxy': 'private-synthetic-lower', 'no_proxy': ''}):
                    with self.subTest(host=host, executor=executor, keys=list(values)):
                        result, root, _ = harness.invoke({'real_binary': str(BINARY), 'executor': executor},
                                                         host=host, controls=values)
                        diagnostic = result.stderr.decode(errors='replace')
                        for value in values.values():
                            if value:
                                diagnostic = diagnostic.replace(value, '[proxy value]')
                        self.assertEqual(result.returncode, 0, diagnostic[:2000])
                        self.assertEqual(json.loads((root / 'provider-pass.json').read_text()),
                                         {'environment': True, 'network': True})
                        public = result.stdout + result.stderr
                        for path in (root / 'tasks').rglob('*'):
                            if path.is_file():
                                public += path.read_bytes()
                        for value in values.values():
                            if value:
                                self.assertNotIn(value.encode(), public, 'proxy leaked into public output/archive')


class NativeOwnership(unittest.TestCase):
    def test_ambiguous_observation_retains_owned_inputs_and_evidence(self):
        for outcome in ('timeout', 'code-3', 'interrupted'):
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory(prefix='revust-retention-test-') as directory:
                parent = Path(directory)
                roots = []
                endpoints = []
                servers = []
                class Server:
                    def __init__(self, *_):
                        self.server_port = 12345
                        self.stopped = self.closed = False
                        servers.append(self)
                    def serve_forever(self):
                        pass
                    def shutdown(self):
                        self.stopped = True
                    def server_close(self):
                        self.closed = True
                def launch(argv, **kwargs):
                    root = kwargs['cwd']
                    roots.append(root)
                    endpoint = kwargs['env']['http_proxy']
                    endpoints.append(endpoint.encode())
                    (root / 'tasks/proxy/01/owned-evidence').write_text('synthetic job evidence')
                    if outcome == 'timeout':
                        raise subprocess.TimeoutExpired(argv, 45, output=endpoint.encode(), stderr=endpoint.encode())
                    if outcome == 'interrupted':
                        raise KeyboardInterrupt
                    return subprocess.CompletedProcess(argv, 3, endpoint.encode(), endpoint.encode())
                original_mkdtemp = tempfile.mkdtemp
                def allocate(**kwargs):
                    return original_mkdtemp(dir=parent, **kwargs)
                public = io.StringIO()
                with mock.patch.dict(os.environ, {'GUILD_SESSION': 'synthetic', 'GUILD_SOCKET': 'synthetic', 'TMPDIR': str(parent)}), \
                     mock.patch.object(tempfile, 'mkdtemp', side_effect=allocate), \
                     mock.patch.object(http.server, 'ThreadingHTTPServer', Server), \
                     mock.patch.object(subprocess, 'run', side_effect=launch), \
                     contextlib.redirect_stdout(public), contextlib.redirect_stderr(public):
                    with self.assertRaises((SystemExit, KeyboardInterrupt)):
                        native()
                self.assertEqual(len(roots), 1, 'must never restart an ambiguous job')
                root = roots[0]
                self.assertTrue(root.is_dir(), 'ambiguous observer must retain the job workspace')
                self.assertTrue((root / 'provider').is_file())
                self.assertTrue((root / 'expected.json').is_file())
                self.assertTrue((root / 'tasks/proxy/01/input/scope.md').is_file())
                self.assertEqual((root / 'tasks/proxy/01/owned-evidence').read_text(), 'synthetic job evidence')
                for name in ('observation.json', 'stdout.log', 'stderr.log'):
                    path = root / name
                    self.assertTrue(path.is_file())
                    self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                    self.assertNotIn(endpoints[0], path.read_bytes())
                self.assertEqual(root.stat().st_mode & 0o777, 0o700)
                self.assertIn(str(root), public.getvalue())
                self.assertIn('inspect', public.getvalue().lower())
                self.assertNotIn(endpoints[0].decode(), public.getvalue())
                self.assertEqual(len(servers), 2)
                self.assertTrue(all(server.stopped and server.closed for server in servers))
                observation = json.loads((root / 'observation.json').read_text())
                expected = {'timeout': 'timeout', 'code-3': 'launcher-exit-3', 'interrupted': 'interrupted'}
                self.assertEqual(observation['observation'], expected[outcome])


def native():
    """Parent-owned, one synthetic provider, temporary archive, loopback only."""
    if not all(os.environ.get(k) for k in ('GUILD_SOCKET', 'GUILD_SESSION')):
        raise SystemExit('native fixture requires the owning Guild session')
    flags = {'origin': False, 'proxy': False}
    class Origin(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass
        def do_GET(self):
            flags['origin'] = True
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'origin-ok')
    origin = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Origin)
    class Proxy(Origin):
        def do_GET(self):
            flags['proxy'] = True
            connection = http.client.HTTPConnection('127.0.0.1', origin.server_port, timeout=3)
            try:
                connection.request('GET', '/')
                reply = connection.getresponse()
                data = reply.read()
                self.send_response(reply.status)
                self.end_headers()
                self.wfile.write(data)
            finally:
                connection.close()
    proxy = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Proxy)
    servers = (origin, proxy)
    threads = [threading.Thread(target=s.serve_forever, daemon=True) for s in servers]
    for thread in threads:
        thread.start()
    try:
        root = Path(tempfile.mkdtemp(prefix='revust-native-proxy-'))
        passed = False
        observation = 'preparing'
        stdout = stderr = b''
        endpoint = 'http://127.0.0.1:' + str(proxy.server_port)
        # Set every key so allowed host-global values cannot affect this fixture.
        values = {key: '' if key.lower() == 'no_proxy' else endpoint for key in KEYS}
        try:
            args = prepare(root, 'claude', values, 'http://synthetic.invalid/')
            environment = {k: v for k, v in os.environ.items() if k not in KEYS}
            environment.update(values, REVUST_BIN=str(BINARY), REVUST_HOST='guild', HOME=str(root))
            try:
                result = subprocess.run(['/bin/bash', str(ROOT / 'scripts/launch-revust.sh'), *args],
                                        cwd=root, env=environment, capture_output=True, timeout=45)
                stdout, stderr = result.stdout, result.stderr
                observation = 'launcher-exit-' + str(result.returncode)
                passed = result.returncode == 0 and all(flags.values()) and (root / 'provider-pass.json').is_file()
            except subprocess.TimeoutExpired as error:
                observation = 'timeout'
                stdout, stderr = error.stdout or b'', error.stderr or b''
            except KeyboardInterrupt:
                observation = 'interrupted'
                raise
            except OSError:
                observation = 'launch-error'
            print(json.dumps({'native_chain': passed, **flags}))
            if not passed:
                raise SystemExit(1)
        finally:
            if passed:
                shutil.rmtree(root)
            else:
                def save(name, data):
                    if isinstance(data, str):
                        data = data.encode()
                    for value in sorted({v for v in values.values() if v}, key=len, reverse=True):
                        data = data.replace(value.encode(), b'[proxy value]')
                    descriptor = os.open(root / name, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
                    with os.fdopen(descriptor, 'wb') as output:
                        output.write(data)
                save('stdout.log', stdout)
                save('stderr.log', stderr)
                save('observation.json', json.dumps({'observation': observation, 'native_chain': False, **flags}))
                print('Fixture retained at ' + json.dumps(str(root)) +
                      '; inspect Guild Terminal Jobs and these artifacts before retrying. '
                      'Do not restart or cancel an unconfirmed job.', file=sys.stderr)

    finally:
        for server in servers:
            server.shutdown()
            server.server_close()
        for thread in threads:
            thread.join(timeout=3)


if __name__ == '__main__':
    if sys.argv[1:] == ['--native-guild']:
        native()
    else:
        unittest.main()
