//! Property tests (`DESIGN.md` §7.17.10, step 1): random programs and
//! edit sequences, the runtime against the stub's oracle. A seeded
//! generator keeps the workspace free of dependencies; a failure names
//! its seed.

#![allow(clippy::many_single_char_names)]

use partex_ssa::stub::generate::{self, Feats, Rng};
use partex_ssa::stub::oracle::{self, Streams};
use partex_ssa::stub::{Func, Seen, Stub, Val, expected_misses, source, streams_text, to_pmap};
use partex_ssa::{Config, Outcome, PMap, PStack, PVec, Runtime, Trace, Value};

fn out_text(o: &Outcome<Stub>) -> String {
    partex_ssa::stub::link(&o.effects)
}

fn ostreams(s: &Streams) -> std::collections::BTreeMap<String, Vec<String>> {
    s.iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                v.iter().map(|x| x.as_str().to_string()).collect(),
            )
        })
        .collect()
}

/// One build on both sides; checks output, streams and trips; returns
/// the runtime's outcome and the oracle's build.
fn both(
    rt: &mut Runtime<Stub>,
    prev: &Streams,
    lines: &[String],
    seed: u64,
) -> (Outcome<Stub>, oracle::OBuild) {
    let o = oracle::build(lines, prev, 5);
    let r = rt.build(&Stub, Func::Main, &[source(lines)]);
    assert_eq!(
        out_text(&r),
        o.output,
        "seed {seed}: output\n{}",
        lines.join("\n")
    );
    assert_eq!(
        streams_text(&r.streams),
        ostreams(&o.streams),
        "seed {seed}: streams"
    );
    assert_eq!(r.trips.len(), o.trips.len(), "seed {seed}: trips");
    assert_eq!(r.converged, o.converged, "seed {seed}: convergence");
    assert!(r.trips.len() <= 5);
    (r, o)
}

fn feats(r: &mut Rng) -> Feats {
    Feats {
        vars: r.below(4) > 0,
        refs: r.below(2) > 0,
        cites: r.below(3) == 0,
        lineno: r.below(3) == 0,
    }
}

/// (a), (b), (f): outputs, streams and trips equal the oracle's over
/// random edit sequences, and the calls re-run are exactly the oracle's
/// calls with a changed read.
#[test]
fn oracle_and_exactness() {
    for seed in 0..300u64 {
        let mut r = Rng(seed);
        let f = feats(&mut r);
        let paras = 1 + r.below(12);
        let mut lines = generate::program(&mut r, paras, f);
        let mut rt = Runtime::<Stub>::new(Config {
            keep: usize::MAX,
            ..Config::default()
        });
        let mut prev = Streams::new();
        let mut seen = Seen::new();
        for step in 0..8 {
            let (out, ob) = both(&mut rt, &prev, &lines, seed);
            for (k, t) in ob.trips.iter().enumerate() {
                let mut exp = Vec::new();
                expected_misses(&mut seen, &t.calls, &mut exp);
                assert_eq!(
                    out.trips[k].reran,
                    exp,
                    "seed {seed} step {step} trip {k}: re-run calls\n{}",
                    lines.join("\n")
                );
            }
            prev = ob.streams;
            generate::edit(&mut r, &mut lines, f);
        }
    }
}

/// (h): recording off gives the same output, streams and trips.
#[test]
fn record_off_is_identical() {
    for seed in 0..200u64 {
        let mut r = Rng(seed ^ 0x55);
        let f = feats(&mut r);
        let n = 1 + r.below(10);
        let mut lines = generate::program(&mut r, n, f);
        let mut on = Runtime::<Stub>::new(Config::default());
        let mut off = Runtime::<Stub>::new(Config {
            record: false,
            ..Config::default()
        });
        for _ in 0..5 {
            let a = on.build(&Stub, Func::Main, &[source(&lines)]);
            let b = off.build(&Stub, Func::Main, &[source(&lines)]);
            assert_eq!(out_text(&a), out_text(&b), "seed {seed}");
            assert_eq!(
                streams_text(&a.streams),
                streams_text(&b.streams),
                "seed {seed}"
            );
            assert_eq!(a.trips.len(), b.trips.len(), "seed {seed}");
            generate::edit(&mut r, &mut lines, f);
        }
    }
}

fn reran(o: &Outcome<Stub>) -> Vec<Func> {
    o.trips
        .iter()
        .flat_map(|t| t.reran.iter().map(|x| x.0))
        .filter(|f| *f != Func::Main)
        .collect()
}

