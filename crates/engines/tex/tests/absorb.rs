//! Texts sent while a build runs: the build ends with the result for
//! the latest one, equal to a cold build of it, and says which version
//! it is for.

mod common;

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

fn story() -> (std::path::PathBuf, String, String) {
    let fx = common::fixtures().join("plain");
    let src = common::read(&fx.join("story.tex"));
    let want = std::fs::read(fx.join("story.dvi")).expect("story.dvi");
    (fx, src, common::dvi_comment(&want))
}

/// `src` with its first "the" (after `\mark`) turned into "eht".
fn edited(src: &str, k: usize) -> String {
    let body = src.find("\\mark").unwrap_or(0);
    let mut at = body;
    for _ in 0..=k {
        at = src[at + 1..].find(" the ").map_or(at, |i| at + 1 + i);
    }
    format!("{} eht {}", &src[..at], &src[at + 5..])
}

#[test]
fn a_text_sent_between_edits() {
    let (fx, src, comment) = story();
    let opts = || common::options("./story.tex", "story", &comment, &fx);
    let mut b = common::build(opts(), Some(&common::plain()), &src);
    assert_eq!(b.version(), 0);
    let tx = b.edit_sender();
    let new = edited(&src, 0);
    let v = tx.send(&new);
    assert!(b.absorb_pending());
    assert_eq!(b.version(), v);
    let cold = common::build(opts(), Some(&common::plain()), &new);
    assert!(common::first_difference(&b.output().0, &cold.output().0).is_none());
    assert!(!b.absorb_pending());
}

#[test]
fn texts_sent_while_an_edit_runs() {
    let (fx, src, comment) = story();
    let seen: Arc<Mutex<Vec<(usize, u64, u64)>>> = Arc::default();
    let s2 = seen.clone();
    let opts = || common::options("./story.tex", "story", &comment, &fx);
    let mut b = common::build(
        phitex_tex::Options {
            on_page_settled: Some(Arc::new(move |p, h, v| {
                s2.lock().unwrap().push((p, h, v));
            })),
            ..opts()
        },
        Some(&common::plain()),
        &src,
    );
    let first = seen.lock().unwrap().len();
    assert!(first > 0, "the cold build's pages");
    let tx = b.edit_sender();
    let texts: Vec<String> = (0..4).map(|k| edited(&src, k)).collect();
    let last = texts[3].clone();
    let t = std::thread::spawn(move || {
        for x in &texts[1..] {
            std::thread::sleep(std::time::Duration::from_millis(5));
            tx.send(x);
        }
    });
    b.edit(&edited(&src, 0));
    t.join().unwrap();
    b.absorb_pending();
    assert_eq!(b.version(), 4);
    let cold = common::build(opts(), Some(&common::plain()), &last);
    assert!(
        common::first_difference(&b.output().0, &cold.output().0).is_none(),
        "the result is not the latest text's"
    );
    let seen = seen.lock().unwrap();
    assert!(seen[first..].iter().all(|&(_, _, v)| v >= 1));
}

/// A plain document of `n` paragraphs, slow enough that a text sent
/// while it builds comes during the evaluation.
fn slow(n: usize) -> String {
    let words = "the printer admired a quiet galley of careful proofs while the \
        reader looked at every ligature kern and hyphen in the long paragraph";
    let mut t = String::from("\\hsize=6in \\vsize=8.5in \\parindent=20pt\n");
    for k in 0..n {
        let _ = write!(t, "Paragraph {k}: {words} {words} {words} {words}.\n\n");
    }
    t.push_str("\\bye\n");
    t
}

