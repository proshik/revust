#!/usr/bin/env python3
"""Controlled zellij launch/acknowledgement tests; no terminal or model service."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from tmux_launcher_checks import CHILD

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / 'scripts/launch-revust.sh'
ZELLIJ = r'''#!PYTHON
import json,os,pathlib,subprocess,sys
root=pathlib.Path(os.environ['FIXTURE']);args=sys.argv[1:];mode=os.environ.get('MODE','')
with (root/'operations.jsonl').open('a') as output: output.write(json.dumps(args)+'\n')
if args==['run','--help']:
    print('zellij run --floating'+(' --tab-id' if mode!='old' else ''));sys.exit()
if args==['action','list-panes','--json','--tab']:
    print(json.dumps([{'id':1,'tab_id':99},{'id':42,'tab_id':7,'is_plugin':False}]));sys.exit()
if args[0]!='run': raise SystemExit('unexpected zellij command')
if mode=='old' and '--tab-id' in args: print('unexpected argument --tab-id',file=sys.stderr);sys.exit(1)
if mode=='fail': print('transport failed',file=sys.stderr);sys.exit(1)
environment={key:os.environ[key] for key in ('FIXTURE','CHILD_CODE','EMPTY_REPORT') if key in os.environ}
environment.update(PATH='/usr/bin:/bin',HOME=str(root/'server-home'))
if mode in ('active-lost','queued-lost'):
    environment['CHILD_DELAY']='0.4' if mode=='active-lost' else '0'
    worker="""import json,os,pathlib,subprocess,sys,time
