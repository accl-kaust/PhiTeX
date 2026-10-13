//! The page plan's own memory between its runs: the main list's
//! entries and their fold, and every page the plan folded, so that a
//! run after an edit folds again only what the edit changed.
//!
//! The entries are a sequence value (a canonical treap, shared between
//! versions): the edit's range is found by walking the old and the new
//! sequence together and skipping the subtrees they share, without
//! listing either. The fold (`build::fold_entries`, blocks bounded by
//! their content) runs again from the block before the change until it
//! reaches a block of the old list in the same state; the rest is the
//! old fold. A page is the old page, moved, when its start (but its
//! index), what the routine before it left, and the items it read are
//! unchanged.
//!
//! The values held here are the previous run's: they stay valid while
//! that run's definitions (or the evaluation it ran in) keep them, so a
//! speculative run's memory is used only in its own evaluation.

use std::sync::{Arc, Mutex, OnceLock};

use phitex_defuse::{FxHashMap, Value, ValueId, Values};

use crate::build::{FoldState, IGNORE_DEPTH_STATE, fold_entries};
use crate::page::PageStart;

/// The fold of the main list's entries: the sequence it was made from,
/// its entries, each block's start (entry index, items before it, the
/// state before it, whether the block deletes), and the items.
pub(crate) struct ListMemo {
    pub root: ValueId,
    pub entries: Vec<ValueId>,
    pub blocks: Vec<(usize, usize, FoldState, bool)>,
    pub items: Vec<ValueId>,
}

/// A page as a run of the plan found it, at its own indices.
#[derive(Clone)]
pub(crate) struct PageRec {
    pub start: PageStart,
    pub fx: Vec<ValueId>,
    pub reach: usize,
    pub fired: Option<(Vec<ValueId>, PageStart)>,
    pub passed: Vec<i64>,
    pub writes: Vec<ValueId>,
    pub stuck: bool,
    /// `pp:items:<j>` and `pp:start:<j>` as that run set them.
    pub items_v: ValueId,
    pub start_v: ValueId,
}

impl PageRec {
    /// The page moved by `d` items.
    pub(crate) fn shifted(&self, d: isize) -> PageRec {
        let mv = |i: usize| i.saturating_add_signed(d);
        let mut r = self.clone();
        r.start.idx = mv(r.start.idx);
        r.reach = mv(r.reach);
        if let Some((_, end)) = &mut r.fired {
            end.idx = mv(end.idx);
        }
        r
    }
}

/// What a run of the plan leaves for the next.
pub(crate) struct Plan {
    pub items: Arc<ListMemo>,
    pub table: ValueId,
    pub marker: ValueId,
    pub pages: Vec<PageRec>,
    pub by_idx: FxHashMap<usize, Vec<usize>>,
    pub epoch: u64,
    pub spec: bool,
}

type Cache = Mutex<FxHashMap<u64, Arc<Plan>>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Cache::default)
}

/// The previous run's plan for the value table `v`, when its values
/// are still valid at `epoch`.
pub(crate) fn previous(v: &Values, epoch: u64) -> Option<Arc<Plan>> {
    let c = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let p = c.get(&v.instance())?;
    (!p.spec || p.epoch == epoch).then(|| p.clone())
}

/// Keeps `p` for the next run.
pub(crate) fn keep(v: &Values, p: Plan) {
    let mut c = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // (bounded: one entry a build; a long test run starts again)
    if c.len() > 256 {
        c.clear();
    }
    c.insert(v.instance(), Arc::new(p));
}

/// Whether `a` and `b` are the same start but for the index.
pub(crate) fn same_start(a: &PageStart, b: &PageStart) -> bool {
    a.carried == b.carried
        && a.marker == b.marker
        && a.ending == b.ending
        && a.deadcycles == b.deadcycles
}

// ------------------------------------------------------------ sequences

