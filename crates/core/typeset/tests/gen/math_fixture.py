#!/usr/bin/env python3
"""Generates the math fixtures from the oracle (e-TeX with its
plain-based etex.fmt, run through scripts/sandbox):

  fixtures/math-fonts.txt  plain's ten math fonts (metrics, lig/kern,
                           extensibles, parameters), in font-id order
  fixtures/math.txt        per formula: the mlist \\showlists prints just
                           before the closing $, and the hlist between
                           \\mathon and \\mathoff of the paragraph's line

    python3 crates/core/typeset/tests/gen/math_fixture.py  (from the repo root)
"""
import os, re, subprocess, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tfm import tfm  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '../../../../..'))
WORK = os.path.join(ROOT, 'target', 'probe-math')
FIX = os.path.join(ROOT, 'crates/core/typeset/tests/fixtures')

FONTS = [('tenrm', 'cmr10'), ('sevenrm', 'cmr7'), ('fiverm', 'cmr5'),
         ('teni', 'cmmi10'), ('seveni', 'cmmi7'), ('fivei', 'cmmi5'),
         ('tensy', 'cmsy10'), ('sevensy', 'cmsy7'), ('fivesy', 'cmsy5'),
         ('tenex', 'cmex10'), ('tenbf', 'cmbx10'), ('sevenbf', 'cmbx7'),
         ('fivebf', 'cmbx5'), ('tenit', 'cmti10')]

FORMULAS = [
    r'x^2', r'x_i^2', r'x_i', r'a+b=c', r'-x+-y', r'{a\over b}', r'\sqrt{x+y}',
    r'\sum_{i=1}^n x_i', r'\displaystyle\sum_{i=1}^n x_i', r'\int_0^1 f(x)\,dx',
    r'\displaystyle\int_0^1 f(x)\,dx', r'\left( a\over b\right)', r'\overline{ab}',
    r'\underline{x}y', r'\hat a', r'\widehat{xyz}', r'\vec x_1', r'{a\atop b}',
    r'{n\choose k}', r'\displaystyle{n\choose k}', r'x\mathbin{*}y', r"f'(x)",
    r'\lim_{n\to\infty}x_n', r'\displaystyle\lim_{n\to\infty}x_n', r'\bigl(x\bigr)',
    r'\mathop{\rm sin}\nolimits x', r'a\mathpunct{,}b', r'x^{\mathchoice{D}{T}{S}{s}}',
    r'\mathchoice{D}{T}{S}{s}', r'a\nonscript\mskip 6mu b^{a\nonscript\mskip 6mu b}',
    r'a\mkern3mu b\mskip 2mu plus 1mu minus 1mu c', r'\vcenter{\hbox{x}}',
    r"{\rm --}\ {\rm ``x''}", r'\left(\middle| x\right)', r'\sqrt{\sqrt{x^2}}',
    r'\left(\vcenter{\hrule height 30pt width 1pt}\right.', r'\displaystyle{a+b\over c}',
    r'{\textstyle {1\over2}}^{1\over 2}', r'x\over\displaystyle y', r'a=\,b\;c\!d',
    r'\left\{x\right\}', r'\Bigg\langle x\Bigg\rangle', r'\root 3\of x',
    r'\bf x+\it y', r'\mathrel{=}\mathrel{=}a\mathbin{+}', r'(+)', r'\{a\mathpunct{;}+b\}',
    r'x\atopwithdelims[] y', r'\displaystyle\prod\limits_k\coprod\nolimits^j',
    r'\hbox{a}^2_3\vrule height 9pt depth 2pt', r'\penalty5 a\discretionary{x}{y}{}b',
    r'\mathaccent"7015 a^2', r'\tilde{\tilde{A}}', r'\dot\imath', r'AV\mathrel{AV}',
]

def main():
    os.makedirs(WORK, exist_ok=True)
    kp = lambda f: subprocess.run(['kpsewhich', f], capture_output=True, text=True).stdout.strip()
    with open(os.path.join(FIX, 'math-fonts.txt'), 'w') as f:
        for name, file in FONTS:
            f.write('font %s\n' % name)
            f.write('\n'.join(tfm(kp(file + '.tfm'))) + '\n')
    src = ['\\tracingonline1 \\showboxdepth=100 \\showboxbreadth=100000 \\hbadness=10000',
           '\\catcode`\\^^M=9 \\hsize=16000pt \\parindent=0pt \\parfillskip=0pt plus 1fil']
    for t in FORMULAS:
        src.append('\\setbox0\\vbox{\\noindent$%s\\showlists$\\par}\\showbox0' % t)
    src.append('\\end')
    open(os.path.join(WORK, 'm.tex'), 'w').write('\n'.join(src) + '\n')
    subprocess.run([os.path.join(ROOT, 'scripts/sandbox'), 'sh', '-c',
                    'cd target/probe-math && etex -interaction=batchmode m.tex'],
                   cwd=ROOT, stdout=subprocess.DEVNULL, check=False)
    log = open(os.path.join(WORK, 'm.log'), encoding='latin-1').read()
    assert '! ' not in log.replace('! OK', ''), 'errors in m.log'
    mlists = re.findall(r'### math mode entered at line \d+\n(.*?)\n### ', log, re.S)
    hlists = [re.search(r'\n\.\.\\mathon\n(.*?)\n\.\.\\mathoff\n', b.split('! OK')[0], re.S).group(1)
              for b in log.split('> \\box0=')[1:]]
    assert len(mlists) == len(FORMULAS) == len(hlists), (len(mlists), len(hlists))
    with open(os.path.join(FIX, 'math.txt'), 'w') as f:
        for i, (t, m, h) in enumerate(zip(FORMULAS, mlists, hlists)):
            h = '\n'.join(l[2:] for l in h.split('\n'))
            f.write('case %d\ntext=%s\nexpect\n%s\n%%\n%s\nend\n' % (i, t, m, h))

main()
