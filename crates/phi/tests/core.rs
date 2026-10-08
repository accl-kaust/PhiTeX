#![allow(clippy::pedantic)]

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
    assert_eq!(
        value_of(r"\def\a{o} { \def\a{l1} { \gdef\a{g} } }", "a"),
        "g"
    );
    assert_eq!(
        value_of(r"\def\a{o} { \gdef\a{g} } { \def\a{l} }", "a"),
        "g"
    );
    assert_eq!(value_of(r"{ \def\a{l} }", "a"), "undefined");
    assert_eq!(
        value_of(r"\def\a{o} { { \def\a{l2} } \def\a{l1} }", "a"),
        "o"
    );
    // a group opened in one paragraph and closed in another
    assert_eq!(
        value_of(r"\def\a{o} { \def\a{l} \par x \par } \a", "a"),
        "o"
    );
    // the readers inside see the local one
    let mut d = Doc::new(ids(lex(r"\def\a{o} { \def\a{l} \write{\a} } \write{\a}")));
    d.g.run();
    let _ = &mut d;
}

/// A word changed in one paragraph of hundreds: the steps of that
/// paragraph run at most, and the scans step a few elements, whatever
/// the document's length.
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
    // (the step before the edit and the word's; then, since a step's
    // interior is transient, each later word of the paragraph, whose
    // push reads the changed `par@`: the paragraph's chain, never the
    // document's)
    assert!(r.steps <= 13, "{r:?}");
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
    let at = d
        .toks
        .iter()
        .position(|t| t.1 == Tok::Word("s17".into()))
        .unwrap();
    d.splice(at, 1, lex("renamed"));
    let r = d.g.run();
    assert_eq!(r.iterations, 1, "{r:?}");
    assert_eq!(d.g.scanned(Op::TocLine), 1, "{r:?}");
    let mut f = Doc::new(d.toks.clone());
    f.g.run();
    assert_eq!(d.g.to_text(), f.g.to_text());
    // an edit elsewhere: the TOC is predicted right, nothing iterates
    let at = d
        .toks
        .iter()
        .position(|t| t.1 == Tok::Word("text".into()))
        .unwrap();
    d.splice(at, 1, lex("prose"));
    let r = d.g.run();
    assert_eq!(r.iterations, 0, "{r:?}");
    assert_eq!(d.g.scanned(Op::TocLine), 0);
}

/// A woken sealed region unseals and stays live (DESIGN 7.11): the
/// first edit in it re-runs the region, the next edit there re-runs
/// exactly the steps it would with sealing off.
#[test]
fn a_second_edit_in_a_sealed_region_is_step_precise() {
    let mut src = String::new();
    for p in 0..300 {
        for w in 0..12 {
            src += &format!("w{p}x{w} ");
        }
        src += "\\par ";
    }
    let mut a = Doc::new(ids(lex(&src)));
    a.g.cfg.seal = 4;
    let r0 = a.g.run();
    assert!(r0.sealed > 1000, "{r0:?}");
    let mut b = Doc::new(ids(lex(&src)));
    b.g.run();
    let at = 150 * 13 + 5;
    a.splice(at, 1, lex("other"));
    b.splice(at, 1, lex("other"));
    let (ra, rb) = (a.g.run(), b.g.run());
    assert!(
        ra.steps >= rb.steps,
        "first edit: sealed {ra:?}, live {rb:?}"
    );
    eprintln!("first edit: sealed {} steps, live {}", ra.steps, rb.steps);
    assert_eq!(a.observe(), b.observe());
    a.splice(at + 2, 1, lex("again"));
    b.splice(at + 2, 1, lex("again"));
    let (ra, rb) = (a.g.run(), b.g.run());
    assert_eq!(
        ra.steps, rb.steps,
        "second edit: sealed {ra:?}, live {rb:?}"
    );
    assert_eq!(
        ra.evals, rb.evals,
        "second edit: sealed {ra:?}, live {rb:?}"
    );
    assert_eq!(a.observe(), b.observe());
}

