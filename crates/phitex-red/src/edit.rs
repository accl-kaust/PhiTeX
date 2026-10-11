//! Edits as SSA propagation: the nodes whose consumed text an edit touches are
//! dirty; a dirty node is re-reduced where it stands; definitions whose value
//! changed dirty their users (def→use edges), a changed rest of input or
//! shape of definitions continues sequentially until the chains meet again.
//! Nothing else is visited: no walk to the end, no state comparison.
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use crate::graph::Graph;
use crate::env::{Change, DefCell, Env};
use crate::term::{Emit, Input, Node, NodeId, Op, Span, Src, TextId, Thunk};
use crate::val::Key;

/// One change to one text, in the OLD text's byte offsets: `from..to` was
/// replaced by `inserted` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub text: TextId,
    pub from: u32,
    pub to: u32,
    pub inserted: u32,
}

/// An edit: the changes (non-overlapping per text) and each changed text's new contents.
#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub edits: Vec<TextEdit>,
    pub data: HashMap<TextId, Arc<str>>,
}

impl Changes {
    /// One `Tree::edit(start, old_len, text)`-style change; `data` is the whole new text.
    pub fn add(&mut self, text: TextId, start: usize, old_len: usize, inserted: usize, data: Arc<str>) {
        self.edits.push(TextEdit { text, from: start as u32, to: (start + old_len) as u32, inserted: inserted as u32 });
        self.data.insert(text, data);
    }

    /// A CST splice: `old_ends` are the paragraph ends before `Tree::edit`,
    /// `tree` the tree after it. The paragraphs replaced map to one byte change.
    pub fn add_splice(
        &mut self,
        text: TextId,
        old_ends: &[usize],
        tree: &phitex_syntax::Tree,
        splice: &phitex_syntax::Splice,
        data: Arc<str>,
    ) {
        let start = if splice.at == 0 { 0 } else { old_ends[splice.at - 1] };
        let old_end = if splice.removed == 0 { start } else { old_ends[splice.at + splice.removed - 1] };
        let new_ends = tree.para_ends();
        let new_end = if splice.inserted == 0 { start } else { new_ends[splice.at + splice.inserted - 1] };
        self.add(text, start, old_end - start, new_end - start, data);
    }

    fn sorted(&self) -> HashMap<TextId, Vec<TextEdit>> {
        let mut m: HashMap<TextId, Vec<TextEdit>> = HashMap::new();
        for e in &self.edits {
            m.entry(e.text).or_default().push(*e);
        }
        for v in m.values_mut() {
            v.sort_by_key(|e| e.from);
        }
        m
    }
}

/// A reduced chain: the first thunk and the nodes reduced from it, in order.
#[derive(Clone, Debug)]
pub struct Chain {
    pub start: Thunk,
    pub nodes: Vec<Arc<Node>>,
}

/// Where a node of the new chain came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The old node, reused (reads held).
    Kept(usize),
    /// Re-applied at the old node's place with equal emits: nothing downstream changes.
    Same(usize),
    /// New material.
    New,
}

/// What changed, for the folds downstream (page, PDF).
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Per new node.
    pub origin: Vec<Origin>,
    /// New node index ranges whose emits are new.
    pub changed: Vec<Range<usize>>,
    /// Old node indices that have no counterpart in the new chain.
    pub dropped: Vec<usize>,
    /// The first old node un-reduced (None: nothing touched).
    pub first_touched: Option<usize>,
}

impl Report {
    /// The new node index from which emits may differ (None: no change).
    pub fn first_changed(&self) -> Option<usize> {
        let a = self.changed.first().map(|r| r.start);
        if self.dropped.is_empty() {
            return a;
        }
        // a dropped old node: its items are gone after the last new node kept before it
        let d = self.dropped[0];
        let b = self.origin.iter().position(|o| match o {
            Origin::Kept(j) | Origin::Same(j) => *j > d,
            Origin::New => false,
        });
        match (a, b) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y).or(Some(self.origin.len())),
        }
    }
}

/// Positions in one epoch's texts → the next epoch's (one edit).
#[derive(Debug)]
pub struct EditMap {
    edits: HashMap<TextId, Vec<TextEdit>>,
    data: HashMap<TextId, Arc<str>>,
}

