#![allow(clippy::pedantic)]

//! Random edit sequences: the incremental graph equals a fresh build.

mod toy;

use toy::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const SNIPPETS: &[&str] = &[
    "w",
    "ab",
    "cde",
    "longerword",
    "\\par",
    "\\def\\a{x y}",
    "\\def\\b{\\step z}",
    "\\gdef\\a{q}",
    "\\a",
    "\\b",
    "\\x",
    "\\step",
    "\\the\\count",
    "{",
    "}",
    "{ \\def\\a{inner} \\a }",
    "\\def\\x{0}",
    "\\def\\x{\\step}",
    "\\ifzero\\x yes \\def\\y{t} \\else no \\def\\y{f} \\fi",
    "\\ifzero\\count zero \\else nonzero \\step \\fi",
    "\\the\\y",
    "\\defname{dyn}{d1 d2}",
    "\\use{dyn}",
    "\\write{o}",
    "\\message{m}",
    "\\label{k}",
    "\\ref{k}",
    "\\label{j}",
    "\\ref{j}",
    "\\hbox{h1 h2 \\the\\count}",
    "\\barrier",
    "\\section{s1}",
    "\\section{s2}",
    "\\toc",
    "\\addto{h}{p}",
    "\\addto{h}{q}",
    "\\usehook{h}",
    "\\twice",
    "\\scoped",
    "{ \\def\\x{7} \\scoped }",
];

fn snippet(r: &mut Rng) -> Vec<Tok> {
    lex(SNIPPETS[r.below(SNIPPETS.len())])
}

fn random_doc(r: &mut Rng, n: usize) -> Vec<Tok> {
    let mut out = Vec::new();
    while out.len() < n {
        out.extend(snippet(r));
    }
    out
}

fn fresh(d: &Doc) -> Doc {
    let mut f = Doc::new(d.toks.clone());
    f.g.cfg.check = true;
    f.g.run();
    // parallel (speculative entry at every paragraph) equals in turn
    let mut p = Doc::new(d.toks.clone());
    p.g.cfg.workers = 3;
    p.g.cfg.check = true;
    p.g.run();
    let (a, b) = (p.g.to_text(), f.g.to_text());
    if a != b {
        if let Ok(dir) = std::env::var("PHI_DUMP") {
            std::fs::write(format!("{dir}/par.txt"), &a).unwrap();
            std::fs::write(format!("{dir}/seq.txt"), &b).unwrap();
        }
        let al: Vec<&str> = a.lines().collect();
        let bl: Vec<&str> = b.lines().collect();
        let k = al.iter().zip(&bl).take_while(|(x, y)| x == y).count();
        panic!(
            "3 workers differ from line {k}:\n{}\nin turn:\n{}",
            al[k.saturating_sub(4)..(k + 6).min(al.len())].join("\n"),
            bl[k.saturating_sub(4)..(k + 6).min(bl.len())].join("\n")
        );
    }
    if p.observe() != f.observe() {
        let src: Vec<String> = d.toks.iter().map(|t| t.1.text()).collect();
        eprintln!("SRC {}", src.join(" "));
        for n in ["a", "b", "x", "y", "count"] {
            if let (Some(i), Some(j)) = (p.g.name_id(n.as_bytes()), f.g.name_id(n.as_bytes())) {
                eprintln!(
                    "{n} parallel:\n{}in turn:\n{}",
                    p.g.debug_defs(i),
                    f.g.debug_defs(j)
                );
            }
        }
        if let Ok(dir) = std::env::var("PHI_DUMP") {
            std::fs::write(format!("{dir}/par.txt"), p.g.to_text_ids()).unwrap();
            std::fs::write(format!("{dir}/seq.txt"), f.g.to_text_ids()).unwrap();
        }
    }
    assert_eq!(p.observe(), f.observe(), "3 workers");
    f
}

fn edit(r: &mut Rng, d: &mut Doc) {
    let n = d.toks.len();
    let at = r.below(n + 1);
    let del = r.below(4).min(n - at);
    if std::env::var("PHI_TRACE").is_ok() {
        eprintln!("  splice at {at} del {del}");
    }
    let mut ins = Vec::new();
    for _ in 0..r.below(3) {
        ins.extend(snippet(r));
    }
    d.splice(at, del, ins);
}

fn run_seed(seed: u64, len: usize, edits: usize) {
    run_seed_with(seed, len, edits, false);
}

