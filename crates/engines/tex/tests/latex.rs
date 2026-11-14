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
mod sweep;

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
        "{name}: {} nodes run, {} runs, {} stops ({} resumed, {} restarted {:?}), {} live, {} rounds, {} loop rounds, {} waits, starved {}, wall {:?}",
        s.nodes_run,
        s.runs,
        s.stops,
        s.resumed,
        s.restarted,
        s.restarted_by,
        s.live,
        s.rounds,
        s.loop_rounds,
        s.pending_waits,
        s.starved,
        s.wall
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
                "rounds {name} fixpoint={fixpoint}: cold {} loop rounds / {} evaluations / {} nodes run, edit {} / {} / {}; wkeys {keys:?}",
                cold.0, cold.1, cold.2, b.stats.loop_rounds, b.stats.rounds, b.stats.nodes_run,
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
        "cold {name}: {regions} document nodes, {} paragraph nodes, {} paragraphs taken, {} scopes refused (a field at both levels), {} pages; {}",
        b.stats.nodes_paragraphs,
        b.stats.paragraphs_taken,
        b.stats.scope_both_levels,
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
        "chains {name}: critical path {:?}, realised cp {}, {} nodes run, {} regions, {} rounds, {} loop rounds, {} undeclared, guesses refuted: {} claims, {} effects, {} claim siblings, {} definitions, {} violations, {} mispredictions, everypar links {everypar}, chain {} links {by:?}",
        s.critical_path,
        s.realised_critical_path,
        s.nodes_run,
        s.nodes_text_frames,
        s.rounds,
        s.loop_rounds,
        s.undeclared_definitions,
        s.claims_refuted,
        s.effects_refuted,
        s.claim_siblings,
        s.defs_refuted,
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
    // (a definition outside a node's static list is a guess the engine
    // widens, `defs_refuted` above; only the engine's own error, outside
    // its `may_define` bound, fails)
    assert_eq!(
        b.stats.undeclared_definitions, 0,
        "{name}: undeclared definitions (engine errors)"
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

/// After a one-word edit (one paragraph touched): the build equals a
/// cold one of the new text, and what re-ran is the node model's
/// (`common::assert_edit_cost`) for the pages whose content changed
/// from `before` (the page hashes before the edit).
fn edit_check(name: &str, b: &Build, cold: &Build, before: &[u64]) {
    let pages = b.pages().len() as u64;
    eprintln!(
        "edit {name}: {} nodes run, {} runs, {pages} pages; {}",
        b.stats.nodes_run,
        b.stats.runs,
        report(name, b)
    );
    if let Ok(dir) = std::env::var("PHITEX_TEST_OUT") {
        let _ = std::fs::write(format!("{dir}/{name}-edit.pdf"), b.output().0);
        let _ = std::fs::write(format!("{dir}/{name}-cold.pdf"), cold.output().0);
    }
    if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
        panic!("{name}: edit differs from cold: {d}");
    }
    common::assert_edit_cost(name, b, 1, before);
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
    let before = b.pages();
    b.edit(&new);
    let cold = common::build(pdf_opts("paras30", &dir), Some(&prelude(true)), &new);
    edit_check("paras30", &b, &cold, &before);
}

/// `pdflatex` on articles of 20 and 28 paragraphs, each with a
/// `\marginpar` (`pdfdocs/marginpars20.tex`, `marginpars28.tex`, 2 and 3
/// pages): each marginpar takes two boxes from `\@freelist` in the text,
/// and its routine, fired at `\penalty-10002`, gives them back; the next
/// marginpar's text reads the list that routine left, a serial chain
/// through the page layer (docs/spec-settle.md §2): held to quiet points,
/// a pass advanced one marginpar and the 12 passes ran out ('the page
/// layout did not settle', 'Float(s) lost'). The PDF equals pdfTeX's at
/// -j1 and -j8, settled.
#[test]
fn latex_marginpars_settle() {
    let dir = common::fixtures().join("latex/pdfdocs");
    for name in ["marginpars20", "marginpars28"] {
        let src = common::read(&dir.join(format!("{name}.tex")));
        let want = std::fs::read(common::fixtures().join(format!("latex/pdf/{name}.pdf")))
            .expect("the oracle");
        for threads in [1, 8] {
            let opts = Options {
                threads: Some(threads),
                ..pdf_opts(name, &dir)
            };
            let b = common::build(opts, Some(&prelude(true)), &src);
            let (pdf, log) = b.output();
            let bad: Vec<String> = b
                .diagnostics()
                .into_iter()
                .map(|d| d.message)
                .filter(|m| m.contains("did not settle") || m.contains("Float(s) lost"))
                .collect();
            eprintln!("{name} -j {threads}: {}", report(name, &b));
            assert!(
                bad.is_empty() && !log.contains("Float(s) lost"),
                "{name} -j {threads}: {bad:?}"
            );
            if let Some(d) = common::first_difference(&pdf, &want) {
                panic!("{name} -j {threads}: pdf {d}");
            }
        }
    }
}

/// `pdflatex` on thirty pages made by one loop, each ended by `\newpage`
/// and each beginning by reading a counter the shipout hook stepped
/// (`pdfdocs/newpages20.tex`, as thumbs' overview does; the hook loops
/// a while, so the text runs ahead of the routines): a serial chain
/// from the text to the routine and back through a page fired at
/// `\penalty-10000`, the text's own break. The finisher of such a page
/// re-asks the text at once (docs/spec-settle.md §2); held to the quiet
/// point, each pass advanced one page, and a text that speculated wrong
/// early ran out the 12 passes ('the page layout did not settle', a
/// page count off: thumbs-example -j4, 2 to 6 builds in 24). The PDF
/// equals pdfTeX's at -j1, -j4 and -j8, settled, in each of three
/// builds.
#[test]
fn latex_newpage_chain_settles() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let name = "newpages20";
    let src = common::read(&dir.join(format!("{name}.tex")));
    let want = std::fs::read(common::fixtures().join(format!("latex/pdf/{name}.pdf")))
        .expect("the oracle");
    for threads in [1, 4, 8] {
        for k in 0..3 {
            let opts = Options {
                threads: Some(threads),
                ..pdf_opts(name, &dir)
            };
            let b = common::build(opts, Some(&prelude(true)), &src);
            let (pdf, _) = b.output();
            let bad: Vec<String> = b
                .diagnostics()
                .into_iter()
                .map(|d| d.message)
                .filter(|m| m.contains("did not settle"))
                .collect();
            eprintln!("{name} -j {threads} build {k}: {}", report(name, &b));
            assert!(bad.is_empty(), "{name} -j {threads} build {k}: {bad:?}");
            if let Some(d) = common::first_difference(&pdf, &want) {
                panic!("{name} -j {threads} build {k}: pdf {d}");
            }
        }
    }
}

/// `pdflatex` on a two-column article with 56 marginpars, 19 figures, 5
/// wide figures and 7 tables (`pdfdocs/floats2col.tex`): LaTeX's output
/// routine gives back floats, `\@freelist` and the column box at every
/// fire, and the page loop must not re-fire every later page after each
/// give-back it guessed wrong (Grail's -j1 that never ended: the loop
/// stepping one checkpoint a run, re-running every later finisher). The
/// finishers' runs stay within twice the fires at -j1, three times at
/// -j8, and the PDF is pdfTeX's.
#[test]
#[ignore = "the page loop re-fires every later page after a give-back miss: finisher runs grow with the square of the fires"]
fn latex_twocol_floats_runs() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let name = "floats2col";
    let src = common::read(&dir.join(format!("{name}.tex")));
    let want = std::fs::read(common::fixtures().join(format!("latex/pdf/{name}.pdf")))
        .expect("the oracle");
    let mut bad = Vec::new();
    for (threads, bound) in [(1u64, 2u64), (8, 3)] {
        let opts = Options {
            threads: Some(usize::try_from(threads).unwrap_or(1)),
            ..pdf_opts(name, &dir)
        };
        let t0 = std::time::Instant::now();
        let b = common::build(opts, Some(&prelude(true)), &src);
        let wall = t0.elapsed();
        let (pdf, _) = b.output();
        let fires = b.live_by_op().get("finish").copied().unwrap_or(0);
        let by = |op: &str| {
            b.stats
                .runs_by_op
                .iter()
                .find(|x| x.0 == op)
                .map_or(0, |x| x.1)
        };
        eprintln!(
            "{name} -j {threads}: {fires} fires, {} finisher runs, {} page loop runs, {} pages, wall {wall:?}\n  {}",
            by("finish"),
            by("pages"),
            b.pages().len(),
            report(name, &b)
        );
        if let Some(d) = common::first_difference(&pdf, &want) {
            bad.push(format!("-j {threads}: pdf {d}"));
        }
        if by("finish") > bound * fires {
            bad.push(format!(
                "-j {threads}: {} finisher runs for {fires} fires (bound {bound}x)",
                by("finish")
            ));
        }
    }
    assert!(bad.is_empty(), "{name}: {bad:?}");
}

/// At one thread every node runs in TeX's order, the page built after each
/// paragraph and a routine run before the text after its page, so no run
/// is thrown away: no node is refuted, and every run past a node's first
/// is a resumption (`Stats::runs` equals `Stats::nodes_run` plus
/// `Stats::resumptions`): a page loop node parked on a contributor not in
/// yet, or a give-back or a start not made yet, woken by it and going on
/// from where it stopped, none of its work done again, as TeX's builder
/// goes on after each paragraph; each resumption pairs with a park
/// (`Stats::resumptions` at most `Stats::parks`). A second run of a node at
/// -j1 that is not one is a run on a guess where waiting for the node
/// before it gave the true value: a false dependency. On floats2col (194
/// routine fires, floats and marginpars handing `\@freelist` back to the
/// text), marginpars28 and tikz; the PDF is pdfTeX's.
#[test]
fn at_one_thread_no_run_is_refuted() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let mut bad = Vec::new();
    for name in ["floats2col", "marginpars28", "tikz"] {
        let src = common::read(&dir.join(format!("{name}.tex")));
        let want = std::fs::read(common::fixtures().join(format!("latex/pdf/{name}.pdf")))
            .expect("the oracle");
        let opts = Options {
            threads: Some(1),
            ..pdf_opts(name, &dir)
        };
        let b = common::build(opts, Some(&prelude(true)), &src);
        let (pdf, _) = b.output();
        let s = &b.stats;
        eprintln!(
            "{name} -j 1: {} runs of {} nodes run, {} refuted, {} resumed of {} parks, by op {:?}",
            s.runs, s.nodes_run, s.refuted_nodes, s.resumptions, s.parks, s.runs_by_op
        );
        if let Some(d) = common::first_difference(&pdf, &want) {
            bad.push(format!("{name}: pdf {d}"));
        }
        if s.refuted_nodes != 0 || s.runs != s.nodes_run + s.resumptions || s.resumptions > s.parks
        {
            bad.push(format!(
                "{name}: {} runs of {} nodes, {} refuted, {} resumed of {} parks ({} thrown away)",
                s.runs,
                s.nodes_run,
                s.refuted_nodes,
                s.resumptions,
                s.parks,
                s.runs.saturating_sub(s.nodes_run + s.resumptions)
            ));
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

/// A picture child that refuses (a read-time object) after a paragraph
/// of its piece broken with font expansion, whose appends to the fonts'
/// expansion lists it started with: it drops them with its table, and
/// the empty control sequence that `\csname\endcsname` makes `\relax`
/// afterwards is that name's, defined in the pieces after it, at one
/// thread and at eight (a refused child kept them, and its publication
/// put the empty name's field at a spill location for the thread:
/// beamertheme-arguelles' `\pdfprotrudechars` made `\relax`).
#[test]
#[ignore = "heavy: the LaTeX format"]
fn a_refused_picture_child_leaves_the_empty_name_its_own() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let name = "picrefuse";
    let src = common::read(&dir.join(format!("{name}.tex")));
    for threads in [1usize, 8] {
        let opts = Options {
            threads: Some(threads),
            ..pdf_opts(name, &dir)
        };
        let b = common::build(opts, Some(&prelude(true)), &src);
        let (_, log) = b.output();
        assert!(
            log.contains("[counts 4 5 6]"),
            "-j {threads}: the registers past 255 the picture's paragraph set keep their values: {}",
            log.lines()
                .find(|l| l.contains("[counts"))
                .unwrap_or("no message")
        );
        assert!(
            log.contains("[counts 7 8 9]"),
            "-j {threads}: the registers past 255 the paragraph set keep their values: {}",
            log.lines()
                .find(|l| l.contains("[counts"))
                .unwrap_or("no message")
        );
        assert!(
            log.contains("[empty defined]"),
            "-j {threads}: the empty name is defined after \\csname\\endcsname, as TeX has it"
        );
    }
}

/// A picture after a page fired inside conditionals of its piece's own
/// (`pdfdocs/picroutine.tex`: a heading's spacing and break inside
/// `\ifnum`, the picture in the same expansion): the page's routine draws
/// two pictures in the shipout hook, stepping the picture counter of `tikz`,
/// and the piece goes on past the candidate with the routine's values put
/// in force there. The picture child reads them as its parent holds them
/// (it read the counter from before the routine: beautybook-en's
/// `pgfid59` twice, its page 19 frame drawn from page 14's position). The
/// PDF is pdfTeX's at -j1, -j4 and -j8.
#[test]
#[ignore = "heavy: the LaTeX format"]
fn a_picture_after_a_page_fired_in_its_piece_sees_the_routines_values() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let name = "picroutine";
    let src = common::read(&dir.join(format!("{name}.tex")));
    let want = std::fs::read(common::fixtures().join(format!("latex/pdf/{name}.pdf")))
        .expect("the oracle");
    for threads in [1usize, 4, 8] {
        let opts = Options {
            threads: Some(threads),
            ..pdf_opts(name, &dir)
        };
        let b = common::build(opts, Some(&prelude(true)), &src);
        let (pdf, _) = b.output();
        if let Some(d) = common::first_difference(&pdf, &want) {
            panic!("{name} -j {threads}: pdf {d}");
        }
    }
}

/// A `\csname` built from a stepped counter taken as a macro's argument
/// (`pdfdocs/dnamearg.tex`: an `edichokey` key with hyperref, each lead
/// linked by `\hyperref[name]`, the name built from the couplet counter;
/// hyperref's `\hyperref[x]` is `\expandafter\label@@hyperref\csname
/// r@x\endcsname{x}`): the argument is the one control sequence, as TeX
/// takes it. At -j4 the derived name's token was expanded in the
/// argument, so `\label@@hyperref` got `\r@x`'s body: a hyper reference
/// named by a page number undefined, the anchor's name typeset, the links
/// lost (edichokey-ex in every -j4 build). The PDF is pdfTeX's at -j1, -j4 and -j8.
#[test]
fn a_name_built_from_a_counter_is_one_token_as_an_argument() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let name = "dnamearg";
    let src = common::read(&dir.join(format!("{name}.tex")));
    let want = std::fs::read(common::fixtures().join(format!("latex/pdf/{name}.pdf")))
        .expect("the oracle");
    for threads in [1usize, 4, 8] {
        let opts = Options {
            threads: Some(threads),
            fixpoint: true,
            ..pdf_opts(name, &dir)
        };
        let b = common::build(opts, Some(&prelude(true)), &src);
        let (pdf, _) = b.output();
        if let Some(d) = common::first_difference(&pdf, &want) {
            panic!("{name} -j {threads}: pdf {d}");
        }
    }
}

