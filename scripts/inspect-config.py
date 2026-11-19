#!/usr/bin/env python3
"""Render selected invocation readiness and saved task inventory without execution."""
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
from helper_common import HelperError, configuration, executable, safe, open_readonly


PREFLIGHT_USAGE = 'usage: preflight.sh [--config-dir PATH] [PROFILE] [--lenses]'
TASK_USAGE = 'usage: task-state.sh [--tasks-dir PATH] [--config-dir PATH] TASK'


def explicit_roots(arguments, names, usage):
    """Split out the session's explicit roots, kept in the given order for `revust config`."""
    roots, rest, seen = [], [], set()
    values = iter(arguments)
    for argument in values:
        name, separator, value = argument.partition('=')
        if name not in names:
            rest.append(argument)
            continue
        if not separator:
            value = next(values, '')
            if value.startswith('-'):
                value = ''
        if not value:
            raise HelperError(name + ' requires a non-empty PATH; ' + usage)
        if name in seen:
            raise HelperError(name + ' is given more than once; ' + usage)
        seen.add(name)
        roots.append(name + '=' + value)
    return roots, rest


def preflight(arguments):
    roots, arguments = explicit_roots(arguments, ('--config-dir',), PREFLIGHT_USAGE)
    lenses = '--lenses' in arguments
    profiles = [value for value in arguments if value != '--lenses']
    if len(profiles) > 1 or any(value.startswith('-') for value in profiles):
        raise HelperError(PREFLIGHT_USAGE)
    profile = profiles[0] if profiles else None
    config = configuration('--effective', *roots, *(['--profile', profile] if profile else []))
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


def physical(path):
    """Symlink-free path; a not yet created tail is kept below its nearest existing parent."""
    path, missing = Path(os.path.abspath(path)), []
    while not os.path.exists(path) and path != path.parent:
        missing.append(path.name)
        path = path.parent
    return Path(os.path.realpath(path)).joinpath(*reversed(missing))


def task_directory(tasks, task, approved):
    """The task's physical directory, or None.

    A link is followed only for a catalog-approved relative alias whose target
    stays inside the tasks root; rounds below it keep the no-follow walk.
    """
    directory = tasks / task
    if not directory.is_symlink():
        return directory if directory.is_dir() else None
    if not approved or os.path.isabs(os.readlink(directory)):
        return None
    target = Path(os.path.realpath(directory))
    return target if tasks in target.parents and target.is_dir() else None


def holding_worktree(tasks_root):
    """The caller's linked Git worktree when the tasks root lies inside it, else None."""
    try:
        result = subprocess.run(['git', 'rev-parse', '--git-dir', '--git-common-dir', '--show-toplevel'],
                                capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    lines = result.stdout.splitlines()
    if result.returncode or len(lines) != 3:
        return None
    git_dir, common_dir, top = (Path(os.path.realpath(line)) for line in lines)
    tasks = physical(tasks_root)
    return top if git_dir != common_dir and (tasks == top or top in tasks.parents) else None


def task_state(arguments):
    roots, arguments = explicit_roots(arguments, ('--tasks-dir', '--config-dir'), TASK_USAGE)
    if len(arguments) != 1 or arguments[0].startswith('-'):
        raise HelperError(TASK_USAGE)
    task = arguments[0]
    if not task or task.startswith('.') or '..' in task or any(ch in task for ch in '/\\'):
        raise HelperError('invalid task ID; use a single non-hidden path component')
    config = configuration(*roots)
    try:
        selected_root = Path(config['paths']['tasks_dir'])
        tasks = selected_root.resolve()
        entries = config['paths'].get('tasks', [])
        metadata = next((item for item in entries if item.get('id') == task), None)
        approved = metadata is not None
        metadata = metadata or {}
        completed = set(metadata.get('rounds', []))
    except (KeyError, TypeError):
        raise HelperError('configuration has no usable task inventory')
    print('tasks_dir: ' + safe(selected_root))
    print('task_dir: ' + safe(selected_root / task))
    worktree = holding_worktree(selected_root)
    if worktree:
        # An ignored archive does not stop `git worktree remove`, which deletes it.
        print('archive_in_worktree: true (removed with worktree ' + safe(worktree) + ')')
    directory = task_directory(tasks, task, approved)
    if directory is None:
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
