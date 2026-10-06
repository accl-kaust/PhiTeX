//! `cargo xtask e2e`: run partex and the installed oracle engine on the same
//! jobs, side by side, and compare everything observable: the terminal
//! output, every log, and every DVI file.
//!
//! The incremental cases run `partex -watch` and edit its inputs between
//! builds; after each rebuild, the oracle runs afresh on the same files and
//! everything must match again.
//!
//! Each case is a sequence of runs in one fresh directory (so a format
//! dumped by one run is loaded by the next). The first line of each log and
//! of the terminal output (banner and date) is not compared, nor are format
//! files, whose layout is partex's own, nor TeX's memory statistics (see
//! `mask.rs`).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

struct Case {
    name: &'static str,
    /// The oracle binary; partex runs with the same arguments (in that
    /// engine's flavor).
    oracle: &'static str,
    /// Files from `tests/e2e/` copied into the run directory.
    inputs: &'static [&'static str],
    runs: &'static [&'static [&'static str]],
}

/// A run's first argument that makes it a BibTeX run (`bibtex` against
/// `partex -bibtex`).
const BIBTEX: &str = "*bibtex";

/// A run's first argument that makes it a makeindex run (`makeindex`
/// against `partex -makeindex`).
const MAKEINDEX: &str = "*makeindex";

const CASES: &[Case] = &[
    // `\write18` in each mode (web2c's `runsystem`): the log's lines,
    // restricted shell escape's quoting and refusals
    Case {
        name: "shell",
        oracle: "pdftex",
        inputs: &["shell.tex"],
        runs: &[
            &[
                "-ini",
                "-shell-restricted",
                "-interaction=nonstopmode",
                "-jobname=restricted",
                "shell",
            ],
            &[
                "-ini",
                "-shell-escape",
                "-interaction=nonstopmode",
                "-jobname=unrestricted",
                "shell",
            ],
            &[
                "-ini",
                "-no-shell-escape",
                "-interaction=nonstopmode",
                "-jobname=disabled",
                "shell",
            ],
        ],
    },
    // minted v3 through restricted shell escape (latexminted, which TeX
    // Live allows): the first run highlights in a batch at the end, the
    // second reads the cache
    Case {
        name: "minted",
        oracle: "pdftex",
        inputs: &["minted.tex"],
        runs: &[
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-ini",
                "-etex",
                "-jobname=pdflatex",
                "*pdflatex.ini",
            ],
            &[
                "-shell-restricted",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=pdflatex",
                "minted",
            ],
            &[
                "-shell-restricted",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=pdflatex",
                "minted",
            ],
        ],
    },
    Case {
        name: "plain",
        oracle: "tex",
        inputs: &[],
        runs: &[
            &["-ini", "-interaction=nonstopmode", r"\input plain \dump"],
            &[
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "&plain",
                "story",
                r"\end",
            ],
        ],
    },
    Case {
        name: "token_lists",
        oracle: "tex",
        inputs: &[],
        runs: &[
            &["-ini", "-interaction=nonstopmode", r"\input plain \dump"],
            &[
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "&plain",
                r"\def\swap#1#2.{<#2|#1>}\toks0={A{B}}\toks2=\toks0\def\a{\swap x{y}.}\let\b=\a\ifx\a\b\message{IFX-YES}\else\message{IFX-NO}\fi\message{TOKS:\the\toks0}\uppercase{\message{CASE-ok}}\immediate\write16{WRITE:\the\toks2}\mark{top}\halign{#&#\cr a&b\cr}\hbox{\a}\bye",
            ],
        ],
    },
    Case {
        name: "skips",
        oracle: "tex",
        inputs: &["skips.tex"],
        runs: &[&["-ini", "-interaction=nonstopmode", "skips"]],
    },
    Case {
        name: "cut_page",
        oracle: "tex",
        inputs: &[],
        runs: &[
            &["-ini", "-interaction=nonstopmode", r"\input plain \dump"],
            &[
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "&plain",
                r"\def\a{\b\b}\def\b{\c\c}\def\c{\d\d}\def\d{\e\e}\def\e{\f\f}\def\f{\g\g}\def\g{\u\u}\shipout\vbox{\hbox{A}\hbox{\kern1pt\special{x}\hbox{B\write-1{\a}C}\hbox{D}}\hbox{E}}\end",
            ],
        ],
    },
    Case {
        name: "pages",
        oracle: "tex",
        inputs: &["pages.tex"],
        runs: &[
            &["-ini", "-interaction=nonstopmode", r"\input plain \dump"],
            &[
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "&plain",
                "pages",
            ],
        ],
    },
    Case {
        name: "math",
        oracle: "tex",
        inputs: &["math.tex"],
        runs: &[
            &["-ini", "-interaction=nonstopmode", r"\input plain \dump"],
            &[
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "&plain",
                "math",
            ],
        ],
    },
    Case {
        name: "align",
        oracle: "tex",
        inputs: &["align.tex"],
        runs: &[
            &["-ini", "-interaction=nonstopmode", r"\input plain \dump"],
            &[
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "&plain",
                "align",
            ],
        ],
    },
    Case {
        name: "texxet",
        oracle: "pdftex",
        inputs: &["texxet.tex"],
        runs: &[&[
            "-ini",
            "-etex",
            "-no-shell-escape",
            "-no-parse-first-line",
            "-output-format=dvi",
            "-output-comment=partex",
            "-interaction=nonstopmode",
            "texxet",
        ]],
    },
    Case {
        name: "microtype",
        oracle: "pdftex",
        inputs: &["microtype.tex"],
        runs: &[&[
            "-ini",
            "-no-shell-escape",
            "-no-parse-first-line",
            "-interaction=nonstopmode",
            "microtype",
        ]],
    },
    Case {
        name: "etex_format",
        oracle: "pdftex",
        inputs: &["etexfmt.tex", "etexuse.tex"],
        runs: &[
            &[
                "-ini",
                "-etex",
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "etexfmt",
            ],
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=etexfmt",
                "etexuse",
            ],
        ],
    },
    // LaTeX: the format from `latex.ini`, then a document (twice, for its
    // aux and toc files)
    Case {
        name: "latex",
        oracle: "pdftex",
        inputs: &["latexdoc.tex"],
        runs: &[
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-ini",
                "-etex",
                "-jobname=latex",
                "*latex.ini",
            ],
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=latex",
                "latexdoc",
            ],
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=latex",
                "latexdoc",
            ],
        ],
    },
    // PDF inclusion: pages of PDF files (pdfTeX's, with object streams;
    // one made by hand, a classic xref table) through graphicx, as
    // pdftoepdf.cc writes them
    Case {
        name: "images",
        oracle: "pdftex",
        inputs: &["images.tex", "images-fig.pdf", "images-hand.pdf"],
        runs: &[
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-ini",
                "-etex",
                "-jobname=pdflatex",
                "*pdflatex.ini",
            ],
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=pdflatex",
                "images",
            ],
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=pdflatex",
                "images",
            ],
        ],
    },
    // PNG images (copied IDAT data, decoded rows, soft masks, a palette
    // with transparency, 16 bits, interlaced) through graphicx, as
    // writepng.c with libpng writes them
    Case {
        name: "png",
        oracle: "pdftex",
        inputs: &[
            "png.tex",
            "png-rgb8.png",
            "png-rgb8srgb.png",
            "png-gray16.png",
            "png-rgba8.png",
            "png-ga8.png",
            "png-pal4trns.png",
            "png-rgb8i.png",
        ],
        runs: &[
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-ini",
                "-etex",
                "-jobname=pdflatex",
                "*pdflatex.ini",
            ],
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=pdflatex",
                "png",
            ],
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=pdflatex",
                "png",
            ],
        ],
    },
    Case {
        name: "bibtex",
        oracle: "pdftex",
        inputs: &["bibdoc.tex"],
        runs: &[
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-ini",
                "-etex",
                "-jobname=latex",
                "*latex.ini",
            ],
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=latex",
                "bibdoc",
            ],
            &[BIBTEX, "bibdoc"],
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=latex",
                "bibdoc",
            ],
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=latex",
                "bibdoc",
            ],
        ],
    },
    Case {
        name: "bibtex_builtins",
        oracle: "pdftex",
        inputs: &["bibtest.aux", "bibtest.bst", "bibtest.bib"],
        runs: &[
            &[BIBTEX, "bibtest"],
            &[BIBTEX, "-terse", "-min-crossrefs=1", "bibtest"],
        ],
    },
    // an index: latex, makeindex, latex
    Case {
        name: "makeindex",
        oracle: "pdftex",
        inputs: &["idxdoc.tex"],
        runs: &[
            LATEX_INI,
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=latex",
                "idxdoc",
            ],
            &[MAKEINDEX, "idxdoc"],
            &[
                "-no-shell-escape",
                "-no-parse-first-line",
                "-output-comment=partex",
                "-interaction=nonstopmode",
                "-fmt=latex",
                "idxdoc",
            ],
        ],
    },
    // makeindex's options, a style, and malformed entries
    Case {
        name: "makeindex_styles",
        oracle: "pdftex",
        inputs: &["mkidx.idx", "mkidx.ist"],
        runs: &[
            &[MAKEINDEX, "mkidx"],
            &[
                MAKEINDEX,
                "-s",
                "mkidx.ist",
                "-o",
                "s.ind",
                "-t",
                "s.ilg",
                "mkidx",
            ],
            &[
                MAKEINDEX,
                "-lcr",
                "-o",
                "lcr.ind",
                "-t",
                "lcr.ilg",
                "mkidx.idx",
            ],
            &[MAKEINDEX, "-g", "-o", "g.ind", "-t", "g.ilg", "mkidx.idx"],
            &[
                MAKEINDEX, "-q", "-p", "7", "-o", "p.ind", "-t", "p.ilg", "mkidx",
            ],
            &[MAKEINDEX, "-s", "missing.ist", "mkidx"],
        ],
    },
    // e-TeX's and web2c's behaviour pdfTeX and XeTeX share (`\primitive`,
    // `\tracingassigns`, `\tracingstacklevels`, `\vadjust pre`,
    // `\special shipout`, the effective tail, display boxes) and kpathsea's
    // case-insensitive search, in each engine
    Case {
        name: "etex_shared_pdftex",
        oracle: "pdftex",
        inputs: &["etex-shared.tex"],
        runs: &[&["-ini", "-etex", "-interaction=nonstopmode", "etex-shared"]],
    },
    Case {
        name: "etex_shared_xetex",
        oracle: "xetex",
        inputs: &["etex-shared.tex"],
        runs: &[&[
            "-ini",
            "-etex",
            "-interaction=nonstopmode",
            "-no-pdf",
            "etex-shared",
        ]],
    },
    // XeTeX: Unicode input, native fonts, inter-character tokens,
    // interword space shaping, the font queries, XDV output
    Case {
        name: "xetex_plain",
        oracle: "xetex",
        inputs: &["xetex-plain.tex"],
        runs: &[
            &[
                "-ini",
                "-etex",
                "-interaction=nonstopmode",
                "-output-comment=partex",
                "-no-pdf",
                "xetex-plain",
            ],
            &[
                "-ini",
                "-etex",
                "-interaction=nonstopmode",
                "-jobname=xetex-plain-pdf",
                "xetex-plain",
            ],
        ],
    },
    // xelatex, its format made here: fontspec, hyphenation of native
    // words, hyperref, polyglossia's right-to-left scripts, beamer and
    // TikZ through xdvipdfmx, graphicx's pictures (PNG, JPEG, BMP, PDF
    // pages), unicode-math with OpenType math fonts (one, then several by
    // range, boxes shown); each run twice (the second reads the aux)
    Case {
        name: "xelatex",
        oracle: "xetex",
        inputs: &[
            "xelatex-fonts.tex",
            "xelatex-lang.tex",
            "xelatex-beamer.tex",
            "xelatex-pics.tex",
            "xpic-plain.png",
            "xpic-dpi300.png",
            "xpic-jfif200.jpg",
            "xpic-exif300.jpg",
            "xpic-big24.bmp",
            "xpic-rle8.bmp",
            "xpic-inherit.pdf",
            "xpic-three.pdf",
            "xelatex-math.tex",
            "xelatex-math-body.tex",
            "xelatex-math-mix.tex",
        ],
        runs: &[
            &[
                "-ini",
                "-etex",
                "-interaction=nonstopmode",
                "-jobname=xelatex",
                "xelatex.ini",
            ],
            &["-fmt=xelatex", "-interaction=nonstopmode", "xelatex-fonts"],
            &["-fmt=xelatex", "-interaction=nonstopmode", "xelatex-fonts"],
            &["-fmt=xelatex", "-interaction=nonstopmode", "xelatex-lang"],
            &["-fmt=xelatex", "-interaction=nonstopmode", "xelatex-beamer"],
            &["-fmt=xelatex", "-interaction=nonstopmode", "xelatex-pics"],
            &["-fmt=xelatex", "-interaction=nonstopmode", "xelatex-pics"],
            &["-fmt=xelatex", "-interaction=nonstopmode", "xelatex-math"],
            &["-fmt=xelatex", "-interaction=nonstopmode", "xelatex-math"],
            &[
                "-fmt=xelatex",
                "-interaction=nonstopmode",
                "xelatex-math-mix",
            ],
            &[
                "-fmt=xelatex",
                "-interaction=nonstopmode",
                "-no-pdf",
                "-jobname=xelatex-fonts-xdv",
                "xelatex-fonts",
            ],
        ],
    },
];