/// The page layer's work, by the engine's counters, on floats2col (the
/// document of [`latex_twocol_floats_runs`]: twocolumn, 194 routine
/// fires, floats and marginpars whose routines hand back `\@freelist`,
/// `\@currlist`, `\@colroom` and the column flag to the text after them).
/// At one thread the work runs in TeX's order, so each page's routine
/// runs once: a second run of a finisher is a false dependency. At eight
/// threads the text that reads what a routine hands back waits for the
/// page fold to pass it, so the routines run at most 1.1 times the fires.
/// The PDF is pdfTeX's at both.
#[test]
fn floats2col_routines_run_once_per_fire() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let name = "floats2col";
    let src = common::read(&dir.join(format!("{name}.tex")));
    let want = std::fs::read(common::fixtures().join(format!("latex/pdf/{name}.pdf")))
        .expect("the oracle");
    let mut bad = Vec::new();
    // (`PHITEX_TEST_THREADS`, a measure: the thread counts, comma
    // separated, each its build in this process in turn)
    let counts: Vec<u64> = std::env::var("PHITEX_TEST_THREADS").map_or_else(
        |_| vec![1, 8, 16],
        |x| x.split(',').filter_map(|t| t.trim().parse().ok()).collect(),
    );
    for threads in counts {
        // (`PHITEX_TEST_FIXPOINT=1`, a measure: the read-back files to
        // their fixpoint, as the CLI builds)
        let opts = Options {
            threads: Some(usize::try_from(threads).unwrap_or(1)),
            fixpoint: std::env::var("PHITEX_TEST_FIXPOINT").is_ok_and(|x| x == "1"),
            ..pdf_opts(name, &dir)
        };
        let b = common::build(opts, Some(&prelude(true)), &src);
        let (pdf, _) = b.output();
        let fires = b.live_by_op().get("finish").copied().unwrap_or(0);
        let runs = b
            .stats
            .runs_by_op
            .iter()
            .find(|x| x.0 == "finish")
            .map_or(0, |x| x.1);
        eprintln!("{name} -j {threads}: {fires} fires, {runs} finisher runs");
        if let Some(d) = common::first_difference(&pdf, &want) {
            bad.push(format!("-j {threads}: pdf {d}"));
        }
        let ok = if threads == 1 {
            runs == fires
        } else {
            runs * 10 <= fires * 11
        };
        if !ok {
            // (why the nodes ran again, by operation and cause, most
            // first: what a schedule that ran them too often read)
            let mut why: std::collections::BTreeMap<(&str, &str), u64> =
                std::collections::BTreeMap::new();
            for (_, op, c) in &b.stats.reruns {
                *why.entry((op, c.as_str())).or_default() += 1;
            }
            let mut why: Vec<_> = why.into_iter().collect();
            why.sort_by_key(|e| std::cmp::Reverse(e.1));
            let why: Vec<String> = why
                .iter()
                .take(40)
                .map(|((op, c), n)| format!("{n} {op} {c}"))
                .collect();
            // (and the nodes run again most, with what they are)
            let mut by_node: std::collections::BTreeMap<u64, Vec<&str>> =
                std::collections::BTreeMap::new();
            for (n, _, c) in &b.stats.reruns {
                by_node.entry(*n).or_default().push(c.as_str());
            }
            let mut by_node: Vec<_> = by_node.into_iter().collect();
            by_node.sort_by_key(|e| std::cmp::Reverse(e.1.len()));
            let nodes: Vec<String> = by_node
                .iter()
                .take(12)
                .map(|(n, cs)| {
                    let mut d = b.describe_node(*n);
                    d.truncate(d.char_indices().nth(300).map_or(d.len(), |(i, _)| i));
                    let k = cs.len();
                    let mut cs = cs.clone();
                    cs.sort_unstable();
                    cs.dedup();
                    format!("node {n} ({k} reruns: {cs:?}): {d}")
                })
                .collect();
            bad.push(format!(
                "-j {threads}: {runs} finisher runs for {fires} fires; reruns:\n{}\nnodes:\n{}",
                why.join("\n"),
                nodes.join("\n")
            ));
        }
    }
    assert!(bad.is_empty(), "{name}: {bad:?}");
}

/// `pdflatex` on an article with an abstract, two sections, itemize,
/// enumerate, a theorem and a proof (amsthm), a figure with a caption
/// and a quotation (`pdfdocs/envs.tex`, 4 pages): every paragraph end
/// is a cut, inside environments too (an environment's `\begin` leaves
/// its group to the engine as a scope, its `\end` closes it); the PDF
/// equals pdfTeX's cold and after an edit inside the itemize.
#[test]
fn latex_environments() {
    // the paragraphs inside an environment that builds a box of its own
    // (its float), by line: the box is one node, like a picture
    const IN_BOX: &[usize] = &[67]; // \begin{figure}[t] ... \end{figure}
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("envs.tex"));
    let (mut b, regions) = cold_nodes("envs", &dir, &src);
    let pars = paragraphs(&src);
    // (the nodes whose own slice is a paragraph region, not every node
    // reading text: a paragraph another node's run took outside a box is
    // not one)
    let (own, taken) = (b.stats.nodes_paragraphs, b.stats.paragraphs_taken);
    let want = pars - IN_BOX.len() as u64;
    eprintln!(
        "envs: {own} paragraph nodes ({taken} paragraphs taken by another node's run outside a box), {regions} text nodes, {pars} paragraphs ({want} outside a box)"
    );
    // (the count is the hints' own, a slice whose hint says it is a
    // paragraph region: `PHITEX_HINTS=any` and `adversarial` give every
    // node another hint on purpose, and only the output must not change)
    assert!(
        std::env::var_os("PHITEX_HINTS").is_some() || (own >= want && taken == 0),
        "{own} paragraph nodes for {want} paragraphs outside a box, {taken} paragraphs taken by another node's run"
    );
    let new = quack(&src, "Item 3 of the list: the quick ");
    let before = b.pages();
    b.edit(&new);
    let cold = common::build(pdf_opts("envs", &dir), Some(&prelude(true)), &new);
    edit_check("envs", &b, &cold, &before);
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
/// (`pdf:lastlink`, `pg:`, `box:`, parameters); the build
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
    let before = b.pages();
    b.edit_file("chap1.tex", Some(new.as_bytes()));
    std::fs::write(dir.join("chap1.tex"), &new).expect("write");
    let cold = common::build(pdf_opts("inputs", &dir), Some(&prelude(true)), &src);
    let _ = std::fs::write(dir.join("chap1.tex"), &chap1);
    edit_check("inputs", &b, &cold, &before);
}

/// An `\input` file read inside a `TikZ` picture (the bayesnet example's
/// figures), edited first thing after the cold build: the PDF is a cold
/// build's of the edited file. The first push of the environment after
/// the cold build differed from the last pushed by every file the
/// provider had answered, more than 64, and was set whole, which re-asks
/// no reader: the edit ran no node (accl 25876).
#[test]
fn latex_input_in_picture_first_edit() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("latex-input-pic");
    let _ = std::fs::create_dir_all(&dir);
    let main = "\\documentclass{article}\n\\usepackage{tikz}\n\\begin{document}\nA figure.\n\n\\begin{tikzpicture}\n\\input{drawing}\n\\end{tikzpicture}\n\nAfter it.\n\\end{document}\n";
    let drawing = "% the drawing\n\\node[draw] (a) {first words};\n\\node[draw] (b) at (3,0) {second words};\n\\draw (a) -- (b);\n";
    std::fs::write(dir.join("inpic.tex"), main).expect("write");
    std::fs::write(dir.join("drawing.tex"), drawing).expect("write");
    let mut b = common::build(pdf_opts("inpic", &dir), Some(&prelude(true)), main);
    let before = b.pages();
    let new = drawing.replace("first words", "first changed words");
    b.edit_file("drawing.tex", Some(new.as_bytes()));
    assert!(b.stats.nodes_run > 0, "the edit of drawing.tex ran no node");
    std::fs::write(dir.join("drawing.tex"), &new).expect("write");
    let cold = common::build(pdf_opts("inpic", &dir), Some(&prelude(true)), main);
    let _ = std::fs::write(dir.join("drawing.tex"), drawing);
    assert_eq!(b.pages().len(), before.len(), "the page count moved");
    if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
        panic!("the edit differs from a cold build: {d}");
    }
}

/// A run again costs no more than its first run: a one-word edit of a
/// paragraph holding a `TikZ` picture of many circles (the frontends'
/// 12-line document: 16 s an edit for a 5.8 s cold build) makes fewer
/// values than the cold build made. The edited node's run took a
/// checkpoint of the picture's state every 600 commands, whatever it
/// cost; a cold run spaces them by their cost (`resume::ck_gap`).
#[test]
fn latex_a_rerun_costs_no_more_than_its_first_run() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("latex-rerun-cost");
    let _ = std::fs::create_dir_all(&dir);
    let src = "\\documentclass{article}\n\\usepackage{tikz}\n\\begin{document}\nMost edits change one paragraph.\n\\begin{tikzpicture}\\foreach \\i in {1,...,20}{\\foreach \\j in {1,...,30}{\\draw (\\i*0.3,\\j*0.2) circle (0.05);}}\\end{tikzpicture}\n\\end{document}\n";
    std::fs::write(dir.join("rerun.tex"), src).expect("write");
    let empty = {
        let b = common::build(
            pdf_opts("rerun", &dir),
            Some(&prelude(true)),
            "\\documentclass{article}\\begin{document}x\\end{document}\n",
        );
        b.stats.values_bytes
    };
    let mut b = common::build(pdf_opts("rerun", &dir), Some(&prelude(true)), src);
    let cold = b.stats.values_bytes.saturating_sub(empty);
    let before = b.stats.values_bytes;
    let new = src.replace("Most edits", "Many edits");
    b.edit(&new);
    let edit = b.stats.values_bytes.saturating_sub(before);
    eprintln!("values: cold {cold} bytes past an empty document's, the edit {edit} more");
    assert!(b.stats.nodes_run > 0, "the edit ran nothing");
    assert!(
        edit <= cold,
        "a one-word edit made {edit} bytes of values, its cold build {cold}"
    );
}

/// Pictures in a paragraph are children of its run, after characters
/// and after a display too (the frontends' document: its picture ran
/// again at every word edit of its paragraph): a word edit before them,
/// on their own line too (a typed word moves their bytes in the file),
/// runs no picture again, and the PDF is a cold build's.
#[test]
fn latex_pictures_in_a_paragraph_are_children() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("latex-par-pics");
    let _ = std::fs::create_dir_all(&dir);
    let src = "\\documentclass{article}\n\\usepackage{amsmath}\n\\usepackage{tikz}\n\\begin{document}\nSome words before it.\nMore words \\begin{tikzpicture}\\draw (0,0) circle (1);\\end{tikzpicture} and after it.\n\nA paragraph with a display, most edits change one paragraph.\n\\begin{equation} t_{edit} = t_{par} + t_{page} \\end{equation}\n\\begin{tikzpicture}\\foreach \\i in {1,...,5}{\\draw (\\i*0.3,0) circle (0.05);}\\end{tikzpicture}\nAnd words after the picture.\n\\end{document}\n";
    std::fs::write(dir.join("parpics.tex"), src).expect("write");
    let mut b = common::build(pdf_opts("parpics", &dir), Some(&prelude(true)), src);
    let before = b.pages();
    let cold_pics = common::runs_of(&b, "pic");
    for (k, (a, z)) in [
        ("Some words", "Some other words"),
        ("More words", "More of the words"),
        ("most edits", "many edits"),
    ]
    .into_iter()
    .enumerate()
    {
        let new = src.replace(a, z);
        b.edit(&new);
        let pics = common::runs_of(&b, "pic");
        let cold = common::build(pdf_opts("parpics", &dir), Some(&prelude(true)), &new);
        // (with lists left open, `phitex_tex::boxscope_on`, a picture is
        // no child: its statements are nodes; the PDF below is the check)
        if !phitex_tex::boxscope_on() {
            assert!(
                cold_pics >= 2,
                "the cold build placed {cold_pics} pictures as children"
            );
            assert_eq!(pics, 0, "edit {k}: {pics} pictures ran again");
        }
        assert_eq!(b.pages().len(), before.len());
        if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
            panic!("edit {k} differs from a cold build: {d}");
        }
        b.edit(src);
    }
}

/// A picture asked again for a read late in its run (the definition of
/// `\mymark` before it deleted: the picture, in a figure of its own
/// file, defines it at its end now, as a first use does; put back; its
/// value changed and put back) runs again each time, and the PDF is a
/// cold build's.
#[test]
fn latex_a_picture_asked_again_runs_again() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("latex-pic-ck");
    let _ = std::fs::create_dir_all(&dir);
    let src = "\\documentclass{article}\n\\usepackage{tikz}\n\\begin{document}\nFirst words.\n\n\\gdef\\mymark{marked}Second words.\n\nThird words.\n\n\\input{picfig}\n\nLast words \\mymark.\n\\end{document}\n";
    let fig = "\\begin{figure}\n\\centering\n\\begin{tikzpicture}\n\\foreach \\i in {1,...,40}{\\foreach \\j in {1,...,30}{\\draw (\\i*0.3,\\j*0.2) circle (0.05);}}\n\\node at (0,-1) {\\ifdefined\\mymark\\else\\gdef\\mymark{marked}\\fi\\mymark};\n\\end{tikzpicture}\n\\caption{A picture.}\n\\end{figure}\n";
    std::fs::write(dir.join("picck.tex"), src).expect("write");
    std::fs::write(dir.join("picfig.tex"), fig).expect("write");
    let mut b = common::build(pdf_opts("picck", &dir), Some(&prelude(true)), src);
    let before = b.pages();
    let cold_pics = common::runs_of(&b, "pic");
    // (a picture is no child with lists left open: the PDFs are the check)
    let children = !phitex_tex::boxscope_on();
    assert!(
        !children || cold_pics >= 1,
        "the cold build placed no picture as a child"
    );
    let gone = src.replace("\\gdef\\mymark{marked}", "");
    let other = src.replace("\\gdef\\mymark{marked}", "\\gdef\\mymark{market}");
    for (k, text) in [&gone, src, &other, src].into_iter().enumerate() {
        b.edit(text);
        let pics = common::runs_of(&b, "pic");
        assert!(
            !children || pics >= 1,
            "edit {k}: the picture did not run again"
        );
        std::fs::write(dir.join("picck.tex"), text).expect("write");
        let cold = common::build(pdf_opts("picck", &dir), Some(&prelude(true)), text);
        assert_eq!(b.pages().len(), before.len());
        if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
            panic!("edit {k} differs from a cold build: {d}");
        }
    }
    let _ = std::fs::write(dir.join("picck.tex"), src);
}

