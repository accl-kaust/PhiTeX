//! A session's runs against bibtex.web's (`partex_bibtex::run`): the same
//! `.bbl`, `.blg` and terminal after every edit, with TeX Live's styles
//! and `xampl.bib` (found by `kpsewhich`; skipped without them).

use std::collections::HashMap;
use std::fmt::Write;

use partex_bibtex::{Files, Options, Session};

#[derive(Clone, Default)]
struct Mem(HashMap<Vec<u8>, Vec<u8>>);

impl Files for Mem {
    fn aux(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.0.get(name).cloned()
    }
    fn bst(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let mut n = name.to_vec();
        n.extend_from_slice(b".bst");
        self.0.get(&n).cloned()
    }
    fn bib(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let mut n = name.to_vec();
        if !n.ends_with(b".bib") {
            n.extend_from_slice(b".bib");
        }
        self.0.get(&n).cloned()
    }
}

fn texlive(name: &str) -> Option<Vec<u8>> {
    let out = std::process::Command::new("kpsewhich")
        .arg(name)
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    std::fs::read(path.trim()).ok()
}

/// A small deterministic generator.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap_or(0)
    }
}

fn aux(style: &str, cites: &[&str]) -> Vec<u8> {
    let mut a = String::from("\\relax\n");
    for c in cites {
        let _ = writeln!(a, "\\citation{{{c}}}");
    }
    let _ = write!(a, "\\bibstyle{{{style}}}\n\\bibdata{{xampl}}\n");
    a.into_bytes()
}

const KEYS: &[&str] = &[
    "article-minimal",
    "article-full",
    "article-crossref",
    "whole-journal",
    "inbook-minimal",
    "inbook-full",
    "inbook-crossref",
    "book-minimal",
    "book-full",
    "book-crossref",
    "whole-set",
    "booklet-full",
    "incollection-full",
    "incollection-crossref",
    "whole-collection",
    "manual-full",
    "mastersthesis-full",
    "misc-full",
    "inproceedings-full",
    "inproceedings-crossref",
    "proceedings-full",
    "phdthesis-full",
    "techreport-full",
    "unpublished-full",
    "random-note-crossref",
];

/// Field edits that keep the database well formed: (from, to).
const BIB_EDITS: &[(&str, &str)] = &[
    ("Donald E. Knuth", "Donald Ervin Knuth"),
    ("1986", "1987"),
    (
        "The Gnats and Gnus Document Preparation System",
        "Gnats and Gnus",
    ),
    ("L[eslie] A. Aamport", "Leslie A. Aamport"),
    ("Jill C. Knvth", "Jill C. Knuth"),
    ("Sorting and Searching", "Searching and Sorting"),
    // (a crossref's parent changed, a field taken away, a string, the
    // preamble, an entry's type, an entry gone, a key's case)
    (
        "crossref = \"whole-set\"",
        "crossref = \"whole-collection\"",
    ),
    ("   volume = 1,\n", ""),
    (
        "\"The OX Association for Computing Machinery\"",
        "\"The Association\"",
    ),
    (
        "\\newcommand{\\noopsort}[1]{} ",
        "\\newcommand{\\noopsort}[1]{}",
    ),
    ("@BOOK{book-minimal,", "@MANUAL{book-minimal,"),
    ("@BOOKLET{booklet-minimal,", "@BOOKLET{booklet-gone,"),
    ("@MISC{misc-minimal,", "@MISC{Misc-Minimal,"),
];

