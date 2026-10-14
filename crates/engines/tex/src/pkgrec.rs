//! Package loads' records: what a load (`\documentclass`, `\usepackage`,
//! `\RequirePackage`, `\LoadClass` with its arguments, one document
//! node) defined and read when it last ran, kept machine-wide beside the
//! format and given to the same load in any later build as its declared
//! hint, so loads that touch disjoint fields run side by side.
//!
//! A record is keyed by the load, never by the document: the command
//! as written (its kind, options and names, blanks dropped) and every
//! file its run read, each with the hash of its text (or absent). A key
//! that no longer matches (a file changed, appeared or went) leaves the
//! load unbounded and the next run records it anew. A record is a hint:
//! a run that defines outside it is the engine's undeclared-definition
//! refutation (the node then may define anything, its readers re-run)
//! and the run's own sets replace the record. A load never seen gets
//! [`crate::hints::optimistic_hint`] under speculation.

use std::sync::atomic::Ordering;
use std::sync::{Mutex, PoisonError};

use phitex_defuse::{FxHashMap, ValueId, Values};

use crate::val::{int_of, list_of, str_of};

/// The commands whose slice is a package load.
const LOADERS: [&str; 4] = ["documentclass", "usepackage", "RequirePackage", "LoadClass"];

/// One load's record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rec {
    /// Every file the run read, in order, with its text's hash (0:
    /// absent).
    pub files: Vec<(String, u64)>,
    /// The fields it defined.
    pub writes: Vec<String>,
    /// The fields it read from outside its run.
    pub reads: Vec<String>,
    /// Its run opened and closed no scope, and took no sibling.
    pub no_scope: bool,
    pub claim0: bool,
}

/// Records by load key ([`load_key`]).
pub(crate) type Recs = FxHashMap<String, Rec>;

/// The key of a slice that is a package load: its text up to the end
/// of its arguments, blanks and comment lines dropped; `None` for any
/// other slice.
pub(crate) fn load_key(text: &str) -> Option<String> {
    let mut body = String::new();
    for line in text.lines() {
        let t = line.trim_start();
        if body.is_empty() && (t.is_empty() || t.starts_with('%')) {
            continue;
        }
        // (a comment ends the line's text)
        let t = match t.find('%') {
            Some(i) if i == 0 || t.as_bytes()[i - 1] != b'\\' => &t[..i],
            _ => t,
        };
        body.extend(t.chars().filter(|c| !c.is_ascii_whitespace()));
    }
    let rest = body.strip_prefix('\\')?;
    let name_len = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
    LOADERS.contains(&&rest[..name_len]).then_some(body)
}

/// Whether a node's arguments are a top-level document slice that is a
/// package load (its first frame the slice's text).
pub(crate) fn is_load(v: &Values, args: &[ValueId]) -> bool {
    if args.get(1).is_none_or(|&t| int_of(v, t) != 3) {
        return false;
    }
    let Some(&fr) = args.first() else {
        return false;
    };
    let Some(&f) = list_of(v, fr).first() else {
        return false;
    };
    let xs = list_of(v, f);
    if xs.first().map(|&t| str_of(v, t)).as_deref() != Some("s") {
        return false;
    }
    xs.get(1)
        .is_some_and(|&t| load_key(&str_of(v, t)).is_some())
}

/// Whether a name can go into a hint's encoding (its separators are
/// control characters no TeX name of a package uses).
fn encodable(n: &str) -> bool {
    !n.bytes().any(|b| (0x1d..=0x1f).contains(&b))
}

impl Rec {
    /// The hint the record gives its load, when every name encodes.
    pub(crate) fn hint(&self) -> Option<String> {
        (self.writes.iter().chain(&self.reads).all(|n| encodable(n))).then(|| {
            // (`PHITEX_PKG_NOREADS=1`: the definitions only, for comparison)
            let reads: &[String] = if crate::exec::env_flag!("PHITEX_PKG_NOREADS") {
                &[]
            } else {
                &self.reads
            };
            crate::hints::record_hint(&self.writes, reads, self.no_scope, self.claim0)
        })
    }

    /// Whether the files it read are as `now` says (a text's hash, 0
    /// absent, `None` unknown: no match).
    pub(crate) fn matches(&self, now: impl Fn(&str) -> Option<u64>) -> bool {
        self.files.iter().all(|(n, h)| now(n) == Some(*h))
    }
}

