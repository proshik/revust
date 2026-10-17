#!/usr/bin/env python3
"""Controlled cmux boundary checks; no terminal app or model service is used."""
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
with (root/'operations.jsonl').open('a') as output:output.write(json.dumps(args)+'\n')
mode=os.environ.get('MODE','')
if args==['new-split','down']:
    if mode=='create-fail':print('controlled split failure',file=sys.stderr);sys.exit(1)
    print('OK surface:' if mode=='empty-id' else 'OK no id' if mode=='missing-id' else 'OK surface:27')
elif args[:1]==['send']:
    if args[1:3]!=['--surface','surface:27']:sys.exit(1)
    if mode=='send-fail':sys.exit(1)
    command=args[3]
    if not command.endswith('\\n'):raise SystemExit('missing Enter escape')
    environment={key:os.environ[key] for key in ('FIXTURE','CHILD_CODE','EMPTY_REPORT') if key in os.environ}
    environment.update(PATH='/usr/bin:/bin',HOME=str(root/'server-home'))
    # A newly split shell can inherit a different cwd. Execute the command
    # actually sent by the launcher, with its terminating Enter removed.
    subprocess.run(['/bin/sh','-c',command[:-2]],cwd=root/'server-home',env=environment)
    if mode=='lost':sys.exit(1)
else:raise SystemExit('unexpected cmux operation')
'''


class CmuxLauncher(unittest.TestCase):
    def launch(self, *, code=0, mode='', marker='surface', empty=False, available=True):
        temporary=tempfile.TemporaryDirectory(prefix="revust cmux '$-")
        self.addCleanup(temporary.cleanup)
        root=Path(temporary.name).resolve();binary=root/'bin';binary.mkdir()
        programs=[('revust',CHILD),('osascript','#!/bin/sh\nexit 88\n')]
        if available:programs.append(('cmux',CLIENT))
        for name,body in programs:
            path=binary/name;path.write_text(body.replace('PYTHON',sys.executable));path.chmod(0o755)
        for name in ('home','server-home','tmp'):(root/name).mkdir()
        env={'PATH':str(binary)+':/usr/bin:/bin','HOME':str(root/'home'),
             'TMPDIR':str(root/'tmp'),'FIXTURE':str(root),'CHILD_CODE':str(code),
             'MODE':mode,'TERM_PROGRAM':'ghostty',
             'XDG_CONFIG_HOME':str(root/'xdg home'),'CODEX_HOME':str(root/'codex home'),
             'CLAUDE_CONFIG_DIR':str(root/'claude home')}
        env.update({'surface':{'CMUX_SURFACE_ID':'surface:1'},
                    'bundle':{'__CFBundleIdentifier':'com.cmuxterm.app'},
                    'resources':{'GHOSTTY_RESOURCES_DIR':'/Applications/cmux.app/Contents/Resources'}}[marker])
        if empty:env['EMPTY_REPORT']='1'
        argv=['--task','literal task','--run',"quote'\"$`\nстрока $(touch injected)"]
        result=subprocess.run(['/bin/bash',str(LAUNCHER),*argv],cwd=root,env=env,
                              capture_output=True,timeout=15)
        path=root/'operations.jsonl'
        operations=[json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        self.assertFalse((root/'injected').exists())
        self.assertFalse((root/'server-home'/'injected').exists())
        return result,root,argv,env,operations

    def test_targeted_split_preserves_caller_cwd_roots_literal_argv_and_status(self):
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
                self.assertEqual(operations[0],['new-split','down'])
                self.assertEqual(operations[1][:3],['send','--surface','surface:27'])
                self.assertEqual(len(operations),2)
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_all_session_markers_select_cmux_before_ghostty(self):
        for marker in ('surface','bundle','resources'):
            with self.subTest(marker=marker):
                result,root,_,_,operations=self.launch(marker=marker)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(len(operations),2)

    def test_bad_or_failed_split_never_sends_into_the_callers_shell(self):
        for mode in ('create-fail','missing-id','empty-id'):
            with self.subTest(mode=mode):
                result,root,_,_,operations=self.launch(mode=mode)
                self.assertEqual(result.returncode,3,result.stderr)
                self.assertEqual(result.stdout,b'')
                self.assertFalse((root/'calls').exists())
                self.assertEqual(operations,[['new-split','down']])
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_completed_results_survive_lost_send_reply_without_another_split(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,mode='lost')
                self.assertEqual(result.returncode,code,result.stderr)
                self.assertEqual(result.stdout,b'controlled report\n\n')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(len(operations),2)
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_unacknowledged_send_keeps_the_queued_launch_artifacts(self):
        result,root,_,_,operations=self.launch(mode='send-fail')
        self.assertEqual(result.returncode,3,result.stderr)
        self.assertEqual(result.stdout,b'')
        self.assertFalse((root/'calls').exists())
        self.assertEqual(len(operations),2)
        self.assertIn(b'artifacts retained',result.stderr)
        self.assertTrue(any((root/'tmp').glob('revust-*')))

    def test_empty_reports_do_not_become_success_or_findings(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,empty=True)
                self.assertEqual(result.returncode,2 if code==2 else 3,result.stderr)
                self.assertEqual(result.stdout,b'')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(len(operations),2)
                self.assertIn(b'controlled diagnostic',result.stderr)

    def test_missing_cmux_never_falls_back_to_ghostty(self):
        result,root,_,_,operations=self.launch(available=False)
        self.assertEqual(result.returncode,3,result.stderr)
        self.assertEqual(result.stdout,b'')
        self.assertFalse((root/'calls').exists())
        self.assertEqual(operations,[])
        self.assertEqual(list((root/'tmp').glob('revust-*')),[])


if __name__=='__main__':unittest.main()
