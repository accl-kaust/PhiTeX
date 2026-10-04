//! A session's runs against makeindex's (`partex_makeindex::run`): the
//! same `.ind`, `.ilg` and terminal after every edit of the `.idx`.

use std::collections::HashMap;
use std::fmt::Write;

use partex_makeindex::{Files, Session};

#[derive(Clone, Default)]
struct Mem(HashMap<Vec<u8>, Vec<u8>>);

impl Files for Mem {
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.0.get(name).cloned()
    }
    fn readable(&mut self, name: &[u8]) -> bool {
        self.0.contains_key(name)
    }
    fn style(&mut self, name: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        self.0.get(name).map(|d| (name.to_vec(), Some(d.clone())))
    }
    fn out_name_ok(&mut self, _name: &[u8]) -> Result<(), Vec<u8>> {
        Ok(())
    }
    fn stdin(&mut self) -> Vec<u8> {
        Vec::new()
    }
}

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

/// Index entries' arguments to draw from: plain, sub-entries, sort keys,
/// encapsulators, ranges, cross-references, symbols and numbers.
const KEYS: &[&str] = &[
    "alpha",
    "beta",
    "beta!sub",
    "beta!sub!subsub",
    "gamma|textbf",
    "delta@$\\delta$",
    "\"!bang",
    "range|(",
    "range|)",
    "seeing|see{alpha}",
    "Zeta",
    "zeta!Sub",
    "10",
    "2",
    "epsilon|textit",
    "eta",
    "theta|(",
    "theta|)",
    "a long entry with many words in it to wrap the line",
];

fn idx(entries: &[(usize, u32)]) -> Vec<u8> {
    let mut s = String::new();
    for &(k, page) in entries {
        s.push_str("\\indexentry{");
        s.push_str(KEYS[k]);
        s.push_str("}{");
        s.push_str(&page.to_string());
        s.push_str("}\n");
    }
    s.into_bytes()
}

fn check(seed: u64, steps: usize, style: Option<&str>) {
    let version = b"version 2.18 [TeX Live 2026] (kpathsea + Thai support)";
    let mut rng = Lcg(seed);
    let mut entries: Vec<(usize, u32)> = (0..12)
        .map(|i| (rng.below(KEYS.len()), u32::try_from(i / 3 + 1).unwrap_or(1)))
        .collect();
    let mut files = Mem::default();
    let mut args = Vec::new();
    if let Some(st) = style {
        files.0.insert(b"job.ist".to_vec(), st.as_bytes().to_vec());
        args.push(b"-s".to_vec());
        args.push(b"job.ist".to_vec());
    }
    args.push(b"job.idx".to_vec());
    let mut session = Session::new();
    let mut reused = 0;
    for step in 0..steps {
        match rng.below(4) {
            0 => {
                let at = rng.below(entries.len() + 1);
                let page = u32::try_from(rng.below(6) + 1).unwrap_or(1);
                entries.insert(at, (rng.below(KEYS.len()), page));
            }
            1 if entries.len() > 1 => {
                let at = rng.below(entries.len());
                entries.remove(at);
            }
            2 => {
                let at = rng.below(entries.len());
                entries[at].1 = u32::try_from(rng.below(6) + 1).unwrap_or(1);
            }
            _ => {
                // (a page moved by everything after it: a page break)
                let at = rng.below(entries.len());
                for e in &mut entries[at..] {
                    e.1 += 1;
                }
            }
        }
        files.0.insert(b"job.idx".to_vec(), idx(&entries));
        let want = partex_makeindex::run(&args, version, &mut files.clone());
        let (got, stats) = session.run(&args, version, &mut files);
        let ctx = format!("seed {seed} step {step}: {entries:?} {stats:?}");
        assert_eq!(
            String::from_utf8_lossy(
                &got.ind
                    .as_ref()
                    .map(|f| f.contents.clone())
                    .unwrap_or_default()
            ),
            String::from_utf8_lossy(
                &want
                    .ind
                    .as_ref()
                    .map(|f| f.contents.clone())
                    .unwrap_or_default()
            ),
            "ind: {ctx}"
        );
        assert_eq!(
            String::from_utf8_lossy(
                &got.ilg
                    .as_ref()
                    .map(|f| f.contents.clone())
                    .unwrap_or_default()
            ),
            String::from_utf8_lossy(
                &want
                    .ilg
                    .as_ref()
                    .map(|f| f.contents.clone())
                    .unwrap_or_default()
            ),
            "ilg: {ctx}"
        );
        assert_eq!(got.stderr, want.stderr, "stderr: {ctx}");
        assert_eq!(got.status, want.status, "{ctx}");
        if step > 0 && stats.run < stats.blocks {
            reused += 1;
        }
    }
    assert!(reused > 0, "no run reused a block");
}

#[test]
fn default_style() {
    for seed in 1..=8 {
        check(seed, 40, None);
    }
}

#[test]
fn headings_style() {
    let st = "headings_flag 1\nheading_prefix \"{\\\\bfseries \"\nheading_suffix \"}\\\\nopagebreak\\n\"\nline_max 40\n";
    for seed in 10..=13 {
        check(seed, 40, Some(st));
    }
}

/// What a session's runs cost for single edits (printed: `--nocapture`).
#[test]
fn costs() {
    let version = b"version 2.18";
    let words: Vec<String> = (0..300).map(|i| format!("word{i:03}")).collect();
    let make = |extra: Option<&str>, moved: bool| -> Vec<u8> {
        let mut s = String::new();
        for (i, w) in words.iter().enumerate() {
            let page = i / 10 + 1 + usize::from(moved && i == 150);
            let _ = writeln!(s, "\\indexentry{{{w}}}{{{page}}}");
            if i == 100
                && let Some(x) = extra
            {
                let _ = writeln!(s, "\\indexentry{{{x}}}{{11}}");
            }
        }
        s.into_bytes()
    };
    let mut files = Mem::default();
    let args = vec![b"job.idx".to_vec()];
    let mut session = Session::new();
    let mut step = |what: &str, data: Vec<u8>| {
        files.0.insert(b"job.idx".to_vec(), data);
        let want = partex_makeindex::run(&args, version, &mut files.clone());
        let (got, stats) = session.run(&args, version, &mut files);
        assert_eq!(got.ind, want.ind, "{what}");
        assert_eq!(got.ilg, want.ilg, "{what}");
        eprintln!("{what}: {stats:?}");
        stats
    };
    step("cold", make(None, false));
    let s = step("nothing", make(None, false));
    assert_eq!(s.run, 0);
    let s = step("an entry added", make(Some("word0995"), false));
    assert!(s.run <= 3, "{s:?}");
    let s = step("an entry's page", make(Some("word0995"), true));
    assert!(s.run <= 2, "{s:?}");
}
