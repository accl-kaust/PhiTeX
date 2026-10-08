//! The acceptance properties of DESIGN 7.13 that are counts.

mod toy;

use std::sync::Arc;

use phi::{Class, ElemId, Graph, Sel, Seq, Slot, Value};
use toy::*;

/// A field nobody reads changes: nothing downstream runs.
#[test]
fn field_level_stop() {
    let mut g: Graph<Toy> = Graph::new();
    let x = g.input(TV::rec(vec![TV::Int(1), TV::Int(2)]));
    let mut last = g.leaf(Op::Double, Class::Pure, &[(x, Sel::field(0))]);
    for _ in 0..10 {
        last = g.leaf(Op::Sum, Class::Pure, &[(last, Sel::WHOLE)]);
    }
    g.run();
    assert_eq!(g.value(last).int(), 2);
    g.set(x, TV::rec(vec![TV::Int(1), TV::Int(99)]));
    let r = g.run();
    assert_eq!(r.evals, 0, "{r:?}");
    g.set(x, TV::rec(vec![TV::Int(5), TV::Int(99)]));
    let r = g.run();
    assert_eq!(r.evals, 11, "{r:?}");
    assert_eq!(g.value(last).int(), 10);
    // an equal result stops the change: Double of 5 is 10 either way
    g.set(x, TV::rec(vec![TV::Int(5), TV::Int(7)]));
    assert_eq!(g.run().evals, 0);
}

fn words(n: usize, seed: u64) -> Vec<(ElemId, TV)> {
    let mut x = seed | 1;
    (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let w = 1 + (x % 9) as i64;
            (ElemId(i as u64 + 1), TV::Word("w".into(), w))
        })
        .collect()
}

/// A scan resumes at the edit and stops where its state meets the old
/// run's: the elements stepped are exactly those.
#[test]
fn scan_resumes_and_converges() {
    let xs = words(2000, 7);
    let mut g: Graph<Toy> = Graph::new();
    let input = g.input(TV::Seq(Seq::from_vec(xs.clone())));
    let init = g.input(TV::Dp(Arc::new((Vec::new(), Vec::new()))));
    let w = g.input(TV::Int(WIDTH));
    let sc = g.scan(Op::Break, (input, Sel::WHOLE), init, &[w]);
    let r = g.run();
    assert_eq!(r.scanned, 2000);
    let before = g.state_vers(sc);
    for at in [1000usize, 17, 1999, 0, 555] {
        let mut ys = xs.clone();
        ys[at].1 = TV::Word("w".into(), 1 + xs[at].1.int() % 9);
        g.set(input, TV::Seq(Seq::from_vec(ys.clone())));
        let r = g.run();
        let after = g.state_vers(sc);
        // the oracle: from the edit to the first state equal to the old one
        let mut want = 0;
        for i in at..ys.len() {
            want += 1;
            if after[i] == before[i] {
                break;
            }
        }
        assert_eq!(r.scanned, want, "edit at {at}");
        assert!(want < 40, "converges within a few lines: {want}");
        // and the result equals a fresh scan's
        let mut f: Graph<Toy> = Graph::new();
        let fi = f.input(TV::Seq(Seq::from_vec(ys)));
        let f0 = f.input(TV::Dp(Arc::new((Vec::new(), Vec::new()))));
        let fw = f.input(TV::Int(WIDTH));
        let fs = f.scan(Op::Break, (fi, Sel::WHOLE), f0, &[fw]);
        f.run();
        assert_eq!(f.value(fs).ver(), g.value(sc).ver());
        assert_eq!(f.state_vers(fs), after);
        // back to the original
        g.set(input, TV::Seq(Seq::from_vec(xs.clone())));
        g.run();
    }
}

/// A label whose value is predicted right costs no iteration.
#[test]
fn cross_run_prediction() {
    let src = r"\step \step \label{k} a b \ref{k} \par";
    let mut d = Doc::new(ids(lex(src)));
    let r = d.g.run();
    assert_eq!(r.iterations, 1, "the first run learns the label");
    assert_eq!(d.g.slot(Slot(phi::ver::hash64("k"))).int(), 2);
    d.splice(6, 1, lex("c"));
    let r = d.g.run();
    assert_eq!(r.iterations, 0, "{r:?}");
    // a cold start predicted from the last run: none either
    let mut c = Doc::new(d.toks.clone());
    c.g.predict(Slot(phi::ver::hash64("k")), TV::Int(2));
    assert_eq!(c.g.run().iterations, 0);
    // the label moves: one iteration, only the readers of the slot
    d.splice(0, 1, Vec::new());
    let r = d.g.run();
    assert_eq!(r.iterations, 1);
    assert_eq!(d.g.slot(Slot(phi::ver::hash64("k"))).int(), 1);
}

/// A slot that never settles is reported, not cut off silently.
#[test]
fn oscillation_is_reported() {
    let mut g: Graph<Toy> = Graph::new();
    let s = Slot(42);
    let c = g.cross(s);
    // publishes 1 when it read 0, 0 when it read 1
    g.leaf(Op::IsZero, Class::Publish(s), &[(c, Sel::WHOLE)]);
    let r = g.run();
    assert_eq!(r.oscillating.len(), 1, "{r:?}");
    assert_eq!(r.oscillating[0].0, s);
    assert!(r.oscillating[0].1.len() >= 5);
}

