import sys, collections
L = open(sys.argv[1], errors="replace").read().split("\n")
# the edit round: after the last "incr walk"
i = max(k for k, l in enumerate(L) if l.startswith("incr walk"))
E = L[i:]
cnt = collections.Counter(); tm = collections.Counter()
for l in E:
    if l.startswith("nodetime: "):
        _, op, n, t = l.split()
        cnt[op] += 1; tm[op] += float(t)
print("op: runs, busy ms")
for op in sorted(cnt, key=lambda o: -tm[o]):
    print(f"  {op}: {cnt[op]} runs, {tm[op]:.1f} ms")
why = collections.Counter()
for l in E:
    if l.startswith("rerunop: "):
        op = l.split("[", 1)[1].split()[0].rstrip("]")
        cause = l.split("] ", 1)[1].split()[0]
        fld = l.split()[-1] if not l.rstrip().endswith("}") else ""
        why[(op, cause, fld)] += 1
print("rerun causes (op, cause, field): count")
for k, c in sorted(why.items(), key=lambda x: -x[1]):
    print("  ", k, c)
for l in E:
    if l.startswith("round: ") or l.startswith("round rerun") or l.startswith("output file") or l.startswith("pdf pages"):
        print(l[:300])
