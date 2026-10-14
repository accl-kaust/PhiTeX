//! Gates for LaTeX: the format made from `latex.ini` (`latex.ltx` with
//! `texsys.cfg`, `hyphen.cfg`, the font configuration) as the prelude,
//! then the documents of `oracle/fixtures/latex/docs` run as `latex`
//! runs them: the DVI file (its preamble's date masked) equal to TeX
//! Live's, on the first run from nothing (`NAME.dvi`) and at the
//! fixpoint of the files read back (`NAME.fix.dvi`); then `pdflatex` on
//! the same documents through the PDF writer
//! (`oracle/fixtures/latex/pdf`), and one-word edits. The files a job
//! writes and reads back (`.aux`, `.toc`, ...) live in memory: the
//! gates compare the output file, never their bytes.
//!
//! Heavy (the format is evaluated once per process): ignored by default;
//! run with `--ignored`, one test at a time, `PHITEX_GATE=name` for one
//! document.

#![allow(clippy::many_single_char_names)]

mod common;

use std::path::{Path, PathBuf};

use phitex_tex::{Build, Options};

fn docs() -> PathBuf {
    common::fixtures().join("latex/docs")
}

/// The DVI file with its preamble's comment masked.
fn masked(dvi: &[u8]) -> Vec<u8> {
    let mut d = dvi.to_vec();
    if d.len() > 15 {
        let k = usize::from(d[14]);
        for b in d.iter_mut().skip(15).take(k) {
            *b = b'.';
        }
    }
    d
}

/// `latex.ini` (or `pdflatex.ini`) as `-ini` reads it.
fn prelude(pdf: bool) -> String {
    if pdf {
        "\\input pdflatex.ini\n".to_owned()
    } else {
        "\\input latex.ini\n".to_owned()
    }
}

fn options(name: &str, dir: &Path, comment: &str, fixpoint: bool, pdf: bool) -> Options {
    Options {
        epoch: Some(0),
        fixpoint,
        prelude_jobname: if pdf { "pdflatex" } else { "latex" }.to_owned(),
        ..common::options(&format!("./{name}.tex"), name, comment, dir)
    }
}

fn report(name: &str, b: &Build) -> String {
    let s = &b.stats;
    let mut out = format!(
        "{name}: {} nodes run, {} runs, {} stops ({} resumed, {} restarted {:?}), {} gated, ready mean {:.2}, {} live, {} rounds, {} loop rounds, {} waits, starved {}, wall {:?}\n  waits by kind {:?}",
        s.nodes_run,
        s.runs,
        s.stops,
        s.resumed,
        s.restarted,
        s.restarted_by,
        s.gated,
        s.ready_mean(),
        s.live,
        s.rounds,
        s.loop_rounds,
        s.pending_waits,
        s.starved,
        s.wall,
        s.waits_by_kind
    );
    let _ = std::fmt::Write::write_fmt(
        &mut out,
        format_args!(
            "\n  critical path {:?}, page steps {}",
            s.critical_path, s.page_steps
        ),
    );
    {
        let mut by = std::collections::BTreeMap::<String, usize>::new();
        for (_, kind, field) in b.critical_chain() {
            *by.entry(format!("{kind} {field}")).or_default() += 1;
        }
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("\n  chain by cause {by:?}"));
    }
    for &(n, w) in &s.top_waited {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("\n  waited on {w}x: {}", b.describe_node(n)),
        );
    }
    out
}

fn names() -> Vec<String> {
    let only = std::env::var("PHITEX_GATE").ok();
    let mut v: Vec<String> = std::fs::read_dir(docs())
        .expect("the documents")
        .flatten()
        .filter_map(|e| {
            let f = e.file_name().to_string_lossy().into_owned();
            f.strip_suffix(".tex").map(str::to_owned)
        })
        .filter(|n| only.as_ref().is_none_or(|o| o == n))
        .collect();
    v.sort();
    v
}