root=pathlib.Path(sys.argv[1])
if sys.argv[5]=='queued-lost': time.sleep(.3)
try: code=subprocess.run(json.loads(sys.argv[2]),cwd=sys.argv[3],env=json.loads(sys.argv[4])).returncode
except OSError: code=127
(root/'async.done').write_text(str(code))
(root/('async-'+str(os.getpid())+'.done')).write_text(str(code))
"""
    child=subprocess.Popen([sys.executable,'-c',worker,str(root),json.dumps(args[args.index('--')+1:]),
                           args[args.index('--cwd')+1],json.dumps(environment),mode],
                           stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    with (root/'async.pids').open('a') as output:output.write(str(child.pid)+'\n')
    if mode=='active-lost':
        import time
        deadline=time.monotonic()+3
        while not (root/'calls').exists():
            assert time.monotonic()<deadline
            time.sleep(.01)
    sys.exit(1)
subprocess.run(args[args.index('--')+1:],cwd=args[args.index('--cwd')+1],env=environment)
if mode=='lost': print('acknowledgement lost after dispatch',file=sys.stderr);sys.exit(1)
'''


class ZellijLauncher(unittest.TestCase):
    def launch(self, *, code=0, mode='', pane=True, empty=False):
        temporary=tempfile.TemporaryDirectory(prefix="revust zellij '$-")
        self.addCleanup(temporary.cleanup)
        root=Path(temporary.name).resolve();binary=root/'bin';binary.mkdir()
        for name,body in [('revust',CHILD),('zellij',ZELLIJ)]:
            if name=='revust':
                body=body.replace('if not os.environ.get',"import time; time.sleep(float(os.environ.get('CHILD_DELAY','0')))\nif not os.environ.get")
            path=binary/name;path.write_text(body.replace('PYTHON',sys.executable));path.chmod(0o755)
        jq=shutil.which('jq');self.assertIsNotNone(jq,'Tab-routing qualification requires the launcher\'s optional jq tool')
        (binary/'jq').symlink_to(jq)
        for name in ('home','server-home','tmp'): (root/name).mkdir()
        env={'PATH':str(binary)+':/usr/bin:/bin','HOME':str(root/'home'),
             'TMPDIR':str(root/'tmp'),'FIXTURE':str(root),'CHILD_CODE':str(code),
             'MODE':mode,'ZELLIJ':'controlled-session','XDG_CONFIG_HOME':str(root/'xdg home'),
             'CODEX_HOME':str(root/'codex home'),'CLAUDE_CONFIG_DIR':str(root/'claude home')}
        if pane:env['ZELLIJ_PANE_ID']='42'
        if empty:env['EMPTY_REPORT']='1'
        argv=['--task','literal task','--run',"quote'\"$`\nстрока $(touch injected)"]
        result=subprocess.run(['/bin/bash',str(LAUNCHER),*argv],cwd=root,env=env,
                              capture_output=True,timeout=15)
        operations_file=root/'operations.jsonl'
        operations=[json.loads(line) for line in operations_file.read_text().splitlines()] if operations_file.exists() else []
        if mode in ('active-lost','queued-lost'):
            deadline=time.monotonic()+5
            owned=root.joinpath('async.pids').read_text().splitlines()
            while not all((root/('async-'+pid+'.done')).exists() for pid in owned):
                self.assertLess(time.monotonic(),deadline,'controlled async child did not finish')
                time.sleep(.01)
        self.assertFalse((root/'injected').exists())
        return result,root,argv,env,[op for op in operations if '--floating' in op]

    def test_targeted_float_preserves_roots_arguments_and_review_status(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,argv,env,starts=self.launch(code=code)
                self.assertEqual(result.returncode,code,result.stderr)
                self.assertEqual(result.stdout,b'controlled report\n\n')
                self.assertEqual((root/'calls').read_text(),'run\n')
                child=json.loads((root/'child.json').read_text())
                self.assertEqual(child['args'],argv+['--auto-exit=30s'])
                self.assertEqual(child['cwd'],str(root))
                self.assertEqual(child['env'],{key:env[key] for key in child['env']})
                self.assertEqual(len(starts),1)
                self.assertEqual(starts[0][starts[0].index('--tab-id')+1],'7')
                self.assertIn('--close-on-exit',starts[0])
                self.assertEqual(starts[0][starts[0].index('--width')+1],'90%')
                self.assertEqual(starts[0][starts[0].index('--height')+1],'90%')
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_older_cli_and_missing_pane_use_one_untargeted_dispatch(self):
        for mode,pane in [('old',True),('',False)]:
            with self.subTest(mode=mode,pane=pane):
                result,root,_,_,starts=self.launch(mode=mode,pane=pane)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(len(starts),1)
                self.assertNotIn('--tab-id',starts[0])
                self.assertEqual((root/'calls').read_text(),'run\n')

    def test_lost_ack_observes_completed_dispatch_without_launching_again(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,starts=self.launch(mode='lost',code=code)
                self.assertEqual(result.returncode,code,result.stderr)
                self.assertEqual(result.stdout,b'controlled report\n\n')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(len(starts),1)
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_unacknowledged_launch_never_retries_and_retains_observation_files(self):
        result,root,_,_,starts=self.launch(mode='fail')
        self.assertEqual(result.returncode,3,result.stderr)
        self.assertEqual(result.stdout,b'')
        self.assertFalse((root/'calls').exists())
        self.assertEqual(len(starts),1)
        self.assertIn(b'artifacts retained',result.stderr)
        self.assertTrue(any((root/'tmp').glob('revust-*')))

    def test_active_or_queued_dispatch_survives_lost_ack_without_a_second_run(self):
        for mode,code in [('active-lost',1),('queued-lost',0)]:
            with self.subTest(mode=mode):
                result,root,_,_,starts=self.launch(mode=mode,code=code)
                self.assertEqual(result.returncode,code if mode=='active-lost' else 3,result.stderr)
                self.assertTrue((root/'calls').exists(),'queued launch script was removed')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(len(starts),1)
                self.assertEqual((root/'async.done').read_text(),'0')
                if mode=='active-lost':
                    self.assertEqual(result.stdout,b'controlled report\n\n')
                    self.assertEqual(list((root/'tmp').glob('revust-*')),[])
                else:
                    self.assertEqual(result.stdout,b'')
                    self.assertTrue(any((root/'tmp').glob('revust-*')))

    def test_no_report_cannot_be_success_or_findings(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,starts=self.launch(code=code,empty=True)
                self.assertEqual(result.returncode,2 if code==2 else 3,result.stderr)
                self.assertEqual(result.stdout,b'')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(len(starts),1)
                self.assertIn(b'controlled diagnostic',result.stderr)


if __name__=='__main__':unittest.main()
