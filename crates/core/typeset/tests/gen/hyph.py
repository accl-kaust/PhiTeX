#!/usr/bin/env python3
"""Generates the hyphenation fixtures from the oracle (Knuth's tex with
plain.fmt, run through scripts/sandbox):

  fixtures/cmr10.txt      cmr10 metrics and its lig/kern table, from the TFM
                          (fix_words scaled as tex.web §571-§575 does)
  fixtures/hyphen.tex     plain TeX's patterns, copied unmodified
  fixtures/hyphenate.txt  per case: \\showbox of the paragraph broken
                          in its first pass (the list before
                          hyphenation) and after a second pass that
                          offers every word to hyphenation

    python3 crates/core/typeset/tests/gen/hyph.py  (from the repo root)
"""
import os, re, shutil, struct, subprocess

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '../../../../..'))
WORK = os.path.join(ROOT, 'target', 'probe-hyph')
FIX = os.path.join(ROOT, 'crates/core/typeset/tests/fixtures')

TEXTS = [
    'difficult official affluent efficient baffling shuffling',
    'Effective OFFICIAL (difficult) ``offline\'\' affirmation',
    'manuscript manuscripts reciprocity university table',
    'hyphenation algorithm characteristically representation',
    'Typewriter AVAILABILITY waffle-iron stuffiness',
    'fluffier sufficiently traffickers ineffectual diffident',
    'office--ffi coffee toffee buffalo affliction suffix',
    'Wolfflin schiffahrt ofttimes ``Afflict\'\' finally',
    'disestablishmentarianism supercalifragilisticexpialidocious',
    'tomorrow\'s VAT-WAVE YYAvA AVoid overwhelmingly',
]

def tfm(path):
    d = open(path, 'rb').read()
    lf, lh, bc, ec, nw, nh, nd, ni, nl, nk, ne, np = struct.unpack('>12H', d[:24])
    w = lambda i: d[4 * i: 4 * i + 4]
    base = 6 + lh
    ci = {c: w(base + c - bc) for c in range(bc, ec + 1)}
    wb = base + ec - bc + 1
    hb, db = wb + nw, wb + nw + nh
    ib, lb = db + nd, db + nd + ni
    kb = lb + nl
    design = struct.unpack('>i', d[24 + 4:24 + 8])[0]
    # tex.web §568, §572: at its design size, z is the design size in sp
    z = design // 16
    alpha = 16
    while z >= 0o40000000:
        z //= 2
        alpha += alpha
    beta = 256 // alpha
    alpha *= z
    def sw(b):
        a, b1, c, dd = b
        v = (((dd * z) // 256 + c * z) // 256 + b1 * z) // beta
        if a == 0:
            return v
        assert a == 255
        return v - alpha
    lk = [w(lb + i) for i in range(nl)]
    out = []
    chars = {}
    for c, info in ci.items():
        if info[0] == 0:
            continue
        wd = sw(w(wb + info[0]))
        ht = sw(w(hb + (info[1] >> 4)))
        dp = sw(w(db + (info[1] & 15)))
        it = sw(w(ib + (info[2] >> 2)))
        chars[c] = (info[2] & 3, info[3])
        out.append('char %d %d %d %d %d' % (c, wd, ht, dp, it))
    def program(i, rights):
        res = {}
        for r in rights:
            k = i
            while True:
                s, n, op, rem = lk[k]
                if n == r and s <= 128:
                    if op >= 128:
                        res[r] = 'kern %d' % sw(w(kb + 256 * (op - 128) + rem))
                    else:
                        res[r] = 'lig %d %d' % (op, rem)
                    break
                if s >= 128:
                    break
                k += s + 1
        return res
    for c, (tag, rem) in chars.items():
        if tag != 1:
            continue
        i = rem
        if lk[i][0] > 128:
            i = 256 * lk[i][2] + lk[i][3]
        for r, v in program(i, range(256)).items():
            out.append('lk %d %d %s' % (c, r, v))
    if nl and lk[-1][0] == 255:
        for r, v in program(256 * lk[-1][2] + lk[-1][3], range(256)).items():
            out.append('lk -1 %d %s' % (r, v))
    if nl and lk[0][0] == 255:
        out.append('bchar %d' % lk[0][1])
    return out

def main():
    os.makedirs(WORK, exist_ok=True)
    kp = lambda f: subprocess.run(['kpsewhich', f], capture_output=True, text=True).stdout.strip()
    with open(os.path.join(FIX, 'cmr10.txt'), 'w') as f:
        f.write('\n'.join(tfm(kp('cmr10.tfm'))) + '\n')
    shutil.copyfile(kp('hyphen.tex'), os.path.join(FIX, 'hyphen.tex'))
    src = ['\\tracingonline1 \\showboxdepth=100 \\showboxbreadth=100000 \\hbadness=10000',
           '\\catcode`\\^^M=9 \\hsize=16000pt \\pretolerance=-1 \\tolerance=10000 \\parindent=0pt']
    for t in TEXTS:
        src.append('\\setbox1\\vbox{\\pretolerance=10000 \\noindent %s\\par}\\showbox1' % t)
        src.append('\\setbox0\\vbox{\\noindent %s\\par}\\showbox0' % t)
    src.append('\\end')
    open(os.path.join(WORK, 'hy.tex'), 'w').write('\n'.join(src) + '\n')
    subprocess.run([os.path.join(ROOT, 'scripts/sandbox'), 'sh', '-c',
                    'cd target/probe-hyph && tex -interaction=batchmode hy.tex'],
                   cwd=ROOT, stdout=subprocess.DEVNULL, check=False)
    log = open(os.path.join(WORK, 'hy.log'), encoding='latin-1').read()
    boxes = re.findall(r'> \\box[01]=\n(.*?)\n\n! OK', log, re.S)
    assert len(boxes) == 2 * len(TEXTS), len(boxes)
    with open(os.path.join(FIX, 'hyphenate.txt'), 'w') as f:
        for i, t in enumerate(TEXTS):
            f.write('case %d\ntext=%s\nexpect\n%s\n%s\nend\n' % (i, t, boxes[2 * i], boxes[2 * i + 1]))

main()
