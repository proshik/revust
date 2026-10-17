#!/usr/bin/env python3
"""Ghostty launcher boundary checks; AppleScript/native delivery is not emulated."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from tmux_launcher_checks import CHILD

ROOT=Path(__file__).resolve().parents[2]
LAUNCHER=ROOT/'scripts/launch-revust.sh'
CLIENT=r'''#!PYTHON
import json,os,pathlib,subprocess,sys
root=pathlib.Path(os.environ['FIXTURE']);args=sys.argv[1:]
script=sys.stdin.read()
with (root/'operations.jsonl').open('a') as output:output.write(json.dumps(args)+'\n')
mode=os.environ.get('MODE','')
if len(args)==3:
    if mode=='fail':print('controlled launch failure',file=sys.stderr);sys.exit(1)
    environment={key:os.environ[key] for key in ('FIXTURE','CHILD_CODE','EMPTY_REPORT') if key in os.environ}
    environment.update(PATH='/usr/bin:/bin',HOME=str(root/'server-home'))
    subprocess.run([args[1]],cwd=args[2],env=environment)
    if mode=='lost':sys.exit(1)
    print('owned-terminal')
elif args==['-','owned-terminal']:
    if mode=='close-fail':print('controlled close failure',file=sys.stderr);sys.exit(1)
else:raise SystemExit('unexpected osascript arguments')
'''


class GhosttyLauncher(unittest.TestCase):
    def launch(self, *, code=0, mode='', empty=False, manual=False):
        temporary=tempfile.TemporaryDirectory(prefix="revust ghostty '$-")
        self.addCleanup(temporary.cleanup)
        root=Path(temporary.name).resolve();binary=root/'bin';binary.mkdir()
        for name,body in [('revust',CHILD),('osascript',CLIENT)]:
            path=binary/name;path.write_text(body.replace('PYTHON',sys.executable));path.chmod(0o755)
        for name in ('home','server-home','tmp'):(root/name).mkdir()
        env={'PATH':str(binary)+':/usr/bin:/bin','HOME':str(root/'home'),
             'TMPDIR':str(root/'tmp'),'FIXTURE':str(root),'CHILD_CODE':str(code),
             'MODE':mode,'TERM_PROGRAM':'ghostty',
             'XDG_CONFIG_HOME':str(root/'xdg home'),'CODEX_HOME':str(root/'codex home'),
             'CLAUDE_CONFIG_DIR':str(root/'claude home')}
        if empty:env['EMPTY_REPORT']='1'
        if manual:env['REVUST_AUTO_EXIT']='0'
        argv=['--task','literal task','--run',"quote'\"$`\nстрока $(touch injected)"]
        result=subprocess.run(['/bin/bash',str(LAUNCHER),*argv],cwd=root,env=env,
                              capture_output=True,timeout=15)
        operations=[json.loads(line) for line in (root/'operations.jsonl').read_text().splitlines()]
        self.assertFalse((root/'injected').exists())
        return result,root,argv,env,operations

    def test_literal_roots_cwd_report_and_result_survive_the_script_boundary(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,argv,env,operations=self.launch(code=code)
                self.assertEqual(result.returncode,code,result.stderr)
                self.assertEqual(result.stdout,b'controlled report\n\n')
                self.assertEqual((root/'calls').read_text(),'run\n')
                child=json.loads((root/'child.json').read_text())
                self.assertEqual(child['args'],argv+['--auto-exit=30s'])
                self.assertEqual(child['cwd'],str(root))
                self.assertEqual(child['env'],{key:env[key] for key in child['env']})
                self.assertEqual(operations[0][0],'-')
                self.assertEqual(operations[0][2],str(root))
                self.assertEqual(operations[1:],[['-','owned-terminal']])
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_cleanup_error_never_replaces_an_already_saved_result(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,mode='close-fail')
                self.assertEqual(result.returncode,code,result.stderr)
                self.assertEqual(result.stdout,b'controlled report\n\n')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(operations[1:],[['-','owned-terminal']])

    def test_unknown_launch_failure_retains_evidence_without_a_repeat(self):
        result,root,_,_,operations=self.launch(mode='fail')
        self.assertEqual(result.returncode,3,result.stderr)
        self.assertEqual(result.stdout,b'')
        self.assertFalse((root/'calls').exists())
        self.assertEqual(len(operations),1)
        self.assertIn(b'artifacts retained',result.stderr)
        self.assertTrue(any((root/'tmp').glob('revust-*')))

    def test_lost_reply_observes_completed_result_without_a_guessed_cleanup_id(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,mode='lost')
                self.assertEqual(result.returncode,code,result.stderr)
                self.assertEqual(result.stdout,b'controlled report\n\n')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(len(operations),1)
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_empty_report_never_becomes_success_or_findings(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,empty=True)
                self.assertEqual(result.returncode,2 if code==2 else 3,result.stderr)
                self.assertEqual(result.stdout,b'')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(operations[1:],[['-','owned-terminal']])
                self.assertIn(b'controlled diagnostic',result.stderr)

    def test_manual_exit_setting_is_forwarded_without_a_countdown(self):
        result,root,argv,_,operations=self.launch(manual=True)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads((root/'child.json').read_text())['args'],argv)
        self.assertEqual(len(operations),2)


if __name__=='__main__':unittest.main()
