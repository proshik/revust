#!/usr/bin/env python3
"""Exercise the herdr adapter with local CLIs, without a host or model service."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from tmux_launcher_checks import CHILD

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / 'scripts/launch-revust.sh'
CLIENT = r'''#!PYTHON
import json,os,pathlib,subprocess,sys
root=pathlib.Path(os.environ['FIXTURE']);args=sys.argv[1:]
with (root/'operations.jsonl').open('a') as output:output.write(json.dumps(args)+'\n')
mode=os.environ.get('MODE','')
if args[:2]==['tab','create']:
    if mode=='create-fail':print('controlled create failure',file=sys.stderr);sys.exit(1)
    (root/'tab-cwd').write_text(args[args.index('--cwd')+1])
    print(json.dumps({'result':{'tab':{'tab_id':'owned-tab'},
                                'root_pane':{} if mode=='missing-pane' else {'pane_id':'owned-pane'}}}))
elif args[:2]==['pane','run']:
    if mode=='run-fail':sys.exit(1)
    environment={key:os.environ[key] for key in ('FIXTURE','CHILD_CODE','EMPTY_REPORT') if key in os.environ}
    environment.update(PATH='/usr/bin:/bin',HOME=str(root/'server-home'))
    subprocess.run(['/bin/sh','-c',args[3]],cwd=(root/'tab-cwd').read_text(),env=environment)
    if mode=='lost':sys.exit(1)
elif args[:2]!=['tab','close']:raise SystemExit('unexpected herdr operation')
'''


class HerdrLauncher(unittest.TestCase):
    def launch(self, *, code=0, mode='', workspace=True, empty=False):
        temporary=tempfile.TemporaryDirectory(prefix="revust herdr '$-")
        self.addCleanup(temporary.cleanup)
        root=Path(temporary.name).resolve();binary=root/'bin';binary.mkdir()
        # A kitty marker is inherited when herdr runs inside kitty. Herdr must
        # win; the fake kitty rejects any accidental remote-control dispatch.
        for name,body in [('revust',CHILD),('herdr',CLIENT),('kitty','#!/bin/sh\nexit 88\n')]:
            path=binary/name;path.write_text(body.replace('PYTHON',sys.executable));path.chmod(0o755)
        for name in ('home','server-home','tmp'):(root/name).mkdir()
        env={'PATH':str(binary)+':/usr/bin:/bin','HOME':str(root/'home'),
             'TMPDIR':str(root/'tmp'),'FIXTURE':str(root),'CHILD_CODE':str(code),
             'MODE':mode,'HERDR_ENV':'1','KITTY_LISTEN_ON':'inherited-kitty',
             'XDG_CONFIG_HOME':str(root/'xdg home'),'CODEX_HOME':str(root/'codex home'),
             'CLAUDE_CONFIG_DIR':str(root/'claude home')}
        if workspace:env['HERDR_WORKSPACE_ID']='caller-workspace'
        if empty:env['EMPTY_REPORT']='1'
        argv=['--task','literal task','--run',"quote'\"$`\nстрока $(touch injected)"]
        result=subprocess.run(['/bin/bash',str(LAUNCHER),*argv],cwd=root,env=env,
                              capture_output=True,timeout=15)
        operations=[json.loads(line) for line in (root/'operations.jsonl').read_text().splitlines()]
        self.assertFalse((root/'injected').exists())
        return result,root,argv,env,operations

    def test_targeted_tab_preserves_literal_roots_cwd_and_review_status(self):
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
                self.assertEqual(operations[0][:4],['tab','create','--cwd',str(root)])
                self.assertEqual(operations[0][-3:],['--workspace','caller-workspace','--focus'])
                self.assertEqual(operations[1][:3],['pane','run','owned-pane'])
                self.assertEqual(operations[2],['tab','close','owned-tab'])
                self.assertEqual(len(operations),3)
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_without_workspace_uses_current_workspace_once(self):
        result,root,_,_,operations=self.launch(workspace=False)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertNotIn('--workspace',operations[0])
        self.assertEqual((root/'calls').read_text(),'run\n')
        self.assertEqual(len(operations),3)

    def test_create_failure_or_missing_ids_never_dispatch_into_another_pane(self):
        for mode in ('create-fail','missing-pane'):
            with self.subTest(mode=mode):
                result,root,_,_,operations=self.launch(mode=mode)
                self.assertEqual(result.returncode,3,result.stderr)
                self.assertEqual(result.stdout,b'')
                self.assertFalse((root/'calls').exists())
                self.assertFalse(any(op[:2]==['pane','run'] for op in operations))
                self.assertEqual(operations[1:],
                                 [['tab','close','owned-tab']] if mode=='missing-pane' else [])
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_completed_result_survives_lost_pane_reply_without_a_second_dispatch(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,mode='lost')
                self.assertEqual(result.returncode,code,result.stderr)
                self.assertEqual(result.stdout,b'controlled report\n\n')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(sum(op[:2]==['pane','run'] for op in operations),1)
                self.assertEqual(operations[-1],['tab','close','owned-tab'])
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_unacknowledged_pane_keeps_evidence_and_tab_for_a_possible_queued_start(self):
        result,root,_,_,operations=self.launch(mode='run-fail')
        self.assertEqual(result.returncode,3,result.stderr)
        self.assertEqual(result.stdout,b'')
        self.assertFalse((root/'calls').exists())
        self.assertEqual(sum(op[:2]==['pane','run'] for op in operations),1)
        self.assertFalse(any(op[:2]==['tab','close'] for op in operations))
        self.assertIn(b'artifacts retained',result.stderr)
        self.assertTrue(any((root/'tmp').glob('revust-*')))

    def test_empty_report_is_explicit_failure_and_owned_tab_is_closed(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,empty=True)
                self.assertEqual(result.returncode,2 if code==2 else 3,result.stderr)
                self.assertEqual(result.stdout,b'')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(operations[-1],['tab','close','owned-tab'])
                self.assertIn(b'controlled diagnostic',result.stderr)


if __name__=='__main__':unittest.main()