/// Each picture of a clean build runs once, in its child: a figure's
/// run goes on as if its picture were empty, and takes the child's output
/// when it has run, never reading the picture inline as well (Grail ran
/// each of its pictures twice, 6.1 s of 19.3 s at -j1). The pictures: one
/// in a `\resizebox` argument of a figure in an `\input` file (a token
/// list, Grail's drawn figures), one in a file `\input` inside a
/// `\resizebox` (its generated ones), two in one figure, one in a
/// paragraph. At -j1 five runs of `pic`, and the two in one figure read
/// inline once: met while the first's output is to come, the run goes
/// again and reads both inline, placing their children (a run of many
/// pictures goes once more, not once a picture: lie-hasse); at -j4 the
/// PDF of -j1. Then an edit in the text before them runs none, and one in
/// a picture's body runs that one again; each PDF a clean build's.
#[test]
fn latex_each_picture_runs_once_on_a_clean_build() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("latex-pic-once");
    let _ = std::fs::create_dir_all(&dir);
    let src = "\\documentclass{article}\n\\usepackage{graphicx}\n\\usepackage{tikz}\n\\begin{document}\nFirst words, before the figures.\n\n\\input{picdrawn}\n\nWords between the figures.\n\n\\begin{figure}\n\\centering\n\\resizebox{0.5\\columnwidth}{!}{\\input{picgen}}\n\\caption{A generated picture.}\n\\end{figure}\n\n\\begin{figure}\n\\centering\n\\begin{tikzpicture}\\draw (0,0) rectangle (1,1); \\node at (0.5,0.5) {\\small one};\\end{tikzpicture}\n\\hfill\n\\begin{tikzpicture}\\draw (0,0) circle (0.5); \\node at (0,0) {\\footnotesize two};\\end{tikzpicture}\n\\caption{Two pictures.}\n\\end{figure}\n\nA paragraph \\begin{tikzpicture}\\draw (0,0) -- (1,0.2);\\end{tikzpicture} with a picture in it.\n\nLast words.\n\\end{document}\n";
    let drawn = "\\begin{figure}[t]\n\\centering\n\\resizebox{\\columnwidth}{!}{%\n\\begin{tikzpicture}[x=0.5cm,y=0.5cm]\n\\foreach \\i in {1,...,12}{\\draw (\\i,0) circle (0.3); \\node at (\\i,1) {\\scriptsize \\i};}\n\\end{tikzpicture}%\n}\n\\caption{A drawn picture.}\n\\end{figure}\n";
    let generated = "% generated\n\\begin{tikzpicture}\n\\foreach \\i in {1,...,8}{\\fill (\\i*0.4,0) rectangle +(0.3,\\i*0.2);}\n\\node at (2,-0.5) {\\sffamily bars};\n\\end{tikzpicture}%\n";
    std::fs::write(dir.join("piconce.tex"), src).expect("write");
    std::fs::write(dir.join("picdrawn.tex"), drawn).expect("write");
    std::fs::write(dir.join("picgen.tex"), generated).expect("write");
    let at = |threads: usize| Options {
        threads: Some(threads),
        ..pdf_opts("piconce", &dir)
    };
    let one = common::build(at(1), Some(&prelude(true)), src);
    // (with lists left open, `phitex_tex::boxscope_on`, a picture is no
    // child: its statements are nodes; the PDFs are the check)
    let children = !phitex_tex::boxscope_on();
    if children {
        assert_eq!(
            (common::runs_of(&one, "pic"), one.stats.pics_inline),
            (5, 2),
            "-j1: pictures run in children, read inline: {:?}",
            one.stats.runs_by_op
        );
    }
    let four = common::build(at(4), Some(&prelude(true)), src);
    if let Some(d) = common::first_difference(&four.output().0, &one.output().0) {
        panic!("-j4 differs from -j1: {d}");
    }
    let mut b = one;
    let before = b.pages();
    let word = src.replace("First words", "Some first words");
    b.edit(&word);
    if children {
        assert_eq!(
            common::runs_of(&b, "pic"),
            0,
            "a word edit before the pictures ran one"
        );
    }
    std::fs::write(dir.join("piconce.tex"), &word).expect("write");
    let cold = common::build(at(1), Some(&prelude(true)), &word);
    assert_eq!(b.pages().len(), before.len());
    if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
        panic!("the word edit differs from a clean build: {d}");
    }
    let drawn2 = drawn.replace("circle (0.3)", "circle (0.25)");
    std::fs::write(dir.join("picdrawn.tex"), &drawn2).expect("write");
    b.edit_file("picdrawn.tex", Some(drawn2.as_bytes()));
    if children {
        assert!(
            common::runs_of(&b, "pic") >= 1,
            "the drawing edited, its picture did not run again"
        );
    }
    let cold = common::build(at(1), Some(&prelude(true)), &word);
    if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
        panic!("the drawing's edit differs from a clean build: {d}");
    }
    let _ = std::fs::write(dir.join("piconce.tex"), src);
    let _ = std::fs::write(dir.join("picdrawn.tex"), drawn);
}

/// Unchanged bytes change nothing (`r` in a watch: every file the build
/// read staged again from disk, the main text sent again; an editor
/// saving a file unchanged): with `pdfdocs/inputs.tex`, its cut
/// `chap1.tex` and its other files staged again with the bytes they
/// have, nothing is staged, the main text sent again runs no node and
/// the PDF is the one before; the cut file given as an editor's buffer
/// with its bytes too. A real edit of `chap1.tex` after them still
/// equals a cold build.
#[test]
fn latex_same_bytes_restage_runs_nothing() {
    let src_dir = common::fixtures().join("latex/pdfdocs");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("latex-same-bytes");
    let _ = std::fs::create_dir_all(&dir);
    for f in ["inputs.tex", "chap1.tex", "chap2.tex"] {
        std::fs::copy(src_dir.join(f), dir.join(f)).expect("copy");
    }
    let src = common::read(&dir.join("inputs.tex"));
    let (mut b, _) = cold_nodes("inputs", &dir, &src);
    let pdf = b.output().0;
    let names: Vec<String> = b.reads().into_iter().map(|(n, _)| n).collect();
    assert!(
        names.iter().any(|n| n.starts_with("chap1")),
        "chap1 not among the files read: {names:?}"
    );
    // (every file read, as `r` stages them: the cut one included)
    let mut staged = Vec::new();
    for n in &names {
        b.stage_file(n, None);
        if b.absorb_pending() {
            staged.push((n.clone(), b.stats.nodes_run));
        }
    }
    assert!(
        staged.is_empty(),
        "a same-bytes restage of {} files staged a change: {staged:?} (file, nodes run)",
        names.len()
    );
    b.edit(&src);
    assert_eq!(
        b.stats.nodes_run, 0,
        "{} nodes run after a same-bytes restage and the same main text",
        b.stats.nodes_run
    );
    assert!(b.output().0 == pdf, "the PDF changed with nothing changed");
    // (the cut file as an editor's buffer, its bytes; then the main text)
    let chap1 = common::read(&dir.join("chap1.tex"));
    b.stage_file("chap1.tex", Some(chap1.as_bytes()));
    assert!(
        !b.absorb_pending(),
        "chap1.tex's same bytes staged a change"
    );
    b.edit(&src);
    assert_eq!(b.stats.nodes_run, 0, "{} nodes run", b.stats.nodes_run);
    // (a real edit after them: as a cold build)
    let new = quack(&chap1, "Paragraph 5 of chapter one: the quick ");
    let before = b.pages();
    b.edit_file("chap1.tex", Some(new.as_bytes()));
    assert!(b.stats.nodes_run > 0, "the edit of chap1.tex ran nothing");
    std::fs::write(dir.join("chap1.tex"), &new).expect("write");
    let cold = common::build(pdf_opts("inputs", &dir), Some(&prelude(true)), &src);
    let _ = std::fs::write(dir.join("chap1.tex"), &chap1);
    edit_check("inputs", &b, &cold, &before);
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
    let before = b.pages();
    b.edit(&new);
    let cold = common::build(pdf_opts("red02", &dir), Some(&prelude(true)), &new);
    edit_check("red02", &b, &cold, &before);
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
    let before = b.pages();
    b.edit(&new);
    let cold = common::build(pdf_opts("secpars", &dir), Some(&prelude(true)), &new);
    edit_check("secpars", &b, &cold, &before);
}

/// The chain census gate (`common::Census`) on a `pdflatex` document:
/// hello + 30 paragraphs, cold and after a one-word edit.
#[test]
#[ignore = "bar not met (CHANGELOG, Known not to work): a cold critical path of at most 6 nodes; a scaling fact, measured 83 (paras30) and 138 (envs)"]
fn census_latex() {
    census_of("paras30", "Paragraph 17 of the text: the quick ");
}

/// The chain census gate on the environments article (its cold build
/// is `latex_environments`'s).
#[test]
#[ignore = "bar not met (CHANGELOG, Known not to work): a cold critical path of at most 6 nodes; a scaling fact, measured 83 (paras30) and 138 (envs)"]
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
    let before = b.pages();
    b.edit(&new);
    let cold = common::build(pdf_opts("utf8", &dir), Some(&prelude(true)), &new);
    edit_check("utf8", &b, &cold, &before);
}

/// A `multicols` index after a heading whose `\addcontentsline` lands on
/// multicol's saved partial page (box register, shipped by the next
/// page's routine and emptied there): its toc line is written once,
/// whatever the finishers' predictions (two runs of one page once made
/// the next finisher take that box as still full, and ship the heading's
/// write a second time: amsldoc's toc had "Index 36" and "Index 37").
#[test]
fn latex_multicol_heading_written_once() {
    use std::fmt::Write as _;
    let dir = common::fixtures().join("latex/pdfdocs");
    let mut src = String::from(
        "\\documentclass{article}\\usepackage{multicol}\n\\begin{document}\n\\tableofcontents\n",
    );
    for s in 0..3 {
        let _ = writeln!(src, "\\section{{Part {s}}}");
        for p in 0..6 {
            let _ = writeln!(
                src,
                "Paragraph {p} of part {s}: words enough for a few lines of an article page, \
                 so that the sections fill some pages before the index begins.\n"
            );
        }
    }
    src.push_str(
        "\\section*{Index}\\addcontentsline{toc}{section}{Index}\n\\begin{multicols}{2}\n",
    );
    for k in 0..400 {
        let _ = writeln!(src, "entry {k}, {}\\par", k * 7 % 97);
    }
    src.push_str("\\end{multicols}\n\\end{document}\n");
    let b = common::build(pdf_opts("mcindex", &dir), Some(&prelude(true)), &src);
    let files = b.files();
    let count = |ext: &str| {
        files
            .iter()
            .filter(|(n, _)| n.ends_with(ext))
            .map(|(_, t)| t.matches("{Index}").count())
            .sum::<usize>()
    };
    eprintln!(
        "mcindex: {} pages, files {:?}",
        b.pages().len(),
        files.iter().map(|f| &f.0).collect::<Vec<_>>()
    );
    assert_eq!(count(".aux"), 1, "the index's toc line in the .aux");
    assert_eq!(count(".toc"), 1, "the index's toc line in the .toc");
}

/// `latexmk -pdf` from nothing (`oracle/fixtures/latexmk/NAME/`, its
/// oracle `NAME.pdf` and latexmk's pdflatex runs `NAME.runs` from
/// `scripts/bless-latexmk.sh`): the sources alone copied to an empty
/// directory, the build's own rounds and the tool runs its reads make
/// (BibTeX, biber, makeindex), the PDF byte-identical at `threads`, in
/// at most as many rounds as latexmk ran pdflatex.
fn latexmk_cold(name: &str, threads: usize) {
    let fx = common::fixtures().join("latexmk");
    let src_dir = fx.join(name);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("latexmk")
        .join(format!("{name}-j{threads}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    for e in std::fs::read_dir(&src_dir).expect("the sources").flatten() {
        std::fs::copy(e.path(), dir.join(e.file_name())).expect("a source");
    }
    let want = std::fs::read(fx.join(format!("{name}.pdf"))).expect("the oracle");
    let runs: u64 = std::fs::read_to_string(fx.join(format!("{name}.runs")))
        .expect("latexmk's runs")
        .trim()
        .parse()
        .expect("a count");
    let src = common::read(&dir.join(format!("{name}.tex")));
    let opts = Options {
        fixpoint: true,
        threads: Some(threads),
        tool_finder: Some(common::tool_finder(&dir)),
        ..pdf_opts(name, &dir)
    };
    let b = common::build(opts, Some(&prelude(true)), &src);
    eprintln!(
        "latexmk {name} -j {threads}: {} rounds (latexmk: {runs} pdflatex runs); {}",
        b.stats.loop_rounds,
        report(name, &b)
    );
    for d in b.diagnostics() {
        if d.severity != phitex_tex::Severity::Error || d.range != (0, 0) {
            continue;
        }
        eprintln!("{d:?}");
    }
    if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
        let _ = std::fs::write(format!("{d}/{name}-j{threads}.pdf"), b.output().0);
        let _ = std::fs::write(format!("{d}/{name}-j{threads}.log"), b.output().1);
    }
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("{name} -j {threads}: {d}");
    }
    assert!(
        b.stats.loop_rounds <= runs,
        "{name} -j {threads}: {} rounds, latexmk ran pdflatex {runs} times",
        b.stats.loop_rounds
    );
}

/// The biber document edited (a citation taken out, then put back, then
/// another taken out) as a watch edits it, each round's `.bbl` written
/// and read back: each edit's PDF the PDF a cold build of its text makes.
/// A written file's units are cut again from its last cut where the
/// text changed (`ops::units_again`): the cuts decide how it is read
/// back in pieces, never what is read.
#[test]
fn biber_edits_equal_cold_builds() {
    let fx = common::fixtures().join("latexmk");
    let src_dir = fx.join("biber");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("latexmk")
        .join("biber-edits");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    for e in std::fs::read_dir(&src_dir).expect("the sources").flatten() {
        std::fs::copy(e.path(), dir.join(e.file_name())).expect("a source");
    }
    let src = common::read(&dir.join("biber.tex"));
    let opts = |threads: usize| Options {
        fixpoint: true,
        threads: Some(threads),
        tool_finder: Some(common::tool_finder(&dir)),
        ..pdf_opts("biber", &dir)
    };
    let two = "described by Lamport~\\cite{lamport94}, ";
    assert!(src.contains("~\\cite{mittelbach04}") && src.contains(two));
    let texts = [
        src.replace("~\\cite{mittelbach04}", ""),
        src.clone(),
        src.replace(two, "described by Lamport, "),
        src.replace("~\\cite{mittelbach04}", "")
            .replace(two, "described by Lamport, "),
        src.clone(),
    ];
    for threads in [1, 8] {
        let mut b = common::build(opts(threads), Some(&prelude(true)), &src);
        let mut failed = Vec::new();
        for (k, t) in texts.iter().enumerate() {
            b.edit(t);
            let cold = common::build(opts(threads), Some(&prelude(true)), t);
            if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
                failed.push(format!("edit {k}: {d}"));
            }
        }
        assert!(failed.is_empty(), "-j{threads}: {failed:?}");
    }
}

/// The biber document built twice in one process, one build after the
/// other: each as `latexmk -pdf` makes it. A written file's last cut was
/// kept by its name for the process (`ops::unit_ends`), and the second
/// build's `.bbl` was cut from the first's last text.
#[test]
fn latexmk_biber_twice_in_one_process() {
    latexmk_cold("biber", 1);
    latexmk_cold("biber", 1);
}

macro_rules! latexmk_tests {
    ($($test:ident: $name:literal, $j:literal;)*) => {
        $(
            #[test]
            fn $test() {
                latexmk_cold($name, $j);
            }
        )*
    };
}

// (a group opened by an earlier node and closed inside a paragraph: the
// node is cut at the paragraph's start (`\begingroup\leftskip=50pt`
// before it), and the run that closes the engine scope ends there, its
// paragraph going on outside the scope as a resume, none of the scope's
// bindings carried (§282); LaTeX's center with \centering or \small
// after \item left them standing past \end{center}: the next paragraphs
// centered, or at \small's size)
latexmk_tests! {
    latexmk_corpus_group_par_close_j1: "corpus-group-par-close", 1;
    latexmk_corpus_group_par_close_j8: "corpus-group-par-close", 8;
    latexmk_corpus_group_close_after_par_j1: "corpus-group-close-after-par", 1;
    latexmk_corpus_group_close_after_par_j8: "corpus-group-close-after-par", 8;
    latexmk_corpus_center_after_item_j1: "corpus-center-after-item", 1;
    latexmk_corpus_center_after_item_j8: "corpus-center-after-item", 8;
    latexmk_corpus_center_small_j1: "corpus-center-small", 1;
    latexmk_corpus_center_small_j8: "corpus-center-small", 8;
    latexmk_corpus_footnote_split_marginpar_j1: "corpus-footnote-split-marginpar", 1;
    latexmk_corpus_footnote_split_marginpar_j8: "corpus-footnote-split-marginpar", 8;
}

/// The paragraph after `\end{center}` edited (one word): the build equals
/// a cold one of the new text, at one thread and at eight.
#[test]
fn latexmk_corpus_center_edit() {
    let name = "corpus-center-after-item";
    let fx = common::fixtures().join("latexmk").join(name);
    for threads in [1, 8] {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("latexmk")
            .join(format!("{name}-edit-j{threads}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        for e in std::fs::read_dir(&fx).expect("the sources").flatten() {
            std::fs::copy(e.path(), dir.join(e.file_name())).expect("a source");
        }
        let src = common::read(&dir.join(format!("{name}.tex")));
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            tool_finder: Some(common::tool_finder(&dir)),
            ..pdf_opts(name, &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &src);
        let new = src.replacen("powerful", "mighty", 1);
        assert_ne!(new, src, "the edited word is in the text");
        b.edit(&new);
        let cold = common::build(opts(), Some(&prelude(true)), &new);
        if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
            panic!("{name} -j {threads}: edit differs from cold: {d}");
        }
    }
}