/// The entry a load's run contributes to `pkgrecs`: `[node, files,
/// writes, reads, flags]`, files `[name, hash]` (hash 0: absent), flags
/// 1 no scope, 2 no sibling taken.
pub(crate) fn entry(
    v: &Values,
    node: u64,
    files: &[(String, u64)],
    writes: &[String],
    reads: &[String],
    flags: i64,
) -> ValueId {
    let fs = files
        .iter()
        .map(|(n, h)| v.list(vec![v.str(n), crate::val::int_id(v, h.cast_signed())]))
        .collect();
    let strs = |xs: &[String]| v.list(xs.iter().map(|x| v.str(x)).collect());
    v.list(vec![
        crate::val::int_id(v, node.cast_signed()),
        v.list(fs),
        strs(writes),
        strs(reads),
        v.int(flags),
    ])
}

/// An entry decoded: its node and record.
pub(crate) fn decode_entry(v: &Values, e: ValueId) -> Option<(u64, Rec)> {
    let xs = list_of(v, e);
    let node = crate::val::int_at(v, e, 0).cast_unsigned();
    let files = list_of(v, *xs.get(1)?)
        .into_iter()
        .map(|f| {
            (
                str_of(v, list_of(v, f)[0]),
                crate::val::int_at(v, f, 1).cast_unsigned(),
            )
        })
        .collect();
    let strs = |x: ValueId| list_of(v, x).into_iter().map(|s| str_of(v, s)).collect();
    let flags = int_of(v, *xs.get(4)?);
    Some((
        node,
        Rec {
            files,
            writes: strs(*xs.get(2)?),
            reads: strs(*xs.get(3)?),
            no_scope: flags & 1 != 0,
            claim0: flags & 2 != 0,
        },
    ))
}

/// The records a build gives its loads, the files' hashes as the build
/// finds them now, and whether never-seen loads get the optimistic hint.
pub(crate) struct LoadHints {
    pub recs: Mutex<Recs>,
    finder: crate::api::Finder,
    /// The hash of each file's text as found (0: absent), asked once.
    now: Mutex<FxHashMap<String, u64>>,
    pub optimistic: bool,
    /// Loads given a record, given the optimistic hint (counts).
    pub given: [std::sync::atomic::AtomicU64; 2],
}

impl LoadHints {
    pub(crate) fn new(recs: Recs, finder: crate::api::Finder, optimistic: bool) -> LoadHints {
        LoadHints {
            recs: Mutex::new(recs),
            finder,
            now: Mutex::new(FxHashMap::default()),
            optimistic,
            given: Default::default(),
        }
    }

