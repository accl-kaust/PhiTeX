#!/usr/bin/env python3
"""pdflatex -synctex=1's answers for a document, as the SyncTeX tests
read them (crates/engines/tex/tests/latex.rs `latex_synctex_*`; made by
scripts/bless-synctex.sh, which runs this in the build's directory).

Usage: synctex-answers.py NAME > NAME.answers  (in the directory holding
NAME.tex, NAME.pdf and NAME.synctex.gz; needs `synctex` and poppler's
`pdftotext`). Coordinates are PDF points from the page's top left, as
`synctex` gives them; pages count from 1. Three kinds of line:

  view L P X0 Y0 X1 Y1   source line L (with words of its own, which the
                         PDF shows)
                         has a box on page P from (X0, Y0) to (X1, Y1)
                         (its height and its depth, 3 pt, included); page
                         boxes and the foot's line left out: pdfTeX tags
                         them with the line its output routine fired on
  edit P X Y L WORD      the PDF's word WORD, its middle at (X, Y), is on
                         line L: where pdfTeX's answer is that word's own
                         line (the line's text has it, not as an
                         environment's or a label's name)
  span P X Y A B WORD    the PDF's word WORD at (X, Y) is in a display
                         that came from lines A to B, its environment's
                         (pdfTeX tags a display with one of them, mostly
                         its last)

Lines before \begin{document} and in displays have no `view`: pdfTeX
tags a display's boxes with the line it ended on, and a page's boxes
with the line its output routine fired on.
"""
import os, re, subprocess, sys

FOOT = 690.0
name = sys.argv[1]
pdf = os.path.abspath(name + ".pdf")
lines = open(name + ".tex", encoding="latin-1").read().split("\n")

def alnum(s):
    return "".join(c for c in s.lower() if c.isalnum())

def words_of(l):
    return alnum(re.sub(r"\\[A-Za-z]+|%.*|\{[^}]*\}" if l.lstrip().startswith("\\label") else r"\\[A-Za-z]+|%.*", " ", l))

def view(line):
    o = subprocess.run(["synctex", "view", "-i", f"{line}:0:{name}.tex", "-o", pdf],
                       capture_output=True, text=True).stdout
    res, cur = [], {}
    for l in o.splitlines():
        k, _, v = l.partition(":")
        if k in ("Page", "h", "v", "W", "H"):
            cur[k] = float(v)
            if k == "H":
                res.append(cur)
                cur = {}
    return [r for r in res if r["H"] < 60 and r["v"] < FOOT]

def edit(p, x, y):
    o = subprocess.run(["synctex", "edit", "-o", f"{p}:{x:.2f}:{y:.2f}:{pdf}"],
                       capture_output=True, text=True).stdout
    inp = ln = None
    for l in o.splitlines():
        if l.startswith("Input:") and inp is None:
            inp = l[6:]
        if l.startswith("Line:") and ln is None:
            ln = int(l[5:])
    return ln if inp and inp.endswith("/" + name + ".tex") else None

out = [f"# pdflatex -synctex=1's answers for {name}.tex (scripts/synctex-answers.py)"]
# (the displays: their environments' first and last lines)
displays, env = [], None
for i, l in enumerate(lines, 1):
    if env is None and re.search(r"\\begin\{(equation|align|gather|multline)\*?\}|^\s*\\\[\s*$", l):
        env = i
    if env and re.search(r"\\end\{(equation|align|gather|multline)\*?\}|^\s*\\\]\s*$", l):
        displays.append((env, i))
        env = None
display_of = lambda i: next(((a, b) for a, b in displays if a <= i <= b), None)
body = next((i for i, l in enumerate(lines, 1) if "\\begin{document}" in l), 0)
t = subprocess.run(["pdftotext", "-bbox", pdf, "-"], capture_output=True, text=True).stdout
shown = {alnum(m.group(1)) for m in re.finditer(r'">(.*)</word>', t)}
# (a line with words of its own: one of three letters or more the PDF shows)
def glyph_line(l):
    ws = re.findall(r"[A-Za-z0-9]{3,}", re.sub(r"\\[A-Za-z]+|%.*", " ", l))
    return any(alnum(w) in shown for w in ws)
# (an environment's or a label's line: its words are names, not text)
structural = lambda l: re.match(r"\s*\\(begin|end|label)\b", l)
for i, l in enumerate(lines, 1):
    if i <= body or l.lstrip().startswith("%") or display_of(i):
        continue
    if glyph_line(l) and not structural(l):
        for b in view(i):
            out.append(f"view {i} {b['Page']:.0f} {b['h']:.2f} {b['v'] - b['H']:.2f} {b['h'] + b['W']:.2f} {b['v'] + 3:.2f}")
page = 0
for l in t.splitlines():
    if l.strip().startswith("<page"):
        page += 1
    m = re.search(r'xMin="([\d.]+)" yMin="([\d.]+)" xMax="([\d.]+)" yMax="([\d.]+)">(.*)</word>', l)
    if not m:
        continue
    x0, y0, x1, y1 = map(float, m.groups()[:4])
    w = m.group(5)
    x, y = (x0 + x1) / 2, y0 + (y1 - y0) * 0.6
    a = alnum(w.rstrip("-"))
    if y >= FOOT:
        continue
    ln = edit(page, x, y)
    if not ln:
        continue
    d = display_of(ln)
    # (a word of the text before the display: pdfTeX tags the paragraph's
    # last line, broken where the display begins, with its first line)
    before = d and len(a) >= 2 and any(a in words_of(lines[k - 1]) for k in range(max(d[0] - 5, 1), d[0]))
    if d and not before:
        out.append(f"span {page} {x:.2f} {y:.2f} {d[0]} {d[1]} {w}")
    elif len(a) >= 2 and a in words_of(lines[ln - 1]) and not structural(lines[ln - 1]):
        out.append(f"edit {page} {x:.2f} {y:.2f} {ln} {w}")
print("\n".join(dict.fromkeys(out)))
