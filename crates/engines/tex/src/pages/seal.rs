//! Sealed lines: a finisher reads its page's lines by their shape (their
//! dimensions and fonts, [`crate::nodes::unit_shape`]), not by their
//! characters. Each line it reads so stands in its run as a box whose
//! list is a stand-in (`[T_SEALED, n, fonts]`, the run's `n`th sealed
//! line): the page builder and the routine measure it, and nothing in the
//! run looks into it. Where a value leaves the run (a definition, a
//! contribution, the routine's give-back), a stand-in is replaced by the
//! line's own glue setting and list, read then ([`Run::unseal`]). The
//! page itself leaves as its shape (the `pages` entry, the page's key)
//! and as the page box with its lines' lists passed on from the window,
//! not read ([`REALPAGES`], [`Run::contribute_real_pages`]): a word that
//! keeps its line's dimensions changes the page's writer's input, not
//! the finisher's.
//!
//! Example: a page of lines A, B, C; a word edited in B. The finisher's
//! reads (the window's shape) hold; the composed page box follows B's
//! list and is refiled; the page's PDF side (`crate::pdfout::PdfPage`)
//! and the link run again.

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

impl Run<'_, '_> {
    /// Unit items `xs` (its shape, read through `items`, the view of the
    /// unit's items) with each summarised line given a stand-in of this
    /// run ([`Run::sealed`]).
    pub(crate) fn seal_items(&mut self, items: View, xs: &mut [ValueId]) {
        let v = self.v;
        for (k, x) in xs.iter_mut().enumerate() {
            let Some(item) = v.with_list(*x, <[ValueId]>::to_vec) else { continue };
            if item.len() != 5 || v.as_int(item[0]) != Some(nodes::T_VBOX) {
                continue;
            }
            let Some(mut b) = v.with_list(item[1], <[ValueId]>::to_vec) else { continue };
            let Some(fonts) = b.get(9).and_then(|&l| nodes::summary_fonts(v, l)) else {
                continue;
            };
            let n = u32::try_from(self.sealed.len()).unwrap_or(u32::MAX);
            self.sealed.push((items, u32::try_from(k).unwrap_or(u32::MAX)));
            b[9] = nodes::sealed(v, n, fonts);
            let mut it = item;
            it[1] = v.list(b);
            *x = v.list(it);
        }
    }

    /// The view of sealed line `n`'s box in its unit's items.
    fn sealed_line(&mut self, n: u32) -> Option<View> {
        let (items, k) = *self.sealed.get(n as usize)?;
        let item = self.cx.at(items, Step::Index(k))?;
        self.cx.at(item, Step::Index(1))
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
            && let Some((n, _)) = nodes::sealed_of(v, xs[9])
        {
            let line = self.sealed_line(n).map(|l| self.cx.whole(l));
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
            Some(xs) if xs.len() == 3 && v.as_int(xs[0]) == Some(nodes::T_SEALED) => true,
            Some(xs) => xs.into_iter().any(|e| self.holds_sealed(e, memo)),
        };
        memo.insert(x, b);
        b
    }

    /// Box value `x` as a composition: its sealed lines' glue settings
    /// and lists passed on from the window, the rest its own.
    fn real_part(&mut self, x: ValueId, memo: &mut crate::FxHashMap<ValueId, bool>) -> Part {
        let v = self.v;
        if !self.holds_sealed(x, memo) {
            return Part::Val(x);
        }
        let xs = list_of(v, x);
        if xs.len() == 10
            && v.as_int(xs[0]) == Some(nodes::T_BOX)
            && let Some((n, _)) = nodes::sealed_of(v, xs[9])
        {
            let mut parts: Vec<Part> = xs[..6].iter().map(|&e| Part::Val(e)).collect();
            let line = self.sealed_line(n);
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
    /// lines' lists passed on: one contribution, the list of the boxes.
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
