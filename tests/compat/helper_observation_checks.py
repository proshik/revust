#!/usr/bin/env python3
"""Public helper observations against authored config and disposable archives."""
import hashlib
import json
import os
from pathlib import Path
import shutil
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

    def invoke(self, helper, *arguments, cwd=None, **environment):
        self.config.write_text(json.dumps(self.value))
        command = [sys.executable, '-B', str(ROOT / 'scripts' / helper)] if helper.endswith('.py') else ['/bin/bash',str(ROOT/'scripts'/helper)]
        result = subprocess.run([*command,*map(str,arguments)],cwd=cwd or self.root,env={**self.environment,**environment},capture_output=True,text=True,timeout=15)
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

    def test_preflight_selects_only_regular_files_as_revust_launches_them(self):
        fifo=self.root/'fifo';fifo.mkdir()
        os.mkfifo(fifo/'opencode',0o700)
        later=self.root/'later';later.mkdir()
        self.value['policy']['values']['executors']['opencode']['executable']='opencode'
        result=self.invoke('preflight.sh','--lenses',PATH=os.pathsep.join([str(fifo),'/usr/bin','/bin']))
        self.assertEqual(result.returncode,1,result.stderr)
        self.assertIn('opencode: MISSING',result.stdout)
        shutil.copy2(self.provider,later/'opencode')
        result=self.invoke('preflight.sh','--lenses',PATH=os.pathsep.join([str(fifo),str(later),'/usr/bin','/bin']))
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn('opencode: '+str(later/'opencode'),result.stdout)

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

    def calls(self):
        return [json.loads(line) for line in (self.root/'cli-calls').read_text().splitlines()]

    def test_explicit_roots_are_forwarded_to_config_in_given_order(self):
        result=self.invoke('task-state.sh','--tasks-dir',self.tasks,'--config-dir=alt config','task')
        self.assertEqual(result.returncode,0,result.stderr)
        result=self.invoke('task-state.sh','--config-dir','alt config','task','--tasks-dir='+str(self.tasks))
        self.assertEqual(result.returncode,0,result.stderr)
        result=self.invoke('preflight.sh','--config-dir','alt config','mixed','--lenses')
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(self.calls(),[['config','--tasks-dir='+str(self.tasks),'--config-dir=alt config'],
                                       ['config','--config-dir=alt config','--tasks-dir='+str(self.tasks)],
                                       ['config','--effective','--config-dir=alt config','--profile','mixed']])

    def test_explicit_root_options_reject_unusable_forms_before_config(self):
        for helper,arguments in [('task-state.sh',('--tasks-dir=','task')),('task-state.sh',('task','--tasks-dir')),
                                 ('task-state.sh',('--config-dir','--tasks-dir','x','task')),
                                 ('task-state.sh',('--tasks-dir','a','--tasks-dir','b','task')),
                                 ('task-state.sh',('--workdir','x','task')),('task-state.sh',('--tasks-dir','x')),
                                 ('preflight.sh',('--config-dir=',)),('preflight.sh',('--tasks-dir','x')),
                                 ('preflight.sh',('--config-dir',))]:
            result=self.invoke(helper,*arguments)
            self.assertEqual(result.returncode,1,(helper,arguments,result.stdout))
            self.assertIn('usage: '+helper,result.stderr)
        self.assertFalse((self.root/'cli-calls').exists())

    def test_task_state_sees_tasks_under_explicit_tasks_and_config_roots(self):
        cli=str(ROOT/'target/debug/revust')
        alternate=self.root/'elsewhere/tasks';(alternate/'task/01/input').mkdir(parents=True)
        (alternate/'task/01/input/scope.md').write_text('scope')
        default=self.invoke('task-state.sh','task',REVUST_BIN=cli)
        self.assertIn('exists: false',default.stdout)
        explicit=self.invoke('task-state.sh','--tasks-dir',alternate,'task',REVUST_BIN=cli)
        self.assertEqual(explicit.returncode,0,explicit.stderr)
        self.assertIn('exists: true',explicit.stdout)
        self.assertIn('round: 01 prepared scope=present',explicit.stdout)
        selected=self.root/'selected config';selected.mkdir()
        (selected/'config').write_text('tasks-dir = '+str(alternate)+'\n')
        configured=self.invoke('task-state.sh','--config-dir',selected,'task',REVUST_BIN=cli)
        self.assertEqual(configured.returncode,0,configured.stderr)
        self.assertIn('tasks_dir: '+str(alternate),configured.stdout)
        self.assertIn('exists: true',configured.stdout)

    def test_preflight_finds_profile_defined_only_in_explicit_config_root(self):
        cli=str(ROOT/'target/debug/revust')
        selected=self.root/'selected config'
        profile=selected/'prompts/profiles/only-here.md';profile.parent.mkdir(parents=True)
        profile.write_text('---\nmodel: claude/fixture\nagents:\n  - name: finder\n    lenses: [bugs]\n---\nInspect {{SCOPE}}\n')
        (selected/'extensions.json').write_text(json.dumps({'version':1,'policy':{
            'stages':{'synthesis':{'enabled':False},'verify':{'enabled':False}},
            'executors':{'claude':{'executable':str(self.provider)}}}}))
        self.assertEqual(self.invoke('preflight.sh','only-here',REVUST_BIN=cli).returncode,1)
        result=self.invoke('preflight.sh','--config-dir',selected,'only-here',REVUST_BIN=cli)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn('profile: only-here',result.stdout)
        self.assertIn('claude: '+str(self.provider).replace('\n','?'),result.stdout)
        self.assertIn('ok: true',result.stdout)

    def test_task_state_follows_catalog_alias_but_not_unapproved_or_escaping_links(self):
        (self.tasks/'alpha/01/input').mkdir(parents=True)
        (self.tasks/'alpha/01/input/scope.md').write_text('scope')
        (self.tasks/'alpha/01/manifest.json').write_text('{}')
        (self.tasks/'alias').symlink_to('alpha',target_is_directory=True)
        cli=str(ROOT/'target/debug/revust')
        result=self.invoke('task-state.sh','--tasks-dir',self.tasks,'alias',REVUST_BIN=cli)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn('task_dir: '+str(self.tasks/'alias'),result.stdout)
        self.assertIn('exists: true',result.stdout)
        self.assertIn('round: 01 ran scope=present',result.stdout)
        outside=self.root/'outside task';(outside/'01/input').mkdir(parents=True)
        (outside/'01/input/scope.md').write_text('scope')
        (self.tasks/'escape').symlink_to('../outside task',target_is_directory=True)
        (self.tasks/'absolute').symlink_to(self.tasks/'alpha',target_is_directory=True)
        # The helper keeps its own containment even when a catalog approves a link.
        self.value['paths']['tasks']=[{'id':name,'rounds':['01']} for name in ('escape','absolute')]
        for task in ('escape','absolute','alias'):
            result=self.invoke('task-state.sh',task)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertIn('exists: false\nrounds: none',result.stdout,task)

    def test_helpers_do_not_write_bytecode_into_their_installation(self):
        installed=self.root/'installed/scripts';installed.mkdir(parents=True)
        names=sorted(['helper_common.py','inspect-config.py','analyze-corpus.py','preflight.sh','task-state.sh',
                      'host-launch.py','guild-launch.py'])
        for name in names:
            shutil.copy2(ROOT/'scripts'/name,installed/name)
        self.config.write_text(json.dumps(self.value))
        # An explicit cache prefix makes local-module caching observable on every
        # Python build, including ones that cache outside the source tree by default.
        prefix=self.root/'bytecode cache'
        for command in [['/bin/bash',installed/'preflight.sh'],['/bin/bash',installed/'task-state.sh','task'],
                        [sys.executable,installed/'analyze-corpus.py','--json']]:
            result=subprocess.run(list(map(str,command)),cwd=self.root,env={**self.environment,'PYTHONPYCACHEPREFIX':str(prefix)},
                                  capture_output=True,text=True,timeout=15)
            self.assertEqual(result.returncode,0,(command,result.stderr))
            self.assertEqual(sorted(path.name for path in installed.iterdir()),names,command)
            self.assertEqual([path for path in prefix.rglob('helper_common*')],[],command)
        # Launchers import the same helper; a refused start exercises that import
        # without a terminal host or provider.
        for launcher in ['host-launch.py','guild-launch.py']:
            command=[sys.executable,installed/launcher,str(self.program),'--no-tui']
            result=subprocess.run(list(map(str,command)),cwd=self.root,env={**self.environment,'PYTHONPYCACHEPREFIX':str(prefix)},
                                  capture_output=True,text=True,timeout=15)
            self.assertNotEqual(result.returncode,0,(command,result.stderr))
            self.assertTrue(result.stderr.startswith('error: '),(command,result.stderr))
            self.assertEqual(sorted(path.name for path in installed.iterdir()),names,command)
            self.assertEqual([path for path in prefix.rglob('helper_common*')],[],command)

    def git(self, *arguments, cwd):
        subprocess.run(['git','-c','user.name=fixture','-c','user.email=fixture@example.invalid',*arguments],cwd=cwd,
                       env={**self.environment,'GIT_CONFIG_NOSYSTEM':'1'},check=True,capture_output=True,timeout=15)

    def test_task_state_reports_archive_inside_linked_worktree(self):
        main=self.root/'repository';main.mkdir()
        self.git('init','-q',cwd=main)
        self.git('commit','-q','--allow-empty','-m','fixture',cwd=main)
        worktree=self.root/'linked';self.git('worktree','add','-q',str(worktree),cwd=main)
        sibling=self.root/'linked-archive';sibling.mkdir()
        alias=self.root/'linked alias';alias.symlink_to(worktree,target_is_directory=True)
        marker='archive_in_worktree:'
        cases=[(worktree,worktree/'.revust/tasks',True),(alias,alias/'.revust/not yet/tasks',True),
               (alias,worktree,True),(worktree,self.tasks,False),(worktree,sibling,False),
               (main,main/'.revust/tasks',False),(self.root,self.tasks,False)]
        for cwd,tasks,inside in cases:
            self.value['paths']['tasks_dir']=str(tasks)
            result=self.invoke('task-state.sh','task',cwd=cwd)
            self.assertEqual(result.returncode,0,result.stderr)
            lines=[line for line in result.stdout.splitlines() if line.startswith(marker)]
            if inside:
                self.assertEqual(lines,[marker+' true (removed with worktree '+str(worktree.resolve())+')'],(cwd,tasks))
            else:
                self.assertEqual(lines,[],(cwd,tasks))

    def test_corpus_categories_missing_stages_unknowns_and_read_only_evidence(self):
        before={'findings':[{'id':'a','severity':'major','lenses':['bugs'],'sources':['one']}, {'id':'b','severity':'minor','lenses':['docs','bugs'],'sources':['one','two']} ]}
        after={'pre_existing':[{'id':'a','severity':'minor','lenses':['bugs']}], 'findings':[{'id':'b','severity':'minor','lenses':['docs','bugs']}]}
        for run in ('01','02','03'):
            self.write(run,'manifest.json',{'run':run})
            self.write(run,'stages/1-found.json',before)
        self.write('01','stages/3-verified.json',{**after,'stats':{'stages':[{'name':'find'},{'name':'verify'}]}})
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

    def test_corpus_verify_baseline_requires_evidence_that_synthesis_did_not_run(self):
        found={'findings':[{'id':'a','severity':'major'},{'id':'b','severity':'major'}]}
        verified={'findings':[{'id':'a','severity':'minor'}]}
        def ran(*names):
            return {**verified,'stats':{'stages':[{'name':name} for name in names]}}
        for run in ('01-disabled','02-failed','03-corrupt','04-unrecorded','05-large'):
            self.write(run,'manifest.json',{})
            self.write(run,'stages/1-found.json',found)
        self.write('01-disabled','stages/3-verified.json',ran('find','verify'))
        self.write('02-failed','stages/3-verified.json',ran('find','synthesis','verify'))
        self.write('03-corrupt','stages/2-synthesized.json','broken')
        self.write('03-corrupt','stages/3-verified.json',ran('find','synthesis','verify'))
        self.write('04-unrecorded','stages/3-verified.json',verified)
        # Above the old 32 MiB read bound, within the archive writers' ceiling.
        large={'findings':[{**row,'body':'x'*(33<<20) if row['id']=='a' else ''} for row in found['findings']]}
        self.write('05-large','stages/2-synthesized.json',large)
        self.write('05-large','stages/3-verified.json',ran('find','synthesis','verify'))
        result=self.invoke('analyze-corpus.py','--tasks-dir',self.tasks,'--json')
        self.assertEqual(result.returncode,0,result.stderr)
        data=json.loads(result.stdout)
        rounds={row['run']:row for row in data['rounds']}
        self.assertEqual(rounds['01-disabled']['verify']['removed'],1)
        self.assertEqual(rounds['01-disabled']['verify']['demoted'],1)
        for run,unavailable in [('02-failed',['verify']),('03-corrupt',['synthesis','verify']),('04-unrecorded',['verify'])]:
            self.assertNotIn('verify',rounds[run],run)
            self.assertNotIn('synthesis',rounds[run],run)
            self.assertEqual(rounds[run]['unavailable'],unavailable,run)
        self.assertEqual(rounds['05-large']['synthesis'],{'removed':0})
        self.assertEqual(rounds['05-large']['verify']['removed'],1)
        self.assertNotIn('unavailable',rounds['05-large'])
        self.assertEqual(data['totals']['synthesis'],{'rounds':1,'removed':0})
        self.assertEqual(data['totals']['verify']['rounds'],2)
        self.assertEqual(data['totals']['verify']['removed'],2)

    def test_corpus_duplicate_ids_and_unknown_severities_are_not_comparable(self):
        self.write('01','manifest.json',{})
        self.write('01','stages/1-found.json',{'findings':[{'id':'d','severity':'major'},{'id':'d','severity':'minor'},{'id':'u','severity':'unknown'}]})
        self.write('01','stages/3-verified.json',{'immaterial':[{'id':'d','severity':'minor'},{'id':'u','severity':'critical'}],
                                                  'stats':{'stages':[{'name':'find'},{'name':'verify'}]}})
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
