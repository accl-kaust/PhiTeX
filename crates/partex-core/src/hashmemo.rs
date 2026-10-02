//! Sub-hashes of shared, immutable pieces of engine state, memoized by
//! the `Arc` that holds them (DESIGN.md §5.3, "an incremental state
//! hash").
//!
//! The state hash of a machine region's start (`machine.rs`) covers
//! megabytes that rarely change: font metrics, hyphenation patterns, the
//! string pool, the hash table. They live in shared chunks (`JVec`,
//! `Flat`, `Arc<Font>`): a chunk that did not change since the last
//! snapshot is the same `Arc`. So a hash is a tree: the state's hash
//! combines each chunk's own hash, and a chunk's hash is looked up by
//! its address instead of computed again. The memo keeps every `Arc` it
//! answers for alive, so an address is never reused while its entry
//! exists; entries no pass used lately are dropped.
//!
//! A sub-hash is a pure function of the chunk's contents, so a hash
//! computed with the memo equals one computed without it (the fallback
//! when there is none, or when a chunk is not shared yet).

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

/// One kind of memoized piece.
pub(crate) struct Memo<T: ?Sized> {
    map: BTreeMap<usize, (Arc<T>, u128, u32)>,
}

impl<T: ?Sized> Default for Memo<T> {
    fn default() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }
}

impl<T: ?Sized> Clone for Memo<T> {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
}

fn key<T: ?Sized>(a: &Arc<T>) -> usize {
    Arc::as_ptr(a).cast::<()>().addr()
}

impl<T: ?Sized> Memo<T> {
    /// The hash of `a`'s contents: `f` of them, unless known.
    pub(crate) fn get_or(&mut self, pass: u32, a: &Arc<T>, f: impl FnOnce(&T) -> u128) -> u128 {
        let k = key(a);
        if let Some(e) = self.map.get_mut(&k)
            && Arc::ptr_eq(&e.0, a)
        {
            e.2 = pass;
            return e.1;
        }
        let h = f(a);
        self.map.insert(k, (a.clone(), h, pass));
        h
    }

    /// Drop what no pass used since `since`, once the memo has grown.
    fn sweep(&mut self, since: u32, used: usize) {
        if self.map.len() > 2 * used + 256 {
            self.map.retain(|_, e| e.2 >= since);
        }
    }

    fn used(&self, pass: u32) -> usize {
        self.map.values().filter(|e| e.2 == pass).count()
    }
}

/// The memo of every kind of piece the state hash follows (scratch: not
/// state, and a clone shares the `Arc`s, not the engine's).
#[derive(Clone, Default)]
pub(crate) struct HashMemo {
    pub(crate) pass: u32,
    pub(crate) words: Memo<[crate::mem::MemoryWord; crate::journal::CHUNK]>,
    /// Hash-table chunks with the names made by the run by their
    /// characters (`Canon::strs`). A chunk not written since a state
    /// names only strings made before it, the same in every state that
    /// shares it.
    pub(crate) names: Memo<[crate::mem::MemoryWord; crate::journal::CHUNK]>,
    pub(crate) bytes: Memo<[u8]>,
    pub(crate) sizes: Memo<[usize]>,
    pub(crate) fonts: Memo<partex_engine::font::Font>,
    pub(crate) patterns: Memo<partex_engine::hyph::Patterns>,
    pub(crate) ints: Memo<alloc::vec::Vec<i32>>,
}

impl HashMemo {
    /// A new pass begins.
    pub(crate) fn begin(&mut self) {
        self.pass = self.pass.wrapping_add(1);
    }

    /// The pass is over: forget what the last few passes did not use.
    pub(crate) fn end(&mut self) {
        if !self.pass.is_multiple_of(64) {
            return;
        }
        let since = self.pass.saturating_sub(64);
        let p = self.pass;
        let u = self.words.used(p);
        self.words.sweep(since, u);
        let u = self.names.used(p);
        self.names.sweep(since, u);
        let u = self.bytes.used(p);
        self.bytes.sweep(since, u);
        let u = self.sizes.used(p);
        self.sizes.sweep(since, u);
        let u = self.fonts.used(p);
        self.fonts.sweep(since, u);
        let u = self.patterns.used(p);
        self.patterns.sweep(since, u);
        let u = self.ints.used(p);
        self.ints.sweep(since, u);
    }
}
