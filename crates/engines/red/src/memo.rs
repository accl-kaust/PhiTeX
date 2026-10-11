//! A1: memo of applications keyed by (op, args, values read).
//!
//! * Nodes: keyed by the identity of the input they start from; a candidate
//!   is reused when every value it read still holds at the new thunk
//!   (`Graph::holds`). The op is a function of those, so it is not a key.
//! * Macro applications: the substituted body keyed by (macro, args).
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use crate::graph::Graph;
use crate::term::{Input, Node, Src, Thunk};
use crate::tok::Toks;
use crate::val::Macro;

#[derive(Default)]
pub struct Memo {
    nodes: Mutex<HashMap<u64, Vec<(Input, Arc<Node>)>>>,
    applies: Mutex<HashMap<(Arc<Macro>, Arc<[Toks]>), Toks>>,
}

fn hash_src(s: &Src, h: &mut impl Hasher) {
    match s {
        Src::Text { text, pos, line, line_end, eol, ending, .. } => {
            0u8.hash(h);
            text.hash(h);
            pos.hash(h);
            line.hash(h);
            line_end.hash(h);
            eol.hash(h);
            ending.hash(h);
        }
        Src::Toks { toks, at, kind } => {
            1u8.hash(h);
            (toks.as_ptr() as usize).hash(h);
            toks.len().hash(h);
            at.hash(h);
            kind.hash(h);
        }
    }
}

/// A hash of an input's identity: its top two sources and the pointer below.
pub fn input_key(i: &Input) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut cur = i.clone();
    for _ in 0..2 {
        match cur.top() {
            Some(s) => hash_src(s, &mut h),
            None => {
                9u8.hash(&mut h);
                return h.finish();
            }
        }
        cur = cur.below();
    }
    cur.0.as_ref().map_or(0usize, |a| Arc::as_ptr(a) as usize).hash(&mut h);
    h.finish()
}

impl Memo {
    /// A node reduced before from an equal input whose reads hold at `t`.
    pub fn find(&self, g: &Graph, t: &Thunk) -> Option<Arc<Node>> {
        let cands: Vec<(Input, Arc<Node>)> = {
            let m = self.nodes.lock().unwrap();
            m.get(&input_key(&t.input))?.clone()
        };
        cands
            .into_iter()
            .filter(|(i, _)| *i == t.input)
            .find(|(_, n)| g.reads_hold(t, &n.reads))
            .map(|(_, n)| n)
    }

    pub fn insert(&self, start: &Input, n: Arc<Node>) {
        let mut m = self.nodes.lock().unwrap();
        let v = m.entry(input_key(start)).or_default();
        // An identical reduction (same reads) need not be kept twice.
        if v.iter().any(|(i, o)| i == start && o.reads == n.reads) {
            return;
        }
        v.push((start.clone(), n));
    }

    /// The body of `f` applied to `args`, made by `make` the first time.
    pub fn apply(&self, f: &Arc<Macro>, args: &Arc<[Toks]>, make: impl FnOnce() -> Toks) -> Toks {
        let key = (f.clone(), args.clone());
        if let Some(b) = self.applies.lock().unwrap().get(&key) {
            return b.clone();
        }
        let b = make();
        self.applies.lock().unwrap().insert(key, b.clone());
        b
    }

    pub fn clear(&self) {
        self.nodes.lock().unwrap().clear();
        self.applies.lock().unwrap().clear();
    }
}
