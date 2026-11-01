#!/usr/bin/env python3
"""Writes the correctness ladder's documents (oracle/fixtures/ladder/<nn>-<name>/):
each rung's NAME.tex and its two edits, edit-word/NAME.tex (one word of the
middle paragraph changed) and edit-para/NAME.tex (a paragraph inserted
before it). The middle paragraph follows the line `%MID`. Deterministic;
the oracle outputs come from scripts/bless-ladder.sh."""
import os, sys, shutil

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "oracle", "fixtures", "ladder")
WORDS = ("the of a computer page line word box glue penalty paragraph break "
         "measure document galaxy craftsman typesetting beautiful river column "
         "table figure index reference section chapter number letter font "
         "shape height depth width kern ligature accent hyphen margin "
         "baseline register token macro group mode list node insert mark").split()

class Rng:
    def __init__(self, s): self.s = s
    def next(self):
        self.s = (self.s * 1103515245 + 12345) % 2**31
        return self.s

def para(r, n=None):
    n = n or 40 + r.next() % 50
    out, cap = [], True
    for i in range(n):
        w = WORDS[r.next() % len(WORDS)]
        if cap: w = w.capitalize(); cap = False
        if i == n - 1: w += "."
        elif r.next() % 11 == 0: w += "."; cap = True
        elif r.next() % 7 == 0: w += ","
        out.append(w)
    # wrap at ~70 columns
    lines, cur = [], ""
    for w in out:
        if len(cur) + len(w) + 1 > 70: lines.append(cur); cur = w
        else: cur = (cur + " " + w).strip()
    lines.append(cur)
    return "\n".join(lines)

def paras(r, k, mid_at=None, extra=None):
    """k paragraphs; `%MID` before paragraph mid_at (default k//2)."""
    mid_at = k // 2 if mid_at is None else mid_at
    out = []
    for i in range(k):
        p = para(r)
        if extra: p = extra(i, p)
        if i == mid_at: p = "%MID\n" + p
        out.append(p)
    return "\n\n".join(out)

R = {}

def rung(nn, name):
    def deco(f): R[(nn, name)] = f; return f
    return deco

@rung("01", "plain")
def plain(r):
    body = []
    for i in range(18):
        p = para(r)
        if i % 3 == 1:
            p += "\\footnote{$^{%d}$}{A footnote, %s}" % (i, para(r, 12))
        if i % 5 == 2:
            p += "\n\\insert\\topins{\\vbox{\\hrule\\smallskip\\centerline{An insert %d}\\smallskip\\hrule}}" % i
        if i == 9: p = "%MID\n" + p
        body.append(p)
    return ("\\vsize=6in\n\\headline={\\tenrm\\firstmark\\hfil\\folio}\n"
            "\\centerline{\\bf A LONGER STORY}\n\\mark{Start}\\bigskip\n\n"
            + "\n\n".join(body) + "\n\\bye\n")

@rung("02", "article")
def article(r):
    return ("\\documentclass{article}\n\\begin{document}\n\n"
            + paras(r, 20) + "\n\n\\end{document}\n")

@rung("03", "structure")
def structure(r):
    s, n = [], 0
    for sec in range(6):
        s.append("\\section{Section %d of the %s}\\label{sec:%d}" % (sec + 1, WORDS[sec + 3], sec))
        for sub in range(2):
            s.append("\\subsection{Part %d}\\label{sub:%d:%d}" % (sub + 1, sec, sub))
            for k in range(3):
                p = para(r)
                if k == 1:
                    p += " See Section~\\ref{sec:%d} on page~\\pageref{sec:%d}.\\footnote{%s}" % ((sec + 2) % 6, (sec + 2) % 6, para(r, 10))
                if sec == 3 and sub == 0 and k == 0: p = "%MID\n" + p
                s.append(p)
            if sub == 1:
                env = "enumerate" if sec % 2 else "itemize"
                s.append("\\begin{%s}\n" % env + "\n".join("\\item %s\\label{it:%d:%d}" % (para(r, 15), sec, j) for j in range(4)) + "\n\\end{%s}" % env)
                s.append("Item~\\ref{it:%d:2} is on page~\\pageref{it:%d:2}." % (sec, sec))
    return ("\\documentclass{article}\n\\begin{document}\n\\tableofcontents\n\n"
            + "\n\n".join(s) + "\n\n\\end{document}\n")

