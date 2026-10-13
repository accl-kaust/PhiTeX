//! Builds to profile, not gates: run one under a profiler
//! (`--profile profiling`, frame pointers), ignored by default.
//!
//! - `profile_pages`: the plain `pages` fixture, cold.
//! - `profile_latex_hello`: the LaTeX `hello` document after
//!   `latex.ini` (the prelude, expl3-code.tex included), no time limit.

mod common;

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
    let b = common::build(
        common::options(&format!("./{stem}.tex"), &stem, "", &dir),
        Some(&prelude),
        &src,
    );
    eprintln!("{stem}: {:?}\n{:#?}", t0.elapsed(), b.stats);
}
