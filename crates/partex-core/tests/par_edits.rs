//! A rebuild's steps on workers (DESIGN 3.10, `ssa/par.rs`) against the
//! same rebuilds in turn: random edits of a small document, each rebuilt
//! by a build with one worker and by builds with several (every dirty
//! step sent to them), whose steps' effects and programs must be the
//! same after each edit, whatever the workers' timing.
//!
//! In a process of its own, as `view.rs`: an SSA build sets switches the
//! whole process shares.
#![cfg(feature = "std")]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use partex_core::diag::Diagnostic;
use partex_core::host::{DateTime, FileKind, Host, OpenedFile, WriteId};
use partex_core::ssa::{Recorder, SsaTracker, rebuild, run, step_effects_as, view};
use partex_core::{Params, Tex};

/// Files in memory.
#[derive(Default)]
struct Disk {
    files: BTreeMap<Vec<u8>, Vec<u8>>,
    next: u32,
}

impl Host for Disk {
    fn read_file(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile> {
        let mut with = name.to_vec();
        with.extend_from_slice(match kind {
            FileKind::Tex => b".tex",
            FileKind::Tfm => b".tfm",
            _ => b"",
        });
        [with, name.to_vec()].into_iter().find_map(|n| {
            let c = self.files.get(&n)?;
            Some(OpenedFile {
                name: [b"./", &n[..]].concat(),
                contents: Arc::from(&c[..]),
            })
        })
    }
    fn open_write(&mut self, name: &[u8], _: FileKind) -> Option<(WriteId, Vec<u8>)> {
        self.next += 1;
        Some((WriteId(self.next), name.to_vec()))
    }
    fn write(&mut self, _: WriteId, _: &[u8]) {}
    fn close(&mut self, _: WriteId) {}
    fn term_write(&mut self, _: &[u8]) {}
    fn term_read_line(&mut self) -> Option<Vec<u8>> {
        None
    }
    fn diagnostic(&mut self, _: &Diagnostic) {}
    fn now(&self) -> DateTime {
        DateTime {
            year: 1776,
            month: 7,
            day: 4,
            minutes: 720,
        }
    }
}

/// The document's paragraphs: each reads the counter, the macro and the
/// one before's font, and some change them, so an edit's changes reach
/// the steps after it.
fn doc(paras: &[String], x: &str) -> String {
    let mut s = String::from(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n\
         \\font\\rm=cmr10 \\rm \\hsize=150pt \\vsize=120pt \\parfillskip=0pt plus 1fil\n\
         \\baselineskip=12pt \\tolerance=10000 \\output={\\shipout\\box255}\n\
         \\def\\greet#1{Hello #1.}\n\\count1=5\n",
    );
    s.push_str("\\def\\x{");
    s.push_str(x);
    s.push_str("}\n\n");
    for p in paras {
        s.push_str(p);
        s.push_str("\n\n");
    }
    s.push_str("\\end\n");
    s
}

/// A small deterministic generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap_or(1)).unwrap_or(0)
    }
}

const WORDS: &[&str] = &[
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
];

fn para(r: &mut Rng, i: usize) -> String {
    let mut p = String::new();
    match r.below(4) {
        0 => p.push_str("\\advance\\count1 by 1 "),
        1 => p.push_str("{\\count1=99 inner \\the\\count1} "),
        2 => p.push_str("\\greet{World} "),
        _ => {}
    }
    for _ in 0..(3 + r.below(20)) {
        p.push_str(WORDS[r.below(WORDS.len())]);
        p.push(' ');
    }
    let _ = write!(p, "\\x\\ {i} \\the\\count1.");
    p
}

/// `src` built in SSA mode with `workers` workers.
fn build(src: &str, workers: usize) -> Tex<Disk, SsaTracker> {
    let mut host = Disk::default();
    host.files.insert(
        b"cmr10.tfm".to_vec(),
        include_bytes!("../testdata/cmr10.tfm").to_vec(),
    );
    host.files
        .insert(b"doc.tex".to_vec(), src.as_bytes().to_vec());
    let params = Params {
        ini: true,
        interaction: Some(0),
        ..Params::default()
    };
    let tracker = SsaTracker::new(Recorder::new());
    tracker.par.workers.set(workers);
    tracker.par.thresholds.set(Some((0, 0)));
    let mut tex = Tex::new(host, tracker, params);
    let rep = run(&mut tex, b"doc", false, 0);
    assert!(rep.history <= 1, "the build failed: {}", rep.history);
    tex
}

/// What a build's steps hold: each step's effects' version by key, and
/// the program.
fn outputs(tex: &Tex<Disk, SsaTracker>) -> (Vec<(u64, u128)>, String) {
    let fx = step_effects_as(&tex.tracker().rec.borrow(), true)
        .into_iter()
        .map(|(k, e)| (k, e.0.0))
        .collect();
    (fx, view(tex).to_text())
}

#[test]
fn workers_rebuild_as_one_does() {
    let mut taken = 0;
    for seed in 1..=6u64 {
        let mut r = Rng(0x9e37_79b9_7f4a_7c15 ^ seed);
        let mut paras: Vec<String> = (0..12).map(|i| para(&mut r, i)).collect();
        let mut x = String::from("AAA");
        let src = doc(&paras, &x);
        let mut builds: Vec<Tex<Disk, SsaTracker>> =
            [1, 2, 4, 8].iter().map(|&w| build(&src, w)).collect();
        for edit in 0..8 {
            match r.below(4) {
                0 => x = WORDS[r.below(WORDS.len())].to_string(),
                1 => {
                    let i = r.below(paras.len());
                    paras[i] = para(&mut r, i);
                }
                2 if paras.len() > 4 => {
                    paras.remove(r.below(paras.len()));
                }
                _ => {
                    let i = r.below(paras.len() + 1);
                    paras.insert(i, para(&mut r, i));
                }
            }
            let src = doc(&paras, &x);
            let mut want = None;
            for (k, tex) in builds.iter_mut().enumerate() {
                tex.host_mut()
                    .files
                    .insert(b"doc.tex".to_vec(), src.as_bytes().to_vec());
                let rep = rebuild(tex, false, true);
                assert!(
                    rep.unsupported.is_none(),
                    "seed {seed} edit {edit}: {:?}",
                    rep.unsupported
                );
                let got = outputs(tex);
                taken += tex.tracker().par.stats.borrow().taken;
                *tex.tracker().par.stats.borrow_mut() = partex_core::ssa::ParStats::default();
                match &want {
                    None => want = Some(got),
                    Some(w) => {
                        assert_eq!(
                            w.0,
                            got.0,
                            "seed {seed} edit {edit}: the effects of {} workers' rebuild",
                            [1, 2, 4, 8][k]
                        );
                        assert_eq!(
                            w.1,
                            got.1,
                            "seed {seed} edit {edit}: the program of {} workers' rebuild",
                            [1, 2, 4, 8][k]
                        );
                    }
                }
            }
        }
    }
    eprintln!("runs taken from workers: {taken}");
    assert!(taken > 0, "no worker's run was taken");
}