/// A job built by `partex -watch`, with edits `(file, marker, replacement)`
/// applied one at a time (an empty marker: no edit, rebuild for the files
/// the job wrote itself).
struct Incremental {
    name: &'static str,
    inputs: &'static [&'static str],
    job: &'static str,
    edits: &'static [(&'static str, &'static str, &'static str)],
    /// How many rebuilds must stop early (early cutoff, at least).
    cutoffs: usize,
    /// How many rebuilds must run nothing, every changed line reading as
    /// the same tokens (DESIGN.md §7.2), at least (unless
    /// `PARTEX_TOKEN_DEPS=0`).
    invisible: usize,
    /// Built by pdfTeX, writing a PDF file (else Knuth's TeX, a DVI file).
    pdf: bool,
    by: By,
}

impl Incremental {
    /// The oracle's program, and partex's `--compat` argument.
    fn engines(&self) -> (&'static str, &'static str) {
        if self.pdf {
            ("pdftex", "--compat=pdftex")
        } else {
            ("tex", "--compat=tex")
        }
    }
}

/// How an incremental case is built.
#[derive(Clone, Copy, PartialEq, Eq)]
enum By {
    /// One `partex -watch` process.
    Watch,
    /// `partex -resident` invocations (a session in the background; the
    /// terminal output is compared too).
    Resident,
    /// `partex -converge` processes, each rebuilding from the session the
    /// last one saved (DESIGN.md §5.3); the oracle runs to its fixpoint.
    Persisted,
}

const INCREMENTAL: &[Incremental] = &[
    // Files read verbatim (under other category codes) or by \read: their
    // edits rebuild, spaces included.
    Incremental {
        name: "verbatim",
        inputs: &["verbatim.tex", "verbatim-part.tex", "verbatim-read.tex"],
        job: "verbatim",
        edits: &[
            ("verbatim-part.tex", "x = 1", "x = 2"),
            ("verbatim-part.tex", "spaces  too", "spaces too"),
            ("verbatim-read.tex", "Second line", "Second  line"),
            ("verbatim-read.tex", "First line", "The first line"),
        ],
        cutoffs: 0,
        invisible: 0,
        pdf: false,
        by: By::Watch,
    },
    // Backdating compares effects as well as the state: a file only looked
    // up changes what is written, but no state. `\pdffilemoddate` is a
    // lookup too.
    Incremental {
        name: "effects",
        inputs: &["effects.tex", "effects-part.tex"],
        job: "effects",
        edits: &[
            ("effects-part.tex", "version one", "version two"),
            ("effects-part.tex", "version two", "version three, longer"),
            // only touched: its date changes
            ("effects-part.tex", "version three", "version three"),
        ],
        cutoffs: 0,
        invisible: 0,
        pdf: true,
        by: By::Watch,
    },
    // A job that reads back a file it writes (LaTeX's .aux file): a label
    // edit changes the file, yet the rebuild stops early, joining the
    // previous build's pages up to where the file is read back; the pass
    // that reads the file again joins them up to where the label is read,
    // a label renamed too (DESIGN.md §7.4).
    Incremental {
        name: "readback",
        inputs: &["readback.tex"],
        job: "readback",
        edits: &[
            ("readback.tex", "{a}{one}", "{a}{three}"),
            ("readback.tex", "{b}{two}", "{b}{tow}"),
            // another name: a control sequence of another name is made
            ("readback.tex", "{c}{one}", "{cc}{one}"),
        ],
        cutoffs: 3,
        invisible: 0,
        pdf: true,
        by: By::Watch,
    },
    // Early cutoff with a PDF file: the rebuild goes on from the previous
    // build's last checkpoint, its byte positions moved. Pages are short
    // and the page objects wait in an object stream, so a later checkpoint
    // has written less than the meeting point's offset (a PDF whose
    // cross-reference table pointed before every later object, once).
    Incremental {
        name: "cutoff_pdf",
        inputs: &["cutoff-pdf.tex"],
        job: "cutoff-pdf",
        edits: &[
            ("cutoff-pdf.tex", "% edit-2", "Page age."),
            ("cutoff-pdf.tex", "% edit-1", "age"),
            ("cutoff-pdf.tex", "Page age.", "% edit-2"),
            ("cutoff-pdf.tex", "age\n", "Page\n"),
        ],
        cutoffs: 3,
        invisible: 0,
        pdf: true,
        by: By::Watch,
    },
    Incremental {
        name: "incremental",
        inputs: &["incr.tex", "incr-part.tex"],
        job: "incr",
        edits: &[
            // the table of contents settles
            ("incr.tex", "", ""),
            ("incr.tex", "", ""),
            // near the end, then the middle, an included file, the start
            ("incr.tex", "% edit-3", "Closing words, edited."),
            ("incr.tex", "% edit-2", r"A middle paragraph appears.\par"),
            ("incr-part.tex", "% edit-4", r"\words\words\words\par"),
            (
                "incr.tex",
                "% edit-1",
                r"An early edit moves every page after it.\par\words\words\par",
            ),
            // and back to a text seen before
            ("incr.tex", "Closing words, edited.", "% edit-3"),
        ],
        cutoffs: 0,
        invisible: 0,
        pdf: false,
        by: By::Watch,
    },
    Incremental {
        name: "cutoff",
        inputs: &["cutoff.tex"],
        job: "cutoff",
        edits: &[
            // a page grows (the DVI file after it moves), then one later on
            (
                "cutoff.tex",
                "% edit-1",
                "Words inserted early on, and more.",
            ),
            ("cutoff.tex", "% edit-2", "Middle."),
            // back, against the checkpoints kept from before
            (
                "cutoff.tex",
                "Words inserted early on, and more.",
                "% edit-1",
            ),
            ("cutoff.tex", "% edit-3", r"\count1=100 "),
            ("cutoff.tex", "Middle.", "% edit-2"),
        ],
        cutoffs: 4,
        invisible: 0,
        pdf: false,
        by: By::Watch,
    },
    Incremental {
        name: "tokens",
        inputs: &["tokens.tex", "tokens-part.tex"],
        job: "tokens",
        edits: &[
            // the same tokens: spaces, a comment, spaces in an included file
            ("tokens.tex", "Alpha beta", "Alpha   beta"),
            ("tokens.tex", "% note-1", "% a longer note"),
            ("tokens-part.tex", "Included   text", "Included text"),
            // not: under \obeyspaces, in a line that changes category
            // codes, in a line an error shows
            ("tokens.tex", "keep  their", "keep their"),
            ("tokens.tex", "Text with   spaces", "Text with spaces"),
            ("tokens.tex", "here  too", "here too"),
            // the same tokens again, in a line \write writes
            ("tokens.tex", "A   written line", "A written line"),
            // other text
            ("tokens.tex", "Delta epsilon.", "Delta zeta."),
            ("tokens.tex", "Alpha   beta", "Alpha beta"),
            // a line more, then spaces after it
            ("tokens-part.tex", "% part-edit", "A new line.\n% part-edit"),
            ("tokens-part.tex", "Included text", "Included  text"),
            ("tokens.tex", "Alpha beta", "Alpha  beta"),
            // a line \read reads
            ("tokens-part.tex", "first  line", "first line"),
        ],
        cutoffs: 0,
        invisible: 7,
        pdf: false,
        by: By::Watch,
    },
    Incremental {
        name: "resident",
        inputs: &["cutoff.tex"],
        job: "cutoff",
        edits: &[
            ("cutoff.tex", "", ""),
            (
                "cutoff.tex",
                "% edit-1",
                "Words inserted early on, and more.",
            ),
            ("cutoff.tex", "% edit-2", "Middle."),
            ("cutoff.tex", "% edit-3", r"\count1=100 "),
        ],
        cutoffs: 2,
        invisible: 0,
        pdf: false,
        by: By::Resident,
    },
    Incremental {
        name: "persisted",
        inputs: &["incr.tex", "incr-part.tex"],
        job: "incr",
        edits: &[
            ("incr.tex", "", ""),
            ("incr.tex", "% edit-3", "Closing words, edited."),
            ("incr.tex", "% edit-2", r"A middle paragraph appears.\par"),
            ("incr-part.tex", "% edit-4", r"\words\words\words\par"),
            (
                "incr.tex",
                "% edit-1",
                r"An early edit moves every page after it.\par\words\words\par",
            ),
            ("incr.tex", "Closing words, edited.", "% edit-3"),
        ],
        cutoffs: 0,
        invisible: 0,
        pdf: false,
        by: By::Persisted,
    },
];

