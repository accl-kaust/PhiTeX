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
//! a run that defines outside it refutes it (a guess the engine widens:
//! the node then may define anything, its readers re-run)
//! and the run's own sets replace the record. A load never seen gets
//! [`crate::hints::optimistic_hint`] under speculation.
//!
//! Every other document node whose text alone cannot bound what it
//! defines (its static hint answers `Any`: a main-file slice, a region
//! of a file `\input` reads) is recorded the same way, keyed by its
//! text ([`node_key`], `n:` and its hash), kept per document: a later
//! build gives it its record's hint and predicts its outputs from its
//! record's values.

use crate::exec::LockWait as _;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

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
    /// The values it published: per field, its
    /// name, whether global, and the value as bytes
    /// ([`crate::api::tree_bytes`]); the prediction of its outputs when
    /// its key matches again.
    pub values: RecValues,
}

/// A record's published values: per field its name, whether global,
/// and the value as bytes.
pub(crate) type RecValues = Vec<(String, bool, Vec<u8>)>;

/// Records by key ([`node_key`]).
pub(crate) type Recs = FxHashMap<String, Rec>;

/// The key of a slice that is a package load: its text up to the end
/// of its arguments, blanks and comment lines dropped; `None` for any
/// other slice.
pub(crate) fn load_key(text: &str) -> Option<String> {
    // (the body's head first: a slice that does not begin with a
    // loader's name is no load, its whole text not copied; every
    // paragraph of a document asks)
    // (its first command's name, past blanks and comment lines, read
    // where it is: no copy, no look past it)
    let mut t = text;
    loop {
        t = t.trim_start();
        match t.strip_prefix('%') {
            Some(r) => t = r.find('\n').map_or("", |k| &r[k..]),
            None => break,
        }
    }
    let rest = t.strip_prefix('\\')?;
    let name_len = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
    if !LOADERS.contains(&&rest[..name_len]) {
        return None;
    }
    let body = load_body(text, usize::MAX);
    let rest = body.strip_prefix('\\')?;
    let name_len = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
    LOADERS.contains(&&rest[..name_len]).then_some(body)
}

/// The key of a node's record: a package load's [`load_key`], else
/// `n:` and the hash of the slice's text as its node runs it (each
/// line's blanks at its end dropped, [`crate::run::slice_args`]).
pub(crate) fn node_key(text: &str) -> String {
    load_key(text).unwrap_or_else(|| {
        let mut t = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(k) = rest.find('\n') {
            t.push_str(rest[..k].trim_end_matches([' ', '\t', '\r']));
            t.push('\n');
            rest = &rest[k + 1..];
        }
        t.push_str(rest);
        text_key(&t)
    })
}

/// A document node's key from its text as run.
fn text_key(t: &str) -> String {
    format!(
        "n:{:016x}",
        phitex_defuse::hash_bytes(0x6e6f_6465, t.as_bytes())
    )
}

/// Whether `key` is a document node's ([`node_key`]), not a load's.
pub(crate) fn is_node_key(key: &str) -> bool {
    key.starts_with("n:")
}

/// The text a node's arguments run when they are a document slice's
/// lines (`s` frames only): each segment, a line's end after one that
/// ends its line.
fn args_text(v: &Values, args: &[ValueId]) -> Option<String> {
    let mut out = String::new();
    for f in list_of(v, *args.first()?) {
        let xs = list_of(v, f);
        if xs.first().map(|&t| str_of(v, t)).as_deref() != Some("s") {
            return None;
        }
        out.push_str(&str_of(v, *xs.get(1)?));
        if xs.get(4).is_some_and(|&x| int_of(v, x) & 2 != 0) {
            out.push('\n');
        }
    }
    Some(out)
}

/// The record key of a node from its arguments: a document slice's
/// [`node_key`].
pub(crate) fn args_key(
    _reg: &crate::hints::Registry,
    v: &Values,
    args: &[ValueId],
) -> Option<String> {
    args_text(v, args).map(|t| node_key(&t))
}