/// One document as `latex` (DVI) runs it: the first run (`fix` false)
/// or the fixpoint.
fn latex_gate(name: &str, fix: bool) -> Result<String, String> {
    let dir = docs();
    let suffix = if fix { ".fix" } else { "" };
    let want = std::fs::read(dir.join(format!("{name}{suffix}.dvi"))).map_err(|e| e.to_string())?;
    let src = common::read(&dir.join(format!("{name}.tex")));
    let opts = options(name, &dir, &common::dvi_comment(&want), fix, false);
    let b = common::build(opts, Some(&prelude(false)), &src);
    let (dvi, log) = b.output();
    if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
        let _ = std::fs::write(format!("{d}/{name}{suffix}.dvi"), &dvi);
        let _ = std::fs::write(format!("{d}/{name}{suffix}.log"), &log);
    }
    let r = report(name, &b);
    let mut bad = Vec::new();
    if let Some(d) = common::first_difference(&masked(&dvi), &masked(&want)) {
        bad.push(format!("dvi {d}"));
    }
    if bad.is_empty() {
        Ok(r)
    } else {
        let tail: String = log
            .chars()
            .rev()
            .take(1500)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Err(format!(
            "{r}\n  FAIL {}\n--- log tail ---\n{tail}",
            bad.join("\n  FAIL ")
        ))
    }
}