// (a group-local binding around a read-back file, its close after the
// file's units: restored there, and a written file's lines in TeX's
// order; then the tools a read runs)
latexmk_tests! {
    latexmk_group_toc_j1: "group-toc", 1;
    latexmk_group_toc_j8: "group-toc", 8;
    latexmk_group_file_j1: "group-file", 1;
    latexmk_group_file_j8: "group-file", 8;
    latexmk_group_index_j1: "group-index", 1;
    latexmk_group_index_j8: "group-index", 8;
    latexmk_index_shell_j1: "index-shell", 1;
    latexmk_index_shell_j8: "index-shell", 8;
    latexmk_class_toc_j1: "class-toc", 1;
    latexmk_class_toc_j8: "class-toc", 8;
    latexmk_class_index_j1: "class-index", 1;
    latexmk_class_index_j8: "class-index", 8;
    latexmk_class_index_shell_j1: "class-index-shell", 1;
    latexmk_class_index_shell_j8: "class-index-shell", 8;
    latexmk_bibtex_j1: "bibtex", 1;
    latexmk_bibtex_j8: "bibtex", 8;
    // (a database the job writes, revtex4-2's Notes.bib, achemso's
    // control entries: BibTeX reads it as the run before wrote it)
    latexmk_bibtex_written_bib_j1: "bibtex-written-bib", 1;
    latexmk_bibtex_written_bib_j8: "bibtex-written-bib", 8;
    latexmk_biber_j1: "biber", 1;
    latexmk_biber_j8: "biber", 8;
    latexmk_index_j1: "index", 1;
    latexmk_index_j8: "index", 8;
    latexmk_index_style_j1: "index-style", 1;
    latexmk_index_style_j8: "index-style", 8;
    // (a long index in two columns after a forced page: the rest after
    // the routine reads the column height and page state it leaves)
    latexmk_ams_index_j1: "amsidx", 1;
    latexmk_ams_index_j8: "amsidx", 8;
    // (a line read back under the state at its read-back point: a
    // register it allocates unless defined there, declared after it)
    latexmk_total_counter_j1: "total-counter", 1;
    latexmk_total_counter_j8: "total-counter", 8;
    // (corpus documents whose objects equal pdfLaTeX's but whose file
    // differed: a document-level command that writes where it runs, an
    // open action's object, does so with the writer's parameters there,
    // the format's version and object streams, §792)
    latexmk_corpus_links_text_j1: "corpus-links-text", 1;
    latexmk_corpus_links_text_j8: "corpus-links-text", 8;
    latexmk_corpus_links_url_j1: "corpus-links-url", 1;
    latexmk_corpus_links_url_j8: "corpus-links-url", 8;
    latexmk_corpus_links_table_j1: "corpus-links-table", 1;
    latexmk_corpus_links_table_j8: "corpus-links-table", 8;
    latexmk_corpus_title_links_j1: "corpus-title-links", 1;
    latexmk_corpus_title_links_j8: "corpus-title-links", 8;
    latexmk_corpus_image_twice_j1: "corpus-image-twice", 1;
    latexmk_corpus_image_twice_j8: "corpus-image-twice", 8;
    latexmk_corpus_char_string_j1: "corpus-char-string", 1;
    latexmk_corpus_char_string_j8: "corpus-char-string", 8;
    latexmk_corpus_version_class_j1: "corpus-version-class", 1;
    latexmk_corpus_version_class_j8: "corpus-version-class", 8;
    // (a numbered heading's number after \hskip\z@ in \@hangfrom's box:
    // glue of its own spec, which margin protrusion does not pass over;
    // 79 corpus manuals with microtype)
    latexmk_corpus_protrude_heading_j1: "corpus-protrude-heading", 1;
    latexmk_corpus_protrude_heading_j8: "corpus-protrude-heading", 8;
    // (KOMA-Script's heading: \hskip\glueexpr<indent>\relax before the
    // number, a spec of its own as e-TeX's scan_expr leaves it)
    latexmk_corpus_protrude_koma_j1: "corpus-protrude-koma", 1;
    latexmk_corpus_protrude_koma_j8: "corpus-protrude-koma", 8;
    // (tikz's fadings: an \immediate\pdfxform whose box names a form
    // not written yet writes it, an object for its resources, before the
    // next \pdflastxform; 10 corpus manuals' /pgfsmask names)
    latexmk_corpus_pgf_fading_j1: "corpus-pgf-fading", 1;
    latexmk_corpus_pgf_fading_j8: "corpus-pgf-fading", 8;
    // (a file name in braces, \openin and \input: TeX Live's
    // scan_file_name_braced; pgfmanual's `from file` code examples)
    latexmk_corpus_braced_filename_j1: "corpus-braced-filename", 1;
    latexmk_corpus_braced_filename_j8: "corpus-braced-filename", 8;
    // (a stream closed and opened again on another file in one run)
    latexmk_reopen_stream_j1: "reopen-stream", 1;
    latexmk_reopen_stream_j8: "reopen-stream", 8;
    // (job-written files read line by line, and a tool's output named as
    // a file the job also writes)
    latexmk_topic_units_j1: "topic-units", 1;
    latexmk_topic_units_j8: "topic-units", 8;
}

/// A rebuild given the files the job wrote as an earlier build left them
/// (`Options::written_init`, what the CLI reads from disk) reads them
/// back in round one where the job reads them, as TeX's next run reads
/// the files the last one wrote: a document whose files come out as they
/// were read takes one round, as latexmk runs pdflatex once on it, and
/// the output is the cold build's. A file read back per field (the
/// `.aux`) and one read back as its text (the `.toc`), at 1 and 8
/// threads.
#[test]
fn rebuild_reads_the_written_files_back_in_round_one() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("rebuild-seeded");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let docs = [
        (
            "hello",
            "\\documentclass{article}\n\\begin{document}\nHello, world.\n\\end{document}\n",
        ),
        (
            "label",
            "\\documentclass{article}\n\\begin{document}\n\\section{One}\\label{one}See section~\\ref{one} on page~\\pageref{one}.\n\\end{document}\n",
        ),
        (
            "toc",
            "\\documentclass{article}\n\\begin{document}\n\\tableofcontents\n\\section{One}\\label{one}See~\\ref{one}.\n\\section{Two}Text.\n\\end{document}\n",
        ),
    ];
    for (name, src) in docs {
        for threads in [1, 8] {
            let opts = |init: Vec<(String, String)>| Options {
                fixpoint: true,
                threads: Some(threads),
                written_init: init,
                ..pdf_opts(name, &dir)
            };
            let cold = common::build(opts(Vec::new()), Some(&prelude(true)), src);
            let files = cold.files();
            let b = common::build(opts(files.clone()), Some(&prelude(true)), src);
            eprintln!(
                "{name} -j {threads}: cold {} rounds, rebuilt {} rounds; files {:?}",
                cold.stats.loop_rounds,
                b.stats.loop_rounds,
                files.iter().map(|f| &f.0).collect::<Vec<_>>()
            );
            if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
                panic!("{name} -j {threads}: the rebuild's output is not the cold build's: {d}");
            }
            assert_eq!(b.files(), files, "{name} -j {threads}: the files written");
            assert_eq!(
                b.stats.loop_rounds, 1,
                "{name} -j {threads}: the rebuild's rounds"
            );
        }
    }
}

/// A rebuild given, beside the files the job wrote, what the tools made
/// of them in the earlier build (`Options::made_init`, the `.bbl` and the
/// `.ind` as TeX's next run finds them) reads those in round one too: a
/// document with a bibliography and an index that comes out as it was
/// read takes one round, as latexmk runs pdflatex once on it, not the
/// three of a first build (the citation written, the tool's output read,
/// the `.aux` that output changes), and the output is the cold build's.
/// At 1 and 8 threads.
#[test]
fn rebuild_reads_the_tools_outputs_back_in_round_one() {
    let fx = common::fixtures().join("latexmk");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("rebuild-made");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::copy(fx.join("bibtex/refs.bib"), dir.join("refs.bib")).expect("the database");
    let src = "\\documentclass{article}\n\\usepackage{makeidx}\n\\makeindex\n\
               \\begin{document}\nKnuth~\\cite{knuth84}.\nA word\\index{word}.\n\
               \\bibliographystyle{plain}\n\\bibliography{refs}\n\\printindex\n\\end{document}\n";
    std::fs::write(dir.join("rm.tex"), src).expect("the document");
    for threads in [1, 8] {
        let opts = |init: Vec<(String, String)>, made: Vec<u8>| Options {
            fixpoint: true,
            threads: Some(threads),
            written_init: init,
            made_init: made,
            tool_finder: Some(common::tool_finder(&dir)),
            ..pdf_opts("rm", &dir)
        };
        let cold = common::build(opts(Vec::new(), Vec::new()), Some(&prelude(true)), src);
        let (files, made) = (cold.files(), cold.export_made());
        assert!(
            !made.is_empty(),
            "-j {threads}: the tools made the .bbl and the .ind"
        );
        let b = common::build(opts(files.clone(), made), Some(&prelude(true)), src);
        eprintln!(
            "-j {threads}: cold {} rounds, rebuilt {} rounds",
            cold.stats.loop_rounds, b.stats.loop_rounds
        );
        if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
            panic!("-j {threads}: the rebuild's output is not the cold build's: {d}");
        }
        assert_eq!(b.files(), files, "-j {threads}: the files written");
        assert_eq!(b.stats.loop_rounds, 1, "-j {threads}: the rebuild's rounds");
    }
}

/// What BibTeX and makeindex say reaches the build's diagnostics as
/// warnings, at the line that reads their output: a `\cite` key the
/// database lacks (BibTeX's `Warning--I didn't find a database entry`)
/// and a range closed without being opened (makeindex's `## Warning`).
#[test]
fn tool_warnings_are_diagnostics() {
    let fx = common::fixtures().join("latexmk");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tool-warnings");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::copy(fx.join("bibtex/refs.bib"), dir.join("refs.bib")).expect("the database");
    let src = "\\documentclass{article}\n\\usepackage{makeidx}\n\\makeindex\n\
               \\begin{document}\nKnuth~\\cite{knuth84} and \\cite{nosuchkey}.\n\
               A word\\index{word} and a range end\\index{range|)}.\n\
               \\bibliographystyle{plain}\n\\bibliography{refs}\n\\printindex\n\\end{document}\n";
    std::fs::write(dir.join("tw.tex"), src).expect("the document");
    let opts = Options {
        fixpoint: true,
        tool_finder: Some(common::tool_finder(&dir)),
        ..pdf_opts("tw", &dir)
    };
    let b = common::build(opts, Some(&prelude(true)), src);
    let warnings: Vec<String> = b
        .diagnostics()
        .into_iter()
        .filter(|d| d.severity == phitex_tex::Severity::Warning)
        .map(|d| d.message)
        .collect();
    assert!(
        warnings
            .iter()
            .any(|m| m.starts_with("bibtex warning:") && m.contains("nosuchkey")),
        "{warnings:#?}"
    );
    assert!(
        warnings.iter().any(|m| m.starts_with("makeindex warning:")),
        "{warnings:#?}"
    );
}

/// An error in the `.bbl` BibTeX made is placed at its line there, as
/// pdflatex's log says it (`l.5`): the diagnostic's line was found in
/// the file's text asked of the finder, which has no `.bbl` the build
/// made, so every error in one was at line 1 (the Overleaf extension's
/// "TeX capacity exceeded ... at paper.bbl:1", the error on line 16).
#[test]
fn an_error_in_the_bbl_is_at_its_line() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bbl-error-line");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::write(
        dir.join("refs.bib"),
        "@misc{x, author = {A. Author}, title = {Tea \\undefinedmacro time}, year = 2020}\n",
    )
    .expect("the database");
    let src = "\\documentclass{article}\n\\begin{document}\nSee~\\cite{x}.\n\
               \\bibliographystyle{plain}\n\\bibliography{refs}\n\\end{document}\n";
    std::fs::write(dir.join("be.tex"), src).expect("the document");
    let opts = Options {
        fixpoint: true,
        tool_finder: Some(common::tool_finder(&dir)),
        ..pdf_opts("be", &dir)
    };
    let b = common::build(opts, Some(&prelude(true)), src);
    let ds = b.diagnostics();
    let d = ds
        .iter()
        .find(|d| d.message == "Undefined control sequence")
        .unwrap_or_else(|| panic!("the error: {ds:#?}"));
    assert!(d.file.ends_with("be.bbl"), "{d:?}");
    assert_eq!(d.line, 5, "{d:?}");
    assert!(d.context.starts_with("l.5 "), "{d:?}");
}

/// An error in an `\input` file after a series of edits of it is where
/// a cold build of the last text places it (oracle/fixtures/latex/
/// diagshift: five texts of the edit fuzz's two-column table document,
/// the last with an extra `&` in a table's head). A run that reads on
/// into the next piece's lines placed their errors by the running node's
/// move since the edits, not by that piece's: the Overleaf extension's
/// "Extra alignment tab" a line below its row, until a refresh.
#[test]
fn an_input_files_error_after_edits_is_where_a_cold_build_places_it() {
    let fx = common::fixtures().join("latex/diagshift");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latex-diagshift");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tex")).expect("a scratch directory");
    let src = common::read(&fx.join("main.tex"));
    std::fs::write(dir.join("main.tex"), &src).expect("the document");
    let text = |k: u32| std::fs::read(fx.join(format!("s{k}.tex"))).expect("a text of the file");
    std::fs::write(dir.join("tex/D-caching.tex"), text(30)).expect("the input file");
    let errors = |b: &phitex_tex::Build| -> Vec<(String, usize, (usize, usize), String)> {
        b.diagnostics()
            .into_iter()
            .filter(|d| d.severity == phitex_tex::Severity::Error)
            .map(|d| (d.file, d.line, d.range, d.message))
            .collect()
    };
    let mut b = common::build(pdf_opts("main", &dir), Some(&prelude(true)), &src);
    for k in 31..=34 {
        b.edit_file("tex/D-caching.tex", Some(&text(k)));
    }
    std::fs::write(dir.join("tex/D-caching.tex"), text(34)).expect("the input file");
    let cold = common::build(pdf_opts("main", &dir), Some(&prelude(true)), &src);
    let want = errors(&cold);
    assert!(
        want.iter()
            .any(|e| e.3.starts_with("Extra alignment tab") && e.1 == 47),
        "the cold build's error at line 47: {want:#?}"
    );
    assert_eq!(errors(&b), want, "the edits' errors, the cold build's");
}

/// The diagnostics of an `\input` file after an edit that adds a line
/// before them and leaves the bytes before them as many (a word made a
/// wider character: oracle/fixtures/latex/diagshift2, the edit fuzz's
/// longtable document) are a cold build's: their lines, and the "on input
/// line N" of LaTeX's warnings. The diagnostics kept from the last call
/// were found by their entry, line text and byte, not their line's
/// number, and stayed a line early.
#[test]
fn an_input_files_warnings_after_a_line_is_added_are_a_cold_builds() {
    let fx = common::fixtures().join("latex/diagshift2");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latex-diagshift2");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tex")).expect("a scratch directory");
    let src = common::read(&fx.join("main.tex"));
    std::fs::write(dir.join("main.tex"), &src).expect("the document");
    let text = |k: u32| std::fs::read(fx.join(format!("s{k}.tex"))).expect("a text of the file");
    std::fs::write(dir.join("tex/D-caching.tex"), text(10)).expect("the input file");
    let shown = |b: &phitex_tex::Build| -> Vec<(String, usize, String)> {
        let mut ds: Vec<_> = b
            .diagnostics()
            .into_iter()
            .filter(|d| d.file.ends_with("D-caching.tex"))
            .map(|d| (d.file, d.line, d.message))
            .collect();
        ds.sort();
        ds
    };
    let mut b = common::build(pdf_opts("main", &dir), Some(&prelude(true)), &src);
    let _ = shown(&b);
    b.edit_file("tex/D-caching.tex", Some(&text(11)));
    std::fs::write(dir.join("tex/D-caching.tex"), text(11)).expect("the input file");
    let cold = common::build(pdf_opts("main", &dir), Some(&prelude(true)), &src);
    let want = shown(&cold);
    assert!(
        want.iter()
            .any(|d| d.1 == 77 && d.2.contains("on input line 77")),
        "the cold build's warning at line 77: {want:#?}"
    );
    assert_eq!(shown(&b), want, "the edit's diagnostics, the cold build's");
}

