#!/usr/bin/env python3
"""CPU sets from this process's affinity: cpus.py ccx1|ccx2|smt4|one
ccx1: 8 cores of one L3, one thread each; ccx2: 4 cores of each of two L3s;
smt4: 4 cores of one L3, both threads; one: a single CPU. Prints a taskset list."""
import os, sys
def rd(c, f):
    return open(f"/sys/devices/system/cpu/cpu{c}/{f}").read().strip()
cpus = sorted(os.sched_getaffinity(0))
by = {}
for c in cpus:
    l3 = rd(c, "cache/index3/id"); core = rd(c, "topology/core_id")
    by.setdefault(l3, {}).setdefault(core, []).append(c)
l3s = sorted(by.values(), key=lambda d: -len(d))
first = lambda d, n: [v[0] for v in list(d.values())[:n]]
m = sys.argv[1]
if m == "ccx1": out = first(l3s[0], 8)
elif m == "ccx2": out = first(l3s[0], 4) + first(l3s[1], 4)
elif m == "smt4": out = [c for v in list(l3s[0].values())[:4] for c in v]
else: out = first(l3s[0], 1)
print(",".join(map(str, out)))
