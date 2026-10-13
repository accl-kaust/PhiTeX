//! Gates for LaTeX: the format made from `latex.ini` (`latex.ltx` with
//! `texsys.cfg`, `hyphen.cfg`, the font configuration) as the prelude,
//! then the documents of `oracle/fixtures/latex/docs` run as `latex`
//! runs them: the DVI file (its preamble's date masked) and the files
//! written (`NAME.writes`) equal TeX Live's, on the first run from
//! nothing (`NAME.dvi`) and at the fixpoint of the files read back
//! (`NAME.fix.dvi`); then `pdflatex` on the same documents through the
//! PDF writer (`oracle/fixtures/latex/pdf`), and one-word edits.
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

/// `=== name length` headers, each before the file's text.
fn parse_writes(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(head) = rest.strip_prefix("=== ") {
        let Some((line, after)) = head.split_once('\n') else {
            break;
        };
        let Some((name, n)) = line.rsplit_once(' ') else {
            break;
        };
        let n: usize = n.parse().unwrap_or(0);
        let end = after.char_indices().nth(n).map_or(after.len(), |(i, _)| i);
        out.push((name.to_string(), after[..end].to_string()));
        rest = &after[end..];
    }
    out
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
        "{name}: {} nodes run, {} runs, {} stops, {} gated, ready mean {:.2}, {} live, {} rounds, {} loop rounds, {} waits, starved {}, wall {:?}\n  waits by kind {:?}",
        s.nodes_run,
        s.runs,
        s.stops,
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
    let writes = common::read(&dir.join(format!("{name}{suffix}.writes")));
    let files = b.files();
    for (n, w) in parse_writes(&writes) {
        let got = files
            .iter()
            .find(|(m, _)| *m == n)
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        if let Some(d) = common::first_difference(got.as_bytes(), w.as_bytes()) {
            bad.push(format!("{n} {d}"));
        }
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

/// `pdflatex` on the documents with a PDF oracle: the PDF file and the
/// written files byte for byte.
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
        let dir = if docs().join(format!("{name}.tex")).exists() {
            docs()
        } else {
            common::fixtures().join("latex/pdfdocs")
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
        let writes = common::read(&oracles.join(format!("{name}.writes")));
        let files = b.files();
        for (n, w) in parse_writes(&writes) {
            let got = files
                .iter()
                .find(|(m, _)| *m == n)
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            if let Some(d) = common::first_difference(got.as_bytes(), w.as_bytes()) {
                bad.push(format!("{n} {d}"));
            }
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
        let same = b.output().0 == cold.output().0 && b.files() == cold.files();
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

/// `pdflatex` on hello + 30 paragraphs (`pdfdocs/paras30.tex`): the
/// document environment's body is sliced into its paragraphs, one node
/// each (`\begin{document}` leaves its group to the engine as the scope
/// they run in); the PDF equals pdfTeX's cold and after a one-word edit,
/// which reruns a few document nodes.
#[test]
#[ignore = "heavy: the LaTeX format"]
fn latex_paragraph_nodes() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("paras30.tex"));
    let want =
        std::fs::read(common::fixtures().join("latex/pdf/paras30.pdf")).expect("the oracle");
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/cache/pdflatex.fmt");
    let opts = || Options {
        prelude_bytes: std::fs::read(&cache).ok(),
        time_limit: None,
        ..options("paras30", &dir, "", false, true)
    };
    let cached = cache.exists();
    let mut b = common::build(opts(), Some(&prelude(true)), &src);
    if !cached {
        let _ = std::fs::create_dir_all(cache.parent().expect("dir"));
        if let Some(bytes) = b.export_prelude() {
            let _ = std::fs::write(&cache, bytes);
        }
    }
    let regions = b.stats.nodes_text_frames;
    eprintln!("cold: {regions} document nodes; {}", report("paras30", &b));
    for d in b.diagnostics() {
        eprintln!("{d:?}");
    }
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("paras30.pdf cold: {d}");
    }
    assert!(regions >= 30, "{regions} document nodes for 30 paragraphs");
    let at = src.find("Paragraph 17 of the text: the quick").expect("p17") + 35;
    let new = format!("{}quack{}", &src[..at], &src[at + 5..]);
    let t0 = std::time::Instant::now();
    b.edit(&new);
    let pages = b.pages().len() as u64;
    let s = &b.stats;
    eprintln!(
        "edit {:?}: {} nodes run, {} runs, {} pages; {}",
        t0.elapsed(),
        s.nodes_run,
        s.runs,
        pages,
        report("paras30", &b)
    );
    let cold = common::build(opts(), Some(&prelude(true)), &new);
    assert_eq!(b.output().0, cold.output().0, "edit differs from cold");
    assert!(
        b.stats.nodes_run <= 3 + pages + 2,
        "edit reran {} nodes ({} pages)",
        b.stats.nodes_run,
        pages
    );
}
