//! Dev tool: random old/new pairs from the corpus, each diffed.
//!
//!     scripts/sandbox cargo run -q -p phitex-diff --example fuzz -- CORPUS OUT N [SEED]
//!
//! For each pair `OUT/<k>/`: `old/main.tex` and `new/main.tex`, two random
//! edits (lines dropped, duplicated, swapped or moved; words replaced or
//! dropped) of a corpus document, and `diff.tex`, their diff.
//! `tests/fuzz.sh` compiles them.

use phitex_diff::{Options, diff};
use std::collections::BTreeMap;
use std::path::Path;

struct Rng(u64);

impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % (n.max(1) as u64)).unwrap_or(0)
    }
}

/// A random edit of `src`'s body.
#[allow(clippy::many_single_char_names)]
fn mutate(src: &str, rng: &mut Rng) -> String {
    let (Some(b), Some(e)) = (src.find("\\begin{document}"), src.rfind("\\end{document}")) else {
        return src.to_owned();
    };
    let b = b + "\\begin{document}\n".len();
    let mut lines: Vec<String> = src[b..e].lines().map(str::to_owned).collect();
    let words: Vec<String> = src[b..e]
        .split_whitespace()
        .filter(|w| w.chars().all(char::is_alphabetic))
        .map(str::to_owned)
        .collect();
    for _ in 0..=rng.next(4) {
        if lines.is_empty() {
            break;
        }
        let i = rng.next(lines.len());
        let structural = |l: &str| l.contains("\\begin") || l.contains("\\end");
        match rng.next(6) {
            0 if !structural(&lines[i]) => {
                lines.remove(i);
            }
            1 if !structural(&lines[i]) => {
                let l = lines[i].clone();
                lines.insert(i, l);
            }
            2 if i + 1 < lines.len() && !structural(&lines[i]) && !structural(&lines[i + 1]) => {
                lines.swap(i, i + 1);
            }
            3 if !words.is_empty() => {
                let mut ws: Vec<String> = lines[i].split(' ').map(str::to_owned).collect();
                let k = rng.next(ws.len());
                if ws[k].chars().all(char::is_alphabetic) && !ws[k].is_empty() {
                    ws[k].clone_from(&words[rng.next(words.len())]);
                }
                lines[i] = ws.join(" ");
            }
            4 => {
                let mut ws: Vec<String> = lines[i].split(' ').map(str::to_owned).collect();
                let k = rng.next(ws.len());
                if ws[k].chars().all(char::is_alphabetic) {
                    ws.remove(k);
                }
                lines[i] = ws.join(" ");
            }
            5 if !structural(&lines[i]) => {
                let j = rng.next(lines.len());
                let l = lines.remove(i);
                lines.insert(j.min(lines.len()), l);
            }
            _ => {}
        }
    }
    let mut out = src[..b].to_owned();
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out.push_str(&src[e..]);
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: fuzz CORPUS OUT N [SEED]");
        std::process::exit(2);
    }
    let corpus = Path::new(&args[0]);
    let out = Path::new(&args[1]);
    let n: usize = args[2].parse().unwrap_or(10);
    let mut rng = Rng(args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x5eed_1234_abcd));
    let mut bases = Vec::new();
    let mut cases: Vec<_> = std::fs::read_dir(corpus)
        .expect("corpus")
        .flatten()
        .map(|e| e.path())
        .collect();
    cases.sort();
    for c in cases {
        // (single-file documents)
        if let Ok(t) = std::fs::read_to_string(c.join("new/main.tex"))
            && !t.contains("\\input")
        {
            bases.push(t);
        }
    }
    for k in 0..n {
        let base = &bases[rng.next(bases.len())];
        let old = mutate(base, &mut rng);
        let new = mutate(base, &mut rng);
        let dir = out.join(k.to_string());
        for (side, text) in [("old", &old), ("new", &new)] {
            std::fs::create_dir_all(dir.join(side)).expect("mkdir");
            std::fs::write(dir.join(side).join("main.tex"), text).expect("write");
        }
        let mut o = BTreeMap::new();
        o.insert("main.tex".to_owned(), old);
        let mut nw = BTreeMap::new();
        nw.insert("main.tex".to_owned(), new);
        let d = diff(&o, &nw, "main.tex", &Options::default()).expect("diff");
        std::fs::write(dir.join("diff.tex"), d.tex).expect("write");
    }
}
