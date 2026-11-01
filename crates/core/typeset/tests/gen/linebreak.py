#!/usr/bin/env python3
"""Generates tests/fixtures/linebreak.txt from the oracle (e-TeX in INITEX
mode, run through scripts/sandbox). Paragraphs of rules, glue, penalties,
kerns and discretionaries under varied parameters; the expectation is
\\showbox of the \\vbox that holds the broken paragraph.

    python3 crates/core/typeset/tests/gen/linebreak.py  (from the repo root)
"""
import os, random, re, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '../../../../..'))
WORK = os.path.join(ROOT, 'target', 'probe-linebreak')
OUT = os.path.join(ROOT, 'crates/core/typeset/tests/fixtures/linebreak.txt')
PT = 65536

def para(rng):
    toks = []
    n = rng.randint(8, 60)
    for i in range(n):
        toks.append('w%d' % rng.randint(3 * PT, 45 * PT))
        if i == n - 1:
            break
        r = rng.random()
        if r < 0.08:
            toks.append('p%d' % rng.choice([-10000, -500, 0, 50, 300, 10000]))
        elif r < 0.16:
            toks.append('d%d,%d,%d' % (rng.choice([0, rng.randint(PT, 4 * PT)]),
                                      rng.choice([0, rng.randint(PT, 4 * PT)]),
                                      rng.choice([0, rng.randint(PT, 4 * PT)])))
            continue
        elif r < 0.2:
            toks.append('k%d' % rng.randint(-PT, 3 * PT))
        sh = rng.choice([PT, PT // 2, 2 * PT // 3])
        toks.append('g%d,%d,%d' % (rng.randint(2 * PT, 5 * PT), rng.randint(PT, 3 * PT), sh))
    return toks

def params(rng):
    p = {
        'hsize': rng.randint(60, 220) * PT + rng.randint(0, PT - 1),
        'pretolerance': rng.choice([-1, 100, 100, 1000]),
        'tolerance': rng.choice([100, 200, 200, 500, 1000, 10000]),
        'looseness': rng.choice([0, 0, 0, 0, 1, -1, 2]),
        'linepenalty': rng.choice([10, 10, 100, 0]),
        'hyphenpenalty': rng.choice([50, 50, 500, -100]),
        'exhyphenpenalty': rng.choice([50, 1000]),
        'adjdemerits': rng.choice([10000, 0, 500000]),
        'doublehyphendemerits': rng.choice([10000, 0]),
        'finalhyphendemerits': rng.choice([5000, 0]),
        'emergencystretch': rng.choice([0, 0, 0, 10 * PT]),
        'hangindent': 0, 'hangafter': 1,
        'lastlinefit': rng.choice([0, 0, 0, 500, 1000]),
        'clubpenalty': 150, 'widowpenalty': 150, 'brokenpenalty': 100,
        'interlinepenalty': rng.choice([0, 0, 7]),
        'leftskip': '0,0,0', 'rightskip': '0,0,0',
        'parfillskip': rng.choice(['0,65536fil,0', '0,65536fil,0', '0,0,0', '%d,65536fill,0' % (5 * PT)]),
        'parshape': '',
        'interlinepenalties': '', 'clubpenalties': '', 'widowpenalties': '',
    }
    r = rng.random()
    if r < 0.2:
        p['hangindent'] = rng.choice([1, -1]) * rng.randint(5, 30) * PT
        p['hangafter'] = rng.choice([-2, -1, 0, 1, 3])
    elif r < 0.3:
        k = rng.randint(1, 4)
        p['parshape'] = ' '.join('%d:%d' % (rng.randint(0, 20) * PT, rng.randint(50, 200) * PT) for _ in range(k))
    if rng.random() < 0.2:
        p['leftskip'] = '%d,%d,0' % (rng.randint(0, 10) * PT, rng.choice([0, 5 * PT]))
    if rng.random() < 0.2:
        p['rightskip'] = '0,%d,0' % rng.choice([0, 20 * PT])
    if rng.random() < 0.15:
        p['interlinepenalties'] = ' '.join(str(rng.randint(-50, 300)) for _ in range(rng.randint(1, 3)))
    if rng.random() < 0.15:
        p['clubpenalties'] = ' '.join(str(rng.randint(0, 300)) for _ in range(rng.randint(1, 3)))
    if rng.random() < 0.15:
        p['widowpenalties'] = ' '.join(str(rng.randint(0, 300)) for _ in range(rng.randint(1, 3)))
    return p

def glue_tex(s):
    w, st, sh = s.split(',')
    def inf(x):
        for o in ('filll', 'fill', 'fil'):
            if x.endswith(o):
                v = int(x[:-len(o)])
                assert v % PT == 0
                return '%d%s' % (v // PT, o)
        return '%dsp' % int(x)
    return '%dsp plus %s minus %s' % (int(w), inf(st), inf(sh))

def tex_for(p, toks):
    out = []
    for k in ('hsize', 'emergencystretch', 'hangindent'):
        out.append('\\%s=%dsp' % (k, p[k]))
    for k in ('pretolerance', 'tolerance', 'looseness', 'linepenalty', 'hyphenpenalty', 'exhyphenpenalty',
              'adjdemerits', 'doublehyphendemerits', 'finalhyphendemerits', 'hangafter', 'lastlinefit',
              'clubpenalty', 'widowpenalty', 'brokenpenalty', 'interlinepenalty'):
        out.append('\\%s=%d' % (k, p[k]))
    for k in ('leftskip', 'rightskip', 'parfillskip'):
        out.append('\\%s=%s' % (k, glue_tex(p[k])))
    if p['parshape']:
        pairs = p['parshape'].split()
        out.append('\\parshape=%d %s' % (len(pairs), ' '.join('%ssp %ssp' % tuple(x.split(':')) for x in pairs)))
    for k in ('interlinepenalties', 'clubpenalties', 'widowpenalties'):
        if p[k]:
            v = p[k].split()
            out.append('\\%s=%d %s' % (k, len(v), ' '.join(v)))
    body = []
    for t in toks:
        c, v = t[0], t[1:]
        if c == 'w':
            body.append('\\vrule width %ssp height 1pt' % v)
        elif c == 'g':
            a, b, s = v.split(',')
            body.append('\\hskip %ssp plus %ssp minus %ssp' % (a, b, s))
        elif c == 'p':
            body.append('\\penalty%s ' % v)
        elif c == 'k':
            body.append('\\kern%ssp ' % v)
        elif c == 'd':
            parts = ['\\vrule width %ssp height 1pt' % x if x != '0' else '' for x in v.split(',')]
            body.append('\\discretionary{%s}{%s}{%s}' % tuple(parts))
    return '\\setbox0\\vbox{%s\\noindent %s\\par}\\showbox0\n' % (' '.join(out), ''.join(body))

def main():
    rng = random.Random(20261011)
    os.makedirs(WORK, exist_ok=True)
    cases = [(params(rng), para(rng)) for _ in range(80)]
    src = ['\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6',
           '\\tracingonline1 \\showboxdepth=100 \\showboxbreadth=100000 \\hbadness=10000 \\hfuzz=1000pt',
           '\\baselineskip=0pt \\lineskip=0pt \\lineskiplimit=0pt \\overfullrule=0pt']
    for p, t in cases:
        src.append(tex_for(p, t))
    src.append('\\end\n')
    with open(os.path.join(WORK, 'lb.tex'), 'w') as f:
        f.write('\n'.join(src))
    subprocess.run([os.path.join(ROOT, 'scripts/sandbox'), 'sh', '-c',
                    'cd target/probe-linebreak && etex -ini -etex -interaction=batchmode lb.tex'],
                   cwd=ROOT, stdout=subprocess.DEVNULL, check=False)
    log = open(os.path.join(WORK, 'lb.log'), encoding='latin-1').read()
    boxes = re.findall(r'> \\box0=\n(.*?)\n\n! OK', log, re.S)
    assert len(boxes) == len(cases), (len(boxes), len(cases))
    with open(OUT, 'w') as f:
        for i, ((p, t), b) in enumerate(zip(cases, boxes)):
            f.write('case %d\n' % i)
            for k, v in p.items():
                f.write('%s=%s\n' % (k, v))
            f.write('list=%s\n' % ' '.join(t))
            f.write('expect\n%s\nend\n' % b)

main()
