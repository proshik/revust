#!/usr/bin/env python3
"""One host selection, one execution transaction, durable result observation."""
import colorsys
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from helper_common import HelperError, executable, safe

HERE = Path(__file__).resolve().parent
FORWARDED = ('PATH', 'HOME', 'XDG_CONFIG_HOME', 'CLAUDE_CONFIG_DIR', 'CODEX_HOME', 'TMPDIR')


class LaunchError(Exception):
    pass


def call(args, script=None):
    return subprocess.run(args, input=script, text=True, capture_output=True)


def query(args):
    result = call(args)
    if result.returncode:
        raise LaunchError('host inspection failed: ' + args[0])
    return result.stdout.strip()


class Transaction:
    def __init__(self, program, arguments):
        self.root = Path(tempfile.mkdtemp(prefix='revust-launch-'))
        self.nonce = str(uuid.uuid4())
        self.sent = False
        self.finished = False
        self.on_complete = []
        spec = {'nonce': self.nonce, 'program': program, 'args': arguments,
                'cwd': os.getcwd(), 'env': {key: os.environ[key] for key in FORWARDED if key in os.environ}}
        self.write('request.json', json.dumps(spec))
        self.write('report', '')
        self.write('stderr', '')
        # Hosts accepting only shell text receive this fixed quoted path. User
        # arguments and environment values travel in the private JSON descriptor.
        self.write('run', '#!/bin/sh\nexec ' + shlex.join([sys.executable, str(HERE / 'launch-worker.py'), str(self.root)]) + '\n', 0o700)

    def write(self, name, content, mode=0o600):
        descriptor = os.open(self.root / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
        with os.fdopen(descriptor, 'w') as output:
            output.write(content)

    @property
    def shell(self):
        return shlex.join([str(self.root / 'run')])

    def record(self, name):
        try:
            value = json.loads((self.root / name).read_text())
        except (OSError, ValueError):
            return None
        if isinstance(value, dict) and value.get('nonce') == self.nonce:
            return value
        return None

    def dispatch(self, argv, script=None):
        self.sent = True
        try:
            result = call(argv, script)
        except OSError:
            # A local exec failure precedes host delivery and is unambiguous.
            self.sent = False
            raise
        # Host acknowledgements and diagnostics remain distinct from review stdout.
        # Only a bounded inert diagnostic is echoed; raw evidence stays private.
        self.write('host-reply.json', json.dumps({'status': result.returncode,
                   'stdout': result.stdout[:65536], 'stderr': result.stderr[:65536],
                   'stdout_truncated': len(result.stdout) > 65536,
                   'stderr_truncated': len(result.stderr) > 65536}))
        if result.returncode and not self.record('started.json') and not self.record('result.json'):
            if result.stderr:
                print('host diagnostic: ' + safe(result.stderr[:65536]), file=sys.stderr)
            raise LaunchError('host launch was not acknowledged; no repeat dispatch')
        return result.stdout.strip() if result.returncode == 0 else ''

    def wait(self, alive=None):
        startup = time.monotonic() + 10
        while not self.record('result.json'):
            started = self.record('started.json')
            if started:
                pid = started.get('pid')
                if not isinstance(pid, int) or pid < 1:
                    raise LaunchError('invalid owned worker observation')
                try:
                    os.kill(pid, 0)
                except ProcessLookupError:
                    if self.record('result.json'):
                        break
                    raise LaunchError('owned worker ended without a result')
            elif time.monotonic() >= startup or (alive is not None and not alive()):
                raise LaunchError('overlay ended or never published its owned process observation')
            time.sleep(.05)
        return self.result()

    def result(self):
        record = self.record('result.json')
        code = record.get('exit_code') if record else None
        if type(code) is not int or not 0 <= code <= 255:
            raise LaunchError('invalid owned result')
        self.finished = True
        data = (self.root / 'report').read_bytes()
        diagnostics = (self.root / 'stderr').read_bytes()
        sys.stderr.buffer.write(diagnostics)
        if not data and code in (0, 1):
            raise LaunchError('no report produced; completion is unavailable')
        sys.stdout.buffer.write(data)
        return code

    def close(self):
        if self.finished:
            for cleanup in self.on_complete:
                try:
                    cleanup()
                except (OSError, LaunchError):
                    print('warning: completed host presentation remains available', file=sys.stderr)
        if self.finished or not self.sent:
            shutil.rmtree(self.root)
        else:
            print('artifacts retained: ' + json.dumps(str(self.root)), file=sys.stderr)


def title(arguments):
    task = ''
    for index, argument in enumerate(arguments):
        if argument.startswith('--task='):
            task = argument.split('=', 1)[1]
            break
        if argument == '--task' and index + 1 < len(arguments):
            task = arguments[index + 1]
            break
    return 'revust: ' + Path.cwd().name + (f' [{task}]' if task else '')


def agterm_client():
    bundle = Path(os.environ.get('GHOSTTY_BIN_DIR', '/nonexistent')) / 'agtermctl'
    return str(bundle) if bundle.is_file() and os.access(bundle, os.X_OK) else shutil.which('agtermctl')


def agterm_args(client, *args):
    return [client, *args] + (['--socket', os.environ['AGTERM_SOCKET']] if os.environ.get('AGTERM_SOCKET') else [])


def agterm_context(client):
    session = os.environ.get('AGTERM_SESSION_ID')
    pane = os.environ.get('AGTERM_PANE', '')
    tree = {}
    try:
        args = ['tree', '--json']
        if os.environ.get('AGTERM_WINDOW_ID'):
            args += ['--window', os.environ['AGTERM_WINDOW_ID']]
        tree = json.loads(query(agterm_args(client, *args))).get('result', {}).get('tree', {})
        sessions = [item for workspace in tree.get('workspaces', []) for item in workspace.get('sessions', [])]
        if not session:
            common = query(['git', '-C', os.getcwd(), 'rev-parse', '--path-format=absolute', '--git-common-dir'])
            matches = []
            for item in sessions:
                cwd = item.get('cwd') or item.get('workingDirectory')
                if cwd:
                    try:
                        if query(['git', '-C', cwd, 'rev-parse', '--path-format=absolute', '--git-common-dir']) == common:
                            matches.append(item)
                    except (OSError, LaunchError):
                        pass
            if len(matches) == 1:
                session = matches[0]['id']
                pane = matches[0].get('pane', '')
        selected = next((item for item in sessions if item.get('id') == session), {})
    except (OSError, ValueError, LaunchError, KeyError, TypeError):
        selected = {}
    return session, pane, selected


def agterm(transaction, client, context):
    session, pane, observation = context
    background = observation.get('background') or {}
    color = background.get('colorHex', '')
    tint = '#252525'
    if isinstance(color, str) and re.fullmatch(r'#[0-9a-fA-F]{6}', color):
        # Keep the caller's hue while giving the owned overlay a small lightness
        # distinction. This uses the standard library's HLS representation.
        rgb = [int(color[index:index+2], 16) / 255 for index in (1, 3, 5)]
        hue, lightness, saturation = colorsys.rgb_to_hls(*rgb)
        lightness += .05 if lightness < .5 else -.05
        shaded = colorsys.hls_to_rgb(hue, lightness, saturation)
        tint = '#{:02x}{:02x}{:02x}'.format(*(round(value * 255) for value in shaded))
    geometry = ['--size-percent', os.environ.get('REVUST_AGTERM_PERCENT', '80')]
    if pane in ('left', 'right') and observation.get('split') and 'REVUST_AGTERM_PERCENT' not in os.environ:
        help_text = query(agterm_args(client, 'session', 'overlay', 'open', '--help'))
        if '--pane' in help_text:
            geometry = ['--pane', pane]
    command = agterm_args(client, 'session', 'overlay', 'open', transaction.shell, '--target', session,
                          '--cwd', os.getcwd(), *geometry, '--background-color', tint, '--block')
    transaction.dispatch(command)
    return transaction.wait()


def tmux(transaction, heading):
    setting = os.environ.get('REVUST_TMUX_WINDOW')
    automatic = False
    if setting is None:
        automatic = bool(os.environ.get('AGENTDECK_INSTANCE_ID')) or call(['tmux', 'show-environment', 'AGENTDECK_INSTANCE_ID']).returncode == 0
        if not automatic:
            automatic = query(['tmux', 'display-message', '-p', '#{session_name}']).startswith('agentdeck_')
    if setting == '1' or automatic:
        previous = query(['tmux', 'display-message', '-p', '#{window_id}']) if setting == '1' else ''
        owned = transaction.dispatch(['tmux', 'new-window', '-d', '-P', '-F', '#{window_id}', '-c', os.getcwd(), '-n', heading, '--', 'sh', '-c', transaction.shell])
        if not re.fullmatch(r'@[A-Za-z0-9_-]+', owned):
            if not transaction.record('result.json'):
                raise LaunchError('tmux window acknowledgement has no owned ID')
            owned = ''
        if owned and previous:
            call(['tmux', 'select-window', '-t', owned])
        try:
            return transaction.wait(lambda: owned in query(['tmux', 'list-windows', '-F', '#{window_id}']).splitlines())
        finally:
            if previous and transaction.finished:
                call(['tmux', 'select-window', '-t', previous])
    args = ['tmux', 'display-popup', '-E', '-w', os.environ.get('REVUST_POPUP_WIDTH', '90%'), '-h', os.environ.get('REVUST_POPUP_HEIGHT', '90%')]
    version = re.search(r'(\d+)\.(\d+)', query(['tmux', '-V']))
    if version and tuple(map(int, version.groups())) >= (3, 3):
        args += ['-T', heading]
    transaction.dispatch([*args, '-d', os.getcwd(), '--', 'sh', '-c', transaction.shell])
    return transaction.wait()


def zellij(transaction, heading):
    args = ['zellij', 'run', '--floating', '--close-on-exit']
    if os.environ.get('ZELLIJ_PANE_ID'):
        try:
            panes = json.loads(query(['zellij', 'action', 'list-panes', '--json', '--tab']))
            selected = next(item for item in panes if str(item.get('id')) == os.environ['ZELLIJ_PANE_ID'] and not item.get('is_plugin'))
            if '--tab-id' in query(['zellij', 'run', '--help']):
                args += ['--tab-id', str(selected['tab_id'])]
        except (LaunchError, ValueError, StopIteration, KeyError, TypeError):
            pass
    transaction.dispatch([*args, '--width', os.environ.get('REVUST_POPUP_WIDTH', '90%'), '--height', os.environ.get('REVUST_POPUP_HEIGHT', '90%'), '--name', heading, '--cwd', os.getcwd(), '--', str(transaction.root / 'run')])
    return transaction.wait()


def herdr(transaction, heading):
    args = ['herdr', 'tab', 'create', '--cwd', os.getcwd(), '--label', heading]
    if os.environ.get('HERDR_WORKSPACE_ID'):
        args += ['--workspace', os.environ['HERDR_WORKSPACE_ID']]
    result = call([*args, '--focus'])
    if result.returncode:
        raise LaunchError('herdr tab creation failed')
    try:
        created = json.loads(result.stdout)['result']
        tab = created['tab']['tab_id']
        pane = created['root_pane']['pane_id']
        if not all(isinstance(value, str) and value for value in (tab, pane)):
            raise ValueError()
    except (ValueError, KeyError, TypeError):
        # A tab ID alone is sufficient to clean an empty tab, never to send input.
        try:
            tab = json.loads(result.stdout)['result']['tab']['tab_id']
            if isinstance(tab, str) and tab:
                call(['herdr', 'tab', 'close', tab])
        except (ValueError, KeyError, TypeError):
            pass
        raise LaunchError('herdr did not identify the owned tab and pane')
    transaction.dispatch(['herdr', 'pane', 'run', pane, 'sh ' + transaction.shell])
    transaction.on_complete.append(lambda: call(['herdr', 'tab', 'close', tab]))
    return transaction.wait()


def apple(transaction, kind):
    runner = str(transaction.root / 'run')
    if kind == 'ghostty':
        arguments = [runner, os.getcwd()]
        source = '''on run values
  tell application "Ghostty"
    set options to new surface configuration
    set command of options to item 1 of values
    set initial working directory of options to item 2 of values
    set wait after command of options to false
    set origin to focused terminal of selected tab of front window
    set child to split origin direction down with configuration options
    perform action "toggle_split_zoom" on child
    return id of child
  end tell
end run'''
        cleanup = '''on run values
  tell application "Ghostty" to close terminal id (item 1 of values)
end run'''
    elif kind == 'iterm':
        arguments = [os.environ['ITERM_SESSION_ID'].rsplit(':', 1)[-1], runner]
        lookup = '''on sessionMatching(wantedID)
  tell application id "com.googlecode.iterm2"
    set matches to {}
    repeat with container in every window
      repeat with page in every tab of container
        set matches to matches & (every session of page whose id is wantedID)
      end repeat
    end repeat
    if (count matches) is 1 then return item 1 of matches
  end tell
  return missing value
end sessionMatching
'''
        source = lookup + '''on run values
  set owner to my sessionMatching(item 1 of values)
  if owner is missing value then error "Caller session is unavailable or ambiguous"
  set commandText to quoted form of (item 2 of values)
  tell application id "com.googlecode.iterm2"
    set wideLayout to (columns of owner > 159) and (columns of owner > 2 * (rows of owner))
    tell owner
      if wideLayout then
        set child to split vertically with same profile command commandText
      else
        set child to split horizontally with same profile command commandText
      end if
    end tell
    return id of child
  end tell
end run'''
        cleanup = lookup + '''on run values
  set owner to my sessionMatching(item 1 of values)
  if owner is not missing value then
    tell application id "com.googlecode.iterm2" to tell owner to close
  end if
end run'''
    else:
        arguments = [runner]
        source = '''on ownerOfTTY(terminalIdentity)
  tell application id "com.apple.Terminal"
    set matches to {}
    repeat with candidate in every window
      try
        if exists (first tab of candidate whose tty is terminalIdentity) then
          set end of matches to id of candidate
        end if
      end try
    end repeat
    if (count matches) is 1 then return item 1 of matches
  end tell
  error "Owned window is unavailable or ambiguous"
end ownerOfTTY
on run values
  tell application id "com.apple.Terminal"
    set child to do script (quoted form of (item 1 of values))
    set terminalIdentity to tty of child
  end tell
  return my ownerOfTTY(terminalIdentity)
end run'''
        cleanup = '''on run values
  set wanted to (item 1 of values) as integer
  tell application id "com.apple.Terminal"
    set matches to (every window whose id is wanted)
    if (count matches) is not 1 then return
    set owner to item 1 of matches
    set remaining to 20
    repeat while remaining > 0
      if (count tabs of owner) is not 1 then return
      if busy of tab 1 of owner then
        set remaining to remaining - 1
        delay 0.1
      else
        close owner saving no
        return
      end if
    end repeat
  end tell
end run'''
    owned = transaction.dispatch(['osascript', '-', *arguments], source)
    if owned and '\n' not in owned:
        transaction.on_complete.append(lambda: call(['osascript', '-', owned], cleanup))
    return transaction.wait()


def emacs(transaction, heading):
    try:
        server = int(query(['emacsclient', '--eval', '(emacs-pid)']))
        if server <= 0:
            raise ValueError()
    except (ValueError, LaunchError):
        raise LaunchError('emacs server not reachable')
    caller = os.getpid()
    for _ in range(64):
        parent = query(['ps', '-o', 'ppid=', '-p', str(caller)])
        if parent in (str(server), '1', ''):
            break
        if not parent.isdecimal():
            raise LaunchError('invalid caller process ancestry')
        caller = int(parent)
    name = json.dumps(heading, ensure_ascii=False)
    runner = json.dumps(str(transaction.root / 'run'), ensure_ascii=False)
    tag = json.dumps(transaction.nonce)
    # Frame ownership is a nonce, never a title shared with another launch.
    frame = f'''(progn (require 'cl-lib)
      (let* ((origin (cl-find-if (lambda (b) (let ((p (get-buffer-process b)))
                        (and p (equal (process-id p) {caller})))) (buffer-list)))
             (old-window (and origin (get-buffer-window origin t)))
             (buffer (generate-new-buffer "*revust*"))
             (window (display-buffer buffer '((display-buffer-pop-up-frame)
                       (pop-up-frame-parameters . ((name . {name})))))))
        (set-frame-parameter (window-frame window) 'revust-owner {tag})
        (set-frame-parameter (window-frame window) 'revust-buffer buffer)
        (set-frame-parameter (window-frame window) 'revust-origin old-window)))'''
    if call(['emacsclient', '--eval', frame]).returncode:
        raise LaunchError('could not create the owned review frame')
    find = f"(cl-find-if (lambda (f) (equal (frame-parameter f 'revust-owner) {tag})) (frame-list))"
    start = f'''(progn (require 'cl-lib) (let ((frame {find}))
        (when frame (with-current-buffer (frame-parameter frame 'revust-buffer)
          (let ((vterm-shell {runner})) (vterm-mode))))))'''
    transaction.dispatch(['emacsclient', '--no-wait', '--eval', start])
    cleanup = f'''(progn (require 'cl-lib) (let ((frame {find}))
      (when frame (let ((buffer (frame-parameter frame 'revust-buffer))
                        (origin (frame-parameter frame 'revust-origin)))
        (delete-frame frame) (when (buffer-live-p buffer) (kill-buffer buffer))
        (when (window-live-p origin) (select-window origin))))))'''
    transaction.on_complete.append(lambda: call(['emacsclient', '--no-wait', '--eval', cleanup]))
    return transaction.wait()


def select_host(transaction, arguments):
    heading = title(arguments)
    client = agterm_client()
    context = agterm_context(client) if client else (None, '', {})
    if context[0]:
        return agterm(transaction, client, context)
    if os.environ.get('TMUX') and shutil.which('tmux'):
        return tmux(transaction, heading)
    if os.environ.get('ZELLIJ') and shutil.which('zellij'):
        return zellij(transaction, heading)
    if os.environ.get('HERDR_ENV') == '1' and shutil.which('herdr'):
        return herdr(transaction, heading)
    if os.environ.get('KITTY_LISTEN_ON') and shutil.which('kitty'):
        args = ['kitty', '@', '--to', os.environ['KITTY_LISTEN_ON'], 'launch', '--type=overlay', '--title=' + heading, '--cwd=current']
        if os.environ.get('KITTY_WINDOW_ID'):
            args += ['--match', 'window_id:' + os.environ['KITTY_WINDOW_ID']]
        transaction.dispatch([*args, 'sh', '-c', transaction.shell])
        return transaction.wait()
    if os.environ.get('WEZTERM_PANE'):
        client = shutil.which('wezterm') or shutil.which('kaku')
        if client:
            transaction.dispatch([client, 'cli', 'split-pane', '--bottom', '--percent', os.environ.get('REVUST_POPUP_HEIGHT', '90%').rstrip('%'), '--pane-id', os.environ['WEZTERM_PANE'], '--cwd', os.getcwd(), '--', 'sh', '-c', transaction.shell])
            return transaction.wait()
    cmux = os.environ.get('CMUX_SURFACE_ID') or os.environ.get('__CFBundleIdentifier') == 'com.cmuxterm.app' or any('cmux.app' in os.environ.get(key, '') for key in ('GHOSTTY_RESOURCES_DIR', 'GHOSTTY_BIN_DIR'))
    if cmux:
        if not shutil.which('cmux'):
            raise LaunchError('cmux session detected but cmux CLI not found')
        created = call(['cmux', 'new-split', 'down'])
        surface = re.search(r'\bsurface:[0-9]+\b', created.stdout) if created.returncode == 0 else None
        if not surface:
            raise LaunchError('cmux did not identify the created surface')
        transaction.dispatch(['cmux', 'send', '--surface', surface.group(), 'exec ' + transaction.shell + '\\n'])
        return transaction.wait()
    if os.environ.get('TERM_PROGRAM') == 'ghostty' and shutil.which('osascript'):
        return apple(transaction, 'ghostty')
    if os.environ.get('ITERM_SESSION_ID') and shutil.which('osascript'):
        return apple(transaction, 'iterm')
    if os.environ.get('TERM_PROGRAM') == 'Apple_Terminal' and shutil.which('osascript'):
        return apple(transaction, 'terminal')
    if os.environ.get('INSIDE_EMACS') == 'vterm' and shutil.which('emacsclient'):
        return emacs(transaction, heading)
    raise LaunchError('no overlay terminal available; select headless mode explicitly')


def main(arguments):
    try:
        program = executable()
    except (HelperError, OSError, RuntimeError) as error:
        print('error: ' + safe(error), file=sys.stderr)
        return 127
    transaction = None
    try:
        if not arguments or '--no-tui' in arguments:
            raise LaunchError('overlay requires review arguments without --no-tui')
        host = os.environ.get('REVUST_HOST', 'auto')
        if host not in ('auto', 'guild'):
            raise LaunchError('REVUST_HOST must be auto or guild')
        if not any(arg == '--auto-exit' or arg.startswith('--auto-exit=') for arg in arguments):
            delay = os.environ.get('REVUST_AUTO_EXIT', '30s')
            if delay != '0':
                arguments = [*arguments, '--auto-exit=' + delay]
        if host == 'guild' or os.environ.get('GUILD_SESSION') or os.environ.get('GUILD_SOCKET'):
            os.execv(sys.executable, [sys.executable, str(HERE / 'guild-launch.py'), program, *arguments])
        transaction = Transaction(program, arguments)
        return select_host(transaction, arguments)
    except (LaunchError, OSError, ValueError, TypeError, AttributeError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        print('error: ' + safe(error), file=sys.stderr)
        return 3
    except KeyboardInterrupt:
        print('launcher observation interrupted; inspect retained evidence before another start', file=sys.stderr)
        return 130
    finally:
        if transaction:
            transaction.close()


if __name__ == '__main__':
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    sys.exit(main(sys.argv[1:]))
