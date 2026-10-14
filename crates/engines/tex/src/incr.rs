//! A document's text kept as paragraph units ([`phitex_source::Source`]
//! cut by [`Paragraphs`]), with the hint analysis cached per chunk: a run
//! of units analysed together (one unit, unless a conditional, a
//! `\makeatletter` or `\ExplSyntaxOn` or a `\catcode` stays open at its
//! end, which the analysis of a slice must see whole).
//!
//! A chunk's analysis depends on its text and on the definitions in
//! force before it: the format's and those of the earlier chunks that
//! define anything. The latter are named by a key, the hash chain of the
//! texts of the defining chunks before it. An edit splices the units
//! ([`Source::edit`], from the common prefix and suffix of the old and new
//! texts); a chunk whose first unit kept its identity, whose units are
//! the same and whose key is the same keeps its analysis, the others are
//! analysed again.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};

use phitex_source::pass::{Definitions, FileKind};
use phitex_source::{Paragraphs, Source, UnitId};

use crate::hints::{Defs, TextHints};

/// A chunk's analysis, under the key of the definitions before it.
struct Entry {
    units: Vec<UnitId>,
    key: u64,
    th: Arc<TextHints>,
    /// Its slices' keys ([`Chunk::keys`]).
    keys: Arc<[u64]>,
    /// [`Chunk::input`].
    input: Input,
    /// The definitions after it (its key and set): the incoming ones
    /// when it defines nothing.
    out: (u64, Defs),
    /// Whether nothing stays open at its end ([`Open::closed`]).
    closed: bool,
}

/// One chunk of the document as analysed: where it begins in the text,
/// and its analysis (slices and hints relative to that start).
pub(crate) struct Chunk {
    pub start: usize,
    pub th: Arc<TextHints>,
    /// Its slices' keys (`api::key`), a slice at the chunk's start
    /// taken as beginning a line: made when the chunk is analysed.
    pub keys: Arc<[u64]>,
    /// Whether its text names `\\input`, and whether it defines it.
    pub input: Input,
}

/// What a chunk's text does with `\\input`.
#[derive(Clone, Copy, Default)]
pub(crate) struct Input {
    pub names: bool,
    pub defines: bool,
}

impl Input {
    fn of(text: &str) -> Input {
        let names = text.contains("\\input");
        Input {
            names,
            defines: names
                && [
                    "\\def\\input",
                    "\\let\\input",
                    "\\edef\\input",
                    "\\gdef\\input",
                    "\\xdef\\input",
                ]
                .iter()
                .any(|d| text.contains(d)),
        }
    }
}

/// The document's units and their chunks' analyses.
pub(crate) struct Incr {
    src: Source,
    text: String,
    /// The format's definitions.
    defs: Defs,
    cache: HashMap<UnitId, Entry>,
    /// The chunks of the text, in order.
    pub chunks: Vec<Chunk>,
    /// How many chunks the last text analysed again.
    pub analysed: usize,
    /// File ids for the definitions of the document's chunks.
    next_file: u32,
    /// The workers its chunks are analysed on (`crate::exec::each`).
    pub threads: usize,
}

impl Incr {
    pub(crate) fn new(defs: Defs) -> Incr {
        Incr {
            src: Source::with_split("", Arc::new(Paragraphs)),
            text: String::new(),
            defs,
            cache: HashMap::new(),
            chunks: Vec::new(),
            analysed: 0,
            next_file: 2_000_000_000,
            threads: 1,
        }
    }

    /// The format's definitions.
    pub(crate) fn defs(&self) -> &Defs {
        &self.defs
    }

    /// The document's slices, in the whole text's bytes.
    #[cfg(test)]
    pub(crate) fn slices(&self) -> Vec<(usize, usize)> {
        self.chunks
            .iter()
            .flat_map(|c| {
                c.th.slices
                    .iter()
                    .map(move |&(s, e)| (c.start + s, c.start + e))
            })
            .collect()
    }

    /// The chunk holding byte `s` of the text, and `s` in it.
    pub(crate) fn chunk_at(&self, s: usize) -> (&Chunk, usize) {
        let k = self.chunks.partition_point(|c| c.start <= s).max(1) - 1;
        let c = &self.chunks[k];
        (c, s - c.start)
    }