/// Whether a node's run is recorded: a top-level package load
/// ([`is_load`]), or a document node (a main-file slice, kind 3; a
/// region of a file `\input` reads, kind 2) whose hint answers `Any`
/// or is its record's.
pub(crate) fn is_recorded(reg: &crate::hints::Registry, v: &Values, args: &[ValueId]) -> bool {
    if is_load(v, args) {
        return true;
    }
    if args.get(1).is_none_or(|&t| !matches!(int_of(v, t), 2 | 3)) {
        return false;
    }
    let Some(&fr) = args.first() else {
        return false;
    };
    let fs = list_of(v, fr);
    let frame = |f: ValueId| list_of(v, f).first().map(|&t| str_of(v, t));
    let slice = match fs.as_slice() {
        [] => false,
        [f] => matches!(frame(*f).as_deref(), Some("s" | "w")),
        _ => fs.iter().all(|&f| frame(f).as_deref() == Some("s")),
    };
    if !slice {
        return false;
    }
    let h = crate::hints::hint_of_args(reg, v, args);
    h.bound.is_none() || crate::hints::is_record(&h)
}

/// Whether a hint string (a node's hint argument) answers `Any`.
pub(crate) fn unbounded(hint: &str) -> bool {
    hint.starts_with("A\x1e")
}

/// [`load_key`]'s body: the text, blank and comment lines dropped before
/// it, comments and blanks dropped, up to `max` characters.
fn load_body(text: &str, max: usize) -> String {
    let mut body = String::new();
    for line in text.lines() {
        if body.len() >= max {
            break;
        }
        let t = line.trim_start();
        if body.is_empty() && (t.is_empty() || t.starts_with('%')) {
            continue;
        }
        // (a comment ends the line's text)
        let t = match t.find('%') {
            Some(i) if i == 0 || t.as_bytes()[i - 1] != b'\\' => &t[..i],
            _ => t,
        };
        let room = max.saturating_sub(body.len());
        body.extend(t.chars().filter(|c| !c.is_ascii_whitespace()).take(room));
    }
    body
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
    /// The hint the record gives its load (a document node's, `node`),
    /// when every name encodes.
    pub(crate) fn hint(&self, node: bool) -> Option<String> {
        (self.writes.iter().chain(&self.reads).all(|n| encodable(n))).then(|| {
            // (`PHITEX_PKG_NOREADS=1`: the definitions only, for comparison)
            let reads: &[String] = if false { &[] } else { &self.reads };
            // (`PHITEX_PKG_NOSCOPE=1`, `PHITEX_PKG_NOCLAIM=1`: the record's
            // effect, its claim left to the run, for comparison)
            crate::hints::record_hint(&self.writes, reads, self.no_scope, self.claim0, node)
        })
    }

    /// Whether the files it read are as `now` says (a text's hash, 0
    /// absent, `None` unknown: no match); a file read as units
    /// ([`UNITS`]) matches while it is found.
    pub(crate) fn matches(&self, now: impl Fn(&str) -> Option<u64>) -> bool {
        self.files.iter().all(|(n, h)| match *h {
            UNITS => now(n).is_some_and(|x| x != 0),
            h => now(n) == Some(h),
        })
    }
}

/// A record's hash of a file its run read cut (a document file `\input`
/// reads, `crate::run::Run::read_cut`): what the run read is that it is
/// cut, not the text, so any text of it matches.
pub(crate) const UNITS: u64 = u64::MAX;

/// A value's bytes past which a document node's record keeps no value
/// (the field stays in its writes: no prediction of it).
const VALUE_MAX: usize = 64 << 10;

/// The entry a recorded run contributes to `pkgrecs`: `[node, files,
/// writes, reads, flags, values, key]`, files `[name, hash]` (hash 0:
/// absent), flags 1 no scope, 2 no sibling taken, values `[name,
/// global, value]`, key its [`node_key`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn entry(
    v: &Values,
    node: u64,
    key: &str,
    files: &[(String, u64)],
    writes: &[String],
    reads: &[String],
    flags: i64,
    values: &[(String, bool, ValueId)],
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
        v.list(
            values
                .iter()
                .map(|(n, g, x)| v.list(vec![v.str(n), v.int(i64::from(*g)), *x]))
                .collect(),
        ),
        v.str(key),
    ])
}

