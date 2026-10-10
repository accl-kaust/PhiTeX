#!/usr/bin/env python3
# cpus.py MODE N: N CPUs of this process's allowed set, comma-separated:
# MODE percore = one hardware thread on each of N distinct cores; ht = N
# hardware threads packed two to a core (siblings together).
import os, sys, collections
mode, n = sys.argv[1], int(sys.argv[2])
allowed = sorted(os.sched_getaffinity(0))
core = {}
for c in allowed:
    try:
        sib = open(f'/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list').read().strip()
    except OSError:
        sib = str(c)
    core[c] = sib
bycore = collections.OrderedDict()
for c in allowed:
    bycore.setdefault(core[c], []).append(c)
if mode == 'percore':
    out = [v[0] for v in bycore.values()][:n]
else:
    out = [c for v in bycore.values() if len(v) > 1 for c in v][:n]
if len(out) < n:
    sys.exit(f'only {len(out)} cpus for {mode} {n} (allowed {allowed}, cores {list(bycore.values())})')
print(','.join(map(str, out)))
