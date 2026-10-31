//! Gates for pdfTeX's primitives: the expandable commands and random
//! numbers under INITEX (`oracle/fixtures/pdf/pdf*.tex` with `.log`
//! oracles, run as `core.tex`), and what `\shipout` resolves of
//! `\pdfsavepos` (`savepos.tex`: the `\write` stream equals pdfTeX's
//! `savepos.pos`).
//!
//! The `.pdf` oracles need a PDF output, which this crate does not have
//! yet (it writes DVI): their byte comparison is the backend's gap.

mod common;

const PRELUDE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\&=4 \\catcode`\\#=6 \\catcode`\\^=7 \\catcode`\\~=13 \\hsize=468pt \\vsize=643pt \\parindent=20pt \\maxdepth=4pt \\baselineskip=12pt \\topskip=10pt \\tolerance=200 \\linepenalty=10 %\n";

fn log_gate(n: &str) -> Result<(), String> {
    let dir = common::fixtures().join("pdf");
    let src = common::read(&dir.join(format!("{n}.tex")));
    let want = common::read(&dir.join(format!("{n}.log")));
    let want = want.trim_end_matches('\n');
    let doc = format!("{PRELUDE}{src}\\end\n");
    let opts = common::options("./core.tex", "texput", "", &dir);
    let b = common::build(opts, None, &doc);
    let (_, log) = b.output();
    let log = common::log_excerpt(&log);
    match common::first_difference(log.as_bytes(), want.as_bytes()) {
        None => Ok(()),
        Some(d) => Err(format!("pdf/{n}: FAIL {d}")),
    }
}

#[test]
fn expandable() {
    log_gate("pdfexpand").unwrap_or_else(|e| panic!("{e}"));
}

/// A font that cannot be loaded: pdfTeX's error for it (§561's
/// `start_font_error_message`, web2c's reasons: a TFM not found, a bad
/// one, a name in quotes where it has a space, `at` and `scaled`), as
/// `pdftex -ini` logs it (`pdf/fontload.log`, TeX Live 2026, with
/// `fontloadbad.tfm`, not a TFM).
#[test]
fn font_not_loadable_messages() {
    let dir = common::fixtures().join("pdf");
    let src = common::read(&dir.join("fontload.tex"));
    let want = common::read(&dir.join("fontload.log"));
    let opts = common::options("./fontload.tex", "fontload", "", &dir);
    let b = common::build(opts, None, &src);
    let (_, log) = b.output();
    let errs = |s: &str| -> Vec<String> {
        s.lines()
            .filter(|l| l.starts_with("! "))
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(errs(&log), errs(&want));
}

/// `\pdftexbanner` is pdfTeX's `pdftex_banner`, TeX Live's and
/// kpathsea's versions included, as the oracle's `/PTEX.FullBanner`
/// shows it: hyperxmp makes acmart's `/Producer` of it.
#[test]
fn banner() {
    let dir = common::fixtures().join("pdf");
    let doc = format!("{PRELUDE}\\message{{[\\pdftexbanner]}}\\end\n");
    let opts = common::options("./core.tex", "texput", "", &dir);
    let b = common::build(opts, None, &doc);
    let (_, log) = b.output();
    let want = "[This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux) kpathsea version 6.4.2]";
    assert!(log.replace('\n', "").contains(want), "{log}");
}

#[test]
fn random() {
    log_gate("pdfrandom").unwrap_or_else(|e| panic!("{e}"));
}

/// The numbers `\pdflastlink` and `\pdflastximage` give after a page
/// fired at a paragraph's start, as the log shows them (`[L<k>=<n>]` of
/// `pdf/linkloop`, `[I<k>=<n>]` of `pdf/imgloop`): the page's objects
/// come before them, as pdfTeX numbers them.
#[test]
fn read_time_numbers_in_the_log() {
    let dir = common::fixtures().join("pdf");
    for (name, tag) in [("linkloop", "[L"), ("imgloop", "[I")] {
        let marks = |log: &str| -> Vec<String> {
            let flat = log.replace('\n', "");
            flat.split(tag)
                .skip(1)
                .filter_map(|x| x.split_once(']').map(|(m, _)| m.to_owned()))
                .collect()
        };
        let src = common::read(&dir.join(format!("{name}.tex")));
        let want = marks(&common::read(&dir.join(format!("{name}.log"))));
        assert!(
            !want.is_empty(),
            "pdf/{name}: no numbers in the oracle's log"
        );
        let opts = phitex_tex::Options {
            epoch: Some(0),
            ..common::options(&format!("./{name}.tex"), name, "", &dir)
        };
        let b = common::build(opts, None, &src);
        let (_, log) = b.output();
        assert_eq!(marks(&log), want, "pdf/{name}: the numbers in the log");
    }
}

#[test]
fn savepos() {
    let dir = common::fixtures().join("pdf");
    let src = common::read(&dir.join("savepos.tex"));
    let want = common::read(&dir.join("savepos.pos"));
    let opts = common::options("./savepos.tex", "savepos", "", &dir);
    let b = common::build(opts, None, &src);
    let mut got = String::new();
    for (_, t) in b.writes().into_iter().filter(|(k, _)| *k == 1) {
        got.push_str(&t);
        got.push('\n');
    }
    if let Some(d) = common::first_difference(got.as_bytes(), want.as_bytes()) {
        panic!("pdf/savepos.pos: FAIL {d}");
    }
}

/// The PDF documents not yet byte-identical, compared by their streams
/// (PDF-BACKEND-GAP):
/// - vfgap: a form without `\immediate` whose virtual font brings in a
///   new font has it numbered at `\pdfxform`, not when pdfTeX writes
///   the form (after the page that references it), so a font the page
///   brings in first gets the other number (`pdf_documents_gaps`).
const BACKEND_GAPS: &[&str] = &["vfgap"];

/// The `stream ... endstream` bodies of a PDF file.
fn streams(b: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(k) = find(&b[i..], b"stream\n") {
        let s = i + k + 7;
        let Some(e) = find(&b[s..], b"endstream") else {
            break;
        };
        out.push(b[s..s + e].to_vec());
        i = s + e + 9;
    }
    out
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// The documents that do not pass yet: un-ignored as they are fixed.
const PENDING: &[&str] = &[];

/// Gate 1: `pdftex -ini` on `oracle/fixtures/pdf/*.tex` with a `.pdf`
/// oracle: byte-identical; for the documents on a backend gap, whether
/// each of the oracle's streams (pages, fonts) is ours.
#[test]
fn pdf_documents() {
    pdf_gate(false);
}

/// The documents on [`BACKEND_GAPS`], byte-identical: fails until the
/// gaps are fixed.
#[test]
#[ignore = "PDF-BACKEND-GAP: vfgap (virtual fonts of a form read at \\pdfxform)"]
fn pdf_documents_gaps() {
    let dir = common::fixtures().join("pdf");
    for name in BACKEND_GAPS {
        let src = common::read(&dir.join(format!("{name}.tex")));
        let want = std::fs::read(dir.join(format!("{name}.pdf"))).expect("the oracle");
        let opts = phitex_tex::Options {
            epoch: Some(0),
            ..common::options(&format!("./{name}.tex"), name, "", &dir)
        };
        let (pdf, _) = common::build(opts, None, &src).output();
        if let Some(d) = common::first_difference(&pdf, &want) {
            panic!("{name}: {d}");
        }
    }
}

fn pdf_gate(pending: bool) {
    let dir = common::fixtures().join("pdf");
    let only = std::env::var("PHITEX_GATE").ok();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("the fixtures")
        .flatten()
        .filter_map(|e| {
            let f = e.file_name().to_string_lossy().into_owned();
            f.strip_suffix(".tex").map(str::to_owned)
        })
        .filter(|n| dir.join(format!("{n}.pdf")).exists())
        .filter(|n| PENDING.contains(&n.as_str()) == pending)
        .filter(|n| only.as_ref().is_none_or(|o| o == n))
        .collect();
    names.sort();
    let mut failed = Vec::new();
    for name in names {
        let src = common::read(&dir.join(format!("{name}.tex")));
        let want = std::fs::read(dir.join(format!("{name}.pdf"))).expect("the oracle");
        let opts = phitex_tex::Options {
            epoch: Some(0),
            ..common::options(&format!("./{name}.tex"), &name, "", &dir)
        };
        let b = common::build(opts, None, &src);
        let (pdf, _) = b.output();
        if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
            let _ = std::fs::write(format!("{d}/{name}.pdf"), &pdf);
        }
        match common::first_difference(&pdf, &want) {
            None => eprintln!("ok {name}: byte-identical"),
            Some(d) => {
                let gap = BACKEND_GAPS.contains(&name.as_str());
                let ours = streams(&pdf);
                let theirs = streams(&want);
                let missing = theirs.iter().filter(|s| !ours.contains(s)).count();
                eprintln!(
                    "{} {name}: {d}\n  {missing} of the oracle's {} streams not ours",
                    if gap { "gap" } else { "FAIL" },
                    theirs.len()
                );
                if !gap {
                    failed.push(name);
                }
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

/// The writer as a fold: edits on PDF documents (objects, forms, links,
/// outlines, page references, images), each equal to a cold build.
#[test]
fn pdf_edits() {
    let dir = common::fixtures().join("pdf");
    let cases: &[(&str, &[(&str, &str)])] = &[
        (
            "annot",
            &[
                ("link text", "lnik text"),
                ("(Hi)", "(Ho)"),
                ("zoom 2000", "zoom 1000"),
            ],
        ),
        (
            "objstm2",
            &[
                ("<< /A 1 >>", "<< /A 2 >>"),
                ("{First}", "{Fist}"),
                ("B\\par", "BB\\par"),
            ],
        ),
        (
            "xform",
            &[
                ("X\\pdfliteral", "Z\\pdfliteral"),
                ("width 20pt", "width 30pt"),
            ],
        ),
        (
            "pageref",
            &[("A\\a", "AA\\a"), ("\\pdfpageref 3", "\\pdfpageref 4")],
        ),
        ("outlinetree", &[("{One.A}", "{One.Z}"), ("{Two}", "{Too}")]),
        (
            "csform",
            &[
                ("1 0 0 rg", "0 1 0 rg"),
                ("D\\pdfrefxform", "DD\\pdfrefxform"),
            ],
        ),
        ("linkbreak", &[("several", "sevral"), ("Then", "Thn")]),
        ("png", &[("{png-gray.png}", "{png-rgba.png}")]),
    ];
    let only = std::env::var("PHITEX_GATE").ok();
    let mut failed = Vec::new();
    for &(name, edits) in cases {
        if only.as_ref().is_some_and(|o| o != name) {
            continue;
        }
        let src = common::read(&dir.join(format!("{name}.tex")));
        let opts = || phitex_tex::Options {
            epoch: Some(0),
            ..common::options(&format!("./{name}.tex"), name, "", &dir)
        };
        let mut b = common::build(opts(), None, &src);
        let mut text = src.clone();
        for &(from, to) in edits {
            assert!(text.contains(from), "{name}: {from:?}");
            text = text.replacen(from, to, 1);
            b.edit(&text);
            let cold = common::build(opts(), None, &text);
            match common::first_difference(&b.output().0, &cold.output().0) {
                None => eprintln!("ok {name} {from:?}: equals cold"),
                Some(d) => {
                    eprintln!("FAIL {name} {from:?}: {d}");
                    failed.push(format!("{name} {from:?}"));
                }
            }
        }
    }
    assert!(failed.is_empty(), "edits differing from cold: {failed:?}");
}

/// The executor's counters on plain documents (story.tex, pages.tex)
/// at 1, 4 and 16 threads, with the same output at each.
#[test]
fn parallel_counters() {
    let fx = common::fixtures().join("plain");
    let prelude = common::plain();
    for name in ["story", "pages"] {
        let src = common::read(&fx.join(format!("{name}.tex")));
        let run = |t: usize| {
            let opts = phitex_tex::Options {
                threads: Some(t),
                ..common::options(&format!("./{name}.tex"), name, "", &fx)
            };
            let b = common::build(opts, Some(&prelude), &src);
            let s = b.stats.clone();
            eprintln!(
                "{name} {t} threads: wall {:?} busy {:?} peak {} critical path {:?} nodes run {}, \
                 batches {} ({} inline), largest {}",
                s.wall,
                s.busy,
                s.peak_running,
                s.critical_path,
                s.nodes_run,
                s.batches,
                s.batches_inline,
                s.jobs_max_batch
            );
            (s, b.output().0)
        };
        // (the runs at once and the busy time are counted in `profile`
        // builds only: no write to shared counters per run else)
        let counted = cfg!(feature = "profile");
        let (four, out4) = run(4);
        assert_eq!(four.threads, 4);
        assert!(four.peak_running <= 4 && (four.peak_running >= 1 || !counted));
        assert!(four.critical_path.is_some());
        let (one, out1) = run(1);
        assert_eq!(one.threads, 1);
        assert_eq!(one.peak_running, usize::from(counted));
        assert_eq!(one.batches, one.batches_inline);
        assert!(one.busy <= one.wall);
        // (at most as many runs at once as threads asked for: the
        // engine's thread running a batch's job counts as one of them)
        let (sixteen, out16) = run(16);
        assert!(sixteen.peak_running <= 16);
        assert!(
            out1 == out4 && out1 == out16,
            "{name}: output differs across threads"
        );
    }
}

/// `\openout` under TeX Live's `openout_any = p` (web2c §1374): an
/// absolute name, a `../` name, and one at `\shipout` are refused, and
/// the nonstop job ends there with no PDF file, as `pdftex -ini` does
/// (`! I can't write on file`, `*** (job aborted, file error in nonstop
/// mode)`, `no output PDF file produced`); a plain name is written.
#[test]
fn openout_refused() {
    let dir = common::fixtures().join("pdf");
    let doc = |open: &str| {
        format!(
            "\\catcode`\\{{=1 \\catcode`\\}}=2 \\pdfoutput=1 \\font\\r=cmr10 \\r \\hsize=100pt \\vsize=100pt\nBefore.{open} After.\\par\n\\end\n"
        )
    };
    let cases = [
        (
            "\\immediate\\openout1=../escape.txt \\immediate\\write1{x}\\immediate\\closeout1",
            Some("../escape.txt"),
        ),
        (
            "\\immediate\\openout1=/tmp/abs.txt \\immediate\\write1{x}\\immediate\\closeout1",
            Some("/tmp/abs.txt"),
        ),
        ("\\openout1=../late.txt \\write1{x}", Some("../late.txt")),
        (
            "\\immediate\\openout1=ok.txt \\immediate\\write1{x}\\immediate\\closeout1",
            None,
        ),
    ];
    for (open, refused) in cases {
        let opts = common::options("./t.tex", "t", "", &dir);
        let b = common::build(opts, None, &doc(open));
        let (pdf, log) = b.output();
        if let Some(name) = refused {
            assert!(pdf.is_empty(), "{name}: a PDF file was written");
            assert!(b.fatal(), "{name}: the job did not end");
            for want in [
                format!("! I can't write on file `{name}'."),
                "*** (job aborted, file error in nonstop mode)".to_owned(),
                "no output PDF file produced".to_owned(),
            ] {
                assert!(
                    log.contains(&want),
                    "{name}: no {want:?} in the log:\n{log}"
                );
            }
            assert!(b.files().iter().all(|(n, _)| n != name), "{name}: written");
        } else {
            assert!(pdf.starts_with(b"%PDF"), "ok.txt: no PDF file\n{log}");
            assert!(!b.fatal(), "ok.txt: the job ended\n{log}");
        }
    }
}

/// A font no map file has an entry for (`nomap10.tfm`: cmr10's metrics
/// under a name pdftex.map lacks): pdfTeX finds no Type 1 file and makes
/// a bitmap (PK) font through mktexpk, which this engine does not; as
/// pdfTeX without one (`Font nomap10 at 72 not found`), the job ends on
/// a fatal pdfTeX error with no output PDF file, the error among the
/// log's error lines (it has no place in the document, as pdfTeX's has
/// none). (The build was reported fine, its output a file
/// of no bytes with the pages' count: a viewer showed them blank.)
#[test]
fn font_without_map_entry_is_fatal() {
    // (pdfTeX's `writet3`: no PK font found at `\pdfpkresolution` scaled
    // by the font's size over its design size, on a magstep; 0 is
    // `pk_dpi`, 72; `pdftex_fail`'s two lines, no PDF file)
    let dir = common::fixtures().join("pdf");
    for (set, at) in [
        ("", 72),
        ("\\pdfpkresolution=600 ", 600),
        ("\\pdfpkresolution=600 ", 720),
    ] {
        let size = if at == 720 { " at 12pt" } else { "" };
        let doc = format!(
            "\\catcode`\\{{=1 \\catcode`\\}}=2 \\pdfoutput=1 {set}\\font\\r=nomap10{size} \\r Hello.\\end\n"
        );
        let opts = phitex_tex::Options {
            program: "pdflatex".to_owned(),
            ..common::options("./t.tex", "t", "", &dir)
        };
        let b = common::build(opts, None, &doc);
        let (pdf, log) = b.output();
        assert!(pdf.is_empty(), "a PDF file was written:\n{log}");
        assert!(b.fatal(), "the job did not end:\n{log}");
        let line =
            format!("!pdfTeX error: pdflatex (file nomap10): Font nomap10 at {at} not found");
        let lines: Vec<&str> = log.lines().collect();
        let k = lines.iter().position(|l| *l == line);
        assert!(k.is_some(), "no {line:?} in the log:\n{log}");
        assert_eq!(
            lines.last(),
            Some(&phitex_tex::FAIL_END),
            "the log's last line:\n{log}"
        );
        assert!(
            !log.contains("!  ==>"),
            "TeX's fatal line, not pdfTeX's:\n{log}"
        );
        assert_eq!(b.error_lines(), std::slice::from_ref(&line));
        // (a diagnostic: the error at the document's end, its help the
        // line pdfTeX says under it)
        let d: Vec<_> = b
            .diagnostics()
            .into_iter()
            .filter(|d| d.severity == phitex_tex::Severity::Error)
            .collect();
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].message, line[1..]);
        assert_eq!(d[0].help.as_deref(), Some(phitex_tex::FAIL_END));
        assert_eq!(d[0].line, 1);
    }
}

/// A plain pdfTeX document for glyph origins: paragraphs of varied
/// words over a few pages, a logo, a footnote, ligatures.
fn origins_doc() -> String {
    use std::fmt::Write as _;
    const WORDS: &[&str] = &[
        "origins",
        "viewer",
        "glyph",
        "source",
        "editor",
        "cursor",
        "paragraph",
        "letter",
        "column",
        "margin",
        "printer",
        "reader",
        "galley",
        "proof",
        "kerning",
        "spacing",
        "chapter",
        "section",
        "figure",
        "table",
        "index",
        "note",
        "line",
        "page",
        "word",
        "quiet",
        "modest",
        "elegant",
        "careful",
        "patient",
        "rebuilt",
        "shrank",
        "looked",
        "thought",
        "admired",
        "signature",
        "glossary",
        "breakfast",
        "ruler",
        "hyphen",
        "the",
        "of",
        "and",
        "a",
        "to",
        "in",
        "with",
        "from",
        "under",
        "over",
        "between",
        "around",
    ];
    let mut s = String::from(
        "\\pdfoutput=1\n\\hsize=4in \\vsize=4in \\parindent=20pt\n\\centerline{\\bf Glyph origins}\n\\medskip\n",
    );
    let mut x: u64 = 7;
    for k in 0..14 {
        let mut line = String::new();
        for _ in 0..(40 + k * 3) {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let w = WORDS[((x >> 33) as usize) % WORDS.len()];
            if line.is_empty() {
                let mut c = w.chars();
                let first = c.next().unwrap().to_ascii_uppercase();
                line.push(first);
                line.push_str(c.as_str());
            } else {
                line.push(' ');
                line.push_str(w);
            }
        }
        let _ = writeln!(s, "{line}.\n");
        if k == 4 {
            s.push_str("Here \\TeX\\ makes its logo, and a footnote\\footnote*{Footnotes come from the same paragraph as their mark.} follows the mark.\n\n");
        }
        if k == 8 {
            s.push_str("Ligatures like office, fluffy, affluent and difficult waffles join their letters.\n\n");
        }
    }
    s.push_str("\\bye\n");
    s
}

fn origins_build(src: &str, threads: usize, on: bool) -> phitex_tex::Build {
    let dir = common::fixtures().join("pdf");
    let opts = phitex_tex::Options {
        epoch: Some(0),
        threads: Some(threads),
        origins: on,
        ..common::options("./orig.tex", "orig", "", &dir)
    };
    common::build(opts, Some(&common::plain()), src)
}

/// Glyph origins of a plain document: every page's codes have one
/// each, every word maps to its bytes, the same at -j1 and -j8, the PDF
/// the same with them off; after a word's edit and a line inserted
/// before, the answers are a cold build's of the new text, and the
/// pages the edit left keep theirs (docs/note-origins.md).
#[test]
fn glyph_origins_plain() {
    let src = origins_doc();
    let b = origins_build(&src, 1, true);
    assert!(b.pages().len() >= 3, "pages: {}", b.pages().len());
    let f = common::origin_failures(&b, &src, "orig.tex");
    assert!(f.is_empty(), "origins:\n{}", f.join("\n"));
    let off = origins_build(&src, 1, false);
    assert!(
        off.output_bytes() == b.output_bytes(),
        "the PDF with origins on is not the PDF with them off"
    );
    assert!(off.page_origins(0).is_none(), "origins off: none");
    let b8 = origins_build(&src, 8, true);
    assert_eq!(
        b8.all_page_origins(),
        b.all_page_origins(),
        "-j8's origins are -j1's"
    );
    // edits: a word, then a line before everything
    let mut b = b;
    let (first, _) = b.origins_changed();
    assert_eq!(
        first.len(),
        b.pages().len(),
        "the first ask lists every page"
    );
    let at = src
        .find("Ligatures like office")
        .expect("the ligature line");
    let new1 = format!("{}Lagatures{}", &src[..at], &src[at + "Ligatures".len()..]);
    b.edit(&new1);
    let (changed, deltas) = b.origins_changed();
    assert!(
        !changed.is_empty() && changed.len() < b.pages().len(),
        "a word's edit changes some pages: {changed:?}"
    );
    assert!(
        deltas.is_empty(),
        "a word of the same length moves nothing: {deltas:?}"
    );
    let cold = origins_build(&new1, 1, true);
    assert_eq!(
        b.all_page_origins(),
        cold.all_page_origins(),
        "the edit's origins are a cold build's"
    );
    let new2 = format!("\\relax\n{new1}");
    b.edit(&new2);
    let (changed, deltas) = b.origins_changed();
    assert!(
        matches!(deltas[..], [(0, 0 | 1, 7)]),
        "a line inserted before: every byte moved: {deltas:?}"
    );
    assert!(
        changed.is_empty(),
        "a line inserted before: no page's origins changed but by the delta: {changed:?}"
    );
    let cold = origins_build(&new2, 1, true);
    assert_eq!(
        b.all_page_origins(),
        cold.all_page_origins(),
        "the second edit's origins are a cold build's"
    );
    let f = common::origin_failures(&b, &new2, "orig.tex");
    assert!(f.is_empty(), "origins after the edits:\n{}", f.join("\n"));
}

/// `\pdffilemoddate` of a file the job opened with `\openout` before
/// it (TeX made it then, §1374): a date, the build's clock as the job's
/// own log's is, not nothing. (Nothing: a document taking an `\openout`
/// file's date apart character by character, `tests/e2e/xelatex-filemoddate.tex`,
/// ended in 199 errors and no XDV where xelatex has none.)
#[test]
fn a_written_files_date_is_the_builds_clock() {
    let dir = common::scratch("written-file-date");
    let doc = "\\immediate\\openout1=w.txt \\immediate\\write1{x}\\immediate\\closeout1 \\immediate\\openout2=v \\message{[\\pdffilemoddate{w.txt}][\\pdffilemoddate{v}][\\pdffilemoddate{texput.log}][\\pdfcreationdate]}\\end\n";
    let opts = common::options("./texput.tex", "texput", "", &dir);
    let b = common::build(opts, Some(PRELUDE), doc);
    let (_, log) = b.output();
    let dates: Vec<&str> = log
        .split('[')
        .skip(1)
        .filter_map(|x| x.split(']').next())
        .take(4)
        .collect();
    assert_eq!(dates.len(), 4, "{log}");
    assert!(dates[3].starts_with("D:") && dates[3].len() > 2, "{log}");
    assert_eq!(dates[0], dates[3], "the written file's date:\n{log}");
    assert_eq!(
        dates[1], dates[3],
        "the file opened as `v`, `v.tex`:\n{log}"
    );
    assert_eq!(dates[2], dates[3], "the log's date:\n{log}");
}
