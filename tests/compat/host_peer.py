#!/usr/bin/env python3
"""File-backed, synthetic host protocol peer used only by launcher tests."""
import json
import os
from pathlib import Path
import subprocess
import sys

root = Path(os.environ['PEER_ROOT'])
case = json.loads((root / 'case.json').read_text())
role = Path(sys.argv[0]).name
arguments = sys.argv[1:]
with (root / 'trace.jsonl').open('a') as trace:
    trace.write(json.dumps([role, arguments]) + '\n')

if role == 'selected program':
    if arguments and arguments[0] == 'config':
        print(json.dumps(case.get('config', {'paths': {'archive': str(root / 'archive')}})))
    else:
        with (root / 'deliveries.jsonl').open('a') as output:
            output.write(json.dumps({'args': arguments, 'cwd': os.getcwd()}) + '\n')
        if not case.get('empty'):
            sys.stdout.write('Synthetic saved output\n\n')
        print('Synthetic child diagnostic', file=sys.stderr)
        sys.exit(case.get('code', 0))
elif role == 'agtermctl':
    if arguments[:3] == ['session', 'overlay', 'open']:
        (root / 'overlay.json').write_text(json.dumps(arguments))
        sys.exit(subprocess.run(['/bin/bash', '-c', arguments[3]]).returncode)
    if arguments == ['tree']:
        print(json.dumps({'result': {'tree': {'workspaces': [{'sessions': [
            {'id': 'owned-session', 'background': {'colorHex': '#808080'}}]}]}}}))
    elif arguments[:2] == ['theme', 'list']:
        print('* default ghostty')
elif role == 'guild':
    operation = arguments[1]
    reply = None
    if operation == 'capabilities':
        if case.get('unreachable'):
            sys.exit(7)
        reply = {'kind': 'capabilities', 'version': 1,
                 'operations': ['start', 'status', 'wait', 'close', 'archive_viewer']}
        reply.update(case.get('capabilities', {}))
    else:
        saved = root / 'descriptor.json'
        if operation == 'start':
            descriptor_path = Path(arguments[arguments.index('--from-file') + 1])
            descriptor = json.loads(descriptor_path.read_text())
            key = [str(descriptor_path), descriptor]
            if saved.exists():
                assert json.loads(saved.read_text()) == key, 'retry changed descriptor'
            else:
                saved.write_text(json.dumps(key))
                child = subprocess.run([descriptor['program'], *descriptor['args']],
                                       cwd=descriptor['cwd'], env={**os.environ, **descriptor['env']})
                (root / 'exit.json').write_text(str(child.returncode))
                if case.get('missing_report'):
                    Path(descriptor['args'][1]).unlink()
            loss = case.get('lost', 0)
            starts = sum(json.loads(line)[1][1] == 'start' for line in (root / 'trace.jsonl').read_text().splitlines() if json.loads(line)[0] == 'guild')
            if starts <= loss:
                sys.exit(9)
        state = 'running' if operation == 'start' else 'exited'
        pending = root / 'pending'
        if operation == 'wait' and case.get('pending') and not pending.exists():
            pending.touch()
            state = 'running'
        receipt = {'id': 'ab239b52-2831-461d-bde2-4be29b10e2ae', 'state': state,
                   'exit_code': int((root / 'exit.json').read_text())}
        receipt.update(case.get('receipt', {}))
        if operation == 'wait':
            receipt.update(case.get('wait_receipt', {}))
        reply = {'kind': 'job', 'receipt': receipt}
    if case.get('reply_null'):
        reply = None
    print(json.dumps({'answer': 'terminal', 'reply': reply}))
else:
    raise SystemExit('unexpected executable identity')