/// A measure: amsldoc (`bench/amsldoc`) built as latexmk would, then
/// one sentence added to a paragraph (its lines re-broken): the nodes
/// that ran again and why (`PHITEX_EDIT_TEXT`: `OLD=>NEW` instead).
#[test]
#[ignore = "a measure: amsldoc's re-break edit, the reruns by node"]
fn amsldoc_rebreak_reruns() {
    let src_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../bench/amsldoc");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("amsldoc-rebreak");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::copy(src_dir.join("amsldoc.tex"), dir.join("amsldoc.tex")).expect("the source");
    let src = common::read(&dir.join("amsldoc.tex"));
    let opts = Options {
        fixpoint: true,
        threads: Some(1),
        tool_finder: Some(common::tool_finder(&dir)),
        ..pdf_opts("amsldoc", &dir)
    };
    let mut b = common::build(opts, Some(&prelude(true)), &src);
    let (old, new) = std::env::var("PHITEX_EDIT_TEXT")
        .ok()
        .and_then(|s| s.split_once("=>").map(|(a, b)| (a.to_owned(), b.to_owned())))
        .unwrap_or_else(|| {
            (
                "pagebreak after a given line, as usual.".to_owned(),
                "pagebreak after a given line, as usual. This added sentence makes the paragraph one line longer than it was before, so the breaks after it move.".to_owned(),
            )
        });
    assert_eq!(src.matches(&old).count(), 1);
    b.edit(&src.replacen(&old, &new, 1));
    eprintln!("runs by op {:?}", b.stats.runs_by_op);
    for (n, op, why) in &b.stats.reruns {
        eprintln!("  rerun {op} {n}: {why} [{}]", b.describe_node(*n));
    }
}

/// `\inputlineno` in token lists a resumed piece reads (a page candidate
/// cut the piece, its continuation the hooks `\begin{document}` runs) is
/// the line of the file below them, as pdflatex's log has it: P=3, H=5
/// (the hook, at `\begin{document}`'s line), D=6.
#[test]
fn inputlineno_in_a_resumed_piece_is_the_file_line_below_it() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("inputlineno");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let src = "\\documentclass{article}\n\\usepackage{graphicx}\n\\typeout{P=\\the\\inputlineno}\n\\AtBeginDocument{\\typeout{H=\\the\\inputlineno}}\n\\begin{document}\n\\typeout{D=\\the\\inputlineno}\nx\n\\end{document}\n";
    for threads in [1, 8] {
        let opts = Options {
            threads: Some(threads),
            ..pdf_opts("line", &dir)
        };
        let b = common::build(opts, Some(&prelude(true)), src);
        let (_, log) = b.output();
        let got: Vec<&str> = log
            .lines()
            .filter(|l| l.len() > 2 && l.as_bytes()[1] == b'=' && "PHD".contains(&l[..1]))
            .collect();
        assert_eq!(got, ["P=3", "H=5", "D=6"], "-j {threads}");
    }
}

/// The native runner of `\write18`'s commands, as the CLI has it.
/// (the tests use part of it: what only the CLI calls is not dead code)
#[allow(dead_code)]
#[path = "../../../cli/src/shell.rs"]
mod process_hook;

/// The process runner, its calls counted.
struct Counting {
    inner: process_hook::ProcessHook,
    calls: std::sync::Mutex<Vec<String>>,
}

impl phitex_tex::ShellHook for Counting {
    fn run(&self, command: &str, files: &[(String, std::sync::Arc<[u8]>)]) -> phitex_tex::ShellRan {
        let r = self.inner.run(command, files);
        // (the data files it was handed: two calls on the same are one
        // call made twice, by runs at once)
        let data: Vec<u64> = files
            .iter()
            .filter(|(n, _)| n.ends_with(".data.minted"))
            .map(|(_, b)| {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                b.hash(&mut h);
                h.finish()
            })
            .collect();
        self.calls.lock().unwrap().push(format!(
            "{command} on {data:x?}: status {}, {} files, read {:?}, wrote {:?}, removed {:?}",
            r.status,
            files.len(),
            r.read,
            r.wrote.iter().map(|w| &w.0).collect::<Vec<_>>(),
            r.removed
        ));
        r
    }
}

/// `latexmk -pdf` on a document with minted listings (python, C, Rust,
/// an inline one; `oracle/fixtures/latexmk/minted/`): pdflatex runs
/// `latexminted` through TeX Live's restricted shell escape, the build
/// through the hook (here TeX Live's own latexminted as a process, as the
/// CLI runs it): the PDF equals latexmk's at `threads`, the first
/// evaluation as pdflatex's first run (placeholders, `batch` at the end)
/// and the second from the files it made (`Stats::shell_passes` 1). Then
/// the first listing edited (`minted.edit.tex`, latexmk run on it after
/// the first in the same directory: `minted.edit.pdf`): equal again, the
/// one listing highlighted again and no `batch`.
#[allow(clippy::too_many_lines)]
fn latexmk_minted(threads: usize) {
    let name = "minted";
    let fx = common::fixtures().join("latexmk");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("latexmk")
        .join(format!("{name}-j{threads}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    for e in std::fs::read_dir(fx.join(name))
        .expect("the sources")
        .flatten()
    {
        std::fs::copy(e.path(), dir.join(e.file_name())).expect("a source");
    }
    let path = std::env::var("PATH").unwrap_or_default();
    let bin = std::env::split_paths(&path)
        .find(|d| d.join("kpsewhich").is_file())
        .expect("TeX Live's kpsewhich on PATH");
    let k = phitex_kpse::Kpse::new(&bin, "pdftex", "pdftex");
    let hook = std::sync::Arc::new(Counting {
        inner: process_hook::ProcessHook::new(&dir, k.exported()),
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let opts = || Options {
        fixpoint: true,
        threads: Some(threads),
        shell_hook: Some(hook.clone() as std::sync::Arc<dyn phitex_tex::ShellHook>),
        ..pdf_opts(name, &dir)
    };
    let src = common::read(&dir.join(format!("{name}.tex")));
    let mut b = common::build(opts(), Some(&prelude(true)), &src);
    let calls = std::mem::take(&mut *hook.calls.lock().unwrap());
    eprintln!(
        "minted -j {threads}: {} shell passes, {} rounds, {} calls; {}",
        b.stats.shell_passes,
        b.stats.loop_rounds,
        calls.len(),
        report(name, &b)
    );
    for c in &calls {
        eprintln!("  call {c}");
    }
    for d in b.diagnostics() {
        if d.severity == phitex_tex::Severity::Error {
            eprintln!("{d:?}");
        }
    }
    if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
        let _ = std::fs::write(format!("{d}/{name}-j{threads}.pdf"), b.output().0);
        let _ = std::fs::write(format!("{d}/{name}-j{threads}.log"), b.output().1);
    }
    let want = std::fs::read(fx.join(format!("{name}.pdf"))).expect("the oracle");
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("{name} -j {threads}: {d}");
    }
    assert_eq!(
        b.stats.shell_passes, 1,
        "{name} -j {threads}: evaluations again for the commands' files"
    );
    // (the first listing edited)
    let new = common::read(&fx.join(format!("{name}.edit.tex")));
    b.edit(&new);
    let calls = std::mem::take(&mut *hook.calls.lock().unwrap());
    eprintln!(
        "minted edit -j {threads}: {} shell passes, {} calls, {} nodes run, {} runs",
        b.stats.shell_passes,
        calls.len(),
        b.stats.nodes_run,
        b.stats.runs
    );
    for c in &calls {
        eprintln!("  call {c}");
    }
    if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
        let _ = std::fs::write(format!("{d}/{name}-edit-j{threads}.pdf"), b.output().0);
        let _ = std::fs::write(format!("{d}/{name}-edit-j{threads}.log"), b.output().1);
    }
    let want = std::fs::read(fx.join(format!("{name}.edit.pdf"))).expect("the edit's oracle");
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("{name} edit -j {threads}: {d}");
    }
    let of = |what: &str| {
        let mut x: Vec<&str> = calls
            .iter()
            .filter(|c| c.contains(&format!("'{what}'")))
            .map(|c| c.split(':').next().unwrap_or(c))
            .collect();
        x.sort_unstable();
        x.dedup();
        x.len()
    };
    assert_eq!(of("highlight"), 1, "{name} edit -j {threads}: {calls:#?}");
    assert_eq!(of("batch"), 0, "{name} edit -j {threads}: {calls:#?}");
    // (one run from nothing, without the fixpoint: pdflatex once in an
    // empty directory, `minted.run1.pdf` (SOURCE_DATE_EPOCH=0,
    // FORCE_SOURCE_DATE=1, `-interaction=batchmode`): no file a command
    // makes is read before the command, so minted's placeholders, and
    // `batch` at the end)
    let one = Options {
        fixpoint: false,
        ..opts()
    };
    let b = common::build(one, Some(&prelude(true)), &src);
    let want = std::fs::read(fx.join(format!("{name}.run1.pdf"))).expect("the first run's oracle");
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("{name} one run -j {threads}: {d}");
    }
    assert_eq!(b.stats.shell_passes, 0, "{name} one run -j {threads}");
}

#[test]
fn latexmk_minted_j1() {
    latexmk_minted(1);
}

#[test]
fn latexmk_minted_j8() {
    latexmk_minted(8);
}

/// Glyph origins of `pdflatex` on `pdfdocs/envs.tex` (an abstract,
/// sections, lists, a theorem, a figure, a quotation; its paragraphs
/// repeat their sentences): every page's codes have one each, every word
/// of the text maps to its bytes, the same at -j1 and -j8, the PDF still
/// pdfTeX's; after a word's edit, a cold build's answers
/// (docs/note-origins.md).
#[test]
fn latex_glyph_origins() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("envs.tex"));
    let want = std::fs::read(common::fixtures().join("latex/pdf/envs.pdf")).expect("the oracle");
    let build = |text: &str, threads: usize| {
        let opts = Options {
            threads: Some(threads),
            origins: true,
            ..pdf_opts("envs", &dir)
        };
        common::build(opts, Some(&prelude(true)), text)
    };
    let mut b = build(&src, 1);
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("envs with origins: pdf {d}");
    }
    let f = common::origin_failures(&b, &src, "envs.tex");
    assert!(f.is_empty(), "origins:\n{}", f.join("\n"));
    let b8 = build(&src, 8);
    assert_eq!(
        b8.all_page_origins(),
        b.all_page_origins(),
        "-j8's origins are -j1's"
    );
    let _ = b.origins_changed();
    let new = quack(&src, "Item 3 of the list: the quick ");
    b.edit(&new);
    let (changed, _) = b.origins_changed();
    eprintln!(
        "envs: origins changed on pages {changed:?} of {}",
        b.pages().len()
    );
    let cold = build(&new, 1);
    assert_eq!(
        b.all_page_origins(),
        cold.all_page_origins(),
        "the edit's origins are a cold build's"
    );
    let f = common::origin_failures(&b, &new, "envs.tex");
    assert!(f.is_empty(), "origins after the edit:\n{}", f.join("\n"));
}

/// A two-section article with its `.aux` on disk from a pdflatex run,
/// built outside a fixpoint (`--compat=pdflatex`): the file the job
/// writes reads as absent, so the build is the one without the file.
/// The `.aux` was cut as a document file when `\@input` first read it;
/// once the job opened it for writing its text read absent while the
/// engine kept the cut, and the reader read an empty line from it for
/// ever: past 10 GB in 45 s, never stopped.
#[test]
fn latex_aux_on_disk_outside_a_fixpoint() {
    let art = "\\documentclass{article}\n\\begin{document}\n\\section{Origins}\nOrigins tell a viewer where each glyph of a page came from in the source,\nso that a double click finds the word.\n\n\\section{Lines}\nA second paragraph, with \\emph{emphasis} and a footnote\\footnote{The\nfootnote's text.} in it, ends here.\n\\end{document}\n";
    let aux = "\\relax \n\\@writefile{toc}{\\contentsline {section}{\\numberline {1}Origins}{1}{}\\protected@file@percent }\n\\gdef \\@abspage@last{1}\n";
    let mut pages = Vec::new();
    for (name, aux) in [
        ("none", None),
        ("relax", Some("\\relax \n")),
        ("full", Some(aux)),
    ] {
        let dir = common::scratch(&format!("aux-on-disk-{name}"));
        std::fs::write(dir.join("art.tex"), art).unwrap();
        if let Some(a) = aux {
            std::fs::write(dir.join("art.aux"), a).unwrap();
        }
        let t = std::time::Instant::now();
        let opts = Options {
            time_limit: Some(std::time::Duration::from_secs(60)),
            memory_limit: Some(common::rss_bytes() + (4 << 30)),
            ..pdf_opts("art", &dir)
        };
        let b = common::build(opts, Some(&prelude(true)), art);
        let stuck: Vec<String> = b
            .diagnostics()
            .into_iter()
            .map(|d| d.message)
            .filter(|m| m.contains("stuck") || m.contains("deadline"))
            .collect();
        assert!(
            stuck.is_empty(),
            "{name}: the build ends, not stuck, after {:?}: {stuck:?}",
            t.elapsed()
        );
        if let Ok(out) = std::env::var("PHITEX_TEST_OUT") {
            let (pdf, log) = b.output();
            let _ = std::fs::write(format!("{out}/aux-{name}.pdf"), pdf);
            let _ = std::fs::write(format!("{out}/aux-{name}.log"), log);
        }
        assert_eq!(b.pages().len(), 1, "{name}: {}", report(name, &b));
        pages.push((name, b.output()));
    }
    for (name, (pdf, log)) in &pages[1..] {
        if let Some(d) = common::first_difference(pdf, &pages[0].1.0) {
            panic!("{name}: the PDF differs from the one built without the .aux: {d}");
        }
        assert_eq!(
            log, &pages[0].1.1,
            "{name}: the log is the one built without the .aux"
        );
    }
}

/// `lineno`'s numbered lines: its routine is fired at a special penalty
/// (`\penalty-32000`) at every line, gives the lines back numbered and
/// ships nothing; only an ordinary break's routine ships a page and steps
/// `\c@page`. Its `.aux` lines (`\@LN`) read back run the pages a second
/// round. A finisher of a line's fire is predicted to step no counter
/// (`Finish::alike`: no relation learned for the operation from a
/// shipping finisher's), and one past the loop's frontier waits its turn
/// by place (`sched::sched_key`): the finishers run a few times each, not
/// once for every finisher placed before them on their page (lineno's
/// manual: 60 000 finisher runs for 2 900 fires, more than 600 s where
/// pdflatex takes 4.4 s; 92 s after).
#[test]
fn lineno_numbered_lines_run_their_finishers_a_few_times() {
    use std::fmt::Write as _;
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lineno-lines");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let mut src = String::from(
        "\\documentclass{article}\n\\usepackage{lineno}\n\\linenumbers\n\\begin{document}\nSee line \\lineref{l:last} on page \\pageref{l:last}.\n\n",
    );
    for i in 0..60 {
        let _ = write!(
            src,
            "Paragraph {i} of numbered lines, long enough to be set as three or four lines of the article class's text width, so that the pages fill and every line has its number.\n\n"
        );
    }
    src.push_str("The last line.\\linelabel{l:last}\n\\end{document}\n");
    for threads in [1, 4] {
        let opts = Options {
            threads: Some(threads),
            fixpoint: true,
            ..pdf_opts("lines", &dir)
        };
        let b = common::build(opts, Some(&prelude(true)), &src);
        let pages = b.pages().len() as u64;
        let fin = b.stats.placements_of("finish") as u64;
        let runs = common::runs_of(&b, "finish");
        eprintln!(
            "-j{threads}: {pages} pages, {fin} finishers placed, {runs} finisher runs: {}",
            report("lines", &b)
        );
        let errors: Vec<String> = b
            .diagnostics()
            .iter()
            .filter(|d| d.severity == phitex_tex::Severity::Error)
            .map(|d| d.message.clone())
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
        assert!(pages >= 4, "a document of several pages: {pages}");
        assert!(fin >= 150, "a finisher per numbered line: {fin}");
        assert!(
            runs <= 3 * fin,
            "-j{threads}: {runs} finisher runs for {fin} finishers placed: {}",
            report("lines", &b)
        );
    }
}