#[test]
fn a_text_sent_while_the_cold_build_evaluates() {
    let fx = common::fixtures().join("plain");
    let comment = " TeX output 2026.01.01:0000".to_owned();
    let opts = || common::options("./slow.tex", "slow", &comment, &fx);
    let src = slow(1500);
    // (the first paragraph, run early, and the last, run late, both edited)
    let new = src
        .replacen("Paragraph 0: the", "Paragraph 0: eht", 1)
        .replacen("Paragraph 1499: the", "Paragraph 1499: eht", 1);
    let progress = Arc::new(phitex_tex::Progress::new(
        std::thread::available_parallelism().map_or(1, usize::from),
    ));
    let tx = phitex_tex::EditSender::default();
    let (p2, tx2, new2) = (progress.clone(), tx.clone(), new.clone());
    // (sent once the document's own nodes run: a text node past the start)
    let t = std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        while t0.elapsed() < std::time::Duration::from_secs(600) {
            let deep = p2
                .running()
                .iter()
                .flatten()
                .any(|&(op, off)| op == 1 && off > 2000 && off != phitex_tex::Progress::INSIDE);
            if deep {
                // (then the same text again: what a watcher sends on
                // every write; those change nothing)
                let first = tx2.send(&new2);
                let mut last = first;
                for _ in 0..20 {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    last = tx2.send(&new2);
                }
                return Some((first, last));
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        None
    });
    let b = common::build(
        phitex_tex::Options {
            progress: Some(progress),
            edits: Some(tx.clone()),
            ..opts()
        },
        Some(&common::plain()),
        &src,
    );
    let (v, last) = t.join().unwrap().expect("the document's nodes ran");
    // (what the cold build's evaluation took, before a later one runs)
    let absorbed = b.stats.absorbed.clone();
    let mut b = b;
    b.absorb_pending();
    assert_eq!(b.version(), last);
    eprintln!("absorbed (version, step): {absorbed:?}");
    assert!(
        absorbed.iter().any(|&(x, step)| x >= v && step > 0),
        "the text was not absorbed during the evaluation: {absorbed:?}"
    );
    let cold = common::build(opts(), Some(&common::plain()), &new);
    assert!(
        common::first_difference(&b.output().0, &cold.output().0).is_none(),
        "the result is not the edited text's cold build"
    );
}

/// Texts sent while the cold build evaluates that differ from each other
/// (a watcher reading a file half written: empty, cut short, whole):
/// the result is the last text's cold build.
#[test]
fn texts_cut_short_while_the_cold_build_evaluates() {
    let fx = common::fixtures().join("plain");
    let comment = " TeX output 2026.01.01:0000".to_owned();
    let opts = || common::options("./slow.tex", "slow", &comment, &fx);
    let src = slow(600);
    let new = src.replacen("Paragraph 599: the", "Paragraph 599: eht", 1);
    let progress = Arc::new(phitex_tex::Progress::new(
        std::thread::available_parallelism().map_or(1, usize::from),
    ));
    let tx = phitex_tex::EditSender::default();
    let (p2, tx2, new2) = (progress.clone(), tx.clone(), new.clone());
    let t = std::thread::spawn(move || {
        loop {
            let deep = p2
                .running()
                .iter()
                .flatten()
                .any(|&(op, off)| op == 1 && off > 2000 && off != phitex_tex::Progress::INSIDE);
            if deep {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        for x in [
            String::new(),
            new2[..new2.len() / 3].to_owned(),
            new2.clone(),
        ] {
            tx2.send(&x);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    });
    let mut b = common::build(
        phitex_tex::Options {
            progress: Some(progress),
            edits: Some(tx.clone()),
            ..opts()
        },
        Some(&common::plain()),
        &src,
    );
    t.join().unwrap();
    eprintln!("absorbed (version, step): {:?}", b.stats.absorbed);
    b.absorb_pending();
    assert_eq!(b.version(), 3);
    let cold = common::build(opts(), Some(&common::plain()), &new);
    assert!(!b.pages().is_empty(), "no pages");
    assert!(
        common::first_difference(&b.output().0, &cold.output().0).is_none(),
        "the result is not the last text's cold build"
    );
}

/// An empty document edited to a whole one: the result is the whole
/// one's cold build (the cut-short texts' case without the timing).
#[test]
fn an_empty_document_edited_to_a_whole_one() {
    let fx = common::fixtures().join("plain");
    let comment = " TeX output 2026.01.01:0000".to_owned();
    let opts = || common::options("./slow.tex", "slow", &comment, &fx);
    let src = slow(30);
    let mut b = common::build(opts(), Some(&common::plain()), "");
    b.edit(&src[..src.len() / 3]);
    b.edit(&src);
    let cold = common::build(opts(), Some(&common::plain()), &src);
    assert!(!b.pages().is_empty(), "no pages");
    assert!(
        common::first_difference(&b.output().0, &cold.output().0).is_none(),
        "the result is not the whole text's cold build"
    );
}