impl EditMap {
    pub fn new(changes: &Changes) -> EditMap {
        EditMap { edits: changes.sorted(), data: changes.data.clone() }
    }
    pub fn pos(&self, text: TextId, p: u32) -> u32 {
        let Some(es) = self.edits.get(&text) else { return p };
        let mut shift: i64 = 0;
        for e in es {
            if e.to <= p && !(e.from == e.to && e.from == p) {
                shift += i64::from(e.inserted) - i64::from(e.to - e.from);
            } else if e.from < p {
                // inside a replaced range: after the inserted text
                return (i64::from(e.from) + shift + i64::from(e.inserted)) as u32;
            } else {
                break;
            }
        }
        (i64::from(p) + shift) as u32
    }

    pub fn span(&self, s: Span) -> Span {
        Span { text: s.text, from: self.pos(s.text, s.from), to: self.pos(s.text, s.to) }
    }

    /// Closed overlap: an insertion at a span's edge touches it (a run of
    /// characters or a scanned number could have read further).
    pub fn touches(&self, s: &Span) -> bool {
        self.edits.get(&s.text).is_some_and(|es| es.iter().any(|e| s.from <= e.to && e.from <= s.to))
    }


    pub fn src(&self, s: &Src) -> Src {
        match s {
            Src::Text { text, data, pos, line, line_end, eol, ending } => {
                let Some(new) = self.data.get(text) else { return s.clone() };
                let np = self.pos(*text, *pos);
                let changed_line = self
                    .edits
                    .get(text)
                    .is_some_and(|es| es.iter().any(|e| e.from <= *line_end && *pos <= e.to));
                let nle = if changed_line { line_end_of(new, np) } else { self.pos(*text, *line_end) };
                let _ = data;
                Src::Text {
                    text: *text,
                    data: new.clone(),
                    pos: np,
                    line: *line,
                    line_end: nle,
                    eol: *eol,
                    ending: *ending,
                }
            }
            Src::Toks { .. } => s.clone(),
        }
    }

    pub fn input(&self, i: &Input) -> Input {
        let mut v = Vec::new();
        let mut cur = i.clone();
        while let Some(s) = cur.top() {
            v.push(self.src(s));
            cur = cur.below();
        }
        let mut out = Input::default();
        for s in v.into_iter().rev() {
            out = out.push(s);
        }
        out
    }
}

/// The end of the line starting at or containing `p`, as the reader keeps
/// it: before the line terminator, trailing spaces removed (tex.web §31).
fn line_end_of(data: &str, p: u32) -> u32 {
    let b = data.as_bytes();
    let mut e = (p as usize).min(b.len());
    while e < b.len() && b[e] != b'\n' && b[e] != b'\r' {
        e += 1;
    }
    let mut t = e;
    while t > p as usize && b[t - 1] == b' ' {
        t -= 1;
    }
    t as u32
}

/// Input equality without comparing whole texts: a text is the same if it is
/// the same `Arc` (every remapped source shares its text's new data).
fn src_eq(a: &Src, b: &Src) -> bool {
    match (a, b) {
        (
            Src::Text { text: t1, data: d1, pos: p1, line: l1, line_end: e1, eol: o1, ending: n1 },
            Src::Text { text: t2, data: d2, pos: p2, line: l2, line_end: e2, eol: o2, ending: n2 },
        ) => {
            t1 == t2
                && p1 == p2
                && l1 == l2
                && e1 == e2
                && o1 == o2
                && n1 == n2
                && (Arc::ptr_eq(d1, d2) || d1 == d2)
        }
        (Src::Toks { toks: a1, at: x1, kind: k1 }, Src::Toks { toks: a2, at: x2, kind: k2 }) => {
            x1 == x2 && k1 == k2 && (Arc::ptr_eq(a1, a2) || a1[*x1 as usize..] == a2[*x2 as usize..])
        }
        _ => false,
    }
}

fn input_eq(a: &Input, b: &Input) -> bool {
    let (mut a, mut b) = (a.clone(), b.clone());
    loop {
        match (&a.0, &b.0) {
            (None, None) => return true,
            (Some(x), Some(y)) => {
                if Arc::ptr_eq(x, y) {
                    return true;
                }
                if !src_eq(&x.0, &y.0) {
                    return false;
                }
            }
            _ => return false,
        }
        a = a.below();
        b = b.below();
    }
}

