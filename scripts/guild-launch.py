#!/usr/bin/env python3
"""Observe one versioned Guild job with a mandatory neutral archive viewer."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import uuid
from helper_common import safe

FINAL = {'exited', 'interrupted', 'failed_to_start'}
STATES = FINAL | {'starting', 'running', 'cancelling'}
FORWARDED = ('PATH', 'HOME', 'XDG_CONFIG_HOME', 'CLAUDE_CONFIG_DIR', 'CODEX_HOME', 'TMPDIR')


class ProtocolError(Exception):
    pass


class Guild:
    def __init__(self):
        if not all(os.environ.get(key) for key in ('GUILD_SESSION', 'GUILD_SOCKET')):
            raise ProtocolError('Guild requires the actual GUILD_SESSION and GUILD_SOCKET')
        self.program = shutil.which('guild')
        if not self.program:
            raise ProtocolError('guild CLI is unavailable')
        answer = self.request('capabilities')
        operations = answer.get('operations')
        required = {'start', 'status', 'wait', 'close', 'archive_viewer'}
        if (answer.get('kind') != 'capabilities' or type(answer.get('version')) is not int or answer.get('version') != 1
                or not isinstance(operations, list) or not all(isinstance(item, str) for item in operations)
                or not required.issubset(operations)):
            raise ProtocolError('Guild compatibility: version-1 terminal jobs with archive_viewer are required; update the selected Guild build before starting')

    def request(self, *arguments):
        result = subprocess.run([self.program, 'terminal', *arguments, '--json'], capture_output=True, timeout=40)
        if result.returncode:
            raise ProtocolError('Guild did not acknowledge ' + arguments[0])
        try:
            answer = json.loads(result.stdout)
            if answer.get('answer') != 'terminal' or not isinstance(answer.get('reply'), dict):
                raise ValueError()
            return answer['reply']
        except (ValueError, AttributeError) as error:
            raise ProtocolError('invalid Guild terminal response') from error

    def receipt(self, answer, expected=None):
        receipt = answer.get('receipt')
        if answer.get('kind') != 'job' or not isinstance(receipt, dict) or not isinstance(receipt.get('state'), str) or receipt.get('state') not in STATES:
            raise ProtocolError('invalid Guild job receipt')
        identity = receipt.get('id')
        try:
            uuid.UUID(identity)
        except (ValueError, TypeError, AttributeError):
            raise ProtocolError('invalid Guild job identity')
        if expected is not None and identity != expected:
            raise ProtocolError('Guild observation changed job identity')
        return receipt


def main(arguments):
    artifacts = None
    dispatched = False
    terminal = False
    try:
        guild = Guild()  # Capability errors precede config resolution and dispatch.
        if len(arguments) < 2 or '--no-tui' in arguments[1:]:
            raise ProtocolError('expected executable and review arguments without --no-tui')
        program = str(Path(arguments[0]).resolve())
        argv = arguments[1:]
        if not any(value == '--auto-exit' or value.startswith('--auto-exit=') for value in argv):
            delay = os.environ.get('REVUST_AUTO_EXIT', '30s')
            if delay != '0':
                argv = [*argv, '--auto-exit=' + delay]
        config = subprocess.run([program, 'config', *argv], capture_output=True, timeout=40)
        try:
            archive = json.loads(config.stdout)['paths']['archive'] if config.returncode == 0 else None
            if not isinstance(archive, str) or not Path(archive).is_absolute():
                raise ValueError()
        except (ValueError, KeyError, TypeError):
            raise ProtocolError('selected executable did not resolve an absolute archive directory')
        artifacts = Path(tempfile.mkdtemp(prefix='revust-guild-'))
        for name in ('report', 'stderr'):
            descriptor = os.open(artifacts / name, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            os.close(descriptor)
        descriptor = artifacts / 'request.json'
        spec = {
            'version': 1, 'request_id': str(uuid.uuid4()),
            'program': '/bin/bash',
            'args': [str(Path(__file__).resolve().with_name('guild-run.sh')), str(artifacts / 'report'), str(artifacts / 'stderr'), program, *argv],
            'cwd': os.getcwd(), 'env': {key: os.environ[key] for key in FORWARDED if key in os.environ},
            'target': {'kind': 'session', 'id': os.environ['GUILD_SESSION']},
            'title': 'revust', 'foreground': False,
            'geometry': {'width_percent': 90, 'height_percent': 90},
            'archive_viewer': {'directory': archive, 'program': program},
        }
        with os.fdopen(os.open(descriptor, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), 'w') as output:
            json.dump(spec, output)
        dispatched = True
        for attempt in range(2):
            try:
                receipt = guild.receipt(guild.request('start', '--from-file', str(descriptor)))
                break
            except (ProtocolError, subprocess.TimeoutExpired):
                if attempt:
                    raise
                # Retry the exact immutable descriptor/key, never another job.
        identity = receipt['id']
        (artifacts / 'job-id').write_text(identity)
        while receipt['state'] not in FINAL:
            receipt = guild.receipt(guild.request('wait', identity, '--timeout', '30'), identity)
        terminal = True
        if receipt.get('error') or receipt['state'] != 'exited' or receipt.get('signal') or receipt.get('cancellation'):
            raise ProtocolError('Guild job did not produce a normal process result')
        code = receipt.get('exit_code')
        if type(code) is not int or not 0 <= code <= 255:
            raise ProtocolError('Guild process result has no valid exit code')
        diagnostics = (artifacts / 'stderr').read_bytes()
        sys.stderr.buffer.write(diagnostics)
        data = (artifacts / 'report').read_bytes()
        if not data and code in (0, 1):
            raise ProtocolError('no report produced; terminal completion is not review success')
        sys.stdout.buffer.write(data)
        try:
            guild.request('close', identity)
        except (ProtocolError, subprocess.TimeoutExpired):
            print('warning: completed Guild presentation remains available', file=sys.stderr)
        shutil.rmtree(artifacts)
        artifacts = None
        return code
    except (ProtocolError, OSError, ValueError, TypeError, RuntimeError, subprocess.SubprocessError) as error:
        print('error: Guild launcher: ' + safe(error), file=sys.stderr)
        return 3
    except KeyboardInterrupt:
        print('Guild observation interrupted; the job may continue. Inspect Terminal Jobs before another start.', file=sys.stderr)
        return 130
    finally:
        if artifacts:
            if dispatched:
                print(('completed' if terminal else 'possibly running') + ' job artifacts retained at ' + json.dumps(str(artifacts)), file=sys.stderr)
            else:
                shutil.rmtree(artifacts)


if __name__ == '__main__':
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    sys.exit(main(sys.argv[1:]))