/// A case's directories, made afresh with its inputs: the oracle's and
/// partex's.
fn case_dirs(root: &Path, name: &str, inputs: &[&str]) -> Result<(PathBuf, PathBuf)> {
    let work = out_root(root).join(name);
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let (o, p) = (work.join("o"), work.join("p"));
    fs::create_dir_all(&o)?;
    fs::create_dir_all(&p)?;
    for input in inputs {
        let src = root.join("tests/e2e").join(input);
        for dir in [&o, &p] {
            fs::copy(&src, dir.join(input))?;
            stamp(&dir.join(input), 0)?;
        }
    }
    Ok((o, p))
}

/// Give `f` the fixed modification time `at` seconds after the cases'
/// epoch: `\pdffilemoddate` must say the same on both sides.
fn stamp(f: &Path, at: u64) -> Result<()> {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_758_800_000 + at);
    fs::File::options().write(true).open(f)?.set_modified(t)?;
    Ok(())
}

/// Replace `marker` by `new` in `file` of the oracle's directory `o` and
/// partex's `p` (the same text again touches it), changed at `at`.
fn edit_file(o: &Path, p: &Path, file: &str, marker: &str, new: &str, at: u64) -> Result<()> {
    for dir in [o, p] {
        let f = dir.join(file);
        let text = fs::read_to_string(&f)?;
        ensure!(text.contains(marker), "{file} has no `{marker}`");
        // (as editors save: a watcher never sees half a file)
        let tmp = dir.join(".edit.tmp");
        fs::write(&tmp, text.replacen(marker, new, 1))?;
        stamp(&tmp, at)?;
        fs::rename(&tmp, &f)?;
    }
    Ok(())
}

/// Run an incremental case; the names of the files that differ, by build.
fn run_incremental(root: &Path, partex: &Path, case: &Incremental) -> Result<Vec<String>> {
    if case.by != By::Watch {
        return run_resident(root, partex, case);
    }
    let (o, p) = case_dirs(root, case.name, case.inputs)?;
    let ini = ["-ini", "-interaction=nonstopmode", r"\input plain \dump"];
    let (oracle, compat) = case.engines();
    let partex_as = || {
        let mut cmd = Command::new(partex);
        cmd.arg(compat);
        cmd
    };
    exec(Command::new(oracle), &o, &ini, "term0.txt")?;
    exec(partex_as(), &p, &ini, "term0.txt")?;
    let args = [
        "-output-comment=partex",
        "-interaction=nonstopmode",
        "&plain",
        case.job,
    ];
    let mut child = partex_as()
        .current_dir(&p)
        // (the oracle's fixed time: a PDF file records when it was made)
        .env("SOURCE_DATE_EPOCH", "1758800000")
        .env("FORCE_SOURCE_DATE", "1")
        .args(["-watch", "-checkpoint-every=4"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("piped");
    let mut lines = BufReader::new(child.stderr.take().expect("piped")).lines();
    // Commands run by rebuilds and by the whole job, per rebuild.
    let mut runs: Vec<(u64, u64)> = Vec::new();
    let (mut cutoffs, mut invisible) = (0, 0);
    let mut wait_for = |what: &str| -> Result<()> {
        for line in lines.by_ref() {
            let line = line?;
            eprintln!("    {line}");
            invisible += usize::from(line.contains("read as the same tokens"));
            if let Some(rest) = line.strip_prefix("partex: rebuilt in ") {
                // "X ms: A of B commands run, ..."
                let mut w = rest.split(' ').skip(2);
                let ran = w.next().and_then(|n| n.parse().ok());
                let all = w.nth(1).and_then(|n| n.parse().ok());
                runs.extend(ran.zip(all));
                cutoffs += usize::from(line.contains("converged at"));
            }
            if line.starts_with(what) {
                return Ok(());
            }
        }
        anyhow::bail!("partex -watch stopped before `{what}`")
    };
    let mut diffs = Vec::new();
    // (-watch runs every build to its fixpoint, as -converge does)
    wait_for("partex: built")?;
    fixpoint(oracle, &o, &args, true)?;
    diffs.extend(
        compare(&o, &p, true)?
            .into_iter()
            .map(|n| format!("build: {n}")),
    );
    for (i, &(file, marker, new)) in case.edits.iter().enumerate() {
        if !marker.is_empty() {
            edit_file(&o, &p, file, marker, new, (i as u64 + 1) * 1000)?;
        }
        writeln!(stdin, "rebuild {i}")?;
        wait_for(&format!("partex: done rebuild {i}"))?;
        fixpoint(oracle, &o, &args, true)?;
        diffs.extend(
            compare(&o, &p, true)?
                .into_iter()
                .map(|n| format!("edit {}: {n}", i + 1)),
        );
    }
    writeln!(stdin, "q")?;
    child.wait()?;
    drop(wait_for);
    // (else a bug could make every rebuild start over, unnoticed)
    if !runs.iter().any(|&(a, b)| a < b) {
        diffs.push(format!("no rebuild resumed from a checkpoint: {runs:?}"));
    }
    if cutoffs < case.cutoffs {
        diffs.push(format!(
            "{cutoffs} rebuilds stopped early, not {}",
            case.cutoffs
        ));
    }
    let token_deps = std::env::var_os("PARTEX_TOKEN_DEPS").is_none_or(|v| v != "0");
    if token_deps && invisible < case.invisible {
        diffs.push(format!(
            "{invisible} rebuilds ran nothing (the same tokens), not {}",
            case.invisible
        ));
    }
    Ok(diffs)
}

/// A `-converge` case: `partex -converge` once, against the oracle run
/// until the job's own files stop changing, with BibTeX between runs
/// when the `.aux` file's citation lines change (as latexmk runs it).
struct Converge {
    name: &'static str,
    oracle: &'static str,
    inputs: &'static [&'static str],
    ini: &'static [&'static str],
    args: &'static [&'static str],
    job: &'static str,
    /// The job's files that decide another run.
    watch: &'static [&'static str],
    /// The fewest oracle runs the case must need.
    passes: usize,
    /// With `-output-directory=out`: the job's files there, and BibTeX and
    /// makeindex run in it (as latexmk runs them).
    outdir: bool,
}

const LATEX_INI: &[&str] = &[
    "-no-shell-escape",
    "-no-parse-first-line",
    "-output-comment=partex",
    "-interaction=nonstopmode",
    "-ini",
    "-etex",
    "-jobname=latex",
    "*latex.ini",
];

const CONVERGE: &[Converge] = &[
    // `incr.tex` reads the table of contents it writes
    Converge {
        name: "converge",
        oracle: "tex",
        inputs: &["incr.tex", "incr-part.tex"],
        ini: &["-ini", "-interaction=nonstopmode", r"\input plain \dump"],
        args: &[
            "-output-comment=partex",
            "-interaction=nonstopmode",
            "&plain",
        ],
        job: "incr",
        watch: &["incr.toc"],
        passes: 2,
        outdir: false,
    },
    // citations: LaTeX, BibTeX, LaTeX, LaTeX
    Converge {
        name: "converge_bibtex",
        oracle: "pdftex",
        inputs: &["bibdoc.tex"],
        ini: LATEX_INI,
        args: &[
            "-no-shell-escape",
            "-no-parse-first-line",
            "-output-comment=partex",
            "-interaction=nonstopmode",
            "-fmt=latex",
        ],
        job: "bibdoc",
        watch: &["bibdoc.aux", "bibdoc.bbl"],
        passes: 3,
        outdir: false,
    },
    // an index: LaTeX, makeindex, LaTeX
    Converge {
        name: "converge_makeindex",
        oracle: "pdftex",
        inputs: &["idxdoc.tex"],
        ini: LATEX_INI,
        args: &[
            "-no-shell-escape",
            "-no-parse-first-line",
            "-output-comment=partex",
            "-interaction=nonstopmode",
            "-fmt=latex",
        ],
        job: "idxdoc",
        watch: &["idxdoc.aux", "idxdoc.idx", "idxdoc.ind"],
        passes: 2,
        outdir: false,
    },
    // citations with an output directory (BibTeX runs in it)
    Converge {
        name: "converge_bibtex_outdir",
        oracle: "pdftex",
        inputs: &["bibdoc.tex"],
        ini: LATEX_INI,
        args: &[
            "-no-shell-escape",
            "-no-parse-first-line",
            "-output-comment=partex",
            "-interaction=nonstopmode",
            "-fmt=latex",
            "-output-directory=out",
        ],
        job: "bibdoc",
        watch: &["bibdoc.aux", "bibdoc.bbl"],
        passes: 3,
        outdir: true,
    },
    // an index with an output directory
    Converge {
        name: "converge_makeindex_outdir",
        oracle: "pdftex",
        inputs: &["idxdoc.tex"],
        ini: LATEX_INI,
        args: &[
            "-no-shell-escape",
            "-no-parse-first-line",
            "-output-comment=partex",
            "-interaction=nonstopmode",
            "-fmt=latex",
            "-output-directory=out",
        ],
        job: "idxdoc",
        watch: &["idxdoc.aux", "idxdoc.idx", "idxdoc.ind"],
        passes: 2,
        outdir: true,
    },
];

/// What BibTeX reads in an `.aux` file.
fn citation_lines(aux: &[u8]) -> Vec<u8> {
    aux.split(|&c| c == b'\n')
        .filter(|l| {
            [
                &b"\\citation{"[..],
                b"\\bibdata{",
                b"\\bibstyle{",
                b"\\@input{",
            ]
            .iter()
            .any(|p| l.starts_with(p))
        })
        .flat_map(|l| l.iter().chain(b"\n"))
        .copied()
        .collect()
}

/// Run the oracle in `o` on `args` as latexmk would, BibTeX and makeindex
/// in `out` when their inputs change, until the watched files settle: the
/// runs of each.
fn oracle_passes(
    case: &Converge,
    o: &Path,
    out: &Path,
    args: &[&str],
) -> Result<(usize, usize, usize)> {
    let aux = out.join(format!("{}.aux", case.job));
    let idx = out.join(format!("{}.idx", case.job));
    let (mut passes, mut bibtex_runs, mut cited) = (0, 0, None);
    let (mut makeindex_runs, mut indexed) = (0, None);
    loop {
        let snapshot = || {
            case.watch
                .iter()
                .map(|f| fs::read(out.join(f)).ok())
                .collect::<Vec<_>>()
        };
        let before = snapshot();
        exec(Command::new(case.oracle), o, args, "term.txt")?;
        passes += 1;
        if let Ok(text) = fs::read(&aux)
            && text.windows(9).any(|w| w == b"\\bibdata{")
            && cited.as_ref() != Some(&citation_lines(&text))
        {
            // (latexmk's BIBINPUTS for an output directory: the sources first)
            let mut bibtex = Command::new("bibtex");
            bibtex.env("BIBINPUTS", format!("{}:", o.display()));
            exec(bibtex, out, &[case.job], "bibterm.txt")?;
            bibtex_runs += 1;
            cited = Some(citation_lines(&text));
        }
        if let Ok(text) = fs::read(&idx)
            && indexed.as_ref() != Some(&text)
        {
            exec(Command::new("makeindex"), out, &[case.job], "mkterm.txt")?;
            makeindex_runs += 1;
            indexed = Some(text);
        }
        if snapshot() == before || passes == 5 {
            break;
        }
    }
    Ok((passes, bibtex_runs, makeindex_runs))
}