/// A coarse key for finding the old node that started at an input: depth,
/// the head's shape, the first text position. Candidates are confirmed by `input_eq`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct InKey {
    depth: u32,
    head: (u8, u32, u32),
    text: Option<(TextId, u32)>,
}

fn in_key(i: &Input) -> InKey {
    let head = match i.top() {
        None => (0, 0, 0),
        Some(Src::Text { text, pos, .. }) => (1, *text, *pos),
        Some(Src::Toks { toks, at, .. }) => (2, toks.len() as u32, *at),
    };
    let mut depth = 0;
    let mut text = None;
    let mut cur = i.clone();
    while let Some(s) = cur.top() {
        depth += 1;
        if text.is_none()
            && let Src::Text { text: t, pos, .. } = s
        {
            text = Some((*t, *pos));
        }
        cur = cur.below();
    }
    InKey { depth, head, text }
}

/// Applies `changes`: the nodes whose consumed text they touch are dirty,
/// then dirtiness propagates along def→use and input edges (`propagate`).
pub fn update(graph: &Graph, old: &Chain, changes: &Changes) -> (Chain, Report) {
    let map = EditMap::new(changes);
    let mut dirty = BTreeSet::new();
    for (i, n) in old.nodes.iter().enumerate() {
        let touched = n.spans.iter().any(|s| map.touches(&graph.span_now(*s, n.epoch)))
            || matches!(&n.op, Op::Select { untaken: Some(s), .. } if map.touches(&graph.span_now(*s, n.epoch)));
        // `\inputlineno` readers after an edit of their text
        let line_reader = n.reads.iter().any(|(k, _)| *k == crate::val::Key::LineNo)
            && n.spans.first().is_some_and(|s| {
                let s = graph.span_now(*s, n.epoch);
                map.edits.get(&s.text).is_some_and(|es| es.iter().any(|e| e.from <= s.from))
            });
        if touched || line_reader {
            dirty.insert(i);
        }
    }
    let first = dirty.first().copied();
    graph.push_epoch(map);
    let (chain, mut report) = propagate(graph, old, dirty);
    report.first_touched = first;
    (chain, report)
}

/// The old chain's nodes by arena id → position.
fn positions(g: &Graph, nodes: &[Arc<Node>]) -> HashMap<NodeId, usize> {
    let _ = g;
    nodes.iter().enumerate().map(|(i, n)| (Graph::id_of(n), i)).collect()
}

/// Keys a delta defines (list keys included for a list it opens).
fn defined_keys(d: &crate::env::Delta, out: &mut HashSet<Key>) {
    for c in &d.0 {
        match c {
            Change::Def { key, .. } | Change::Replace { key, .. } => {
                out.insert(key.clone());
            }
            _ => {}
        }
    }
}