/// A `\pageref`-like slot changes a digit with an equal width: the step
/// that shows the page runs again; what reads only the width (a field of
/// a definition two steps on) does not, across a step's edge and across a
/// sealed region's (its net definitions keep their fields' versions).
#[test]
fn an_equal_width_page_change_stops_at_the_width() {
    for seal in [0, 2] {
        let mut src =
            String::from(r"\step \step \step X \pagelabel{k} \pageref{k} \copypr a b \par ");
        src += r"c \usew d e f g h \par ";
        for i in 0..20 {
            src += &format!("w{i} x{i} y{i} z{i} \\par ");
        }
        let mut d = Doc::new(ids(lex(&src)));
        d.g.cfg.seal = seal;
        d.g.run();
        let k = Slot(phi::ver::hash64("k"));
        assert_eq!(d.g.slot(k).field(0).map(|v| v.int()), Some(3));
        if seal > 0 {
            assert!(d.g.to_text().contains("sealed"), "nothing sealed");
        }
        let before = toy::USEW.with(|c| c.get());
        d.splice(3, 1, lex(r"\step"));
        let r = d.g.run();
        assert_eq!(d.g.slot(k).field(0).map(|v| v.int()), Some(4));
        assert_eq!(
            toy::USEW.with(|c| c.get()),
            before,
            "seal {seal}: the width's reader ran again: {r:?}"
        );
        let mut f = Doc::new(d.toks.clone());
        f.g.run();
        assert_eq!(d.observe(), f.observe(), "seal {seal}");
    }
}

/// Step interiors are reported by op (debug), and bounded: a paragraph ten
/// or a hundred times longer gives the same largest interior.
#[test]
fn interiors_are_reported_and_bounded() {
    let mut maxes = Vec::new();
    for words in [10, 100, 1000] {
        let mut src = String::from(r"\def\a{x y} \step ");
        for w in 0..words {
            src += &format!("w{w} ");
            if w % 7 == 0 {
                src += r"\a \the\count { \def\b{z} \b } ";
            }
        }
        src += r"\par \section{s} \toc \par";
        let mut d = Doc::new(ids(lex(&src)));
        d.g.cfg.debug = true;
        d.g.run();
        let sizes = d.g.interior_sizes();
        let doc = sizes
            .iter()
            .find(|x| x.0 == Op::Doc)
            .expect("the document's steps");
        assert!(doc.1 >= words, "{sizes:?}");
        assert!(doc.3 <= doc.2, "{sizes:?}");
        maxes.push(doc.2);
    }
    assert!(maxes.windows(2).all(|w| w[0] == w[1]), "{maxes:?}");
}

/// Appends to a list that do not read it are cheap (DESIGN 7.7): editing
/// one append re-runs that append's step and the uses after it, never the
/// other appends; a use sees only the appends before it.
#[test]
fn an_append_list_reruns_only_what_follows_the_edit() {
    let mut src = String::new();
    for p in 0..40 {
        src += &format!(r"w{p} \addto{{h}}{{a{p}}} \par ");
        if p == 20 {
            src += r"\usehook{h} \par ";
        }
    }
    src += r"\usehook{h} \par ";
    let mut d = Doc::new(ids(lex(&src)));
    d.g.run();
    // the edit: the 30th append's word (after the first use)
    let at = d
        .toks
        .iter()
        .position(|t| t.1.text() == "a30")
        .expect("a30");
    let before = toy::USEH.with(|c| c.get());
    d.splice(at, 1, lex("z30"));
    let r = d.g.run();
    let used = toy::USEH.with(|c| c.get()) - before;
    // (the step of the append, and the last use only: the first use is
    // before the edit, and no other append runs)
    assert_eq!(used, 1, "{r:?}");
    assert!(r.steps <= 2, "{r:?}");
    let mut f = Doc::new(d.toks.clone());
    f.g.run();
    assert_eq!(d.observe(), f.observe());
    assert_eq!(d.g.to_text(), f.g.to_text());
    let v = d.g.name_id(b"hook@h").and_then(|n| d.g.name_value(n));
    let TV::Word(w, _) = v.expect("the hook's last use") else {
        panic!("a word")
    };
    assert!(w.contains("a29+z30+a31"), "{w}");
    assert!(w.starts_with("a0+"), "{w}");
}

