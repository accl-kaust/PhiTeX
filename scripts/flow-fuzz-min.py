"""Delta debugging of a fuzz reproduction over the paragraphs of its text.

    flow-fuzz-min.py BIN REPRO OUT

BIN is phitex-flow. The text before the edit is cut after blank lines;
the paragraph holding the edit stays (its offset follows what goes before
it); a paragraph goes while `BIN --fuzz-classify` gives the same class
(the part before " | ", and whether the DVI or only the log differs).
A timeout's reproduction is cut while its last edit alone still does not
end (`BIN --fuzz-replay FILE --alone` running past HANG seconds, env
FLOW_MIN_HANG, 60) and the cold builds of its text before and after the
edit do end (`BIN --fuzz-cold FILE`, in HANG seconds): a cut that makes
the text itself loop is not the update's hang. When the cold builds
themselves do not end (a loop of ours with no update) and FLOW_MIN_TEX
names TRIP's directory, the reproduction is cut while that loop stays
and the system's `tex` ends on both texts (scripts/flow-fuzz-tex.py): a
cut that makes TeX loop too is not ours.
Run it under scripts/sandbox.
"""
import re
import subprocess
import sys

SEP = '\n\x01'
binary, src, out = sys.argv[1:4]
raw = open(src, encoding='utf-8').read()
parts = raw.split(SEP)
ti = next(i for i, p in enumerate(parts) if p.startswith('text\n'))
text = parts[ti][len('text\n'):]
head = parts[0].split('\n')
ei = next(i for i, l in enumerate(head) if l.startswith('edit '))
_, efile, at, dl, what = head[ei].split(' ', 4)
at = int(at)
in_main = efile == '-'
units = re.split(r'(?<=\n\n)(?=[^\n])', text)
starts, pos = [], 0
for u in units:
    starts.append(pos)
    pos += len(u.encode('utf-8'))
fixed = set()
if in_main:
    for k, s in enumerate(starts):
        if s <= at < s + len(units[k].encode('utf-8')) or (at == pos and k == len(units) - 1):
            fixed.add(k)
tmp = out + '.try'


def write(keep, path):
    ks = sorted(set(keep) | fixed)
    new_at = at
    if in_main:
        new_at = at - sum(len(units[k].encode('utf-8')) for k in range(len(units)) if k not in ks and starts[k] < at)
    h = list(head)
    h[ei] = ' '.join(['edit', efile, str(new_at), dl, what])
    p = list(parts)
    p[0] = '\n'.join(h)
    p[ti] = 'text\n' + ''.join(units[k] for k in ks)
    open(path, 'w', encoding='utf-8').write(SEP.join(p))


def cls(path):
    try:
        r = subprocess.run([binary, '--fuzz-classify', path], capture_output=True, timeout=120)
    except subprocess.TimeoutExpired:
        return None
    line = r.stdout.decode('utf-8', 'replace').split('\t')[0]
    a, _, b = line.partition(' | ')
    return a + ' | ' + b[:4]


def hangs(path):
    try:
        subprocess.run([binary, '--fuzz-cold', path], capture_output=True, timeout=HANG)
    except subprocess.TimeoutExpired:
        return 'the text loops'
    try:
        subprocess.run([binary, '--fuzz-replay', path, '--alone'], capture_output=True, timeout=HANG)
    except subprocess.TimeoutExpired:
        return 'hangs'
    return 'ends'


def cold_loops(path):
    try:
        subprocess.run([binary, '--fuzz-cold', path], capture_output=True, timeout=HANG)
    except subprocess.TimeoutExpired:
        if TEX:
            r = subprocess.run([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), 'flow-fuzz-tex.py'), TEX, path],
                               capture_output=True, text=True)
            if 'before pass1 ends pass2 ends\tafter pass1 ends pass2 ends' not in r.stdout:
                return 'tex loops too'
        return 'cold loops'
    return 'cold ends'


import os
HANG = int(os.environ.get('FLOW_MIN_HANG', '60'))
TEX = os.environ.get('FLOW_MIN_TEX')
want = cls(src)
if want is not None and want.startswith('timeout') and TEX and cold_loops(src) == 'cold loops':
    # (our cold build loops where TeX ends: cut while it does)
    cls = cold_loops
    want = 'cold loops'
elif want is not None and want.startswith('timeout') and hangs(src) == 'hangs':
    # (a hang of the last edit alone: cut while it still hangs)
    cls = hangs
    want = 'hangs'
print('class', want, 'units', len(units), flush=True)
if want is None or want.startswith(('passes', 'history', 'F2?', 'timeout')):
    print('not reducible: the class needs the seed\'s history or never ends')
    sys.exit(0)
cur = [k for k in range(len(units)) if k not in fixed]
n = 2
while len(cur) >= 2:
    size = max(1, len(cur) // n)
    chunks = [cur[j:j + size] for j in range(0, len(cur), size)]
    done = False
    for c in chunks:
        cs = set(c)
        rest = [k for k in cur if k not in cs]
        write(rest, tmp)
        if cls(tmp) == want:
            cur, n, done = rest, max(n - 1, 2), True
            print(len(cur), 'units', flush=True)
            break
    if not done:
        if n >= len(cur):
            break
        n = min(len(cur), n * 2)
write(cur, out)
print('done', len(cur) + len(fixed), 'units ->', out, flush=True)
