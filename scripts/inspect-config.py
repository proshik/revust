#!/usr/bin/env python3
"""Render selected invocation readiness and saved task inventory without execution."""
import os
from pathlib import Path
import shutil
import stat
import sys
from helper_common import HelperError, configuration, executable, safe, open_readonly


def preflight(arguments):
    lenses = '--lenses' in arguments
    profiles = [value for value in arguments if value != '--lenses']
    if len(profiles) > 1 or any(value.startswith('-') for value in profiles):
        raise HelperError('usage: preflight.sh [PROFILE] [--lenses]')
    profile = profiles[0] if profiles else None
    config = configuration('--effective', *(['--profile', profile] if profile else []))
    try:
        selected = config['selected_profile']
        policy = config['policy']['values']
        names = {selected['runner']['executor']} if lenses else {agent['executor'] for agent in selected['roster']}
        for stage in selected['stages']:
            if policy.get('stages', {}).get(stage['name'], {}).get('enabled') is not False:
                names.add((stage.get('runner') or stage)['executor'])
        if not names or not all(isinstance(name, str) for name in names):
            raise ValueError()
        print('revust: ' + safe(executable()))
        print('profile: ' + safe(selected['name']))
        missing = False
        for name in sorted(names):
            program = policy.get('executors', {}).get(name, {}).get('executable')
            located = shutil.which(program) if isinstance(program, str) and program else None
            print(safe(name) + ': ' + (safe(located) if located else 'MISSING - required by this invocation'))
            missing |= located is None
        print('ok: ' + str(not missing).lower())
        return int(missing)
    except (KeyError, TypeError, ValueError):
        raise HelperError('configuration does not describe the selected invocation')


def regular_size(path):
    try:
        descriptor = open_readonly(path)
        try:
            return os.fstat(descriptor).st_size
        finally:
            os.close(descriptor)
    except OSError:
        return None


def context_count(path):
    if not os.path.lexists(path):
        return '0'
    pending = [path]
    seen = files = 0
    while pending:
        directory = pending.pop()
        try:
            descriptor = open_readonly(directory, directory=True)
            try:
                with os.scandir(descriptor) as entries:
                    for entry in entries:
                        seen += 1
                        if seen > 1024:
                            return str(files) + '+ (inventory limit)'
                        if entry.is_dir(follow_symlinks=False):
                            pending.append(directory / entry.name)
                        elif entry.is_file(follow_symlinks=False):
                            files += 1
            finally:
                os.close(descriptor)
        except OSError:
            return 'unknown'
    return str(files)


def task_state(arguments):
    if len(arguments) != 1:
        raise HelperError('usage: task-state.sh TASK')
    task = arguments[0]
    if not task or task.startswith('.') or '..' in task or any(ch in task for ch in '/\\'):
        raise HelperError('invalid task ID; use a single non-hidden path component')
    config = configuration()
    try:
        selected_root = Path(config['paths']['tasks_dir'])
        tasks = selected_root.resolve()
        entries = config['paths'].get('tasks', [])
        metadata = next((item for item in entries if item.get('id') == task), {})
        completed = set(metadata.get('rounds', []))
    except (KeyError, TypeError):
        raise HelperError('configuration has no usable task inventory')
    directory = tasks / task
    print('tasks_dir: ' + safe(selected_root))
    print('task_dir: ' + safe(selected_root / task))
    if not directory.is_dir() or directory.is_symlink():
        print('exists: false\nrounds: none')
        return 0
    print('exists: true')
    for field in ('description', 'url', 'branch', 'base', 'meta_error', 'rounds_error'):
        if metadata.get(field):
            print(field + ': ' + safe(metadata[field]))
    descriptor = open_readonly(directory, directory=True)
    try:
        with os.scandir(descriptor) as stream:
            rounds = sorted(entry.name for entry in stream if entry.is_dir(follow_symlinks=False))
    finally:
        os.close(descriptor)
    print('rounds: ' + (' '.join(map(safe, rounds)) if rounds else 'none'))
    for name in rounds:
        root = directory / name
        status = 'ran' if name in completed else 'claimed' if os.path.lexists(root / 'manifest.json') else 'prepared'
        values = []
        for field in ('scope', 'goal', 'profile'):
            size = regular_size(root / 'input' / (field + '.md'))
            values.append(field + '=' + ('present' if size else 'MISSING' if field == 'scope' else 'absent'))
        values.append('context=' + context_count(root / 'input/context'))
        print('round: ' + safe(name) + ' ' + status + ' ' + ' '.join(values))
    return 0


def main(arguments):
    operation, *options = arguments
    try:
        return preflight(options) if operation == 'preflight' else task_state(options)
    except (HelperError, OSError, ValueError, TypeError) as error:
        print('error: ' + safe(error), file=sys.stderr)
        if operation == 'preflight':
            print('ok: false')
        return 1


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