    /// The hash of file `name`'s text as the build's finder gives it
    /// now: the id its text value has (as a run's `fread` records it), 0
    /// when absent.
    fn hash_now(&self, v: &Values, name: &str) -> u64 {
        if let Some(&h) = self
            .now
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
        {
            return h;
        }
        let h = (self.finder)(name).map_or(0, |(_, bytes)| {
            let text: String = bytes.iter().map(|&b| char::from(b)).collect();
            v.str(&text).raw()
        });
        self.now
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_owned(), h);
        h
    }

    /// File `name` changed: asked again next time.
    pub(crate) fn forget(&self, name: &str) {
        self.now
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name);
    }

    /// Whether the record of the load `text` lists file `name`.
    pub(crate) fn reads_file(&self, text: &str, name: &str) -> bool {
        load_key(text).is_some_and(|k| {
            self.recs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&k)
                .is_some_and(|r| r.files.iter().any(|(n, _)| n == name))
        })
    }

    /// The hint for the slice `text` when it is a package load: its
    /// record's when the key matches, else the optimistic one (when on);
    /// `None`: the text's own hint.
    pub(crate) fn hint_for(&self, v: &Values, text: &str) -> Option<String> {
        let k = load_key(text)?;
        let rec = self
            .recs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&k)
            .cloned();
        if crate::exec::env_flag!("PHITEX_PKG_TRACE") {
            match &rec {
                None => eprintln!("pkgrec: {k}: no record"),
                Some(r) => {
                    let bad: Vec<String> = r
                        .files
                        .iter()
                        .filter(|(n, h)| self.hash_now(v, n) != *h)
                        .map(|(n, h)| {
                            format!("{n} (recorded {h:x}, now {:x})", self.hash_now(v, n))
                        })
                        .collect();
                    eprintln!(
                        "pkgrec: {k}: record of {} files, {} writes, {} reads; differing {bad:?}",
                        r.files.len(),
                        r.writes.len(),
                        r.reads.len()
                    );
                }
            }
        }
        if let Some(r) = rec
            && r.matches(|n| Some(self.hash_now(v, n)))
            && let Some(h) = r.hint()
        {
            self.given[0].fetch_add(1, Ordering::Relaxed);
            return Some(h);
        }
        if self.optimistic {
            self.given[1].fetch_add(1, Ordering::Relaxed);
            return Some(crate::hints::optimistic_hint());
        }
        None
    }

    /// The records with `fresh` (load key, record) replacing theirs.
    pub(crate) fn merge(&self, fresh: Vec<(String, Rec)>) {
        let mut r = self.recs.lock().unwrap_or_else(PoisonError::into_inner);
        for (k, x) in fresh {
            r.insert(k, x);
        }
    }

    pub(crate) fn bytes(&self) -> Vec<u8> {
        serialise(&self.recs.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The records in `bytes` ([`crate::Build::export_pkgrec`]), for
/// inspection: per load key, the fields it defined and read.
#[doc(hidden)]
#[must_use]
pub fn summary(bytes: &[u8]) -> Vec<(String, Vec<String>, Vec<String>)> {
    let mut out: Vec<_> = parse(bytes)
        .into_iter()
        .map(|(k, r)| (k, r.writes, r.reads))
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------
// Bytes: a magic, then per record its key, files, writes, reads, flags;
// strings and counts length-prefixed (u32 little-endian).

const MAGIC: &[u8; 8] = b"phipkr01";

fn put(o: &mut Vec<u8>, b: &[u8]) {
    o.extend_from_slice(&u32::try_from(b.len()).unwrap_or(0).to_le_bytes());
    o.extend_from_slice(b);
}

fn put_n(o: &mut Vec<u8>, n: usize) {
    o.extend_from_slice(&u32::try_from(n).unwrap_or(0).to_le_bytes());
}

/// The records as bytes, in key order (equal records, equal bytes).
pub(crate) fn serialise(recs: &Recs) -> Vec<u8> {
    let mut keys: Vec<&String> = recs.keys().collect();
    keys.sort();
    let mut o = MAGIC.to_vec();
    put_n(&mut o, keys.len());
    for k in keys {
        let r = &recs[k];
        put(&mut o, k.as_bytes());
        put_n(&mut o, r.files.len());
        for (n, h) in &r.files {
            put(&mut o, n.as_bytes());
            o.extend_from_slice(&h.to_le_bytes());
        }
        for xs in [&r.writes, &r.reads] {
            put_n(&mut o, xs.len());
            for x in xs {
                put(&mut o, x.as_bytes());
            }
        }
        o.push(u8::from(r.no_scope) | (u8::from(r.claim0) << 1));
    }
    o
}

struct Rd<'a>(&'a [u8]);

impl Rd<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let (a, b) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = b;
        Some(a)
    }
    fn n(&mut self) -> Option<usize> {
        let b = self.take(4)?;
        usize::try_from(u32::from_le_bytes(b.try_into().ok()?)).ok()
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn string(&mut self) -> Option<String> {
        let n = self.n()?;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn strings(&mut self) -> Option<Vec<String>> {
        let n = self.n()?;
        (0..n).map(|_| self.string()).collect()
    }
}

/// Records from bytes ([`serialise`]); none from anything else.
pub(crate) fn parse(b: &[u8]) -> Recs {
    fn go(b: &[u8]) -> Option<Recs> {
        let mut r = Rd(b.strip_prefix(MAGIC.as_slice())?);
        let n = r.n()?;
        let mut out = Recs::default();
        for _ in 0..n {
            let k = r.string()?;
            let nf = r.n()?;
            let mut files = Vec::with_capacity(nf);
            for _ in 0..nf {
                let name = r.string()?;
                files.push((name, r.u64()?));
            }
            let writes = r.strings()?;
            let reads = r.strings()?;
            let fl = r.take(1)?[0];
            out.insert(
                k,
                Rec {
                    files,
                    writes,
                    reads,
                    no_scope: fl & 1 != 0,
                    claim0: fl & 2 != 0,
                },
            );
        }
        Some(out)
    }
    go(b).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_of_loads_only() {
        assert_eq!(
            load_key("\\usepackage[utf8] {inputenc}\n\n").as_deref(),
            Some("\\usepackage[utf8]{inputenc}")
        );
        assert_eq!(
            load_key("% c\n\\documentclass{article}% x\n").as_deref(),
            Some("\\documentclass{article}")
        );
        assert_eq!(load_key("\\input foo\n"), None);
        assert_eq!(load_key("\\usepackageX{a}"), None);
        assert_eq!(load_key("Some text.\n"), None);
    }

    #[test]
    fn bytes_round_trip() {
        let mut recs = Recs::default();
        recs.insert(
            "\\usepackage{a}".to_owned(),
            Rec {
                files: vec![("a.sty".to_owned(), 7), ("a.cfg".to_owned(), 0)],
                writes: vec!["cs:a".to_owned()],
                reads: vec!["cs:b".to_owned(), "cat:64".to_owned()],
                no_scope: true,
                claim0: false,
            },
        );
        assert_eq!(parse(&serialise(&recs)), recs);
        assert!(parse(b"junk").is_empty());
    }
}
