# fixins.py base|insert FILE: insert a plain dark remember-picture frame before "Frame 9"
import sys
mode, p = sys.argv[1], sys.argv[2]
s = open(p).read()
fr = r"""{\setbeamercolor{background canvas}{bg=blue!30!black}
\begin{frame}[plain]
\begin{tikzpicture}[remember picture,overlay]
  \node[font=\Huge,text=red] at ([yshift=20mm]current page.center) {NEW};
  \node[text=white] at ([yshift=-4mm]current page.center) {Inserted frame.};
\end{tikzpicture}
\end{frame}}
"""
anchor = r"\begin{frame}{Frame 9}"
if mode == "insert":
    assert fr not in s; i = s.index(anchor); s = s[:i] + fr + s[i:]
else:
    s = s.replace(fr, "")
open(p, "w").write(s)
