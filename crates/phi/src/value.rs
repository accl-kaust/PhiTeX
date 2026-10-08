//! Values (DESIGN 7.2): immutable, versioned by content, with fields.

use crate::ver::Ver;

/// A value of the graph. Its version is its content's: two values with
/// equal versions are equal. A composite caches its version when made,
/// so [`Value::ver`] is O(1).
pub trait Value: Clone + Send + Sync + 'static {
    /// The content version.
    fn ver(&self) -> Ver;

    /// Field `f`, a part with a version of its own: a reader of a field
    /// is woken only when that field's version changes.
    fn field(&self, f: u32) -> Option<Self> {
        let _ = f;
        None
    }

    /// The version of field `f` ([`Ver::ABSENT`] if there is none).
    fn field_ver(&self, f: u32) -> Ver {
        self.field(f).map_or(Ver::ABSENT, |v| v.ver())
    }

    /// The bytes this value holds (for the state-size report and the
    /// memo's budget): its own size by default.
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

/// What an operand reads of a value: the whole of it, a field, or a
/// field of a field. The second level is a reader's own, on top of what a
/// name's definition selects (`Arg::NameField`): low 16 bits the first
/// field (`0xffff` none), the next 15 the reader's field plus one (0
/// none).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Sel(pub u32);

impl Sel {
    pub const WHOLE: Sel = Sel(u32::MAX);
    const NOFIRST: u32 = 0xffff;

    /// Field `f` (below 2^16 - 1).
    #[must_use]
    pub fn field(f: u32) -> Sel {
        debug_assert!(f < Self::NOFIRST, "fields below 2^16 - 1");
        Sel(f)
    }

    #[must_use]
    pub fn is_whole(self) -> bool {
        self == Sel::WHOLE
    }

    /// The first level's field, if any.
    #[must_use]
    pub fn first(self) -> Option<u32> {
        let f = self.0 & 0xffff;
        (!self.is_whole() && f != Self::NOFIRST).then_some(f)
    }

    /// The reader's own field on top, if any.
    #[must_use]
    pub fn extra(self) -> Option<u32> {
        let x = (self.0 >> 16) & 0x7fff;
        (!self.is_whole() && x != 0).then(|| x - 1)
    }

    /// What a definition selects (`self`, one level), with a reader's
    /// own field `x` on top.
    #[must_use]
    pub fn then(self, x: Option<u32>) -> Sel {
        let Some(x) = x else {
            return self;
        };
        debug_assert!(x < 0x7ffe, "fields below 2^15 - 2");
        debug_assert!(self.extra().is_none(), "two levels at most");
        let first = self.first().unwrap_or(Self::NOFIRST);
        Sel(((x + 1) << 16) | first)
    }

    /// The version of what this reads of `v`.
    #[inline]
    pub fn ver<V: Value>(self, v: &V) -> Ver {
        if self.is_whole() {
            return v.ver();
        }
        match (self.first(), self.extra()) {
            (Some(f), None) | (None, Some(f)) => v.field_ver(f),
            (Some(f), Some(x)) => v.field(f).map_or(Ver::ABSENT, |y| y.field_ver(x)),
            (None, None) => v.ver(),
        }
    }
}

/// A value read through a [`Sel`]: the value itself, or a field made
/// from it.
pub enum Proj<'a, V> {
    Ref(&'a V),
    Owned(V),
}

impl<V> std::ops::Deref for Proj<'_, V> {
    type Target = V;
    fn deref(&self) -> &V {
        match self {
            Proj::Ref(v) => v,
            Proj::Owned(v) => v,
        }
    }
}

/// `v` read through `sel` (`absent` where the field does not exist).
#[inline]
pub fn project<'a, V: Value>(v: &'a V, sel: Sel, absent: &'a V) -> Proj<'a, V> {
    if sel.is_whole() {
        return Proj::Ref(v);
    }
    let got = match (sel.first(), sel.extra()) {
        (Some(f), None) | (None, Some(f)) => v.field(f),
        (Some(f), Some(x)) => v.field(f).and_then(|y| y.field(x)),
        (None, None) => return Proj::Ref(v),
    };
    match got {
        Some(f) => Proj::Owned(f),
        None => Proj::Ref(absent),
    }
}
