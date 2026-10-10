#!/usr/bin/env python3
# meas.py OUT CMD...: run CMD (stdout+stderr to OUT); print "wall_s maxrss_kb status"
# (wall from the spawn to the reap, python's start not counted; maxrss of the
# child and what it reaped, wait4's rusage; PIN=CPU,CPU...: the child's
# affinity)
import os, sys, time, subprocess
out, cmd = sys.argv[1], sys.argv[2:]
with open(out, 'wb') as f:
    t = time.perf_counter()
    pin = os.environ.get('PIN')
    pre = (lambda: os.sched_setaffinity(0, [int(c) for c in pin.split(',')])) if pin else None
    p = subprocess.Popen(cmd, stdout=f, stderr=subprocess.STDOUT, preexec_fn=pre)
    _, st, ru = os.wait4(p.pid, 0)
    dt = time.perf_counter() - t
code = os.waitstatus_to_exitcode(st)
p.returncode = code
print(f"{dt:.3f} {ru.ru_maxrss} {code}")
