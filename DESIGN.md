# PhiTeX — design

The design is `AGENTS_BRIEF.md` and phitex-red's `CONSTRUCTS.md` (TeX as
graph reduction of pending thunks). This file keeps what is not there:
the crates.

The machine-mode engine (phitex-core, phitex-engine, phitex-incr,
phitex-ssa, phitex-trace, phitex-ir and the old CLI) and its design
(chapters 1–6, appendix A: the dynamic SSA over an imperative engine,
windows, checkpoints, the machine runtime) were removed when the CLI
moved onto phitex-flow; they are in the history up to commit cc8ffa53349c.
Comments in the remaining crates that cite `DESIGN 3.x`–`6.x` refer to
that version. The dataflow engine that followed (phitex-flow, its record
CORE.md, and the φ core `phi`, chapter 7) was removed when the CLI moved
onto phitex-red; they are in the history up to commit 77104f08.

## The crates

The engines, in layers (stubs for now; phitex-red is taken apart into
them). Each depends only on the layers below it:

```
phitex-cli
  phitex-pdftex · phitex-xetex · phitex-luatex (+ phitex-lua)    the engines: what makes each one
    phitex-tex                                                   TeX's semantics, shared; the Engine trait
      phitex-reduce   phitex-typeset   phitex-source             core (no TeX) · pure Knuth · source text
    phitex-tfm  phitex-dvi  phitex-pdf  phitex-otf  phitex-xdvipdfmx   fonts and backends
```

- `phitex-reduce`: the reduction core: thunks, their reads, reuse, the
  loop-back fixpoint, the scheduler. Depends on nothing TeX.
- `phitex-typeset`: Knuth's algorithms as pure functions (glue, packaging,
  line breaking, hyphenation, math, alignment, page decisions). Never
  depends on `phitex-reduce` or `phitex-tex`.
- `phitex-tex`: TeX on the core, shared by every engine: 21-bit tokens,
  expansion, macros, conditionals, registers, groups, modes; the `Engine`
  trait. Engines add behaviour through it, never fork it.
- `phitex-pdftex`, `phitex-xetex`, `phitex-luatex`: input decoding,
  primitives, fonts, output; pdfLaTeX, XeLaTeX and LuaLaTeX are these with
  the LaTeX format. `phitex-lua` (pure-Rust Lua) only for `phitex-luatex`.
- `phitex-tfm`, `phitex-dvi`, `phitex-pdf`: TFM/VF, DVI/XDV, pdfTeX's PDF
  backend.

- `phitex-red`: TeX as graph reduction of pending thunks (CONSTRUCTS.md).
  The only engine.
- `phitex-source`: source text as units with identities (positions that
  edits never move), the lexer and the paragraph splitter (TeX's shape
  without running TeX), and the paragraph tree that `phitex-doc` and
  `phitex-diff` read.
- `phitex-cli`: the `phitex` binary on phitex-red: `build` (the document
  to its loop-back fixpoint, BibTeX and makeindex inside it), `watch` (the
  same Build kept, edits as updates, the live viewer), the engines'
  command lines by program name (`pdflatex`, `latex`, `pdftex`, `etex`,
  `tex`), `outline`, `sync`, `clean`.
- `phitex-doc`, `phitex-diff`, `phitex-git`: the document's structure
  without TeX, latexdiff natively, git.
- `phitex-syntax`: the old paragraph CST, now in `phitex-source`; kept
  only for phitex-red's `edit.rs` until the engine is rewritten.
- `phitex-draw`: the viewer's pages from the PDF's bytes.
- `phitex-codecs`: inflate, MD5, PNG, a PDF reader and its page text.
- `phitex-kpse`, `phitex-bibtex`, `phitex-makeindex`, `phitex-otf`,
  `phitex-xdvipdfmx`: kpathsea, BibTeX, makeindex, OpenType shaping,
  xdvipdfmx, ported as pure libraries.
