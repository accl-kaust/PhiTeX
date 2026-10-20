//! Sequences: pushes, takes and replacements of a field in one frame, read
//! in text order by later nodes of the same frame.
//!
//! A push is a local definition of the field; a take or a replacement is one
//! too, with a reserved value saying which.

use super::core::Core;
use super::world::{Id, World};
use crate::values::{Field, ValueId, Values};

const TAKE: &str = "\u{1}defuse:seq-take";
const REPL: &str = "\u{1}defuse:seq-repl";

/// One entry of a sequence.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum SeqOp {
    Push(ValueId),
    Take,
    Repl(ValueId),
}

pub(crate) fn take(v: &Values) -> ValueId {
    v.str(TAKE)
}

pub(crate) fn repl(v: &Values, r: ValueId) -> ValueId {
    v.list(vec![v.str(REPL), r])
}

fn decode(v: &Values, x: ValueId) -> SeqOp {
    if x == v.str(TAKE) {
        return SeqOp::Take;
    }
    let r = v.str(REPL);
    match v.with_list(x, |l| (l.len() == 2 && l[0] == r).then(|| l[1])).flatten() {
        Some(y) => SeqOp::Repl(y),
        None => SeqOp::Push(x),
    }
}

impl World {
    /// The entries of sequence `f` before node `me` in its frame, in text
    /// order (`Err`: an entry's node has not made it yet).
    pub(crate) fn seq_ops(&self, core: &Core, me: Id, f: Field) -> Result<Vec<SeqOp>, Id> {
        let Some((u, pos, fr)) = self.frame_at(me, false) else { return Ok(Vec::new()) };
        let Some(fd) = self.defs.get(&(u, f)) else { return Ok(Vec::new()) };
        let Some(m) = fd.local.get(&fr) else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for (_, e) in m.range(std::ops::Bound::Unbounded, std::ops::Bound::Excluded(&pos)) {
            if e.node == me {
                continue;
            }
            match e.val {
                Some(x) => out.push(decode(&core.values, x)),
                None => return Err(e.node),
            }
        }
        Ok(out)
    }

    /// Sequence `f` before `me` (a sequence value; empty `NIL`), or its last
    /// element (`prev`).
    pub(crate) fn seq_read(&self, core: &Core, me: Id, f: Field, prev: bool) -> Result<Option<ValueId>, Id> {
        let ops = self.seq_ops(core, me, f)?;
        let v = &core.values;
        if prev {
            let mut k = 0usize;
            for op in ops.iter().rev() {
                match *op {
                    SeqOp::Take => k += 1,
                    SeqOp::Push(x) if k == 0 => return Ok(Some(x)),
                    SeqOp::Push(_) => k -= 1,
                    SeqOp::Repl(x) if k == 0 => return Ok(Some(x)),
                    SeqOp::Repl(_) => {}
                }
            }
            return Ok(None);
        }
        let mut items: Vec<ValueId> = Vec::new();
        for op in ops {
            match op {
                SeqOp::Push(x) => items.push(x),
                SeqOp::Take => {
                    items.pop();
                }
                SeqOp::Repl(x) => {
                    items.pop();
                    items.push(x);
                }
            }
        }
        Ok(Some(items.into_iter().fold(ValueId::NIL, |acc, x| v.seq_concat(acc, v.seq_unit(x)))))
    }
}
