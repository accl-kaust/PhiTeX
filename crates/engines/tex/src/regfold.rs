//! Integer registers' writes that outlive their runs as maps of the
//! register's value.
//!
//! A write of an integer register that outlives its run, made where no
//! group of the document or of the run holds a binding of the register,
//! is a map of the register's value: `set c` (an assignment) or `add c`
//! (`\advance` by a constant). The run composes its maps and defines the
//! register at its end: `add c` as a relation on the value reaching it
//! (`Ctx::define_rel`, which the engine recomputes without the run, and
//! gives the operation's pending nodes ahead of their runs), `set c` as
//! a value. Neither reads the register: a run that never looks at the
//! value does not depend on it.

/// A map of a register's value: `x ↦ a·x + b` with `a` 0 (`set b`) or 1
/// (`add b`), the sum wrapping at 32 bits as `\advance` does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Affine {
    pub keep: bool,
    pub add: i32,
}

impl Affine {
    /// The map that changes nothing.
    pub(crate) const ID: Affine = Affine { keep: true, add: 0 };

    /// `set c`.
    pub(crate) fn set(c: i32) -> Affine {
        Affine { keep: false, add: c }
    }

    /// `add c`.
    pub(crate) fn add(c: i32) -> Affine {
        Affine { keep: true, add: c }
    }

    /// This map, then `next`.
    #[must_use]
    pub(crate) fn then(self, next: Affine) -> Affine {
        if next.keep {
            Affine {
                keep: self.keep,
                add: self.add.wrapping_add(next.add),
            }
        } else {
            next
        }
    }

    /// The value `x` maps to.
    pub(crate) fn apply(self, x: i32) -> i32 {
        if self.keep { x.wrapping_add(self.add) } else { self.add }
    }
}

/// A run's maps by register location ([`crate::run::Run::reg_pending`]):
/// a few registers a run, looked up at every write, so a list searched
/// in order (an index made past [`Maps::INDEXED`] of them).
#[derive(Default, Clone)]
pub(crate) struct Maps {
    xs: Vec<(u32, Affine)>,
    ix: phitex_defuse::FxHashMap<u32, usize>,
}

impl Maps {
    const INDEXED: usize = 32;

    fn find(&self, loc: u32) -> Option<usize> {
        if self.xs.len() > Self::INDEXED {
            self.ix.get(&loc).copied()
        } else {
            self.xs.iter().position(|x| x.0 == loc)
        }
    }

    /// `loc`'s map, if the run wrote one.
    #[inline]
    pub(crate) fn get(&self, loc: u32) -> Option<Affine> {
        self.find(loc).map(|i| self.xs[i].1)
    }

    /// Sets `loc`'s map.
    pub(crate) fn insert(&mut self, loc: u32, m: Affine) {
        if let Some(i) = self.find(loc) {
            self.xs[i].1 = m;
            return;
        }
        self.xs.push((loc, m));
        if self.xs.len() > Self::INDEXED {
            if self.ix.is_empty() {
                self.ix = self.xs.iter().enumerate().map(|(i, x)| (x.0, i)).collect();
            } else {
                self.ix.insert(loc, self.xs.len() - 1);
            }
        }
    }

    /// The maps, in the order written first.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, Affine)> + '_ {
        self.xs.iter().copied()
    }

    /// The maps, taken.
    pub(crate) fn take(&mut self) -> Vec<(u32, Affine)> {
        self.ix.clear();
        std::mem::take(&mut self.xs)
    }
}
