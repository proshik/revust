#!/usr/bin/env python3
"""Terminal.app launcher boundary; fake host executes the actual shell script."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from tmux_launcher_checks import CHILD

ROOT = Path(__file__).resolve().parents[2]
CLIENT = r'''#!PYTHON
import json,os,pathlib,subprocess,sys
root=pathlib.Path(os.environ['FIXTURE']);args=sys.argv[1:]
sys.stdin.read()
with (root/'operations.jsonl').open('a') as f:f.write(json.dumps(args)+'\n')
mode=os.environ.get('MODE','')
if len(args)==2 and args[1]!='12345':
    if mode=='fail':print('controlled Terminal launch failure',file=sys.stderr);sys.exit(1)
    env={k:os.environ[k] for k in ('FIXTURE','CHILD_CODE','EMPTY_REPORT') if k in os.environ}
    env.update(PATH='/usr/bin:/bin',HOME=str(root/'server-home'))
    subprocess.run([args[1]],cwd=root/'server-home',env=env)
    if mode=='lost':print('lost launch reply',file=sys.stderr);sys.exit(1)
    print('12345')
elif args==['-','12345']:
    if mode=='close-fail':sys.exit(1)
else:raise SystemExit('unexpected osascript arguments')
'''


class TerminalLauncher(unittest.TestCase):
    def launch(self, *, code=0, mode='', empty=False, manual=False):
        temporary = tempfile.TemporaryDirectory(prefix="revust terminal '$-")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name).resolve()
        binary = root / 'bin'
        binary.mkdir()
        for name, body in [('revust', CHILD), ('osascript', CLIENT)]:
            path = binary / name
            path.write_text(body.replace('PYTHON', sys.executable))
            path.chmod(0o755)
        for name in ('home', 'server-home', 'tmp'):
            (root / name).mkdir()
        env = {'PATH': str(binary) + ':/usr/bin:/bin', 'HOME': str(root/'home'),
               'TMPDIR': str(root/'tmp'), 'TERM_PROGRAM': 'Apple_Terminal',
               'FIXTURE': str(root), 'CHILD_CODE': str(code), 'MODE': mode,
               'XDG_CONFIG_HOME': str(root/'xdg home'), 'CODEX_HOME': str(root/'codex home'),
               'CLAUDE_CONFIG_DIR': str(root/'claude home')}
        if empty:
            env['EMPTY_REPORT'] = '1'
        if manual:
            env['REVUST_AUTO_EXIT'] = '0'
        argv = ['--task', 'literal task', '--run', "quote'\"$`\nстрока $(touch injected)"]
        result = subprocess.run(['/bin/bash', str(ROOT/'scripts/launch-revust.sh'), *argv],
                                cwd=root, env=env, capture_output=True, timeout=15)
        path = root/'operations.jsonl'
        operations = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        self.assertFalse((root/'injected').exists())
        self.assertFalse((root/'server-home/injected').exists())
        return result, root, argv, env, operations

    def test_literal_arguments_roots_cwd_and_result_codes(self):
        for code in (0, 1, 2):
            with self.subTest(code=code):
                result, root, argv, env, operations = self.launch(code=code)
                self.assertEqual(result.returncode, code, result.stderr)
                self.assertEqual(result.stdout, b'controlled report\n\n')
                self.assertEqual((root/'calls').read_text(), 'run\n')
                child = json.loads((root/'child.json').read_text())
                self.assertEqual(child['args'], argv + ['--auto-exit=30s'])
                self.assertEqual(child['cwd'], str(root))
                self.assertEqual(child['env'], {k: env[k] for k in child['env']})
                self.assertEqual(operations[1:], [['-', '12345']])
                self.assertEqual(list((root/'tmp').glob('revust-*')), [])

    def test_lost_reply_recovers_owned_result_without_redispatch_or_wrong_cleanup(self):
        result, root, _, _, operations = self.launch(code=1, mode='lost')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(result.stdout, b'controlled report\n\n')
        self.assertEqual((root/'calls').read_text(), 'run\n')
        self.assertEqual(len(operations), 1)
        self.assertEqual(list((root/'tmp').glob('revust-*')), [])

    def test_unknown_launch_failure_retains_evidence_and_diagnostic(self):
        result, root, _, _, operations = self.launch(mode='fail')
        self.assertEqual(result.returncode, 3, result.stderr)
        self.assertEqual(result.stdout, b'')
        self.assertEqual(len(operations), 1)
        self.assertFalse((root/'calls').exists())
        self.assertIn(b'controlled Terminal launch failure', result.stderr)
        self.assertIn(b'artifacts retained', result.stderr)
        self.assertTrue(any((root/'tmp').glob('revust-*')))

    def test_cleanup_failure_preserves_review_result_and_manual_exit(self):
        result, root, argv, _, operations = self.launch(code=2, mode='close-fail', manual=True)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertEqual(result.stdout, b'controlled report\n\n')
        self.assertEqual(json.loads((root/'child.json').read_text())['args'], argv)
        self.assertEqual(operations[1:], [['-', '12345']])

    def test_empty_report_is_not_a_success(self):
        for code in (0, 1, 2):
            with self.subTest(code=code):
                result, root, _, _, operations = self.launch(code=code, empty=True)
                self.assertEqual(result.returncode, 2 if code==2 else 3, result.stderr)
                self.assertEqual(result.stdout, b'')
                self.assertEqual((root/'calls').read_text(), 'run\n')
                self.assertEqual(operations[1:], [['-', '12345']])


if __name__ == '__main__':
    unittest.main(verbosity=2)
