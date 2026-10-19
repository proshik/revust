"""Qualify terminal ownership in a disposable PTY, without models or providers."""
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / "target/debug/examples/terminal_probe"

def run(mode):
    master, slave = pty.openpty()
    before = termios.tcgetattr(slave)
    fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",24,80,0,0))
    # Set the controlling terminal after exec, avoiding Python callbacks between
    # fork and exec on macOS. Popen can then return without an unbounded preexec wait.
    process = subprocess.Popen([sys.executable,str(Path(__file__).resolve()),"--child",mode],stdin=slave,stdout=subprocess.PIPE,stderr=slave,start_new_session=True)
    output = bytearray()
    def until(marker):
        deadline=time.monotonic()+10
        while marker not in output:
            assert time.monotonic()<deadline,(mode,bytes(output))
            if select.select([master],[],[],0.1)[0]:
                output.extend(os.read(master,65536))
    try:
        if mode in ["live","live-error"]:
            until(b"OBSERVING")
            until(b"saved activity")
            if mode=="live":
                until(b"COMPLETED")
        elif mode in ["browse","interrupt","countdown"]:until(b"PANE:combined")
        elif mode != "setup-error":until(b"READY")
        if mode == "browse":
            active=termios.tcgetattr(slave)
            assert not active[3] & (termios.ICANON|termios.ECHO|termios.ISIG),active
            os.write(master,b"i")
            until(b"BBBBBBBB")  # unique detail content, not the existing input tab label
            fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",33,91,0,0))
            os.kill(process.pid,signal.SIGWINCH)
            until(b"width91")
            os.write(master,b"q")
        if mode == "interrupt":os.write(master,b"\x03")
        if not mode:
            active=termios.tcgetattr(slave)
            assert not active[3] & (termios.ICANON|termios.ECHO|termios.ISIG),active
            fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",33,91,0,0))
            os.kill(process.pid,signal.SIGWINCH)
            until(b"SIZE 91 33")
            os.write(master,b"q")
        until(b"\x1b[?1049l")
        assert select.select([process.stdout],[],[],5)[0],"missing restoration marker"
        marker=process.stdout.readline()
        assert marker==b"RESTORED\n",marker
        # macOS can briefly report PENDIN while the child enters its canonical
        # acknowledgement read. Require exact restoration within a bounded wait.
        deadline=time.monotonic()+1
        after=termios.tcgetattr(slave)
        while after!=before and time.monotonic()<deadline:
            time.sleep(.01)
            after=termios.tcgetattr(slave)
        assert after==before,(mode,"termios not restored",before,after)
        os.write(master,b"ack\n")
        until(b"ack\r\n")  # do not match "backtrace" in panic diagnostics
        stdout,_=process.communicate(timeout=10)
        assert process.returncode==0,(mode,process.returncode,stdout)
        assert stdout==b"",stdout  # terminal output never leaks to result stdout
        markers=[b"\x1b[?25h"]
        if mode != "setup-error":markers += [b"\x1b[?1049h",b"\x1b[?2004h",b"\x1b[?25l"]
        if mode != "restore-error":markers.append(b"\x1b[?2004l")
        for marker in markers:
            assert marker in output,(mode,marker,bytes(output))
    finally:
        try:
            if process.poll() is None:process.kill();process.wait(timeout=5)
        finally:
            os.close(master);os.close(slave)

if __name__=="__main__":
    if len(sys.argv)>1 and sys.argv[1]=="--child":
        fcntl.ioctl(0,termios.TIOCSCTTY,0)
        os.execv(str(PROBE),[str(PROBE),sys.argv[2]])
    for mode in ["","error","panic","setup-error","restore-error","browse","interrupt","countdown","live","live-error"]:run(mode)
    print("Ten PTY scenarios passed, including live observation, completion timing and producer failure")
