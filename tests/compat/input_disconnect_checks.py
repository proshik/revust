"""Bounded PTY input loss, independent of drawing failure and SIGHUP."""
import os
from pathlib import Path
import pty
import resource
import time
import select
import subprocess

ROOT = Path(__file__).resolve().parents[2]
before_cpu = resource.getrusage(resource.RUSAGE_CHILDREN)
started = time.monotonic()
master, slave = pty.openpty()
# No controlling terminal: closing the master cannot rescue the reader by SIGHUP.
child = subprocess.Popen([os.environ.get("REVUST_INPUT_PROBE", str(ROOT / "target/debug/examples/input_disconnect_probe"))],
                         stdin=slave, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                         start_new_session=True)
try:
    assert select.select([child.stdout], [], [], 5)[0], "reader did not start"
    assert child.stdout.readline() == b"READY\n"
    os.close(master)
    master = None
    # Record the actual OS read outcome on another descriptor for this same PTY.
    # EOF and platform-specific revoked-device errors both mean input loss.
    try:
        lost = os.read(slave, 1)
        assert lost == b"", lost
        print("PTY read after close: EOF", flush=True)
    except OSError as error:
        print(f"PTY read after close: errno={error.errno}", flush=True)
    try:
        stdout, stderr = child.communicate(timeout=2)
    except subprocess.TimeoutExpired as error:
        raise AssertionError("PTY input loss did not return within two seconds") from error
    assert child.returncode == 0, (stdout, stderr)
    assert b"DISCONNECTED:" in stdout, stdout
finally:
    os.close(slave)
    if master is not None:
        os.close(master)
    if child.poll() is None:
        child.kill()
        child.communicate(timeout=5)
    after_cpu = resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu = (after_cpu.ru_utime + after_cpu.ru_stime
           - before_cpu.ru_utime - before_cpu.ru_stime)
    print(f"Reader elapsed={time.monotonic() - started:.3f}s CPU={cpu:.3f}s", flush=True)
print("PTY input loss returned within two seconds without drawing or SIGHUP")
