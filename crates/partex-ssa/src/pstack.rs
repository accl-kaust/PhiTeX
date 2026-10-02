//! A persistent stack: a shared linked list whose nodes carry the
//! version of the stack beneath and including them.

use alloc::sync::Arc;

use crate::hash::Version;
use crate::value::Value;

struct SNode<T> {
    val: T,
    next: Option<Arc<SNode<T>>>,
    len: usize,
    ver: Version,
}

/// A persistent stack of values.
pub struct PStack<T> {
    head: Option<Arc<SNode<T>>>,
}

impl<T> Clone for PStack<T> {
    fn clone(&self) -> Self {
        PStack {
            head: self.head.clone(),
        }
    }
}

impl<T: Value> Default for PStack<T> {
    fn default() -> Self {
        Self::new()
    }
}

const EMPTY: Version = Version(0x7374_6163_6b00_0000_0000_0000_0000_0000);

impl<T: Value> PStack<T> {
    #[must_use]
    pub fn new() -> Self {
        PStack { head: None }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.head.as_ref().map_or(0, |n| n.len)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.head.is_none()
    }

    /// The stack with `v` on top.
    #[must_use]
    pub fn push(&self, v: T) -> Self {
        let ver = Version::node(0x7075_7368, &[self.version(), v.version()]);
        PStack {
            head: Some(Arc::new(SNode {
                len: self.len() + 1,
                val: v,
                next: self.head.clone(),
                ver,
            })),
        }
    }

    /// The top and the rest.
    #[must_use]
    pub fn pop(&self) -> Option<(T, Self)> {
        let n = self.head.as_ref()?;
        Some((
            n.val.clone(),
            PStack {
                head: n.next.clone(),
            },
        ))
    }

    #[must_use]
    pub fn peek(&self) -> Option<&T> {
        self.head.as_ref().map(|n| &n.val)
    }

    #[must_use]
    pub fn version(&self) -> Version {
        self.head.as_ref().map_or(EMPTY, |n| n.ver)
    }

    /// The elements from the top down.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        let mut cur = self.head.as_deref();
        core::iter::from_fn(move || {
            let n = cur?;
            cur = n.next.as_deref();
            Some(&n.val)
        })
    }
}

impl<T: Value> Value for PStack<T> {
    fn version(&self) -> Version {
        PStack::version(self)
    }
}
