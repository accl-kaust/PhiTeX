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
import os, re, shutil, subprocess, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tfm import tfm  # noqa: E402

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
