#!/usr/bin/env python3
"""Prepare an isolated native host fixture. No app, agent or model is launched."""
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import sys
import tempfile

REPO = Path(__file__).resolve().parents[2]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--guild-bin', type=Path, help='Directory holding dev guild and guild-desktop')
p.add_argument('--terminal', action='store_true', help='Prepare Ghostty fixture without Guild')
p.add_argument('--revust-bin', type=Path, default=REPO/'target/debug/revust', help='CLI build to qualify')
a = p.parse_args()
if not a.terminal and a.guild_bin is None: p.error('--guild-bin is required unless --terminal is selected')
guild = a.guild_bin.resolve() if a.guild_bin else None
binary = a.revust_bin.resolve()
files = (binary,) if a.terminal else (binary, guild/'guild', guild/'guild-desktop')
for file in files:
    if not file.is_file(): p.error('Missing build: ' + str(file))
root = Path(tempfile.mkdtemp(prefix='revust-terminal-' if a.terminal else 'revust-guild-', dir='/private/tmp'))
for name in ('project', 'state', 'bin', 'shots', 'home', 'tmp'): (root / name).mkdir()
(root / 'state/settings.json').write_text(json.dumps({'default_tab':'terminal', 'language':'english'}))
project = root / 'project'
config = project / '.revust'
(config / 'extension-profiles').mkdir(parents=True)
(config / 'prompts').mkdir()
executors = ['claude','codex','opencode']
runner = lambda e: {'executor':e,'model':'fixture/model'}
profile = {'runner':runner('claude'), 'agents':[{'name':'a'+str(i), 'lenses':['bugs'], 'runner':runner(e)} for i,e in enumerate(executors)], 'stages':{'synthesis':runner('claude'),'verify':runner('opencode')}}
(config / 'extension-profiles/host.md').write_text('---\n'+json.dumps(profile)+'\n---\nFIND\n{{SCOPE}}\n')
(config / 'prompts/synthesis.md').write_text('SYNTHESIS\n{{FINDINGS}}\n{{SOURCES}}\n')
(config / 'prompts/verify.md').write_text('VERIFY\n{{FINDINGS}}\n')
(project / 'sample.rs').write_text('// Unchanged source for controlled host acceptance.\n')
provider = '''#!PYTHON
import json,os,sys,time
from pathlib import Path
root=Path(__file__).resolve().parents[1]
executor=Path(__file__).name
if '--version' in sys.argv:
 print('opencode v2.0.21'); sys.exit()
prompt=sys.stdin.read(); stage=prompt.splitlines()[0]
assert stage in ('FIND','SYNTHESIS','VERIFY'), stage
assert Path('sample.rs').read_text().startswith('// Unchanged')
with (root/'calls.jsonl').open('a') as f:
 f.write(json.dumps({'executor':executor,'stage':stage,'pid':os.getpid(),'cwd':os.getcwd()})+'\\n')
finding={'file':'sample.rs','line':1,'title':'HOST_FINDING_MARKER','body':'**Controlled finding** with `code`.','fix':'Use the checked value.','confidence':90,'severity':'major'}
if stage=='FIND': output={'findings':[finding]}
elif stage=='SYNTHESIS': output={'findings':[dict(finding,merged_ids=['a0-1','a1-1','a2-1'])],'open_questions':[],'pre_existing':[]}
else: output={'verdicts':[{'id':'a0-1','verdict':'confirmed'}]}
time.sleep(float(os.environ.get('REVUST_FIXTURE_DELAY','1')))
if executor=='claude':
 print(json.dumps({'type':'result','structured_output':output,'usage':{'input_tokens':2,'output_tokens':3}}))
elif executor=='codex':
 print(json.dumps(output)); print('model: observed\\ntokens used\\n5',file=sys.stderr)
else:
 print(json.dumps({'type':'text','part':{'id':'text','messageID':'message','text':json.dumps(output)}}))
 print(json.dumps({'type':'step_finish','part':{'messageID':'message','reason':'stop','tokens':{'input':2,'output':3,'cache':{'read':0,'write':0}}}}))
'''.replace('PYTHON', sys.executable)
for executor in executors:
    file=root/'bin'/executor; file.write_text(provider); file.chmod(0o700)
(root/'bin/revust').symlink_to(binary)
(config/'extensions.json').write_text(json.dumps({'version':1,'profiles':{'host':{'file':'extension-profiles/host.md'}},'policy':{'executors':{e:{'executable':str(root/'bin'/e)} for e in executors},'runtime':{'stagger_delay':'0s','max_parallel':1},'budget':{'max_calls':5}}}))
script='''#!/bin/bash
set -u
root=ROOT
export PATH="$root/bin":FIXTURE_HOST_PATH:"$PATH"
export REVUST_HOST=FIXTURE_HOST_KIND REVUST_AUTO_EXIT=2s
TERMINAL_ENV
run=${1:-automatic}
shift || true
cd "$root/project" || exit 3
# Refuse to resolve any inherited profile after fixture files were removed.
[ -f .revust/extensions.json ] || { echo 'Missing controlled fixture configuration' >&2; exit 3; }
for executor in claude codex opencode; do
    [ -x "$root/bin/$executor" ] || { echo 'Missing controlled fixture executor' >&2; exit 3; }
done
mkdir -p "tasks/host/$run/input"
printf '# HOST_SCOPE_MARKER\\n\\nControlled mixed executor fixture.\\n' > "tasks/host/$run/input/scope.md"
PYTHON -c 'import json,os,sys; json.dump({k:os.environ.get(k) for k in ("GUILD_SESSION","GUILD_SOCKET")},open(sys.argv[1],"w"))' "$root/identity.json"
code=0
bash LAUNCHER --task=host --run="$run" --tasks-dir=tasks --profile=host "$@" > "$root/$run.report.json" 2> "$root/$run.stderr" || code=$?
printf '%s\\n' "$code" > "$root/$run.exit"
printf '\\nHost fixture %s exited %s\\n' "$run" "$code"
exit "$code"
'''
terminal_env='''unset GUILD_SESSION GUILD_SOCKET AGTERM_SOCKET AGTERM_SESSION_ID TMUX ZELLIJ HERDR_ENV KITTY_LISTEN_ON WEZTERM_PANE CMUX_SURFACE_ID INSIDE_EMACS ITERM_SESSION_ID __CFBundleIdentifier GHOSTTY_RESOURCES_DIR GHOSTTY_BIN_DIR
export TERM_PROGRAM=ghostty HOME="$root/home" XDG_CONFIG_HOME="$root/home/.config"
export CODEX_HOME="$root/home/.codex" CLAUDE_CONFIG_DIR="$root/home/.claude" TMPDIR="$root/tmp"
export PATH="$root/bin:/usr/bin:/bin:/usr/sbin:/sbin"''' if a.terminal else ''
script=script.replace('TERMINAL_ENV',terminal_env)
for key,value in {'ROOT':root,'FIXTURE_HOST_PATH':'/usr/bin:/bin' if a.terminal else guild,'FIXTURE_HOST_KIND':'auto' if a.terminal else 'guild','PYTHON':sys.executable,'LAUNCHER':REPO/'scripts/launch-revust.sh'}.items(): script=script.replace(key,shlex.quote(str(value)))
(root/'run-free.sh').write_text(script); (root/'run-free.sh').chmod(0o700)
(root/'builds.json').write_text(json.dumps({str(f):hashlib.sha256(f.read_bytes()).hexdigest() for f in files},indent=2))
print(root)