/// (e): a doubled space re-runs exactly one tokenizer call (and the
/// root, whose input is the source).
#[test]
fn doubled_space_reruns_one_tokenizer() {
    for seed in 0..200u64 {
        let mut r = Rng(seed ^ 0xe);
        let f = feats(&mut r);
        let n = 1 + r.below(10);
        let mut lines = generate::program(&mut r, n, f);
        let mut rt = Runtime::<Stub>::new(Config::default());
        let _ = rt.build(&Stub, Func::Main, &[source(&lines)]);
        let before = rt.build(&Stub, Func::Main, &[source(&lines)]);
        let candidates: Vec<usize> = (0..lines.len())
            .filter(|&i| lines[i].contains(' '))
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let i = candidates[r.below(candidates.len())];
        lines[i] = generate::double_space(&lines[i]);
        let o = rt.build(&Stub, Func::Main, &[source(&lines)]);
        assert_eq!(reran(&o), vec![Func::Tokenize], "seed {seed}");
        // An oscillating program iterates again, all hits after the first.
        if before.converged {
            assert_eq!(o.trips.len(), 1, "seed {seed}");
        }
    }
}

/// (d): a moved paragraph re-runs no tokenizer (no line changed); in a
/// program of text paragraphs, no paragraph either.
#[test]
fn moved_paragraph() {
    for seed in 0..200u64 {
        let mut r = Rng(seed ^ 0xd);
        let text = seed % 2 == 0;
        let f = if text { Feats::TEXT } else { feats(&mut r) };
        let n = 2 + r.below(10);
        let mut lines = generate::program(&mut r, n, f);
        let mut rt = Runtime::<Stub>::new(Config::default());
        let prev = rt.build(&Stub, Func::Main, &[source(&lines)]);
        let _ = prev;
        let _ = rt.build(&Stub, Func::Main, &[source(&lines)]);
        let k = generate::paragraphs(&lines).len();
        generate::move_paragraph(&mut lines, r.below(k), r.below(k + 1));
        let o = rt.build(&Stub, Func::Main, &[source(&lines)]);
        let ran = reran(&o);
        assert!(!ran.contains(&Func::Tokenize), "seed {seed}: {ran:?}");
        if text {
            assert!(!ran.contains(&Func::Para), "seed {seed}: {ran:?}");
        }
    }
}

/// (c): a change that leaves a paragraph's outputs equal re-runs
/// nothing after it.
#[test]
fn cutoff() {
    for seed in 0..200u64 {
        let mut r = Rng(seed ^ 0xc);
        let f = feats(&mut r);
        let n = 1 + r.below(10);
        let mut lines = generate::program(&mut r, n, f);
        let mut rt = Runtime::<Stub>::new(Config::default());
        let _ = rt.build(&Stub, Func::Main, &[source(&lines)]);
        let _ = rt.build(&Stub, Func::Main, &[source(&lines)]);
        // `set X N` becomes `set X N-1 add X 1`... with N-1 >= 0: the
        // same writes and the same box.
        let Some(i) = (0..lines.len()).find(|&i| {
            lines[i]
                .split(' ')
                .collect::<Vec<_>>()
                .windows(3)
                .any(|w| w[0] == "set" && w[2] != "0" && !lines[i].contains("begin"))
        }) else {
            continue;
        };
        let toks: Vec<&str> = lines[i].split(' ').collect();
        let j = toks
            .windows(3)
            .position(|w| w[0] == "set" && w[2] != "0")
            .expect("found");
        let n: i64 = toks[j + 2].parse().expect("a number");
        let mut new: Vec<String> = toks.iter().map(|s| (*s).to_string()).collect();
        new[j + 2] = format!("{} add {} 1", n - 1, toks[j + 1]);
        // `add` reads its variable, which `set` wrote just before: internal.
        // A group open around it would save differently; skip those.
        if toks[..j].contains(&"end") || toks[..j].contains(&"begin") {
            continue;
        }
        lines[i] = new.join(" ");
        let o = rt.build(&Stub, Func::Main, &[source(&lines)]);
        let ran = reran(&o);
        assert!(
            ran.iter().all(|f| matches!(f, Func::Tokenize | Func::Para)),
            "seed {seed}: {ran:?}\n{}",
            lines.join("\n")
        );
        assert_eq!(
            ran.iter().filter(|f| **f == Func::Tokenize).count(),
            1,
            "seed {seed}"
        );
    }
}