/// Run a [`Converge`] case.
fn run_converge(root: &Path, partex: &Path, case: &Converge) -> Result<Vec<String>> {
    let work = out_root(root).join(case.name);
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let (o, p) = (work.join("o"), work.join("p"));
    fs::create_dir_all(&o)?;
    fs::create_dir_all(&p)?;
    for input in case.inputs {
        let src = root.join("tests/e2e").join(input);
        fs::copy(&src, o.join(input))?;
        fs::copy(&src, p.join(input))?;
    }
    exec(Command::new(case.oracle), &o, case.ini, "term0.txt")?;
    let engine = format!("-engine={}", case.oracle);
    let mut cmd = tex_compat(partex);
    cmd.arg(&engine);
    exec(cmd, &p, case.ini, "term0.txt")?;
    fs::remove_file(o.join("term0.txt"))?;
    fs::remove_file(p.join("term0.txt"))?;
    let mut args = case.args.to_vec();
    args.push(case.job);
    // where the job's files go
    let out = if case.outdir {
        fs::create_dir_all(o.join("out"))?;
        fs::create_dir_all(p.join("out"))?;
        o.join("out")
    } else {
        o.clone()
    };
    let (passes, bibtex_runs, makeindex_runs) = oracle_passes(case, &o, &out, &args)?;
    let _ = fs::remove_file(out.join("bibterm.txt"));
    let _ = fs::remove_file(out.join("mkterm.txt"));
    let mut converge = args.clone();
    converge.insert(0, "-converge");
    let cache = work.join("cache");
    let run = || -> Result<()> {
        let mut cmd = tex_compat(partex);
        cmd.arg(&engine)
            .env("PARTEX_REPORT", "1")
            .env("PARTEX_CACHE_DIR", &cache)
            .env_remove("PARTEX_PERSIST");
        for line in exec(cmd, &p, &converge, "term.txt")?.lines() {
            eprintln!("    {line}");
        }
        Ok(())
    };
    run()?;
    eprintln!(
        "    ({}: {passes} runs, bibtex: {bibtex_runs}, makeindex: {makeindex_runs})",
        case.oracle
    );
    ensure!(
        passes >= case.passes,
        "the case must need {} runs",
        case.passes
    );
    let compare_all = || -> Result<Vec<String>> {
        let mut diffs = compare(&o, &p, false)?;
        if case.outdir {
            diffs.extend(
                compare(&o.join("out"), &p.join("out"), false)?
                    .into_iter()
                    .map(|n| format!("out/{n}")),
            );
        }
        Ok(diffs)
    };
    let mut diffs = compare_all()?;
    // Again, from the session it saved (DESIGN.md §5.3): nothing changed.
    run()?;
    diffs.extend(
        compare_all()?
            .into_iter()
            .map(|n| format!("from the saved session: {n}")),
    );
    Ok(diffs)
}

/// The modern command line: `partex build` of `modern.tex` against
/// `pdflatex` run to its fixpoint as latexmk would (BibTeX between runs).
/// Glyph origins (DESIGN 4.4): `glyphs.tex` run by pdfTeX and by partex
/// with `PARTEX_ORIGINS=1`, every file the same but partex's side file,
/// which `crate::origins` checks against the PDF and the text; then SSA
/// mode's rebuilds ([`glyphs_ssa`]). Plain and SSA mode's: in machine
/// mode's e2e run, nothing.
fn run_glyphs(root: &Path, partex: &Path) -> Result<Vec<String>> {
    if std::env::var_os("PARTEX_MACHINE").is_some() {
        return Ok(Vec::new());
    }
    let work = out_root(root).join("glyphs");
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let (oracle, plain) = (work.join("o"), work.join("p"));
    for d in [&oracle, &plain] {
        fs::create_dir_all(d)?;
        for i in GLYPHS_INPUTS {
            fs::copy(root.join("tests/e2e").join(i), d.join(i))?;
        }
    }
    let ini: Vec<&str> = GLYPHS_FLAGS
        .iter()
        .copied()
        .chain(["-ini", "-etex", "-jobname=pdflatex", "*pdflatex.ini"])
        .collect();
    exec(Command::new("pdftex"), &oracle, &ini, "term1.txt")?;
    exec(glyphs_partex(partex, &work), &plain, &ini, "term1.txt")?;
    for i in 2..=3 {
        let term = format!("term{i}.txt");
        exec(Command::new("pdftex"), &oracle, &GLYPHS_DOC, &term)?;
        let mut cmd = glyphs_partex(partex, &work);
        cmd.env("PARTEX_ORIGINS", "1").env("PARTEX_DISPLAY", "1");
        exec(cmd, &plain, &GLYPHS_DOC, &term)?;
    }
    let mut diffs: Vec<String> = compare(&oracle, &plain, false)?
        .into_iter()
        .filter(|n| !is_side_file(n))
        .collect();
    glyphs_check(&plain, "origins", &mut diffs)?;
    display_check(&plain, "glyphs", "display", &mut diffs)?;
    glyphs_outline(partex, &plain, &mut diffs)?;
    glyphs_ssa(root, partex, &work, &mut diffs)?;
    Ok(diffs)
}

/// xdvipdfmx's glyph runs (`PARTEX_GLYPH_RUNS=1`) of
/// `xelatex-runs.tex` (with the xelatex format made here), origins on:
/// a run per origin and per code the PDF shows, where the PDF puts it,
/// with its text as the PDF's `ToUnicode` and `/ActualText` give it
/// (`crate::glyphruns`): native text, Type 1 fonts with built-in and
/// `.enc` encodings, a virtual font, classic math, TrueType and OpenType
/// map entries, leaders, colours, rotated text, right-to-left scripts.
fn run_xelatex_runs(root: &Path, partex: &Path) -> Result<Vec<String>> {
    let work = out_root(root).join("xelatex_runs");
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let p = work.join("p");
    fs::create_dir_all(&p)?;
    fs::copy(
        root.join("tests/e2e/xelatex-runs.tex"),
        p.join("xelatex-runs.tex"),
    )?;
    let fonts = root.join("scripts/xetex/fonts.conf");
    let cmd = || {
        let mut cmd = tex_compat(partex);
        cmd.arg("-engine=xetex")
            .env("FONTCONFIG_FILE", &fonts)
            .env("PARTEX_XETEX", "1")
            .env_remove("PARTEX_MACHINE")
            .env_remove("PARTEX_SSA")
            .env_remove("PARTEX_PERSIST");
        cmd
    };
    let ini = [
        "-ini",
        "-etex",
        "-interaction=nonstopmode",
        "-jobname=xelatex",
        "xelatex.ini",
    ];
    exec(cmd(), &p, &ini, "term1.txt")?;
    let mut run = cmd();
    run.env("PARTEX_ORIGINS", "1").env("PARTEX_GLYPH_RUNS", "1");
    exec(
        run,
        &p,
        &["-fmt=xelatex", "-interaction=nonstopmode", "xelatex-runs"],
        "term2.txt",
    )?;
    let mut diffs: Vec<String> = crate::glyphruns::check(&p, "xelatex-runs")?
        .into_iter()
        .map(|m| format!("glyph runs: {m}"))
        .collect();
    diffs.extend(
        crate::glyphruns::expect_xelatex_runs(&p)?
            .into_iter()
            .map(|m| format!("glyph runs: {m}")),
    );
    Ok(diffs)
}

/// `partex outline glyphs.tex --json` beside the plain run: its heading
/// placed from the side file, where pdfTeX's content stream puts its
/// title's first glyph (`I` of `Intro`, after the number and its kern).
fn glyphs_outline(partex: &Path, dir: &Path, diffs: &mut Vec<String>) -> Result<()> {
    let out = Command::new(partex)
        .current_dir(dir)
        .args(["outline", "glyphs.tex", "--json"])
        .env_remove("PARTEX_MACHINE")
        .output()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .with_context(|| String::from_utf8_lossy(&out.stderr).into_owned())?;
    let h = &v["outline"][0];
    let near = |k: &str, want: f64| h[k].as_f64().is_some_and(|x| (x - want).abs() < 0.01);
    let ok = h["title"] == "Intro"
        && h["number"] == "1"
        && h["page"] == 0
        && near("x", 157.977)
        && near("y", 657.235);
    if !ok {
        diffs.push(format!("outline: {h}"));
    }
    Ok(())
}

/// The `glyphs` job's inputs, and its runs' flags.
const GLYPHS_INPUTS: [&str; 2] = ["glyphs.tex", "images-fig.pdf"];
const GLYPHS_FLAGS: [&str; 4] = [
    "-no-shell-escape",
    "-no-parse-first-line",
    "-output-comment=partex",
    "-interaction=nonstopmode",
];
const GLYPHS_DOC: [&str; 6] = [
    "-no-shell-escape",
    "-no-parse-first-line",
    "-output-comment=partex",
    "-interaction=nonstopmode",
    "-fmt=pdflatex",
    "glyphs",
];

/// partex for the `glyphs` job, in `work`'s cache.
fn glyphs_partex(partex: &Path, work: &Path) -> Command {
    let mut cmd = tex_compat(partex);
    cmd.arg("-engine=pdftex")
        .env("PARTEX_CACHE_DIR", work.join("cache"))
        .env_remove("PARTEX_MACHINE")
        .env_remove("PARTEX_SSA")
        .env_remove("PARTEX_PERSIST");
    cmd
}

/// `crate::origins::check` of `glyphs.tex`'s run in `dir`, its problems
/// added to `diffs` (as `what`'s).
fn glyphs_check(dir: &Path, what: &str, diffs: &mut Vec<String>) -> Result<()> {
    for m in crate::origins::check(dir, "glyphs", crate::origins::GLYPHS)? {
        diffs.push(format!("{what}: {m}"));
    }
    Ok(())
}

