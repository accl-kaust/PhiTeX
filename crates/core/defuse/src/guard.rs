//! The readers of a field at a node, found from the records
//! (docs/spec-guard-reads.md §2.2-§3): no reader is filed anywhere.
//!
//! A node's record holds its reads (`reads::Reads`, by field); a read at
//! another node's position is found through that node's
//! [`Foreign::readers`], a derived read's witness in its
//! [`Foreign::witness`]. A segment keeps the fields read at its nodes
//! (`segment::Inner::guarded`), and the field index the segments whose
//! set has a field (`Engine::readers`), so a walk scans only the nodes
//! of segments that read its field, up to the definition that hides the
//! one walked from.

use crate::api::NodeId;
use crate::engine::Engine;
use crate::node::{Anchor, ReadKind, ReadRec};
use crate::segment::{Inner, Reader};
use crate::values::{Field, FxHashSet};

/// What other nodes read at a node: the readers whose records hold reads
/// anchored at it (`At::Before`, `At::AfterOwn`), and the witnesses of
/// derived reads filed at it (`commit::file_witness`). Filed under the
/// node's segment's lock by the reader's commit.
#[derive(Default)]
pub(crate) struct Foreign {
    pub(crate) readers: FxHashSet<NodeId>,
    pub(crate) witness: Vec<(Field, Reader)>,
}

/// A read of `x`'s record or of a reader's anchored at `x`, as the walks
/// ask it: its anchor `x` and whether it counts `x`'s own definition.
fn at_x(x: NodeId, me: NodeId, rd: &ReadRec) -> Option<(Field, bool)> {
    match *rd.kind() {
        ReadKind::Field { f, at: Anchor::Me } if me == x => Some((f, false)),
        ReadKind::Field {
            f,
            at: Anchor::Before(a),
        } if a == x => Some((f, false)),
        ReadKind::Field {
            f,
            at: Anchor::AfterOwn(a),
        } if a == x => Some((f, true)),
        _ => None,
    }
}

/// Each reader of `f` (of every field, None) anchored at node `x`, as a
/// reader record (its node, its record's serial, anchor `x`, path,
/// answer): `x`'s own reads at itself, its foreign readers' reads at it,
/// and its live witnesses. Under `x`'s segment's lock (any guard of it),
/// the nodes' locks taken one at a time.
pub(crate) fn readers_at(eng: &Engine, x: NodeId, f: Option<Field>, out: &mut dyn FnMut(Field, Reader)) {
    let n = eng.node(x);
    of_record(eng, x, x, f, out);
    let (foreign, witness) = {
        let fo = n.foreign.lock();
        if fo.readers.is_empty() && fo.witness.is_empty() {
            return;
        }
        let rs: Vec<NodeId> = fo.readers.iter().copied().collect();
        let ws: Vec<(Field, Reader)> = fo
            .witness
            .iter()
            .filter(|(g, r)| f.is_none_or(|f| f == *g) && crate::segment::live(eng, r.node(), r.serial))
            .copied()
            .collect();
        (rs, ws)
    };
    for r in foreign {
        if r != x {
            of_record(eng, x, r, f, out);
        }
    }
    for (g, r) in witness {
        out(g, r);
    }
}

/// [`readers_at`] for the record of node `me`.
fn of_record(eng: &Engine, x: NodeId, me: NodeId, f: Option<Field>, out: &mut dyn FnMut(Field, Reader)) {
    let Some(rn) = eng.placed_node(me) else { return };
    let last = rn.last.lock();
    let Some(r) = last.as_ref().filter(|r| !r.provisional) else {
        return;
    };
    let mut prev: Option<ReadRec> = None;
    let mut each = |rd: ReadRec| {
        // (a read made again with the same answer is one reader: the
        // record's reads come sorted, its copies side by side)
        if prev.is_some_and(|p| p.key() == rd.key() && p.seen() == rd.seen()) {
            return;
        }
        prev = Some(rd);
        if let Some((g, inclusive)) = at_x(x, me, &rd) {
            out(g, Reader::new(me, r.serial, x, inclusive, rd.path(), rd.seen()));
        }
    };
    match f {
        Some(f) => r.reads.of_field(f).for_each(&mut each),
        None => r.reads.iter().for_each(&mut each),
    }
}

/// The fields read at node `x` ([`readers_at`]'s, each once or more).
pub(crate) fn fields_at(eng: &Engine, x: NodeId, out: &mut dyn FnMut(Field)) {
    readers_at(eng, x, None, &mut |g, _| out(g));
}

/// The fields read at a segment's nodes now, each once: what its
/// `guarded` set is after a recount.
pub(crate) fn fields_of(eng: &Engine, inner: &Inner) -> FxHashSet<Field> {
    let mut s = FxHashSet::default();
    for &x in inner.nodes.values() {
        fields_at(eng, x, &mut |g| {
            s.insert(g);
        });
    }
    s
}

/// Its `guarded` set counted again from its nodes: the fields that left
/// it, and those that entered it (a node moved in), for the field index.
pub(crate) fn recount(eng: &Engine, inner: &mut Inner) -> (Vec<Field>, Vec<Field>) {
    let now = fields_of(eng, inner);
    let gone: Vec<Field> = inner.guarded.iter().filter(|f| !now.contains(f)).copied().collect();
    let new: Vec<Field> = now.iter().filter(|f| !inner.guarded.contains(f)).copied().collect();
    inner.guarded = now;
    (gone, new)
}

/// The reads of node `r`'s record at node `x`, as reader records.
pub(crate) fn record_reads_at(eng: &Engine, x: NodeId, r: NodeId, out: &mut dyn FnMut(Field, Reader)) {
    of_record(eng, x, r, None, out);
}

/// Whether node `r`'s record reads at node `x` (a foreign reader that
/// still stands for it).
pub(crate) fn reads_at(eng: &Engine, x: NodeId, r: NodeId, out: &mut bool) {
    of_record(eng, x, r, None, &mut |_, _| *out = true);
}