/// imakeidx's `\write18{makeindex ...}` at `\printindex` is a tool node
/// reading the `.idx` stream before it, which the pages' finishers write
/// (`\index` entries go out at `\shipout`). It runs at the committed
/// frontier only, once a pass: run on each guess of the stream, it ran
/// again at every page's finisher that committed before it (amsldoc's
/// paragraph added at -j8: 20 runs, 162 ms). A paragraph added at the
/// start runs it at most once in each of its passes, and the edited PDF equals a
/// cold build's. At -j8.
#[test]
fn an_index_tool_node_runs_once_a_pass_after_an_edit() {
    use std::fmt::Write as _;
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/index-node");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let doc = |extra: &str| {
        let mut s = String::from(
            "\\documentclass{article}\n\\usepackage{imakeidx}\n\\makeindex\n\\begin{document}\n",
        );
        s.push_str(extra);
        for k in 0..24 {
            let _ = write!(
                s,
                "\\section{{Part {k}}}\nText of part {k}\\index{{word{k}}} and more\\index{{common}}.\n\n\\newpage\n"
            );
        }
        s.push_str("\\printindex\n\\end{document}\n");
        s
    };
    let opts = || Options {
        fixpoint: true,
        threads: Some(8),
        tool_finder: Some(common::tool_finder(&dir)),
        ..pdf_opts("idxnode", &dir)
    };
    let mut b = common::build(opts(), Some(&prelude(true)), &doc(""));
    for extra in ["An added paragraph.\n\n", "Another paragraph, longer.\n\n"] {
        b.edit(&doc(extra));
        let runs = b
            .stats
            .reruns
            .iter()
            .filter(|(_, op, _)| *op == "toolnode")
            .count();
        let cold = common::build(opts(), Some(&prelude(true)), &doc(extra));
        assert_eq!(
            common::first_difference(&b.output().0, &cold.output().0),
            None,
            "{extra:?}"
        );
        // (an edit's passes: the text's, the `.idx`'s round, one to
        // settle)
        assert!(
            runs <= 3,
            "{extra:?}: the index's tool node ran {runs} times"
        );
    }
}

/// `\begin{document}` reads the `.aux` inside the `document`
/// environment's group, hyperref's `\newlabel`s binding each label in
/// that group before defining it globally, and goes on past a page
/// candidate (a box the hooks put on the page). The continuation carried
/// the label's global value in that slot, which the resume does not keep
/// (it reads the engine's). The rest of `\begin{document}` is a resume. A heading's
/// text edited changes one label's value: the reader of the `.aux` runs
/// again and the label's readers do, not the rest, which reads the label
/// from the engine and not from the continuation (on Grail it ran again
/// for every heading edit, 144 ms). The edited PDF equals a cold build's.
#[test]
fn a_heading_edit_does_not_run_the_rest_of_begin_document_again() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/heading-rest");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let doc = |title: &str| {
        format!(
            "\\documentclass{{article}}\n\\usepackage{{hyperref}}\n\\AtBeginDocument{{\\null\\par\\def\\hook{{x}}\\null\\par}}\n\\begin{{document}}\n\
             \\section{{Intro}}\\label{{sec:a}}\nSee section~\\ref{{sec:b}}.\n\n\
             \\section{{{title}}}\\label{{sec:b}}\nText of the second section.\n\\end{{document}}\n"
        )
    };
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("headrest", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc("Second"));
        for title in ["Xsecond", "Second part", "Second"] {
            b.edit(&doc(title));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            for x in &r {
                eprintln!("  -j{threads} {title}: rerun {x}");
            }
            let cold = common::build(opts(), Some(&prelude(true)), &doc(title));
            assert_eq!(
                common::first_difference(&b.output().0, &cold.output().0),
                None,
                "-j{threads} {title}"
            );
            // (the `.aux`'s reader, `\begin{document}`'s node, and its
            // rest, a resume reading `cont:<reader>:<k>`)
            let readers: Vec<String> = b
                .stats
                .reruns
                .iter()
                .filter(|(n, op, _)| {
                    *op == "exec" && b.describe_node(*n).contains("\\begin{document}")
                })
                .map(|(n, _, _)| format!("Changed cont:{n}:"))
                .collect();
            assert!(
                !r.iter()
                    .any(|x| readers.iter().any(|c| x.contains(c.as_str()))),
                "-j{threads} {title}: the rest after the .aux's reader ran again: {r:?}"
            );
        }
    }
}

/// `\tableofcontents` reads its `.toc` through `\IfFileExists`, which
/// is expl3's `\file_if_exist`: it expands `\pdffilesize` of the file and
/// tests the digits only for blankness (`\if:w \scan_stop: #1
/// \scan_stop:`, `\tl_if_blank`). The size is a symbol standing for a
/// non-empty digit string, read for presence alone: a heading edit,
/// which changes the `.toc`'s size, wakes no reader on the size
/// (`wsize:`; amsldoc's TOC resume ran again on `wsize:amsldoc.toc` at
/// every heading edit). The `.toc` going from absent to present (a
/// `\tableofcontents` added) runs its reader, and every edited PDF
/// equals a cold build's.
#[test]
fn a_heading_edit_reads_no_file_size() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/toc-size");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let doc = |toc: bool, title: &str| {
        format!(
            "\\documentclass{{article}}\n\\begin{{document}}\n{}\
             \\section{{Intro}}\nFirst section.\n\n\
             \\section{{{title}}}\nText of the second section.\n\n\
             \\section{{Last}}\nThe end.\n\\end{{document}}\n",
            if toc { "\\tableofcontents\n" } else { "" }
        )
    };
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("tocsize", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc(true, "Second"));
        for (toc, title) in [
            (true, "Xsecond"),
            (true, "Second part of it"),
            (false, "Second"),
            (true, "Second"),
        ] {
            b.edit(&doc(toc, title));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            for x in &r {
                eprintln!("  -j{threads} {toc} {title}: rerun {x}");
            }
            let cold = common::build(opts(), Some(&prelude(true)), &doc(toc, title));
            assert_eq!(
                common::first_difference(&b.output().0, &cold.output().0),
                None,
                "-j{threads} {toc} {title}"
            );
            assert!(
                !r.iter()
                    .any(|x| !x.starts_with("files") && x.contains("wsize:")),
                "-j{threads} {toc} {title}: a reader ran again on a file's size: {r:?}"
            );
        }
    }
}

/// `\pdffilesize` of a file the job writes and reads back is a symbol
/// read for presence alone (docs/spec-relative-counters.md §2.4), and
/// every use of it answers what TeX's digits answer: printed, as a
/// number, `\if` and `\ifcat` against a digit and against a non-digit,
/// an undelimited argument (its first digit, the rest left), kept by
/// `\detokenize`, expl3's `\tl_if_blank` (`\use_none:n` then an empty
/// test), `\ifx`. The file is absent, then of sizes with two digits
/// and with one, and empty; each build's lines are what the digits give
/// (`N=` the number read with `\count`), and equal a cold build's.
#[test]
#[allow(clippy::too_many_lines)]
fn a_file_size_symbol_answers_as_its_digits() {
    let doc = |body: Option<&str>| {
        let write = body.map_or_else(String::new, |b| {
            format!("\\newwrite\\sizf\\immediate\\openout\\sizf=\\jobname.siz\n{}\\immediate\\closeout\\sizf\n", if b.is_empty() { String::new() } else { format!("\\immediate\\write\\sizf{{{b}}}\n") })
        });
        format!(
            r"\def\size{{\pdffilesize{{\jobname.siz}}}}
\count255=0\size\relax
\immediate\write16{{N=\the\count255.}}
\immediate\write16{{A=\size.}}
\immediate\write16{{B=\if\relax\size\relax Y\else N\fi.}}
\immediate\write16{{C=\ifcat 1\size Y\else N\fi.}}
\immediate\write16{{D=\if 1\size Y\else N\fi.}}
\def\first#1{{(#1)}}
\edef\sz{{\size}}
\immediate\write16{{E=\expandafter\first\sz!.}}
\immediate\write16{{F=\detokenize\expandafter{{\sz}}.}}
\def\usenone#1{{}}
\def\ifemptyo#1{{\if\relax\detokenize\expandafter{{#1}}\relax}}
\def\gtest#1{{\immediate\write16{{G=\ifemptyo{{\usenone#1?}}Y\else N\fi.}}}}
\expandafter\gtest\expandafter{{\sz}}
\edef\kept{{\size}}
\immediate\write16{{H=\ifnum0\kept>9 Y\else N\fi.}}
\immediate\write16{{I=\expandafter\ifx\sz\relax Y\else N\fi.}}
\immediate\write16{{J=\if\relax\kept\relax Y\else N\fi.}}
Text.
{write}\end{{document}}
"
        )
    };
    // (what TeX writes for a size of `n`, none where absent)
    let want = |n: Option<u64>| -> Vec<String> {
        let d = n.map_or_else(String::new, |n| n.to_string());
        let (h, t) = if d.is_empty() {
            ("", "")
        } else {
            d.split_at(1)
        };
        let yn = |b: bool| if b { "Y" } else { "N" }.to_owned();
        // (`\ifx` of the first two digits, or of the digit and `\relax`;
        // absent: `\relax` and `Y`)
        let i = if d.len() >= 2 && d.as_bytes()[0] == d.as_bytes()[1] {
            format!("{}\\relax Y", &d[2..])
        } else {
            "N".to_owned()
        };
        vec![
            format!("N={}.", n.unwrap_or(0)),
            format!("A={d}."),
            format!("B={}.", yn(d.is_empty())),
            // (`\ifcat 1` against the first digit: true, the rest then
            // `Y`; absent: against `Y`, a letter)
            format!(
                "C={}.",
                if d.is_empty() {
                    "N".to_owned()
                } else {
                    format!("{t}Y")
                }
            ),
            format!(
                "D={}.",
                if h == "1" {
                    format!("{t}Y")
                } else {
                    "N".to_owned()
                }
            ),
            if d.is_empty() {
                "E=(!).".to_owned()
            } else {
                format!("E=({h}){t}!.")
            },
            format!("F={d}."),
            format!("G={}.", yn(d.is_empty())),
            format!("H={}.", yn(n.unwrap_or(0) > 9)),
            format!("I={i}."),
            format!("J={}.", yn(d.is_empty())),
        ]
    };
    let lines = |log: &str| -> Vec<String> {
        log.lines()
            .filter(|l| l.len() > 1 && l.as_bytes()[1] == b'=' && l.ends_with('.'))
            .map(str::to_owned)
            .collect()
    };
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/size-symbol");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("sizesym", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc(None));
        // (absent: a cold build that writes no such file; a build after
        // one that wrote it finds what that wrote, as TeX does)
        assert_eq!(lines(&b.output().1), want(None), "-j{threads}: absent");
        for body in ["abcdefghijklm", "ab", "", "abcdefghij"] {
            let body = Some(body);
            b.edit(&doc(body));
            let got = lines(&b.output().1);
            let n = got
                .first()
                .and_then(|l| l.strip_prefix("N=")?.strip_suffix('.')?.parse::<u64>().ok());
            let w = want(n);
            assert_eq!(got, w, "-j{threads} {body:?}: the size's uses");
            let cold = common::build(opts(), Some(&prelude(true)), &doc(body));
            assert_eq!(
                common::first_difference(&b.output().0, &cold.output().0),
                None,
                "-j{threads} {body:?}: edit differs from cold"
            );
            assert_eq!(
                lines(&cold.output().1),
                got,
                "-j{threads} {body:?}: cold's lines"
            );
        }
    }
}

/// A LaTeX document of several pages with a table of contents and
/// citations (BibTeX), built to its fixpoint; a heading edited: the
/// `.toc` read back moves the first page, so the page loop's first node
/// folds again from a moved start, and its rests fold again while the
/// pieces after them are holes. A rest whose start is not decided yet,
/// or found stale, waits with what it made standing
/// (`pages::pageloop`, `Run::wait_keeping`): no loop node and no finisher
/// is made anew (on Grail every rest and finisher after the first pages
/// was, each \cite edit, accl 23278, mc 78930: 51 finishers made anew and
/// their routines run, 4.1 s of a 6.5 s edit), and the edited build equals
/// a cold build of the edited text, at one and at eight threads.
#[test]
fn a_toc_edit_keeps_every_rest_and_finisher() {
    let fx = common::fixtures().join("latexmk").join("bibtex");
    let mut src = String::from(
        "\\documentclass{article}\n\\begin{document}\n\\tableofcontents\n\\clearpage\n",
    );
    for s in 0..12 {
        let _ = std::fmt::Write::write_fmt(
            &mut src,
            format_args!("\\section{{Section number {s}}}\\label{{s{s}}}\n"),
        );
        for p in 0..6 {
            let _ = std::fmt::Write::write_fmt(
                &mut src,
                format_args!(
                    "Paragraph {p} of section {s}, long enough to make a few lines of text in the article class, with a citation~\\cite{{{}}} and a reference to section~\\ref{{s{}}} on page~\\pageref{{s{}}}, so that the pages fill.\n\n",
                    ["knuth84", "lamport94", "mittelbach04"][(s + p) % 3],
                    (s + 5) % 12,
                    (s + 7) % 12
                ),
            );
        }
    }
    src.push_str("\\bibliographystyle{plain}\n\\bibliography{refs}\n\\end{document}\n");
    let edited = src.replacen("\\section{Section number 3}", "\\section{Section number three, a longer title that takes two lines in the table of contents and in the text}", 1);
    assert_ne!(edited, src);
    for threads in [1, 8] {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("toc-edit-j{threads}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        std::fs::copy(fx.join("refs.bib"), dir.join("refs.bib")).expect("the database");
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            tool_finder: Some(common::tool_finder(&dir)),
            ..pdf_opts("tocedit", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &src);
        let pages = b.pages().len();
        assert!(pages >= 6, "-j{threads}: several pages: {pages}");
        let r = common::rebuild(&mut b, &edited);
        eprintln!(
            "-j{threads} toc edit: {} pages, loop nodes made {} run {}, finishers made {} run {}, {} runs: {r:?}",
            b.pages().len(),
            r.placements_made("pages"),
            r.runs_by_op
                .iter()
                .find(|x| x.0 == "pages")
                .map_or(0, |x| x.1),
            r.placements_made("finish"),
            r.runs_by_op
                .iter()
                .find(|x| x.0 == "finish")
                .map_or(0, |x| x.1),
            r.runs
        );
        let cdir = dir.join("cold");
        std::fs::create_dir_all(&cdir).expect("a scratch directory");
        std::fs::copy(fx.join("refs.bib"), cdir.join("refs.bib")).expect("the database");
        let cold = common::build(
            Options {
                fixpoint: true,
                threads: Some(threads),
                tool_finder: Some(common::tool_finder(&cdir)),
                ..pdf_opts("tocedit", &cdir)
            },
            Some(&prelude(true)),
            &edited,
        );
        if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
            panic!("-j{threads}: the edit differs from a cold build: {d}");
        }
        let new_pages = b.pages().len().saturating_sub(pages);
        assert_eq!(
            r.placements_made("pages"),
            new_pages,
            "-j{threads}: no loop node made anew but for a new page: {r:?}"
        );
        assert!(
            r.placements_made("finish") <= new_pages,
            "-j{threads}: no finisher made anew but for a new page: {r:?}"
        );
    }
}

/// The `.aux` hyperref's kind of package writes: `\providecommand` lines,
/// each of which goes through `\def`'s body and skipped text naming
/// `\l@ngrel@x` and `\reserved@c`, which the lines before it define. Those
/// looks ask only whether the name is `#` or a conditional, so the file stays
/// read back per field, and a heading's new `.aux` line runs no reader of the
/// file's text. A line before it that does make such a name a conditional
/// makes it the text (tested against cold).
#[test]
fn providecommand_lines_keep_the_aux_read_back_per_field() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/aux-provide");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let doc = |title: &str| {
        format!(
            "\\documentclass{{article}}\n\\makeatletter\n\\AtBeginDocument{{\\if@filesw\
             \\immediate\\write\\@auxout{{\\string\\providecommand\\string\\aone[1]{{}}}}\
             \\immediate\\write\\@auxout{{\\string\\providecommand\\string\\atwo[1]{{}}}}\
             \\immediate\\write\\@auxout{{\\string\\providecommand\\string\\athree[2]{{}}}}\\fi}}\n\
             \\makeatother\n\\begin{{document}}\n\
             \\section{{Intro}}\\label{{a}}\nFirst, see section~\\ref{{b}}.\n\n\
             \\section{{{title}}}\\label{{b}}\nText of the second section.\n\\end{{document}}\n"
        )
    };
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("auxprovide", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc("Second"));
        for title in ["Xsecond", "Second part"] {
            b.edit(&doc(title));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            let cold = common::build(opts(), Some(&prelude(true)), &doc(title));
            assert_eq!(
                common::first_difference(&b.output().0, &cold.output().0),
                None,
                "-j{threads} {title}"
            );
            assert!(
                !r.iter()
                    .any(|x| x.contains("wunit:") || x.contains("wfile:")),
                "-j{threads} {title}: a reader of the .aux's text ran again: {r:?}"
            );
        }
    }
}