/// The `glyphs` job in SSA mode (with the format the plain run made): the
/// text edited twice, rebuilt after each edit (a comment line inserted
/// before a paragraph: nothing typeset changes, the steps after are kept,
/// their origins moved; then a paragraph inserted there), each rebuild's
/// side file and PDF against a cold SSA build's of its text, and checked.
fn glyphs_ssa(root: &Path, partex: &Path, work: &Path, diffs: &mut Vec<String>) -> Result<()> {
    let text = fs::read_to_string(root.join("tests/e2e/glyphs.tex"))?;
    let edit1 = text.replacen("Hello world", "% A comment line.\nHello world", 1);
    let edit2 = edit1.replacen(
        "Hello world",
        "A paragraph inserted before.\n\nHello world",
        1,
    );
    let ssa = work.join("s");
    let cold = [work.join("c1"), work.join("c2")];
    for (d, edited) in [
        (&ssa, None),
        (&cold[0], Some(&edit1)),
        (&cold[1], Some(&edit2)),
    ] {
        fs::create_dir_all(d)?;
        for i in GLYPHS_INPUTS {
            fs::copy(root.join("tests/e2e").join(i), d.join(i))?;
        }
        if let Some(t) = edited {
            fs::write(d.join("glyphs.tex"), t)?;
        }
        fs::copy(work.join("p/pdflatex.fmt"), d.join("pdflatex.fmt"))?;
    }
    fs::write(ssa.join("glyphs-1.tex"), &edit1)?;
    fs::write(ssa.join("glyphs-2.tex"), &edit2)?;
    for d in [&ssa, &cold[0], &cold[1]] {
        let mut cmd = glyphs_partex(partex, work);
        cmd.env("PARTEX_SSA", "1")
            .env("PARTEX_ORIGINS", "1")
            .env("PARTEX_DISPLAY", "1");
        if d == &ssa {
            // (each line a rebuild after it; the side files each wrote kept)
            cmd.env(
                "PARTEX_SSA_REBUILD",
                "cp glyphs-1.tex glyphs.tex\n\
                 cp glyphs.origins.jsonl rebuild1.origins.jsonl; \
                 cp glyphs.display.jsonl rebuild1.display.jsonl; \
                 cp glyphs.pdf rebuild1.pdf; cp glyphs-2.tex glyphs.tex",
            );
        }
        let err = exec(cmd, d, &GLYPHS_DOC, "term2.txt")?;
        fs::write(d.join("stderr2.txt"), err)?;
    }
    for (c, rebuilt) in [(&cold[0], "rebuild1"), (&cold[1], "glyphs")] {
        display_check(&ssa, rebuilt, &format!("SSA display ({rebuilt})"), diffs)?;
        for ext in ["origins.jsonl", "display.jsonl", "pdf"] {
            let ours = fs::read(ssa.join(format!("{rebuilt}.{ext}")));
            let theirs = fs::read(c.join(format!("glyphs.{ext}")));
            if !matches!((&ours, &theirs), (Ok(x), Ok(y)) if x == y) {
                diffs.push(format!("SSA {rebuilt}.{ext}"));
            }
        }
    }
    glyphs_check(&cold[0], "SSA origins (edit 1)", diffs)?;
    glyphs_check(&cold[1], "SSA origins (edit 2)", diffs)
}

/// The `synctex` job's inputs, copied into its run directory.
const SYNCTEX_INPUTS: [&str; 5] = [
    "synctex.tex",
    "synctex-sub.tex",
    "synctex-doc.tex",
    "glyphs.tex",
    "images-fig.pdf",
];

/// `SyncTeX` (DESIGN 4.5): pdfTeX and partex in one directory, `run/`
/// (the `Input:` lines are absolute names), each side's outputs moved to
/// `o/` and `p/` and compared (the `.synctex.gz` files byte for byte):
/// `synctex.tex` with `-synctex=1`, `-1` (not gzipped) and `12` (forms'
/// records, `=` positions), `synctex-doc.tex`, which sets `\synctex`
/// itself with no option, and `glyphs.tex` (LaTeX, run twice) with
/// `-synctex=1`; then TeX Live's `synctex view` (every line of each
/// source) and `synctex edit` (a grid of points on each page) asked of
/// both sides' files, their answers compared. Plain mode's only.
fn run_synctex(root: &Path, partex: &Path) -> Result<Vec<String>> {
    if std::env::var_os("PARTEX_MACHINE").is_some() {
        return Ok(Vec::new());
    }
    let work = out_root(root).join("synctex");
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let (run, o, p) = (work.join("run"), work.join("o"), work.join("p"));
    for d in [&run, &o, &p] {
        fs::create_dir_all(d)?;
    }
    for i in SYNCTEX_INPUTS {
        fs::copy(root.join("tests/e2e").join(i), run.join(i))?;
    }
    let partex_cmd = || glyphs_partex(partex, &work);
    // (each side's formats, by names of their own)
    for (side, oracle) in [("o", true), ("p", false)] {
        let cmd = || {
            if oracle {
                Command::new("pdftex")
            } else {
                partex_cmd()
            }
        };
        let plain = format!("-jobname=plain-{side}");
        exec(
            cmd(),
            &run,
            &[
                "-ini",
                "-etex",
                "-interaction=nonstopmode",
                &plain,
                r"\input plain \dump",
            ],
            "term-ini.txt",
        )?;
        let latex = format!("-jobname=pdflatex-{side}");
        let ini: Vec<&str> = GLYPHS_FLAGS
            .iter()
            .copied()
            .chain(["-ini", "-etex", &latex, "*pdflatex.ini"])
            .collect();
        exec(cmd(), &run, &ini, "term-ini.txt")?;
    }
    // (job, its format, the options, the file, runs)
    let jobs: [(&str, &str, &str, &str, usize); 5] = [
        ("s1", "plain", "-synctex=1", "synctex", 1),
        ("s2", "plain", "-synctex=-1", "synctex", 1),
        ("s3", "plain", "-synctex=12", "synctex", 1),
        ("s4", "plain", "", "synctex-doc", 1),
        ("glyphs", "pdflatex", "-synctex=1", "glyphs", 2),
    ];
    for (side, oracle, dest) in [("o", true, &o), ("p", false, &p)] {
        for &(job, fmt, opt, file, n) in &jobs {
            let jobname = format!("-jobname={job}");
            let fmt = format!("-fmt={fmt}-{side}");
            let args: Vec<&str> = GLYPHS_FLAGS
                .iter()
                .copied()
                .chain([opt, jobname.as_str(), fmt.as_str(), file])
                .filter(|a| !a.is_empty())
                .collect();
            for _ in 0..n {
                let cmd = if oracle {
                    Command::new("pdftex")
                } else {
                    partex_cmd()
                };
                exec(cmd, &run, &args, &format!("term-{job}.txt"))?;
            }
            // (the job's outputs to its side's directory)
            for e in fs::read_dir(&run)? {
                let n = e?.file_name().to_string_lossy().into_owned();
                let source = Path::new(&n).extension().is_some_and(|e| e == "tex");
                let output = n.starts_with(&format!("{job}.")) && !source;
                if output || n == format!("term-{job}.txt") {
                    fs::rename(run.join(&n), dest.join(&n))?;
                }
            }
        }
    }
    let mut diffs = compare(&o, &p, false)?;
    for (job, ..) in &jobs[..2] {
        let gz = if *job == "s2" {
            "synctex"
        } else {
            "synctex.gz"
        };
        if !o.join(format!("{job}.{gz}")).exists() {
            diffs.push(format!("{job}.{gz} (not written)"));
        }
    }
    synctex_queries(&work, &jobs.map(|j| (j.0, j.3)), &mut diffs)?;
    Ok(diffs)
}