fn run_all(fix: bool) {
    let mut failed = Vec::new();
    for n in names() {
        match latex_gate(&n, fix) {
            Ok(r) => eprintln!("ok {r}"),
            Err(r) => {
                eprintln!("{r}");
                failed.push(n);
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

#[test]
#[ignore = "heavy: the LaTeX format"]
fn latex_first_run() {
    run_all(false);
}

#[test]
#[ignore = "heavy: the LaTeX format"]
fn latex_fixpoint() {
    run_all(true);
}

/// `pdflatex` on the documents with a PDF oracle: the PDF file byte for
/// byte.
#[test]
#[ignore = "heavy: the LaTeX format"]
fn pdflatex() {
    let oracles = common::fixtures().join("latex/pdf");
    let only = std::env::var("PHITEX_GATE").ok();
    let mut failed = Vec::new();
    let mut ns: Vec<String> = std::fs::read_dir(&oracles)
        .expect("the oracles")
        .flatten()
        .filter_map(|e| {
            let f = e.file_name().to_string_lossy().into_owned();
            f.strip_suffix(".pdf").map(str::to_owned)
        })
        .filter(|n| only.as_ref().is_none_or(|o| o == n))
        .collect();
    ns.sort();
    for name in ns {
        // (the PDF oracles' own sources first: `docs/` holds the DVI
        // oracles' documents, and a name in both is two documents)
        let pdfdocs = common::fixtures().join("latex/pdfdocs");
        let dir = if pdfdocs.join(format!("{name}.tex")).exists() {
            pdfdocs
        } else {
            docs()
        };
        let want = std::fs::read(oracles.join(format!("{name}.pdf"))).expect("the oracle");
        let src = common::read(&dir.join(format!("{name}.tex")));
        let opts = options(&name, &dir, "", false, true);
        let b = common::build(opts, Some(&prelude(true)), &src);
        let (pdf, log) = b.output();
        if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
            let _ = std::fs::write(format!("{d}/{name}.pdf"), &pdf);
            let _ = std::fs::write(format!("{d}/{name}-pdf.log"), &log);
        }
        let mut bad = Vec::new();
        if let Some(d) = common::first_difference(&pdf, &want) {
            bad.push(format!("pdf {d}"));
        }
        let r = report(&name, &b);
        if bad.is_empty() {
            eprintln!("ok {r}");
        } else {
            eprintln!("{r}\n  FAIL {}", bad.join("\n  FAIL "));
            failed.push(name);
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

/// The byte ranges of the words (runs of ASCII letters not after `\`).
fn words(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_alphabetic()
            && (i == 0 || b[i - 1] != b'\\' && !b[i - 1].is_ascii_alphabetic())
        {
            let s0 = i;
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            if i - s0 >= 3 {
                out.push((s0, i));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Twenty one-word edits on `sections.tex` after `\begin{document}`,
/// each equal to a cold build of the same text, with the counters.
#[test]
#[ignore = "heavy: the LaTeX format"]
fn sections_edits() {
    let dir = docs();
    let src = common::read(&dir.join("sections.tex"));
    let want = std::fs::read(dir.join("sections.fix.dvi")).expect("sections.fix.dvi");
    let comment = common::dvi_comment(&want);
    let pre = prelude(false);
    let opts = || options("sections", &dir, &comment, true, false);
    let t0 = std::time::Instant::now();
    let mut b = common::build(opts(), Some(&pre), &src);
    eprintln!("cold {:?}: {}", t0.elapsed(), report("sections", &b));
    let body = src.find("\\begin{document}").unwrap_or(0) + 16;
    // words of the text, not of commands or environment names
    let ws: Vec<(usize, usize)> = words(&src)
        .into_iter()
        .filter(|&(s, _)| s > body && src.as_bytes()[s - 1] != b'{')
        .collect();
    let n: usize = std::env::var("PHITEX_EDITS")
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(20);
    let mut text = src.clone();
    let mut failed = Vec::new();
    for k in 0..n {
        let (s, e) = ws[(k * 7) % ws.len()];
        let w = text[s..e].to_owned();
        let mut r: Vec<u8> = w.as_bytes().to_vec();
        r.rotate_left(1);
        let new = format!(
            "{}{}{}",
            &text[..s],
            String::from_utf8_lossy(&r),
            &text[e..]
        );
        let t0 = std::time::Instant::now();
        b.edit(&new);
        let dt = t0.elapsed();
        eprintln!(
            "edit {k}: {w:?} -> {:?} {dt:?}: {}",
            String::from_utf8_lossy(&r),
            report("sections", &b)
        );
        text = new;
        let cold = common::build(opts(), Some(&pre), &text);
        let same = b.output().0 == cold.output().0;
        if same {
            eprintln!("  edit {k}: equals cold");
        } else {
            eprintln!(
                "  edit {k}: differs from cold: {:?}",
                common::first_difference(&b.output().0, &cold.output().0)
            );
            failed.push(k);
        }
    }
    assert!(failed.is_empty(), "edits differing from cold: {failed:?}");
}

/// The `etex` job: the format from `etex.ini` (`etex.src`, `language.def`
/// and the patterns, e-TeX mode) as the prelude, then `story.tex`; the
/// DVI file is `etex story.tex`'s (`oracle/fixtures/plain/story-etex.dvi`,
/// TeX Live 2026, `SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1`).
#[test]
#[ignore = "heavy: the e-TeX format"]
fn etex_story() {
    let dir = common::fixtures().join("plain");
    let want = std::fs::read(dir.join("story-etex.dvi")).expect("the oracle");
    let src = common::read(&dir.join("story.tex"));
    let opts = Options {
        epoch: Some(0),
        prelude_jobname: "etex".to_owned(),
        ..common::options("./story.tex", "story", &common::dvi_comment(&want), &dir)
    };
    let b = common::build(opts, Some("\\input etex.ini\n"), &src);
    eprintln!("{}", report("story", &b));
    if let Some(d) = common::first_difference(&masked(&b.output().0), &masked(&want)) {
        panic!("etex story.dvi: {d}");
    }
}

/// `latex` and `pdflatex` end to end on a one-line `article`: the format
/// from TeX Live's `latex.ini` (`pdflatex.ini`) through the kpathsea
/// finder, then the document: one page, the run not panicking.
fn hello(pdf: bool) -> Build {
    let dir = common::fixtures().join("latex/hello");
    let src = common::read(&dir.join("hello.tex"));
    let want = std::fs::read(dir.join("hello.dvi")).expect("the oracle");
    let comment = if pdf {
        String::new()
    } else {
        common::dvi_comment(&want)
    };
    // the format, kept between runs (the sandbox's $HOME is empty)
    let cache = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/cache")
        .join(if pdf { "pdflatex.fmt" } else { "latex.fmt" });
    let opts = || Options {
        prelude_bytes: std::fs::read(&cache).ok(),
        // the format takes longer than the default limit in a debug build
        time_limit: None,
        ..options("hello", &dir, &comment, false, pdf)
    };
    let cached = cache.exists();
    let t0 = std::time::Instant::now();
    let b = common::build(opts(), Some(&prelude(pdf)), &src);
    eprintln!(
        "{} {:?}: {}",
        if cached { "cached format" } else { "cold" },
        t0.elapsed(),
        report("hello", &b)
    );
    if !cached
        && b.diagnostics()
            .iter()
            .all(|d| !d.message.starts_with("evaluation"))
    {
        let _ = std::fs::create_dir_all(cache.parent().expect("dir"));
        if let Some(bytes) = b.export_prelude() {
            let _ = std::fs::write(&cache, bytes);
        }
    }
    let (out, log) = b.output();
    eprintln!("{} bytes, {} pages\n{log}", out.len(), b.pages().len());
    for d in b.diagnostics() {
        eprintln!("{d:?}");
    }
    let o = std::env::var("PHITEX_GATE_OUT").ok();
    if let Some(o) = o {
        let _ = std::fs::write(
            format!("{o}/hello.{}", if pdf { "pdf" } else { "dvi" }),
            &out,
        );
    }
    if !pdf {
        eprintln!(
            "first difference: {:?}",
            common::first_difference(&masked(&out), &masked(&want))
        );
    } else if let Ok(w) = std::fs::read(dir.join("hello.pdf")) {
        eprintln!("first difference: {:?}", common::first_difference(&out, &w));
    }
    b
}

#[test]
#[ignore = "heavy: the LaTeX format; expl3-code.tex loading is being measured"]
fn latex_hello() {
    let b = hello(false);
    assert_eq!(b.pages().len(), 1);
    assert!(!b.is_pdf());
}

#[test]
#[ignore = "heavy: the LaTeX format; expl3-code.tex loading is being measured"]
fn pdflatex_hello() {
    let b = hello(true);
    assert_eq!(b.pages().len(), 1);
    assert!(b.is_pdf());
}

/// Loop-back rounds of `hello.tex` and of a document with a `\label`,
/// cold and after a one-word edit, without and with the fixpoint
/// (measurement; prints only).
#[test]
#[ignore = "heavy: the LaTeX format"]
fn latex_loop_rounds() {
    let dir = common::fixtures().join("latex/hello");
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/cache/latex.fmt");
    let hello = common::read(&dir.join("hello.tex"));
    let label = "\\documentclass{article}\\begin{document}\\section{One}\\label{one}See section~\\ref{one} on page~\\pageref{one}.\\end{document}\n";
    for (name, src) in [("hello", hello.as_str()), ("label", label)] {
        for fixpoint in [false, true] {
            let opts = Options {
                prelude_bytes: std::fs::read(&cache).ok(),
                time_limit: None,
                ..options("hello", &dir, "", fixpoint, false)
            };
            let mut b = common::build(opts, Some(&prelude(false)), src);
            if !cache.exists()
                && let Some(bytes) = b.export_prelude()
            {
                let _ = std::fs::create_dir_all(cache.parent().expect("dir"));
                let _ = std::fs::write(&cache, bytes);
            }
            let cold = (b.stats.loop_rounds, b.stats.rounds, b.stats.nodes_run);
            let keys = b.wkeys();
            if fixpoint {
                // the same without the key-set guess (an empty guess)
                let o3 = Options {
                    prelude_bytes: std::fs::read(&cache).ok(),
                    time_limit: None,
                    wkeys: vec![("hello.aux".to_owned(), Vec::new())],
                    ..options("hello", &dir, "", fixpoint, false)
                };
                let b3 = common::build(o3, Some(&prelude(false)), src);
                eprintln!(
                    "fixpoint {name} unguessed: {} loop rounds, {} evaluations, {} nodes run",
                    b3.stats.loop_rounds, b3.stats.rounds, b3.stats.nodes_run
                );
                assert_eq!(b.output().0, b3.output().0, "the guess changed the output");
                let (out, log) = b.output();
                eprintln!(
                    "fixpoint {name}: {} bytes out, {:?}, log tail {:?}",
                    out.len(),
                    b.diagnostics(),
                    &log[log.len().saturating_sub(400)..]
                );
            }
            b.edit(&src.replacen("\\begin{document}", "\\begin{document}Hi ", 1));
            eprintln!(
                "rounds {name} fixpoint={fixpoint}: cold {} loop rounds / {} evaluations / {} nodes run, edit {} / {} / {}; wkeys {keys:?}; guess {:?}",
                cold.0,
                cold.1,
                cold.2,
                b.stats.loop_rounds,
                b.stats.rounds,
                b.stats.nodes_run,
                phitex_tex::guess_wkeys(src)
            );
        }
    }
}

/// `pdflatex` hello byte-identical to the oracle, cold and from the
/// format: the format carries `\pdfglyphtounicode`'s table
/// (`dumptounicode`/`undumptounicode`, pdfTeX §1502–§1503), so the
/// font's `/ToUnicode` `CMap` is written after a format load too.
#[test]
#[ignore = "heavy: the LaTeX format"]
fn pdflatex_hello_tounicode() {
    let want = std::fs::read(common::fixtures().join("latex/hello/hello.pdf")).expect("the oracle");
    for _ in 0..2 {
        let b = hello(true);
        let (out, _) = b.output();
        assert!(out == want, "{:?}", common::first_difference(&out, &want));
    }
}

/// The `pdflatex` format, cached under `target/cache` across tests.
fn pdf_cache() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/cache/pdflatex.fmt")
}

/// A `pdfdocs` document built cold from `dir` (the format cached):
/// byte-identical to its oracle, its document nodes counted.
fn cold_nodes(name: &str, dir: &Path, src: &str) -> (Build, u64) {
    let want = std::fs::read(common::fixtures().join(format!("latex/pdf/{name}.pdf")))
        .expect("the oracle");
    let cache = pdf_cache();
    let cached = cache.exists();
    let b = common::build(pdf_opts(name, dir), Some(&prelude(true)), src);
    if !cached {
        let _ = std::fs::create_dir_all(cache.parent().expect("dir"));
        if let Some(bytes) = b.export_prelude() {
            let _ = std::fs::write(&cache, bytes);
        }
    }
    let regions = b.stats.nodes_text_frames;
    eprintln!(
        "cold {name}: {regions} document nodes, {} pages; {}",
        b.pages().len(),
        report(name, &b)
    );
    if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
        let _ = std::fs::write(format!("{d}/{name}.pdf"), b.output().0);
        let _ = std::fs::write(format!("{d}/{name}.log"), b.output().1);
    }
    let chain = b.critical_chain();
    let mut by: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for (_, kind, field) in &chain {
        let f = field.split(':').next().unwrap_or("");
        *by.entry(format!("{kind} {f}:")).or_default() += 1;
    }
    let everypar = chain.iter().filter(|c| c.2 == "param:everypar").count();
    let s = &b.stats;
    eprintln!(
        "chains {name}: critical path {:?}, realised cp {}, {} nodes run, {} regions, {} rounds, {} loop rounds, {} undeclared, {} violations, {} mispredictions, everypar links {everypar}, chain {} links {by:?}",
        s.critical_path,
        s.realised_critical_path,
        s.nodes_run,
        s.nodes_text_frames,
        s.rounds,
        s.loop_rounds,
        s.undeclared_definitions,
        s.regime_violations,
        s.mispredictions,
        chain.len()
    );
    if std::env::var_os("PHITEX_CHAIN").is_some() {
        for l in b.critical_chain_lines() {
            eprintln!("  chain {name}: {l}");
        }
    }
    for d in b.diagnostics() {
        eprintln!("{d:?}");
    }
    let reads: Vec<_> = b
        .reads()
        .into_iter()
        .filter(|(n, _)| !n.to_ascii_lowercase().ends_with(".tfm"))
        .collect();
    eprintln!("reads {name}: {reads:?}");
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("{name}.pdf cold: {d}");
    }
    assert_eq!(
        b.stats.undeclared_definitions, 0,
        "{name}: undeclared definitions"
    );
    (b, regions)
}

fn pdf_opts(name: &str, dir: &Path) -> Options {
    Options {
        prelude_bytes: std::fs::read(pdf_cache()).ok(),
        time_limit: Some(std::time::Duration::from_secs(
            std::env::var("PHITEX_TEST_LIMIT")
                .ok()
                .and_then(|x| x.parse().ok())
                .unwrap_or(120),
        )),
        ..options(name, dir, "", false, true)
    }
}

/// After an edit: the build equals a cold one of the new text, and it
/// ran at most `k` document nodes besides the page fold, the page
/// steps and the writer.
fn edit_check(name: &str, b: &Build, cold: &Build, k: u64) {
    let pages = b.pages().len() as u64;
    eprintln!(
        "edit {name}: {} nodes run, {} runs, {pages} pages; {}",
        b.stats.nodes_run,
        b.stats.runs,
        report(name, b)
    );
    if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
        panic!("{name}: edit differs from cold: {d}");
    }
    assert!(
        b.stats.nodes_run <= k + pages + 2,
        "{name}: edit reran {} nodes ({pages} pages)",
        b.stats.nodes_run
    );
}


/// The paragraphs of a LaTeX body: its blank-line-separated chunks
/// between `\begin{document}` and `\end{document}`.
fn paragraphs(src: &str) -> u64 {
    let a = src.find("\\begin{document}").map_or(0, |a| a + 16);
    let e = src.find("\\end{document}").unwrap_or(src.len());
    src[a..e]
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .count() as u64
}

/// The text with the five bytes after `marker` replaced by `quack`.
fn quack(src: &str, marker: &str) -> String {
    let at = src.find(marker).expect("the marker") + marker.len();
    format!("{}quack{}", &src[..at], &src[at + 5..])
}

/// `pdflatex` on hello + 30 paragraphs (`pdfdocs/paras30.tex`): the
/// document environment's body is sliced into its paragraphs, one node
/// each (`\begin{document}`'s region leaves its group to the engine as
/// the scope they run in); the PDF equals pdfTeX's cold and after a
/// one-word edit, which reruns a few document nodes.
#[test]
fn latex_paragraph_nodes() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("paras30.tex"));
    let (mut b, regions) = cold_nodes("paras30", &dir, &src);
    assert!(regions >= 30, "{regions} document nodes for 30 paragraphs");
    let new = quack(&src, "Paragraph 17 of the text: the quick ");
    b.edit(&new);
    let cold = common::build(pdf_opts("paras30", &dir), Some(&prelude(true)), &new);
    edit_check("paras30", &b, &cold, 3);
}

/// `pdflatex` on an article with an abstract, two sections, itemize,
/// enumerate, a theorem and a proof (amsthm), a figure with a caption
/// and a quotation (`pdfdocs/envs.tex`, 4 pages): every paragraph end
/// is a cut, inside environments too (an environment's `\begin` leaves
/// its group to the engine as a scope, its `\end` closes it); the PDF
/// equals pdfTeX's cold and after an edit inside the itemize.
#[test]
fn latex_environments() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("envs.tex"));
    let (mut b, regions) = cold_nodes("envs", &dir, &src);
    let pars = paragraphs(&src);
    eprintln!("envs: {regions} document nodes for {pars} paragraphs");
    assert!(
        regions >= pars,
        "{regions} document nodes for {pars} paragraphs"
    );
    let new = quack(&src, "Item 3 of the list: the quick ");
    b.edit(&new);
    let cold = common::build(pdf_opts("envs", &dir), Some(&prelude(true)), &new);
    edit_check("envs", &b, &cold, 4);
}

/// `pdflatex` on a book whose chapters and section are followed by
/// paragraphs, right after the heading's line and after a blank line
/// (`pdfdocs/headings.tex`, three pages, `\chapter{B}` on page 3 after
/// the blank page `openright` leaves): the PDF equals pdfTeX's, with no
/// undeclared definition (a heading's `\@afterheading` leaves a token
/// list in `\everypar` that the next paragraph's start runs, §1091).
#[test]
#[ignore = "red: 2 pages instead of 3 (the openright blank page), PDF differs at byte 470; and undeclared cs:if@nobreak from \\everypar"]
fn latex_heading_everypar() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("headings.tex"));
    let (b, _) = cold_nodes("headings", &dir, &src);
    assert_eq!(b.stats.undeclared_definitions, 0, "undeclared definitions");
    assert_eq!(b.stats.regime_violations, 0, "regime violations");
}

