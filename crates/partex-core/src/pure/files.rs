//! The files the job reads by lines (`\input`, `\openin`): each a named
//! source of the core (DESIGN 7.22), its lines the elements, inserted by
//! the step that first finds it (`StepCx::source_or_insert`) and kept up
//! to date by the driver; an `\input` level is a call of an unfold over
//! the file's lines that the document resumes after.

use alloc::sync::Arc;
use alloc::vec::Vec;
use std::collections::HashMap;
use std::sync::RwLock;

use phi::{ElemId, Seq, Ver};

use super::lang::Val;

/// A file as the core has it: the name asked for and the name the host
/// found it by, its bytes, where each line starts, its lines' identities.
pub struct FileDoc {
    pub asked: Arc<[u8]>,
    pub name: Arc<[u8]>,
    pub bytes: Arc<[u8]>,
    /// Line `i` starts at `starts[i]`; one more entry, the file's end.
    pub starts: Vec<usize>,
    pub ids: Vec<ElemId>,
    /// Each line's index by its identity.
    pub index: HashMap<ElemId, usize>,
    /// The source's value.
    pub value: Val,
}

/// The files of the build by key (`key_hash`): the ones the steps
/// inserted, as the driver refreshed them.
pub static DOCS: RwLock<Option<HashMap<u64, Arc<FileDoc>>>> = RwLock::new(None);

/// The source key of a file asked for as `name`.
#[must_use]
pub fn key(name: &[u8]) -> Vec<u8> {
    let mut k = b"tex:".to_vec();
    k.extend_from_slice(name);
    k
}

/// A key's number (the file unfolds' op carries it).
#[must_use]
pub fn key_hash(key: &[u8]) -> u64 {
    phi::ver::hash64(key)
}

/// The lines of `bytes` as `input_ln` reads them: each to its end of
/// line (CR LF one end), the last one maybe without.
#[must_use]
pub fn lines(bytes: &[u8]) -> Vec<core::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = i;
        while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
            i += 1;
        }
        if i < bytes.len() {
            if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
            i += 1;
        }
        out.push(b..i);
    }
    out
}

impl FileDoc {
    /// File `bytes`, its lines' identities kept from `old` where the
    /// lines are the same (a common prefix and suffix), new ones between.
    #[must_use]
    pub fn new(asked: &[u8], name: &[u8], bytes: Arc<[u8]>, old: Option<&FileDoc>) -> FileDoc {
        let ranges = lines(&bytes);
        let line = |r: &core::ops::Range<usize>| &bytes[r.clone()];
        let mut ids: Vec<ElemId> = Vec::with_capacity(ranges.len());
        match old {
            None => {
                ids.extend((0..ranges.len()).map(|i| ElemId((i as u64 + 1) << 20)));
            }
            Some(o) => {
                let ol = lines(&o.bytes);
                let oline = |r: &core::ops::Range<usize>| &o.bytes[r.clone()];
                let pre = ol
                    .iter()
                    .zip(&ranges)
                    .take_while(|(a, b)| oline(a) == line(b))
                    .count();
                let suf = ol[pre..]
                    .iter()
                    .rev()
                    .zip(ranges[pre..].iter().rev())
                    .take_while(|(a, b)| oline(a) == line(b))
                    .count();
                ids.extend_from_slice(&o.ids[..pre]);
                // (new identities between the kept ones around them)
                let lo = if pre == 0 { 0 } else { o.ids[pre - 1].0 };
                let hi = if suf == 0 {
                    o.ids.last().map_or(1 << 40, |x| x.0 + (1 << 40))
                } else {
                    o.ids[ol.len() - suf].0
                };
                let n = ranges.len() - pre - suf;
                let room = (hi - lo) / (n as u64 + 1);
                assert!(
                    room > 0 || n == 0,
                    "pure SSA: no room for new lines' identities"
                );
                ids.extend((0..n).map(|k| ElemId(lo + room * (k as u64 + 1))));
                ids.extend_from_slice(&o.ids[o.ids.len() - suf..]);
            }
        }
        let mut starts: Vec<usize> = ranges.iter().map(|r| r.start).collect();
        starts.push(bytes.len());
        let index = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let seq = Seq::from_vec(
            ranges
                .iter()
                .zip(&ids)
                .map(|(r, id)| (*id, Val::line(line(r))))
                .collect(),
        );
        let value = Val::Lines(seq, Ver::node(0x6c69_6e65, &[Ver::of(&bytes[..])]));
        FileDoc {
            asked: Arc::from(asked),
            name: Arc::from(name),
            bytes,
            starts,
            ids,
            index,
            value,
        }
    }
}

/// File `h`, if the build has it.
#[must_use]
pub fn doc(h: u64) -> Option<Arc<FileDoc>> {
    DOCS.read().ok()?.as_ref()?.get(&h).cloned()
}

/// File `h`, or `d` made the build's if it has none (a step inserted it).
pub fn doc_or_insert(h: u64, d: impl FnOnce() -> FileDoc) -> Arc<FileDoc> {
    let mut w = DOCS.write().expect("pure SSA: the files");
    w.get_or_insert_with(HashMap::new)
        .entry(h)
        .or_insert_with(|| Arc::new(d()))
        .clone()
}

/// Replace (or remove) file `h` (the driver's refresh).
pub fn set_doc(h: u64, d: Option<FileDoc>) {
    let mut w = DOCS.write().expect("pure SSA: the files");
    let m = w.get_or_insert_with(HashMap::new);
    match d {
        Some(d) => {
            m.insert(h, Arc::new(d));
        }
        None => {
            m.remove(&h);
        }
    }
}

/// Every file the build has.
#[must_use]
pub fn all() -> Vec<(u64, Arc<FileDoc>)> {
    DOCS.read()
        .ok()
        .and_then(|d| {
            d.as_ref()
                .map(|m| m.iter().map(|(k, v)| (*k, v.clone())).collect())
        })
        .unwrap_or_default()
}