fn run_seed_with(seed: u64, len: usize, edits: usize, keep: bool) {
    let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut d = Doc::new(ids(random_doc(&mut r, len)));
    d.g.cfg.check = true;
    d.g.cfg.keep_interior = keep;
    // (the same document rebuilt with parallel rounds: equal after each edit)
    let mut dp = Doc::new(d.toks.clone());
    dp.g.cfg.workers = 2;
    dp.g.cfg.round_min_ns = 0;
    dp.g.cfg.check = true;
    dp.g.cfg.keep_interior = keep;
    // (with a small memo store: hits, misses and evictions all exact)
    dp.g.set_memo_budget(4096);
    // (and profiled, tuned when idle after every run: auto memo opt-in
    // with the probe cost set so it opts in, keeps and opts out, profile
    // driven sealing; check mode on)
    let mut dt: DocP<true> = DocP::new(d.toks.clone());
    dt.g.cfg.check = true;
    dt.g.cfg.keep_interior = keep;
    dt.g.cfg.memo_probe_ns = [0, 40, 400][(seed % 3) as usize];
    dt.g.cfg.tune_min_samples = 1;
    dt.g.cfg.tune_min_keyed = 4;
    dt.g.cfg.auto_memo_bytes = 1 << 14;
    dt.g.cfg.seal = 2 + (seed % 3) as u32;
    dt.g.cfg.seal_quiet = 2;
    dt.g.cfg.hot_runs = 2;
    dt.g.run();
    dt.g.tune();
    dp.g.run();
    d.g.run();
    assert_eq!(d.observe(), fresh(&d).observe(), "seed {seed}: cold");
    for e in 0..edits {
        let k = 1 + r.below(3);
        for _ in 0..k {
            edit(&mut r, &mut d);
        }
        if std::env::var("PHI_TRACE").is_ok() {
            eprintln!(
                "edit {e}: {} edits; toks[4..8] = {:?}",
                k,
                &d.toks[4..8.min(d.toks.len())]
            );
        }
        let rep = d.g.run();
        dp.toks = d.toks.clone();
        dp.g.set(dp.input, TV::Seq(seq_of(&dp.toks)));
        let rp = dp.g.run();
        DRY.with(|c| c.set(c.get() + rp.dry_used));
        dt.toks = d.toks.clone();
        dt.g.set(dt.input, TV::Seq(seq_of(&dt.toks)));
        dt.g.run();
        let tu = dt.g.tune();
        TUNED.with(|c| {
            let mut c = c.borrow_mut();
            c.0 += tu.memo_in.len() as u64;
            c.1 += tu.memo_out.len() as u64;
            c.2 += tu.sealed;
        });
        assert_eq!(
            dt.observe(),
            d.observe(),
            "seed {seed}, edit {e}: profiled and tuned differs"
        );
        assert_eq!(
            dp.g.to_text(),
            d.g.to_text(),
            "seed {seed}, edit {e}: parallel rounds differ"
        );
        assert_eq!(
            dp.observe(),
            d.observe(),
            "seed {seed}, edit {e}: parallel rounds differ"
        );
        // the text form round-trips, values included where the client parses them
        let text = d.g.to_text();
        let dump = phi::Dump::parse(&text, <Toy as phi::Lang>::parse_val).expect("parses");
        assert_eq!(dump.to_text(), text);
        for l in &dump.lines {
            if let phi::Line::Node {
                val,
                parsed: Some(v),
                ..
            } = l
            {
                assert_eq!(&<Toy as phi::Lang>::fmt_val(v), val);
            }
        }
        if std::env::var("PHI_DUMP").is_ok() {
            eprintln!(
                "edit {e}: {rep:?} slot j = {:?}",
                d.g.slot(phi::Slot(phi::ver::hash64("j")))
            );
        }
        let f = fresh(&d);
        let src: Vec<String> = d.toks.iter().map(|t| t.1.text()).collect();
        if d.observe() != f.observe() || (!keep && d.g.to_text() != f.g.to_text()) {
            let (a, b) = (d.g.to_text(), f.g.to_text());
            if let Ok(dir) = std::env::var("PHI_DUMP") {
                std::fs::write(format!("{dir}/inc.txt"), &a).unwrap();
                std::fs::write(format!("{dir}/fresh.txt"), &b).unwrap();
            }
            let al: Vec<&str> = a.lines().collect();
            let bl: Vec<&str> = b.lines().collect();
            let k = al.iter().zip(&bl).take_while(|(x, y)| x == y).count();
            panic!(
                "seed {seed}, edit {e}: {}\nincremental from line {k}:\n{}\nfresh:\n{}\n{}\n{}",
                src.join(" "),
                al[k.saturating_sub(3)..(k + 8).min(al.len())].join("\n"),
                bl[k.saturating_sub(3)..(k + 8).min(bl.len())].join("\n"),
                d.observe(),
                f.observe()
            );
        }
    }
}

