//! F6: a cold build's output does not depend on the order its pieces run
//! in (the brief's test 5): big at one worker in random order 400, and at
//! eight workers in random order 7, gives what priority order gives.

use phitex_flow::fuzz;
use phitex_flow::{Order, Output, Reads};

fn big() -> Option<fuzz::Doc> {
    fuzz::corpus().into_iter().find(|d| d.name == "big")
}

fn build(doc: &fuzz::Doc, threads: usize, order: Order) -> Output {
    let s = fuzz::Schedule { threads, reads: Reads::Suspend, order, lean: true };
    let mut o = s.options(false, std::env::var_os("PHITEX_ORDER_DUMP").is_some());
    if let Ok(n) = std::env::var("PHITEX_CUT_LINES") {
        o.cut_lines = n.parse().unwrap_or(usize::MAX);
    }
    fuzz::builder(doc, &doc.files).update(&doc.text, o)
}

fn first_diff(a: &[String], b: &[String]) -> String {
    let i = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    format!("line {i}: {:?} / {:?}", a.get(i), b.get(i))
}

#[test]
fn big_in_any_order_is_big() {
    let Some(doc) = big() else { return };
    if let Ok(d) = std::env::var("PHITEX_ORDER_SRC") {
        let _ = std::fs::write(format!("{d}/big.tex"), &doc.text);
        for (n, t) in &doc.files {
            let _ = std::fs::write(format!("{d}/{n}"), t);
        }
    }
    let base = build(&doc, 1, Order::Priority);
    assert!(base.stats.pages >= 40, "big: {} pages", base.stats.pages);
    assert!(base.errors.is_empty(), "{:?}", &base.errors[..base.errors.len().min(5)]);
    for (threads, order) in [(1, Order::Random(400)), (8, Order::Random(7)), (8, Order::Random(7)), (8, Order::Random(7))] {
        let o = build(&doc, threads, order);
        if let Ok(d) = std::env::var("PHITEX_ORDER_DUMP") {
            let _ = std::fs::write(format!("{d}/base.dump"), &base.dump);
            let _ = std::fs::write(format!("{d}/other.dump"), &o.dump);
        }
        assert!(o.log == base.log, "{threads} workers, {order:?}: another log, {}", first_diff(&o.log, &base.log));
        assert!(o.dvi == base.dvi, "{threads} workers, {order:?}: another DVI");
    }
}

/// F's F6 repro: two equal paragraphs under different `\hsize`s, then an
/// `\input` that lays the sequence out again (story's plain format, its
/// body replaced). A piece keeps its own place whether or not it ran
/// before the restructure; another place's run of the same text is never
/// kept as it is. Every order gives priority's.
#[test]
fn equal_texts_keep_their_places() {
    let Some(doc) = fuzz::corpus().into_iter().find(|d| d.name == "story") else { return };
    let p = "Words of a paragraph that breaks into lines at the width it is set at, and again, and again, and again, and again.\n\n";
    let text = format!("{}\\hsize=300pt\n\n{p}\\hsize=100pt\n\n{p}\\input fzsub\n\n\\end\n", &doc.text[..doc.body]);
    let mut files: Vec<(String, String)> = doc.files.iter().filter(|f| f.0 != fuzz::SUB).cloned().collect();
    files.push((fuzz::SUB.to_string(), "Sub file text.\n".to_string()));
    let run = |threads: usize, order: Order| {
        let s = fuzz::Schedule { threads, reads: Reads::Suspend, order, lean: true };
        fuzz::builder(&doc, &files).update(&text, s.options(false, false))
    };
    let base = run(1, Order::Priority);
    assert!(base.errors.is_empty(), "{:?}", base.errors);
    for seed in 0..40 {
        for threads in [1, 8] {
            let o = run(threads, Order::Random(seed));
            assert!(o.dvi == base.dvi && o.log == base.log, "{threads} workers, random:{seed}: another output");
        }
    }
}
