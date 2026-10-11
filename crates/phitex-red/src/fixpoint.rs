//! A4: the loop-back fixpoint: written files (`.aux`, `.toc`, derived `.bbl`,
//! `.ind`) answer `Key::File` reads in the next round; repeat until equal (≤ 5).
//!
//! A round reduces the graph from the PDF root. The files the job wrote are
//! collected from the nodes in document order: a stream opens when a node's
//! delta binds `Key::WriteOpen(n)` to `Val::Name(file)` (`\immediate\openout`,
//! or an `\openout` whatsit performed at `\shipout`), closes when it binds it
//! to `Val::Void`, and `Emit::Write { stream, text }` (immediate or at shipout)
//! appends `text` and a newline to the file open on that stream. Derived files
//! (`.bbl`, `.ind`) come from `derive::derive` on the written set.
//!
//! The next round's answers are that set. A node that read `Key::File(name)`
//! and saw a value other than the new answer is stale: only those are
//! invalidated (`edit::invalidate_reads`) and the graph re-reduced. No stale
//! read: the fixpoint is reached. Bounded by `MAX_ROUNDS`.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use crate::env::Change;
use crate::graph::Graph;
use crate::output::Shipped;
use crate::term::Emit;
use crate::val::{FileName, Key, Val};

pub const MAX_ROUNDS: u32 = 5;

/// The build's givens.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `\jobname`.
    pub job_name: Arc<str>,
    /// `Key::Given(n)` answers (seed, clock, creation date …).
    pub givens: BTreeMap<u8, Val>,
    /// The files the last build wrote (warm start); empty on a cold build.
    pub previous: BTreeMap<FileName, Arc<str>>,
}

/// What a converged (or bounded) build produced.
#[derive(Clone, Debug)]
pub struct Converged {
    pub pages: Vec<Arc<Shipped>>,
    /// Every file the last round wrote, derived ones included.
    pub written: BTreeMap<FileName, Arc<str>>,
    pub rounds: u32,
    /// False: `MAX_ROUNDS` reached with reads still stale.
    pub converged: bool,
    /// The files whose reads were still stale at the bound.
    pub unsettled: Vec<FileName>,
}

/// The answers the driver gives the graph: givens and loop-back files.
#[derive(Debug, Default)]
pub struct Answers {
    pub job_name: Arc<str>,
    pub givens: BTreeMap<u8, Val>,
    /// Every name the job ever wrote in this build → its current text
    /// (`None`: known as written, but not written in the answering round).
    pub files: BTreeMap<FileName, Option<Arc<str>>>,
}

impl Answers {
    /// The driver's answer to `key`, or `None` when it is not the driver's
    /// (the graph answers from env/disk).
    pub fn answer(&self, key: &Key) -> Option<Val> {
        match key {
            Key::JobName => Some(Val::Name(self.job_name.clone())),
            Key::Given(n) => Some(self.givens.get(n).cloned().unwrap_or(Val::Void)),
            Key::File(name) => self.files.get(name).map(|t| file_val(t.as_ref())),
            _ => None,
        }
    }
}

fn file_val(t: Option<&Arc<str>>) -> Val {
    match t {
        Some(s) => Val::Bytes(Arc::from(s.as_bytes())),
        None => Val::Void,
    }
}

/// Shared handle the graph consults on every read of a driver key.
pub type Oracle = Arc<RwLock<Answers>>;

/// `\openout` names: TeX adds `.tex` when the name has no extension.
pub fn out_name(name: &str) -> FileName {
    let base = name.rsplit('/').next().unwrap_or(name);
    if base.contains('.') { Arc::from(name) } else { Arc::from(format!("{name}.tex")) }
}

/// The files written by the reduced graph, in document order, and the pages.
fn collect(graph: &Graph) -> (BTreeMap<FileName, Arc<str>>, Vec<Arc<Shipped>>) {
    let mut open: BTreeMap<u8, FileName> = BTreeMap::new();
    let mut text: BTreeMap<FileName, String> = BTreeMap::new();
    let mut pages = Vec::new();
    for node in graph.nodes() {
        let mut closes = Vec::new();
        for ch in &node.out.delta.0 {
            if let Change::Def { key: Key::WriteOpen(n), val, .. } = ch {
                match val {
                    Val::Name(f) => {
                        let f = out_name(f);
                        text.insert(f.clone(), String::new());
                        open.insert(*n, f);
                    }
                    _ => closes.push(*n),
                }
            }
        }
        for e in &node.out.emits {
            match e {
                Emit::Ship(p) => pages.push(p.clone()),
                Emit::Write { stream, text: t } if *stream < 16 => {
                    if let Some(f) = open.get(stream) {
                        let s = text.entry(f.clone()).or_default();
                        s.push_str(t);
                        s.push('\n');
                    }
                }
                _ => {}
            }
        }
        for n in closes {
            open.remove(&n);
        }
    }
    (text.into_iter().map(|(k, v)| (k, Arc::from(v))).collect(), pages)
}

/// Driver keys some node read with a value other than the current answer.
fn stale(graph: &Graph, ans: &Answers) -> BTreeSet<Key> {
    let mut out = BTreeSet::new();
    for node in graph.nodes() {
        for (k, v) in &node.reads {
            if matches!(k, Key::File(_) | Key::JobName | Key::Given(_)) {
                if let Some(a) = ans.answer(k) {
                    if &a != v {
                        out.insert(k.clone());
                    }
                }
            }
        }
    }
    out
}

/// Builds to the loop-back fixpoint.
pub fn build(opts: &Options, graph: &mut Graph) -> Converged {
    let oracle: Oracle = Arc::new(RwLock::new(Answers {
        job_name: opts.job_name.clone(),
        givens: opts.givens.clone(),
        files: opts.previous.iter().map(|(k, v)| (k.clone(), Some(v.clone()))).collect(),
    }));
    graph.set_oracle(oracle.clone());
    // A warm graph may hold reads of an older answer set.
    {
        let keys = stale(graph, &oracle.read().unwrap());
        if !keys.is_empty() {
            crate::edit::invalidate_reads(graph, &keys.into_iter().collect::<Vec<_>>());
        }
    }
    let mut rounds = 0;
    loop {
        rounds += 1;
        graph.reduce();
        let (mut written, pages) = collect(graph);
        for (name, t) in crate::derive::derive(&written) {
            written.insert(name, t);
        }
        // The next answers: this round's set; names written before but not now read as absent.
        {
            let mut a = oracle.write().unwrap();
            for v in a.files.values_mut() {
                *v = None;
            }
            for (k, v) in &written {
                a.files.insert(k.clone(), Some(v.clone()));
            }
        }
        let keys = stale(graph, &oracle.read().unwrap());
        if keys.is_empty() || rounds >= MAX_ROUNDS {
            let unsettled: Vec<FileName> =
                keys.iter().filter_map(|k| if let Key::File(f) = k { Some(f.clone()) } else { None }).collect();
            return Converged { pages, written, rounds, converged: keys.is_empty(), unsettled };
        }
        crate::edit::invalidate_reads(graph, &keys.into_iter().collect::<Vec<_>>());
    }
}
