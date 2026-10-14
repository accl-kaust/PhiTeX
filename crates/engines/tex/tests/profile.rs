//! Builds to profile, not gates: run one under a profiler
//! (`--profile profiling`, frame pointers), ignored by default.
//!
//! - `profile_pages`: the plain `pages` fixture, cold.
//! - `profile_latex_hello`: the LaTeX `hello` document after
//!   `latex.ini` (the prelude, expl3-code.tex included), no time limit.

mod common;

// the CLI's allocator: with the system's, concurrent page steps
// contend in it and the measures do not show the binary's scaling
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[test]
#[ignore = "profiling run"]
fn profile_pages() {
    let fx = common::fixtures().join("plain");
    let src = common::read(&fx.join("pages.tex"));
    let prelude = common::plain();
    let t0 = std::time::Instant::now();
    let b = common::build(
        common::options("./pages.tex", "pages", "", &fx),
        Some(&prelude),
        &src,
    );
    eprintln!("pages: {:?}, {} nodes run", t0.elapsed(), b.stats.nodes_run);
}

#[test]
#[ignore = "profiling run (heavy: the LaTeX format)"]
fn profile_latex_hello() {
    let dir = common::fixtures().join("latex/hello");
    let src = common::read(&dir.join("hello.tex"));
    let opts = phitex_tex::Options {
        epoch: Some(0),
        time_limit: None,
        prelude_jobname: "latex".to_owned(),
        ..common::options("./hello.tex", "hello", "", &dir)
    };
    let t0 = std::time::Instant::now();
    let b = common::build(opts, Some("\\input latex.ini\n"), &src);
    eprintln!("hello: {:?}, {} nodes run", t0.elapsed(), b.stats.nodes_run);
}

/// A plain document named by `PHITEX_PROFILE_DOC` (a path), cold, at
/// `PHITEX_THREADS` threads: its stats printed.
#[test]
#[ignore = "profiling run (a document given by path)"]
fn profile_doc() {
    let Some(path) = std::env::var_os("PHITEX_PROFILE_DOC") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let dir = path.parent().expect("a directory").to_path_buf();
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("doc")
        .to_owned();
    let src = common::read(&path);
    let prelude = common::plain();
    let t0 = std::time::Instant::now();
    let mut opts = common::options(&format!("./{stem}.tex"), &stem, "", &dir);
    if let Some(n) = std::env::var("PHITEX_SPEC_BUDGET")
        .ok()
        .and_then(|x| x.parse().ok())
    {
        opts.speculation_budget = n;
    }
    let built = common::build(opts, Some(&prelude), &src);
    eprintln!("{stem}: {:?}\n{:#?}", t0.elapsed(), built.stats);
    let mut bd = built;
    if std::env::var_os("PHITEX_PROFILE_EDIT").is_some() {
        // a word in the middle paragraph, its letters rotated (same length)
        let mid = src.len() / 2;
        let p = src[mid..].find("\n\n").map_or(mid, |k| mid + k + 2);
        let s = p + src[p..].find(|c: char| c.is_ascii_lowercase()).unwrap_or(0);
        let e = s + src[s..]
            .find(|c: char| !c.is_ascii_lowercase())
            .unwrap_or(0);
        let mut w: Vec<u8> = src.as_bytes()[s..e].to_vec();
        w.rotate_left(1);
        let new = format!("{}{}{}", &src[..s], String::from_utf8_lossy(&w), &src[e..]);
        let t1 = std::time::Instant::now();
        bd.edit(&new);
        let st = &bd.stats;
        eprintln!(
            "EDIT {:?} -> {:?}: {:?}, nodes_run {}, runs {}, wall {:?}, busy {:?}",
            &src[s..e],
            String::from_utf8_lossy(&w),
            t1.elapsed(),
            st.nodes_run,
            st.runs,
            st.wall,
            st.busy
        );
    }
}

/// The `n`th paragraph's (blank-line separated) byte range in `src`.
fn paragraph(src: &str, n: usize) -> (usize, usize) {
    let mut starts = vec![0];
    let mut i = 0;
    while let Some(k) = src[i..].find("\n\n") {
        starts.push(i + k + 2);
        i += k + 2;
    }
    let s = starts[n.min(starts.len() - 1)];
    let e = src[s..].find("\n\n").map_or(src.len(), |k| s + k);
    (s, e)
}

/// One word of the `n`th paragraph changed: its first plain word of
/// four or more letters gets its first letter swapped for `Q`.
fn change_word(src: &str, n: usize) -> String {
    let (s, e) = paragraph(src, n);
    let mut at = s;
    for w in src[s..e].split(' ') {
        if w.len() >= 4 && w.bytes().all(|b| b.is_ascii_lowercase()) {
            return format!("{}Q{}", &src[..at], &src[at + 1..]);
        }
        at += w.len() + 1;
    }
    panic!("no word in paragraph {n}");
}

