//! The pieces' run times an earlier build measured, by text: a cold
//! build hints them to the engine (`Engine::set_run_time_hint`), so that
//! a long piece's first attempt does not run ahead of the pieces before
//! it on guesses (`Engine::set_long_runs`, [`crate::Options::long_runs`]).
//! Kept in the cache beside the format, as the package loads' records are.

use std::fmt::Write as _;

use phitex_defuse::FxHashMap;

/// The times kept: a run at least this long (shorter ones are never held).
pub(crate) const MIN_US: u64 = 10_000;

/// At most this many texts kept, the longest first.
const MAX: usize = 4096;

/// A piece's text's key: FNV-1a, stable across builds and binaries.
pub(crate) fn key(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The times stored: a line per text, its key in hex and microseconds.
pub(crate) fn parse(b: &[u8]) -> FxHashMap<u64, u64> {
    std::str::from_utf8(b)
        .unwrap_or("")
        .lines()
        .filter_map(|l| {
            let (k, t) = l.split_once(' ')?;
            Some((u64::from_str_radix(k, 16).ok()?, t.parse().ok()?))
        })
        .collect()
}

/// The times as [`parse`] reads them: the longest [`MAX`], by key.
pub(crate) fn serialise(m: &FxHashMap<u64, u64>) -> Vec<u8> {
    let mut xs: Vec<(u64, u64)> = m.iter().map(|(&k, &t)| (k, t)).collect();
    xs.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    xs.truncate(MAX);
    xs.sort_unstable();
    let mut out = String::new();
    for (k, t) in xs {
        let _ = writeln!(out, "{k:016x} {t}");
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let mut m = phitex_defuse::FxHashMap::default();
        m.insert(super::key("\\usepackage{hyperref}"), 140_000);
        m.insert(super::key("\\begin{document}"), 105_000);
        assert_eq!(super::parse(&super::serialise(&m)), m);
    }
}