@rung("04", "math")
def math(r):
    s = []
    for i in range(24):
        p = para(r, 30) + " Inline $a_{%d}^2+b^2=\\sqrt{c_%d}$ and $\\sum_{k=1}^{n} k=\\frac{n(n+1)}{2}$." % (i, i)
        if i == 12: p = "%MID\n" + p
        s.append(p)
        if i % 4 == 0:
            s.append("\\begin{equation}\\label{eq:%d}\n\\int_0^\\infty e^{-x^%d}\\,dx = \\Gamma\\Bigl(1+\\frac{1}{%d}\\Bigr)\n\\end{equation}" % (i, i + 1, i + 1))
        elif i % 4 == 1:
            s.append("\\begin{align}\nf(x) &= \\alpha x^2 + \\beta x + \\gamma \\label{al:%d}\\\\\ng(x) &\\leq \\mathbb{R}\\cap\\mathcal{O}(x^{%d})\n\\end{align}" % (i, i))
        elif i % 4 == 2:
            s.append("\\begin{gather}\nA = \\begin{pmatrix} 1 & %d \\\\ 0 & 1\\end{pmatrix}\\\\\n\\varepsilon \\lesssim \\delta\\nonumber\n\\end{gather}" % i)
            s.append("By \\eqref{eq:%d} and \\eqref{al:%d} we have \\[ \\prod_{j=1}^{%d} \\left(1-\\frac{1}{p_j}\\right) \\neq 0. \\]" % (i - 2, i - 1, i))
        else:
            s.append("\\begin{align*}\nx &= y & z &= w\\\\\n\\iint_D \\nabla\\cdot F\\,dA &= \\oint_{\\partial D} F\\cdot n\\,ds\n\\end{align*}")
    return ("\\documentclass{article}\n\\usepackage{amsmath,amssymb}\n\\begin{document}\n\n"
            + "\n\n".join(s) + "\n\n\\end{document}\n")

