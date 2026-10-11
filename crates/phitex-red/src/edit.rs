//! A3: edits: CST splices → un-reduction of touched nodes → re-reduction,
//! reusing old nodes whose reads still hold; equal outputs stop propagation.
//!
//! An edit is a set of byte changes per text. A node whose consumed spans
//! (or its untaken arm) touch a change is un-reduced: the chain is cut back
//! to the thunk before the earliest such node, its input remapped onto the
//! new texts, and reduction restarts there. At every new thunk we look for
//! the old node that started at the same (mapped) input; if all of its reads
//! hold in the new env, it is reused — and so are the old nodes after it,
//! while theirs hold (memoized un-reduction). Nodes whose reads changed are
//! re-applied; a re-applied node with the same emits as the old one counts
//! as unchanged for the folds downstream (page builder, PDF).
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use crate::graph::Graph;
use crate::term::{Input, Node, Op, Span, Src, TextId, Thunk};

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
    pub nodes: Vec<Node>,
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

/// Positions in the old texts → the new ones.
struct Map {
    edits: HashMap<TextId, Vec<TextEdit>>,
    data: HashMap<TextId, Arc<str>>,
}

impl Map {
    fn pos(&self, text: TextId, p: u32) -> u32 {
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

    fn span(&self, s: Span) -> Span {
        Span { text: s.text, from: self.pos(s.text, s.from), to: self.pos(s.text, s.to) }
    }

    /// Closed overlap: an insertion at a span's edge touches it (a run of
    /// characters or a scanned number could have read further).
    fn touches(&self, s: &Span) -> bool {
        self.edits.get(&s.text).is_some_and(|es| es.iter().any(|e| s.from <= e.to && e.from <= s.to))
    }

    fn node_touched(&self, n: &Node) -> bool {
        n.spans.iter().any(|s| self.touches(s))
            || matches!(&n.op, Op::Select { untaken: Some(s), .. } if self.touches(s))
    }

    fn src(&self, s: &Src) -> Src {
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

    fn input(&self, i: &Input) -> Input {
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

    fn node(&self, n: &Node) -> Node {
        let mut m = n.clone();
        m.spans = n.spans.iter().map(|s| self.span(*s)).collect();
        if let Op::Select { taken, untaken } = &n.op {
            m.op = Op::Select { taken: *taken, untaken: untaken.map(|s| self.span(s)) };
        }
        m.next = Thunk { env: n.next.env.clone(), input: self.input(&n.next.input) };
        m
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

fn reads_hold(g: &Graph, t: &Thunk, n: &Node) -> bool {
    n.reads.iter().all(|r| crate::graph::read_holds(g, t, r))
}

/// Applies `changes` to the chain: returns the new chain and what changed.
pub fn update(graph: &Graph, old: &Chain, changes: &Changes) -> (Chain, Report) {
    let map = Map { edits: changes.sorted(), data: changes.data.clone() };
    let n = old.nodes.len();
    let touched: Vec<bool> = old.nodes.iter().map(|x| map.node_touched(x)).collect();
    let mut report = Report::default();
    let first = touched.iter().position(|&t| t);
    report.first_touched = first;

    let start = Thunk { env: old.start.env.clone(), input: map.input(&old.start.input) };
    let Some(k) = first else {
        // nothing consumed was touched (e.g. text after `\end`): offsets only
        let nodes: Vec<Node> = old.nodes.iter().map(|x| map.node(x)).collect();
        report.origin = (0..n).map(Origin::Kept).collect();
        return (Chain { start, nodes }, report);
    };

    let mut nodes: Vec<Node> = old.nodes[..k].iter().map(|x| map.node(x)).collect();
    report.origin = (0..k).map(Origin::Kept).collect();
    let mut thunk = if k == 0 { start.clone() } else { nodes[k - 1].next.clone() };

    // the old inputs each later node started from, remapped; indexed by key
    let pre: Vec<Option<Input>> = (0..n)
        .map(|j| {
            if j < k || touched[j] {
                None
            } else {
                let i = if j == 0 { &old.start.input } else { &old.nodes[j - 1].next.input };
                Some(map.input(i))
            }
        })
        .collect();
    let mut index: HashMap<InKey, Vec<usize>> = HashMap::new();
    for (j, p) in pre.iter().enumerate() {
        if let Some(i) = p {
            index.entry(in_key(i)).or_default().push(j);
        }
    }

    let mut cursor = k; // old nodes before it are accounted for
    let mark_dropped = |report: &mut Report, from: usize, to: usize| {
        report.dropped.extend(from..to);
    };

    loop {
        if graph_halted(graph) {
            break;
        }
        // align with an old node starting at this input
        let mut counterpart = None;
        if let Some(c) = index.get(&in_key(&thunk.input)) {
            let s = c.partition_point(|&j| j < cursor);
            counterpart = c[s..]
                .iter()
                .copied()
                .find(|&j| pre[j].as_ref().is_some_and(|p| input_eq(p, &thunk.input)));
        }
        if let Some(mut j) = counterpart {
            // memoized un-reduction: reuse while reads hold
            let mut reused = false;
            while j < n && !touched[j] && reads_hold(graph, &thunk, &old.nodes[j]) {
                let o = &old.nodes[j];
                let mut m = map.node(o);
                m.next = Thunk { env: thunk.env.apply(&o.out.delta), input: m.next.input };
                mark_dropped(&mut report, cursor, j);
                report.origin.push(Origin::Kept(j));
                cursor = j + 1;
                thunk = m.next.clone();
                nodes.push(m);
                j += 1;
                reused = true;
            }
            // the old node at the new thunk (if still aligned) is the re-applied one's counterpart
            counterpart = if reused {
                (j < n && !touched[j]).then_some(j)
            } else {
                Some(j)
            };
            if reused && j >= n {
                counterpart = None;
            }
        }
        let Some(node) = crate::graph::reduce(graph, &thunk) else { break };
        let origin = match counterpart {
            // same items (definitions may differ: later nodes' reads decide): folds see no change
            Some(j) if old.nodes[j].out.emits == node.out.emits => {
                mark_dropped(&mut report, cursor, j);
                cursor = j + 1;
                Origin::Same(j)
            }
            _ => Origin::New,
        };
        report.origin.push(origin);
        thunk = node.next.clone();
        nodes.push(node);
    }
    report.dropped.extend(cursor..n);
    // coalesce the New ones
    let mut i = 0;
    while i < report.origin.len() {
        if report.origin[i] == Origin::New {
            let s = i;
            while i < report.origin.len() && report.origin[i] == Origin::New {
                i += 1;
            }
            report.changed.push(s..i);
        } else {
            i += 1;
        }
    }
    (Chain { start, nodes }, report)
}

fn graph_halted(g: &Graph) -> bool {
    crate::graph::halted(g)
}
