//! Sealed lines: a finisher reads its PDF page's lines by their shape
//! (their dimensions and fonts), not by their characters. A piece gives
//! its unit's items with their shape ([`crate::nodes::unit_shape`]): each
//! line whose list a page's writer need not look into stands there as a
//! box whose list is a stand-in (`[T_SEALED, piece, k, fonts, head]`).
//! The page builder and the routine measure it, and nothing in the
//! finisher's run looks into it. A definition keeps it sealed (a box
//! register, the routine's give-back): the run that reads the register
//! measures the line as the finisher did. Where a line's list is looked
//! into (a contribution leaving the run, a box opened by `\unvbox` in
//! any run, a form), the stand-in is replaced by the line's own glue
//! setting and list, read then from its piece's unit, the finisher's
//! window's or, for a line a register carried from another finisher,
//! the piece's own ([`Run::unseal`], [`Run::unseal_open`]). The page
//! itself leaves as its shape (the `pages` entry, the page's key): its
//! writer (`crate::pdfout::PdfPage`, `crate::assemble`'s DVI pages) and
//! the link read the lines from the pieces' units, by the window's
//! places and, for lines from other finishers' pieces (given back,
//! carried in a register; a page with no window of its own), by piece
//! ([`crate::nodes::fill_sealed`]). A word that keeps its line's
//! dimensions changes the writer's input, not the finishers'.
//!
//! Example: a page of lines A, B, C; a word edited in B. The finisher's
//! reads (the window's shapes) hold; the page's PDF side, which reads
//! B's unit, and the link run again. LaTeX passes its last page through
//! a box register at `\end{document}`: the register holds the lines
//! sealed, so the next finisher, which reads it, stands as well.

use phitex_defuse::{Step, ValueId, View};

use crate::nodes;
use crate::run::Run;

/// Where a page's sealed lines' units are: the finisher's window (after
/// which node, none: the unit's start; to which piece; raw ids) and each
/// unit's place in it, by piece (sorted).
pub(crate) type Places = (Option<i64>, i64, Vec<(i64, u32)>);

/// A finisher's sealed lines: the view of each piece's unit's items the
/// run read by its shape, and the views of the line boxes made from them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sealing {
    units: crate::FxHashMap<i64, (View, u32)>,
    lines: crate::FxHashMap<(i64, u32), View>,
    /// The units of pieces outside the window that lines a definition
    /// carried here were read from (by piece; `None`: none found).
    foreign: crate::FxHashMap<i64, Option<View>>,
    /// The window the units were read in: after which node (none: the
    /// unit's start), to which piece (raw ids).
    window: Option<(Option<i64>, i64)>,
}

impl Sealing {
    pub(crate) fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// The window the sealed units were read in and each one's place in
    /// it (by piece, sorted): where the page's writer reads them
    /// (`crate::pdfout::PdfPage`).
    pub(crate) fn places(&self) -> Option<Places> {
        let (after, to) = self.window?;
        let mut at: Vec<(i64, u32)> = self.units.iter().map(|(&p, &(_, i))| (p, i)).collect();
        at.sort_unstable();
        Some((after, to, at))
    }
}

impl Run<'_, '_> {
    /// Piece `piece`'s unit read by its shape, at place `i` of the window
    /// after `after` to `to`: `items` the view of its items, for its
    /// sealed lines.
    pub(crate) fn seal_unit(
        &mut self,
        piece: i64,
        items: View,
        i: u32,
        after: Option<i64>,
        to: i64,
    ) {
        self.sealed.units.insert(piece, (items, i));
        self.sealed.window = Some((after, to));
    }

    /// The view of sealed line `s`'s box in its unit's items.
    fn sealed_line(&mut self, s: nodes::Sealed) -> Option<View> {
        if let Some(&l) = self.sealed.lines.get(&(s.piece, s.k)) {
            return Some(l);
        }
        let items = match self.sealed.units.get(&s.piece) {
            Some(&(items, _)) => items,
            None => self.foreign_unit(s.piece)?,
        };
        let item = self.cx.at(items, Step::Index(s.k))?;
        let l = self.cx.at(item, Step::Index(1))?;
        self.sealed.lines.insert((s.piece, s.k), l);
        Some(l)
    }

    /// Piece `piece`'s unit's items, a piece outside this run's window
    /// (a line a definition carried here, a box register's): the last of
    /// the units up to the piece, read by itself
    /// ([`crate::pages::unit_items_of`]).
    fn foreign_unit(&mut self, piece: i64) -> Option<View> {
        if let Some(&u) = self.sealed.foreign.get(&piece) {
            return u;
        }
        let v = self.v;
        let w = self
            .cx
            .view_through(v.field(super::PITEMS), None, super::node_of(piece))
            .ok()?;
        let u = self
            .cx
            .at(w, Step::Last)
            .filter(|&u| self.cx.get(u, Step::Index(0)).and_then(|x| v.as_int(x)) == Some(piece))
            .and_then(|u| self.cx.at(u, Step::Index(2)));
        self.sealed.foreign.insert(piece, u);
        u
    }

    /// [`Self::unseal`] in any run: a box opened (`\unvbox`) whose lines
    /// a definition carried, sealed, from a finisher.
    pub(crate) fn unseal_open(&mut self, x: ValueId) -> ValueId {
        let mut memo = crate::FxHashMap::default();
        self.unseal_in(x, &mut memo)
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
}