/// `synctex view` of every line of the job's source files and `synctex
/// edit` of a grid of points on each of its pages, against `o/` and `p/`:
/// the answers must be the same (the directory's name masked). Without
/// TeX Live's `synctex` on the path, nothing (the files were compared).
fn synctex_queries(work: &Path, jobs: &[(&str, &str)], diffs: &mut Vec<String>) -> Result<()> {
    let ask = |args: &[String]| -> Option<String> {
        let out = Command::new("synctex")
            .current_dir(work)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        Some(text.replace("o/", "SIDE/").replace("p/", "SIDE/"))
    };
    if ask(&["help".to_owned()]).is_none() {
        eprintln!("e2e synctex: no `synctex` command; the files were compared byte for byte");
        return Ok(());
    }
    for &(job, file) in jobs {
        let pdf = |side: &str| format!("{side}/{job}.pdf");
        let mut inputs = vec![format!("{file}.tex")];
        if file.starts_with("synctex") {
            inputs.push("synctex-sub.tex".to_owned());
        }
        for input in &inputs {
            let lines = fs::read_to_string(work.join("run").join(input))?
                .lines()
                .count();
            for l in 1..=lines {
                let q = |side: &str| {
                    ask(&[
                        "view".to_owned(),
                        "-i".to_owned(),
                        format!("{l}:0:{input}"),
                        "-o".to_owned(),
                        pdf(side),
                    ])
                };
                if q("o") != q("p") {
                    diffs.push(format!("synctex view {job} {input}:{l}"));
                }
            }
        }
        let pages = fs::read_to_string(work.join("o").join(format!("{job}.log")))
            .ok()
            .and_then(|log| {
                let at = log.find("Output written on")?;
                let rest = &log[at..];
                let open = rest.find('(')?;
                rest[open + 1..].split(' ').next()?.parse::<usize>().ok()
            })
            .unwrap_or(1);
        for page in 1..=pages {
            for x in (0..=600).step_by(60) {
                for y in (0..=840).step_by(60) {
                    let q = |side: &str| {
                        ask(&[
                            "edit".to_owned(),
                            "-o".to_owned(),
                            format!("{page}:{x}:{y}:{}", pdf(side)),
                        ])
                    };
                    if q("o") != q("p") {
                        diffs.push(format!("synctex edit {job} {page}:{x}:{y}"));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Whether file `n` is one of partex's side files (glyph origins,
/// display lists), which pdfTeX does not write.
fn is_side_file(n: &str) -> bool {
    n.ends_with(".origins.jsonl") || n.ends_with(".display.jsonl")
}

/// `crate::display::check` of job `job`'s run in `dir`, its problems
/// added to `diffs` (as `what`'s).
fn display_check(dir: &Path, job: &str, what: &str, diffs: &mut Vec<String>) -> Result<()> {
    for m in crate::display::check(dir, job)?.0 {
        diffs.push(format!("{what}: {m}"));
    }
    Ok(())
}

/// The `display` job's inputs, and its document's runs' flags.
const DISPLAY_INPUTS: [&str; 3] = ["display.tex", "images-fig.pdf", "png-rgb8.png"];
const DISPLAY_DOC: [&str; 6] = [
    "-no-shell-escape",
    "-no-parse-first-line",
    "-output-comment=partex",
    "-interaction=nonstopmode",
    "-fmt=pdflatex",
    "display",
];

/// Display lists (DESIGN 4.6): `display.tex` run by pdfTeX and by partex
/// with `PARTEX_DISPLAY=1` (and `PARTEX_ORIGINS=1`), every file the same
/// but partex's side files, its lists checked against its PDF
/// (`crate::display`: each page's glyphs, items, box); `microtype.tex`
/// (font expansion's text matrices, plain pdfTeX) likewise; then
/// `display.tex` in SSA mode ([`display_ssa`]). In machine mode's e2e
/// run, nothing.
fn run_display(root: &Path, partex: &Path) -> Result<Vec<String>> {
    if std::env::var_os("PARTEX_MACHINE").is_some() {
        return Ok(Vec::new());
    }
    let work = out_root(root).join("display");
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let (oracle, plain) = (work.join("o"), work.join("p"));
    let (moracle, mplain) = (work.join("mo"), work.join("mp"));
    for d in [&oracle, &plain] {
        fs::create_dir_all(d)?;
        for i in DISPLAY_INPUTS {
            fs::copy(root.join("tests/e2e").join(i), d.join(i))?;
        }
    }
    let ini: Vec<&str> = GLYPHS_FLAGS
        .iter()
        .copied()
        .chain(["-ini", "-etex", "-jobname=pdflatex", "*pdflatex.ini"])
        .collect();
    exec(Command::new("pdftex"), &oracle, &ini, "term1.txt")?;
    exec(glyphs_partex(partex, &work), &plain, &ini, "term1.txt")?;
    for i in 2..=3 {
        let term = format!("term{i}.txt");
        exec(Command::new("pdftex"), &oracle, &DISPLAY_DOC, &term)?;
        let mut cmd = glyphs_partex(partex, &work);
        cmd.env("PARTEX_DISPLAY", "1").env("PARTEX_ORIGINS", "1");
        exec(cmd, &plain, &DISPLAY_DOC, &term)?;
    }
    let mut diffs: Vec<String> = compare(&oracle, &plain, false)?
        .into_iter()
        .filter(|n| !is_side_file(n))
        .collect();
    display_check(&plain, "display", "display", &mut diffs)?;
    // (plain pdfTeX with font expansion: `Tm`'s scaled text)
    let micro = [
        "-ini",
        "-no-shell-escape",
        "-no-parse-first-line",
        "-interaction=nonstopmode",
        "microtype",
    ];
    for d in [&moracle, &mplain] {
        fs::create_dir_all(d)?;
        fs::copy(
            root.join("tests/e2e/microtype.tex"),
            d.join("microtype.tex"),
        )?;
    }
    exec(Command::new("pdftex"), &moracle, &micro, "term1.txt")?;
    let mut cmd = glyphs_partex(partex, &work);
    cmd.env("PARTEX_DISPLAY", "1");
    exec(cmd, &mplain, &micro, "term1.txt")?;
    diffs.extend(
        compare(&moracle, &mplain, false)?
            .into_iter()
            .filter(|n| !is_side_file(n))
            .map(|n| format!("microtype: {n}")),
    );
    display_check(&mplain, "microtype", "microtype display", &mut diffs)?;
    display_ssa(root, partex, &work, &mut diffs)?;
    Ok(diffs)
}

/// `display.tex` in SSA mode (with the format the plain run made), its
/// text edited twice and rebuilt after each edit (a comment line before
/// a paragraph: the pages' streams kept; then a paragraph inserted before
/// the figure, moving it); each rebuild's display lists, page hashes and
/// PDF a cold SSA build's of its text, and checked against its PDF.
fn display_ssa(root: &Path, partex: &Path, work: &Path, diffs: &mut Vec<String>) -> Result<()> {
    let text = fs::read_to_string(root.join("tests/e2e/display.tex"))?;
    let edit1 = text.replacen("Plain text,", "% A comment line.\nPlain text,", 1);
    let edit2 = edit1.replacen(
        "\\begin{tikzpicture}",
        "A paragraph inserted before the picture.\n\n\\begin{tikzpicture}",
        1,
    );
    let ssa = work.join("s");
    let cold = [work.join("c1"), work.join("c2")];
    for (d, edited) in [
        (&ssa, None),
        (&cold[0], Some(&edit1)),
        (&cold[1], Some(&edit2)),
    ] {
        fs::create_dir_all(d)?;
        for i in DISPLAY_INPUTS {
            fs::copy(root.join("tests/e2e").join(i), d.join(i))?;
        }
        if let Some(t) = edited {
            fs::write(d.join("display.tex"), t)?;
        }
        fs::copy(work.join("p/pdflatex.fmt"), d.join("pdflatex.fmt"))?;
        // (the auxiliary files of the plain run: the same passes)
        fs::copy(work.join("p/display.aux"), d.join("display.aux"))?;
    }
    fs::write(ssa.join("display-1.tex"), &edit1)?;
    fs::write(ssa.join("display-2.tex"), &edit2)?;
    for d in [&ssa, &cold[0], &cold[1]] {
        let mut cmd = glyphs_partex(partex, work);
        cmd.env("PARTEX_SSA", "1").env("PARTEX_DISPLAY", "1");
        if d == &ssa {
            cmd.env(
                "PARTEX_SSA_REBUILD",
                "cp display-1.tex display.tex\n\
                 cp display.display.jsonl rebuild1.display.jsonl; \
                 cp display.pdf rebuild1.pdf; cp display-2.tex display.tex",
            );
        }
        let err = exec(cmd, d, &DISPLAY_DOC, "term2.txt")?;
        fs::write(d.join("stderr2.txt"), err)?;
    }
    for (c, rebuilt) in [(&cold[0], "rebuild1"), (&cold[1], "display")] {
        display_check(&ssa, rebuilt, &format!("SSA display ({rebuilt})"), diffs)?;
        for ext in ["display.jsonl", "pdf"] {
            let ours = fs::read(ssa.join(format!("{rebuilt}.{ext}")));
            let theirs = fs::read(c.join(format!("display.{ext}")));
            if !matches!((&ours, &theirs), (Ok(x), Ok(y)) if x == y) {
                diffs.push(format!("SSA {rebuilt}.{ext}"));
            }
        }
    }
    Ok(())
}

/// Every file must be identical (not the terminal: the renderer's), and
/// the renderer must name each kind of problem the document has; then a
/// rebuild from the saved session (`-v`), `partex why` and `partex clean`.
fn run_modern(root: &Path, partex: &Path) -> Result<Vec<String>> {
    let case = Converge {
        name: "modern",
        oracle: "pdflatex",
        inputs: &["modern.tex"],
        ini: &[],
        args: &["-interaction=nonstopmode"],
        job: "modern",
        watch: &["modern.aux", "modern.bbl"],
        passes: 3,
        outdir: false,
    };
    let work = out_root(root).join(case.name);
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let (o, p) = (work.join("o"), work.join("p"));
    fs::create_dir_all(&o)?;
    fs::create_dir_all(&p)?;
    for input in case.inputs {
        let src = root.join("tests/e2e").join(input);
        fs::copy(&src, o.join(input))?;
        fs::copy(&src, p.join(input))?;
    }
    // (as `partex build modern.tex` gives it to TeX)
    let args = ["-interaction=nonstopmode", "modern.tex"];
    let (passes, bibtex_runs, _) = oracle_passes(&case, &o, &o, &args)?;
    let _ = fs::remove_file(o.join("bibterm.txt"));
    // (its formats outlive the case: making pdflatex's takes a while)
    let formats = root.join("target/e2e-formats");
    let modern = |args: &[&str]| -> Result<(String, String)> {
        let mut cmd = Command::new(partex);
        cmd.env("PARTEX_CACHE_DIR", work.join("cache"))
            .env("PARTEX_FORMATS", &formats)
            .env("NO_COLOR", "1")
            .env_remove("PARTEX_PERSIST");
        let err = exec(cmd, &p, args, "term.txt")?;
        let out = fs::read_to_string(p.join("term.txt"))?;
        Ok((out, err))
    };
    // (the build has an error: `--copy-pdf` must not copy its PDF)
    let (out, report) = modern(&["build", "--copy-pdf=copied", "modern.tex"])?;
    ensure!(
        !p.join("copied").exists(),
        "--copy-pdf copied the PDF of a failed build"
    );
    for line in report.lines() {
        eprintln!("    {line}");
    }
    eprintln!("    (pdflatex: {passes} runs, bibtex: {bibtex_runs})");
    ensure!(
        passes >= case.passes,
        "the case must need {} runs",
        case.passes
    );
    ensure!(out.is_empty(), "partex build wrote to standard output");
    for want in [
        "error[undefined-control-sequence]: Undefined control sequence \\undefinedcontrolsequence",
        "  --> modern.tex:17:4",
        "warning: 1 overfull \\hbox",
        "warning: 1 undefined reference: `sec:nowhere`",
        "warning: 1 font substitution",
        "OT1/cmr/bx/sc -> OT1/cmr/bx/n",
    ] {
        ensure!(report.contains(want), "the report lacks `{want}`");
    }
    // (the last line: `Failed modern.tex · 1 error · 3 warnings · 1 page ·
    // 3 passes · 0.41 s`, its separators as the locale has them)
    let last = report
        .lines()
        .find(|l| l.trim_start().starts_with("Failed modern.tex"))
        .context("the report has no `Failed modern.tex` line")?;
    for want in ["1 error", "3 warnings", "1 page", "3 passes"] {
        ensure!(last.contains(want), "the last line lacks `{want}`: {last}");
    }
    ensure!(
        !report.contains("modern: a line from typeout"),
        "the report shows \\typeout lines without -v"
    );
    let mut diffs = compare(&o, &p, true)?;
    // Again, from the session it saved: nothing changed.
    let (_, report) = modern(&["build", "-v", "modern.tex"])?;
    ensure!(
        report.contains("modern: a line from typeout"),
        "-v does not show \\typeout lines"
    );
    diffs.extend(
        compare(&o, &p, true)?
            .into_iter()
            .map(|n| format!("from the saved session: {n}")),
    );
    let (why, _) = modern(&["why", "modern.tex"])?;
    ensure!(
        why.contains("warning: 1 overfull \\hbox"),
        "partex why lacks the warnings"
    );
    modern(&["clean", "modern.tex"])?;
    ensure!(
        !p.join("modern.pdf").exists() && !p.join("modern.aux").exists(),
        "partex clean left the outputs"
    );
    Ok(diffs)
}

/// `partex watch` (machine mode, its default) of `modern.tex` against
/// `pdflatex` run to its fixpoint as latexmk would: the files after the
/// first build, and after an edit (a word, then a new forward reference),
/// must be identical; the watch must say it runs as a machine.
#[allow(clippy::too_many_lines)] // (one scripted session, kept whole)
fn run_modern_watch(root: &Path, partex: &Path, sanitize: bool) -> Result<Vec<String>> {
    let case = Converge {
        name: if sanitize {
            "modern_watch_sanitized"
        } else {
            "modern_watch"
        },
        oracle: "pdflatex",
        inputs: &["modern.tex"],
        ini: &[],
        args: &["-interaction=nonstopmode"],
        job: "modern",
        watch: &["modern.aux", "modern.bbl"],
        passes: 3,
        outdir: false,
    };
    let work = out_root(root).join(case.name);
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let (o, p) = (work.join("o"), work.join("p"));
    fs::create_dir_all(&o)?;
    fs::create_dir_all(&p)?;
    for input in case.inputs {
        let src = root.join("tests/e2e").join(input);
        fs::copy(&src, o.join(input))?;
        fs::copy(&src, p.join(input))?;
    }
    let args = ["-interaction=nonstopmode", "modern.tex"];
    oracle_passes(&case, &o, &o, &args)?;
    let _ = fs::remove_file(o.join("bibterm.txt"));
    let mut child = Command::new(partex)
        .env("PARTEX_CACHE_DIR", work.join("cache"))
        // (its own: `modern` may be making its formats at the same time)
        .env(
            "PARTEX_FORMATS",
            root.join(if sanitize {
                "target/e2e-formats-watch-sanitized"
            } else {
                "target/e2e-formats-watch"
            }),
        )
        .env("NO_COLOR", "1")
        .env("SOURCE_DATE_EPOCH", "1758800000")
        .env("FORCE_SOURCE_DATE", "1")
        .env_remove("PARTEX_PERSIST")
        .env_remove("PARTEX_MACHINE")
        // (with the sanitizer, each rebuild is also built afresh and
        // compared: its passes, BibTeX between them, against one fresh
        // build of the same inputs)
        .env("PARTEX_MACHINE_SANITIZE", if sanitize { "1" } else { "0" })
        .current_dir(&p)
        .args(["watch", "-v", "modern.tex"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let (tx, rx) = std::sync::mpsc::channel();
    let err = child.stderr.take().context("the watch's stderr")?;
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(err).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut report = String::new();
    // (until the build ends: `Watching` after the first; after a rebuild,
    // `-v`'s report of it, which follows its line in the log)
    let mut wait = |until: &str| -> Result<()> {
        loop {
            let line = rx
                .recv_timeout(std::time::Duration::from_secs(120))
                .with_context(|| format!("partex watch stopped before `{until}`"))?;
            eprintln!("    {line}");
            report.push_str(&line);
            report.push('\n');
            if line.contains(until) {
                return Ok(());
            }
        }
    };
    wait("Watching")?;
    let mut diffs: Vec<String> = compare(&o, &p, true)?
        .into_iter()
        .map(|n| format!("build: {n}"))
        .collect();
    let edits = [
        ("is at the end", "is at the very end"),
        ("\\section{End}", "See~\\ref{sec:more}.\n\\section{End}"),
        (
            "\\bibliographystyle",
            "\\section{More}\\label{sec:more}\n\\bibliographystyle",
        ),
    ];
    for (i, (marker, new)) in edits.iter().enumerate() {
        for dir in [&o, &p] {
            let f = dir.join("modern.tex");
            let text = fs::read_to_string(&f)?;
            ensure!(text.contains(marker), "modern.tex has no `{marker}`");
            // (as editors save: a watcher never sees half a file)
            let tmp = dir.join("modern.tex.new");
            fs::write(&tmp, text.replacen(marker, new, 1))?;
            fs::rename(&tmp, &f)?;
        }
        oracle_passes(&case, &o, &o, &args)?;
        let _ = fs::remove_file(o.join("bibterm.txt"));
        wait("Machine rebuilt in")?;
        diffs.extend(
            compare(&o, &p, true)?
                .into_iter()
                .map(|n| format!("edit {}: {n}", i + 1)),
        );
    }
    if let Some(mut stdin) = child.stdin.take() {
        writeln!(stdin, "q")?;
    }
    child.wait()?;
    ensure!(
        report.contains("Machine built in"),
        "partex watch did not run in machine mode"
    );
    ensure!(
        report.contains("Machine rebuilt in"),
        "partex watch did not rebuild incrementally"
    );
    Ok(diffs)
}

/// Machine-mode rebuilds (`PARTEX_MACHINE=1`, DESIGN.md §7.4, §7.11) of
/// `edits.tex` (40 sections of random words, so lines and pages break
/// unevenly, one paragraph per line) under an output directory, edited
/// four times in memory (`PARTEX_MACHINE_EDIT`), against the oracle run
/// once on the edited text, both reading the `.aux` and `.toc` two earlier
/// runs wrote: every file must be identical, also when every rebuild but
/// the last stops early at a region boundary (`PARTEX_MACHINE_STOP=1`) and
/// the next goes on from there. LaTeX reads its `.aux` from disk at
/// `\begin{document}` and back at `\end{document}`, writing the `.toc`
/// from it: a rebuild that replayed a region inside the read back once
/// read on in the `.aux` from disk (these edits of this text showed it).
/// The last two insert lines (a paragraph, then a blank line that changes
/// no output), which moves every later line of the file; a third run
/// checks each rebuild with the sanitizer (`PARTEX_MACHINE_SANITIZE=1`),
/// which found that keying positions by lines left broke how the regions
/// before an inserted line chain.
fn run_machine_edits(root: &Path, partex: &Path) -> Result<Vec<String>> {
    let long = format!("Para20x1 {}", "inserted ".repeat(150));
    let edits = [
        ("Para5x2 ", "Para5x2 TYPED "),
        ("Para20x1 ", long.as_str()),
        ("Para5x2 TYPED ", "Para5x2 TYPED MORE "),
        ("Para2x0 ", "Para2x0 early "),
        ("Para7x1 ", "Para7x1 \n\nA paragraph of its own.\n\n"),
        ("\\label{sec:12}\n", "\\label{sec:12}\n\n"),
    ];
    machine_edits(
        root,
        partex,
        "machine_edits",
        "edits.tex",
        &edits,
        "pdflatex",
        true,
    )
}

/// [`run_machine_edits`] in DVI mode (LaTeX's `latex` format): an edit in
/// section 5 of `edits-dvi.tex`, then one in section 2. The DVI writer
/// keeps a page's bytes in its buffer after the page is shipped; a rebuild
/// that did not compare them met the old run at the next boundary, and
/// the page came out as it was before the edit.
fn run_machine_edits_dvi(root: &Path, partex: &Path) -> Result<Vec<String>> {
    let edits = [("P5x2 ", "P5x2 typed "), ("P2x0 ", "P2x0 early ")];
    // (no stopped rebuilds: a DVI page's bytes depend on every page before
    // it, so a re-run after a longer page goes on to the job's end, where
    // there is no boundary left to stop at)
    machine_edits(
        root,
        partex,
        "machine_edits_dvi",
        "edits-dvi.tex",
        &edits,
        "latex",
        false,
    )
}

/// The case `name`: `input` (from `tests/e2e/`, as `edits.tex`) built by
/// `format`, edited by `edits` in one machine-mode process, against the
/// oracle on the edited text; with `stops`, again with every rebuild but
/// the last stopped early.
fn machine_edits(
    root: &Path,
    partex: &Path,
    name: &str,
    input: &str,
    edits: &[(&str, &str)],
    format: &str,
    stops: bool,
) -> Result<Vec<String>> {
    let work = root.join("target/e2e").join(name);
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let text = fs::read_to_string(root.join("tests/e2e").join(input))?;
    let mut edited = text.clone();
    for &(from, to) in edits {
        ensure!(edited.contains(from), "the document has no `{from}`");
        edited = edited.replacen(from, to, 1);
    }
    let flags = [
        "-no-shell-escape",
        "-no-parse-first-line",
        "-output-comment=partex",
        "-interaction=nonstopmode",
    ];
    let (jobname, ini_file, fmt) = (
        format!("-jobname={format}"),
        format!("*{format}.ini"),
        format!("-fmt={format}"),
    );
    let mut ini = flags.to_vec();
    ini.extend(["-ini", "-etex", jobname.as_str(), ini_file.as_str()]);
    let mut args = flags.to_vec();
    args.extend([fmt.as_str(), "-output-directory=out", "edits"]);
    let dirs = ["seed", "o", "p", "q", "r"].map(|d| work.join(d));
    for d in &dirs {
        fs::create_dir_all(d.join("out"))?;
    }
    let [seed, o, p, q, r] = &dirs;
    // (the files two runs leave)
    fs::write(seed.join("edits.tex"), &text)?;
    exec(Command::new("pdftex"), seed, &ini, "term0.txt")?;
    for _ in 0..2 {
        exec(Command::new("pdftex"), seed, &args, "term.txt")?;
    }
    for d in [o, p, q, r] {
        for f in ["edits.aux", "edits.toc"] {
            fs::copy(seed.join("out").join(f), d.join("out").join(f))?;
        }
    }
    fs::write(o.join("edits.tex"), &edited)?;
    exec(Command::new("pdftex"), o, &ini, "term0.txt")?;
    exec(Command::new("pdftex"), o, &args, "term.txt")?;
    let spec: Vec<String> = edits
        .iter()
        .map(|(from, to)| format!("edits.tex|{from}|{to}"))
        .collect();
    let spec = spec.join(";;");
    let mut diffs = Vec::new();
    // (the directory, `PARTEX_MACHINE_STOP`, and whether sanitized)
    let runs: &[(&Path, Option<&str>, bool)] = if stops {
        &[(p, None, false), (q, Some("1"), false), (r, None, true)]
    } else {
        &[(p, None, false)]
    };
    for &(d, stop, sanitize) in runs {
        fs::write(d.join("edits.tex"), &text)?;
        let mut cmd = tex_compat(partex);
        cmd.arg("-engine=pdftex");
        exec(cmd, d, &ini, "term0.txt")?;
        let mut cmd = tex_compat(partex);
        cmd.arg("-engine=pdftex")
            .env("PARTEX_MACHINE", "1")
            .env("PARTEX_MACHINE_EDIT", &spec)
            .env("PARTEX_MACHINE_SANITIZE", if sanitize { "1" } else { "0" })
            .env_remove("PARTEX_MACHINE_STOP");
        if let Some(n) = stop {
            cmd.env("PARTEX_MACHINE_STOP", n);
        }
        let err = exec(cmd, d, &args, "term.txt")?;
        let rebuilt = err.matches("machine: rebuilt in").count();
        let stopped = err.matches("machine: stopped in").count();
        ensure!(
            rebuilt + stopped == edits.len() && (stop.is_none() || stopped > 0),
            "{rebuilt} rebuilds and {stopped} stopped, for {} edits",
            edits.len()
        );
        let name = d
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut found = compare(o, d, false)?;
        found.retain(|n| n != "edits.tex");
        found.extend(
            compare(&o.join("out"), &d.join("out"), false)?
                .into_iter()
                .map(|n| format!("out/{n}")),
        );
        diffs.extend(found.into_iter().map(|n| format!("{name}: {n}")));
    }
    Ok(diffs)
}

/// Run plain TeX in `o` on `args`; with `fixpoint`, again while the files
/// it writes (but its log and DVI file) change, as `-converge` does.
fn fixpoint(program: &str, o: &Path, args: &[&str], fixpoint: bool) -> Result<()> {
    let snapshot = || -> Result<Vec<(std::ffi::OsString, Vec<u8>)>> {
        let mut files = Vec::new();
        for e in fs::read_dir(o)? {
            let e = e?;
            let name = e.file_name();
            let n = name.to_string_lossy();
            if !(n.ends_with(".log")
                || n.ends_with(".dvi")
                || n.ends_with(".pdf")
                || n == "term.txt")
            {
                files.push((name, fs::read(e.path())?));
            }
        }
        files.sort();
        Ok(files)
    };
    for _ in 0..5 {
        let before = snapshot()?;
        exec(Command::new(program), o, args, "term.txt")?;
        if !fixpoint || snapshot()? == before {
            break;
        }
    }
    Ok(())
}

/// Run an incremental case as `partex -resident` invocations, or as
/// `partex -converge` processes with a persisted session.
fn run_resident(root: &Path, partex: &Path, case: &Incremental) -> Result<Vec<String>> {
    let work = out_root(root).join(case.name);
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let (o, p) = (work.join("o"), work.join("p"));
    fs::create_dir_all(&o)?;
    fs::create_dir_all(&p)?;
    for input in case.inputs {
        let src = root.join("tests/e2e").join(input);
        fs::copy(&src, o.join(input))?;
        fs::copy(&src, p.join(input))?;
    }
    let ini = ["-ini", "-interaction=nonstopmode", r"\input plain \dump"];
    exec(Command::new("tex"), &o, &ini, "term0.txt")?;
    exec(tex_compat(partex), &p, &ini, "term0.txt")?;
    fs::remove_file(o.join("term0.txt"))?;
    fs::remove_file(p.join("term0.txt"))?;
    let args = [
        "-output-comment=partex",
        "-interaction=nonstopmode",
        "&plain",
        case.job,
    ];
    let persisted = case.by == By::Persisted;
    let mut resident = args.to_vec();
    resident.splice(
        0..0,
        [
            if persisted { "-converge" } else { "-resident" },
            "-checkpoint-every=4",
        ],
    );
    let cache = work.join("cache");
    // (a session per case directory; it ends soon after the last build)
    let build = || -> Result<String> {
        let mut cmd = tex_compat(partex);
        cmd.env("PARTEX_REPORT", "1")
            .env("PARTEX_RESIDENT_IDLE", "5")
            .env("PARTEX_CACHE_DIR", &cache)
            .env_remove("PARTEX_PERSIST");
        exec(cmd, &p, &resident, "term.txt")
    };
    let oracle = || fixpoint("tex", &o, &args, persisted);
    let mut loads = 0;
    let (mut diffs, mut runs, mut cutoffs) = (Vec::new(), Vec::new(), 0);
    let mut note = |report: &str| {
        for line in report.lines() {
            eprintln!("    {line}");
            loads += usize::from(line.contains("loaded a saved session"));
            if let Some(rest) = line.trim_start().strip_prefix("partex: rebuilt in ") {
                let mut w = rest.split(' ').skip(2);
                let ran: Option<u64> = w.next().and_then(|n| n.parse().ok());
                let all: Option<u64> = w.nth(1).and_then(|n| n.parse().ok());
                runs.extend(ran.zip(all));
                cutoffs += usize::from(line.contains("converged at"));
            }
        }
    };
    note(&build()?);
    oracle()?;
    diffs.extend(
        compare(&o, &p, false)?
            .into_iter()
            .map(|n| format!("build: {n}")),
    );
    for (i, &(file, marker, new)) in case.edits.iter().enumerate() {
        for dir in [&o, &p].into_iter().filter(|_| !marker.is_empty()) {
            let f = dir.join(file);
            let text = fs::read_to_string(&f)?;
            ensure!(text.contains(marker), "{file} has no `{marker}`");
            fs::write(&f, text.replacen(marker, new, 1))?;
        }
        note(&build()?);
        oracle()?;
        diffs.extend(
            compare(&o, &p, false)?
                .into_iter()
                .map(|n| format!("edit {}: {n}", i + 1)),
        );
    }
    if persisted && loads != case.edits.len() {
        diffs.push(format!(
            "{loads} builds loaded a saved session, not {}",
            case.edits.len()
        ));
    }
    if !runs.iter().any(|&(a, b)| a < b) {
        diffs.push(format!("no rebuild resumed from a checkpoint: {runs:?}"));
    }
    if cutoffs < case.cutoffs {
        diffs.push(format!(
            "{cutoffs} rebuilds stopped early, not {}",
            case.cutoffs
        ));
    }
    Ok(diffs)
}

/// A case to run, by name.
type Job<'a> = (
    &'static str,
    Box<dyn Fn() -> Result<Vec<String>> + Send + Sync + 'a>,
);

/// Where the cases' outputs go: `target/e2e`, or `XTASK_E2E_DIR` (the
/// gate's machine-mode run keeps its own).
fn out_root(root: &Path) -> PathBuf {
    root.join(std::env::var("XTASK_E2E_DIR").unwrap_or_else(|_| "target/e2e".to_owned()))
}

/// The binary the cases run: `XTASK_PARTEX`, one built before, as it is;
/// else the release build, made now.
fn partex_binary(root: &Path) -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("XTASK_PARTEX") {
        return Ok(PathBuf::from(p));
    }
    let status = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(["build", "--release", "-p", "partex-cli"])
        .status()?;
    ensure!(status.success(), "building partex failed");
    Ok(root.join("target/release/partex"))
}

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    let filter = args.first().map(String::as_str);
    let partex = partex_binary(root)?;
    // Each case has its own directory: they run side by side, reported in
    // order.
    let mut jobs: Vec<Job> = Vec::new();
    let partex = &partex;
    for case in CASES {
        jobs.push((
            case.name,
            Box::new(move || {
                // Knuth's TeX reads the clock: a run that straddles a
                // minute differs in the date, so a difference must repeat
                // to count
                let diffs = run_case(root, partex, case)?;
                if diffs.is_empty() {
                    Ok(diffs)
                } else {
                    run_case(root, partex, case)
                }
            }),
        ));
    }
    for case in INCREMENTAL {
        jobs.push((
            case.name,
            Box::new(move || run_incremental(root, partex, case)),
        ));
    }
    for case in CONVERGE {
        jobs.push((
            case.name,
            Box::new(move || run_converge(root, partex, case)),
        ));
    }
    jobs.push((
        "machine_edits",
        Box::new(move || run_machine_edits(root, partex)),
    ));
    jobs.push((
        "machine_edits_dvi",
        Box::new(move || run_machine_edits_dvi(root, partex)),
    ));
    jobs.push(("modern", Box::new(move || run_modern(root, partex))));
    jobs.push(("glyphs", Box::new(move || run_glyphs(root, partex))));
    jobs.push((
        "xelatex_runs",
        Box::new(move || run_xelatex_runs(root, partex)),
    ));
    jobs.push(("synctex", Box::new(move || run_synctex(root, partex))));
    jobs.push(("display", Box::new(move || run_display(root, partex))));
    jobs.push((
        "modern_watch",
        Box::new(move || run_modern_watch(root, partex, false)),
    ));
    jobs.push((
        "modern_watch_sanitized",
        Box::new(move || run_modern_watch(root, partex, true)),
    ));
    jobs.retain(|(name, _)| filter.is_none_or(|f| name.contains(f)));
    let ran = jobs.len();
    let width = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<Result<Vec<String>>>>> =
        (0..ran).map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..width.min(ran) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((_, job)) = jobs.get(i) else { break };
                    let r = job();
                    if let Ok(mut slot) = results[i].lock() {
                        *slot = Some(r);
                    }
                }
            });
        }
    });
    let mut failed = 0;
    for ((name, _), r) in jobs.iter().zip(results) {
        match r.into_inner().ok().flatten() {
            Some(Ok(diffs)) if diffs.is_empty() => println!("ok       {name}"),
            Some(Ok(diffs)) => {
                failed += 1;
                println!("DIFFERS  {name}: {}", diffs.join(", "));
            }
            Some(Err(e)) => {
                failed += 1;
                println!("FAILED   {name}: {e:#}");
            }
            None => {
                failed += 1;
                println!("FAILED   {name}: did not finish");
            }
        }
    }
    println!(
        "\ne2e: {}/{ran} cases identical (outputs in {})",
        ran - failed,
        out_root(root).display()
    );
    ensure!(failed == 0, "e2e outputs differ");
    Ok(())
}