fn check(style: &str, seed: u64, steps: usize) {
    let (Some(bst), Some(bib)) = (texlive(&format!("{style}.bst")), texlive("xampl.bib")) else {
        eprintln!("skipped: no {style}.bst or xampl.bib");
        return;
    };
    let opts = Options::default();
    let mut files = Mem::default();
    files.0.insert(format!("{style}.bst").into_bytes(), bst);
    files.0.insert(b"xampl.bib".to_vec(), bib.clone());
    let mut rng = Lcg(seed);
    let mut cites: Vec<&str> = KEYS[..8].to_vec();
    let mut bib = bib;
    let mut session = Session::new();
    let mut runs_small = 0;
    for step in 0..steps {
        match rng.below(6) {
            0 => {
                let k = KEYS[rng.below(KEYS.len())];
                let at = rng.below(cites.len() + 1);
                cites.insert(at, k);
            }
            1 if cites.len() > 1 => {
                let at = rng.below(cites.len());
                cites.remove(at);
            }
            2 => {
                let (a, b) = BIB_EDITS[rng.below(BIB_EDITS.len())];
                let text = String::from_utf8_lossy(&bib).into_owned();
                let text = if text.contains(a) {
                    text.replacen(a, b, 1)
                } else {
                    text.replacen(b, a, 1)
                };
                bib = text.into_bytes();
            }
            3 if cites.len() > 2 => {
                let (i, j) = (rng.below(cites.len()), rng.below(cites.len()));
                cites.swap(i, j);
            }
            4 if rng.below(3) == 0 => cites.push("*"),
            4 => cites.push("booklet-minimal"),
            _ => cites.retain(|c| *c != "*"),
        }
        files.0.insert(b"xampl.bib".to_vec(), bib.clone());
        files.0.insert(b"job.aux".to_vec(), aux(style, &cites));
        let want = partex_bibtex::run(b"job", &opts, &mut files.clone());
        let (got, stats) = session.run(b"job", &opts, &mut files);
        let ctx = format!("{style} seed {seed} step {step}: {cites:?} {stats:?}");
        assert_eq!(
            String::from_utf8_lossy(&got.bbl.as_ref().unwrap().contents),
            String::from_utf8_lossy(&want.bbl.as_ref().unwrap().contents),
            "bbl: {ctx}"
        );
        assert_eq!(
            String::from_utf8_lossy(&got.blg.as_ref().unwrap().contents),
            String::from_utf8_lossy(&want.blg.as_ref().unwrap().contents),
            "blg: {ctx}"
        );
        assert_eq!(
            String::from_utf8_lossy(&got.term),
            String::from_utf8_lossy(&want.term),
            "term: {ctx}"
        );
        assert_eq!(
            (got.history, got.status),
            (want.history, want.status),
            "{ctx}"
        );
        if step > 0 && !stats.fresh && stats.run < stats.calls {
            runs_small += 1;
        }
    }
    assert!(runs_small > 0, "{style}: no run reused a call");
}

#[test]
fn plain() {
    for seed in 1..=4 {
        check("plain", seed, 50);
    }
}

#[test]
fn alpha() {
    for seed in 10..=15 {
        check("alpha", seed, 50);
    }
}

#[test]
fn abbrv() {
    check("abbrv", 3, 40);
}

#[test]
fn unsrt() {
    for seed in 20..=22 {
        check("unsrt", seed, 40);
    }
}

#[test]
fn ieeetran() {
    for seed in 30..=32 {
        check("IEEEtran", seed, 30);
    }
}

/// What a session's runs cost for single edits (printed: `--nocapture`).
#[test]
fn costs() {
    let (Some(bst), Some(bib)) = (texlive("alpha.bst"), texlive("xampl.bib")) else {
        return;
    };
    let opts = Options::default();
    let mut files = Mem::default();
    files.0.insert(b"alpha.bst".to_vec(), bst);
    files.0.insert(b"xampl.bib".to_vec(), bib.clone());
    let cites: Vec<&str> = KEYS.to_vec();
    let mut session = Session::new();
    let mut step = |what: &str, cites: &[&str], bib: &[u8]| {
        files.0.insert(b"xampl.bib".to_vec(), bib.to_vec());
        files.0.insert(b"job.aux".to_vec(), aux("alpha", cites));
        let want = partex_bibtex::run(b"job", &opts, &mut files.clone());
        let (got, stats) = session.run(b"job", &opts, &mut files);
        assert_eq!(got.bbl, want.bbl, "{what}");
        assert_eq!(got.blg, want.blg, "{what}");
        eprintln!("{what}: {stats:?}");
        stats
    };
    step("cold", &cites, &bib);
    let s = step("nothing", &cites, &bib);
    assert_eq!(s.run, 0);
    let text = String::from_utf8_lossy(&bib).into_owned();
    let edited = text.replacen(
        "This is a full MISC entry",
        "This is a fuller MISC entry",
        1,
    );
    let s = step("a note", &cites, edited.as_bytes());
    assert!(s.run <= 2, "{s:?}");
    let mut more = cites.clone();
    more.push("booklet-minimal");
    step("a citation added", &more, edited.as_bytes());
    step("and taken away", &cites, edited.as_bytes());
}
