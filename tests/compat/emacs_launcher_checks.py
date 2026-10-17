#!/usr/bin/env python3
"""Controlled Emacs CLI boundary; no Emacs server or vterm UI is emulated."""
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
import json,os,pathlib,re,subprocess,sys
root=pathlib.Path(os.environ['FIXTURE']);args=sys.argv[1:];mode=os.environ.get('MODE','')
expression=args[-1]
level=0;quoted=False;escaped=False
for character in expression:
    if escaped: escaped=False;continue
    if quoted and character=='\\': escaped=True;continue
    if character=='"':quoted=not quoted;continue
    if not quoted:
        if character=='(':level+=1
        elif character==')':level-=1
        assert level>=0, 'unbalanced generated expression'
assert level==0 and not quoted, 'unfinished generated expression'
shell=re.search(r'\(vterm-shell ("(?:[^"\\]|\\.)*")\)',expression)
phase='pid' if expression=='(emacs-pid)' else 'start' if shell else 'frame' if args[0]=='--eval' else 'cleanup'
with (root/'operations.jsonl').open('a') as output:output.write(json.dumps({'phase':phase,'args':args})+'\n')
if phase=='pid':
    if mode=='unreachable':sys.exit(1)
    print({'empty-pid':'','zero-pid':'0','bad-pid':'unknown'}.get(mode,'73161'))
elif phase=='frame':
    if mode=='frame-fail':sys.exit(1)
    (root/'caller-pid').write_text(re.search(r'\(process-id p\)\s+(\d+)',expression).group(1))
elif phase=='start':
    if mode=='start-fail':sys.exit(1)
    # Production only escapes backslash and quote in these Elisp strings.
    # Decode that string value; do not interpret the UI expressions.
    executable=json.loads(shell.group(1),strict=False)
    environment={key:os.environ[key] for key in ('FIXTURE','CHILD_CODE','EMPTY_REPORT') if key in os.environ}
    environment.update(PATH='/usr/bin:/bin',HOME=str(root/'server-home'))
    subprocess.run([executable],cwd=root/'server-home',env=environment)
    if mode=='lost':sys.exit(1)
elif phase=='cleanup' and mode=='cleanup-fail':sys.exit(1)
'''
PS=r'''#!PYTHON
import json,os,pathlib,sys
root=pathlib.Path(os.environ['FIXTURE']);path=root/'parents.jsonl'
first=not path.exists()
with path.open('a') as output:output.write(json.dumps(sys.argv[1:])+'\n')
print('12345' if os.environ.get('WALK') and first else '73161')
'''


class EmacsLauncher(unittest.TestCase):
    def launch(self, *, code=0, mode='', empty=False, walk=False, manual=False):
        temporary=tempfile.TemporaryDirectory(prefix='revust emacs "\\$-\n')
        self.addCleanup(temporary.cleanup)
        root=Path(temporary.name).resolve();binary=root/'bin';binary.mkdir()
        for name,body in [('revust',CHILD),('emacsclient',CLIENT),('ps',PS)]:
            path=binary/name;path.write_text(body.replace('PYTHON',sys.executable));path.chmod(0o755)
        for name in ('home','server-home','tmp'):(root/name).mkdir()
        env={'PATH':str(binary)+':/usr/bin:/bin','HOME':str(root/'home'),
             'TMPDIR':str(root/'tmp'),'FIXTURE':str(root),'CHILD_CODE':str(code),
             'MODE':mode,'INSIDE_EMACS':'vterm',
             'XDG_CONFIG_HOME':str(root/'xdg home'),'CODEX_HOME':str(root/'codex home'),
             'CLAUDE_CONFIG_DIR':str(root/'claude home')}
        if empty:env['EMPTY_REPORT']='1'
        if walk:env['WALK']='1'
        if manual:env['REVUST_AUTO_EXIT']='0'
        argv=['--task','literal task','--run',"quote'\"$`\nстрока $(touch injected)"]
        result=subprocess.run(['/bin/bash',str(LAUNCHER),*argv],cwd=root,env=env,
                              capture_output=True,timeout=15)
        operations=[json.loads(line) for line in (root/'operations.jsonl').read_text().splitlines()]
        self.assertFalse((root/'injected').exists())
        self.assertFalse((root/'server-home'/'injected').exists())
        return result,root,argv,env,operations

    def test_escaped_script_literal_roots_cwd_report_and_review_status(self):
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
                self.assertEqual([op['phase'] for op in operations],['pid','frame','start','cleanup'])
                parents=[json.loads(line) for line in (root/'parents.jsonl').read_text().splitlines()]
                self.assertEqual((root/'caller-pid').read_text(),parents[0][-1])
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_cleanup_failure_never_replaces_a_saved_result(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,mode='cleanup-fail')
                self.assertEqual(result.returncode,code,result.stderr)
                self.assertEqual(result.stdout,b'controlled report\n\n')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(operations[-1]['phase'],'cleanup')

    def test_unreachable_or_invalid_server_never_starts_a_review(self):
        for mode in ('unreachable','empty-pid','zero-pid','bad-pid'):
            with self.subTest(mode=mode):
                result,root,_,_,operations=self.launch(mode=mode)
                self.assertEqual(result.returncode,3,result.stderr)
                self.assertEqual(result.stdout,b'')
                self.assertFalse((root/'calls').exists())
                self.assertEqual([op['phase'] for op in operations],['pid'])
                self.assertIn(b'emacs server not reachable',result.stderr)
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_failed_frame_creation_never_sends_the_vterm_start(self):
        result,root,_,_,operations=self.launch(mode='frame-fail')
        self.assertEqual(result.returncode,3,result.stderr)
        self.assertEqual(result.stdout,b'')
        self.assertFalse((root/'calls').exists())
        self.assertEqual([op['phase'] for op in operations],['pid','frame'])
        self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_lost_start_reply_recovers_result_without_repeating_vterm_execution(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,mode='lost')
                self.assertEqual(result.returncode,code,result.stderr)
                self.assertEqual(result.stdout,b'controlled report\n\n')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual([op['phase'] for op in operations],['pid','frame','start','cleanup'])
                self.assertEqual(list((root/'tmp').glob('revust-*')),[])

    def test_unknown_start_retains_evidence_for_a_possible_queued_vterm(self):
        result,root,_,_,operations=self.launch(mode='start-fail')
        self.assertEqual(result.returncode,3,result.stderr)
        self.assertEqual(result.stdout,b'')
        self.assertFalse((root/'calls').exists())
        self.assertEqual([op['phase'] for op in operations],['pid','frame','start'])
        self.assertIn(b'artifacts retained',result.stderr)
        self.assertTrue(any((root/'tmp').glob('revust-*')))

    def test_empty_report_never_becomes_success_or_findings(self):
        for code in (0,1,2):
            with self.subTest(code=code):
                result,root,_,_,operations=self.launch(code=code,empty=True)
                self.assertEqual(result.returncode,2 if code==2 else 3,result.stderr)
                self.assertEqual(result.stdout,b'')
                self.assertEqual((root/'calls').read_text(),'run\n')
                self.assertEqual(operations[-1]['phase'],'cleanup')
                self.assertIn(b'controlled diagnostic',result.stderr)

    def test_parent_walk_and_manual_exit_preserve_the_calling_shell(self):
        result,root,argv,_,operations=self.launch(walk=True,manual=True)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((root/'caller-pid').read_text(),'12345')
        self.assertEqual(json.loads((root/'child.json').read_text())['args'],argv)
        self.assertEqual(sum(op['phase']=='start' for op in operations),1)


if __name__=='__main__':unittest.main()