/// A file the job writes and reads back itself (a listing's temporary, as
/// tcolorbox makes for every example) is read from that file's own stream:
/// a heading's edit moves the `.aux`'s, and no such reader runs again. All
/// of `streams` was read: the 306-page circuitikz manual's heading edit
/// re-ran 960 examples on it, 94 s.
#[test]
fn a_file_read_back_by_its_writer_reads_its_own_stream_alone() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/own-stream");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let doc = |title: &str| {
        format!(
            "\\documentclass{{article}}\n\\newwrite\\w\n\\begin{{document}}\n\
             \\tableofcontents\n\\section{{{title}}}\nText of the section.\n\n\\newpage\n\
             Before the listing.\n\n\
             \\immediate\\openout\\w=ex.tmp \\immediate\\write\\w{{Written and read.}}\
             \\immediate\\closeout\\w \\input{{ex.tmp}}\n\n\
             After the listing.\n\\end{{document}}\n"
        )
    };
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("ownstream", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc("First"));
        for title in ["Xfirst", "First part of it", "First"] {
            b.edit(&doc(title));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            let cold = common::build(opts(), Some(&prelude(true)), &doc(title));
            assert_eq!(
                common::first_difference(&b.output().0, &cold.output().0),
                None,
                "-j{threads} {title}"
            );
            assert!(
                !r.iter().any(|x| x.contains("Range streams")),
                "-j{threads} {title}: a reader ran again on all the streams: {r:?}"
            );
        }
    }
}

/// A box's report in the log names the lines of its paragraph, and an
/// edit above moves them: the report's number is made when the log is laid
/// out, so the paragraph does not run again (the 306-page circuitikz
/// manual: a paragraph added near the start re-ran 133 pieces with such a
/// report, 3.6 s), and the log still says what a whole build's does.
#[test]
fn a_box_report_does_not_tie_its_paragraph_to_the_lines_above() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/report-lines");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let doc = |above: &str| {
        format!(
            "\\documentclass{{article}}\n\\begin{{document}}\n\
             First paragraph.\n{above}\n\
             \\newpage\n\
             Another paragraph.\n\n\
             \\hbox to 10pt{{Far too wide for the box it is set in.}}\n\n\
             \\parbox{{5pt}}{{Words\nunderfullwordstoolongforitswidth}}\n\n\
             The end.\n\\end{{document}}\n"
        )
    };
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("reportlines", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc(""));
        for above in [
            "One line more.\n",
            "Two lines\nmore here.\n\nAnd a paragraph.\n",
            "",
        ] {
            b.edit(&doc(above));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            let cold = common::build(opts(), Some(&prelude(true)), &doc(above));
            let (mine, want) = (b.output(), cold.output());
            assert_eq!(
                common::first_difference(&mine.0, &want.0),
                None,
                "-j{threads} {above:?}"
            );
            assert_eq!(mine.1, want.1, "-j{threads} {above:?}: the log");
            assert!(
                mine.1.contains("at lines") || mine.1.contains("detected at line"),
                "{}",
                mine.1
            );
            assert!(
                !r.iter()
                    .any(|x| x.contains("wide") || x.contains("underfullwords")),
                "-j{threads} {above:?}: a piece with a report ran again: {r:?}"
            );
        }
    }
}

/// A message written to the log says "on input line" (`\PackageInfo`,
/// `\on@line`): its number is made when the log is laid out, so a line added
/// above does not run the piece again, and the log still says what a whole
/// build's does. A number that goes anywhere else in such a line's expansion
/// (a comparison, a file) is read as ever.
#[test]
fn a_message_on_a_line_does_not_tie_its_paragraph_to_the_lines_above() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/message-lines");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let doc = |above: &str, body: &str| {
        format!(
            "\\documentclass{{article}}\n\\makeatletter\n\\begin{{document}}\n\
             First paragraph.\n{above}\n\
             \\newpage\n\
             Another paragraph.\n\n\
             {body}\n\n\
             The end.\n\\end{{document}}\n"
        )
    };
    let quiet = "\\PackageInfo{mine}{A message\\MessageBreak over lines}\n\
                 A paragraph.\\immediate\\write-1{twice \\the\\inputlineno, \\the\\inputlineno}\n\n\
                 \\immediate\\write\\@unused{Direct\\on@line.}";
    // (each of these must see the digits: its result shows them)
    let digits = "\\immediate\\write-1{\\ifnum\\the\\inputlineno>5 past\\else before\\fi}\n\
                  \\immediate\\write-1{\\pdfstrcmp{\\the\\inputlineno}{9}}\n\
                  \\newwrite\\f\\immediate\\openout\\f=\\jobname.lines\n\
                  \\immediate\\write\\f{in a file \\the\\inputlineno}\\immediate\\closeout\\f\n\
                  \\def\\m{\\the\\inputlineno}\\immediate\\write-1{through \\m}\n\
                  \\edef\\n{\\the\\inputlineno}\\immediate\\write-1{kept \\n}\n\
                  Page \\the\\inputlineno.";
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("messagelines", &dir)
        };
        for body in [quiet, digits] {
            let mut b = common::build(opts(), Some(&prelude(true)), &doc("", body));
            for above in [
                "One line more.\n",
                "Two lines\nmore here.\n\nA paragraph.\n",
                "",
            ] {
                b.edit(&doc(above, body));
                let r: Vec<String> = b
                    .stats
                    .reruns
                    .iter()
                    .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                    .collect();
                let cold = common::build(opts(), Some(&prelude(true)), &doc(above, body));
                let (mine, want) = (b.output(), cold.output());
                assert_eq!(
                    common::first_difference(&mine.0, &want.0),
                    None,
                    "-j{threads} {above:?}"
                );
                assert_eq!(mine.1, want.1, "-j{threads} {above:?}: the log");
                if body == quiet {
                    assert!(mine.1.contains("on input line"), "{}", mine.1);
                    assert!(
                        !r.iter()
                            .any(|x| x.contains("PackageInfo") || x.contains("A paragraph")),
                        "-j{threads} {above:?}: {r:?}"
                    );
                }
            }
        }
    }
}

/// The pieces of a file `\input` read their lines from where the file's
/// text puts them, as the document's own nodes do: a box's report in one
/// does not tie it to the lines above it in the file, and a number read
/// from it is the line in the file at each of its reads.
#[test]
fn a_piece_of_an_input_file_has_its_lines_given() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/input-lines");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let part = |above: &str| {
        format!(
            "First paragraph.\n{above}\n\
             \\newpage\n\
             Another paragraph.\n\n\
             \\hbox to 10pt{{Far too wide for the box it is set in.}}\n\n\
             \\parbox{{5pt}}{{Words\nunderfullwordstoolongforitswidth}}\n\n\
             Numbered \\here\\typeout{{numbered \\here}}.\n\n\
             The end.\n"
        )
    };
    // (a file that names `\\inputlineno` itself is read whole, not cut:
    // the number read through a macro)
    let doc = "\\documentclass{article}\n\\newcommand\\here{\\the\\inputlineno}\n\\begin{document}\n\\input{part}\n\n\
               Between the two.\n\n\\input{part}\n\\end{document}\n";
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("inputlines", &dir)
        };
        std::fs::write(dir.join("part.tex"), part("")).expect("the part");
        let mut b = common::build(opts(), Some(&prelude(true)), doc);
        for above in [
            "One line more.\n",
            "Two lines\nmore here.\n\nAnd a paragraph.\n",
            "",
        ] {
            let text = part(above);
            b.edit_file("part.tex", Some(text.as_bytes()));
            std::fs::write(dir.join("part.tex"), &text).expect("the part");
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            let cold = common::build(opts(), Some(&prelude(true)), doc);
            let (mine, want) = (b.output(), cold.output());
            assert_eq!(
                common::first_difference(&mine.0, &want.0),
                None,
                "-j{threads} {above:?}"
            );
            assert_eq!(mine.1, want.1, "-j{threads} {above:?}: the log");
            let line = 1 + text
                .lines()
                .position(|l| l.contains("Numbered"))
                .expect("the line");
            let said = format!("numbered {line}\n");
            assert_eq!(
                mine.1.matches(&said).count(),
                2,
                "-j{threads} {above:?}: {said:?} in {}",
                mine.1
            );
            assert!(
                !r.iter()
                    .any(|x| x.contains("wide") || x.contains("underfullwords")),
                "-j{threads} {above:?}: a piece with a report ran again: {r:?}"
            );
        }
    }
}

/// LaTeX's warnings (an undefined `\ref` or `\cite`, a font shape
/// substituted) are warning diagnostics at the command that raised them,
/// the command with its argument the culprit, so a viewer underlines it;
/// the rerun warning at `\end{document}`, which names nothing, has no
/// culprit. The output is not changed by them.
#[test]
fn latex_warnings_are_diagnostics() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latex-warnings");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let src = "\\documentclass{article}\n\\begin{document}\n\
               See \\ref{nosuchlabel} and \\cite[p.~3]{nosuchkey} here.\n\n\
               Shape {\\fontshape{sc}\\bfseries bold small caps} here.\n\
               \\end{document}\n";
    std::fs::write(dir.join("lw.tex"), src).expect("the document");
    let b = common::build(pdf_opts("lw", &dir), Some(&prelude(true)), src);
    let ds = b.diagnostics();
    for d in &ds {
        eprintln!("{d:?}");
    }
    let at = |needle: &str| {
        let d = ds
            .iter()
            .find(|d| d.message.contains(needle))
            .unwrap_or_else(|| panic!("no diagnostic saying {needle:?}: {ds:#?}"));
        assert_eq!(d.severity, phitex_tex::Severity::Warning, "{d:?}");
        (d.line, &src[d.range.0..d.range.1], d.help.clone())
    };
    let r = at("Reference `nosuchlabel'");
    assert_eq!((r.0, r.1), (3, "\\ref{nosuchlabel}"));
    assert_eq!(at("Citation `nosuchkey'").1, "\\cite[p.~3]{nosuchkey}");
    let (line, culprit, help) = at("Font shape `OT1/cmr/bx/sc' undefined");
    assert_eq!((line, culprit), (5, "\\bfseries"));
    assert!(
        help.as_deref()
            .is_some_and(|h| h.contains("using `OT1/cmr/bx/n' instead")),
        "{help:?}"
    );
    let rerun = at("There were undefined references");
    assert_eq!(rerun.1, "");
    assert!(
        ds.iter().all(|d| d.severity != phitex_tex::Severity::Error),
        "{ds:#?}"
    );
}

/// A file `\input` names and the run cannot find: LaTeX's error, given
/// after the stop it ends in (`why_read_stopped`), has the `\input` with
/// its argument as its culprit.
#[test]
fn a_missing_files_input_is_the_culprit() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latex-missing-file");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let src = "\\documentclass{article}\n\\begin{document}\n\
               Include \\input{nosuchfile} here.\n\\end{document}\n";
    std::fs::write(dir.join("mf.tex"), src).expect("the document");
    let b = common::build(pdf_opts("mf", &dir), Some(&prelude(true)), src);
    let ds = b.diagnostics();
    for d in &ds {
        eprintln!("{d:?}");
    }
    let d = ds
        .iter()
        .find(|d| d.message.contains("nosuchfile.tex' not found"))
        .unwrap_or_else(|| panic!("no diagnostic naming the file: {ds:#?}"));
    assert_eq!(d.severity, phitex_tex::Severity::Error);
    assert_eq!(
        (d.line, &src[d.range.0..d.range.1]),
        (3, "\\input{nosuchfile}")
    );
}

/// A `\label` added to a paragraph adds a key to the `.aux` the
/// `\begin{document}` run reads back per field: that run reads the keys
/// it looks up through the read-back's child (docs/spec-readback-child.md),
/// not the key set, so it does not run again; a `\ref` to the new label
/// finds it, and every edited PDF equals a cold build's.
#[test]
fn a_new_label_leaves_the_read_back_run_standing() {
    use std::fmt::Write as _;
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/rb-label");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    // (pages of text between the start and the end: `\\begin{document}`'s
    // run is not the end's, whose `\\@testdef`s compare every label)
    let filler: String = (0..40).fold(String::new(), |mut s, i| {
        let _ = write!(
            s,
            "Paragraph {i} of filler, long enough to make a line or two when set in the \
             article class's ten point roman, and a few words more.\n\n"
        );
        s
    });
    let doc = |extra: &str, re: &str| {
        format!(
            "\\documentclass{{article}}\n\\usepackage{{hyperref}}\n\
             \\newcommand\\sect[1]{{\\section{{#1}}}}\n\n\\begin{{document}}\n\
             \\section{{Intro}}\\label{{sec:intro}}\nFirst section, see \\ref{{sec:last}}.\n\n\
             \\section{{Middle}}\nText of the second section.{extra}\n\n{filler}\
             \\section{{Last}}\\label{{sec:last}}\nThe end{re}.\n\\end{{document}}\n"
        )
    };
    for threads in [1, 8] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("rblabel", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc("", ""));
        let mut prev = "";
        for (extra, re) in [
            ("\\label{new}", ""),
            ("\\label{new}", ", see \\ref{new}"),
            ("", ""),
            ("\\label{newer}", ""),
        ] {
            b.edit(&doc(extra, re));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            for x in &r {
                eprintln!("  -j{threads} {extra:?} {re:?}: rerun {x}");
            }
            let cold = common::build(opts(), Some(&prelude(true)), &doc(extra, re));
            assert_eq!(
                common::first_difference(&b.output().0, &cold.output().0),
                None,
                "-j{threads} {extra:?} {re:?}"
            );
            // (the `\\begin{document}` run reads the keys it uses;
            // `\\enddocument`'s piece compares every label, `\\@testdef`,
            // so reads the key set, as TeX's does: one run at most)
            let mut on_keys: Vec<&str> = r
                .iter()
                .filter(|x| {
                    !x.starts_with("readback") && !x.starts_with("files") && x.contains("wkeys:")
                })
                .filter_map(|x| x.rsplit_once(" [").map(|(_, n)| n))
                .collect();
            on_keys.sort_unstable();
            on_keys.dedup();
            assert!(
                on_keys.len() <= 1,
                "-j{threads} {extra:?} {re:?}: the document's start read the key set again: {r:?}"
            );
            // (the read-back's child answers for the keys: a key added or
            // gone runs it)
            if extra != prev {
                assert!(
                    r.iter().any(|x| x.starts_with("readback")),
                    "-j{threads} {extra:?} {re:?}: no read-back child ran: {r:?}"
                );
            }
            prev = extra;
        }
    }
}

/// `\begin{document}`'s hooks define a name the `.aux` defines too, after
/// the read-back point (biblatex's own names at its `\begin{document}`):
/// the read-back's definition is made there from the child's output, then
/// the hook's, as the inline read-back leaves them. The pass was refused
/// and made again inline, which read the key set whole: every new
/// `\label` ran `\begin{document}` again (biblatex's example, label 119 ms
/// to 178 ms with the `.aux` read per field, mc 80723). It stands, the
/// name's value is the hook's, and every edited PDF equals a cold build's.
#[test]
fn a_name_defined_after_the_read_back_point_keeps_the_child() {
    use std::fmt::Write as _;
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/rb-defafter");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let filler: String = (0..40).fold(String::new(), |mut s, i| {
        let _ = write!(
            s,
            "Paragraph {i} of filler, long enough to make a line or two when set in the \
             article class's ten point roman, and a few words more.\n\n"
        );
        s
    });
    let doc = |extra: &str| {
        format!(
            "\\documentclass{{article}}\n\\makeatletter\n\
             \\AtBeginDocument{{\\gdef\\rbkey{{hook}}\\global\\let\\rbwas\\rbkeyprev}}\n\
             \\AtEndDocument{{\\immediate\\write\\@auxout{{\\gdef\\string\\rbkey{{aux}}\\gdef\\string\\rbkeyprev{{prev}}}}}}\n\
             \\makeatother\n\\begin{{document}}\n\
             \\section{{Intro}}\\label{{sec:intro}}\nKey \\rbkey, was \\ifdefined\\rbwas\\rbwas\\else none\\fi.\n\n\
             \\section{{Middle}}\nText of the second section.{extra}\n\n{filler}\
             \\section{{Last}}\\label{{sec:last}}\nThe end, see \\ref{{sec:intro}}.\n\\end{{document}}\n"
        )
    };
    for threads in [1, 8] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("rbdefafter", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc(""));
        for extra in ["\\label{new}", "", "\\label{newer}"] {
            b.edit(&doc(extra));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            for x in &r {
                eprintln!("  -j{threads} {extra:?}: rerun {x}");
            }
            let cold = common::build(opts(), Some(&prelude(true)), &doc(extra));
            assert_eq!(
                common::first_difference(&b.output().0, &cold.output().0),
                None,
                "-j{threads} {extra:?}"
            );
            let mut on_keys: Vec<&str> = r
                .iter()
                .filter(|x| {
                    !x.starts_with("readback") && !x.starts_with("files") && x.contains("wkeys:")
                })
                .filter_map(|x| x.rsplit_once(" [").map(|(_, n)| n))
                .collect();
            on_keys.sort_unstable();
            on_keys.dedup();
            assert!(
                on_keys.len() <= 1,
                "-j{threads} {extra:?}: the document's start read the key set again: {r:?}"
            );
        }
    }
}

