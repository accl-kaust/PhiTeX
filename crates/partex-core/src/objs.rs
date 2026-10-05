//! The values eqtb entries hold beyond their word (DESIGN 7.17.12,
//! `objs`): token lists (a macro's body, a token register or
//! parameter), glue specifications (`glue_ref`), boxes (`box_ref`) and
//! paragraph shapes (`shape_ref`).
//!
//! tex.web keeps them in `mem` and puts pointers in eqtb. Here an entry
//! holds the value itself, an immutable shared value carrying its
//! version, never an address of its own: eqtb's words and the save
//! stack's copies of them keep tex.web's layout for their scalar part,
//! and beside each word is the object it names ([`Obj`]). `eq_destroy`
//! (§275) is the drop of the old value when the entry is overwritten or
//! restored. Only glue's identity is observable in TeX (e-TeX's
//! `\tracingassigns` tells whether a register is assigned the very glue
//! it has, and `zero_glue`'s), so a glue value carries a lineage as data:
//! glue copied unchanged keeps it, new glue gets a new one.

use alloc::sync::Arc;

use partex_engine::Scaled;
use partex_engine::node::{BoxNode, GlueSpec, Tokens};

/// A glue value: the specification and its lineage (0 for `zero_glue`).
#[derive(Clone, Copy, Debug, PartialEq, Hash)]
pub(crate) struct Glue {
    pub(crate) spec: GlueSpec,
    pub(crate) lineage: u64,
}

partex_engine::persist_struct!(Glue { spec, lineage });

impl Glue {
    /// `zero_glue` (§162), shared.
    pub(crate) const ZERO: Glue = Glue {
        spec: GlueSpec::ZERO_GLUE,
        lineage: 0,
    };
}

/// The value an eqtb entry holds beyond its word.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Obj {
    /// A macro's body or a token register's or parameter's list.
    Toks(Tokens),
    Glue(Glue),
    Shape(Arc<Shaped>),
    Box(Arc<BoxNode>),
}

partex_engine::persist_enum!(Obj {
    Toks(a0),
    Glue(a0),
    Shape(a0),
    Box(a0)
});

/// eqtb's objects by chunks of this many (the default, as measured on
/// the course; the objects a region writes are scattered over the hash's
/// places, so a chunk written holds few of them and is copied whole:
/// smaller chunks, 128, are expected to cut those copies, not measured).
pub(crate) const OBJ_CHUNK: usize = crate::journal::CHUNK;

/// eqtb's objects in a [`crate::journal::JVec`]: an entry written back
/// as it was holds the very value it held (glue by its content).
impl crate::journal::Elem for Option<Obj> {
    #[inline]
    fn same(a: &Self, b: &Self) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(Obj::Toks(x)), Some(Obj::Toks(y))) => Arc::ptr_eq(x, y),
            (Some(Obj::Glue(x)), Some(Obj::Glue(y))) => x == y,
            (Some(Obj::Shape(x)), Some(Obj::Shape(y))) => Arc::ptr_eq(x, y),
            (Some(Obj::Box(x)), Some(Obj::Box(y))) => Arc::ptr_eq(x, y),
            _ => false,
        }
    }
}

impl Obj {
    /// The object's version (DESIGN 7.17.12): a list's, a box's, made
    /// when each was made; glue and shapes by their content (a few words).
    #[must_use]
    pub(crate) fn version(&self) -> u128 {
        match self {
            Obj::Toks(t) => t.version(),
            Obj::Box(b) => b.ver,
            Obj::Glue(g) => partex_ssa::Version::of(g).0,
            Obj::Shape(s) => partex_ssa::Version::of(&**s).0,
        }
    }

    /// The version made again from the content (check mode's test).
    #[must_use]
    pub(crate) fn version_by_content(&self) -> u128 {
        match self {
            Obj::Toks(t) => t.version_by_tokens(),
            Obj::Box(b) => b.parts_version(),
            Obj::Glue(_) | Obj::Shape(_) => self.version(),
        }
    }

    /// The token list, if the object is one.
    pub(crate) fn toks(&self) -> Option<&Tokens> {
        match self {
            Obj::Toks(t) => Some(t),
            _ => None,
        }
    }
}

/// §1070: a `\parshape`: (indentation, length) per line.
pub(crate) type Shape = Arc<[(Scaled, Scaled)]>;

/// What a `shape_ref` names: a `\parshape`, or one of e-TeX's penalty
/// arrays (`\interlinepenalties` and the like).
#[derive(Clone, Debug, PartialEq, Hash)]
pub(crate) enum Shaped {
    Lines(Shape),
    Penalties(Arc<[i32]>),
}

partex_engine::persist_enum!(Shaped { Lines(a0), Penalties(a0) });

impl<H: crate::host::Host, T: crate::track::Tracker> crate::tex::Tex<H, T> {
    /// What `\tracingstats` reports as `var_used` and `dyn_used` (§639,
    /// §1311): tex.web's numbers count `mem` words, which this
    /// implementation does not have; the comparisons mask them.
    #[allow(clippy::unused_self, reason = "the statistics are not emulated")]
    pub(crate) fn memory_usage(&self) -> (i32, i32) {
        (0, 0)
    }
}
