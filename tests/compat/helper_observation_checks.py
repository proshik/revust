#!/usr/bin/env python3
"""Public helper observations against authored config and disposable archives."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


def hashes(root):
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in root.rglob('*') if path.is_file() and not path.is_symlink()}


class Helpers(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='revust-helper-contract-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.tasks = self.root / 'saved tasks'
        self.tasks.mkdir()
        self.config = self.root / 'effective.json'
        self.program = self.root / 'selected CLI'
        self.program.write_text('#!' + sys.executable + '''
import json,os,pathlib,sys
root=pathlib.Path(os.environ['FIXTURE'])
with (root/'cli-calls').open('a') as output: output.write(json.dumps(sys.argv[1:])+'\\n')
assert sys.argv[1]=='config', 'helper started an unexpected command'
if os.environ.get('CONFIG_FAIL'): sys.exit(1)
print((root/'effective.json').read_text())
''')
        self.program.chmod(0o700)
        self.provider = self.root / 'custom tool\nwith spaces'
        self.provider.write_text('#!/bin/sh\necho unexpected > "' + str(self.root / 'provider-called') + '"\nexit 91\n')
        self.provider.chmod(0o700)
        self.value = {'selected_profile': {'name':'mixed','runner':{'executor':'opencode'},
                      'roster':[{'executor':'claude'}], 'stages':[{'name':'synthesis','runner':{'executor':'codex'}}, {'name':'verify','runner':{'executor':'claude'}}]},
                      'policy': {'values': {'stages': {'synthesis': {'enabled':False}}, 'executors': {
                          name:{'executable': str(self.provider) if name != 'codex' else '/not/available',
                                'env': {'TOKEN':'synthetic-do-not-print'}} for name in ('claude','codex','opencode')}}},
                      'paths': {'tasks_dir':str(self.tasks),'tasks':[]}}
        self.environment = {'PATH':'/usr/bin:/bin','REVUST_BIN':str(self.program), 'FIXTURE':str(self.root), 'HOME':str(self.root)}

    def invoke(self, helper, *arguments, **environment):
        self.config.write_text(json.dumps(self.value))
        command = [sys.executable, '-B', str(ROOT / 'scripts' / helper)] if helper.endswith('.py') else ['/bin/bash',str(ROOT/'scripts'/helper)]
        result = subprocess.run([*command,*map(str,arguments)],cwd=self.root,env={**self.environment,**environment},capture_output=True,text=True,timeout=15)
        self.assertFalse((self.root/'provider-called').exists())
        self.assertNotIn('synthetic-do-not-print',result.stdout+result.stderr)
        return result

    def write(self, run, name, data):
        path=self.tasks/'task'/run/name
        path.parent.mkdir(parents=True,exist_ok=True)
        path.write_text(json.dumps(data))

    def test_preflight_selected_identity_lenses_and_disabled_stages_without_provider_probe(self):
        for arguments, wanted in [((), 'claude:'),(('--lenses',), 'opencode:')]:
            result=self.invoke('preflight.sh',*arguments)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertIn(wanted,result.stdout)
            self.assertNotIn('codex:',result.stdout)
            self.assertIn('ok: true',result.stdout)
        calls=[json.loads(line) for line in (self.root/'cli-calls').read_text().splitlines()]
        self.assertEqual(calls,[['config','--effective'],['config','--effective']])
        self.provider.chmod(0o600)
        result=self.invoke('preflight.sh','mixed')
        self.assertEqual(result.returncode,1)
        self.assertIn('MISSING',result.stdout)

    def test_current_cli_catalog_drives_preflight_without_provider_execution(self):
        profile=self.root/'.revust/prompts/profiles/fixture.md'
        profile.parent.mkdir(parents=True)
        profile.write_text('---\nmodel: claude/fixture\nagents:\n  - name: finder\n    lenses: [bugs]\n---\nInspect {{SCOPE}}\n')
        (self.root/'.revust/config').write_text('profile = fixture\n')
        (self.root/'.revust/extensions.json').write_text(json.dumps({'version':1,'policy':{
            'stages':{'synthesis':{'enabled':False},'verify':{'enabled':False}},
            'executors':{'claude':{'executable':str(self.provider)}}}}))
        before=hashes(self.root/'.revust')
        result=self.invoke('preflight.sh',REVUST_BIN=str(ROOT/'target/debug/revust'))
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn('profile: fixture',result.stdout)
        self.assertIn('claude:',result.stdout)
        self.assertNotIn('codex:',result.stdout)
        self.assertIn('ok: true',result.stdout)
        self.assertEqual(hashes(self.root/'.revust'),before)

    def test_preflight_rejects_wrong_invocation_or_broken_resolution(self):
        for arguments in [('--lenses=bugs',),('one','two')]:
            result=self.invoke('preflight.sh',*arguments)
            self.assertEqual(result.returncode,1)
            self.assertIn('ok: false',result.stdout)
        result=self.invoke('preflight.sh',CONFIG_FAIL='1')
        self.assertEqual(result.returncode,1)
        self.assertIn('config: FAILED',result.stderr)

    def test_task_inventory_uses_cli_completion_and_does_not_write_inputs(self):
        self.value['paths']['tasks']=[{'id':'task','description':'Authored description','rounds':['01']}]
        for run in ('01','02','03'):
            self.write(run,'input/scope.md','scope')
        self.write('01','manifest.json',{})
        self.write('02','manifest.json',None)
        before=hashes(self.tasks)
        result=self.invoke('task-state.sh','task')
        self.assertEqual(result.returncode,0,result.stderr)
        for observation in ['description: Authored description','round: 01 ran','round: 02 claimed','round: 03 prepared']:
            self.assertIn(observation,result.stdout)
        self.assertEqual(hashes(self.tasks),before)
        for task in ('../outside','a/b','a\\b','.hidden','a..b'):
            self.assertEqual(self.invoke('task-state.sh',task).returncode,1)

    def test_corpus_categories_missing_stages_unknowns_and_read_only_evidence(self):
        before={'findings':[{'id':'a','severity':'major','lenses':['bugs'],'sources':['one']}, {'id':'b','severity':'minor','lenses':['docs','bugs'],'sources':['one','two']} ]}
        after={'pre_existing':[{'id':'a','severity':'minor','lenses':['bugs']}], 'findings':[{'id':'b','severity':'minor','lenses':['docs','bugs']}]}
        for run in ('01','02','03'):
            self.write(run,'manifest.json',{'run':run})
            self.write(run,'stages/1-found.json',before)
        self.write('01','stages/3-verified.json',after)
        self.write('01','findings.json',after)
        self.write('02','stages/2-synthesized.json',{'findings':[before['findings'][0]]})
        self.write('04-corrupt','manifest.json',{})
        self.write('04-corrupt','stages/1-found.json','broken')
        self.write('05-pending','stages/1-found.json',before)
        snapshot=hashes(self.tasks)
        result=self.invoke('analyze-corpus.py','--json')
        self.assertEqual(result.returncode,0,result.stderr)
        data=json.loads(result.stdout)
        self.assertEqual(data['totals']['rounds'],3)
        self.assertEqual(data['totals']['raised'],6)
        self.assertEqual(data['totals']['reports'],1)
        self.assertEqual(data['totals']['verify']['demoted'],1)
        self.assertEqual(data['totals']['synthesis'],{'rounds':1,'removed':1})
        self.assertEqual(data['totals']['demoted_lenses'],{'bugs':1})
        self.assertEqual(data['rounds'][0]['verify']['removed'],0)
        self.assertEqual(data['unreadable'],['task/04-corrupt'])
        self.assertEqual(data['skipped'],['task/05-pending'])
        self.assertFalse(any(row['completion_observed'] for row in data['rounds']))
        self.assertIn('?',data['totals']['lens_naming'])
        self.assertEqual(hashes(self.tasks),snapshot)

    def test_corpus_duplicate_ids_and_unknown_severities_are_not_comparable(self):
        self.write('01','manifest.json',{})
        self.write('01','stages/1-found.json',{'findings':[{'id':'d','severity':'major'},{'id':'d','severity':'minor'},{'id':'u','severity':'unknown'}]})
        self.write('01','stages/3-verified.json',{'immaterial':[{'id':'d','severity':'minor'},{'id':'u','severity':'critical'}]})
        result=self.invoke('analyze-corpus.py','--tasks-dir',self.tasks,'--json')
        data=json.loads(result.stdout)
        observed=data['totals']['verify']
        self.assertEqual(observed['ambiguous'],1)
        self.assertEqual(observed['unknown_severity'],1)
        self.assertEqual(observed['promoted'],0)
        self.assertEqual(observed['demoted'],0)
        self.assertIsNone(data['totals']['gating'])

    def test_selected_task_root_alias_preserves_identity_without_following_descendants(self):
        self.write('01','manifest.json',{})
        self.write('01','stages/1-found.json',{'findings':[{'id':'a','severity':'minor'}]})
        alias=self.root/'selected alias';alias.symlink_to(self.tasks,target_is_directory=True)
        self.value['paths']['tasks_dir']=str(alias)
        result=self.invoke('analyze-corpus.py','--json')
        self.assertEqual(result.returncode,0,result.stderr)
        data=json.loads(result.stdout)
        self.assertEqual(data['tasks_dir'],str(alias))
        self.assertEqual(data['totals']['rounds'],1)
        state=self.invoke('task-state.sh','task')
        self.assertEqual(state.returncode,0,state.stderr)
        self.assertIn('task_dir: '+str(alias/'task'),state.stdout)
        self.assertIn('round: 01 claimed',state.stdout)

    def test_corpus_does_not_follow_artifact_or_parent_links(self):
        self.write('01','manifest.json',{})
        outside=self.root/'outside';outside.mkdir()
        (outside/'1-found.json').write_text('{"findings":[{"id":"PRIVATE_EVIDENCE_MARKER","body":"not for observation"}]}')
        (self.tasks/'task/01/stages').symlink_to(outside,target_is_directory=True)
        result=self.invoke('analyze-corpus.py','--tasks-dir',self.tasks,'--json')
        self.assertEqual(result.returncode,0,result.stderr)
        data=json.loads(result.stdout)
        self.assertEqual(data['totals']['rounds'],0)
        self.assertEqual(data['unreadable'],['task/01'])
        self.assertNotIn('PRIVATE_EVIDENCE_MARKER',result.stdout)
        self.assertNotIn('not for observation',result.stdout)


if __name__=='__main__':
    unittest.main()