    /// The document is now `text`: the units are spliced and the chunks
    /// analysed again where their text or incoming definitions changed.
    pub(crate) fn set_text(&mut self, text: &str) {
        if text != self.text {
            self.splice(text);
            text.clone_into(&mut self.text);
        } else if !self.chunks.is_empty() || text.is_empty() {
            self.analysed = 0;
            return;
        }
        self.walk();
    }

    fn splice(&mut self, text: &str) {
        let old = &self.text;
        if old.is_empty() || self.src.start().is_none() {
            self.src = Source::with_split(text, Arc::new(Paragraphs));
            return;
        }
        let (a, b) = (old.as_bytes(), text.as_bytes());
        let mut p = common_prefix(a, b);
        while !old.is_char_boundary(p) || !text.is_char_boundary(p) {
            p -= 1;
        }
        let max = a.len().min(b.len()) - p;
        let mut s = common_suffix(&a[a.len() - max..], &b[b.len() - max..]);
        while !old.is_char_boundary(a.len() - s) || !text.is_char_boundary(b.len() - s) {
            s -= 1;
        }
        let (Some(from), Some(to)) = (self.src.at_byte(p), self.src.at_byte(a.len() - s)) else {
            self.src = Source::with_split(text, Arc::new(Paragraphs));
            return;
        };
        let _ = self.src.edit(from, to, &text[p..b.len() - s]);
    }

    /// The chunks of the units `ids`, with the cached analyses of their
    /// first units (taken from `cache`) where the units are the same.
    fn parts(&self, ids: &[UnitId], cache: &mut HashMap<UnitId, Entry>) -> Vec<Part> {
        let mut parts: Vec<Part> = Vec::new();
        let (mut i, mut off) = (0, 0);
        while i < ids.len() {
            // (a cached chunk of the same first unit is what a scan
            // finds when its units follow and it closed or ends the text)
            let cached = cache.remove(&ids[i]).filter(|e| {
                ids[i..].starts_with(&e.units) && (e.closed || i + e.units.len() == ids.len())
            });
            let (j, closed) = if let Some(e) = &cached {
                (i + e.units.len(), e.closed)
            } else {
                let mut st = Open::default();
                let mut j = i;
                loop {
                    st.scan(self.src.text(ids[j]));
                    j += 1;
                    if st.closed() || j == ids.len() {
                        break;
                    }
                }
                (j, st.closed())
            };
            let len: usize = ids[i..j].iter().map(|&u| self.src.text(u).len()).sum();
            parts.push(Part {
                units: i..j,
                start: off,
                len,
                closed,
                cached,
            });
            off += len;
            i = j;
        }
        debug_assert_eq!(off, self.text.len(), "the units are the text");
        parts
    }