/// `pdflatex` on an article that writes a part file the way
/// `\include`'s `\@writeckpt` does, one `\@setckpt{i1-a}{` line, a
/// `\setcounter` line, a `}` line (`pdfdocs/writecont.tex`), and puts
/// `\@input{i1-a.aux}` in the main `.aux`: the file is read back whole,
/// its first line's argument going on in the next lines, and the next
/// run's `\@input` defines `\cp@i1-a`; the PDF equals pdfTeX's.
#[test]
fn latex_write_continued_lines() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("writecont.tex"));
    let want =
        std::fs::read(common::fixtures().join("latex/pdf/writecont.pdf")).expect("the oracle");
    let opts = Options {
        fixpoint: true,
        ..pdf_opts("writecont", &dir)
    };
    let b = common::build(opts, Some(&prelude(true)), &src);
    eprintln!("writecont: {}", report("writecont", &b));
    if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
        let _ = std::fs::write(format!("{d}/writecont.pdf"), b.output().0);
    }
    let errors: Vec<String> = b
        .diagnostics()
        .iter()
        .filter(|d| d.severity == phitex_tex::Severity::Error)
        .map(|d| d.message.clone())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("writecont.pdf: {d}");
    }
}

/// `pdflatex` on an article whose part file's last line leaves an
/// argument open (`pdfdocs/wopen.tex`: `\@setckpt{wopen-a}{` and a
/// `\setcounter` line, no `}`): each read-back of the file ends inside
/// it, §338 "File ended while scanning ..." with what it inserts (at
/// `\begin{document}` a `}`, the definition made; at `\enddocument` a
/// `\par`, the call dropped): pdflatex's two errors of its later passes;
/// the PDF equals pdfTeX's.
#[test]
fn latex_write_file_ended() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("wopen.tex"));
    let want = std::fs::read(common::fixtures().join("latex/pdf/wopen.pdf")).expect("the oracle");
    let opts = Options {
        fixpoint: true,
        ..pdf_opts("wopen", &dir)
    };
    let b = common::build(opts, Some(&prelude(true)), &src);
    eprintln!("wopen: {}", report("wopen", &b));
    let mut errors: Vec<String> = b
        .diagnostics()
        .iter()
        .filter(|d| d.severity == phitex_tex::Severity::Error)
        .map(|d| d.message.clone())
        .collect();
    // (pdflatex's later passes: `\begin{document}`'s read-back ends in
    // `\@setckpt`'s definition, `\enddocument`'s in its use)
    errors.sort();
    assert_eq!(
        errors,
        [
            "File ended while scanning definition of \\cp@wopen-a",
            "File ended while scanning use of \\@setckpt",
        ],
        "pdflatex's two errors"
    );
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("wopen.pdf: {d}");
    }
}

