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

use phitex_defuse::{Field, FxHashMap, Value, ValueId, Values};

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

impl Default for ListMemo {
    fn default() -> ListMemo {
        ListMemo {
            root: ValueId::UNIT,
            entries: Vec::new(),
            blocks: Vec::new(),
            items: Vec::new(),
        }
    }
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
    /// The flush points met folding this page (`(node, k, counts)`: no index,
    /// so a shifted page keeps them).
    pub flushes: Vec<(i64, i64, ValueId)>,
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
    pub items: ListMemo,
    /// The insertions on the list by class.
    pub ins: std::collections::BTreeMap<u16, isize>,
    pub table: ValueId,
    pub marker: ValueId,
    pub pages: Vec<Arc<PageRec>>,
    /// `pp:items:<j>`, `pp:start:<j>`, `pp:mk:<j>`.
    pub fields: Vec<[Field; 3]>,
    pub by_idx: FxHashMap<usize, Vec<usize>>,
    pub epoch: u64,
    pub spec: bool,
}

type Cache = Mutex<FxHashMap<u64, Arc<Plan>>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Cache::default)
}

/// The previous run's plan for the value table `v`, taken, when its
/// values are still valid at `epoch`.
pub(crate) fn take(v: &Values, epoch: u64) -> Option<Plan> {
    let mut c = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let p = Arc::try_unwrap(c.remove(&v.instance())?).ok()?;
    (!p.spec || p.epoch == epoch).then_some(p)
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

/// Where the fold changed: the items before `pre` and the last `suf`
/// are the old ones (moved), `removed` the old items between, and the
/// old fold had `old_len` items.
pub(crate) struct Change {
    pub pre: usize,
    pub suf: usize,
    pub removed: Vec<ValueId>,
    pub old_len: usize,
}

/// The main list's entries `all` folded, `prev`'s fold changed in place
/// where the edit changed it; `None` when nothing of it was kept.
#[allow(clippy::too_many_lines)]
pub(crate) fn fold(v: &Values, all: ValueId, prev: Option<ListMemo>) -> (ListMemo, Option<Change>) {
    let full = |entries: Vec<ValueId>| {
        let mut items = Vec::new();
        let mut blocks = Vec::new();
        let mut st = IGNORE_DEPTH_STATE;
        let mut at = 0;
        while at < entries.len() {
            let end = block_end(&entries, at);
            let del = deletes(v, &entries[at..end]);
            blocks.push((at, items.len(), st, del));
            fold_entries(v, 0, &entries[at..end], &mut st, &mut items);
            at = end;
        }
        ListMemo {
            root: all,
            entries,
            blocks,
            items,
        }
    };
    let Some(mut p) = prev else {
        return (full(v.seq_to_vec(all)), None);
    };
    let old_len = p.items.len();
    if p.root == all {
        return (
            p,
            Some(Change {
                pre: old_len,
                suf: 0,
                removed: Vec::new(),
                old_len,
            }),
        );
    }
    let (old_n, new_n) = (p.entries.len() as u64, v.seq_len(all));
    let pre = common(v, p.root, all, false).min(old_n.min(new_n));
    let suf = common(v, p.root, all, true).min(old_n.min(new_n) - pre);
    let mut mid = Vec::new();
    range(v, all, pre, new_n - suf, &mut mid);
    let (pre, suf) = (to_usize(pre), to_usize(suf));
    let (old_n, new_n) = (to_usize(old_n), to_usize(new_n));
    if pre + mid.len() + suf != new_n {
        return (full(v.seq_to_vec(all)), None);
    }
    p.entries.splice(pre..old_n - suf, mid);
    // the last block starting at or before the change, and none that
    // deletes after it (a deletion reaches into the items before it)
    let mut bi = p.blocks.partition_point(|b| b.0 <= pre).saturating_sub(1);
    while bi > 0 && p.blocks[bi].3 {
        bi -= 1;
    }
    let Some(&(mut at, out0, mut st, _)) = p.blocks.get(bi) else {
        return (full(p.entries), None);
    };
    let dn = delta(new_n, old_n);
    let mut fresh: Vec<ValueId> = Vec::new();
    let mut fblocks = Vec::new();
    while at < p.entries.len() {
        if at >= new_n - suf
            && let Some(o) = at.checked_add_signed(-dn)
            && let Ok(k) = p.blocks.binary_search_by_key(&o, |b| b.0)
            && converges(st, &p, k)
        {
            let made = fresh.len();
            let removed = splice(&mut p, bi, k, out0, fresh, fblocks, st, dn);
            p.root = all;
            // (narrowed to the items that differ)
            let new = &p.items[out0..out0 + made];
            let a = removed.iter().zip(new).take_while(|(x, y)| x == y).count();
            let b = removed[a..]
                .iter()
                .rev()
                .zip(new[a..].iter().rev())
                .take_while(|(x, y)| x == y)
                .count();
            let suf = p.items.len() - out0 - made + b;
            return (
                p,
                Some(Change {
                    pre: out0 + a,
                    suf,
                    removed: removed[a..removed.len() - b].to_vec(),
                    old_len,
                }),
            );
        }
        let end = block_end(&p.entries, at);
        if deletes(v, &p.entries[at..end]) {
            // (a deletion may reach into the items before the change)
            return (full(p.entries), None);
        }
        fblocks.push((at, out0 + fresh.len(), st, false));
        fold_entries(v, 0, &p.entries[at..end], &mut st, &mut fresh);
        at = end;
    }
    let removed: Vec<ValueId> = p.items.drain(out0..).collect();
    p.items.extend(fresh);
    p.blocks.truncate(bi);
    p.blocks.extend(fblocks);
    p.root = all;
    (
        p,
        Some(Change {
            pre: out0,
            suf: 0,
            removed,
            old_len,
        }),
    )
}

/// The old fold from its block `k` on after the new items `fresh` and
/// their blocks (the old ones from `bi` replaced): the same entries in
/// the same state (the items since the last `build_page` aside, which
/// only a deletion reads). The old items replaced.
#[allow(clippy::too_many_arguments)]
fn splice(
    p: &mut ListMemo,
    bi: usize,
    k: usize,
    out0: usize,
    fresh: Vec<ValueId>,
    fblocks: Vec<(usize, usize, FoldState, bool)>,
    st: FoldState,
    dn: isize,
) -> Vec<ValueId> {
    let k_out = p.blocks[k].1;
    let dout = delta(out0 + fresh.len(), k_out);
    let dbp = delta(st.since_bp, p.blocks[k].2.since_bp);
    let removed: Vec<ValueId> = p.items.splice(out0..k_out, fresh).collect();
    let mut prev_bp = 0;
    let mut shifting = true;
    let tail: Vec<_> = p.blocks[k..]
        .iter()
        .map(|&(e, o2, mut s, d)| {
            // (until a `build_page` sets the count again)
            shifting &= s.since_bp >= prev_bp;
            prev_bp = s.since_bp;
            if shifting {
                s.since_bp = s.since_bp.saturating_add_signed(dbp);
            }
            (
                e.saturating_add_signed(dn),
                o2.saturating_add_signed(dout),
                s,
                d,
            )
        })
        .collect();
    p.blocks.truncate(bi);
    p.blocks.extend(fblocks);
    p.blocks.extend(tail);
    removed
}

/// Whether the fold in state `st` reaches the same items as the old
/// fold from its block `k` on: the same state, or the same but for the
/// count of items since the last `build_page`, which only a deletion
/// reads (`build::fold_item`), when no block from there on deletes.
fn converges(st: FoldState, p: &ListMemo, k: usize) -> bool {
    let o = p.blocks[k].2;
    if p.blocks[k].3 {
        return false;
    }
    st == o || (st.sf == o.sf && st.pd == o.pd && !p.blocks[k..].iter().any(|b| b.3))
}

/// `a - b`, signed.
pub(crate) fn delta(a: usize, b: usize) -> isize {
    isize::try_from(a).unwrap_or(isize::MAX) - isize::try_from(b).unwrap_or(isize::MAX)
}

fn to_usize(x: u64) -> usize {
    usize::try_from(x).unwrap_or(usize::MAX)
}
