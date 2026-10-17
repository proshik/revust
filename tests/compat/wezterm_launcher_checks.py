#!/usr/bin/env python3
"""Controlled wezterm/kaku launcher checks; no app or model service is used."""
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
with (root/'operations.jsonl').open('a') as output:
    output.write(json.dumps({'program':pathlib.Path(sys.argv[0]).name,'args':args})+'\n')
if os.environ.get('MODE')=='fail':sys.exit(1)
environment={key:os.environ[key] for key in ('FIXTURE','CHILD_CODE','EMPTY_REPORT') if key in os.environ}
environment.update(PATH='/usr/bin:/bin',HOME=str(root/'server-home'))
subprocess.run(args[args.index('--')+1:],cwd=args[args.index('--cwd')+1],env=environment)
if os.environ.get('MODE')=='lost':sys.exit(1)
'''


class WeztermLauncher(unittest.TestCase):
    def launch(self, *, available=('wezterm',), code=0, mode='', empty=False, height='90%'):
        temporary=tempfile.TemporaryDirectory(prefix="revust wezterm '$-")
        self.addCleanup(temporary.cleanup)
        root=Path(temporary.name).resolve();binary=root/'bin';binary.mkdir()
        for name,body in [('revust',CHILD),*[(name,CLIENT) for name in available]]:
            path=binary/name;path.write_text(body.replace('PYTHON',sys.executable));path.chmod(0o755)
        for name in ('home','server-home','tmp'):(root/name).mkdir()
        env={'PATH':str(binary)+':/usr/bin:/bin','HOME':str(root/'home'),
             'TMPDIR':str(root/'tmp'),'FIXTURE':str(root),'CHILD_CODE':str(code),
             'MODE':mode,'WEZTERM_PANE':'42','REVUST_POPUP_HEIGHT':height,
             'XDG_CONFIG_HOME':str(root/'xdg home'),'CODEX_HOME':str(root/'codex home'),
             'CLAUDE_CONFIG_DIR':str(root/'claude home')}
        if empty:env['EMPTY_REPORT']='1'
        argv=['--task','literal task','--run',"quote'\"$`\nстрока $(touch injected)"]
        result=subprocess.run(['/bin/bash',str(LAUNCHER),*argv],cwd=root,env=env,
                              capture_output=True,timeout=15)
        path=root/'operations.jsonl'
        operations=[json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        self.assertFalse((root/'injected').exists())
        return result,root,argv,env,operations

    def test_each_cli_preserves_literal_roots_cwd_and_review_status(self):
        for name in ('wezterm','kaku'):
            for code in (0,1,2):
                with self.subTest(name=name,code=code):
                    result,root,argv,env,operations=self.launch(available=(name,),code=code)
                    self.assertEqual(result.returncode,code,result.stderr)
                    self.assertEqual(result.stdout,b'controlled report\n\n')
                    self.assertEqual((root/'calls').read_text(),'run\n')
                    child=json.loads((root/'child.json').read_text())
                    self.assertEqual(child['args'],argv+['--auto-exit=30s'])
                    self.assertEqual(child['cwd'],str(root))
                    self.assertEqual(child['env'],{key:env[key] for key in child['env']})
                    self.assertEqual(len(operations),1)
                    self.assertEqual(operations[0]['program'],name)
                    self.assertEqual(operations[0]['args'][:7],
                                     ['cli','split-pane','--bottom','--percent','90','--pane-id','42'])
                    self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_wezterm_priority_and_requested_split_height(self):
        for available,height,expected in [(('wezterm','kaku'),'90%','wezterm'),(('kaku',),'73%','kaku')]:
            with self.subTest(available=available):
                result,root,_,_,operations=self.launch(available=available,height=height)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(len(operations),1)
                self.assertEqual(operations[0]['program'],expected)
                args=operations[0]['args'];self.assertEqual(args[args.index('--percent')+1],height[:-1])

    def test_completed_results_survive_lost_reply_without_a_second_dispatch(self):
        for name in ('wezterm','kaku'):
            for code in (0,1,2):
                with self.subTest(name=name,code=code):
                    result,root,_,_,operations=self.launch(available=(name,),code=code,mode='lost')
                    self.assertEqual(result.returncode,code,result.stderr)
                    self.assertEqual(result.stdout,b'controlled report\n\n')
                    self.assertEqual((root/'calls').read_text(),'run\n')
                    self.assertEqual(len(operations),1)
                    self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_unacknowledged_or_missing_cli_is_launcher_failure(self):
        for available in [('wezterm',),('kaku',),()]:
            with self.subTest(available=available):
                result,root,_,_,operations=self.launch(available=available,mode='fail')
                self.assertEqual(result.returncode,3,result.stderr)
                self.assertEqual(result.stdout,b'')
                self.assertFalse((root/'calls').exists())
                self.assertEqual(len(operations),1 if available else 0)
                if available:
                    self.assertIn(b'artifacts retained',result.stderr)
                    self.assertTrue(any((root/'tmp').glob('revust-*')))
                else:
                    self.assertIn(b'no overlay terminal available',result.stderr)
                    self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_empty_report_never_becomes_success_or_findings(self):
        for name in ('wezterm','kaku'):
            for code in (0,1,2):
                with self.subTest(name=name,code=code):
                    result,root,_,_,operations=self.launch(available=(name,),code=code,empty=True)
                    self.assertEqual(result.returncode,2 if code==2 else 3,result.stderr)
                    self.assertEqual(result.stdout,b'')
                    self.assertEqual((root/'calls').read_text(),'run\n')
                    self.assertEqual(len(operations),1)
                    self.assertIn(b'controlled diagnostic',result.stderr)


if __name__=='__main__':unittest.main()