/// Run `cmd`, keeping its terminal output in `term`; returns its stderr.
/// Runs `case` on both sides; the names of the files that differ.
fn run_case(root: &Path, partex: &Path, case: &Case) -> Result<Vec<String>> {
    let work = out_root(root).join(case.name);
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let (o, p) = (work.join("o"), work.join("p"));
    fs::create_dir_all(&o)?;
    fs::create_dir_all(&p)?;
    for input in case.inputs {
        let src = root.join("tests/e2e").join(input);
        fs::copy(&src, o.join(input))?;
        fs::copy(&src, p.join(input))?;
    }
    for (i, args) in case.runs.iter().enumerate() {
        let term = format!("term{}.txt", i + 1);
        if let Some(&tool @ (BIBTEX | MAKEINDEX)) = args.first() {
            exec(Command::new(&tool[1..]), &o, &args[1..], &term)?;
            let mut cmd = Command::new(partex);
            cmd.arg(format!("-{}", &tool[1..])); // (not an engine: no `--compat`)
            exec(cmd, &p, &args[1..], &term)?;
            continue;
        }
        let mut oracle = Command::new(case.oracle);
        let mut cmd = tex_compat(partex);
        if case.oracle == "xetex" {
            // (TeX Live's fonts only, on both sides: DESIGN 4.7)
            let fonts = root.join("scripts/xetex/fonts.conf");
            oracle.env("FONTCONFIG_FILE", &fonts);
            cmd.env("FONTCONFIG_FILE", &fonts).env("PARTEX_XETEX", "1");
        }
        exec(oracle, &o, args, &term)?;
        cmd.arg(format!("-engine={}", case.oracle));
        exec(cmd, &p, args, &term)?;
    }
    compare(&o, &p, false)
}