/// `pdflatex` with hyperref on an article with a table of contents and
/// fifteen cross-reference links: the `.aux` read-back defines only
/// what reading the file defines (the labels, the contents' lines),
/// never the writer's state the interpretation of a line touches
/// (`pdf:lastlink`, `pdf:state`, `pg:`, `box:`, parameters); the build
/// settles in at most two evaluations.
#[test]
fn latex_links_aux_defines_only() {
    use std::fmt::Write as _;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("latex-links-aux");
    let _ = std::fs::create_dir_all(&dir);
    let mut src = String::from(
        "\\documentclass{article}\n\\usepackage{hyperref}\n\\begin{document}\n\\tableofcontents\n",
    );
    for k in 0..15 {
        let _ = writeln!(
            src,
            "\\section{{S{k}}}\\label{{s{k}}} See section~\\ref{{s{}}} on page~\\pageref{{s{}}}.\n",
            (k + 1) % 15,
            (k + 7) % 15
        );
    }
    src.push_str("\\end{document}\n");
    let opts = Options {
        fixpoint: true,
        ..pdf_opts("links", &dir)
    };
    let b = common::build(opts, Some(&prelude(true)), &src);
    eprintln!("links: {}", report("links", &b));
    let keys: Vec<String> = b.wkeys().into_iter().flat_map(|(_, ks)| ks).collect();
    let bad: Vec<&String> = keys
        .iter()
        .filter(|k| {
            let f = k.rsplit('\u{1}').next().unwrap_or(k);
            [
                "pdf:",
                "pg:",
                "aftertoken",
                "cat:",
                "parshape",
                "param:everypar",
            ]
            .iter()
            .any(|p| f.starts_with(p))
        })
        .collect();
    assert!(
        bad.is_empty(),
        "read-back keys besides definitions: {bad:?}"
    );
    assert!(
        b.stats.rounds <= 2,
        "{} evaluations ({} loop rounds)",
        b.stats.rounds,
        b.stats.loop_rounds
    );
}