fn value_of(src: &str, name: &str) -> String {
    let mut d = Doc::new(ids(lex(src)));
    d.g.cfg.check = true;
    d.g.run();
    match d.g.name_id(name.as_bytes()).and_then(|n| d.g.name_value(n)) {
        Some(TV::Toks(t)) => t.iter().map(Tok::text).collect::<Vec<_>>().join(" "),
        Some(v) => format!("{v:?}"),
        None => "undefined".into(),
    }
}

/// Groups as TeX's save stack (§279–§283), including `\global`.
#[test]
fn groups_restore_like_the_save_stack() {
    assert_eq!(value_of(r"\def\a{o} { \def\a{l} }", "a"), "o");
    assert_eq!(value_of(r"\def\a{o} { \def\a{l} \gdef\a{g} }", "a"), "g");
    assert_eq!(value_of(r"{ \gdef\a{g} \def\a{l} }", "a"), "g");
    assert_eq!(value_of(r"\def\a{o} { \def\a{l1} { \gdef\a{g} } }", "a"), "g");
    assert_eq!(value_of(r"\def\a{o} { \gdef\a{g} } { \def\a{l} }", "a"), "g");
    assert_eq!(value_of(r"{ \def\a{l} }", "a"), "undefined");
    assert_eq!(value_of(r"\def\a{o} { { \def\a{l2} } \def\a{l1} }", "a"), "o");
    // a group opened in one paragraph and closed in another
    assert_eq!(value_of(r"\def\a{o} { \def\a{l} \par x \par } \a", "a"), "o");
    // the readers inside see the local one
    let mut d = Doc::new(ids(lex(r"\def\a{o} { \def\a{l} \write{\a} } \write{\a}")));
    d.g.run();
    let _ = &mut d;
}

/// A word changed in one paragraph of hundreds: two steps run (the one
/// before the edit, whose successor's key comes from the element after
/// it, and the word's), and the scans step a few elements, whatever the
/// document's length.
#[test]
fn a_word_edit_costs_the_edit() {
    let mut src = String::new();
    for p in 0..300 {
        for w in 0..12 {
            src += &format!("w{p}x{w} ");
        }
        src += "\\par ";
    }
    let mut d = Doc::new(ids(lex(&src)));
    d.g.run();
    let at = 150 * 13 + 5;
    d.splice(at, 1, lex("other"));
    let r = d.g.run();
    assert_eq!(r.steps, 2, "{r:?}");
    assert!(r.scanned < 20, "{r:?}");
    assert!(r.evals < 20, "{r:?}");
}

/// Moving one label wakes only its own readers.
#[test]
fn moving_a_label_wakes_its_readers_only() {
    let src = r"\label{a} \step \label{b} \step \label{c} \ref{a} \ref{b} \ref{c} \par \ref{b} x \ref{a} \par";
    let mut d = Doc::new(ids(lex(src)));
    d.g.run();
    let b = Slot(phi::ver::hash64("b"));
    assert_eq!(d.g.slot(b).int(), 1);
    // \label{b} (tokens 5..9) moved after the second \step
    let toks: Vec<Tok> = d.toks[5..9].iter().map(|t| t.1.clone()).collect();
    assert_eq!(toks[0], Tok::Cs("label".into()));
    d.splice(5, 4, Vec::new());
    d.splice(6, 0, toks);
    let r = d.g.run();
    assert_eq!(d.g.slot(b).int(), 2);
    assert_eq!(r.iterations, 1, "{r:?}");
    assert_eq!(r.cross_evals, 2, "the two refs to b, no other: {r:?}");
    let mut f = Doc::new(d.toks.clone());
    f.g.run();
    assert_eq!(d.observe(), f.observe());
    assert_eq!(d.g.to_text(), f.g.to_text());
}

/// Renaming one table-of-contents entry runs exactly one TOC line again.
#[test]
fn renaming_a_toc_entry_runs_one_line() {
    let mut src = String::from(r"\toc \par ");
    for k in 0..40 {
        src += &format!(r"\section{{s{k}}} text {k} \par ");
    }
    let mut d = Doc::new(ids(lex(&src)));
    let r = d.g.run();
    assert_eq!(r.iterations, 1);
    assert_eq!(d.g.scanned(Op::TocLine), 40);
    let at = d.toks.iter().position(|t| t.1 == Tok::Word("s17".into())).unwrap();
    d.splice(at, 1, lex("renamed"));
    let r = d.g.run();
    assert_eq!(r.iterations, 1, "{r:?}");
    assert_eq!(d.g.scanned(Op::TocLine), 1, "{r:?}");
    let mut f = Doc::new(d.toks.clone());
    f.g.run();
    assert_eq!(d.g.to_text(), f.g.to_text());
    // an edit elsewhere: the TOC is predicted right, nothing iterates
    let at = d.toks.iter().position(|t| t.1 == Tok::Word("text".into())).unwrap();
    d.splice(at, 1, lex("prose"));
    let r = d.g.run();
    assert_eq!(r.iterations, 0, "{r:?}");
    assert_eq!(d.g.scanned(Op::TocLine), 0);
}
