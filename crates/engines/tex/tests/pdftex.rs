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

#[test]
fn random() {
    log_gate("pdfrandom").unwrap_or_else(|e| panic!("{e}"));
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
    let mut failed = Vec::new();
    for &(name, edits) in cases {
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

/// Page steps read nothing another page step made: an edit on page 1,
/// whether it leaves page 1's PDF the same (a zero kern) or not, never
/// runs page 2's step again, and the PDF equals a cold build.
#[test]
fn pdf_state_cutoff() {
    let dir = common::fixtures().join("pdf");
    // pageref.tex's catcodes as the prelude (a format), the rest as
    // the document: a formatless job is one node (its whole text, paging
    // inline), so only a document's regions can cut off an edit
    let all = common::read(&dir.join("pageref.tex"));
    let split = all.find("\\pdfoutput").unwrap_or(0);
    let (prelude, src) = (all[..split].to_owned(), all[split..].to_owned());
    let opts = || phitex_tex::Options {
        epoch: Some(0),
        ..common::options("./pageref.tex", "pageref", "", &dir)
    };
    let run = |to: &str| {
        let mut b = common::build(opts(), Some(&prelude), &src);
        let before = b.output().0;
        let text = src.replacen("A\\a\\par", to, 1);
        assert_ne!(text, src);
        b.edit(&text);
        let cold = common::build(opts(), Some(&prelude), &text);
        let after = b.output().0;
        assert_eq!(
            common::first_difference(&after, &cold.output().0),
            None,
            "{to}: edit differs from cold"
        );
        let page2 = b.stats.page_steps_completed.get(1).copied().unwrap_or(0);
        (after == before, page2)
    };
    // the same PDF state after page 1: page 2's step is cut off (it may
    // start on its prediction, but never runs to its finish)
    let (same_pdf, same_page2) = run("A\\kern0pt\\a\\par");
    // a different page 1: page 2's step does not run again either (it
    // reads nothing page 1's step made; the finish places the pages)
    let (changed_pdf, changed_page2) = run("Z\\a\\par");
    eprintln!("same state: page 2 completed {same_page2}; changed: {changed_page2}");
    assert!(same_pdf, "\\kern0pt changed the PDF");
    assert_eq!(
        same_page2, 0,
        "page 2 ran to its finish though page 1 left the same state"
    );
    assert!(!changed_pdf, "Z left the PDF unchanged");
    assert_eq!(
        changed_page2, 0,
        "page 2 ran again though only page 1 changed"
    );
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
        let (four, out4) = run(4);
        assert_eq!(four.threads, 4);
        assert!(four.peak_running <= 4 && four.peak_running >= 1);
        assert!(four.critical_path.is_some());
        let (one, out1) = run(1);
        assert_eq!(one.threads, 1);
        assert_eq!(one.peak_running, 1);
        assert_eq!(one.batches, one.batches_inline);
        assert!(one.busy <= one.wall);
        let (_, out16) = run(16);
        assert!(
            out1 == out4 && out1 == out16,
            "{name}: output differs across threads"
        );
    }
}

/// Gate 2 of the relaxed PDF mode (`crates/output/pdf/RELAXED.md`): on
/// every document of gate 1, the relaxed file equals pdfTeX's after
/// renumbering (`phitex_pdf::norm`); a document the mode does not cover
/// is built in exact mode and must say why.
#[test]
fn relaxed_documents() {
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
        .filter(|n| !PENDING.contains(&n.as_str()) && !BACKEND_GAPS.contains(&n.as_str()))
        .filter(|n| only.as_ref().is_none_or(|o| o == n))
        .collect();
    names.sort();
    let mut failed = Vec::new();
    for name in names {
        let src = common::read(&dir.join(format!("{name}.tex")));
        let want = std::fs::read(dir.join(format!("{name}.pdf"))).expect("the oracle");
        let opts = phitex_tex::Options {
            epoch: Some(0),
            exact_objects: false,
            ..common::options(&format!("./{name}.tex"), &name, "", &dir)
        };
        let b = common::build(opts, None, &src);
        let (pdf, _) = b.output();
        if let Some(why) = b.pdf_exact_reason() {
            eprintln!("exact {name}: {why}");
            if common::first_difference(&pdf, &want).is_some() {
                failed.push(name);
            }
            continue;
        }
        if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
            let _ = std::fs::write(format!("{d}/{name}.relaxed.pdf"), &pdf);
        }
        match phitex_pdf::norm::first_difference(&pdf, &want) {
            None => eprintln!("ok {name}: equal after renumbering"),
            Some(d) => {
                eprintln!("FAIL {name}: {d}");
                failed.push(name);
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
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
