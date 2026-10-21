//! Integer registers as folds of the writes that outlive their runs
//! (docs/note-register-folds.md, spec-page-chain §5.1).
//!
//! A write of an integer register that outlives its run, made where no
//! group of the document or of the run holds a binding of the register,
//! is a map of the register's value: `set c` (an assignment) or `add c`
//! (`\advance` by a constant). It is contributed to the register's
//! accumulator ([`acc_field`]) instead of defining the register, and it
//! reads nothing: the map does not depend on the value it applies to.
//! A reader's value is the definition reaching it (a write made inside a
//! group, or the start value) with the maps after that definition applied
//! in order. A run that read a guessed fold contributes the same maps, so
//! a cold build's readers converge in a second wave instead of waiting in
//! a chain through every writer.

use phitex_defuse::{Monoid, ValueId, Values};

use crate::val::int_of;

/// The prefix of the registers' accumulators: `acc:count:<n>`.
pub(crate) const ACC_PREFIX: &str = "acc:count:";

/// The accumulator of integer register `n`.
pub(crate) fn acc_field(v: &Values, n: i32) -> phitex_defuse::Field {
    v.field(&format!("{ACC_PREFIX}{n}"))
}

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

/// The registers' accumulators: maps composed in position order. A
/// reader may read past contributors still in work (as they last
/// contributed, or as nothing): a map never depends on the value read, so
/// a wrong guess costs the reader one more run and nothing else.
pub(crate) struct AffineMonoid;

impl Monoid for AffineMonoid {
    fn empty(&self, v: &Values) -> ValueId {
        Affine::ID.encode(v)
    }

    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        Affine::decode(v, a).then(Affine::decode(v, b)).encode(v)
    }

    fn speculate(&self) -> bool {
        true
    }
}
