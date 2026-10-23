//! The fields a long piece is known to define last before its readers
//! (`Engine::wait_on_pending`): learned from an earlier build, by the
//! piece's text, so a later build's readers of those fields wait for the
//! piece while it runs instead of guessing past it ([`crate::Options::wait_fields`]).
//!
//! A field is learned for a long piece (its run longer than
//! [`crate::Options::long_runs`]) that defines it when no piece after it
//! does, and its readers were refuted on it at least [`MIN_REFUTED`] times
//! (or a tenth of the refuted nodes, the larger) in that build. A learned
//! field stays while the piece still defines it and no later piece does:
//! the readers that waited are not refuted again, which is not a reason to
//! forget it. Speed only: the output is the same either way.

use phitex_defuse::FxHashMap;
use std::fmt::Write as _;

/// The refutations a field needs to be learned.
pub(crate) const MIN_REFUTED: u64 = 20;

/// Per piece key ([`crate::runtimes::key`]): the fields its readers wait on.
pub(crate) type WaitFields = FxHashMap<u64, Vec<String>>;

/// The fields as [`serialise`] wrote them: a line per piece, its key in
/// hex, then its fields, tab-separated.
pub(crate) fn parse(b: &[u8]) -> WaitFields {
    std::str::from_utf8(b)
        .unwrap_or("")
        .lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let k = u64::from_str_radix(it.next()?, 16).ok()?;
            let fs: Vec<String> = it.filter(|f| !f.is_empty()).map(str::to_owned).collect();
            (!fs.is_empty()).then_some((k, fs))
        })
        .collect()
}

/// The fields as [`parse`] reads them, by key.
pub(crate) fn serialise(m: &WaitFields) -> Vec<u8> {
    let mut ks: Vec<&u64> = m.keys().collect();
    ks.sort_unstable();
    let mut out = String::new();
    for k in ks {
        let mut fs = m[k].clone();
        fs.sort_unstable();
        let _ = writeln!(out, "{k:016x}\t{}", fs.join("\t"));
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let mut m = super::WaitFields::default();
        m.insert(3, vec!["cattab".to_owned(), "parpar".to_owned()]);
        assert_eq!(super::parse(&super::serialise(&m)), m);
    }
}