@rung("05", "floats")
def floats(r):
    s = []
    for i in range(16):
        p = para(r)
        if i == 8: p = "%MID\n" + p
        s.append(p)
        if i % 3 == 0:
            img = "small.png" if i % 2 else "small.pdf"
            s.append("\\begin{figure}[tp]\n\\centering\n\\includegraphics[width=%dcm]{%s}\n\\caption{Figure from %s, number %d.}\\label{fig:%d}\n\\end{figure}" % (3 + i % 4, img, img, i, i))
        if i % 4 == 1:
            rows = "\\\\\n".join("%s & %d & %d.%d" % (WORDS[(i + j) % len(WORDS)], j, i, j) for j in range(6))
            s.append("\\begin{table}[htbp]\n\\centering\n\\caption{Table %d.}\\label{tab:%d}\n\\begin{tabular}{lrr}\n\\toprule\nName & A & B\\\\\n\\midrule\n%s\\\\\n\\bottomrule\n\\end{tabular}\n\\end{table}" % (i, i, rows))
        if i % 5 == 4:
            s.append("Figure~\\ref{fig:%d} and Table~\\ref{tab:%d} are on pages \\pageref{fig:%d} and \\pageref{tab:%d}." % (i // 3 * 3, (i // 4) * 4 + 1 if (i // 4) * 4 + 1 <= i else 1, i // 3 * 3, (i // 4) * 4 + 1 if (i // 4) * 4 + 1 <= i else 1))
    return ("\\documentclass{article}\n\\usepackage{graphicx,booktabs}\n\\begin{document}\n\\listoffigures\n\\listoftables\n\n"
            + "\n\n".join(s) + "\n\n\\end{document}\n")

BIB = [
    ("knuth84", "book", "author={Donald E. Knuth}, title={The {\\TeX}book}, publisher={Addison-Wesley}, year={1984}"),
    ("knuth86", "book", "author={Donald E. Knuth}, title={{\\TeX}: The Program}, publisher={Addison-Wesley}, year={1986}"),
    ("lamport94", "book", "author={Leslie Lamport}, title={{\\LaTeX}: A Document Preparation System}, publisher={Addison-Wesley}, year={1994}, edition={2nd}"),
    ("plass81", "phdthesis", "author={Michael F. Plass}, title={Optimal Pagination Techniques for Automatic Typesetting Systems}, school={Stanford University}, year={1981}"),
    ("knuthplass81", "article", "author={Donald E. Knuth and Michael F. Plass}, title={Breaking Paragraphs into Lines}, journal={Software: Practice and Experience}, volume={11}, number={11}, pages={1119--1184}, year={1981}"),
    ("liang83", "phdthesis", "author={Frank M. Liang}, title={Word Hy-phen-a-tion by Com-put-er}, school={Stanford University}, year={1983}"),
    ("mittelbach04", "book", "author={Frank Mittelbach and Michel Goossens}, title={The {\\LaTeX} Companion}, publisher={Addison-Wesley}, year={2004}"),
    ("thanh01", "inproceedings", "author={H\\`an Th\\'{\\^e} Th\\`anh}, title={Micro-typographic extensions to the {\\TeX} typesetting system}, booktitle={TUGboat}, pages={317--434}, year={2000}"),
    ("patashnik88", "manual", "author={Oren Patashnik}, title={{BibTeX}ing}, year={1988}"),
    ("tantau13", "manual", "author={Till Tantau}, title={The {TikZ} and {PGF} Packages}, year={2013}, note={Manual for version 3.0}"),
]

def bibfile():
    return "\n\n".join("@%s{%s,\n  %s\n}" % (t, k, b.replace(", ", ",\n  ")) for k, t, b in BIB) + "\n"

@rung("06", "bibtex")
def bibtex(r):
    s = []
    for i in range(12):
        p = para(r) + " See \\cite{%s} and \\citep{%s}." % (BIB[i % 10][0], BIB[(i * 3 + 1) % 10][0])
        if i == 6: p = "%MID\n" + p
        s.append(p)
    return ("\\documentclass{article}\n\\usepackage{natbib}\n\\begin{document}\n\n"
            + "\n\n".join(s) + "\n\n\\bibliographystyle{plainnat}\n\\bibliography{refs}\n\\end{document}\n")

@rung("07", "index")
def index(r):
    s = []
    for i in range(16):
        ws = [WORDS[(i * 7 + j) % len(WORDS)] for j in range(3)]
        p = para(r) + " " + " ".join("%s\\index{%s}" % (w, w) for w in ws) + " sub\\index{%s!%s}." % (ws[0], ws[1])
        if i == 8: p = "%MID\n" + p
        s.append(p)
    return ("\\documentclass{article}\n\\usepackage{makeidx}\n\\makeindex\n\\begin{document}\n\n"
            + "\n\n".join(s) + "\n\n\\printindex\n\\end{document}\n")

@rung("08", "hyperref")
def hyperref(r):
    s = []
    for sec in range(5):
        s.append("\\section{The %s and the %s}\\label{s%d}" % (WORDS[sec + 5], WORDS[sec + 9], sec))
        for k in range(3):
            p = para(r)
            if k == 1: p += " Back to Section~\\ref{s%d}, see \\href{https://www.tug.org/%d}{the TUG page} or \\url{https://ctan.org/pkg/hyperref}." % ((sec + 1) % 5, sec)
            if sec == 2 and k == 0: p = "%MID\n" + p
            s.append(p)
        s.append("\\subsection{Accents: caf\\'e, na\\\"ive, \\c{c}a, $\\alpha$}")
    return ("\\documentclass{article}\n\\usepackage[colorlinks,bookmarks]{hyperref}\n\\begin{document}\n\\tableofcontents\n\n"
            + "\n\n".join(s) + "\n\n\\end{document}\n")

UTF = ["Caf\u00e9 cr\u00e8me, na\u00efve fa\u00e7ade, \u00fcber Stra\u00dfe, \u00e5ngstr\u00f6m, ma\u00f1ana.",
       "Le c\u0153ur, d\u00e9j\u00e0 vu, pi\u00f1ata, r\u00e9sum\u00e9, \u00e9l\u00e8ve, \u00e2me."]

@rung("09", "babel")
def babel(r):
    s = []
    for i in range(14):
        p = para(r) + " " + UTF[i % 2] + " hyphenationdemonstration extraordinarily"
        if i % 4 == 3:
            p = "\\begin{otherlanguage}{british}\n%s colour behaviour\n\\end{otherlanguage}" % p
        if i == 7: p = "%MID\n" + p
        s.append(p)
    return ("\\documentclass{article}\n\\usepackage[T1]{fontenc}\n\\usepackage[utf8]{inputenc}\n\\usepackage{lmodern}\n"
            "\\usepackage[british,american]{babel}\n\\usepackage{microtype}\n\\begin{document}\n\n"
            + "\n\n".join(s) + "\n\n\\end{document}\n")

@rung("10", "tikz")
def tikz(r):
    pics = [
        "\\begin{tikzpicture}[->,>=stealth]\n\\node[draw,circle] (a) at (0,0) {A};\n\\node[draw,rectangle] (b) at (3,1) {B};\n\\node[draw,ellipse] (c) at (3,-1) {C};\n\\draw (a) -- (b); \\draw[thick,red] (a) to[bend right] (c); \\draw[dashed] (b) -- (c);\n\\end{tikzpicture}",
        "\\begin{tikzpicture}\n\\draw[->] (-0.2,0) -- (4,0) node[right] {$x$};\n\\draw[->] (0,-1.2) -- (0,1.2) node[above] {$y$};\n\\draw[domain=0:4,smooth,variable=\\x,blue] plot ({\\x},{sin(\\x r)});\n\\fill[green!30] (1,0.2) rectangle (1.5,0.7);\n\\end{tikzpicture}",
        "\\begin{tikzpicture}\n\\foreach \\i in {0,...,5} \\draw[fill=blue!\\i0] (\\i,0) circle (0.4) node {\\i};\n\\end{tikzpicture}",
        "\\begin{tikzpicture}\n\\begin{axis}[width=7cm,height=4cm]\n\\addplot[domain=-2:2,samples=20] {x^2};\n\\addplot coordinates {(-2,1) (0,2) (2,1)};\n\\end{axis}\n\\end{tikzpicture}",
    ]
    s = []
    for i in range(10):
        p = para(r)
        if i == 5: p = "%MID\n" + p
        s.append(p)
        if i % 2 == 0: s.append("\\begin{center}\n%s\n\\end{center}" % pics[(i // 2) % 4])
    return ("\\documentclass{article}\n\\usepackage{tikz}\n\\usetikzlibrary{shapes.geometric}\n\\usepackage{pgfplots}\n\\pgfplotsset{compat=1.18}\n\\begin{document}\n\n"
            + "\n\n".join(s) + "\n\n\\end{document}\n")

@rung("11", "biblatex")
def biblatex(r):
    src = os.path.join(ROOT, "..", "biber", "numeric.tex")
    t = open(src).read()
    return t.replace("\\begin{document}\n", "\\begin{document}\n%MID\n" + para(r) + "\n\n", 1)

@rung("12", "book")
def book(r):
    s = []
    for ch in range(10):
        s.append("\\chapter{The %s}" % WORDS[ch + 4])
        for sec in range(3):
            s.append("\\section{About the %s}" % WORDS[(ch * 3 + sec) % len(WORDS)])
            for k in range(5):
                p = para(r)
                if ch == 4 and sec == 0 and k == 0: p = "%MID\n" + p
                s.append(p)
    return ("\\documentclass[twoside]{book}\n\\usepackage{fancyhdr}\n\\pagestyle{fancy}\n"
            "\\fancyhead[LE,RO]{\\thepage}\n\\fancyhead[RE]{\\leftmark}\n\\fancyhead[LO]{\\rightmark}\n\\fancyfoot{}\n"
            "\\begin{document}\n\\tableofcontents\n\n" + "\n\n".join(s) + "\n\n\\end{document}\n")

def edits(name, t, r):
    at = t.index("%MID\n") + 5
    w = t.index(" the ", at) if " the " in t[at:at + 400] else t.index(" ", at + 10)
    word = t[:w] + " thy " + t[w + 5:] if t[w:w + 5] == " the " else t[:w] + " zebra" + t[w:]
    para_ = t[:at - 5] + para(r) + "\n\n" + t[at - 5:]
    return word, para_

def main():
    only = set(sys.argv[1:])
    for (nn, name), f in sorted(R.items()):
        if only and nn not in only: continue
        d = os.path.join(ROOT, "%s-%s" % (nn, name))
        os.makedirs(d, exist_ok=True)
        r = Rng(int(nn) * 7919)
        t = f(r)
        open(os.path.join(d, name + ".tex"), "w").write(t)
        word, ins = edits(name, t, Rng(int(nn) * 104729))
        for sub, txt in (("edit-word", word), ("edit-para", ins)):
            os.makedirs(os.path.join(d, sub), exist_ok=True)
            open(os.path.join(d, sub, name + ".tex"), "w").write(txt)
        if name == "bibtex":
            open(os.path.join(d, "refs.bib"), "w").write(bibfile())
        if name == "biblatex":
            shutil.copy(os.path.join(ROOT, "..", "biber", "refs.bib"), d)
        if name == "floats":
            for a in ("small.png", "small.pdf"):
                p = os.path.join(d, a)
                if not os.path.exists(p): print("missing asset", p, file=sys.stderr)

main()