/// An entry's node and key, its record not decoded.
pub(crate) fn entry_head(v: &Values, e: ValueId) -> Option<(u64, String)> {
    let xs = list_of(v, e);
    Some((
        crate::val::int_at(v, e, 0).cast_unsigned(),
        str_of(v, *xs.get(6)?),
    ))
}

/// An entry decoded: its node, key and record (its values always:
/// predicting or not, a later build may predict from it; a document
/// node's past [`VALUE_MAX`] bytes left out).
pub(crate) fn decode_entry(v: &Values, e: ValueId) -> Option<(u64, String, Rec)> {
    let xs = list_of(v, e);
    let (node, key) = entry_head(v, e)?;
    let node_rec = is_node_key(&key);
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
    let values = xs
        .get(5)
        .map(|&l| {
            list_of(v, l)
                .into_iter()
                .filter_map(|e| {
                    let ys = list_of(v, e);
                    let b = crate::api::tree_bytes(v, *ys.get(2)?);
                    if node_rec && b.len() > VALUE_MAX {
                        return None;
                    }
                    Some((str_of(v, *ys.first()?), int_of(v, *ys.get(1)?) != 0, b))
                })
                .collect()
        })
        .unwrap_or_default();
    Some((
        node,
        key,
        Rec {
            files,
            writes: strs(*xs.get(2)?),
            reads: strs(*xs.get(3)?),
            no_scope: flags & 1 != 0,
            claim0: flags & 2 != 0,
            values,
        },
    ))
}

/// The records a build gives its loads and document nodes (the
/// machine-wide loads' and the document's own, merged), the files'
/// hashes as the build finds them now, and whether never-seen loads get
/// the optimistic hint.
pub(crate) struct LoadHints {
    pub recs: Mutex<Recs>,
    finder: crate::api::Finder,
    /// The hash of each file's text as found (0: absent), asked once.
    now: Mutex<FxHashMap<String, u64>>,
    pub optimistic: bool,
    /// Whether a matching load's record
    /// predicts its outputs (`PHITEX_PKG_PREDICT=1`, or
    /// [`crate::Options::pkg_predict`]).
    pub predict: bool,
    /// The predictions given this build, by load key (loads' only).
    pub predicted: Mutex<FxHashMap<String, RecValues>>,
    /// Loads given a record, given the optimistic hint, document nodes
    /// given a record (counts).
    pub given: [std::sync::atomic::AtomicU64; 3],
}

impl LoadHints {
    pub(crate) fn new(
        recs: Recs,
        finder: crate::api::Finder,
        optimistic: bool,
        predict: bool,
    ) -> LoadHints {
        LoadHints {
            recs: Mutex::new(recs),
            finder,
            now: Mutex::new(FxHashMap::default()),
            optimistic,
            predict,
            predicted: Mutex::new(FxHashMap::default()),
            given: Default::default(),
        }
    }

    /// The record under `key` when it matches the files as they are now.
    fn matching(&self, v: &Values, key: &str) -> Option<Rec> {
        let r = self.recs.locked().get(key).cloned()?;
        r.matches(|n| Some(self.hash_now(v, n))).then_some(r)
    }

    /// The predicted publications of the node `text` (the hook's
    /// answer): the values its record says it defines, when predicting
    /// and its key matches; each field's name, whether global, and value.
    pub(crate) fn prediction(
        &self,
        v: &Values,
        text: &str,
    ) -> Option<Vec<(String, bool, ValueId)>> {
        self.prediction_of(v, &node_key(text))
    }

    /// [`Self::prediction`] of the record under `key` ([`node_key`]).
    pub(crate) fn prediction_of(
        &self,
        v: &Values,
        key: &str,
    ) -> Option<Vec<(String, bool, ValueId)>> {
        if !self.predict {
            return None;
        }
        let r = self.matching(v, key)?;
        r.values
            .iter()
            .map(|(n, g, b)| Some((n.clone(), *g, crate::api::tree_from_bytes(v, b)?)))
            .collect()
    }

