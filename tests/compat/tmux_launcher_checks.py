#!/usr/bin/env python3
"""Exercise shipped tmux adapters with local CLIs; no terminal app or model call."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / 'scripts/launch-revust.sh'

CHILD = r'''#!PYTHON
import json, os, pathlib, sys
root = pathlib.Path(os.environ['FIXTURE'])
with (root/'calls').open('a') as output: output.write('run\n')
(root/'child.json').write_text(json.dumps({'args':sys.argv[1:], 'cwd':os.getcwd(),
    'env':{key:os.environ.get(key) for key in ('PATH','HOME','XDG_CONFIG_HOME','CODEX_HOME','CLAUDE_CONFIG_DIR','TMPDIR')}}))
if not os.environ.get('EMPTY_REPORT'): sys.stdout.write('controlled report\n\n')
print('controlled diagnostic', file=sys.stderr)
sys.exit(int(os.environ['CHILD_CODE']))
'''

TMUX = r'''#!PYTHON
import json, os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['FIXTURE'])
args = sys.argv[1:]
with (root/'operations.jsonl').open('a') as output: output.write(json.dumps(args)+'\n')
if args == ['-V']: print('tmux '+os.environ.get('TMUX_VERSION','3.3')); sys.exit()
if args[0] == 'show-environment': sys.exit(1)
if args[0] == 'display-message':
    print('@previous' if args[-1]=='#{window_id}' else 'ordinary'); sys.exit()
if args[0] == 'select-window': sys.exit()
if args[0] == 'list-windows': sys.exit()
if args[0] not in ('display-popup','new-window'): raise SystemExit('unexpected tmux command')
if os.environ.get('HOST_FAIL'): sys.exit(1)
if os.environ.get('WINDOW_DIED'):
    print('@review'); sys.exit()
cwd = args[args.index('-d' if args[0]=='display-popup' else '-c')+1]
command = args[args.index('--')+1:]
# A host starts a child using its older environment. The launcher must forward
# the selected configuration roots/PATH and literal argv through its command.
environment = {key:os.environ[key] for key in ('FIXTURE','CHILD_CODE','EMPTY_REPORT') if key in os.environ}
environment.update(PATH='/usr/bin:/bin',HOME=str(root/'server-home'))
result = subprocess.run(command, cwd=cwd, env=environment)
if args[0]=='new-window': print('@review'); sys.exit()
sys.exit(result.returncode)
'''


class TmuxLauncher(unittest.TestCase):
    def launch(self, *, mode='popup', code=0, version='3.3', fail=False, died=False,
               empty=False, automatic=False, extra=(), delay=None, controls=None, entry=None):
        temporary = tempfile.TemporaryDirectory(prefix="revust tmux '$-")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name).resolve()
        binary = root/'bin'; binary.mkdir()
        for name, content in [('revust', CHILD), ('tmux', TMUX)]:
            path = binary/name; path.write_text(content.replace('PYTHON',sys.executable)); path.chmod(0o755)
        for name in ('home','server-home','tmp'): (root/name).mkdir()
        env = {'PATH':str(binary)+':/usr/bin:/bin','HOME':str(root/'home'),
               'TMPDIR':str(root/'tmp'),'FIXTURE':str(root),'CHILD_CODE':str(code),
               'TMUX':'controlled-socket,1,1','TMUX_VERSION':version,
               'XDG_CONFIG_HOME':str(root/"xdg ' config"), 'CODEX_HOME':str(root/'codex home'),
               'CLAUDE_CONFIG_DIR':str(root/'claude home'),
               'REVUST_TMUX_WINDOW':'1' if mode=='window' else '0'}
        if automatic:
            del env['REVUST_TMUX_WINDOW']; env['AGENTDECK_INSTANCE_ID']='fixture-instance'
        if fail: env['HOST_FAIL']='1'
        if died: env['WINDOW_DIED']='1'
        if empty: env['EMPTY_REPORT']='1'
        if delay is not None: env['REVUST_AUTO_EXIT']=delay
        env.update(controls or {})
        env = {key: value for key, value in env.items() if value is not None}
        argv = ['--task','literal task', '--run',"quote'\"$`\nстрока $(touch injected)", *extra]
        result = subprocess.run(['/bin/bash',str(entry or LAUNCHER),*argv], cwd=root,env=env,
                                capture_output=True,timeout=15)
        operations = [json.loads(line) for line in (root/'operations.jsonl').read_text().splitlines()]
        self.assertFalse((root/'injected').exists())
        if fail or died:
            self.assertTrue(list((root/'tmp').glob('revust-*')), 'ambiguous dispatch evidence lost')
        else:
            self.assertEqual(list((root/'tmp').glob('revust-*')), [], 'launcher artifacts survived')
        return result,root,argv,env,operations

    def assert_child(self, result, root, argv, env, code):
        self.assertEqual(result.returncode,code,result.stderr)
        self.assertEqual(result.stdout,b'controlled report\n\n')
        self.assertEqual((root/'calls').read_text(),'run\n')
        child=json.loads((root/'child.json').read_text())
        self.assertEqual(child['args'],argv)
        self.assertEqual(child['cwd'],str(root))
        self.assertEqual(child['env'],{key:env[key] for key in child['env']})

    def test_explicit_window_entry_uses_the_same_owned_transaction(self):
        result,root,argv,env,operations=self.launch(mode='window',code=1,entry=ROOT/'scripts/agentdeck-window.sh')
        self.assert_child(result,root,argv+['--auto-exit=30s'],env,1)
        self.assertEqual(sum(op[0]=='new-window' for op in operations),1)
        self.assertFalse(any(op[0]=='display-popup' for op in operations))

    def test_requested_geometry_and_delay_are_preserved(self):
        controls = {'REVUST_AUTO_EXIT':'4s', 'REVUST_POPUP_WIDTH':'61%', 'REVUST_POPUP_HEIGHT':'62%'}
        result, root, argv, env, operations = self.launch(controls=controls)
        self.assert_child(result, root, argv+['--auto-exit=4s'], env, 0)
        popup = next(op for op in operations if op[0]=='display-popup')
        self.assertEqual(popup[1:6], ['-E','-w','61%','-h','62%'])

    def test_popup_preserves_literal_arguments_roots_and_result_codes(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,argv,env,operations=self.launch(code=code)
                self.assert_child(result,root,argv+['--auto-exit=30s'],env,code)
                popup=next(op for op in operations if op[0]=='display-popup')
                self.assertEqual(popup[1:6],['-E','-w','90%','-h','90%'])
                self.assertIn('-T',popup)
                self.assertFalse(any(op[0]=='new-window' for op in operations))

    def test_older_popup_and_explicit_exit_delay_keep_requested_behavior(self):
        for delay,extra in [('0',()),(None,('--auto-exit','2s'))]:
            with self.subTest(delay=delay):
                result,root,argv,env,operations=self.launch(version='3.2a',delay=delay,extra=extra)
                self.assert_child(result,root,argv,env,0)
                self.assertNotIn('-T',next(op for op in operations if op[0]=='display-popup'))

    def test_window_restores_focus_and_propagates_review_result(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,argv,env,operations=self.launch(mode='window',code=code)
                self.assert_child(result,root,argv+['--auto-exit=30s'],env,code)
                self.assertEqual([op for op in operations if op[0]=='select-window'],
                                 [['select-window','-t','@review'],['select-window','-t','@previous']])
                self.assertFalse(any(op[0]=='display-popup' for op in operations))

    def test_agentdeck_window_does_not_steal_focus(self):
        result,root,argv,env,operations=self.launch(automatic=True)
        self.assert_child(result,root,argv+['--auto-exit=30s'],env,0)
        self.assertEqual(sum(op[0]=='new-window' for op in operations),1)
        self.assertFalse(any(op[0] in ('display-popup','select-window') for op in operations))

    def test_failed_host_and_lost_window_never_claim_review_success_or_fall_back(self):
        for mode,fail,died in [('popup',True,False),('window',True,False),('window',False,True)]:
            with self.subTest(mode=mode,fail=fail,died=died):
                result,root,_,_,operations=self.launch(mode=mode,fail=fail,died=died)
                self.assertEqual(result.returncode,3,result.stderr)
                self.assertEqual(result.stdout,b'')
                self.assertFalse((root/'calls').exists())
                self.assertEqual(sum(op[0] in ('display-popup','new-window') for op in operations),1)

    def test_empty_report_cannot_be_success_or_findings(self):
        for mode in ('popup','window'):
            for code in (0,1,2):
                with self.subTest(mode=mode,code=code):
                    result,root,_,_,_=self.launch(mode=mode,code=code,empty=True)
                    self.assertEqual(result.returncode,2 if code==2 else 3,result.stderr)
                    self.assertEqual(result.stdout,b'')
                    self.assertEqual((root/'calls').read_text(),'run\n')
                    self.assertIn(b'controlled diagnostic',result.stderr)


if __name__=='__main__': unittest.main()
