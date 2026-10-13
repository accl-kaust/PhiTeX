//! Per-run work counters for profiling builds (`profile` feature): tokens
//! read, node values encoded and decoded, words of encoded lists. Printed
//! with the engine's probes under `PHITEX_OBS`; without the feature every
//! probe is a no-op.

#[cfg(feature = "profile")]
pub(crate) mod on {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    pub static TOKENS: AtomicU64 = AtomicU64::new(0);
    pub static ENCODE: AtomicU64 = AtomicU64::new(0);
    pub static ENCODE_WORDS: AtomicU64 = AtomicU64::new(0);
    pub static DECODE: AtomicU64 = AtomicU64::new(0);
    pub static DECODE_LISTS: AtomicU64 = AtomicU64::new(0);

    pub fn add(c: &AtomicU64, n: u64) {
        c.fetch_add(n, Relaxed);
    }

    pub fn take() -> String {
        let g = |c: &AtomicU64| c.swap(0, Relaxed);
        format!(
            "tex counters: tokens {} encode {} encode_words {} decode {} decode_lists {}\n",
            g(&TOKENS),
            g(&ENCODE),
            g(&ENCODE_WORDS),
            g(&DECODE),
            g(&DECODE_LISTS)
        )
    }
}

/// Adds `$n` to counter `$c` (a no-op without the `profile` feature).
macro_rules! prof {
    ($c:ident, $n:expr) => {
        #[cfg(feature = "profile")]
        crate::prof::on::add(&crate::prof::on::$c, $n as u64);
    };
}
pub(crate) use prof;

/// The counters as a line, reset (empty without the `profile` feature).
pub(crate) fn take() -> String {
    #[cfg(feature = "profile")]
    return on::take();
    #[cfg(not(feature = "profile"))]
    String::new()
}