/// (g): a stream nothing loads never causes a trip.
#[test]
fn unloaded_streams_cause_no_trip() {
    let f = Feats {
        vars: true,
        refs: false,
        cites: false,
        lineno: true,
    };
    for seed in 0..200u64 {
        let mut r = Rng(seed ^ 0x9);
        let n = 1 + r.below(10);
        let mut lines = generate::program(&mut r, n, f);
        lines.push("label L0 note n1".to_string());
        let mut rt = Runtime::<Stub>::new(Config::default());
        for _ in 0..4 {
            let o = rt.build(&Stub, Func::Main, &[source(&lines)]);
            assert_eq!(o.trips.len(), 1, "seed {seed}");
            generate::edit(&mut r, &mut lines, f);
        }
    }
}

fn filler(n: usize) -> String {
    "x".repeat(n)
}

/// (f): a bistable program reaches the fixed point its previous build's
/// stores lead to, as the oracle's Jacobi iteration does.
#[test]
fn bistable() {
    // Around the bistable width, from both starting points.
    let a = |w: usize| format!("{} ref X", filler(w));
    for w in 40..50 {
        let lines = vec![a(w), String::new(), "label X".to_string()];
        let mut rt = Runtime::<Stub>::new(Config::default());
        let (o1, ob) = both(&mut rt, &Streams::new(), &lines, w as u64);
        // From the other fixed point's streams.
        let mut other = Streams::new();
        other.insert(partex_ssa::stub::Addr::stream("aux"), vec![Val::str("X 1")]);
        let mut rt2 = Runtime::<Stub>::new(Config::default());
        rt2.set_streams(to_pmap(&other));
        let (o2, _) = both(&mut rt2, &other, &lines, w as u64 + 100);
        let _ = (o1, ob, o2);
    }
    // 46 characters: with `??` or `ii` the paragraph is 49 characters (4
    // lines, the label's paragraph goes to page 2), with `i` 48 (3 lines,
    // it stays on page 1): two fixed points.
    let lines = vec![
        format!("{} ref X", filler(46)),
        String::new(),
        "label X".to_string(),
    ];
    let mut rt = Runtime::<Stub>::new(Config::default());
    let (fresh, _) = both(&mut rt, &Streams::new(), &lines, 1);
    let mut other = Streams::new();
    other.insert(partex_ssa::stub::Addr::stream("aux"), vec![Val::str("X 1")]);
    let mut rt2 = Runtime::<Stub>::new(Config::default());
    rt2.set_streams(to_pmap(&other));
    let (from_one, _) = both(&mut rt2, &other, &lines, 2);
    assert_ne!(out_text(&fresh), out_text(&from_one), "two fixed points");
    assert!(out_text(&fresh).contains(" ii"), "{}", out_text(&fresh));
    assert_eq!(fresh.trips.len(), 2);
    assert_eq!(from_one.trips.len(), 1);
    assert!(fresh.converged && from_one.converged);
}

/// (i): the trace's text form round-trips, for random builds and
/// rebuilds.
#[test]
fn text_form_round_trips() {
    for seed in 0..100u64 {
        let mut r = Rng(seed ^ 0x7);
        let f = feats(&mut r);
        let n = 1 + r.below(6);
        let mut lines = generate::program(&mut r, n, f);
        let mut rt = Runtime::<Stub>::new(Config::default());
        for _ in 0..3 {
            let _ = rt.build(&Stub, Func::Main, &[source(&lines)]);
            let t = rt.trace();
            let s = t.to_text();
            let back = Trace::parse(&s).unwrap_or_else(|e| panic!("seed {seed}: {e}\n{s}"));
            assert_eq!(back, t, "seed {seed}");
            assert_eq!(back.to_text(), s, "seed {seed}");
            generate::edit(&mut r, &mut lines, f);
        }
    }
}

/// A rebuild's dump marks the first differing read of a miss.
#[test]
fn dump_marks_misses() {
    let mut lines = vec!["set x 1".to_string(), String::new(), "print x".to_string()];
    let mut rt = Runtime::<Stub>::new(Config::default());
    let _ = rt.build(&Stub, Func::Main, &[source(&lines)]);
    lines[0] = "set x 2".to_string();
    let _ = rt.build(&Stub, Func::Main, &[source(&lines)]);
    let s = rt.trace().to_text();
    assert!(
        s.contains("call @para(") && s.contains("miss=@var:x"),
        "{s}"
    );
    assert!(s.contains(" hit"), "{s}");
    assert!(s.contains("= phi ["), "{s}");
}