/// Marks the readers of what a changed list content affects: the node that
/// closes the list (it gathers the items) and the nodes before it that read
/// the list's tail. `from` is the first old node after the change.
fn dirty_list_readers(old: &[Arc<Node>], from: usize, dirty: &mut BTreeSet<usize>) {
    let mut depth = 0i32;
    for (j, n) in old.iter().enumerate().skip(from) {
        if n.reads.iter().any(|(k, _)| matches!(k, Key::Tail) || matches!(k, Key::Other(0, x) if crate::graph::pk::class(*x) == crate::graph::pk::class(crate::graph::pk::CHAIN)))
        {
            dirty.insert(j);
        }
        for e in &n.out.emits {
            match e {
                Emit::Open(_) => depth += 1,
                Emit::Close => {
                    if depth == 0 {
                        dirty.insert(j);
                        return;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        // a list closed by a node that gathers it without an Emit::Close (the outer list)
        if depth < 0 {
            return;
        }
    }
}

/// Whether the new env `a` (at a realignment candidate) binds every key of
/// `keys`, every open group's snapshot of them and every open list's keys as
/// the old env `b` does: the same definitions, or equal values (then the new
/// cell is forwarded to the old, so later readers keep one definition).
fn envs_meet(g: &Graph, a: &Env, b: &Env, keys: &HashSet<Key>) -> bool {
    let (ra, rb) = (&*a.inner, &*b.inner);
    if ra.depth() != rb.depth() || ra.list.depth != rb.list.depth {
        return false;
    }
    let mut pairs: Vec<(Arc<DefCell>, Arc<DefCell>)> = Vec::new();
    let entry_pair = |x: Option<&crate::graph::Entry>, y: Option<&crate::graph::Entry>, pairs: &mut Vec<(Arc<DefCell>, Arc<DefCell>)>| -> bool {
        match (x, y) {
            (None, None) => true,
            (Some(x), Some(y)) => {
                if x.cell.id_now() == y.cell.id_now() {
                    return true;
                }
                if x.level == y.level && x.cell.val() == y.cell.val() {
                    pairs.push((x.cell.clone(), y.cell.clone()));
                    return true;
                }
                false
            }
            _ => false,
        }
    };
    for k in keys {
        if !entry_pair(ra.defs.get(k), rb.defs.get(k), &mut pairs) {
            return false;
        }
    }
    // groups: kinds, after-tokens, snapshots and globals of the keys
    let (mut fa, mut fb) = (ra.group.clone(), rb.group.clone());
    while let (Some(x), Some(y)) = (fa.clone(), fb.clone()) {
        if x.kind != y.kind || x.after_toks() != y.after_toks() {
            return false;
        }
        for k in keys {
            if !entry_pair(x.snap.get(k), y.snap.get(k), &mut pairs) {
                return false;
            }
        }
        let gx: Vec<(Key, crate::graph::Entry)> = crate::graph::cons_list(&x.globals);
        let gy: Vec<(Key, crate::graph::Entry)> = crate::graph::cons_list(&y.globals);
        if gx.len() != gy.len() {
            return false;
        }
        for ((k1, e1), (k2, e2)) in gx.iter().zip(&gy) {
            if k1 != k2 || !entry_pair(Some(e1), Some(e2), &mut pairs) {
                return false;
            }
        }
        if crate::graph::cons_list(&x.locals) != crate::graph::cons_list(&y.locals) {
            return false;
        }
        fa = x.up.clone();
        fb = y.up.clone();
    }
    // lists: modes and list keys
    let (mut la, mut lb) = (Some(ra.list.clone()), Some(rb.list.clone()));
    while let (Some(x), Some(y)) = (la.clone(), lb.clone()) {
        if x.mode != y.mode {
            return false;
        }
        for w in 0..4 {
            let (cx, cy) = (&x.vals[w], &y.vals[w]);
            if cx.id_now() != cy.id_now() {
                if cx.val() != cy.val() {
                    return false;
                }
                pairs.push((cx.clone(), cy.clone()));
            }
        }
        la = x.up.clone();
        lb = y.up.clone();
    }
    // one definition per value from here on: the new cells stand for the old
    for (n, o) in pairs {
        g.forward_def(&n, &o);
    }
    true
}

/// Re-reduces the dirty nodes of `old` (positions) in document order and
/// propagates: a re-reduced node with the same shape keeps its definitions
/// (values updated in place) and dirties the users of those whose value
/// changed; one whose shape or rest of input differs is followed by
/// sequential reduction until the new chain meets the old one again (same
/// input, every differing binding equal). Untouched nodes are kept as they are.
pub fn propagate(graph: &Graph, old: &Chain, mut dirty: BTreeSet<usize>) -> (Chain, Report) {
    let n = old.nodes.len();
    let pos = positions(graph, &old.nodes);
    let start = Thunk { env: old.start.env.clone(), input: graph.input_now(&old.start.input, 0) };
    let mut out: Vec<Arc<Node>> = Vec::with_capacity(n);
    let mut report = Report::default();
    // the old starts, in the current epoch, by input key (built on first need)
    let mut index: Option<HashMap<InKey, Vec<usize>>> = None;
    let mut i = 0usize;
    loop {
        if graph.halted() {
            break;
        }
        let next = dirty.range(i..).next().copied();
        let upto = next.unwrap_or(n);
        for j in i..upto {
            out.push(old.nodes[j].clone());
            report.origin.push(Origin::Kept(j));
        }
        i = upto;
        if next.is_none() {
            break;
        }
        // re-reduce node i where it stands
        let thunk = match out.last() {
            None => start.clone(),
            Some(p) => Thunk { env: p.next.env.clone(), input: graph.input_now(&p.next.input, p.epoch) },
        };
        if Graph::done(&thunk) {
            report.dropped.extend(i..n);
            break;
        }
        let o = old.nodes[i].clone();
        let (node, changed) = graph.reduce_reusing(&thunk, &o);
        let old_next = graph.input_now(&o.next.input, o.epoch);
        let emits_same = node.out.emits == o.out.emits;
        if let (Some(changed), true) = (&changed, input_eq(&node.next.input, &old_next)) {
            for d in changed {
                for u in graph.users_of(*d) {
                    if let Some(&p) = pos.get(&u)
                        && p > i
                    {
                        dirty.insert(p);
                    }
                }
            }
            if !emits_same {
                dirty_list_readers(&old.nodes, i + 1, &mut dirty);
            }
            report.origin.push(if emits_same { Origin::Same(i) } else { Origin::New });
            out.push(node);
            i += 1;
            continue;
        }
        // the shape or the rest of the input changed: reduce on until the chains meet
        let mut differ: HashSet<Key> = HashSet::new();
        defined_keys(&node.out.delta, &mut differ);
        defined_keys(&o.out.delta, &mut differ);
        let mut t = node.next.clone();
        report.origin.push(Origin::New);
        out.push(node);
        let idx = index.get_or_insert_with(|| {
            let mut m: HashMap<InKey, Vec<usize>> = HashMap::new();
            for j in 0..n {
                let inp = if j == 0 { start.input.clone() } else { graph.input_now(&old.nodes[j - 1].next.input, old.nodes[j - 1].epoch) };
                m.entry(in_key(&inp)).or_default().push(j);
            }
            m
        });
        let mut met = None;
        loop {
            if graph.halted() || Graph::done(&t) {
                break;
            }
            // an old node starting here, after i, with the same bindings?
            if let Some(c) = idx.get(&in_key(&t.input)) {
                for &k in c.iter().filter(|&&k| k > i) {
                    let ok_input = {
                        let inp = graph.input_now(&old.nodes[k - 1].next.input, old.nodes[k - 1].epoch);
                        input_eq(&inp, &t.input)
                    };
                    if ok_input && envs_meet(graph, &t.env, &old.nodes[k - 1].next.env, &differ) {
                        met = Some(k);
                        break;
                    }
                }
            }
            if met.is_some() {
                break;
            }
            let nn = graph.reduce_one(&t);
            defined_keys(&nn.out.delta, &mut differ);
            t = nn.next.clone();
            report.origin.push(Origin::New);
            out.push(nn);
        }
        match met {
            Some(k) => {
                report.dropped.extend(i + 1..k);
                // the old list around the meeting point gathers new items
                dirty_list_readers(&old.nodes, k, &mut dirty);
                // dirty nodes skipped over were re-reduced as part of the walk
                let skipped: Vec<usize> = dirty.range(i..k).copied().collect();
                for s in skipped {
                    dirty.remove(&s);
                }
                i = k;
            }
            None => {
                report.dropped.extend(i + 1..n);
                break;
            }
        }
    }
    // coalesce the New ones
    let mut j = 0;
    while j < report.origin.len() {
        if report.origin[j] == Origin::New {
            let s = j;
            while j < report.origin.len() && report.origin[j] == Origin::New {
                j += 1;
            }
            report.changed.push(s..j);
        } else {
            j += 1;
        }
    }
    (Chain { start, nodes: out }, report)
}

/// The answers of `keys` changed (a loop-back file, an editor buffer): the
/// nodes that read them are dirty; propagate from them.
pub fn invalidate_reads(g: &Graph, keys: &[Key]) {
    let old = g.chain();
    let dirty: BTreeSet<usize> =
        old.nodes.iter().enumerate().filter(|(_, n)| n.reads.iter().any(|(k, _)| keys.contains(k))).map(|(i, _)| i).collect();
    if dirty.is_empty() {
        return;
    }
    let (chain, _) = propagate(g, &old, dirty);
    g.set_chain(chain);
}