/// The `.aux` sets a switch globally (KOMA-Script's
/// `\global\csname @altsecnumformattrue\endcsname`, written where an
/// appendix's numbers are letters) and a `\begin{document}` hook sets it
/// back locally (KOMA's `numbers=noenddot` hook, `\@altsecnumformatfalse`),
/// in the document's group: the hook's value stands, as with no such
/// line. The run's local value equals the one it read before the
/// read-back, so it published nothing, and the read-back child defined
/// the `.aux`'s value globally after it: every section number in the
/// biblatex manual's contents got KOMA's end dot, its `.aux` widths 2.4pt
/// more (accl 25680).
#[test]
fn a_hook_setting_back_what_the_aux_set_keeps_its_value() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/rb-setback");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let doc = |line: bool| {
        format!(
            "\\documentclass{{article}}\n\\makeatletter\n\\newif\\if@tswitch\n\
             \\AtBeginDocument{{\\@tswitchfalse}}\n{}\
             \\makeatother\n\\begin{{document}}\n\
             \\section{{Intro}}\\label{{sec:intro}}\nText of the first section.\n\n\
             \\makeatletter Switch \\if@tswitch on\\else off\\fi.\\makeatother\n\n\
             \\section{{Last}}\nThe end, see \\ref{{sec:intro}}.\n\\end{{document}}\n",
            if line {
                "\\AtEndDocument{\\immediate\\write\\@auxout{\\global\\string\\csname\\space @tswitchtrue\\string\\endcsname}}\n"
            } else {
                ""
            }
        )
    };
    for threads in [1, 8] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("rbsetback", &dir)
        };
        let with = common::build(opts(), Some(&prelude(true)), &doc(true));
        let without = common::build(opts(), Some(&prelude(true)), &doc(false));
        assert_eq!(
            common::first_difference(&with.output().0, &without.output().0),
            None,
            "-j{threads}: the .aux's switch outlived the hook's"
        );
    }
}

/// A long `\begin{document}` run that defines a name the `.aux` defines,
/// after the read-back point (uspace's and `ClassicThesis`'s `\@gtempa`;
/// such a pass was once refused and made again inline), in a round whose
/// `.aux` changed: the run's own checks while it runs read the output of
/// its read-back child by the placement's key, not the last run's
/// placement at the same index (a page's node there after a pass that
/// read back inline). Read by the index, the read moved at every check,
/// the run stopped each time and never committed, and its child never
/// ran: no progress (dev 1fa65f142's revert, gate run 41954286956).
#[test]
fn a_long_run_refusing_its_read_back_child_ends() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/rb-long");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    // (`\rbseen` is 1 the first round, 2 once `\rbprev` came back: the
    // `.aux` changes once more after its first read-back; the hook defines
    // `\rbkey`, which the `.aux` defines too, after the read-back point,
    // then works long enough for the run to check its reads)
    let doc = "\\documentclass{article}\n\\makeatletter\n\
         \\AtBeginDocument{\\gdef\\rbkey{run}\\count@\\z@\
         \\@whilenum\\count@<120000\\do{\\advance\\count@\\@ne\\edef\\rbtmp{\\the\\count@}}}\n\
         \\AtEndDocument{\\immediate\\write\\@auxout{\\gdef\\string\\rbkey{aux}\
         \\gdef\\string\\rbprev{}\\gdef\\string\\rbseen{\\ifdefined\\rbprev 2\\else 1\\fi}}}\n\
         \\makeatother\n\\begin{document}\nSeen \\ifdefined\\rbseen\\rbseen\\else 0\\fi, key \\rbkey.\n\\end{document}\n";
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("rblong", &dir)
        };
        let b = common::build(opts(), Some(&prelude(true)), doc);
        let (pdf, log) = b.output();
        assert!(
            !pdf.is_empty(),
            "-j{threads}: a PDF: {}",
            common::log_excerpt(&log)
        );
        assert!(
            !log.contains("no progress"),
            "-j{threads}: {}",
            common::log_excerpt(&log)
        );
    }
}

/// showexpl's code examples under hyperref (the circuitikz manual's
/// shape): each `LTXexample` writes its code to `\jobname.tmp` and reads
/// it back, and after that its caption step's `\hyper@makecurrent`
/// `\let`s `\HyperLocalCurrentHref`, which the last heading set, never
/// looking at it. A section added before them renames every later
/// heading's anchor; the examples' products do not change, so none may
/// run again on that field. Each did, whole: the value its `\let`
/// replaced was read to keep it for the lines written to the file later
/// (`Run::note_pre`); now it is kept unseen, an alias of the value
/// reaching the run. In the manual, 282 such runs, 24.9 s of the edit's
/// 109 s of runs at -j8 (mc 80209, the read found by `PHITEX_LOAD_TRACE`
/// in mc 80218 and 80224). The edited PDF equals a cold build's.
#[test]
fn latex_examples_do_not_run_again_on_a_renamed_anchor() {
    use std::fmt::Write as _;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("latex-showexpl-anchor");
    let _ = std::fs::create_dir_all(&dir);
    let doc = |added: bool| {
        let mut s = String::from(
            "\\documentclass{article}\n\\usepackage{showexpl}\n\\usepackage{hyperref}\n\\begin{document}\n",
        );
        if added {
            s.push_str("\\section{An added section}\n\n");
        }
        for k in 0..4 {
            let _ = write!(
                s,
                "\\section{{Part {k}}}\nSome text of part {k}.\n\n\\begin{{LTXexample}}\nExample {k}: \\textbf{{bold}}.\n\\end{{LTXexample}}\n\nMore text after example {k}.\n\n"
            );
        }
        s.push_str("\\end{document}\n");
        s
    };
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("showexpl", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc(false));
        for added in [true, false] {
            b.edit(&doc(added));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            for x in &r {
                eprintln!(
                    "  -j{threads} added {added}: rerun {}",
                    x.chars().take(200).collect::<String>()
                );
            }
            let cold = common::build(opts(), Some(&prelude(true)), &doc(added));
            let d = common::first_difference(&b.output().0, &cold.output().0);
            assert!(
                d.is_none(),
                "-j{threads} added {added}: edit differs from cold: {d:?}"
            );
            let bad: Vec<&String> = r
                .iter()
                .filter(|x| x.contains("HyperLocalCurrentHref"))
                .collect();
            assert!(
                bad.is_empty(),
                "-j{threads} added {added}: runs again on the anchor a \\let replaces: {bad:?}"
            );
        }
    }
}

/// An alignment's cells that set a switch locally (`\@tempswafalse`,
/// `\let\if@tempswa\iffalse`: xcolor in every tikz-cd matrix cell) do not
/// run again when an edit before them leaves the switch the other way:
/// the name `\let` defines is read for its class alone, all an
/// alignment's `get_next` looks at (§342), not its meaning (tikz-cd's
/// cite edit: 14 matrices ran again on `cs:if@tempswa`, 1.07 s of
/// 1.7 s, accl 25714). Every edited PDF equals a cold build's.
#[test]
fn alignment_cells_setting_a_switch_do_not_read_it() {
    use std::fmt::Write as _;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("latex-align-switch");
    let _ = std::fs::create_dir_all(&dir);
    let doc = |on: bool| {
        let mut s = String::from(
            "\\documentclass{article}\n\\makeatletter\n\
             \\newcommand\\setsw[1]{\\if#1y\\global\\@tempswatrue\\else\\global\\@tempswafalse\\fi}\n\
             \\newcommand\\cellsw{\\@tempswafalse}\n\\makeatother\n\\begin{document}\n",
        );
        let _ = write!(
            s,
            "First paragraph, its switch \\setsw{{{}}} set.\n\n",
            if on { "y" } else { "n" }
        );
        for k in 0..4 {
            let _ = write!(
                s,
                "Text before table {k}.\n\n\\begin{{tabular}}{{ll}}\n\\cellsw a{k} & \\cellsw b{k}\\\\\n\\cellsw c{k} & d{k}\\\\\n\\end{{tabular}}\n\nText after table {k}.\n\n"
            );
        }
        s.push_str("\\end{document}\n");
        s
    };
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("alignsw", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc(false));
        for on in [true, false] {
            b.edit(&doc(on));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            for x in &r {
                eprintln!(
                    "  -j{threads} on {on}: rerun {}",
                    x.chars().take(200).collect::<String>()
                );
            }
            let cold = common::build(opts(), Some(&prelude(true)), &doc(on));
            let d = common::first_difference(&b.output().0, &cold.output().0);
            assert!(
                d.is_none(),
                "-j{threads} on {on}: edit differs from cold: {d:?}"
            );
            let bad: Vec<&String> = r.iter().filter(|x| x.contains("if@tempswa")).collect();
            assert!(
                bad.is_empty(),
                "-j{threads} on {on}: runs again on the switch the cells set: {bad:?}"
            );
        }
    }
}

/// microtype's list of the fonts it has set up (`\MT@font@list`), which
/// the first use of each font adds to (`\xdef\MT@font@list{\MT@font@list
/// \MT@font,}`): a paragraph that first used a font deleted, the list
/// before every later first use moves, yet each later appender adds its
/// font to whatever list reaches it: an append relation the engine
/// recomputes (`Run::append_def`), not a run. Only the paragraph that now
/// first uses the deleted paragraph's font runs again on the list (Grail's
/// `TikZ` figures each ran again, a second each). A name in the list
/// redefined to expand reads in full where an append meets it. Every
/// edited PDF equals a cold build's, at one and eight threads. At eight,
/// the paragraphs after the deleted one wait for the first of them to run
/// again (`sched::mark_front` in the engine): run beside it, they ran on
/// the deleted paragraph's font gone and none in its place, then again on
/// the list (3 builds in 40 on dev, accl 26125; 8 in 30 on acclnode11).
#[test]
fn a_font_list_append_does_not_run_again_when_the_list_moves() {
    font_list_append("mt-append", |first| {
        if first {
            "A paragraph that first uses \\texttt{a typewriter font}.\n\n"
        } else {
            ""
        }
    });
}

/// [`a_font_list_append_does_not_run_again_when_the_list_moves`] with the
/// first use edited away, its paragraph kept: its commit withdraws the
/// font's definitions, and the paragraphs after it wait for the first of
/// them to run again, as after a deletion (8 builds in 30 ran the appenders
/// again on acclnode11 before).
#[test]
fn a_font_list_append_does_not_run_again_when_a_first_use_is_edited_away() {
    font_list_append("mt-append-edit", |first| {
        if first {
            "A paragraph that first uses \\texttt{a typewriter font}.\n\n"
        } else {
            "A paragraph that first uses a typewriter font.\n\n"
        }
    });
}

/// The two font list tests: `head(first)` the document's first paragraph.
fn font_list_append(name: &str, head: fn(bool) -> &'static str) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("latexmk")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let doc = |first: bool| {
        let mut s = String::from(
            "\\documentclass{article}\n\\usepackage{microtype}\n\\makeatletter\n\
             \\def\\L{}\\font\\fa=cmr10 \\font\\fb=cmr12\n\\makeatother\n\\begin{document}\n",
        );
        s.push_str(head(first));
        for p in [
            "The \\texttt{typewriter} again, and \\xdef\\L{\\L\\fa,}a name.",
            "Now \\textbf{bold} type.",
            "Then \\textit{italic} type, \\xdef\\L{\\L\\fb,}and another.",
            "And \\textsc{small capitals}.",
            "With {\\large a larger size} \\def\\fa{x}too.",
            "Some \\textsf{sans serif}.",
            "A {\\small smaller size} \\xdef\\L{\\L y,}and the list: \\meaning\\L.",
            "Last, {\\Large a large one}.",
        ] {
            s.push_str(p);
            s.push_str("\n\n");
        }
        s.push_str("\\end{document}\n");
        s
    };
    for threads in [1, 8] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("mtappend", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc(true));
        for first in [false, true] {
            b.edit(&doc(first));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            for x in &r {
                eprintln!("  -j{threads} first {first}: rerun {x}");
            }
            let cold = common::build(opts(), Some(&prelude(true)), &doc(first));
            assert_eq!(
                common::first_difference(&b.output().0, &cold.output().0),
                None,
                "-j{threads} first {first}"
            );
            let mut on_list: Vec<&str> = r
                .iter()
                .filter(|x| x.contains("MT@font@list"))
                .filter_map(|x| x.rsplit_once(" [").map(|(_, n)| n))
                .collect();
            on_list.sort_unstable();
            on_list.dedup();
            assert!(
                on_list.len() <= 1,
                "-j{threads} first {first}: appenders ran again on the font list: {r:?}"
            );
        }
    }
}

/// pdfTeX's ratios a font has asked for (`pdf_font_elink`, the engine's
/// `pdf:elink:<f>`) grow by each paragraph whose lines first ask for one
/// (microtype's font expansion). A reader cares which ratios are there,
/// not in what order they were first asked: deleting the paragraph that
/// asked first moves the first asks to the paragraphs after it, and only
/// those that now ask first run again on the list, never every paragraph
/// set in the font after them. Every edited PDF equals a cold build's,
/// at one and eight threads.
#[test]
fn a_fonts_expansion_list_reader_does_not_run_again_when_a_first_ask_moves() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latexmk/elink-set");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let words = "expansion stretches and shrinks the glyphs of a line a little, so that \
                 the spaces between the words stay even where a justified paragraph is set";
    let doc = |first: bool| {
        let mut s = String::from(
            "\\documentclass{article}\n\\usepackage[expansion=true,protrusion=true]{microtype}\n\
             \\begin{document}\n",
        );
        if first {
            s += "A first paragraph that asks for the ratios first: ";
            s += words;
            s += ".\n\n";
        }
        for k in 0..10 {
            let w = if k % 2 == 0 { "\\itshape " } else { "" };
            s += "{";
            s += w;
            s += "Paragraph ";
            s += &k.to_string();
            s += ": ";
            s += words;
            s += "; and again ";
            s += words;
            s += ".}\n\n";
        }
        s.push_str("\\end{document}\n");
        s
    };
    for threads in [1, 8] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..pdf_opts("elinkset", &dir)
        };
        let mut b = common::build(opts(), Some(&prelude(true)), &doc(true));
        for first in [false, true] {
            b.edit(&doc(first));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            for x in &r {
                eprintln!("  -j{threads} first {first}: rerun {x}");
            }
            let cold = common::build(opts(), Some(&prelude(true)), &doc(first));
            assert_eq!(
                common::first_difference(&b.output().0, &cold.output().0),
                None,
                "-j{threads} first {first}"
            );
            let mut on_list: Vec<&str> = r
                .iter()
                .filter(|x| x.contains("pdf:elink"))
                .filter_map(|x| x.rsplit_once(" [").map(|(_, n)| n))
                .collect();
            on_list.sort_unstable();
            on_list.dedup();
            assert!(
                on_list.len() <= 2,
                "-j{threads} first {first}: paragraphs ran again on a font's expansion list: {r:?}"
            );
        }
    }
}