/// Persistent containers: versions by content, sharing, field reads.
#[test]
fn containers() {
    for seed in 0..100u64 {
        let mut r = Rng(seed);
        let mut v: PVec<Val> = PVec::new();
        let mut model: Vec<i64> = Vec::new();
        for _ in 0..300 {
            let n = i64::try_from(r.below(50)).expect("small");
            match r.below(4) {
                0 | 1 => {
                    let i = r.below(model.len() + 1);
                    v.insert(i, Val::Int(n));
                    model.insert(i, n);
                }
                2 if !model.is_empty() => {
                    let i = r.below(model.len());
                    v.remove(i);
                    model.remove(i);
                }
                _ if !model.is_empty() => {
                    let i = r.below(model.len());
                    v.set(i, Val::Int(n));
                    model[i] = n;
                }
                _ => {}
            }
            let fresh = PVec::from_vec(model.iter().map(|x| Val::Int(*x)).collect());
            assert_eq!(
                v.version(),
                fresh.version(),
                "seed {seed}: version by content"
            );
            assert_eq!(v.iter().map(Val::int).collect::<Vec<_>>(), model);
        }
        let big = PVec::from_vec((0..5000).map(Val::Int).collect());
        let mut b2 = big.clone();
        b2.set(1234, Val::Int(-1));
        assert!(big.shared_nodes(&b2) > 100, "structural sharing");
        assert_ne!(big.version(), b2.version());

        let mut m: PMap<u64, Val> = PMap::new();
        let mut mm = std::collections::BTreeMap::new();
        for _ in 0..300 {
            let k = r.below(64) as u64;
            if r.below(3) == 0 {
                assert_eq!(m.remove(&k).map(|x| x.int()), mm.remove(&k));
            } else {
                let n = i64::try_from(r.below(9)).expect("small");
                m.insert(k, Val::Int(n));
                mm.insert(k, n);
            }
            let mut fresh: PMap<u64, Val> = PMap::new();
            for (k, v) in mm.iter().rev() {
                fresh.insert(*k, Val::Int(*v));
            }
            assert_eq!(
                m.version(),
                fresh.version(),
                "seed {seed}: map version by content"
            );
            assert_eq!(m.len(), mm.len());
        }
        let s = PStack::<Val>::new().push(Val::Int(1)).push(Val::Int(2));
        let t = PStack::<Val>::new().push(Val::Int(1)).push(Val::Int(2));
        assert_eq!(s.version(), t.version());
        assert_eq!(s.pop().map(|(v, _)| v.int()), Some(2));
    }
    // A field read sees the field's version, not the whole value's.
    let a = Val::tup(vec![Val::Int(1), Val::Int(5), Val::str("hello")]);
    let b = Val::tup(vec![Val::Int(1), Val::Int(5), Val::str("world")]);
    assert_ne!(a.version(), b.version());
    assert_eq!(
        a.field(1).map(|x| x.version()),
        b.field(1).map(|x| x.version())
    );
}

/// A box's text changes and its width does not: `wd` hits.
#[test]
fn field_reads_cut_off() {
    let mut lines = vec![
        "setbox b hello".to_string(),
        String::new(),
        "wd b".to_string(),
    ];
    let mut rt = Runtime::<Stub>::new(Config::default());
    let _ = rt.build(&Stub, Func::Main, &[source(&lines)]);
    lines[0] = "setbox b world".to_string();
    let o = rt.build(&Stub, Func::Main, &[source(&lines)]);
    let ran = reran(&o);
    assert_eq!(
        ran.iter().filter(|f| **f == Func::Para).count(),
        1,
        "{ran:?}"
    );
}

/// biber: two passes, biber, one pass.
#[test]
fn biber_chain() {
    let lines = vec![
        "cite k2 cite k1".to_string(),
        String::new(),
        "printbib".to_string(),
    ];
    let mut rt = Runtime::<Stub>::new(Config::default());
    let (o, _) = both(&mut rt, &Streams::new(), &lines, 0);
    assert_eq!(o.trips.len(), 3);
    assert!(out_text(&o).contains("[k1] [k2]"), "{}", out_text(&o));
    // A rebuild with nothing changed: one trip, all hits.
    let o2 = rt.build(&Stub, Func::Main, &[source(&lines)]);
    assert_eq!(o2.trips.len(), 1);
    assert_eq!(o2.trips[0].reran, vec![]);
}
