"""Exercise public live/archive TUI commands in a disposable PTY with a fake provider."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[2]
BINARY = Path(os.environ.get('REVUST_TEST_BINARY', str(ROOT / 'target/debug/revust'))).resolve()

def wait_input_consumed(slave, master, timeout=5):
    """Wait for the terminal reader to drain raw PTY input before signalling it."""
    deadline = time.monotonic() + timeout
    while struct.unpack('i', fcntl.ioctl(slave, termios.FIONREAD, struct.pack('i', 0)))[0]:
        assert time.monotonic() < deadline, 'terminal reader did not consume PTY input'
        # Keep draining frames so terminal output cannot block the input reader.
        if select.select([master], [], [], .01)[0]:
            os.read(master, 65536)

def session(root, args, interaction, *, terminal_stdout=False):
    master, slave = pty.openpty()
    before = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))
    mode = '--terminal-child' if terminal_stdout else '--child'
    child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), mode, str(root), *args], stdin=slave, stderr=slave, stdout=subprocess.PIPE, start_new_session=True)
    screen = bytearray()
    def until(marker):
        deadline = time.monotonic() + 10
        while marker not in screen:
            assert time.monotonic() < deadline, (args, marker, bytes(screen)[-1000:])
            if select.select([master], [], [], .05)[0]: screen.extend(os.read(master, 65536))
    def send(text, *, wait_consumed=False):
        screen.clear()
        os.write(master, text)
        if wait_consumed:
            wait_input_consumed(slave, master)
    def resize(cols, rows):
        screen.clear()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
        os.killpg(child.pid, signal.SIGWINCH)
        until(b'\x1b[2J')
        del screen[:screen.index(b'\x1b[2J') + len(b'\x1b[2J')]
        until(b'FINDING_MARKER')
    def remain(seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master, child.stdout], [], [], .05)
            assert child.stdout not in ready, 'observer closed before an explicit quit'
            if master in ready:
                screen.extend(os.read(master, 65536))
    try:
        until(b'revust')
        interaction(until, send, resize, remain)
        until(b'\x1b[?1049l')
        deadline = time.monotonic() + 10
        while True:
            ready, _, _ = select.select([master, child.stdout], [], [], .05)
            if master in ready:
                screen.extend(os.read(master, 65536))
            if child.stdout in ready:
                break
            assert time.monotonic() < deadline, 'command did not return'
        result = json.loads(child.stdout.readline())
        assert termios.tcgetattr(slave) == before, 'terminal modes were not restored'
        after_tui = bytes(screen).split(b'\x1b[?1049l', 1)[1]
        send(b'ack\n'); until(b'ack\r\n')
        child.communicate(timeout=5)
        assert child.returncode == 0
        if terminal_stdout:
            assert b'"findings"' not in after_tui and b'FINDING_MARKER' not in after_tui, after_tui
        return result
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
        os.close(master); os.close(slave)

def hashes(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file()}

def saved_outcome(round, state):
    marker = json.loads((round / 'manifest.json').read_text())
    report = json.loads((round / 'findings.json').read_text())
    assert marker['outcome'] == report['outcome']
    assert report['outcome']['state'] == state, report['outcome']
    before = hashes(round)
    viewed = subprocess.run([str(BINARY), 'view', '--archive', str(round), '--no-tui'],
                            env={'HOME': str(round), 'PATH': '/usr/bin:/bin'}, capture_output=True, timeout=5)
    assert viewed.returncode == 2, viewed.stderr
    assert json.loads(viewed.stdout)['outcome'] == report['outcome']
    assert b'review incomplete' in viewed.stderr
    assert hashes(round) == before, 'saved viewing changed archive bytes'
    return report

def hangup_session(root, args):
    """Destroy the terminal, rather than send a signal while keeping it usable."""
    master, slave = pty.openpty()
    child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), '--hangup-child', str(root), *args],
                             stdin=slave, stderr=slave, stdout=subprocess.PIPE, start_new_session=True)
    os.close(slave)
    try:
        deadline = time.monotonic() + 10
        while not (root / 'provider.pid').exists():
            assert time.monotonic() < deadline, 'owned provider did not start'
            assert child.poll() is None, 'review exited before terminal close'
            if select.select([master], [], [], .05)[0]: os.read(master, 65536)
        # Let the event reader enter its wait before revoking the PTY.
        time.sleep(.1)
        os.close(master); master = None
        try:
            stdout, _ = child.communicate(timeout=5)
        except subprocess.TimeoutExpired as error:
            raise AssertionError('revust survived terminal destruction') from error
        assert child.returncode == 2 and not stdout, (child.returncode, stdout)
        pid = int((root / 'provider.pid').read_text())
        try: os.kill(pid, 0)
        except ProcessLookupError: pass
        else: raise AssertionError('owned provider survived terminal destruction')
    finally:
        if child.poll() is None:
            child.kill(); child.communicate(timeout=5)
        if master is not None: os.close(master)

def main():
    with tempfile.TemporaryDirectory(prefix='revust-public-tui-') as directory:
        root = Path(directory)
        profiles = root / '.revust/prompts/profiles'; profiles.mkdir(parents=True)
        (profiles / 'test.md').write_text('---\nmodel: claude/sonnet\nagents:\n  - name: a\n    lenses: [bugs]\n---\nReview {{SCOPE}}\n')
        provider = root / 'provider'
        answer = json.dumps({'type':'result','structured_output':{'findings':[{'title':'FINDING_MARKER','file':'a.rs','line':1,'severity':'major','confidence':90,'body':'**BODY_MARKER** with `code`','fix':'Use the new value'}]}})
        provider.write_text("#!/bin/sh\n/bin/cat >/dev/null\nprintf '%s\\n' '" + answer + "'\n")
        provider.chmod(0o700)
        (root / '.revust/extensions.json').write_text(json.dumps({'version':1,'policy':{'executors':{'claude':{'executable':str(provider)}},'runtime':{'stagger_delay':'0s'}}}))
        round = root / 'tasks/test/01'; (round / 'input').mkdir(parents=True)
        (round / 'input/scope.md').write_text('# SCOPE_MARKER\n\n- synthetic input')
        args = ['--task=test','--run=01','--tasks-dir=tasks','--profile=test','--no-synthesis','--no-verify']
        def browse(until, send, resize, _remain):
            until(b'FINDING_MARKER')
            resize(60, 24)
            send(b'i'); until(b'SCOPE_MARKER')
            send(b'f'); until(b'FINDING_MARKER')
            send(b'/\x1b[200~absent\x1b[201~\r'); until(b'nothing')
            send(b'/\x7f\x7f\x7f\x7f\x7f\x7f\r'); until(b'FINDING_MARKER')
            send(b'q')
        result = session(root, args, browse)
        assert result['code'] == 1, result
        assert json.loads(result['stdout'])['findings'][0]['title'] == 'FINDING_MARKER'
        assert '\x1b' not in result['stdout']
        before = hashes(round)
        # Viewing must work after removal of current profiles and provider.
        provider.unlink(); (profiles / 'test.md').unlink()
        def archive(until, send, _resize, _remain):
            send(b'f'); until(b'FINDING_MARKER'); send(b'i'); until(b'SCOPE_MARKER'); send(b'q')
        viewed = session(root, ['view','--archive',str(round)], archive)
        assert viewed['code'] == 0, viewed
        assert json.loads(viewed['stdout'])['findings'][0]['title'] == 'FINDING_MARKER'
        assert hashes(round) == before
        viewed = session(root, ['view', '--archive', str(round), '--markdown'], archive)
        assert viewed['code'] == 0 and 'FINDING_MARKER' in viewed['stdout'], viewed
        assert hashes(round) == before
        for format_args in [[], ['--markdown']]:
            viewed = session(root, ['view', '--archive', str(round), *format_args], archive,
                             terminal_stdout=True)
            assert viewed == {'code': 0, 'stdout': ''}, viewed
            assert hashes(round) == before
        # Interrupt a genuinely running owned subprocess and check its PID is gone.
        provider.write_text('#!/bin/sh\nprintf "%s" $$ > provider.pid\nexec /bin/sleep 60\n'); provider.chmod(0o700)
        (profiles / 'test.md').write_text('---\nmodel: claude/sonnet\nagents:\n  - name: a\n    lenses: [bugs]\n---\nReview {{SCOPE}}\n')
        for run, external_signal in [('02', None), ('03', signal.SIGTERM), ('05', signal.SIGHUP), ('07', signal.SIGINT), ('08', signal.SIGTERM)]:
            other = root / f'tasks/test/{run}/input'; other.mkdir(parents=True); (other / 'scope.md').write_text('scope')
            (root / 'provider.pid').unlink(missing_ok=True)
            def interrupt(until, send, _resize, _remain):
                deadline = time.monotonic() + 10
                while not (root / 'provider.pid').exists():
                    assert time.monotonic() < deadline
                    time.sleep(.02)
                if run == '08':
                    send(b'\x1b[', wait_consumed=True)  # unfinished CSI must not block a second read
                if external_signal: os.kill(int((root / 'revust.pid').read_text()), external_signal)
                else: send(b'\x03')
            interrupted = session(root, [a.replace('--run=01', f'--run={run}') for a in args], interrupt)
            assert interrupted['code'] == 2, interrupted
            pid = int((root / 'provider.pid').read_text())
            try: os.kill(pid, 0)
            except ProcessLookupError: pass
            else: raise AssertionError('owned provider survived cancellation')
            saved_outcome(root / f'tasks/test/{run}', {'stopped': 'cancelled'})
            assert 'cancelled' in (root / f'tasks/test/{run}/revust/events.jsonl').read_text()
            assert (root / f'tasks/test/{run}/report.md').is_file()
        other = root / 'tasks/test/06/input'; other.mkdir(parents=True); (other / 'scope.md').write_text('scope')
        (root / 'provider.pid').unlink()
        hangup_session(root, [a.replace('--run=01', '--run=06') for a in args])
        saved_outcome(root / 'tasks/test/06', {'stopped': 'cancelled'})
        assert 'cancelled' in (root / 'tasks/test/06/revust/events.jsonl').read_text()
        assert (root / 'tasks/test/06/report.md').is_file()
        provider.write_text("#!/bin/sh\n/bin/cat >/dev/null\nprintf '%s\\n' '" + answer + "'\n")
        other = root / 'tasks/test/04/input'; other.mkdir(parents=True); (other / 'scope.md').write_text('scope')
        started = time.monotonic()
        result = session(root, [a.replace('--run=01','--run=04') for a in args] + ['--auto-exit=1s'], lambda until, send, resize, remain: until(b'FINDING_MARKER'))
        assert result['code'] == 1 and time.monotonic() - started >= .9, result
        incomplete_countdown(root, provider, answer, args)
    print('Twenty-two public PTY scenarios passed: thirteen review/archive/cancellation cases and nine failed/stopped/policy-failed countdown cases')

def incomplete_countdown(root, provider, answer, args):
    for kind in ['failed', 'stopped', 'policy-failed']:
        for mode in ['countdown', 'zero', 'key']:
            run = kind + '-' + mode
            round = root / 'tasks/test' / run
            (round / 'input').mkdir(parents=True)
            (round / 'input/scope.md').write_text('Synthetic incomplete-result acceptance')
            policy = {'executors': {'claude': {'executable': str(provider)}},
                      'runtime': {'stagger_delay': '0s'}, 'retries': {'find': {'count': 0}}}
            if kind == 'failed':
                body = 'printf SYNTHETIC_FAILURE >&2\nexit 7\n'
                state = 'failed'
            elif kind == 'stopped':
                body = 'printf "%s" $$ > provider.pid\nexec /bin/sleep 60\n'
                policy['budget'] = {'elapsed': '500ms'}
                state = {'stopped': 'deadline'}
            else:
                body = "printf '%s\\n' '" + answer + "'\n"
                policy['completeness'] = {'min_successful_reviewers': 2}
                state = 'complete'
            provider.write_text('#!/bin/sh\n/bin/cat >/dev/null\n' + body)
            provider.chmod(0o700)
            (root / '.revust/extensions.json').write_text(json.dumps({'version': 1, 'policy': policy}))
            def observe(until, send, _resize, remain):
                until(b'incomplete')
                if mode != 'zero': until(b'closing in')
                if kind == 'failed': until(b'Provider exited unsuccessfully')
                if kind == 'policy-failed': until(b'FINDING_MARKER')
                if mode == 'zero':
                    remain(1.3)
                    send(b'q')
                elif mode == 'key':
                    send(b' ', wait_consumed=True)
                    remain(1.3)  # Longer than the original countdown.
                    send(b'q')
            started = time.monotonic()
            result = session(root, [a.replace('--run=01', '--run=' + run) for a in args]
                             + ['--auto-exit=' + ('0s' if mode == 'zero' else '1s')], observe)
            assert result['code'] == 2, result
            if mode == 'countdown': assert time.monotonic() - started >= .9
            report = saved_outcome(round, state)
            if kind == 'failed' and mode == 'key':
                def quit_saved(until, send, _resize, _remain):
                    until(b'incomplete')
                    send(b'q')
                viewed = session(root, ['view', '--archive', str(round)], quit_saved,
                                 terminal_stdout=True)
                assert viewed == {'code': 2, 'stdout': ''}, viewed
            if kind == 'failed':
                assert any('exited unsuccessfully' in text for text in report['outcome']['diagnostics'])
            if kind == 'policy-failed':
                assert report['outcome']['policy_failures'] and report['findings']
            if kind == 'stopped':
                try: os.kill(int((root / 'provider.pid').read_text()), 0)
                except ProcessLookupError: pass
                else: raise AssertionError('owned provider survived deadline')

if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--hangup-child':
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        os.chdir(sys.argv[2])
        os.execve(str(BINARY), [str(BINARY), *sys.argv[3:]],
                  {'HOME':sys.argv[2],'PATH':'/usr/bin:/bin','TERM':'xterm-256color'})
    elif len(sys.argv) > 1 and sys.argv[1] in ['--child', '--terminal-child']:
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        root = sys.argv[2]
        output = 2 if sys.argv[1] == '--terminal-child' else subprocess.PIPE
        p = subprocess.Popen([str(BINARY), *sys.argv[3:]], cwd=root, env={'HOME':root,'PATH':'/usr/bin:/bin','TERM':'xterm-256color'}, stdout=output)
        Path(root, 'revust.pid').write_text(str(p.pid))
        stdout, _ = p.communicate()
        print(json.dumps({'code':p.returncode,'stdout':stdout.decode() if stdout else ''}), flush=True)
        assert sys.stdin.readline() == 'ack\n'
    else: main()
