//! Values (`DESIGN.md` §7.17.1): immutable, persistent, versioned by
//! content. A version is computed when a value is made and cached, so
//! reading it is O(1); a composite's version is made from its parts'.

use crate::hash::Version;

/// A value of the build's state.
pub trait Value: Clone {
    /// The content version, cached when the value was made.
    fn version(&self) -> Version;

    /// The subtree at `field`: a field-level read records this subtree's
    /// version, not the whole value's (7.17.1's value tree).
    fn field(&self, field: u32) -> Option<Self> {
        let _ = field;
        None
    }
}

/// The version of an optional value ([`Version::ABSENT`] for `None`).
pub fn version_opt<V: Value>(v: Option<&V>) -> Version {
    v.map_or(Version::ABSENT, Value::version)
}