/// `pdflatex` on a main file that `\input`s one chapter and
/// `\include`s another (`pdfdocs/inputs.tex`, `chap1.tex`,
/// `chap2.tex`; `\include` goes through `\@input` and its own `.aux`):
/// the chapters' paragraphs are regions; the PDF equals pdfTeX's cold
/// and after an edit in `chap1.tex`.
#[test]
fn latex_input_include() {
    let src_dir = common::fixtures().join("latex/pdfdocs");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("latex-input-include");
    let _ = std::fs::create_dir_all(&dir);
    for f in ["inputs.tex", "chap1.tex", "chap2.tex"] {
        std::fs::copy(src_dir.join(f), dir.join(f)).expect("copy");
    }
    let src = common::read(&dir.join("inputs.tex"));
    let (mut b, regions) = cold_nodes("inputs", &dir, &src);
    eprintln!("inputs: {regions} document nodes for 20 chapter paragraphs");
    assert!(regions >= 10, "{regions} document nodes: chap1 not sliced");
    let chap1 = common::read(&dir.join("chap1.tex"));
    let new = quack(&chap1, "Paragraph 5 of chapter one: the quick ");
    b.edit_file("chap1.tex", Some(new.as_bytes()));
    std::fs::write(dir.join("chap1.tex"), &new).expect("write");
    let cold = common::build(pdf_opts("inputs", &dir), Some(&prelude(true)), &src);
    let _ = std::fs::write(dir.join("chap1.tex"), &chap1);
    edit_check("inputs", &b, &cold, 4);
}

