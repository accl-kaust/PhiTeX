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

use phitex_defuse::{ValueId, Values};

use crate::val::int_of;

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

    pub(crate) fn encode(self, v: &Values) -> ValueId {
        v.list(vec![v.int(i64::from(self.keep)), v.int(i64::from(self.add))])
    }

    pub(crate) fn decode(v: &Values, x: ValueId) -> Affine {
        let xs = crate::val::list_of(v, x);
        match xs[..] {
            [k, a] => Affine {
                keep: int_of(v, k) != 0,
                add: i32::try_from(int_of(v, a)).unwrap_or(0),
            },
            _ => Affine::ID,
        }
    }
}
