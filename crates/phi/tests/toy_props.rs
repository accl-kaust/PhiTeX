//! Random edit sequences: the incremental graph equals a fresh build.

mod toy;

use phi::Lang as _;

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
                eprintln!("{n} parallel:\n{}in turn:\n{}", p.g.debug_defs(i), f.g.debug_defs(j));
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
    let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut d = Doc::new(ids(random_doc(&mut r, len)));
    d.g.cfg.check = true;
    d.g.run();
    assert_eq!(d.observe(), fresh(&d).observe(), "seed {seed}: cold");
    for e in 0..edits {
        let k = 1 + r.below(3);
        for _ in 0..k {
            edit(&mut r, &mut d);
        }
        if std::env::var("PHI_TRACE").is_ok() {
            eprintln!("edit {e}: {} edits; toks[4..8] = {:?}", k, &d.toks[4..8.min(d.toks.len())]);
        }
        let rep = d.g.run();
        // the text form round-trips, values included where the client parses them
        let text = d.g.to_text();
        let dump = phi::Dump::parse(&text, <Toy as phi::Lang>::parse_val).expect("parses");
        assert_eq!(dump.to_text(), text);
        for l in &dump.lines {
            if let phi::Line::Node { val, parsed: Some(v), .. } = l {
                assert_eq!(&<Toy as phi::Lang>::fmt_val(v), val);
            }
        }
        if std::env::var("PHI_DUMP").is_ok() {
            eprintln!("edit {e}: {rep:?} slot j = {:?}", d.g.slot(phi::Slot(phi::ver::hash64("j"))));
        }
        let f = fresh(&d);
        let src: Vec<String> = d.toks.iter().map(|t| t.1.text()).collect();
        if d.observe() != f.observe() || d.g.to_text() != f.g.to_text() {
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

#[test]
fn random_edits_equal_fresh_builds() {
    let only: Option<u64> = std::env::var("PHI_SEED").ok().and_then(|s| s.parse().ok());
    for seed in 1..=40 {
        if only.is_none_or(|o| o == seed) {
            run_seed(seed, 60, 60);
        }
    }
}
