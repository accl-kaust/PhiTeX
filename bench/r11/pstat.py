#!/usr/bin/env python3
"""A minimal `perf stat` (no perf binary needed): user-mode hardware and
software counters for a command and all its threads, through
perf_event_open with inherit. Prints one `pstat:` line on stderr.
Usage: pstat.py CMD [ARGS...]"""
import ctypes, os, struct, sys

libc = ctypes.CDLL(None, use_errno=True)
NR = 298  # perf_event_open, x86_64
EVENTS = [  # (name, type, config)
    ("cycles", 0, 0), ("instructions", 0, 1), ("cache-refs", 0, 2), ("cache-misses", 0, 3),
    ("LLC-load-misses", 3, 2 | (0 << 8) | (1 << 16)), ("LLC-loads", 3, 2 | (0 << 8) | (0 << 16)),
    ("L1d-load-misses", 3, 0 | (0 << 8) | (1 << 16)),
    ("task-clock", 1, 1), ("ctx-switches", 1, 3), ("migrations", 1, 4), ("minor-faults", 1, 5),
]

def attr(typ, config):
    # perf_event_attr VER0 (64 bytes): read_format TOTAL_TIME_ENABLED|RUNNING;
    # flags disabled(0) inherit(1) exclude_kernel(5) exclude_hv(6) enable_on_exec(12)
    flags = (1 << 0) | (1 << 1) | (1 << 5) | (1 << 6) | (1 << 12)
    return struct.pack("IIQQQQQIIQ", typ, 64, config, 0, 0, 3, flags, 0, 0, 0)

r, wfd = os.pipe()
pid = os.fork()
if pid == 0:
    os.close(wfd)
    os.read(r, 1)
    os.execvp(sys.argv[1], sys.argv[1:])
os.close(r)
fds = []
for name, t, c in EVENTS:
    buf = ctypes.create_string_buffer(attr(t, c), 64)
    fd = libc.syscall(NR, buf, pid, -1, -1, 0)
    fds.append((name, fd if fd >= 0 else None, ctypes.get_errno() if fd < 0 else 0))
os.write(wfd, b"x")
os.close(wfd)
_, st = os.waitpid(pid, 0)
out = []
for name, fd, err in fds:
    if fd is None:
        out.append(f"{name}=ERR{err}")
        continue
    v, en, ru = struct.unpack("QQQ", os.read(fd, 24))
    if ru and ru < en:
        v = int(v * en / ru)
    out.append(f"{name}={v}")
print("pstat: " + " ".join(out), file=sys.stderr)
sys.exit(os.waitstatus_to_exitcode(st))
