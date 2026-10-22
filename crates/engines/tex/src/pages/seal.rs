//! Sealed lines: a finisher reads its PDF page's lines by their shape
//! (their dimensions and fonts), not by their characters. A piece gives
//! its unit's items with their shape ([`crate::nodes::unit_shape`]): each
//! line whose list a page's writer need not look into stands there as a
//! box whose list is a stand-in (`[T_SEALED, piece, k, fonts, head]`).
//! The page builder and the routine measure it, and nothing in the
//! finisher's run looks into it. Where a value leaves the run (a
//! definition, a contribution, the routine's give-back, a box opened), a
//! stand-in is replaced by the line's own glue setting and list, read
//! then ([`Run::unseal`]). The page itself leaves as its shape (the
//! `pages` entry, the page's key) and as the page box with its lines
//! passed on from the window, not read ([`REALPAGES`],
//! [`Run::contribute_real_pages`]): a word that keeps its line's
//! dimensions changes the page's writer's input, not the finisher's.
//!
//! Example: a page of lines A, B, C; a word edited in B. The finisher's
//! reads (the window's shapes) hold; the composed page box follows B and
//! is refiled; the page's PDF side (`crate::pdfout::PdfPage`) and the
//! link run again.

use phitex_defuse::{Part, Step, ValueId, Values, View};

use crate::nodes;
use crate::run::Run;
use crate::val::list_of;

/// The pages' boxes as shipped, their lines' lists in place: one entry a
/// shipped page, each finisher contributing the list of its pages'
/// boxes ([`ListSeqMonoid`]). Read by a page's PDF side and the link.
pub(crate) const REALPAGES: &str = "pdf:realpages";

/// A sequence whose contributions are lists: each list's elements in
/// order, so a contribution can be composed ([`phitex_defuse::Ctx::compose`]
/// makes lists).
pub(crate) struct ListSeqMonoid;

/// `x` as a sequence: a list's elements, a sequence as it is.
pub(crate) fn as_seq(v: &Values, x: ValueId) -> ValueId {
    match v.with_list(x, <[ValueId]>::to_vec) {
        Some(xs) => xs
            .into_iter()
            .fold(ValueId::NIL, |a, y| v.seq_concat(a, v.seq_unit(y))),
        None if x == ValueId::UNIT => ValueId::NIL,
        None => x,
    }
}

impl phitex_defuse::Monoid for ListSeqMonoid {
    fn empty(&self, _: &Values) -> ValueId {
        ValueId::NIL
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.seq_concat(as_seq(v, a), as_seq(v, b))
    }
}

/// The elements of a [`REALPAGES`] fold, in order.
pub(crate) fn real_pages(v: &Values, x: ValueId) -> Vec<ValueId> {
    v.seq_to_vec(as_seq(v, x))
}

/// A finisher's sealed lines: the view of each piece's unit's items the
/// run read by its shape, and the views of the line boxes made from them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sealing {
    units: crate::FxHashMap<i64, View>,
    lines: crate::FxHashMap<(i64, u32), View>,
}

impl Sealing {
    pub(crate) fn is_empty(&self) -> bool {
        self.units.is_empty()
    }
}

impl Run<'_, '_> {
    /// Piece `piece`'s unit read by its shape: `items` the view of its
    /// items, for its sealed lines.
    pub(crate) fn seal_unit(&mut self, piece: i64, items: View) {
        self.sealed.units.insert(piece, items);
    }

    /// The view of sealed line `s`'s box in its unit's items.
    fn sealed_line(&mut self, s: nodes::Sealed) -> Option<View> {
        if let Some(&l) = self.sealed.lines.get(&(s.piece, s.k)) {
            return Some(l);
        }
        let items = *self.sealed.units.get(&s.piece)?;
        let item = self.cx.at(items, Step::Index(s.k))?;
        let l = self.cx.at(item, Step::Index(1))?;
        self.sealed.lines.insert((s.piece, s.k), l);
        Some(l)
    }

