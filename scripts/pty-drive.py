#!/usr/bin/env python3
"""Run a command under a pseudo-terminal of a known size, optionally
typing at it, and print what came back.

    pty-drive.py <rows> <cols> [--send <text>] [--settle <seconds>] -- cmd...

Why a helper rather than a line in the test: the properties worth
asserting about `-t` cannot be observed from a shell pipeline, because a
pipeline is exactly what `-t` is distinguishable from. To check that the
window size reaches the container, something has to *have* a window; to
check that `^C` becomes SIGINT rather than the byte 0x03, something has
to be a controlling terminal.

`--send` text is written to the master after `--settle` seconds, so a
shell has had time to print its prompt. `\\x03` in it is sent literally,
which is how the SIGINT case is driven.
"""

import os
import pty
import select
import struct
import sys
import termios
import fcntl
import time

argv = sys.argv[1:]
rows, cols = int(argv[0]), int(argv[1])
argv = argv[2:]
send = b""
settle = 0.4
while argv and argv[0] != "--":
    if argv[0] == "--send":
        send = argv[1].encode().decode("unicode_escape").encode()
        argv = argv[2:]
    elif argv[0] == "--settle":
        settle = float(argv[1])
        argv = argv[2:]
    else:
        sys.exit(f"unknown option {argv[0]}")
if not argv:
    sys.exit("no command")
cmd = argv[1:]

master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

pid = os.fork()
if pid == 0:
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    os.dup2(slave, 0)
    os.dup2(slave, 1)
    os.dup2(slave, 2)
    os.close(master)
    if slave > 2:
        os.close(slave)
    os.execvp(cmd[0], cmd)
    os._exit(127)

os.close(slave)
out = bytearray()
sent = False
started = time.time()
deadline = started + 25
while time.time() < deadline:
    if send and not sent and time.time() - started >= settle:
        os.write(master, send)
        sent = True
    r, _, _ = select.select([master], [], [], 0.2)
    if r:
        try:
            chunk = os.read(master, 65536)
        except OSError:
            # EIO: the last slave closed, which is how a pty session ends.
            break
        if not chunk:
            break
        out += chunk
    else:
        # Reap without blocking, so a command that exits silently does not
        # hold us to the deadline.
        done, _ = os.waitpid(pid, os.WNOHANG)
        if done and not send:
            break

try:
    _, status = os.waitpid(pid, 0)
    code = os.waitstatus_to_exitcode(status)
except ChildProcessError:
    code = 0
sys.stdout.write(out.decode("utf-8", "replace"))
sys.stdout.flush()
sys.exit(0 if code is None else max(0, code))
