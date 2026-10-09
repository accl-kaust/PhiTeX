//! Dev tool: the memory a watch's diff keeps (a [`Baseline`] and a
//! [`Live`] over it) on a large document, and the time of a one-word edit.
//!
//!     scripts/sandbox cargo run -q --release -p phitex-diff --example liverss -- [FILE.tex | MB]
//!
//! The old version is `FILE.tex`, else a synthetic article of about `MB`
//! megabytes (default 5: sections of prose, inline and display math,
//! lists, figures); the new one is it with a word changed in one
//! paragraph in a hundred. The resident set is read from
//! `/proc/self/status` before and after each step.

use phitex_diff::{Baseline, Flat, Live, Options, flatten};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Instant;

/// The resident set now, in MB.
fn rss() -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % (n.max(1) as u64)).unwrap_or(0)
    }
}

const WORDS: [&str; 24] = [
    "the", "of", "a", "model", "which", "we", "show", "is", "bounded", "by", "every", "result",
    "graph", "edge", "in", "that", "proof", "follows", "from", "lemma", "and", "its", "case",
    "for",
];

/// A synthetic article of about `mb` megabytes.
fn article(mb: usize) -> String {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let mut s = String::from(
        "\\documentclass{article}\n\\usepackage{amsmath}\n\\usepackage{graphicx}\n\\begin{document}\n",
    );
    let mut k = 0;
    while s.len() < mb << 20 {
        k += 1;
        if k % 12 == 1 {
            let _ = write!(s, "\\section{{Part {k}}}\\label{{sec:{k}}}\n\n");
        }
        let n = 40 + r.next(80);
        for i in 0..n {
            s.push_str(WORDS[r.next(WORDS.len())]);
            s.push(if i % 15 == 14 { '\n' } else { ' ' });
            if r.next(40) == 0 {
                s.push_str("$x_{i} + y^2 \\le \\alpha$ ");
            }
            if r.next(90) == 0 {
                s.push_str("\\cite{ref} ");
            }
        }
        s.push_str(".\n\n");
        match r.next(14) {
            0 => s.push_str("\\begin{equation}\n  \\sum_{i=1}^{n} a_i = \\int_0^1 f(x)\\,dx\n\\end{equation}\n\n"),
            1 => s.push_str("\\begin{itemize}\n  \\item first point\n  \\item second point\n\\end{itemize}\n\n"),
            2 => drop(write!(
                s,
                "\\begin{{figure}}\n  \\centering\n  \\includegraphics[width=.5\\textwidth]{{fig{k}}}\n  \\caption{{A figure.}}\n\\end{{figure}}\n\n"
            )),
            _ => {}
        }
    }
    s.push_str("\\end{document}\n");
    s
}

/// `old` with a word changed in one paragraph in a hundred.
fn edited(old: &str) -> String {
    let mut out = String::with_capacity(old.len());
    for (i, p) in old.split("\n\n").enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        if i % 100 == 50 && p.contains(" model ") {
            out.push_str(&p.replacen(" model ", " network ", 1));
        } else {
            out.push_str(p);
        }
    }
    out
}

fn main() {
    let arg = std::env::args().nth(1);
    let old = match arg.as_deref() {
        Some(f)
            if std::path::Path::new(f)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("tex")) =>
        {
            std::fs::read_to_string(f).expect("read the file")
        }
        Some(mb) => article(mb.parse().expect("a size in MB")),
        None => article(5),
    };
    let new = edited(&old);
    let files = |t: &str| BTreeMap::from([("main.tex".to_owned(), t.to_owned())]);
    let (oldf, newf) = (files(&old), files(&new));
    let flat_new: Flat = flatten(&newf, "main.tex").expect("flatten");
    #[allow(clippy::cast_precision_loss, reason = "a size shown")]
    let mb = old.len() as f64 / 1_048_576.0;
    println!("document: {mb:.1} MB");
    let r0 = rss();
    let t = Instant::now();
    let base = Baseline::new(&oldf, "main.tex", &Options::default()).expect("baseline");
    let r1 = rss();
    println!(
        "Baseline: {:.0} ms, RSS +{:.1} MB",
        t.elapsed().as_secs_f64() * 1e3,
        r1 - r0
    );
    let mut live = Live::new(base);
    let t = Instant::now();
    let n = live.update(&flat_new).changes.len();
    let r2 = rss();
    println!(
        "Live, first diff: {:.0} ms, {n} changes, RSS +{:.1} MB (Baseline + Live: +{:.1} MB)",
        t.elapsed().as_secs_f64() * 1e3,
        r2 - r1,
        r2 - r0
    );
    // (a one-word edit in the middle)
    let mid = new.len() / 2;
    let at = new[mid..].find(" proof ").map_or(mid, |k| mid + k);
    let again = format!("{} proofs {}", &new[..at], &new[at + 7..]);
    let flat_again = flatten(&files(&again), "main.tex").expect("flatten");
    let t = Instant::now();
    let n = live.update(&flat_again).changes.len();
    let s = live.stats();
    println!(
        "one-word edit: {:.2} ms, {n} changes, {s:?}, RSS {:.1} MB",
        t.elapsed().as_secs_f64() * 1e3,
        rss()
    );
}