/// An article with one paragraph between blank lines (the ladder's
/// red-02): LaTeX's paragraph hook resets `\everypar` to its regime
/// guess, no definition; byte-identical, the `.aux` too, no undeclared
/// definition, and an edit equals a cold build.
#[test]
fn latex_one_paragraph() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("red02.tex"));
    let (mut b, _) = cold_nodes("red02", &dir, &src);
    let new = src.replace("Hello", "Hallo");
    b.edit(&new);
    let cold = common::build(pdf_opts("red02", &dir), Some(&prelude(true)), &new);
    edit_check("red02", &b, &cold, 3);
}

/// A heading and three paragraphs (`pdfdocs/secpars.tex`): the first
/// consumes `\@afterheading`'s `\everypar` and resets it, the second and
/// third are indented as pdfTeX indents them (the reset is a declared
/// definition, never dropped); an edit in the first equals a cold build.
#[test]
fn latex_heading_paragraphs() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("secpars.tex"));
    let (mut b, _) = cold_nodes("secpars", &dir, &src);
    let new = quack(&src, "The first paragraph after the heading: the ");
    b.edit(&new);
    let cold = common::build(pdf_opts("secpars", &dir), Some(&prelude(true)), &new);
    edit_check("secpars", &b, &cold, 3);
}

