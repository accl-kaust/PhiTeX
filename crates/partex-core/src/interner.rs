//! The session's shared interner (DESIGN 3.17.10): the control sequences
//! a run makes, placed once for every engine view of the session, so that
//! a name has one location whichever worker made it first.
//!
//! A name the format does not have goes to the first free place of a
//! probe sequence its spelling picks (its hash code's slot, then the
//! `hash_extra` region, as `Tex::probe_names` places names). Inserting is
//! atomic per spelling (a lock per shard of spellings) and per place (a
//! compare-and-swap on the slot), so two workers that make `\foo` at once
//! get one location, and two that make colliding names get two.
//!
//! Where a name lands still depends on which colliding names came before
//! it: a name is never moved once made, since lists hold its location.
//! So no output and no version depends on a location the interner chose:
//! a token list's version counts such a name by its spelling
//! (`partex_engine::node::Spellings`), and every name is printed by its
//! characters. The decoy test (`tests/interner.rs`) makes the names of a
//! document land elsewhere and asks for the same bytes.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use std::collections::HashMap;
use std::sync::Mutex;

use partex_engine::node::Spellings;

/// Shards of the spelling map: a lock each.
const SHARDS: usize = 64;

/// A name's spelling hash (FNV-1a, 64 bits; never 0).
#[must_use]
pub fn spelling_hash(name: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in name {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h.max(1)
}

/// `n` names drawn from `seed` (`zq` and 2 to 8 letters), decoys for a
/// test of the interner (`PARTEX_SHARE_NAMES=seed`).
#[must_use]
pub fn random_names(seed: u64, n: usize) -> Vec<Vec<u8>> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let mut y = x;
            let mut name = b"zq".to_vec();
            for _ in 0..2 + y % 7 {
                y /= 7;
                #[allow(clippy::cast_possible_truncation, reason = "y % 26 < 26")]
                name.push(b'a' + (y % 26) as u8);
                y /= 26;
            }
            name
        })
        .collect()
}

/// The interner, shared by every view of a session's engine.
pub struct SharedNames {
    /// The locations the slots below start at.
    base: i32,
    /// Whether each location holds a name: the format's (from the start)
    /// or one placed here.
    taken: Box<[AtomicU8]>,
    /// Spelling -> location, by shard.
    shards: Vec<Mutex<HashMap<Box<[u8]>, i32>>>,
    /// The spellings of the names placed here, for versions.
    pub(crate) spell: Arc<Spellings>,
    /// Names placed in the `hash_extra` region, and its size.
    extra_used: AtomicUsize,
    extra_size: usize,
    /// The first location of the `hash_extra` region.
    extra_base: i32,
}

impl SharedNames {
    /// An interner over locations `base..base + taken.len()`, the ones
    /// `taken` says hold a name already.
    pub(crate) fn new(base: i32, taken: &[bool], extra_base: i32, extra_size: usize) -> Self {
        SharedNames {
            base,
            taken: taken.iter().map(|&t| AtomicU8::new(u8::from(t))).collect(),
            shards: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
            spell: Arc::new(Spellings::new(base, taken.len())),
            extra_used: AtomicUsize::new(0),
            extra_size,
            extra_base,
        }
    }

    fn shard(&self, h: u64) -> &Mutex<HashMap<Box<[u8]>, i32>> {
        #[allow(clippy::cast_possible_truncation, reason = "a shard index")]
        &self.shards[(h >> 58) as usize % SHARDS]
    }

    /// Where `name` is, if some view made it.
    #[must_use]
    pub fn find(&self, name: &[u8]) -> Option<i32> {
        let h = spelling_hash(name);
        self.shard(h)
            .lock()
            .expect("the interner")
            .get(name)
            .copied()
    }

    /// Where `name` is: found, or placed at the first free place of
    /// `places` (`None`: none is free, or the `hash_extra` region is
    /// full).
    pub fn insert(&self, name: &[u8], places: impl IntoIterator<Item = i32>) -> Option<i32> {
        let h = spelling_hash(name);
        let mut m = self.shard(h).lock().expect("the interner");
        if let Some(&p) = m.get(name) {
            return Some(p);
        }
        for q in places {
            let Some(t) = usize::try_from(q - self.base)
                .ok()
                .and_then(|i| self.taken.get(i))
            else {
                continue;
            };
            if t.load(Ordering::Acquire) != 0 {
                continue;
            }
            let extra = q >= self.extra_base;
            if extra && self.extra_used.load(Ordering::Acquire) >= self.extra_size {
                return None;
            }
            if t.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if extra {
                    self.extra_used.fetch_add(1, Ordering::AcqRel);
                }
                self.spell.set(q, h);
                m.insert(name.into(), q);
                return Some(q);
            }
        }
        None
    }

    /// Names placed so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|s| s.lock().expect("the interner").len())
            .sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Token-list versions made on this thread count the names placed
    /// here by their spellings (call it on every thread an engine view of
    /// the session runs on).
    pub fn enter(&self) {
        partex_engine::node::set_spellings(Some(self.spell.clone()));
    }

    /// This thread's versions count every token by its value again.
    pub fn leave() {
        partex_engine::node::set_spellings(None);
    }
}
