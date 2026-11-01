"""Compares gr2dump's and gr-dump's outputs task by task:

    python3 -I compare.py TASKS ORACLE OURS [EXAMPLES]

Prints the count identical and, for the first differences, the font, the
text and the first differing slot field."""
import struct
import sys


def segs(path):
    out, cur = [], None
    with open(path) as f:
        for l in f:
            if l.startswith('T'):
                cur = [l.split(' ', 1)[1].strip()]
                out.append(cur)
            elif cur is not None:
                cur.append(l.strip())
    return out


def fl(h):
    return struct.unpack('>f', bytes.fromhex(h))[0]


def main():
    tasks = open(sys.argv[1], encoding='utf-8').read().splitlines()
    a, b = segs(sys.argv[2]), segs(sys.argv[3])
    mx = int(sys.argv[4]) if len(sys.argv) > 4 else 30
    same = diff = 0
    ex = []
    byfont = {}
    for i in range(max(len(a), len(b))):
        x = a[i] if i < len(a) else ['(missing)']
        y = b[i] if i < len(b) else ['(missing)']
        if x == y:
            same += 1
            continue
        diff += 1
        t = tasks[i].split('\t') if i < len(tasks) else ['?'] * 5
        byfont[t[0].split('/')[-1]] = byfont.get(t[0].split('/')[-1], 0) + 1
        if len(ex) < mx:
            k = next((j for j in range(min(len(x), len(y))) if x[j] != y[j]), min(len(x), len(y)))
            def show(s):
                if s is None:
                    return '-'
                f = s.split()
                return ' '.join(('%g' % fl(v)) if len(v) == 8 and all(c in '0123456789abcdef' for c in v) else v for v in f)
            ex.append('task %d %s lang=%s feats=%s dir=%s "%s": line %d: oracle [%s] ours [%s]' % (
                i + 1, t[0].split('/')[-1], t[1], t[2], t[3], t[4][:80], k,
                show(x[k] if k < len(x) else None), show(y[k] if k < len(y) else None)))
    print('segments: %d, identical %d, differ %d' % (same + diff, same, diff))
    for k, v in sorted(byfont.items(), key=lambda kv: -kv[1]):
        print('  %d differ in %s' % (v, k))
    for e in ex:
        print(e)


main()
