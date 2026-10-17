#!/usr/bin/env python3
"""Qualify the shipped kitty adapter with local CLIs, without a host or model."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from tmux_launcher_checks import CHILD

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / 'scripts/launch-revust.sh'
KITTY = r'''#!PYTHON
import json,os,pathlib,subprocess,sys
root=pathlib.Path(os.environ['FIXTURE']);args=sys.argv[1:]
with (root/'operations.jsonl').open('a') as output:output.write(json.dumps(args)+'\n')
if os.environ.get('MODE')=='fail':sys.exit(1)
environment={key:os.environ[key] for key in ('FIXTURE','CHILD_CODE','EMPTY_REPORT') if key in os.environ}
environment.update(PATH='/usr/bin:/bin',HOME=str(root/'server-home'))
subprocess.run(args[args.index('sh'):],cwd=root/'server-cwd',env=environment)
if os.environ.get('MODE')=='repeat': subprocess.run(args[args.index('sh'):],cwd=root/'server-cwd',env=environment)
if os.environ.get('MODE')=='lost':sys.exit(1)
'''


class KittyLauncher(unittest.TestCase):
    def launch(self, *, code=0, mode='', window=True, empty=False):
        temporary=tempfile.TemporaryDirectory(prefix="revust kitty '$-")
        self.addCleanup(temporary.cleanup)
        root=Path(temporary.name).resolve();binary=root/'bin';binary.mkdir()
        for name,body in [('revust',CHILD),('kitty',KITTY)]:
            path=binary/name;path.write_text(body.replace('PYTHON',sys.executable));path.chmod(0o755)
        for name in ('home','server-home','server-cwd','tmp'):(root/name).mkdir()
        env={'PATH':str(binary)+':/usr/bin:/bin','HOME':str(root/'home'),
             'TMPDIR':str(root/'tmp'),'FIXTURE':str(root),'CHILD_CODE':str(code),
             'MODE':mode,'KITTY_LISTEN_ON':'unix:'+str(root/'kitty socket'),
             'XDG_CONFIG_HOME':str(root/'xdg home'),'CODEX_HOME':str(root/'codex home'),
             'CLAUDE_CONFIG_DIR':str(root/'claude home')}
        if window:env['KITTY_WINDOW_ID']='43'
        if empty:env['EMPTY_REPORT']='1'
        argv=['--task','literal task','--run',"quote'\"$`\nстрока $(touch injected)"]
        result=subprocess.run(['/bin/bash',str(LAUNCHER),*argv],cwd=root,env=env,
                              capture_output=True,timeout=15)
        operations=[json.loads(line) for line in (root/'operations.jsonl').read_text().splitlines()]
        self.assertFalse((root/'injected').exists())
        return result,root,argv,env,operations

    def test_matched_overlay_preserves_literal_roots_cwd_and_result(self):
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
                self.assertEqual(len(operations),1)
                self.assertEqual(operations[0][:4],['@','--to',env['KITTY_LISTEN_ON'],'launch'])
                self.assertEqual(operations[0][operations[0].index('--match')+1],'window_id:43')
                self.assertIn('--type=overlay',operations[0])
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_repeated_delivery_of_owned_runner_cannot_dispatch_twice(self):
        result,root,_,_,operations=self.launch(mode='repeat')
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((root/'calls').read_text(),'run\n')
        self.assertEqual(len(operations),1)

    def test_missing_window_id_keeps_current_overlay_contract(self):
        result,root,_,_,operations=self.launch(window=False)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((root/'calls').read_text(),'run\n')
        self.assertEqual(json.loads((root/'child.json').read_text())['cwd'],str(root))
        self.assertNotIn('--match',operations[0])

    def test_completed_run_survives_lost_host_reply_without_another_dispatch(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,mode='lost')
                self.assertEqual(result.returncode,code,result.stderr)
                self.assertEqual(result.stdout,b'controlled report\n\n')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(len(operations),1)
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_unacknowledged_launch_retains_evidence_and_never_claims_success(self):
        result,root,_,_,operations=self.launch(mode='fail')
        self.assertEqual(result.returncode,3,result.stderr)
        self.assertEqual(result.stdout,b'')
        self.assertFalse((root/'calls').exists())
        self.assertEqual(len(operations),1)
        self.assertIn(b'artifacts retained',result.stderr)
        self.assertTrue(any((root/'tmp').glob('revust-*')))

    def test_no_report_cannot_be_success_or_findings(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,empty=True)
                self.assertEqual(result.returncode,2 if code==2 else 3,result.stderr)
                self.assertEqual(result.stdout,b'')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(len(operations),1)
                self.assertIn(b'controlled diagnostic',result.stderr)


if __name__=='__main__':unittest.main()
