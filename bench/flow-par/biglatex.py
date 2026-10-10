#!/usr/bin/env python3
# biglatex.py N > biglatex.tex: sections.tex's body N times (labels made
# distinct), blank lines between its paragraphs: a LaTeX document with many
# cuts in its body
import sys
n = int(sys.argv[1]) if len(sys.argv) > 1 else 40
body = open('crates/phitex-flow/tests/latex/docs/sections.tex').read()
body = body.split('\\begin{document}\n', 1)[1].rsplit('\\end{document}', 1)[0]
out = ['\\documentclass{article}\n\\year=2024 \\month=3 \\day=14\n\\begin{document}\n']
for i in range(n):
    b = body
    for l in ('sec:intro', 'sec:more', 'it:first'):
        b = b.replace(l, f'{l}{i}')
    out.append(b + '\n')
out.append('\\end{document}\n')
sys.stdout.write(''.join(out))