/// The chain census gate (`common::Census`) on a `pdflatex` document:
/// hello + 30 paragraphs, cold and after a one-word edit.
#[test]
fn census_latex() {
    census_of("paras30", "Paragraph 17 of the text: the quick ");
}

/// The chain census gate on the environments article (its cold build
/// is `latex_environments`'s).
#[test]
fn census_envs() {
    census_of("envs", "Item 3 of the list: the quick ");
}

fn census_of(name: &str, marker: &str) {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join(format!("{name}.tex")));
    let (mut b, _) = cold_nodes(name, &dir, &src);
    let new = quack(&src, marker);
    common::assert_census(&[common::Census::take(name, &mut b, &new)]);
}

/// `pdflatex` on UTF-8 text (`pdfdocs/utf8.tex`: `inputenc`, T1,
/// multi-byte words at paragraph ends and in the edited word): the
/// text's bytes are its characters, every cut and excerpt of it on a
/// character boundary; the PDF equals pdfTeX's cold and after an edit
/// of a multi-byte word.
#[test]
fn latex_utf8_edit() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("utf8.tex"));
    let (mut b, regions) = cold_nodes("utf8", &dir, &src);
    assert!(regions >= 8, "{regions} document nodes for 8 paragraphs");
    // (the text's characters are its bytes: a UTF-8 word as `read` gives it)
    let bytes = |w: &str| -> String { w.bytes().map(char::from).collect() };
    let new = src.replacen(&bytes("crème"), &bytes("crêpe"), 1);
    assert_ne!(new, src, "the edited word is in the text");
    b.edit(&new);
    let cold = common::build(pdf_opts("utf8", &dir), Some(&prelude(true)), &new);
    edit_check("utf8", &b, &cold, 3);
}
