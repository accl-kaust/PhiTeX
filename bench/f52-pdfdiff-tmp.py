import sys, zlib, re
a, b = (open(p, 'rb').read() for p in sys.argv[1:3])
def objs(x):
    out = {}
    for m in re.finditer(rb'(\d+) 0 obj', x):
        s = x.find(b'stream', m.end()); e = x.find(b'endobj', m.end())
        if 0 <= s < e:
            s += 6
            while x[s:s+1] in b'\r\n': s += 1
            t = x.find(b'endstream', s)
            try: out[int(m.group(1))] = zlib.decompressobj().decompress(x[s:t])
            except Exception: out[int(m.group(1))] = x[s:t]
    return out
oa, ob = objs(a), objs(b)
n = 0
for k in sorted(oa):
    if oa.get(k) != ob.get(k):
        la, lb = oa[k].split(b'\n'), (ob.get(k) or b'').split(b'\n')
        for i, (p, q) in enumerate(zip(la, lb)):
            if p != q:
                print(f"obj {k} line {i}:\n  off {p[:240]!r}\n  box {q[:240]!r}")
                for j in range(max(0, i - 6), i):
                    print(f"  ctx {la[j][:200]!r}")
                break
        n += 1
        if n >= 3: break