fn node(v: &Values, t: ValueId) -> Option<(ValueId, ValueId, ValueId, u64)> {
    match &*v.get(t) {
        Value::Seq(n) => Some((n.left, n.elem, n.right, n.len)),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum It {
    Tree(ValueId, u64),
    Elem(ValueId),
}

fn push_tree(v: &Values, s: &mut Vec<It>, t: ValueId) {
    if let Some((.., n)) = node(v, t) {
        s.push(It::Tree(t, n));
    }
}

fn expand(v: &Values, s: &mut Vec<It>, rev: bool) {
    let Some(It::Tree(t, _)) = s.pop() else {
        return;
    };
    let Some((l, e, r, _)) = node(v, t) else {
        return;
    };
    let (first, last) = if rev { (r, l) } else { (l, r) };
    push_tree(v, s, last);
    s.push(It::Elem(e));
    push_tree(v, s, first);
}

/// How many elements `a` and `b` share at their start (`rev`: at their
/// end), skipping the subtrees they share.
fn common(v: &Values, a: ValueId, b: ValueId, rev: bool) -> u64 {
    let (mut sa, mut sb) = (Vec::new(), Vec::new());
    push_tree(v, &mut sa, a);
    push_tree(v, &mut sb, b);
    let mut n = 0;
    loop {
        let (Some(&x), Some(&y)) = (sa.last(), sb.last()) else {
            return n;
        };
        match (x, y) {
            (It::Tree(p, k), It::Tree(q, _)) if p == q => {
                n += k;
                sa.pop();
                sb.pop();
            }
            (It::Elem(p), It::Elem(q)) => {
                if p != q {
                    return n;
                }
                n += 1;
                sa.pop();
                sb.pop();
            }
            (It::Tree(_, k), It::Tree(_, m)) => {
                if k >= m {
                    expand(v, &mut sa, rev);
                } else {
                    expand(v, &mut sb, rev);
                }
            }
            (It::Tree(..), It::Elem(_)) => expand(v, &mut sa, rev),
            (It::Elem(_), It::Tree(..)) => expand(v, &mut sb, rev),
        }
    }
}

/// The elements of `t` from `lo` to `hi`, appended to `out`.
fn range(v: &Values, t: ValueId, lo: u64, hi: u64, out: &mut Vec<ValueId>) {
    if lo >= hi {
        return;
    }
    let Some((l, e, r, _)) = node(v, t) else {
        return;
    };
    let ll = node(v, l).map_or(0, |x| x.3);
    if lo < ll {
        range(v, l, lo, hi.min(ll), out);
    }
    if lo <= ll && ll < hi {
        out.push(e);
    }
    if hi > ll + 1 {
        range(v, r, lo.max(ll + 1) - ll - 1, hi - ll - 1, out);
    }
}

// ----------------------------------------------------------------- fold

fn block_end(entries: &[ValueId], at: usize) -> usize {
    entries[at..]
        .iter()
        .position(|x| x.raw() % 64 == 0)
        .map_or(entries.len(), |k| at + k + 1)
}

fn deletes(v: &Values, block: &[ValueId]) -> bool {
    block.iter().any(|&x| {
        matches!(&*v.get(x), Value::List(xs)
            if xs.len() == 2 && crate::nodes::tag(v, xs[1]) == crate::nodes::T_DELETE)
    })
}

/// The main list's entries `all` folded, from `prev`'s fold where the
/// edit left it alone.
pub(crate) fn fold(v: &Values, all: ValueId, prev: Option<&ListMemo>) -> Arc<ListMemo> {
    let full = || {
        let entries = v.seq_to_vec(all);
        refold(
            v,
            all,
            entries,
            Vec::new(),
            Vec::new(),
            0,
            IGNORE_DEPTH_STATE,
            None,
        )
    };
    let Some(p) = prev else {
        return Arc::new(full());
    };
    let (old_n, new_n) = (p.entries.len() as u64, v.seq_len(all));
    let pre = common(v, p.root, all, false).min(old_n.min(new_n));
    let suf = common(v, p.root, all, true).min(old_n.min(new_n) - pre);
    let (pre, suf) = (to_usize(pre), to_usize(suf));
    let (old_n, new_n) = (to_usize(old_n), to_usize(new_n));
    let mut entries = Vec::with_capacity(new_n);
    entries.extend_from_slice(&p.entries[..pre]);
    range(v, all, pre as u64, (new_n - suf) as u64, &mut entries);
    entries.extend_from_slice(&p.entries[old_n - suf..]);
    if entries.len() != new_n {
        return Arc::new(full());
    }
    // the last block starting at or before the change, and none that
    // deletes after it (a deletion reaches into the items before it)
    let mut bi = p.blocks.partition_point(|b| b.0 <= pre).saturating_sub(1);
    while bi > 0 && p.blocks[bi].3 {
        bi -= 1;
    }
    let Some(&(at, out0, st, _)) = p.blocks.get(bi) else {
        return Arc::new(full());
    };
    let items = p.items[..out0].to_vec();
    let blocks = p.blocks[..bi].to_vec();
    let dn = delta(new_n, old_n);
    if crate::exec::env_flag!("PHITEX_PLAN_TIME") {
        eprintln!(
            "plan fold: {old_n} -> {new_n} entries, common start {pre} end {suf}, from block {bi} at {at}"
        );
    }
    let r = refold(
        v,
        all,
        entries,
        blocks,
        items,
        at,
        st,
        Some((p, new_n - suf, dn)),
    );
    Arc::new(r)
}

/// `a - b`, signed.
pub(crate) fn delta(a: usize, b: usize) -> isize {
    isize::try_from(a).unwrap_or(isize::MAX) - isize::try_from(b).unwrap_or(isize::MAX)
}

fn to_usize(x: u64) -> usize {
    usize::try_from(x).unwrap_or(usize::MAX)
}

#[allow(clippy::too_many_arguments)]
fn refold(
    v: &Values,
    root: ValueId,
    entries: Vec<ValueId>,
    mut blocks: Vec<(usize, usize, FoldState, bool)>,
    mut items: Vec<ValueId>,
    mut at: usize,
    mut st: FoldState,
    old: Option<(&ListMemo, usize, isize)>,
) -> ListMemo {
    while at < entries.len() {
        if let Some((p, suf, dn)) = old
            && at >= suf
            && let Some(o) = at.checked_add_signed(-dn)
            && let Ok(k) = p.blocks.binary_search_by_key(&o, |b| b.0)
            && p.blocks[k].2 == st
            && !p.blocks[k].3
        {
            // the old fold from here on: the same entries in the same state
            let dout = delta(items.len(), p.blocks[k].1);
            items.extend_from_slice(&p.items[p.blocks[k].1..]);
            blocks.extend(p.blocks[k..].iter().map(|&(e, o2, s, d)| {
                (
                    e.saturating_add_signed(dn),
                    o2.saturating_add_signed(dout),
                    s,
                    d,
                )
            }));
            if crate::exec::env_flag!("PHITEX_PLAN_TIME") {
                eprintln!("plan fold: converged at entry {at} (suffix from {suf})");
            }
            return ListMemo {
                root,
                entries,
                blocks,
                items,
            };
        }
        let end = block_end(&entries, at);
        let del = deletes(v, &entries[at..end]);
        blocks.push((at, items.len(), st, del));
        fold_entries(v, 0, &entries[at..end], &mut st, &mut items);
        at = end;
    }
    ListMemo {
        root,
        entries,
        blocks,
        items,
    }
}
