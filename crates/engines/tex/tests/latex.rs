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
    for threads in [1u64, 8] {
        let opts = Options {
            threads: Some(usize::try_from(threads).unwrap_or(1)),
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
            bad.push(format!(
                "-j {threads}: {runs} finisher runs for {fires} fires"
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
    assert!(
        own >= want && taken == 0,
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
