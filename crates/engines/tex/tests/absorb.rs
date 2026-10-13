//! Texts sent while a build runs: the build ends with the result for
//! the latest one, equal to a cold build of it, and says which version
//! it is for.

mod common;

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