/// A cancelled run returns at once with its work still queued; the next
/// run finishes it, and the result is the uncancelled one.
#[test]
fn a_cancelled_run_leaves_its_work_for_the_next() {
    let mut src = String::new();
    for p in 0..50 {
        src += &format!(r"\def\x{{{p}}} w{p} \x \par ");
    }
    let mut d = Doc::new(ids(lex(&src)));
    d.g.run();
    let at = d.toks.iter().position(|t| t.1.text() == "w3").expect("w3");
    d.splice(at, 1, lex("changed"));
    let token = d.g.cancel_token();
    token.store(true, std::sync::atomic::Ordering::Relaxed);
    let r = d.g.run();
    assert!(r.cancelled, "{r:?}");
    assert_eq!(r.steps, 0, "{r:?}");
    let r = d.g.run();
    assert!(!r.cancelled);
    assert!(r.steps > 0);
    let mut f = Doc::new(d.toks.clone());
    f.g.run();
    assert_eq!(d.observe(), f.observe());
    assert_eq!(d.g.to_text(), f.g.to_text());
}

/// The memo store (DESIGN 7.8): a graph that loads another's image
/// evaluates none of the memo ops it already holds (here the hooks' uses
/// and every line breaker element), and builds the same document; a bad
/// image leaves the store empty and the build exact.
#[test]
fn a_memo_image_spares_a_fresh_build_its_memo_ops() {
    let mut src = String::new();
    for p in 0..30 {
        src += &format!(r"w{p} word{p} more{p} \addto{{h}}{{a{p}}} \par ");
    }
    src += r"\usehook{h} \par ";
    let count = || (toy::USEH.with(|c| c.get()), toy::BRK.with(|c| c.get()));
    let mut a = Doc::new(ids(lex(&src)));
    a.g.set_memo_budget(1 << 20);
    let c0 = count();
    a.g.run();
    let c1 = count();
    assert!(c1.0 > c0.0 && c1.1 > c0.1, "{c0:?} {c1:?}");
    let (entries, bytes, ..) = a.g.memo_stats();
    assert!(entries > 0 && bytes > 0);
    let img = a.g.save_memo(&toy::TvCodec);
    // a fresh graph, the image loaded: no hook use, no breaker element
    let mut b = Doc::new(ids(lex(&src)));
    b.g.set_memo_budget(1 << 20);
    b.g.load_memo(&toy::TvCodec, &img).expect("the image");
    b.g.run();
    assert_eq!(count(), c1);
    let (_, _, probes, hits) = b.g.memo_stats();
    assert!(hits > 0 && hits == probes, "{probes} {hits}");
    let mut f = Doc::new(ids(lex(&src)));
    f.g.run();
    assert_eq!(b.observe(), f.observe());
    assert_eq!(b.g.to_text(), f.g.to_text());
    // an edit: only the paragraph's new words are stepped
    let at = b
        .toks
        .iter()
        .position(|t| t.1.text() == "word7")
        .expect("word7");
    b.splice(at, 1, lex("other"));
    let c1 = count();
    b.g.run();
    let c2 = count();
    assert!(c2.1 > c1.1 && c2.1 - c1.1 <= 4, "{c1:?} {c2:?}");
    assert_eq!(c2.0, c1.0, "{c1:?} {c2:?}");
    // a bad image: an empty store, the same build
    let mut e = Doc::new(ids(lex(&src)));
    e.g.set_memo_budget(1 << 20);
    assert!(e.g.load_memo(&toy::TvCodec, &img[..img.len() - 1]).is_err());
    assert_eq!(e.g.memo_stats().0, 0);
    e.g.run();
    assert_eq!(e.observe(), f.observe());
    // a small budget is kept
    let mut s = Doc::new(ids(lex(&src)));
    s.g.set_memo_budget(256);
    s.g.run();
    assert!(s.g.memo_stats().1 <= 256);
    assert_eq!(s.observe(), f.observe());
}

/// CSE (DESIGN 7.10): a step's two equal pure leaves are one node; a
/// third reading the same name after the step defined it is not merged.
#[test]
fn equal_leaves_of_a_step_are_one_and_a_definition_parts_them() {
    let src = r"\step \step \twice \par \the\y \par";
    let mut d = Doc::new(ids(lex(src)));
    let r = d.g.run();
    assert_eq!(r.merged, 1, "{r:?}");
    let val = |d: &Doc, n: &str| {
        d.g.name_id(n.as_bytes())
            .and_then(|m| d.g.name_value(m))
            .map(|v| v.int())
    };
    assert_eq!(val(&d, "y"), Some(10));
    assert_eq!(val(&d, "count"), Some(3));
    // an edit before it: the merged leaf follows
    let at = d
        .toks
        .iter()
        .position(|t| t.1.text() == "\\step")
        .expect("step");
    d.splice(at, 1, vec![]);
    d.g.run();
    assert_eq!(val(&d, "y"), Some(7));
    let mut f = Doc::new(d.toks.clone());
    f.g.run();
    assert_eq!(d.observe(), f.observe());
    assert_eq!(d.g.to_text(), f.g.to_text());
}