    /// The chunks of the units, analysed where their text or incoming
    /// definitions changed. In three passes: the chunks' bounds (what a
    /// unit leaves open, the text alone) and, for the chunks not cached,
    /// their own definitions, on the pool; the definitions in force
    /// before each chunk, a chain through the chunks that define
    /// anything (in order: each adds to the earlier ones'); the chunks'
    /// analyses under them, on the pool.
    fn walk(&mut self) {
        let ids: Vec<UnitId> = self.src.units().collect();
        let mut cache = std::mem::take(&mut self.cache);
        let mut parts = self.parts(&ids, &mut cache);
        let text = self.text.as_str();
        let chunk_text = |p: &Part| &text[p.start..p.start + p.len];
        let threads = self.threads;
        // the own definitions of the chunks not cached (whether they
        // define anything: what a cached one's analysis says)
        let fresh: Vec<usize> = (0..parts.len())
            .filter(|&k| parts[k].cached.is_none())
            .collect();
        let owns: Vec<Mutex<Option<Definitions>>> = crate::exec::map(threads, fresh.len(), &|x| {
            Mutex::new(Some(crate::hints::own_definitions(
                chunk_text(&parts[fresh[x]]),
                FileKind::Document,
            )))
        });
        let mut own_of: Vec<Option<usize>> = vec![None; parts.len()];
        for (x, &k) in fresh.iter().enumerate() {
            own_of[k] = Some(x);
        }
        // the definitions before each chunk, in order: a cached chunk
        // under the same ones keeps its analysis
        let mut ins: Vec<(u64, Defs)> = Vec::with_capacity(parts.len());
        let mut todo: Vec<usize> = Vec::new();
        let mut outs: Vec<Option<(u64, Defs)>> = Vec::with_capacity(parts.len());
        let (mut key, mut defs) = (0u64, self.defs.clone());
        for (k, p) in parts.iter_mut().enumerate() {
            ins.push((key, defs.clone()));
            // (whether it defines anything is its text's alone: a cached
            // analysis under other definitions says it too)
            let cached_defines = p.cached.as_ref().map(|e| e.th.defines);
            if p.cached.as_ref().is_some_and(|e| e.key != key) {
                p.cached = None;
            }
            let out = if let Some(e) = &p.cached {
                e.out.clone()
            } else {
                todo.push(k);
                let defines = cached_defines.unwrap_or_else(|| {
                    own_of[k].is_some_and(|x| {
                        owns[x]
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .as_ref()
                            .is_some_and(crate::hints::defines_any)
                    })
                });
                if defines {
                    let t = chunk_text(p);
                    let mut d: Definitions = (*defs).clone();
                    d.harvest(t, FileKind::Document, self.next_file);
                    self.next_file = self.next_file.wrapping_add(1);
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    key.hash(&mut h);
                    t.hash(&mut h);
                    (h.finish(), Arc::new(d))
                } else {
                    (key, defs.clone())
                }
            };
            (key, defs) = (out.0, out.1.clone());
            outs.push(Some(out));
        }
        // the analyses of the chunks not kept
        let made: Vec<(Arc<TextHints>, Arc<[u64]>, Input)> =
            crate::exec::map(threads, todo.len(), &|x| {
                let k = todo[x];
                let t = chunk_text(&parts[k]);
                let own = own_of[k].and_then(|o| {
                    owns[o]
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take()
                });
                analyse(t, &ins[k].1, own)
            });
        let analysed = todo.len();
        let mut made = made.into_iter();
        let mut chunks = Vec::with_capacity(parts.len());
        for (k, p) in parts.into_iter().enumerate() {
            let e = if let Some(e) = p.cached {
                e
            } else {
                let (th, keys, input) = made.next().expect("every chunk analysed");
                Entry {
                    units: ids[p.units.clone()].to_vec(),
                    key: ins[k].0,
                    th,
                    keys,
                    input,
                    out: outs[k].take().expect("the definitions after it"),
                    closed: p.closed,
                }
            };
            chunks.push(Chunk {
                start: p.start,
                th: e.th.clone(),
                keys: e.keys.clone(),
                input: e.input,
            });
            self.cache.insert(e.units[0], e);
        }
        self.chunks = chunks;
        self.analysed = analysed;
    }
}

/// A chunk's analysis under the definitions `defs` before it (given its
/// own, [`crate::hints::own_definitions`], when harvested already): its
/// hints, its slices' keys (a slice at its start taken as beginning a
/// line), and what it does with `\\input`.
fn analyse(t: &str, defs: &Defs, own: Option<Definitions>) -> (Arc<TextHints>, Arc<[u64]>, Input) {
    let th = crate::hints::text_hints_with(t, FileKind::Document, defs, own);
    let keys = th
        .slices
        .iter()
        .map(|&(s, e)| {
            let starts = s == 0 || t.as_bytes()[s - 1] == b'\n';
            crate::api::key(&t[s..e], starts)
        })
        .collect();
    (th, keys, Input::of(t))
}

/// A chunk of the units being walked: its units, where it begins and its
/// length in bytes, whether nothing stays open at its end, and its cached
/// analysis while that may be kept.
struct Part {
    units: std::ops::Range<usize>,
    start: usize,
    len: usize,
    closed: bool,
    cached: Option<Entry>,
}

/// The length of the prefix `a` and `b` share: block by block (a
/// memcmp each), then byte by byte in the first block that differs.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    const B: usize = 4096;
    let n = a.len().min(b.len());
    let mut i = 0;
    while i + B <= n && a[i..i + B] == b[i..i + B] {
        i += B;
    }
    i + a[i..n]
        .iter()
        .zip(&b[i..n])
        .take_while(|(x, y)| x == y)
        .count()
}

