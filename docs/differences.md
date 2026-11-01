# Where PhiTeX's PDF differs from pdfTeX's on purpose

PhiTeX writes the PDF that pdflatex writes, byte for byte. A difference is
a bug, except where pdfTeX itself has a bug whose output depends on
something no document controls, such as memory it never initialised. For
those cases PhiTeX writes what pdfTeX means to write, and the case is
listed here. Each entry gives:

- what pdfTeX does, with the place in `pdftex.web`;
- why that is a bug;
- what PhiTeX writes instead;
- whether a reader of the PDF can see the difference;
- the manuals affected (`ci/manuals-pdftex-bugs.tsv`);
- a minimal repro.

The gate holds a listed manual to everything except the listed difference.
Each row of `ci/manuals-pdftex-bugs.tsv` gives the manual, the entry here,
a mask (`scripts/ci/pdfdiff.py`, `MASKS`) and the oracle's object numbers
the mask may change. For such a manual `sweep.py` builds pdflatex's PDF
and compares the two files object by object, with streams compared as
written. The files pass, with status `documented`, only when three things
hold:

- every object sits in the same place, with the same object stream and
  index;
- every unlisted object is identical;
- each listed object is identical once the mask is applied to both sides.

Any other difference is red, as it is for every other manual. To find
the objects for a new row, run `python3 scripts/ci/pdfdiff.py ORACLE.pdf
PHITEX.pdf MASK`.

## Destinations under a matrix

<a id="dest-under-matrix"></a>

**pdfTeX.** `do_dest` in `pdftex.web` (the "pdf_dest_xyz: if matrixused
then set_rect_dimens(p, parent_box, x, y, pdf_width(p), pdf_height(p),
pdf_depth(p), pdf_dest_margin)" branch, and the same for `fith`, `fitbh`,
`fitv` and `fitbv`) places a destination inside a `\pdfsetmatrix` group
(for example `\rotatebox`) by transforming a rectangle. That rectangle is
built from the dest node's `pdf_width`, `pdf_height` and `pdf_depth`.
`scan_pdf_dest` sets those fields only for `fitr`. For every other kind
the node comes from `get_node`, which does not clear memory, so the fields
hold whatever an earlier node left there. The `/XYZ` point pdfTeX writes
therefore depends on that leftover memory: in heria it is about 4096pt
above the page.

**Why it is a bug.** The point depends on the history of TeX's node
memory, not on the document. Without a matrix, the same destination is
written at its own position (`pdf_left := cur_h; pdf_top := cur_v`).

**PhiTeX.** PhiTeX takes the width, height and depth as zero, so it
writes the destination's own point, transformed by the matrix (plus
`\pdfdestmargin`, as pdfTeX's branch adds it). Under the identity matrix
this is exactly the point written without a matrix. Before this change,
PhiTeX took them as running dimensions, which put the target at a corner
of the rotated parent box: heria's vertical-mode target sat at a corner
of its whole canvas.

**Visible.** Yes. A link to such a target, or an outline entry, lands
where the target is. With pdfTeX it lands at a point that has nothing to
do with the target, often above the page, so the viewer scrolls to the
top of the page or past it.

**Manuals.** `latex_heria_heria-proposal`; its summary canvases are
`\rotatebox{90}` boxes containing `\hypertarget`s.

**Repro.**

```tex
\documentclass{article}
\usepackage{graphicx}
\usepackage{hyperref}
\begin{document}
\rotatebox{90}{\parbox[c][\textwidth][c]{\textheight}{%
\hypertarget{one}{A} text \hbox to 100pt{\hypertarget{two}{B}\hfil}
\vbox{\hsize=50pt \hypertarget{three}{C}}}}
\end{document}
```

pdflatex writes `one`, `two` and `three` at y = 4199.95, 4231.499 and
4334.446 on a page 792 high. PhiTeX writes each target at its rotated
point on the page. The test `dests_under_a_matrix` in
`crates/engines/tex/tests/pdftex.rs` checks the identity, a turn about the
point itself, and a turned offset.
