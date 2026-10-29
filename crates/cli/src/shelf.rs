//! The package cache: Shelf's packs and index, as `files = "shelf"`
//! fetches them (`phitex-shelf`), kept in the cache directory's `shelf/`
//! by release (`shelf/<release>/`), across runs and documents. A pack is
//! fetched once and kept; clearing the cache is safe: what a build needs
//! is fetched again.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::cache::{Cache, SHELF, Stats};

/// The Shelf release this PhiTeX builds from (its index and packs): two
/// machines with the same PhiTeX build a document from the same files.
pub const RELEASE: &str = "tl2026.6";

/// The packs read from the cache this process, and those fetched.
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// The package cache's directory: `<cache>/shelf/<release>/`.
pub fn dir() -> PathBuf {
    crate::tex::cache_dir().join(SHELF).join(RELEASE)
}

/// A pack read from the cache (`true`) or fetched.
#[allow(dead_code, reason = "the Shelf source's (files = \"shelf\"), next")]
pub fn count(hit: bool) {
    if hit {
        HITS.fetch_add(1, Ordering::Relaxed);
    } else {
        MISSES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Where the package cache is, what it holds, and its packs found and
/// fetched this process.
pub fn stats() -> (PathBuf, Stats, u64, u64) {
    let d = dir();
    let st = Cache {
        dir: d.clone(),
        limit: 1,
    }
    .stats();
    (
        d,
        st,
        HITS.load(Ordering::Relaxed),
        MISSES.load(Ordering::Relaxed),
    )
}

/// The package cache emptied (every release's): the bytes freed.
pub fn clear() -> Result<u64, String> {
    let d = crate::tex::cache_dir().join(SHELF);
    let held = Cache {
        dir: d.clone(),
        limit: 1,
    }
    .stats()
    .bytes;
    match std::fs::remove_dir_all(&d) {
        Ok(()) => Ok(held),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(format!("the package cache: {}: {e}", d.display())),
    }
}
