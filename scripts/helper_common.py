"""Read-only CLI selection and protocol parsing shared by shipped helpers."""
import json
import os
from pathlib import Path
import shutil
import subprocess


PROXY_ENVIRONMENT = ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY',
                     'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')
FORWARDED = ('PATH', 'HOME', 'XDG_CONFIG_HOME', 'CLAUDE_CONFIG_DIR', 'CODEX_HOME', 'TMPDIR') + PROXY_ENVIRONMENT


class HelperError(Exception):
    pass


def executable():
    selected = os.environ.get('REVUST_BIN', 'revust')
    path = shutil.which(selected)
    if not path:
        raise HelperError('revust executable is unavailable; select REVUST_BIN or adjust PATH')
    return str(Path(path).resolve())


def configuration(*arguments):
    try:
        result = subprocess.run([executable(), 'config', *arguments], capture_output=True, timeout=30)
    except (OSError, subprocess.SubprocessError) as error:
        raise HelperError('selected CLI configuration could not be inspected') from error
    if result.returncode:
        # Effective config can contain child environment entries. Do not relay it
        # or a potentially value-bearing validation diagnostic through this helper.
        raise HelperError('config: FAILED; inspect the selected CLI configuration directly')
    try:
        value = json.loads(result.stdout)
        if not isinstance(value, dict):
            raise ValueError()
        return value
    except ValueError as error:
        raise HelperError('configuration response is not an object') from error


def mask_proxy_values(value):
    text = str(value)
    for secret in sorted({os.environ[key] for key in PROXY_ENVIRONMENT if os.environ.get(key)}, key=len, reverse=True):
        text = text.replace(secret, '[proxy value]')
    return text


def safe(value):
    text = mask_proxy_values(value)
    return ''.join(ch if ch.isprintable() else '?' for ch in text)


def open_readonly(path, directory=False):
    """No-follow descriptor walk, allowing only macOS's system-root aliases."""
    import stat
    path = Path(os.path.abspath(path))
    current = os.open('/', os.O_RDONLY | os.O_DIRECTORY)
    prefix = Path('/')
    try:
        for number, name in enumerate(path.parts[1:]):
            prefix /= name
            last = number == len(path.parts) - 2
            flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
            if not last or directory:
                flags |= os.O_DIRECTORY
            if sys_platform_macos() and prefix in (Path('/var'), Path('/tmp')):
                flags &= ~os.O_NOFOLLOW
            following = os.open(name, flags, dir_fd=current)
            os.close(current)
            current = following
        if not directory and not stat.S_ISREG(os.fstat(current).st_mode):
            raise OSError('not a regular file')
        result = current
        current = None
        return result
    finally:
        if current is not None:
            os.close(current)


def sys_platform_macos():
    import sys
    return sys.platform == 'darwin'
