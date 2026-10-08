//! Speculative entry: a cold build entered at every paragraph on W
//! workers equals the build in turn, and is faster.

mod toy;

use std::time::Instant;

use toy::*;

fn doc(paras: usize) -> String {
    let mut s = String::from(r"\def\m{alpha beta gamma} \def\n{\hbox{x y z}} \par ");
    for p in 0..paras {
        for w in 0..30 {
            s += &format!("w{}x{} ", p % 7, w % 11);
            if w % 10 == 3 {
                s += r"\m ";
            }
            if w % 10 == 7 {
                s += r"\n ";
            }
        }
        if p % 50 == 0 {
            s += r"{ \def\m{local} \m } ";
        }
        s += r"\write{p} \par ";
    }
    s
}

fn build(src: &str, workers: usize) -> (Doc, f64) {
    let mut d = Doc::new(ids(lex(src)));
    d.g.cfg.workers = workers;
    let t = Instant::now();
    let r = d.g.run();
    let e = t.elapsed().as_secs_f64();
    if std::env::var("PHI_TRACE").is_ok() {
        eprintln!("workers {workers}: {e:.3} s {r:?}");
    }
    (d, e)
}

#[test]
fn speculative_entry_equals_in_turn() {
    let src = doc(60);
    let (a, _) = build(&src, 1);
    for w in [2, 4] {
        let (b, _) = build(&src, w);
        assert_eq!(a.observe(), b.observe(), "workers {w}");
        assert_eq!(a.g.to_text(), b.g.to_text(), "workers {w}");
    }
    // with a counter running through every paragraph (a true chain)
    let src2 = src.replace(r"\write{p}", r"\step \the\count");
    let (a, _) = build(&src2, 1);
    let (b, _) = build(&src2, 4);
    assert_eq!(a.observe(), b.observe());
    assert_eq!(a.g.to_text(), b.g.to_text());
    // and edits after it stay exact
    let mut b = b;
    b.splice(40, 1, lex(r"\step new"));
    b.g.run();
    let mut f = Doc::new(b.toks.clone());
    f.g.run();
    assert_eq!(b.g.to_text(), f.g.to_text());
}

#[test]
fn speculative_entry_is_faster() {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    if threads < 4 {
        eprintln!("skipped: {threads} threads");
        return;
    }
    let src = doc(800);
    let mut best1 = f64::MAX;
    let mut best4 = f64::MAX;
    for _ in 0..3 {
        best1 = best1.min(build(&src, 1).1);
        best4 = best4.min(build(&src, 4).1);
    }
    eprintln!("cold build: 1 worker {best1:.3} s, 4 workers {best4:.3} s ({:.2}x)", best1 / best4);
    assert!(best4 < best1 * 0.8, "1 worker {best1:.3} s, 4 workers {best4:.3} s");
}

#[test]
fn a_group_across_segments() {
    for src in [
        r"a { \def\a{x} \par y \par z } \a \par w",
        r"{ \def\a{x} \par y \par } \par \a",
        r"\def\a{o} { \def\a{x} \par { y \par } } \par \a \par",
        r"\par \par zero \else \step \fi \def \x { 0 } { \def \a { inner } \a \def \x { \step } \section { s2 } \ref \ref { j \the \y { { s1 } { j } dyn o } \def \x { \step \par \barrier } { o } \section { s2 } \x j } \par } \def \b { longerword \x \b { t { \def \a { inner } \a } } no \def \y { \def \b { \step z } f } \def \x { \step } \gdef \a { q } q } ab longerword k } \ref { { }",
    ] {
        let (a, _) = build(src, 1);
        let (b, _) = build(src, 3);
        assert_eq!(a.g.to_text(), b.g.to_text(), "{src}");
        assert_eq!(a.observe(), b.observe(), "{src}");
    }
}
