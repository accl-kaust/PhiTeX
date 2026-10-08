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

/// What an operand reads of a value: the whole of it, or a field.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Sel(pub u32);

impl Sel {
    pub const WHOLE: Sel = Sel(u32::MAX);

    #[must_use]
    pub fn field(f: u32) -> Sel {
        debug_assert!(f != u32::MAX);
        Sel(f)
    }

    #[must_use]
    pub fn is_whole(self) -> bool {
        self == Sel::WHOLE
    }

    /// The version of what this reads of `v`.
    #[inline]
    pub fn ver<V: Value>(self, v: &V) -> Ver {
        if self.is_whole() {
            v.ver()
        } else {
            v.field_ver(self.0)
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
        Proj::Ref(v)
    } else {
        match v.field(sel.0) {
            Some(f) => Proj::Owned(f),
            None => Proj::Ref(absent),
        }
    }
}