    /// `x` with each sealed line in it given its glue setting and list
    /// (read now: the run depends on them), as a value leaving the run.
    pub(crate) fn unseal(&mut self, x: ValueId) -> ValueId {
        if self.sealed.is_empty() {
            return x;
        }
        let mut memo = crate::FxHashMap::default();
        self.unseal_in(x, &mut memo)
    }

    fn unseal_in(&mut self, x: ValueId, memo: &mut crate::FxHashMap<ValueId, ValueId>) -> ValueId {
        if let Some(&y) = memo.get(&x) {
            return y;
        }
        let v = self.v;
        let Some(xs) = v.with_list(x, <[ValueId]>::to_vec) else {
            return x;
        };
        let y = if xs.len() == 10
            && v.as_int(xs[0]) == Some(nodes::T_BOX)
            && let Some(s) = nodes::sealed_of(v, xs[9])
        {
            let line = self.sealed_line(s).map(|l| self.cx.whole(l));
            match line.and_then(|l| v.with_list(l, <[ValueId]>::to_vec)) {
                Some(real) if real.len() == 10 => {
                    let mut ys = xs;
                    ys[6..10].copy_from_slice(&real[6..10]);
                    v.list(ys)
                }
                _ => x,
            }
        } else {
            let mut changed = false;
            let ys: Vec<ValueId> = xs
                .iter()
                .map(|&e| {
                    let f = self.unseal_in(e, memo);
                    changed |= f != e;
                    f
                })
                .collect();
            if changed { v.list(ys) } else { x }
        };
        memo.insert(x, y);
        y
    }

    /// Whether `x` holds a sealed line.
    fn holds_sealed(&self, x: ValueId, memo: &mut crate::FxHashMap<ValueId, bool>) -> bool {
        if let Some(&b) = memo.get(&x) {
            return b;
        }
        let v = self.v;
        let b = match v.with_list(x, <[ValueId]>::to_vec) {
            None => false,
            Some(xs) if xs.len() == 5 && v.as_int(xs[0]) == Some(nodes::T_SEALED) => true,
            Some(xs) => xs.into_iter().any(|e| self.holds_sealed(e, memo)),
        };
        memo.insert(x, b);
        b
    }

    /// Box value `x` as a composition: its sealed lines passed on from the
    /// window (a line's box whole where its head is as its piece made it,
    /// else its glue setting and list), the rest its own.
    fn real_part(&mut self, x: ValueId, memo: &mut crate::FxHashMap<ValueId, bool>) -> Part {
        let v = self.v;
        if !self.holds_sealed(x, memo) {
            return Part::Val(x);
        }
        let xs = list_of(v, x);
        if xs.len() == 10
            && v.as_int(xs[0]) == Some(nodes::T_BOX)
            && let Some(s) = nodes::sealed_of(v, xs[9])
        {
            let line = self.sealed_line(s);
            if let Some(l) = line
                && nodes::head_hash(&xs[..6]) == s.head
            {
                return Part::View(l);
            }
            let mut parts: Vec<Part> = xs[..6].iter().map(|&e| Part::Val(e)).collect();
            for j in 6..10 {
                match line.and_then(|l| self.cx.at(l, Step::Index(j))) {
                    Some(p) => parts.push(Part::View(p)),
                    None => parts.push(Part::Val(xs[j as usize])),
                }
            }
            return Part::View(self.cx.compose(&parts));
        }
        let parts: Vec<Part> = xs.into_iter().map(|e| self.real_part(e, memo)).collect();
        Part::View(self.cx.compose(&parts))
    }

    /// The pages this run shipped ([`Run::real_pages`], their boxes as the
    /// run has them) contributed to [`REALPAGES`] with their sealed
    /// lines passed on: one contribution, the list of the boxes.
    pub(crate) fn contribute_real_pages(&mut self) {
        if self.real_pages.is_empty() {
            return;
        }
        let v = self.v;
        let pages = std::mem::take(&mut self.real_pages);
        let mut memo = crate::FxHashMap::default();
        let parts: Vec<Part> = pages.into_iter().map(|b| self.real_part(b, &mut memo)).collect();
        let c = self.cx.compose(&parts);
        self.cx.contribute_from(v.field(REALPAGES), c);
    }
}