/// partex with TeX's command line (bare `partex` is the modern one:
/// DESIGN.md, "Command line and terminal"); `-engine=` still picks the
/// engine.
fn tex_compat(partex: &Path) -> Command {
    let mut cmd = Command::new(partex);
    cmd.arg("--compat=tex");
    cmd
}

fn exec(mut cmd: Command, dir: &Path, args: &[&str], term: &str) -> Result<String> {
    // one fixed time for both sides: runs that straddle a minute would
    // differ in `\time` (LaTeX writes it to texsys.aux)
    let out = cmd
        .env("SOURCE_DATE_EPOCH", "1758800000")
        .env("FORCE_SOURCE_DATE", "1")
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    fs::write(dir.join(term), out.stdout)?; // errors are part of the output
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Names of the files that differ (or exist on one side only); without
/// the terminal transcripts if `no_term`.
#[allow(clippy::case_sensitive_file_extension_comparisons)] // names are ours
fn compare(o: &Path, p: &Path, no_term: bool) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    for dir in [o, p] {
        for e in fs::read_dir(dir)? {
            let e = e?;
            let n = e.file_name().to_string_lossy().into_owned();
            let skip =
                n.ends_with(".fmt") || no_term && n.starts_with("term") || e.file_type()?.is_dir();
            if !skip && !names.contains(&n) {
                names.push(n);
            }
        }
    }
    names.sort();
    let masks = crate::mask::memory_statistics();
    let mask = |b: &[u8]| {
        let mut s = String::from_utf8_lossy(skip_line(b)).into_owned();
        for (re, rep) in &masks {
            s = re.replace_all(&s, *rep).into_owned();
        }
        s
    };
    let mut diffs = Vec::new();
    for n in names {
        let (a, b) = (fs::read(o.join(&n)), fs::read(p.join(&n)));
        let same = match (a, b) {
            (Ok(a), Ok(b))
                if n.ends_with(".log") || n.ends_with(".blg") || n.starts_with("term") =>
            {
                mask(&a) == mask(&b)
            }
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !same {
            diffs.push(n);
        }
    }
    Ok(diffs)
}

fn skip_line(b: &[u8]) -> &[u8] {
    b.iter()
        .position(|&c| c == b'\n')
        .map_or(&[], |i| &b[i + 1..])
}
