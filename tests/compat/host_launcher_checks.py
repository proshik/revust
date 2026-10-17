#!/usr/bin/env python3
"""Launcher invariants against a persisted protocol peer, with no active host."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
ENTRY = ROOT / 'scripts/launch-revust.sh'
PEER = Path(__file__).with_name('host_peer.py')


class HostContract(unittest.TestCase):
    def invoke(self, case=None, *, host='guild', extra=(), controls=None):
        temporary = tempfile.TemporaryDirectory(prefix='revust-host-contract-')
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name).resolve()
        for name in ['bin', 'home', 'tmp']:
            (root / name).mkdir()
        (root / 'case.json').write_text(json.dumps(case or {}))
        for name in ['guild', 'agtermctl', 'selected program']:
            program = root / 'bin' / name
            program.write_text('#!' + sys.executable + '\n' + PEER.read_text().split('\n', 1)[1])
            program.chmod(0o700)
        environment = {'PATH': str(root / 'bin') + ':/usr/bin:/bin',
                       'HOME': str(root / 'home'), 'TMPDIR': str(root / 'tmp'),
                       'PEER_ROOT': str(root), 'REVUST_BIN': str(root / 'bin/selected program')}
        if host == 'guild':
            environment.update(GUILD_SESSION='test-session', GUILD_SOCKET=str(root / 'socket'),
                               AGTERM_SESSION_ID='must-not-fallback', TMUX='must-not-fallback')
        else:
            environment.update(AGTERM_SESSION_ID='owned-session', AGTERM_SOCKET=str(root / 'socket'),
                               AGTERM_WINDOW_ID='owned-window')
        environment.update(controls or {})
        argv = ['--task', 'synthetic task', '--run', 'quote\'"$`\n$(touch escaped)', *extra]
        result = subprocess.run(['/bin/bash', str(ENTRY), *argv], cwd=root, env=environment,
                                capture_output=True, timeout=15)
        self.assertFalse((root / 'escaped').exists())
        self.assertNotIn(b'Traceback', result.stderr)
        return result, root, argv

    def trace(self, root, operation):
        path = root / 'trace.jsonl'
        records = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        return [args for name, args in records if name == 'guild' and args[1] == operation]

    def deliveries(self, root):
        path = root / 'deliveries.jsonl'
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def test_output_exit_identity_and_argument_transport(self):
        for code in [0, 1, 2]:
            with self.subTest(code=code):
                result, root, argv = self.invoke({'code': code})
                self.assertEqual(result.returncode, code, result.stderr)
                self.assertEqual(result.stdout, b'Synthetic saved output\n\n')
                self.assertIn(b'Synthetic child diagnostic', result.stderr)
                self.assertEqual(self.deliveries(root), [{'args': [*argv, '--auto-exit=30s'], 'cwd': str(root)}])
                descriptor = json.loads((root / 'descriptor.json').read_text())[1]
                self.assertEqual(descriptor['archive_viewer'], {'program': str(root / 'bin/selected program'), 'directory': str(root / 'archive')})
                self.assertEqual(descriptor['args'][3], descriptor['archive_viewer']['program'])
                self.assertEqual(len(self.trace(root, 'close')), 1)
                self.assertEqual(list((root / 'tmp').glob('revust-*')), [])

    def test_capability_refusal_precedes_config_and_dispatch(self):
        for case in [
            {'unreachable': True}, {'reply_null': True},
            *[{'capabilities': {'version': value}} for value in [True, 0, 2, None]],
            *[{'capabilities': {'operations': value}} for value in [None, [], ['start', 'status', 'wait', 'close']]],
        ]:
            with self.subTest(case=case):
                result, root, _ = self.invoke(case)
                self.assertEqual(result.returncode, 3)
                self.assertFalse((root / 'descriptor.json').exists())
                self.assertEqual(self.deliveries(root), [])
                self.assertEqual(len(self.trace(root, 'start')), 0)

    def test_archive_resolution_must_be_absolute_and_structured(self):
        for config in [[], {}, {'paths': None}, {'paths': {'archive': 'relative'}}]:
            with self.subTest(config=config):
                result, root, _ = self.invoke({'config': config})
                self.assertEqual(result.returncode, 3)
                self.assertEqual(self.trace(root, 'start'), [])

    def test_lost_start_uses_identical_key_and_descriptor(self):
        for losses, code in [(1, 0), (2, 3)]:
            with self.subTest(losses=losses):
                result, root, _ = self.invoke({'lost': losses})
                self.assertEqual(result.returncode, code, result.stderr)
                self.assertEqual(len(self.trace(root, 'start')), 2)
                self.assertEqual(len(self.deliveries(root)), 1)
                if code == 3:
                    self.assertIn(b'artifacts retained', result.stderr)
                    self.assertEqual(self.trace(root, 'close'), [])

    def test_pending_wait_is_observation_only(self):
        result, root, _ = self.invoke({'pending': True})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.trace(root, 'wait')), 2)
        self.assertEqual(len(self.deliveries(root)), 1)

    def test_invalid_receipts_never_publish_a_successful_report(self):
        cases = [{'receipt': patch} for patch in [
            {'id': 1}, {'state': []}, {'state': 'unrecognized'}, {'exit_code': True},
            {'signal': 'SIGTERM'}, {'cancellation': 'requested'}, {'error': 'failed'},
        ]]
        cases.append({'wait_receipt': {'id': '289c7439-3432-4913-b9ef-908d6d3f374e'}})
        for case in cases:
            with self.subTest(case=case):
                result, root, _ = self.invoke(case)
                self.assertEqual(result.returncode, 3, result.stderr)
                self.assertEqual(result.stdout, b'')
                self.assertEqual(len(self.deliveries(root)), 1)
                self.assertEqual(self.trace(root, 'close'), [])

    def test_missing_output_is_not_success(self):
        for case in [{'empty': True}, {'missing_report': True}]:
            result, _, _ = self.invoke(case)
            self.assertEqual(result.returncode, 3, result.stderr)

    def test_auto_exit_values_are_never_replaced(self):
        for extra, controls in [([], {'REVUST_AUTO_EXIT': '0'}), (['--auto-exit', '0s'], {}), (['--auto-exit=2s'], {})]:
            result, root, argv = self.invoke(extra=extra, controls=controls)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.deliveries(root)[0]['args'], argv)

    def test_invalid_controls_cannot_dispatch(self):
        for controls, extra in [({'REVUST_HOST': 'invalid'}, []), ({}, ['--no-tui'])]:
            result, root, _ = self.invoke(controls=controls, extra=extra)
            self.assertEqual(result.returncode, 3)
            self.assertEqual(self.deliveries(root), [])

    def test_agterm_owned_overlay_retains_result_geometry_and_hue(self):
        for code in [0, 1, 2]:
            result, root, argv = self.invoke({'code': code}, host='agterm', controls={'REVUST_AGTERM_PERCENT': '63'})
            self.assertEqual(result.returncode, code, result.stderr)
            self.assertEqual(result.stdout, b'Synthetic saved output\n\n')
            self.assertEqual(len(self.deliveries(root)), 1)
            geometry = json.loads((root / 'overlay.json').read_text())
            self.assertEqual(geometry[geometry.index('--size-percent') + 1], '63')
            color = geometry[geometry.index('--background-color') + 1]
            self.assertRegex(color, r'^#[0-9a-f]{6}$')
            self.assertEqual(color[1:3], color[3:5])
            self.assertEqual(color[3:5], color[5:7])
            self.assertNotEqual(color, '#808080')


if __name__ == '__main__':
    unittest.main()