/// (measure) `PHITEX_EP_CHECK`: the edited build's output against a
/// cold build of the same text.
fn same_as_cold(b: &phitex_tex::Build, cold: impl FnOnce() -> phitex_tex::Build) {
    if std::env::var_os("PHITEX_EP_CHECK").is_none() {
        return;
    }
    let c = cold();
    let same = b.output().0 == c.output().0;
    eprintln!(
        "== EDITPATH identical to cold: {}",
        if same { "yes" } else { "no" }
    );
}

fn edit_line(what: &str, t: std::time::Duration, b: &phitex_tex::Build) {
    let s = &b.stats;
    eprintln!("== EDITPATH {what}: critical path {:?}", s.critical_path);
    if std::env::var_os("PHITEX_CHAIN").is_some() {
        for (w, k, f) in b.critical_chain() {
            eprintln!(
                "   chain {k:9} {f:24} {}",
                w.chars().take(100).collect::<String>()
            );
        }
    }
    eprintln!(
        "== EDITPATH {what}: wall {t:?}, evaluate wall {:?}, busy {:?}, {} nodes run, {} page steps, {} loop rounds, {} rounds, {} pages",
        s.wall,
        s.busy,
        s.nodes_run,
        s.page_steps,
        s.loop_rounds,
        s.rounds,
        b.pages().len()
    );
}

/// The edit path (`PHITEX_TEX_TIME=1` for the phases): the plain
/// document `PHITEX_PROFILE_DOC` built cold (`\pdfoutput=1` prefixed),
/// then one word changed in its middle paragraph, one in its first, and
/// a paragraph inserted in the middle.
#[test]
#[ignore = "a measure: the edit path"]
fn editpath_doc() {
    let Some(path) = std::env::var_os("PHITEX_PROFILE_DOC") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let dir = path.parent().expect("a directory").to_path_buf();
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("doc")
        .to_owned();
    let src = format!("\\pdfoutput=1 {}", common::read(&path));
    let opts = || common::options(&format!("./{stem}.tex"), &stem, "", &dir);
    let prelude = common::plain();
    let t0 = std::time::Instant::now();
    let mut b = common::build(opts(), Some(&prelude), &src);
    edit_line("cold", t0.elapsed(), &b);
    let pars = src.matches("\n\n").count();
    let mid = pars / 2;
    let e1 = change_word(&src, mid);
    let t0 = std::time::Instant::now();
    b.edit(&e1);
    edit_line("word middle", t0.elapsed(), &b);
    same_as_cold(&b, || common::build(opts(), Some(&prelude), &e1));
    let e2 = change_word(&e1, 1);
    let t0 = std::time::Instant::now();
    b.edit(&e2);
    edit_line("word first", t0.elapsed(), &b);
    let (s, e) = paragraph(&e2, mid);
    let e3 = format!("{}{}\n\n{}", &e2[..s], &e2[s..e], &e2[s..]);
    let t0 = std::time::Instant::now();
    b.edit(&e3);
    edit_line("insert paragraph middle", t0.elapsed(), &b);
    same_as_cold(&b, || common::build(opts(), Some(&prelude), &e3));
    assert!(b.is_pdf());
}

/// The LaTeX `hello` with `pdflatex.ini`, the format cached at
/// `PHITEX_EP_FMT` (written when missing): the cold start, then one
/// word changed (`PHITEX_TEX_TIME=1` for the phases).
#[test]
#[ignore = "a measure: the cold start and an edit, LaTeX"]
fn editpath_hello() {
    let Some(fmt) = std::env::var_os("PHITEX_EP_FMT") else {
        return;
    };
    let fmt = std::path::PathBuf::from(fmt);
    let dir = common::fixtures().join("latex/hello");
    let src = common::read(&dir.join("hello.tex"));
    let opts = || phitex_tex::Options {
        epoch: Some(0),
        time_limit: None,
        prelude_jobname: "pdflatex".to_owned(),
        prelude_bytes: std::fs::read(&fmt).ok(),
        ..common::options("./hello.tex", "hello", "", &dir)
    };
    let cached = fmt.exists();
    let t0 = std::time::Instant::now();
    let mut b = common::build(opts(), Some("\\input pdflatex.ini\n"), &src);
    edit_line(
        if cached {
            "cold start, cached format"
        } else {
            "cold start, format built"
        },
        t0.elapsed(),
        &b,
    );
    if !cached {
        match b.export_prelude() {
            Some(bytes) => {
                let _ = std::fs::create_dir_all(fmt.parent().expect("a directory"));
                let r = std::fs::write(&fmt, &bytes);
                eprintln!("format written: {} bytes, {r:?}", bytes.len());
            }
            None => eprintln!("format not complete: not cached"),
        }
    }
    let i = src.find("\\begin{document}").expect("a body") + 16;
    let w = i + src[i..]
        .find(|c: char| c.is_ascii_lowercase())
        .expect("a word");
    let e1 = format!("{}Q{}", &src[..w], &src[w + 1..]);
    let t0 = std::time::Instant::now();
    b.edit(&e1);
    edit_line("word", t0.elapsed(), &b);
    same_as_cold(&b, || {
        common::build(opts(), Some("\\input pdflatex.ini\n"), &e1)
    });
    assert!(b.is_pdf());
}