/// The length of the suffix `a` and `b` share ([`common_prefix`] from
/// the end).
fn common_suffix(a: &[u8], b: &[u8]) -> usize {
    const B: usize = 4096;
    let n = a.len().min(b.len());
    let (a, b) = (&a[a.len() - n..], &b[b.len() - n..]);
    let mut i = 0;
    while i + B <= n && a[n - i - B..n - i] == b[n - i - B..n - i] {
        i += B;
    }
    i + a[..n - i]
        .iter()
        .rev()
        .zip(b[..n - i].iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

/// The conditionals of TeX, e-TeX and pdfTeX (a document's `\newif`s
/// are added as they are met).
const CONDS: [&str; 26] = [
    "if",
    "ifcat",
    "ifnum",
    "ifdim",
    "ifodd",
    "ifvmode",
    "ifhmode",
    "ifmmode",
    "ifinner",
    "ifvoid",
    "ifhbox",
    "ifvbox",
    "ifx",
    "ifeof",
    "iftrue",
    "iffalse",
    "ifcase",
    "ifdefined",
    "ifcsname",
    "iffontchar",
    "ifincsname",
    "ifpdfprimitive",
    "ifpdfabsnum",
    "ifpdfabsdim",
    "ifabsnum",
    "ifabsdim",
];

/// What a chunk leaves open at a unit's end (a syntactic count, on the
/// safe side: what it cannot close keeps the chunk going).
#[derive(Default)]
struct Open {
    conds: usize,
    at_letter: bool,
    expl: bool,
    catcode: bool,
    newifs: HashSet<String>,
}

impl Open {
    fn closed(&self) -> bool {
        self.conds == 0 && !self.at_letter && !self.expl && !self.catcode
    }

    fn scan(&mut self, t: &str) {
        let b = t.as_bytes();
        let mut k = 0;
        let mut prev = String::new();
        while k < b.len() {
            match b[k] {
                b'%' => {
                    k = b[k..]
                        .iter()
                        .position(|&c| c == b'\n')
                        .map_or(b.len(), |x| k + x);
                }
                b'\\' => {
                    let s = k + 1;
                    let mut e = s;
                    while e < b.len()
                        && (b[e].is_ascii_alphabetic() || (self.at_letter && b[e] == b'@'))
                    {
                        e += 1;
                    }
                    if e == s {
                        k = s + 1;
                        continue;
                    }
                    let name = &t[s..e];
                    match name {
                        "fi" => self.conds = self.conds.saturating_sub(1),
                        "makeatletter" => self.at_letter = true,
                        "makeatother" => self.at_letter = false,
                        "ExplSyntaxOn" => self.expl = true,
                        "ExplSyntaxOff" => self.expl = false,
                        "catcode" => self.catcode = true,
                        _ if prev == "newif" => {
                            self.newifs.insert(name.to_owned());
                        }
                        _ if CONDS.contains(&name) || self.newifs.contains(name) => {
                            self.conds += 1;
                        }
                        _ => {}
                    }
                    name.clone_into(&mut prev);
                    k = e;
                }
                c => {
                    if !c.is_ascii_whitespace() {
                        prev.clear();
                    }
                    k += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The analysis of an edit of one word in the middle of the
    /// document named by `PHITEX_INCR_DOC`: the whole text's (before)
    /// against the chunks' (after), and the slices compared.
    #[test]
    #[ignore = "a measurement (a document given by path)"]
    fn incr_edit_doc() {
        let Some(path) = std::env::var_os("PHITEX_INCR_DOC") else {
            return;
        };
        let b = std::fs::read(path).expect("the document");
        let text: String = b.iter().map(|&c| char::from(c)).collect();
        let mid = text.len() / 2;
        let at = mid + text[mid..].find(' ').expect("a space");
        let edited = format!("{}XYZ{}", &text[..at], &text[at..]);
        let defs = crate::hints::no_defs().clone();
        let mut inc = Incr::new(defs.clone());
        let t0 = std::time::Instant::now();
        inc.set_text(&text);
        let cold = t0.elapsed();
        let cold_chunks = inc.analysed;
        let t0 = std::time::Instant::now();
        inc.set_text(&edited);
        let warm = t0.elapsed();
        let t0 = std::time::Instant::now();
        let whole = crate::hints::text_hints(&edited, FileKind::Document, &defs);
        let before = t0.elapsed();
        eprintln!(
            "incr: cold {cold:?} ({cold_chunks} chunks), edit {warm:?} ({} of {} chunks analysed); whole text {before:?}; slices {} (whole {})",
            inc.analysed,
            inc.chunks.len(),
            inc.slices().len(),
            whole.slices.len()
        );
        let mut fresh = Incr::new(defs);
        fresh.set_text(&edited);
        assert_eq!(
            fresh.slices(),
            inc.slices(),
            "an edit gives the cold slices"
        );
    }

    /// A build of the plain document named by `PHITEX_INCR_DOC` after
    /// plain TeX, then the edit of one word in its middle: the edit's
    /// wall and the nodes it ran (`PHITEX_TEX_TIME=1` prints the text's
    /// analysis).
    #[test]
    #[ignore = "a measurement (a document given by path)"]
    fn incr_edit_build() {
        let Some(path) = std::env::var_os("PHITEX_INCR_DOC") else {
            return;
        };
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../oracle/fixtures");
        let read = |p: &std::path::Path| -> String {
            std::fs::read(p)
                .expect("a file")
                .iter()
                .map(|&c| char::from(c))
                .collect()
        };
        let s = read(&fx.join("dvi/plaintext.tex"));
        let at = s.find("\\output={}\n").expect("plaintext.tex's body");
        let prelude = s[..at].replace(
            "% (\\input hyphen: no hyphenation patterns on the graph yet)\n",
            &read(&fx.join("hyph/hyphen.tex")),
        );
        let kp = std::sync::Mutex::new({
            let p = std::env::var("PATH").unwrap_or_default();
            std::env::split_paths(&p)
                .find(|d| d.join("tex").is_file())
                .map(|d| phitex_kpse::Kpse::new(&d, "tex", "tex"))
        });
        let finder: crate::Finder = Arc::new(move |name: &str| {
            let fmt = match name.rsplit('.').next() {
                Some("tfm") => phitex_kpse::Format::Tfm,
                Some("map") => phitex_kpse::Format::FontMap,
                Some("pfb" | "pfa") => phitex_kpse::Format::Type1,
                Some("enc") => phitex_kpse::Format::Enc,
                Some("vf") => phitex_kpse::Format::Vf,
                _ => phitex_kpse::Format::Tex,
            };
            let mut k = kp.lock().ok()?;
            let p = k.as_mut()?.find_file(name.as_bytes(), fmt, false)?;
            let p = String::from_utf8(p).ok()?;
            Some((p.clone(), std::fs::read(&p).ok()?))
        });
        let text = read(std::path::Path::new(&path));
        let mid = text.len() / 2;
        let at = mid + text[mid..].find(' ').expect("a space");
        let edited = format!("{}XYZ{}", &text[..at], &text[at..]);
        let opts = crate::Options {
            finder,
            ..crate::Options::default()
        };
        let t0 = std::time::Instant::now();
        let mut b = crate::Build::with(opts, Some(&prelude), &text);
        eprintln!(
            "build: cold {:?}, {} nodes run",
            t0.elapsed(),
            b.stats.nodes_run
        );
        let t0 = std::time::Instant::now();
        b.edit(&edited);
        eprintln!(
            "build: edit {:?}, {} nodes run",
            t0.elapsed(),
            b.stats.nodes_run
        );
    }

    /// The cold analysis of the document named by `PHITEX_INCR_DOC`
    /// only (for a profile).
    #[test]
    #[ignore = "a measurement (a document given by path)"]
    fn incr_cold_doc() {
        let Some(path) = std::env::var_os("PHITEX_INCR_DOC") else {
            return;
        };
        let b = std::fs::read(path).expect("the document");
        let text: String = b.iter().map(|&c| char::from(c)).collect();
        let mut inc = Incr::new(crate::hints::no_defs().clone());
        inc.set_text(&text);
        eprintln!("incr: cold, {} chunks", inc.chunks.len());
    }

    #[test]
    fn chunks_follow_conditionals() {
        let mut inc = Incr::new(crate::hints::no_defs().clone());
        inc.set_text("a\n\n\\ifx a b\n\nc\\fi\n\nd\n");
        assert_eq!(inc.chunks.len(), 3);
        inc.set_text("a\n\n\\ifx a b\n\nc\\fi\n\nd e\n");
        assert_eq!(inc.analysed, 1);
        assert_eq!(inc.chunks.len(), 3);
    }
}