    /// The hash of file `name`'s text as the build's finder gives it
    /// now: its text value's content digest (as a run's `fread` records
    /// it, [`crate::val::digest`]), 0 when absent.
    fn hash_now(&self, v: &Values, name: &str) -> u64 {
        if let Some(&h) = self.now.locked().get(name) {
            return h;
        }
        let h = (self.finder)(name).map_or(0, |(_, bytes)| {
            let text: String = bytes.iter().map(|&b| char::from(b)).collect();
            crate::val::digest(v, v.str(&text))
        });
        self.now.locked().insert(name.to_owned(), h);
        h
    }

    /// File `name` changed: asked again next time.
    pub(crate) fn forget(&self, name: &str) {
        self.now.locked().remove(name);
    }

    /// Whether the record of the node `text` lists file `name` by its
    /// text (one it read cut, [`UNITS`], matches any text of it).
    pub(crate) fn reads_file(&self, text: &str, name: &str) -> bool {
        self.recs
            .locked()
            .get(&node_key(text))
            .is_some_and(|r| r.files.iter().any(|(n, h)| n == name && *h != UNITS))
    }

    /// The hint for the slice `text` (a package load, or a document node
    /// whose own hint answers `Any`: the caller asks for those only),
    /// keyed by [`node_key`]: its record's when the key matches, else
    /// for a load the optimistic one (when on); `None`: the text's own
    /// hint.
    pub(crate) fn hint_for(&self, v: &Values, text: &str) -> Option<String> {
        let k = node_key(text);
        let node = is_node_key(&k);
        let rec = self.recs.locked().get(&k).cloned();
        if crate::exec::obs_flag!("PHITEX_PKG_TRACE") {
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
            && let Some(h) = r.hint(node)
        {
            if node {
                self.given[2].fetch_add(1, Ordering::Relaxed);
                return Some(h);
            }
            self.given[0].fetch_add(1, Ordering::Relaxed);
            if self.predict {
                self.predicted.locked().insert(k, r.values);
            }
            return Some(h);
        }
        if self.optimistic && !node {
            self.given[1].fetch_add(1, Ordering::Relaxed);
            return Some(crate::hints::optimistic_hint());
        }
        None
    }

    /// The records with `fresh` (load key, record) replacing theirs.
    pub(crate) fn merge(&self, fresh: Vec<(String, Rec)>) {
        let mut r = self.recs.locked();
        for (k, x) in fresh {
            r.insert(k, x);
        }
    }

    /// The records whose key `keep` keeps, as bytes.
    pub(crate) fn bytes_where(&self, keep: impl Fn(&str) -> bool) -> Vec<u8> {
        let r = self.recs.locked();
        let kept: Recs = r
            .iter()
            .filter(|(k, _)| keep(k))
            .map(|(k, x)| (k.clone(), x.clone()))
            .collect();
        serialise(&kept)
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

const MAGIC: &[u8; 8] = b"phipkr02";

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
        put_n(&mut o, r.values.len());
        for (n, g, b) in &r.values {
            put(&mut o, n.as_bytes());
            o.push(u8::from(*g));
            put(&mut o, b);
        }
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
            let nv = r.n()?;
            let mut values = Vec::with_capacity(nv.min(1 << 16));
            for _ in 0..nv {
                let name = r.string()?;
                let g = r.take(1)?[0] != 0;
                let n = r.n()?;
                values.push((name, g, r.take(n)?.to_vec()));
            }
            out.insert(
                k,
                Rec {
                    files,
                    writes,
                    reads,
                    no_scope: fl & 1 != 0,
                    claim0: fl & 2 != 0,
                    values,
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
                values: vec![("cs:a".to_owned(), true, vec![1, 2, 3])],
            },
        );
        assert_eq!(parse(&serialise(&recs)), recs);
        assert!(parse(b"junk").is_empty());
    }
}