/// The profile (DESIGN 7.21): steps and their re-runs on edits by op,
/// regions that re-ran, sampled leaf costs; a snapshot that a later
/// graph loads; `tune` does nothing while work is queued, and its memo
/// opt-in keeps every value (check mode on).
#[test]
fn a_profiled_graph_reports_and_tunes_without_changing_results() {
    let mut src = String::new();
    for p in 0..200 {
        src += &format!(r"w{p} \step \the\count x{p} \par ");
    }
    let mut d: DocP<true> = DocP::new(ids(lex(&src)));
    d.g.cfg.check = true;
    d.g.cfg.memo_probe_ns = 0;
    d.g.cfg.tune_min_samples = 1;
    d.g.cfg.tune_min_keyed = 4;
    d.g.run();
    let rep = d.g.profile();
    let doc = rep
        .ops
        .iter()
        .find(|(o, _)| *o == Op::Doc)
        .map(|x| x.1.clone())
        .expect("the document's steps");
    assert!(doc.steps > 500 && doc.reruns == 0, "{doc:?}");
    assert!(
        rep.ops.iter().any(|(_, s)| s.timed > 0 && s.evals > 0),
        "{:?}",
        rep.ops
    );
    // edits: the edited paragraph's steps re-run, as a region
    for k in 0..6 {
        let at = d.toks.iter().position(|t| t.1.text() == "w7").expect("w7");
        d.splice(at, 1, lex(&format!("w7{k} \\step")));
        d.g.run();
        let at = d
            .toks
            .iter()
            .position(|t| t.1.text() == format!("w7{k}"))
            .expect("edit");
        d.splice(at, 2, lex("w7"));
        assert!(d.g.tune().busy, "work is queued");
        d.g.run();
        d.g.tune();
    }
    let rep = d.g.profile();
    let doc = rep
        .ops
        .iter()
        .find(|(o, _)| *o == Op::Doc)
        .expect("doc")
        .1
        .clone();
    assert!(doc.reruns > 0, "{doc:?}");
    assert!(
        !rep.regions.is_empty() && rep.regions[0].1.runs >= 2,
        "{:?}",
        rep.regions
    );
    let mut f = Doc::new(d.toks.clone());
    f.g.run();
    assert_eq!(d.observe(), f.observe());
    // the snapshot round-trips and a later graph starts from it
    let snap = d.g.snapshot();
    let back = phi::profile::Snapshot::from_bytes(&snap.to_bytes()).expect("a snapshot");
    assert_eq!(back, snap);
    let mut e: DocP<true> = DocP::new(d.toks.clone());
    e.g.load_profile(&back);
    e.g.run();
    let t = e.g.tune();
    let memo_before = snap.ops.iter().filter(|o| o.memo).count();
    assert!(t.memo_in.len() >= memo_before.min(1), "{t:?}");
    let rep = e.g.profile();
    let doc = rep
        .ops
        .iter()
        .find(|(o, _)| *o == Op::Doc)
        .expect("doc")
        .1
        .clone();
    assert!(
        doc.reruns > 0 && doc.steps > 1000,
        "the earlier session's counts: {doc:?}"
    );
    assert_eq!(e.observe(), f.observe());
}

/// A graph that does not profile reports nothing and `tune` does nothing.
#[test]
fn an_unprofiled_graph_has_an_empty_profile() {
    let mut d = Doc::new(ids(lex(r"a \step b \par")));
    d.g.run();
    assert!(d.g.profile().ops.is_empty());
    assert_eq!(d.g.tune(), phi::profile::Tuned::default());
}

/// `cx.read` after the step closed a group reads what the group had
/// shadowed, as a leaf's operand does (check mode on).
#[test]
fn a_read_after_the_steps_own_close_reads_past_it() {
    let val = |d: &Doc, n: &str| {
        d.g.name_id(n.as_bytes())
            .and_then(|m| d.g.name_value(m))
            .map(|v| v.int())
    };
    let mut d = Doc::new(ids(lex(r"\step \step \scoped \par")));
    d.g.cfg.check = true;
    d.g.run();
    assert_eq!(val(&d, "sc"), Some(2));
    assert_eq!(val(&d, "count"), Some(2));
}