thread_local! {
    static DRY: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// What `tune` did: memo opt-ins, opt-outs, steps sealed.
    static TUNED: std::cell::RefCell<(u64, u64, u64)> = const { std::cell::RefCell::new((0, 0, 0)) };
}

#[test]
fn random_edits_equal_fresh_builds() {
    let only: Option<u64> = std::env::var("PHI_SEED").ok().and_then(|s| s.parse().ok());
    for seed in 1..=40 {
        if only.is_none_or(|o| o == seed) {
            run_seed(seed, 60, 60);
        }
    }
    if only.is_none() {
        let used = DRY.with(|c| c.get());
        eprintln!("dry outcomes used: {used}");
        assert!(used > 0, "no parallel round's outcome was used");
        let t = TUNED.with(|c| *c.borrow());
        eprintln!(
            "tune: {} memo opt-ins, {} opt-outs, {} steps sealed",
            t.0, t.1, t.2
        );
        assert!(t.0 > 0 && t.1 > 0 && t.2 > 0, "tune did not act: {t:?}");
    }
}

/// With sealed regions (DESIGN 7.11): runs of steps folded after every
/// run, woken folds run again. What the document makes equals a fresh
/// build's, unsealed and checked.
fn run_seed_sealed(seed: u64, len: usize, edits: usize, f: u32) {
    let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut d = Doc::new(ids(random_doc(&mut r, len)));
    d.g.cfg.seal = f;
    let mut sealed = d.g.run().sealed;
    // (compacted after every run, whether sparse or not)
    d.g.compact();
    assert_eq!(
        d.observe(),
        fresh(&d).observe(),
        "seed {seed}: cold, sealed"
    );
    for e in 0..edits {
        let k = 1 + r.below(3);
        for _ in 0..k {
            edit(&mut r, &mut d);
        }
        if std::env::var("PHI_TRACE").is_ok() {
            let src: Vec<String> = d.toks.iter().map(|t| t.1.text()).collect();
            eprintln!("edit {e}: {}", src.join(" "));
        }
        let rep = d.g.run();
        sealed += rep.sealed;
        d.g.compact();
        let text = d.g.to_text();
        let dump = phi::Dump::parse(&text, <Toy as phi::Lang>::parse_val).expect("parses");
        assert_eq!(dump.to_text(), text);
        let fr = fresh(&d);
        if d.observe() != fr.observe() {
            let src: Vec<String> = d.toks.iter().map(|t| t.1.text()).collect();
            if let Ok(dir) = std::env::var("PHI_DUMP") {
                std::fs::write(format!("{dir}/inc.txt"), d.g.to_text_ids()).unwrap();
                std::fs::write(format!("{dir}/fresh.txt"), fr.g.to_text_ids()).unwrap();
            }
            panic!(
                "seed {seed}, edit {e}, sealed every {f}: {}\nincremental:\n{}fresh:\n{}",
                src.join(" "),
                d.observe(),
                fr.observe()
            );
        }
    }
    if std::env::var("PHI_TRACE").is_ok() {
        eprintln!("seed {seed}: {sealed} steps sealed");
    }
    assert!(sealed > 0, "seed {seed}: nothing sealed");
}

#[test]
fn sealed_random_edits_equal_fresh_builds() {
    let only: Option<u64> = std::env::var("PHI_SEED").ok().and_then(|s| s.parse().ok());
    for seed in 1..=40 {
        if only.is_none_or(|o| o == seed) {
            run_seed_sealed(seed, 60, 40, 1 + (seed % 4) as u32);
        }
    }
}

/// Interiors kept (every emission a node) with check mode: each step runs
/// again dry after every run and every leaf of it is compared.
#[test]
fn kept_interiors_check_every_leaf() {
    for seed in 1..=12 {
        run_seed_with(seed, 60, 30, true);
    }
}
