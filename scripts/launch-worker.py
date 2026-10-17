#!/usr/bin/env python3
"""Execute a frozen launch transaction once; only its owner publishes a result."""
import json
import os
from pathlib import Path
import subprocess
import sys


def atomic(path, value):
    temporary = path.with_suffix('.pending')
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, 'w') as output:
        json.dump(value, output)
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)


def execute(program, arguments, report, diagnostics, cwd=None, environment=None):
    # These leaves are created by the transaction owner, in its private directory.
    # Opening diagnostics first preserves a failure to open the report.
    with open(diagnostics, 'wb') as errors:
        try:
            with open(report, 'wb') as output:
                return subprocess.run([program, *arguments], cwd=cwd, env=environment,
                                      stdout=output, stderr=errors).returncode
        except OSError as error:
            errors.write(('launch failed: ' + str(error) + '\n').encode())
            return 3


def main(arguments):
    if arguments[:1] == ['--direct']:
        report, errors, program, *argv = arguments[1:]
        return execute(program, argv, report, errors)
    root = Path(arguments[0])
    spec = json.loads((root / 'request.json').read_text())
    # A second delivery of the same script cannot dispatch execution again.
    try:
        claim = os.open(root / 'claim', os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        return 3
    os.close(claim)
    atomic(root / 'started.json', {'nonce': spec['nonce'], 'pid': os.getpid()})
    environment = {**os.environ, **spec['env']}
    code = execute(spec['program'], spec['args'], root / 'report', root / 'stderr',
                   spec['cwd'], environment)
    if code < 0:
        code = 128 - code
    atomic(root / 'result.json', {'nonce': spec['nonce'], 'exit_code': code})
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
