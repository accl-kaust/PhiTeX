//! The exact PDF file (pdfTeX §750–§815) as the link of the line's
//! records (spec-redesign §1, `link`): no node reads the writer's state.
//!
//! - A page is written apart by the node that ships it ([`ship_apart`]:
//!   the box decoded, the fonts, the content stream compressed, as a
//!   record, `phitex_pdf::PageRecord`), on a writer of its own as the
//!   link's stands between two pages ([`ship_base`]), and its key goes
//!   to the line's accumulator ([`XPAGES`]).
//! - A read-time command (`\pdfobj`, `\pdfxform`, `\pdfximage`,
//!   `\pdfannot`, ...) records itself in the same accumulator where it
//!   runs ([`command`]): the writer's step it stands for, kept by the
//!   build ([`Cmd`]), and the key it is known by. What TeX reads back of
//!   it (an object number, `\pdffontobjnum`, a stack's number) is what
//!   the link gave that key the round before ([`OBJNUMS`], a loop-back):
//!   a guess, refuted and run again when the link gives another.
//! - The end node links ([`finish_exact`]): one writer state from the
//!   start, every record of the line put on it in order (a page by
//!   `PdfState::replay_page`, its objects numbered as pdfTeX numbers
//!   them; shipped again where the replay cannot give the walk's bytes;
//!   a command's step applied), the fonts' names over the pages
//!   ([`FontTags`]), then the tail and the cross-reference section by
//!   the prefix sums of the chunks' lengths.

use std::collections::{BTreeMap, BTreeSet};

use phitex_defuse::FxHashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{ValueId, Values};
use phitex_pdf::map::{MapEntry, parse_line, parse_map, parse_map_ps};
use phitex_pdf::{
    Action, ColorStackCmd, DestKind, FontSource, Ident, LiteralMode, PdfBytes, PdfParams, PdfState,
    VirtualFont, Whatsit,
};
use phitex_tfm::ScaledFont;
use phitex_typeset::fonts::FontId;
use phitex_typeset::node::BoxNode;

use crate::fonts::bytes_of;
use crate::nodes::Side;
use crate::run::{R, Run};
use crate::val::{int_of, list_of, str_of};

/// The line's records for the exact writer, in document order: a page,
/// the key of its [`PagePiece`] ([`pieces`]); a read-time command,
/// `["cmd", key, serial]` ([`command`]); a document-level command,
/// `[kind, args...]` ([`doc_op`]). The link puts them on one state in
/// this order ([`finish_exact`]).
pub(crate) const XPAGES: &str = "pdf:xpages";

/// The loop-back of the object numbers the link gave the read-time
/// commands (spec-redesign §1, `pdf:objbase`: what a piece reads where
/// TeX reads a number back, a guess from the round before):
/// `[[key, [n, ...], error], ...]`, by the command's key ([`command`]).
pub(crate) const OBJNUMS: &str = "pdf:objbase";

/// The writer's parameters a step reads as it writes (the compression,
/// the digits, the version: pdfTeX fixes them at the first write,
/// §792), as TeX has them where the step runs.
pub(crate) type WParams = [i32; 5];

/// The writer's parameters where this run is.
pub(crate) fn writer_params(r: &mut Run<'_, '_>) -> R<WParams> {
    let mut ps = [0; 5];
    for (p, n) in ps.iter_mut().zip([
        "pdfcompresslevel",
        "pdfobjcompresslevel",
        "pdfdecimaldigits",
        "pdfminorversion",
        "pdfmajorversion",
    ]) {
        *p = r.param(n)?;
    }
    Ok(ps)
}

/// The writer's parameters `ps` on `st`.
fn set_writer_params(st: &mut PdfState, ps: WParams) {
    let pm = st.params_mut();
    pm.compress_level = ps[0].clamp(0, 9);
    pm.obj_compress_level = ps[1];
    pm.decimal_digits = ps[2];
    pm.minor_version = ps[3];
    pm.major_version = ps[4];
}

/// What the link gave a read-time command: the numbers TeX reads back
/// (an object's, a font's, a stack's), and the writer's error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CmdOut {
    pub nums: Vec<i64>,
    pub err: Option<String>,
}

impl CmdOut {
    /// One number, no error.
    pub(crate) fn num(n: impl TryInto<i64>) -> CmdOut {
        CmdOut {
            nums: vec![n.try_into().unwrap_or(0)],
            err: None,
        }
    }

    /// Nothing but an error, if any.
    pub(crate) fn err(e: Option<impl std::fmt::Display>) -> CmdOut {
        CmdOut {
            nums: Vec::new(),
            err: e.map(|e| e.to_string()),
        }
    }

    /// The `i`-th number.
    pub(crate) fn at(&self, i: usize) -> Option<i64> {
        self.nums.get(i).copied()
    }

    fn value(&self, v: &Values) -> ValueId {
        v.list(vec![
            v.list(self.nums.iter().map(|&n| v.int(n)).collect()),
            self.err.as_deref().map_or(ValueId::UNIT, |e| v.str(e)),
        ])
    }

    fn of_value(v: &Values, x: ValueId) -> CmdOut {
        let xs = list_of(v, x);
        CmdOut {
            nums: xs
                .first()
                .map(|&l| list_of(v, l).iter().map(|&n| int_of(v, n)).collect())
                .unwrap_or_default(),
            err: xs
                .get(1)
                .filter(|&&e| e != ValueId::UNIT)
                .map(|&e| str_of(v, e)),
        }
    }
}

/// A read-time command's step on the writer, as the link applies it.
pub(crate) type CmdFn = Box<dyn FnMut(&mut PdfState, &mut Forms, &Fonts) -> CmdOut + Send>;

/// A read-time command as the build keeps it for the link ([`command`]):
/// its step, the fonts it names, the writer's parameters where it ran.
pub(crate) struct Cmd {
    f: Mutex<CmdFn>,
    fonts: Fonts,
    wparams: WParams,
}

/// The build's read-time commands by serial ([`crate::run::Ops::pdf_cmds`]).
pub(crate) type CmdMemo = Mutex<FxHashMap<u64, Arc<Cmd>>>;

/// One page as the exact writer ships it, made by the page's own step
/// with nothing read from another page's: the box, the parameters
/// `\shipout` read, the whatsits' data, the fonts. The link puts it on
/// its writer in the line's order ([`finish_exact`]); the node that
/// shipped it has already written it once, apart, so that the link finds
/// its content stream compressed.
pub(crate) struct PagePiece {
    /// The writer's parameters where it was shipped ([`WParams`]).
    wparams: WParams,
    /// The page's box as `\shipout` got it (decoded again only for a
    /// page the finish cannot place).
    boxv: ValueId,
    params: Vec<ValueId>,
    margins: [i32; 3],
    whats: BTreeMap<u32, Whatsit>,
    fonts: Fonts,
    /// The page as its step wrote it, for the finish to place.
    rec: phitex_pdf::PageRecord,
    /// The fonts used on the state the step wrote it on.
    base: BTreeSet<FontId>,
    /// The page's index in the page chain and the fonts loaded when it
    /// shipped (`fontcount`: the step's own, an output routine's and its
    /// virtual fonts', numbered after the text's), for
    /// [`FontTags`].
    pj: i64,
    fcount: i64,
    /// The writer's error writing it (pdfTeX's, `PDF-BACKEND-GAP`: a
    /// fatal error there, here logged, the job going on) and the node
    /// that wrote it: the job's error only where the end node places the
    /// page ([`placed_errors`]); a run fed another mode's handles, which
    /// the engine runs again with its own, leaves none in the job.
    err: Option<(String, i64)>,
}

type PieceMemo = Mutex<FxHashMap<u64, Arc<PagePiece>>>;

/// The pages by key: a pure function's memo.
fn pieces() -> &'static PieceMemo {
    static P: OnceLock<PieceMemo> = OnceLock::new();
    P.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// A document-level command (`\pdfinfo`, `\pdfcatalog`, `\pdfnames`,
/// `\pdftrailer`, `\pdftrailerid`): kept in the line's accumulator
/// ([`XPAGES`]) where it ran, and applied by the link there. True when
/// it was kept (the caller does nothing more).
pub(crate) fn doc_op(r: &mut Run<'_, '_>, kind: &str, args: &[ValueId]) -> R<bool> {
    if r.param("pdfoutput")? <= 0 || crate::pdfrelaxed::on(r)? {
        return Ok(false);
    }
    let v = r.v;
    let mut xs = vec![v.str(kind)];
    xs.extend_from_slice(args);
    r.contribute(XPAGES, v.seq_unit(v.list(xs)));
    Ok(true)
}

/// A document-level command kept by [`doc_op`], applied to `st`.
fn apply_doc_op(v: &Values, st: &mut PdfState, e: ValueId) {
    let xs = list_of(v, e);
    let text = |k: usize| {
        xs.get(k)
            .map(|&t| bytes_of(&str_of(v, t)))
            .unwrap_or_default()
    };
    match xs.first().map(|&k| str_of(v, k)).as_deref() {
        Some("info") => st.info(&text(1)),
        Some("names") => st.names(&text(1)),
        Some("trailer") => st.trailer(&text(1)),
        Some("trailerid") => st.trailer_id(&text(1)),
        Some("catalog") => {
            let a = xs
                .get(2)
                .copied()
                .filter(|&x| x != ValueId::UNIT)
                .and_then(|x| action(v, x));
            // (the step's run logged nothing: the error is the fold's)
            let _ = st.catalog(&text(1), a.as_ref());
        }
        _ => {}
    }
}

/// `pdftex_banner` of the oracle's pdfTeX.
pub(crate) const BANNER: &str = "This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux) kpathsea version 6.4.2";

/// A font map by TFM name.
type Map = BTreeMap<Vec<u8>, MapEntry>;

/// A pure function's memo by a value's id, computed once: a key's
/// first caller computes it, concurrent callers of the same key wait for
/// that result instead of computing it again (every page step asks for
/// `pdftex.map` at once in a cold build).
type Once<V> = Mutex<FxHashMap<u64, Arc<OnceLock<V>>>>;

/// The memo's value for `key`, `f` run only by the key's first caller.
fn once<V: Clone>(m: &Once<V>, key: u64, f: impl FnOnce() -> V) -> V {
    let cell = m
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key)
        .or_default()
        .clone();
    cell.get_or_init(f).clone()
}

type MapCache = Once<Arc<Map>>;

/// Parsed map files: a pure function's memo by the file's value.
fn maps() -> &'static MapCache {
    static C: OnceLock<MapCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// What the fold carries beside the writer: the whatsits of the forms'
/// boxes (`\pdfxform`), by the handles their boxes were given (from
/// [`FORM_HANDLES`] up), and the fonts those boxes use.
#[derive(Clone, Default)]
pub(crate) struct Forms {
    pub whats: BTreeMap<u32, Whatsit>,
    pub fonts: BTreeSet<u32>,
}

/// The first handle of a form's whatsits: a page's are below.
pub(crate) const FORM_HANDLES: u32 = 0x8000_0000;

impl Forms {
    /// The next free handle.
    pub(crate) fn next_handle(&self) -> u32 {
        self.whats
            .last_key_value()
            .map_or(FORM_HANDLES, |(&k, _)| k + 1)
    }
}

/// A writer state with its forms: a page is written apart on one
/// ([`bases`]).
#[derive(Clone)]
pub(crate) struct Fold {
    st: PdfState,
    forms: Forms,
}

type TailMemo = Mutex<FxHashMap<u64, Arc<phitex_pdf::Tail>>>;

/// The end of the file's objects by the final state's digest
/// ([`finish_exact`]): a pure function's memo.
fn tails() -> &'static TailMemo {
    static T: OnceLock<TailMemo> = OnceLock::new();
    T.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// The fonts' numbers as pdfTeX gives them in their names (`/F<n>`),
/// where they differ from the engine's, as a fold over the pages placed
/// so far. The engine numbers the fonts in the order the line loads
/// them (`fontcount`, a field every load defines for what follows it:
/// the text's and a finisher's output routine's alike, a finisher being
/// a node in the line), which is pdfTeX's order except where a
/// `\shipout` loads fonts as its walk reads them in: a virtual font's
/// local fonts at its `do_vf` (the engine loads them before the walk,
/// [`vf_for`]) and `pdftexspace` ([`SPACE_FONT`]) where a faked space
/// first needs it. Carried in the writer's fold, not read from every
/// page before: an edited page changes no later placement's reads
/// unless its fonts' order changed.
///
/// Example: text fonts 1 and 2, then a page whose virtual font 2 loads
/// its local font 3 and whose walk needs the space font, then a text
/// font 4: pdfTeX's numbers 1, 2, 3, space 4, then 5 for font 4.
#[derive(Clone, Debug, Default, Hash)]
pub(crate) struct FontTags {
    done: BTreeSet<FontId>,
    next: FontId,
    /// The fonts loaded before the pages placed so far.
    steps: i64,
    map: BTreeMap<FontId, FontId>,
}

impl FontTags {
    fn put(&mut self, f: FontId) {
        if self.done.insert(f) {
            self.next += 1;
            if f != self.next {
                self.map.insert(f, self.next);
            }
        }
    }

    /// The page `pc` placed after the pages so far.
    fn page(&mut self, pc: &PagePiece) {
        let id = |f: i64| FontId::try_from(f).unwrap_or(0);
        let steps = self.steps;
        let mine = |f: FontId| i64::from(f) > steps && i64::from(f) <= pc.fcount;
        // the fonts this page's virtual fonts loaded (at its walk)
        let locals: BTreeSet<FontId> = pc
            .fonts
            .vfs
            .values()
            .flat_map(|vf| vf.fonts.iter().map(|&(_, g)| g))
            .filter(|&g| mine(g))
            .collect();
        // the line's fonts loaded before this `\shipout`: the text's,
        // the output routine's
        for f in steps + 1..=pc.fcount {
            if !locals.contains(&id(f)) {
                self.put(id(f));
            }
        }
        // then as the walk reads them in
        for (vf, f) in pc.rec.font_loads() {
            if vf {
                if let Some(x) = pc.fonts.vfs.get(&f) {
                    for &(_, g) in &x.fonts {
                        if mine(g) {
                            self.put(g);
                        }
                    }
                }
            } else if f == SPACE_FONT {
                self.put(f);
            }
        }
        for f in steps + 1..=pc.fcount {
            self.put(id(f));
        }
        self.steps = self.steps.max(pc.fcount);
    }

    /// The fonts loaded after the last page, `t` in all.
    fn end(&mut self, t: i64) {
        for f in self.steps + 1..=t {
            self.put(FontId::try_from(f).unwrap_or(0));
        }
        self.steps = self.steps.max(t);
    }

    /// The names where they differ from the engine's numbers.
    fn names(&self) -> Option<std::collections::HashMap<FontId, FontId>> {
        (!self.map.is_empty()).then(|| self.map.iter().map(|(&a, &b)| (a, b)).collect())
    }
}

/// The writer a page shipped here is written apart on, and its key: a
/// fresh one for the first page; for a later one, a fresh one that has
/// shipped an empty page, as the link's writer stands between two pages
/// (the header written, the version fixed): the record replays where
/// the link's variables are these ([`PdfState::replay_page`]); a page
/// whose are not (a link or a color running across, a form) is shipped
/// again by the link, the same bytes either way. No other node's output
/// is read.
fn ship_base(r: &mut Run<'_, '_>, params: &[ValueId], wparams: WParams) -> R<(u64, Fold)> {
    let v = r.v;
    let date = r
        .get_loc(crate::fields::cell::PDFDATE)?
        .map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
    let jobname = r
        .get_loc(crate::fields::cell::JOBNAME)?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    let g2u = r.get("pdf:glyphtounicode")?;
    let mut st = fresh(v, date, &jobname, g2u);
    let first = r.page_index() == 0;
    if !first {
        set_writer_params(&mut st, wparams);
        page_params(v, st.params_mut(), params);
        let empty = BoxNode::default();
        let mut wh = |_: u32, _: bool| -> Option<Whatsit> { None };
        let _ = st.ship_out_with(&empty, &Fonts::default(), &mut wh);
        let _ = st.flush_chunk();
    }
    Ok((
        u64::from(!first),
        Fold {
            st,
            forms: Forms::default(),
        },
    ))
}

/// The fonts of the forms made so far (`pdf:formfonts`, defined by each
/// `\pdfxform`): a page or a form that may ship one names them too.
pub(crate) fn form_fonts(r: &mut Run<'_, '_>) -> R<Vec<u32>> {
    let v = r.v;
    Ok(r.get_loc(crate::fields::cell::PDF_FORMFONTS)?
        .map(|x| list_of(v, x))
        .unwrap_or_default()
        .iter()
        .map(|&f| u32::try_from(int_of(v, f)).unwrap_or(0))
        .collect())
}

/// `ids` added to the fonts of the forms made so far (`\pdfxform`).
pub(crate) fn add_form_fonts(r: &mut Run<'_, '_>, ids: &[u32]) -> R<()> {
    let v = r.v;
    let mut all: BTreeSet<u32> = form_fonts(r)?.into_iter().collect();
    let n = all.len();
    all.extend(ids.iter().copied());
    if all.len() != n {
        let x = v.list(all.into_iter().map(|f| v.int(i64::from(f))).collect());
        r.set_cell(crate::fields::cell::PDF_FORMFONTS, x, true);
    }
    Ok(())
}

/// One font as the writer asks for it.
struct Font {
    name: Vec<u8>,
    metrics: ScaledFont,
    entry: Option<MapEntry>,
    ff: Option<Arc<[u8]>>,
    enc: Option<Arc<[u8]>>,
}

/// The fonts a step needs, the files map lines name, and the space
/// font's number if loaded.
#[derive(Default)]
pub(crate) struct Fonts {
    by_id: FxHashMap<FontId, Font>,
    pub(crate) files: FxHashMap<Vec<u8>, Arc<[u8]>>,
    space: Option<FontId>,
    pub(crate) ps: Option<Arc<PsMap>>,
    /// The files asked for and not given (a dry run's findings).
    pub(crate) missed: Mutex<BTreeSet<Vec<u8>>>,
    /// The virtual fonts `do_vf` read, by font.
    vfs: FxHashMap<FontId, VirtualFont>,
}

impl Fonts {
    /// A digest of the fonts as the writer sees them (a memo key):
    /// names, sizes, map entries, the files' bytes.
    pub(crate) fn digest(&self) -> u64 {
        let mut ids: Vec<&FontId> = self.by_id.keys().collect();
        ids.sort_unstable();
        let mut h = 0;
        for f in ids {
            let x = &self.by_id[f];
            let words = [
                u64::from(*f),
                u64::from_ne_bytes(i64::from(x.metrics.size).to_ne_bytes()),
            ];
            h = phitex_defuse::hash_words(h, &words);
            h = phitex_defuse::hash_bytes(h, &x.name);
            h = phitex_defuse::hash_bytes(h, format!("{:?}", x.entry).as_bytes());
            for b in [&x.ff, &x.enc] {
                h = phitex_defuse::hash_words(h, &[b.as_ref().map_or(0, blob_hash)]);
            }
        }
        let mut files: Vec<_> = self.files.iter().collect();
        files.sort_unstable_by(|a, b| a.0.cmp(b.0));
        for (n, b) in files {
            h = phitex_defuse::hash_words(phitex_defuse::hash_bytes(h, n), &[blob_hash(b)]);
        }
        phitex_defuse::hash_words(h, &[u64::from(self.space.unwrap_or(0))])
    }
}

/// The content hash of a file's bytes, memoized by their allocation
/// (every page's fonts share the files' `Arc`s, [`file_bytes`]); a
/// `Weak` checks the allocation is still the one hashed. The empty
/// file hashes as no file.
fn blob_hash(b: &Arc<[u8]>) -> u64 {
    type Memo = Mutex<FxHashMap<usize, (std::sync::Weak<[u8]>, u64)>>;
    static M: OnceLock<Memo> = OnceLock::new();
    if b.is_empty() {
        return 0;
    }
    let key = Arc::as_ptr(b).cast::<u8>() as usize;
    let m = M.get_or_init(|| Mutex::new(FxHashMap::default()));
    if let Some((w, h)) = m.lock().unwrap_or_else(PoisonError::into_inner).get(&key)
        && w.upgrade().is_some_and(|a| Arc::ptr_eq(&a, b))
    {
        return *h;
    }
    let h = phitex_defuse::hash_bytes(0, b).max(1);
    let mut m = m.lock().unwrap_or_else(PoisonError::into_inner);
    if m.len() > 4096 {
        m.retain(|_, (w, _)| w.strong_count() > 0);
    }
    m.insert(key, (Arc::downgrade(b), h));
    h
}

/// `pdftex.map`'s `ps_tree`.
type PsMap = BTreeMap<(Vec<u8>, i32, i32), MapEntry>;

type PsCache = Once<Arc<PsMap>>;

fn ps_maps() -> &'static PsCache {
    static C: OnceLock<PsCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// `pdftex.map` by PostScript name: a pure function's memo.
pub(crate) fn default_ps_map(r: &mut Run<'_, '_>) -> R<Option<Arc<PsMap>>> {
    let v = r.v;
    let Some(x) = r.file("pdftex.map")? else {
        return Ok(None);
    };
    let Some(&m) = list_of(v, x).get(1) else {
        return Ok(None);
    };
    Ok(Some(once(ps_maps(), m.raw(), || {
        Arc::new(parse_map_ps(&bytes_of(&str_of(v, m))))
    })))
}

impl FontSource for Fonts {
    fn metrics(&self, f: FontId) -> &ScaledFont {
        static EMPTY: OnceLock<ScaledFont> = OnceLock::new();
        self.by_id.get(&f).map_or_else(
            || {
                EMPTY.get_or_init(|| ScaledFont {
                    size: 0,
                    design_size: 0,
                    chars: Vec::new(),
                    kerns: Vec::new(),
                    params: Vec::new(),
                })
            },
            |x| &x.metrics,
        )
    }
    fn name(&self, f: FontId) -> &[u8] {
        self.by_id.get(&f).map_or(&[], |x| &x.name)
    }
    fn map_entry(&self, f: FontId) -> Option<MapEntry> {
        self.by_id.get(&f)?.entry.clone()
    }
    fn font_file(&self, f: FontId) -> Option<Vec<u8>> {
        self.by_id.get(&f)?.ff.as_ref().map(|b| b.to_vec())
    }
    fn encoding_file(&self, f: FontId) -> Option<Vec<u8>> {
        self.by_id.get(&f)?.enc.as_ref().map(|b| b.to_vec())
    }
    fn file(&self, name: &[u8]) -> Option<Vec<u8>> {
        let f = self.files.get(name).map(|b| b.to_vec());
        if f.is_none() {
            self.missed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(name.to_vec());
        }
        f
    }
    fn map_by_ps_name(&self, ps_name: &[u8], slant: i32, extend: i32) -> Option<MapEntry> {
        self.ps
            .as_ref()?
            .get(&(ps_name.to_vec(), slant, extend))
            .cloned()
    }
    fn space_font(&self) -> Option<FontId> {
        self.space
    }
    fn virtual_font(&self, f: FontId) -> Option<VirtualFont> {
        self.vfs.get(&f).cloned()
    }
}

/// `store_scaled` of `vf_def_font` (pdfTeX's vfpacket code, as §575
/// reads a TFM's sizes): a `fix_word` `sq` times the size `z`.
fn store_scaled_f(sq: i32, z: i32) -> i32 {
    let mut z = z;
    let mut alpha = 16;
    while z >= 0o40000000 {
        z /= 2;
        alpha += alpha;
    }
    let beta = 256 / alpha;
    let alpha = alpha * z;
    let u = if sq >= 0 {
        sq.cast_unsigned()
    } else {
        (sq + 0x4000_0000 + 0x4000_0000).cast_unsigned()
    };
    let [a, b, c, d] = u.to_be_bytes();
    let a = if sq >= 0 { a } else { a.wrapping_add(128) };
    let (b, c, d) = (i32::from(b), i32::from(c), i32::from(d));
    let sw = (((((d * z) / 0o400) + (c * z)) / 0o400) + (b * z)) / beta;
    if a == 0 { sw } else { sw - alpha }
}

/// `do_vf`: font `f`'s virtual font read once, when the writer first
/// outputs one of its characters; its local fonts are the TeX fonts of
/// that name and size (`tfm_lookup`), else new ones (`read_font_info`:
/// the next font numbers, as pdfTeX's are). `pdf:vf:<f>` holds the
/// local fonts (`[k, font]`) of a virtual font; a font that is not
/// virtual defines nothing: its name goes to the build's memo
/// (`Ops::not_virtual`), and a reader finds no field (`None`: not
/// virtual).
pub(crate) fn do_vf(r: &mut Run<'_, '_>, f: u32) -> R<()> {
    let v = r.v;
    let key = format!("pdf:vf:{f}");
    if r.get(&key)?.is_some() {
        return Ok(());
    }
    let info = crate::fonts::info(v, r.font_get(f, crate::fields::FontCell::Info)?);
    let Some(i) = info.filter(|i| i.file.is_some()) else {
        return Ok(());
    };
    let memo = r.ops.not_virtual.clone();
    if memo
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&i.name)
    {
        return Ok(());
    }
    let not_virtual = |name: &str| {
        memo.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_owned());
    };
    let Some(bytes) = file_bytes(r, &format!("{}.vf", i.name))? else {
        not_virtual(&i.name);
        return Ok(());
    };
    let Ok(vf) = phitex_tfm::parse_vf(&bytes) else {
        not_virtual(&i.name);
        return Ok(());
    };
    let mut locals = Vec::new();
    for lf in &vf.fonts {
        let fs = store_scaled_f(lf.scaled, i.size);
        let name = String::from_utf8_lossy(&lf.name).into_owned();
        let count = r.cint(crate::fields::cell::FONTCOUNT)?;
        let mut found = None;
        for g in 1..=count {
            if let Some(j) = crate::fonts::info(v, r.font_get(g, crate::fields::FontCell::Info)?)
                && j.name == name
                && j.size == fs
            {
                found = Some(g);
                break;
            }
        }
        let g = match found {
            Some(g) => g,
            None => r.read_font_info(&name, fs)?,
        };
        locals.push(v.list(vec![v.int(i64::from(lf.number)), v.int(g)]));
    }
    r.set(&key, v.list(locals.clone()), true);
    for l in locals {
        let g = crate::val::int_at(v, l, 1);
        if let Ok(g) = u32::try_from(g)
            && g > 0
        {
            do_vf(r, g)?;
        }
    }
    Ok(())
}

/// `do_vf` for the fonts of `b`, in the order the writer outputs them.
pub(crate) fn vf_for(r: &mut Run<'_, '_>, b: &BoxNode) -> R<()> {
    fn walk(list: &[phitex_typeset::node::Node], out: &mut Vec<FontId>) {
        use phitex_typeset::node::Node;
        for n in list {
            match n {
                // (a run of characters in one font is one entry: the
                // fonts are taken in their first appearance's order)
                Node::Char { font, .. } => {
                    if out.last() != Some(font) {
                        out.push(*font);
                    }
                }
                Node::Lig(l) => {
                    if out.last() != Some(&l.font) {
                        out.push(l.font);
                    }
                }
                Node::Box(b) => walk(&b.list, out),
                Node::Glue(g) => {
                    if let Some(l) = &g.leader {
                        walk(std::slice::from_ref(&**l), out);
                    }
                }
                Node::Disc(d) => walk(&d.replace, out),
                _ => {}
            }
        }
    }
    let mut fonts = Vec::new();
    walk(&b.list, &mut fonts);
    let mut seen = BTreeSet::new();
    for f in fonts {
        if seen.insert(f) {
            do_vf(r, f)?;
        }
    }
    Ok(())
}

/// A file's bytes, `None` if absent.
pub(crate) fn file_bytes(r: &mut Run<'_, '_>, name: &str) -> R<Option<Arc<[u8]>>> {
    static M: OnceLock<Once<Arc<[u8]>>> = OnceLock::new();
    let v = r.v;
    Ok(r.file(name)?.and_then(|x| {
        let &t = list_of(v, x).get(1)?;
        // (a pure function's memo by the text's value: every page step
        // asks for its fonts' files)
        let m = M.get_or_init(|| Mutex::new(FxHashMap::default()));
        Some(once(m, t.raw(), || Arc::from(bytes_of(&str_of(v, t)))))
    }))
}

/// `pdftex.map`, parsed.
fn default_map(r: &mut Run<'_, '_>) -> R<Option<Arc<Map>>> {
    let v = r.v;
    let Some(x) = r.file("pdftex.map")? else {
        return Ok(None);
    };
    let Some(&m) = list_of(v, x).get(1) else {
        return Ok(None);
    };
    Ok(Some(once(maps(), m.raw(), || {
        Arc::new(parse_map(&bytes_of(&str_of(v, m))))
    })))
}

/// The fonts `ids` as the writer asks for them, with the files of the
/// map lines read so far; `space`: with `pdftexspace` too.
#[allow(clippy::too_many_lines)]
pub(crate) fn fonts_for(r: &mut Run<'_, '_>, ids: &[u32], space: bool) -> R<Fonts> {
    let v = r.v;
    let map = default_map(r)?;
    let mut out = Fonts::default();
    let mut wanted: Vec<(u32, String, ValueId, i32)> = Vec::new();
    // the virtual fonts' local fonts too
    let mut all: Vec<u32> = ids.to_vec();
    let mut k = 0;
    while k < all.len() {
        let f = all[k];
        k += 1;
        let Some(x) = r.get(&format!("pdf:vf:{f}"))? else {
            continue;
        };
        let locals = list_of(v, x);
        if locals.is_empty() {
            continue;
        }
        let mut fonts = Vec::new();
        for l in locals {
            let n = u32::try_from(crate::val::int_at(v, l, 0)).unwrap_or(0);
            let g = u32::try_from(crate::val::int_at(v, l, 1)).unwrap_or(0);
            fonts.push((n, g));
            if !all.contains(&g) {
                all.push(g);
            }
        }
        let name =
            crate::fonts::info(v, r.font_get(f, crate::fields::FontCell::Info)?).map(|i| i.name);
        if let Some(name) = name
            && let Some(b) = file_bytes(r, &format!("{name}.vf"))?
            && let Ok(vf) = phitex_tfm::parse_vf(&b)
        {
            let packets = vf
                .chars
                .iter()
                .filter_map(|c| Some((u8::try_from(c.code).ok()?, c.dvi.to_vec())))
                .collect();
            out.vfs.insert(f, VirtualFont { fonts, packets });
        }
    }
    let ids = &all[..];
    for &f in ids {
        if let Some(i) = crate::fonts::info(v, r.font_get(f, crate::fields::FontCell::Info)?)
            && let Some(file) = i.file
        {
            wanted.push((f, i.name, file, i.s));
        }
    }
    if space && let Some(x) = r.file("pdftexspace.tfm")? {
        // pdfTeX loads the space font at the shipout that first needs
        // it, as the next internal font number (the engine's
        // `fontcount` does not count it)
        let id = if crate::pdfrelaxed::on(r)? {
            let n = r.cint(crate::fields::cell::FONTCOUNT)?;
            u32::try_from(n + 1).unwrap_or(u32::MAX - 1)
        } else {
            // a page written apart: one id for every page, outside the
            // fonts' range, numbered by the link where it is first
            // loaded ([`FontTags`])
            SPACE_FONT
        };
        wanted.push((id, "pdftexspace".to_owned(), x, -1000));
        out.space = Some(id);
    }
    for (id, name, file, s) in wanted {
        let Some(l) = crate::fonts::load(v, file, s) else {
            continue;
        };
        let entry = if Some(id) == out.space {
            // `mapfile.c`: the line pdfTeX adds for its space font
            parse_line(b"pdftexspace PdfTeX-Space <pdftexspace.pfb")
        } else {
            map.as_ref().and_then(|m| m.get(name.as_bytes()).cloned())
        };
        let mut get = |n: Option<&[u8]>| -> R<Option<Arc<[u8]>>> {
            match n {
                Some(n) => file_bytes(r, &String::from_utf8_lossy(n)),
                None => Ok(None),
            }
        };
        let ff = get(entry.as_ref().and_then(|e| e.ff_name.as_deref()))?;
        let enc = get(entry.as_ref().and_then(|e| e.encname.as_deref()))?;
        out.by_id.insert(
            id,
            Font {
                name: name.into_bytes(),
                metrics: l.at.clone(),
                entry,
                ff,
                enc,
            },
        );
    }
    // the files of `\pdfmapline`s and `\pdfmapfile`s
    for e in r
        .get_loc(crate::fields::cell::PDF_MAPS)?
        .map(|x| list_of(v, x))
        .unwrap_or_default()
    {
        let ys = list_of(v, e);
        let file = ys.first().map(|&k| str_of(v, k)).as_deref() == Some("file");
        let text = ys.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
        let t = text.trim_start_matches(['+', '=', '-', ' ']);
        let mut lines: Vec<MapEntry> = Vec::new();
        if file {
            let name = t.split(' ').next().unwrap_or_default().to_owned();
            if let Some(b) = file_bytes(r, &name)? {
                out.files.insert(name.into_bytes(), b.clone());
                lines.extend(parse_map(&b).into_values());
            }
        } else {
            lines.extend(parse_line(&bytes_of(t)));
        }
        for m in lines {
            for n in [m.ff_name.clone(), m.encname.clone()].into_iter().flatten() {
                if !out.files.contains_key(&n)
                    && let Some(b) = file_bytes(r, &String::from_utf8_lossy(&n))?
                {
                    out.files.insert(n, b);
                }
            }
        }
    }
    Ok(out)
}

/// A fresh writer state: nothing written yet.
fn fresh(v: &Values, date: String, jobname: &str, g2u: Option<ValueId>) -> PdfState {
    let p = PdfParams {
        date,
        banner: BANNER.to_owned(),
        output_file_name: format!("{jobname}.pdf"),
        ..PdfParams::default()
    };
    let mut st = PdfState::new(p);
    // the `\pdfglyphtounicode` table a format carries (`dumptounicode`,
    // pdfTeX §1502; `undumptounicode`, §1503): a fresh writer state, a
    // format loaded, starts from it
    if let Some(t) = g2u {
        for e in list_of(v, t) {
            let ab = list_of(v, e);
            if let [a, b] = ab[..] {
                st.glyph_to_unicode(&bytes_of(&str_of(v, a)), &bytes_of(&str_of(v, b)));
            }
        }
    }
    st
}

/// A read-time command on the writer in PDF mode, with the fonts it
/// names: recorded in the line ([`XPAGES`]) where it runs, its step `f`
/// kept for the link ([`Cmd`]); what the link gave it the round before
/// ([`OBJNUMS`]), `None` before the link has (the caller's guess
/// stands: TeX's own count, and the round after the link runs again
/// what read another).
pub(crate) fn command(
    r: &mut Run<'_, '_>,
    input: ValueId,
    fonts: &[u32],
    mut f: impl FnMut(&mut PdfState, &Fonts) -> CmdOut + Send + 'static,
) -> R<Option<CmdOut>> {
    command_forms(r, input, fonts, move |st, _, fs| f(st, fs))
}

/// [`command`], the forms' table in reach.
pub(crate) fn command_forms(
    r: &mut Run<'_, '_>,
    input: ValueId,
    fonts: &[u32],
    f: impl FnMut(&mut PdfState, &mut Forms, &Fonts) -> CmdOut + Send + 'static,
) -> R<Option<CmdOut>> {
    if r.param("pdfoutput")? <= 0 {
        return Ok(None);
    }
    if crate::pdfrelaxed::on(r)? {
        crate::pdfrelaxed::fall_back(r, "a command the relaxed mode does not cover");
        return Ok(None);
    }
    let v = r.v;
    let fs = fonts_for(r, fonts, false)?;
    let wparams = writer_params(r)?;
    // (the command's key: its node, its place among the node's
    // commands, what it is; the same in every round the node runs as
    // this run)
    r.pdf_occ += 1;
    let key = phitex_defuse::hash_words(
        0x6364_6d6b_6579_0001,
        &[r.cx.node().raw(), r.pdf_occ as u64, input.raw()],
    )
    .max(1);
    let serial = CMD_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    r.ops
        .pdf_cmds
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(
            serial,
            Arc::new(Cmd {
                f: Mutex::new(Box::new(f)),
                fonts: fs,
                wparams,
            }),
        );
    let got = objnums_of(r, key)?;
    let e = v.list(vec![
        v.str("cmd"),
        v.int(i64::from_ne_bytes(key.to_ne_bytes())),
        v.int(i64::from_ne_bytes(serial.to_ne_bytes())),
    ]);
    r.contribute(XPAGES, v.seq_unit(e));
    Ok(got)
}

/// The commands' serials ([`command`]): one per run of a command.
static CMD_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

type ObjnumsMemo = Mutex<FxHashMap<(u64, u64), Arc<FxHashMap<u64, CmdOut>>>>;

/// What the link gave command `key` the round before ([`OBJNUMS`]).
fn objnums_of(r: &mut Run<'_, '_>, key: u64) -> R<Option<CmdOut>> {
    static M: OnceLock<ObjnumsMemo> = OnceLock::new();
    let v = r.v;
    let f = v.field(OBJNUMS);
    if !r.cx.is_loopback(f) {
        return Ok(None);
    }
    let Some(x) = r.eng_read(f)? else {
        return Ok(None);
    };
    // (decoded once a value: a pure function's memo by its id)
    let m = M.get_or_init(|| Mutex::new(FxHashMap::default()));
    let k = (v.instance(), x.raw());
    let hit = m
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&k)
        .cloned();
    let map = if let Some(h) = hit {
        h
    } else {
        let mut out = FxHashMap::default();
        for e in list_of(v, x) {
            let xs = list_of(v, e);
            if let (Some(&kx), Some(&o)) = (xs.first(), xs.get(1)) {
                out.insert(
                    u64::from_ne_bytes(int_of(v, kx).to_ne_bytes()),
                    CmdOut::of_value(v, o),
                );
            }
        }
        let a = Arc::new(out);
        let mut g = m.lock().unwrap_or_else(PoisonError::into_inner);
        if g.len() > 1 << 12 {
            g.clear();
        }
        g.insert(k, a.clone());
        a
    };
    Ok(map.get(&key).cloned())
}

fn param(v: &Values, xs: &[ValueId], k: usize) -> i32 {
    xs.get(k)
        .map_or(0, |&y| i32::try_from(int_of(v, y)).unwrap_or(0))
}

/// The page's parameters as `\shipout` read them.
pub(crate) fn page_params(v: &Values, p: &mut PdfParams, xs: &[ValueId]) {
    p.h_origin = param(v, xs, 1);
    p.v_origin = param(v, xs, 2);
    p.page_width = param(v, xs, 3);
    p.page_height = param(v, xs, 4);
    p.h_offset = param(v, xs, 5);
    p.v_offset = param(v, xs, 6);
    p.compress_level = param(v, xs, 7).clamp(0, 9);
    p.decimal_digits = param(v, xs, 8);
    p.minor_version = param(v, xs, 9);
    p.major_version = param(v, xs, 10);
    p.mag = param(v, xs, 11);
    p.obj_compress_level = param(v, xs, 13);
    p.gen_tounicode = param(v, xs, 14);
    let text = |k: usize| {
        xs.get(k)
            .map(|&t| bytes_of(&str_of(v, t)))
            .filter(|t| !t.is_empty())
    };
    p.page_attr = text(15);
    p.pages_attr = text(16);
    p.page_resources = text(17);
}

/// The writer's data for a whatsit `\shipout` resolved
/// ([`crate::pdfprim::writer_data`]).
pub(crate) fn whatsit(v: &Values, d: ValueId) -> Option<Whatsit> {
    let ys = list_of(v, d);
    let kind = ys.first().map(|&k| str_of(v, k)).unwrap_or_default();
    let text = |k: usize| {
        ys.get(k)
            .map(|&t| bytes_of(&str_of(v, t)))
            .unwrap_or_default()
    };
    let int = |k: usize| ys.get(k).map_or(0, |&n| int_of(v, n));
    Some(match kind.as_str() {
        "lit" => Whatsit::Literal(
            match int(1) {
                2 => LiteralMode::DirectAlways,
                1 => LiteralMode::DirectPage,
                _ => LiteralMode::SetOrigin,
            },
            text(2),
        ),
        "cs" => Whatsit::ColorStack(
            usize::try_from(int(1)).unwrap_or(usize::MAX),
            match str_of(v, ys.get(2).copied()?).as_str() {
                "set" => ColorStackCmd::Set(text(3)),
                "push" => ColorStackCmd::Push(text(3)),
                "pop" => ColorStackCmd::Pop,
                _ => ColorStackCmd::Current,
            },
        ),
        "mx" => Whatsit::SetMatrix(text(1)),
        "save" => Whatsit::Save,
        "iwon" => Whatsit::InterwordSpaceOn,
        "iwoff" => Whatsit::InterwordSpaceOff,
        "fake" => Whatsit::FakeSpace,
        "rloff" => Whatsit::RunningLinkOff,
        "rlon" => Whatsit::RunningLinkOn,
        "restore" => Whatsit::Restore,
        // pdfTeX §1366 ff.: the nodes as `\pdfrefobj` ... `\pdfendthread`
        // made them ([`crate::pdfprim::extension`])
        "pdf:refobj" => Whatsit::RefObj(usize::try_from(int(1)).unwrap_or(0)),
        "pdf:refxform" => Whatsit::RefXForm(usize::try_from(int(1)).unwrap_or(0)),
        "pdf:refximage" => Whatsit::RefXImage(usize::try_from(int(1)).unwrap_or(0)),
        "pdf:annot" => Whatsit::Annot {
            objnum: usize::try_from(int(1)).unwrap_or(0),
            width: dim(int(2)),
            height: dim(int(3)),
            depth: dim(int(4)),
            data: text(5),
        },
        "pdf:startlink" => Whatsit::StartLink {
            objnum: usize::try_from(int(1)).unwrap_or(0),
            width: dim(int(2)),
            height: dim(int(3)),
            depth: dim(int(4)),
            attr: some(text(5)),
            action: action(v, ys.get(6).copied()?)?,
        },
        "pdf:endlink" => Whatsit::EndLink,
        "pdf:dest" => {
            let k = list_of(v, ys.get(2).copied()?);
            let kn = k.first().map(|&x| str_of(v, x)).unwrap_or_default();
            let ki = |j: usize| dim(k.get(j).map_or(0, |&x| int_of(v, x)));
            Whatsit::Dest {
                id: ident(v, ys.get(1).copied()?),
                struct_num: None,
                kind: match kn.as_str() {
                    "fit" => DestKind::Fit,
                    "fith" => DestKind::FitH,
                    "fitv" => DestKind::FitV,
                    "fitb" => DestKind::FitB,
                    "fitbh" => DestKind::FitBH,
                    "fitbv" => DestKind::FitBV,
                    "fitr" => DestKind::FitR(ki(1), ki(2), ki(3)),
                    _ => DestKind::Xyz((k.len() > 1).then(|| ki(1))),
                },
            }
        }
        "pdf:thread" | "pdf:startthread" => Whatsit::Thread {
            start: kind == "pdf:startthread",
            id: ident(v, ys.get(1).copied()?),
            width: dim(int(2)),
            height: dim(int(3)),
            depth: dim(int(4)),
            attr: some(text(5)),
        },
        "pdf:endthread" => Whatsit::EndThread,
        "pdf:savepos" => Whatsit::SavePos,
        _ => return None,
    })
}

fn dim(x: i64) -> i32 {
    i32::try_from(x).unwrap_or(0)
}

/// `attr{...}`: absent when empty.
fn some(t: Vec<u8>) -> Option<Vec<u8>> {
    (!t.is_empty()).then_some(t)
}

/// An identifier as [`crate::pdfprim`] scans it: `["num", n]` or
/// `["name", text]`.
fn ident(v: &Values, x: ValueId) -> Ident {
    let ys = list_of(v, x);
    match ys.first().map(|&k| str_of(v, k)).as_deref() {
        Some("name" | "struct") => Ident::Name(
            ys.get(1)
                .map(|&t| bytes_of(&str_of(v, t)))
                .unwrap_or_default(),
        ),
        _ => Ident::Num(dim(ys.get(1).map_or(0, |&n| int_of(v, n)))),
    }
}

/// An action as `scan_action` read it (pdfTeX §1360): `["user", text]`
/// or `[kind, file, page, id, window]`.
fn action(v: &Values, x: ValueId) -> Option<Action> {
    let ys = list_of(v, x);
    let kind = ys.first().map(|&k| str_of(v, k)).unwrap_or_default();
    let text = |k: usize| {
        ys.get(k)
            .map(|&t| bytes_of(&str_of(v, t)))
            .unwrap_or_default()
    };
    if kind == "user" {
        return Some(Action::User(text(1)));
    }
    let file = some(text(1));
    let window = u8::try_from(ys.get(4).map_or(0, |&n| int_of(v, n))).unwrap_or(0);
    match kind.as_str() {
        "goto" => {
            let page = ys.get(2).copied().filter(|&p| p != ValueId::UNIT);
            if let Some(p) = page {
                let ps = list_of(v, p);
                return Some(Action::Page {
                    file,
                    page: dim(ps.first().map_or(0, |&n| int_of(v, n))),
                    spec: ps
                        .get(1)
                        .map(|&t| bytes_of(&str_of(v, t)))
                        .unwrap_or_default(),
                    new_window: window,
                    struct_id: None,
                });
            }
            Some(Action::Goto {
                file,
                id: ident(v, ys.get(3).copied()?),
                new_window: window,
                struct_id: None,
            })
        }
        "thread" => Some(Action::Thread {
            file,
            id: ident(v, ys.get(3).copied()?),
        }),
        _ => None,
    }
}

/// The writer's whatsit of a node's data as `\shipout` resolves it, or
/// `None` for what the writer does not see.
pub(crate) fn writer_whatsit(r: &mut Run<'_, '_>, w: ValueId) -> R<Option<Whatsit>> {
    let v = r.v;
    let xs = list_of(v, w);
    if xs.first().map(|&k| str_of(v, k)).as_deref() == Some("special") {
        return Ok(Some(Whatsit::Special(bytes_of(
            &xs.get(1).map(|&s| str_of(v, s)).unwrap_or_default(),
        ))));
    }
    Ok(crate::pdfprim::writer_data(r, &xs)?.and_then(|d| whatsit(v, d)))
}

/// The action of `\pdfcatalog ... openaction` (pdfTeX §1564).
pub(crate) fn action_of(v: &Values, x: ValueId) -> Option<Action> {
    action(v, x)
}

/// `pdf_ship_out` as a step: `page` is the `pages` entry (box, counts,
/// fonts, parameters, whatsit data).
#[allow(clippy::too_many_lines)]
pub(crate) fn ship(
    r: &mut Run<'_, '_>,
    page: ValueId,
    bx: &BoxNode,
    side: &Side,
    ids: &[u32],
) -> R<u64> {
    let v = r.v;
    let xs = list_of(v, page);
    let p = xs.get(3).map(|&l| list_of(v, l)).unwrap_or_default();
    let mut resolved: FxHashMap<u32, ValueId> = FxHashMap::default();
    for e in xs.get(4).map(|&l| list_of(v, l)).unwrap_or_default() {
        let ys = list_of(v, e);
        if let (Some(&h), Some(&d)) = (ys.first(), ys.get(1)) {
            resolved.insert(u32::try_from(int_of(v, h)).unwrap_or(0), d);
        }
    }
    let t5 = std::time::Instant::now();
    vf_for(r, bx)?;
    phitex_pdf::phase::add(5, t5);
    let space = resolved.values().any(|&d| {
        matches!(
            list_of(v, d).first().map(|&k| str_of(v, k)).as_deref(),
            Some("iwon" | "fake")
        )
    });
    if crate::pdfrelaxed::on(r)? {
        let t6 = std::time::Instant::now();
        let fonts = fonts_for(r, ids, space)?;
        phitex_pdf::phase::add(6, t6);
        let whats = side.whats.clone();
        let mut wh = |h: u32| -> Option<Whatsit> {
            if let Some(&d) = resolved.get(&h) {
                return whatsit(v, d);
            }
            let w = *whats.get(h as usize)?;
            let ys = list_of(v, w);
            (ys.first().map(|&k| str_of(v, k)).as_deref() == Some("special")).then(|| {
                Whatsit::Special(bytes_of(
                    &ys.get(1).map(|&s| str_of(v, s)).unwrap_or_default(),
                ))
            })
        };
        let shown = crate::pdfrelaxed::ship(r, &p, xs[0], bx, &fonts, &mut wh)?;
        let attr = p.get(15).map(|&a| str_of(v, a)).unwrap_or_default();
        return Ok(phitex_defuse::hash_bytes(shown, attr.as_bytes()));
    }
    // the fonts of the forms the page may ship too
    let mut all: BTreeSet<u32> = ids.iter().copied().collect();
    all.extend(form_fonts(r)?);
    let all: Vec<u32> = all.into_iter().collect();
    let mut fonts = fonts_for(r, &all, space)?;
    let images = resolved
        .values()
        .any(|&d| list_of(v, d).first().map(|&k| str_of(v, k)).as_deref() == Some("pdf:refximage"));
    if images {
        // an included PDF's fonts replaced by the map's (`copyFont`
        // off): the files a dry run of the page asks for, read now
        fonts.ps = default_ps_map(r)?;
        let wp = writer_params(r)?;
        let (_, a) = ship_base(r, &p, wp)?;
        let mut st = a.st;
        page_params(v, st.params_mut(), &p);
        let mut wh =
            |h: u32, _l: bool| -> Option<Whatsit> { resolved.get(&h).and_then(|&d| whatsit(v, d)) };
        let _ = st.ship_out_with(bx, &fonts, &mut wh);
        for n in std::mem::take(&mut *fonts.missed.lock().unwrap_or_else(PoisonError::into_inner)) {
            if let Some(b) = file_bytes(r, &String::from_utf8_lossy(&n))? {
                fonts.files.insert(n, b);
            }
        }
    }
    let margins = [
        r.param("pdflinkmargin")?,
        r.param("pdfdestmargin")?,
        r.param("pdfthreadmargin")?,
    ];
    ship_apart(r, page, &p, bx, &side.whats, &resolved, fonts, margins)
}

/// The page's whatsits as the writer asks for them: a form's from the
/// forms' table, the page's from its data.
fn page_whatsit(forms: &Forms, whats: &BTreeMap<u32, Whatsit>, h: u32) -> Option<Whatsit> {
    if h >= FORM_HANDLES {
        return forms.whats.get(&h).cloned();
    }
    whats.get(&h).cloned()
}

/// The page's parameters on `st`, as the fold's step sets them.
fn set_page_params(v: &Values, st: &mut PdfState, params: &[ValueId], margins: [i32; 3]) {
    page_params(v, st.params_mut(), params);
    let pm = st.params_mut();
    pm.link_margin = margins[0];
    pm.dest_margin = margins[1];
    pm.thread_margin = margins[2];
}

/// The page written on its own (no other page's state read): kept as a
/// [`PagePiece`] for the `Dvi` node, which ships the pages in order;
/// written once here, on the state the read-time commands left, so that
/// its content stream is compressed in parallel with the other pages'
/// and the hash of what it shows is known.
#[allow(clippy::too_many_arguments)]
fn ship_apart(
    r: &mut Run<'_, '_>,
    page_v: ValueId,
    p: &[ValueId],
    bx: &BoxNode,
    side_whats: &[ValueId],
    resolved: &FxHashMap<u32, ValueId>,
    fonts: Fonts,
    margins: [i32; 3],
) -> R<u64> {
    let v = r.v;
    let mut whats: BTreeMap<u32, Whatsit> = BTreeMap::new();
    for (&h, &d) in resolved {
        if let Some(w) = whatsit(v, d) {
            whats.insert(h, w);
        }
    }
    for (h, &w) in side_whats.iter().enumerate() {
        let Ok(h) = u32::try_from(h) else { break };
        if whats.contains_key(&h) || resolved.contains_key(&h) {
            continue;
        }
        let ys = list_of(v, w);
        if ys.first().map(|&k| str_of(v, k)).as_deref() == Some("special") {
            whats.insert(
                h,
                Whatsit::Special(bytes_of(
                    &ys.get(1).map(|&s| str_of(v, s)).unwrap_or_default(),
                )),
            );
        }
    }
    let wparams = writer_params(r)?;
    let mut pc = PagePiece {
        wparams,
        boxv: list_of(v, page_v).first().copied().unwrap_or(ValueId::UNIT),
        params: p.to_vec(),
        margins,
        whats,
        fonts,
        rec: phitex_pdf::PageRecord::default(),
        base: BTreeSet::new(),
        pj: i64::try_from(r.page_index()).unwrap_or(i64::MAX),
        fcount: r.cint(crate::fields::cell::FONTCOUNT)?,
        err: None,
    };
    // the key: what the piece is made of, by the values' identities
    // (interned: equal values, equal ids), not by their bytes
    let (k0, base) = ship_base(r, p, pc.wparams)?;
    let maps = r
        .get_loc(crate::fields::cell::PDF_MAPS)?
        .map_or(0, ValueId::raw);
    let pdfmap = r.file("pdftex.map")?.map_or(0, ValueId::raw);
    let mut words = vec![
        page_v.raw(),
        k0,
        maps,
        pdfmap,
        u64::from(pc.fonts.space.is_some()),
        pc.pj.cast_unsigned(),
        pc.fcount.cast_unsigned(),
    ];
    words.extend(pc.margins.iter().map(|&m| u64::from(m.cast_unsigned())));
    words.extend(pc.wparams.iter().map(|&m| u64::from(m.cast_unsigned())));
    let key = phitex_defuse::hash_words(0x7067_7069_6563_6531, &words).max(1);
    // the page written once, apart, on a writer of its own (`ship_base`)
    let Fold { mut st, forms } = base;
    set_writer_params(&mut st, pc.wparams);
    set_page_params(v, &mut st, &pc.params, pc.margins);
    pc.base = st.fonts_used();
    let (res, rec) = {
        let mut wh = |h: u32, _leaders: bool| page_whatsit(&forms, &pc.whats, h);
        st.ship_recorded(bx, &pc.fonts, &mut wh)
    };
    pc.rec = rec;
    let err = res.err();
    let (content, w, h) = st.last_page_shown();
    let words = [
        u64::from_ne_bytes(i64::from(w).to_ne_bytes()),
        u64::from_ne_bytes(i64::from(h).to_ne_bytes()),
    ];
    let shown = phitex_defuse::hash_words(phitex_defuse::hash_bytes(0, content), &words);
    let attr = p.get(15).map(|&a| str_of(v, a)).unwrap_or_default();
    let shown = phitex_defuse::hash_bytes(shown, attr.as_bytes());
    // (the writer's error goes with the page: the job's where the end
    // node places it, [`placed_errors`])
    pc.err = err.map(|e| {
        (
            format!("pdfTeX error: {e}"),
            i64::from_ne_bytes(r.cx.node().raw().to_ne_bytes()),
        )
    });
    pieces()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key)
        .or_insert_with(|| Arc::new(pc));
    // the page's place in the file: its record in the line, where it
    // shipped; the link numbers its objects after every record before
    // it and before every one after
    let k = v.int(i64::from_ne_bytes(key.to_ne_bytes()));
    r.contribute(XPAGES, v.seq_unit(k));
    Ok(shown)
}

/// The loop-back of the fonts' names the finish gave
/// ([`FontTags`]): `[[engine number, name number], ...]` where they
/// differ.
pub(crate) const FONT_TAGS: &str = "pdf:ftags";

/// Font `n`'s name number as the finish gives it (`\pdffontname`):
/// the previous round's, a loop-back (exact mode).
pub(crate) fn font_tag_read(r: &mut Run<'_, '_>, n: i64) -> R<i64> {
    let v = r.v;
    let f = v.field(FONT_TAGS);
    if !r.cx.is_loopback(f) || crate::pdfrelaxed::on(r)? {
        return Ok(n);
    }
    let x = r.eng_read(f)?;
    Ok(x.map(|x| list_of(v, x))
        .unwrap_or_default()
        .iter()
        .map(|&e| list_of(v, e))
        .find(|e| e.first().is_some_and(|&a| int_of(v, a) == n))
        .and_then(|e| e.get(1).map(|&b| int_of(v, b)))
        .unwrap_or(n))
}

/// The id of `pdftexspace` in the pages written apart (none of the
/// engine's: its fonts are numbered from 1 by `fontcount`).
pub(crate) const SPACE_FONT: FontId = FontId::MAX - 15;

/// The fixed parts of the exact file the end node reads: the fields
/// reaching it.
pub(crate) struct EndFields {
    pub date: String,
    pub jobname: String,
    pub g2u: Option<ValueId>,
    /// `fontcount`: the fonts loaded in all.
    pub fonts: i64,
    pub info_omit_date: i32,
    pub suppress_ptex_info: i32,
}

/// The writer's errors of the pages the end node places (`xpages`), with
/// the node that wrote each: the job's errors ([`PagePiece::err`]).
pub(crate) fn placed_errors(v: &Values, xpages: &[ValueId]) -> Vec<(String, i64)> {
    let m = pieces().lock().unwrap_or_else(PoisonError::into_inner);
    xpages
        .iter()
        .filter(|&&x| v.as_int(x).is_some())
        .filter_map(|&x| m.get(&u64::from_ne_bytes(int_of(v, x).to_ne_bytes())))
        .filter_map(|p| p.err.clone())
        .collect()
}

/// What the link made: the file (empty: none), and the loop-backs it
/// gives the next round: what each read-time command got ([`OBJNUMS`])
/// and the fonts' names ([`FONT_TAGS`]).
pub(crate) struct Linked {
    pub file: Vec<u8>,
    pub objnums: ValueId,
    pub ftags: ValueId,
}

/// The exact PDF file, linked (spec-redesign §1 `link`): one writer
/// state from the start, the line's records put on it in order
/// ([`XPAGES`]): a page's record placed (`PdfState::replay_page`, its
/// objects numbered after every record before it), or shipped again
/// from its box where the replay cannot give the walk's bytes; a
/// read-time command's step applied ([`Cmd`]); a document-level
/// command applied ([`apply_doc_op`]). Each record's bytes are a chunk;
/// the fonts' names in pdfTeX's order over the pages ([`FontTags`]);
/// then the tail (the fonts embedded, the pages tree: by the final
/// state's digest) and the cross-reference section, by the prefix sums
/// of the chunks' lengths (`phitex_pdf::Tail::link`).
#[allow(clippy::too_many_lines)]
pub(crate) fn finish_exact(
    v: &Values,
    f: &EndFields,
    xpages: &[ValueId],
    cmds: &CmdMemo,
) -> Linked {
    let none = Linked {
        file: Vec::new(),
        objnums: v.list(Vec::new()),
        ftags: v.list(Vec::new()),
    };
    if xpages.is_empty() {
        return none;
    }
    let t0 = std::time::Instant::now();
    let mut st = fresh(v, f.date.clone(), &f.jobname, f.g2u);
    let mut forms = Forms::default();
    let mut tags = FontTags::default();
    let mut bytes = PdfBytes::new();
    let mut got: Vec<ValueId> = Vec::new();
    let (mut placed, mut shipped) = (0usize, 0usize);
    let pcs: Vec<Option<Arc<PagePiece>>> = {
        let m = pieces().lock().unwrap_or_else(PoisonError::into_inner);
        xpages
            .iter()
            .map(|&x| {
                v.as_int(x)
                    .and_then(|k| m.get(&u64::from_ne_bytes(k.to_ne_bytes())).cloned())
            })
            .collect()
    };
    // (the serials given out so far: a command run after this point is
    // not one this link can have dropped)
    let hi = CMD_SERIAL.load(std::sync::atomic::Ordering::Relaxed);
    let serials: FxHashMap<u64, Arc<Cmd>> = {
        let mut m = cmds.lock().unwrap_or_else(PoisonError::into_inner);
        let serials: FxHashMap<u64, Arc<Cmd>> = xpages
            .iter()
            .filter_map(|&x| {
                let xs = list_of(v, x);
                (xs.first().map(|&k| str_of(v, k)).as_deref() == Some("cmd"))
                    .then(|| {
                        xs.get(2)
                            .map(|&s| u64::from_ne_bytes(int_of(v, s).to_ne_bytes()))
                    })
                    .flatten()
            })
            .filter_map(|s| m.get(&s).map(|c| (s, c.clone())))
            .collect();
        // (the runs whose records the line no longer holds, a re-run's
        // old one, a discarded speculation's: the link waits for every
        // contributor, so none of them is still to come)
        if m.len() > 2 * serials.len() + 64 {
            m.retain(|s, _| *s > hi || serials.contains_key(s));
        }
        serials
    };
    for (&x, pc) in xpages.iter().zip(&pcs) {
        if v.as_int(x).is_some() {
            // a page: no piece in the memo is a page written by a run
            // the engine threw away; nothing to place
            let Some(pc) = pc else {
                return none;
            };
            set_writer_params(&mut st, pc.wparams);
            // the fonts' names as pdfTeX numbers them, over the pages
            // placed so far (a page's numbers never change by a later
            // page)
            tags.page(pc);
            if let Some(t) = tags.names() {
                st.set_font_tags(t);
            }
            set_page_params(v, &mut st, &pc.params, pc.margins);
            let n0 = st.objects_numbered();
            let res = st.replay_page(&pc.rec, &pc.fonts, &pc.base, None);
            if res.is_err() {
                // (the node that wrote the page logged the writer's
                // error when it wrote it)
                let mut wh = |h: u32, _leaders: bool| page_whatsit(&forms, &pc.whats, h);
                let mut side = crate::nodes::Side::default();
                if let Some(phitex_typeset::node::Node::Box(bx)) =
                    crate::nodes::decode(v, pc.boxv, &mut side)
                {
                    let _ = st.ship_out_with(&bx, &pc.fonts, &mut wh);
                }
                shipped += 1;
            } else {
                placed += 1;
            }
            if crate::exec::env_flag!("PHITEX_PDF_TRACE") {
                eprintln!(
                    "  link page {}: objects {n0} -> {} ({})",
                    pc.pj,
                    st.objects_numbered(),
                    match &res {
                        Ok(_) => "replayed".to_owned(),
                        Err(e) => format!("shipped again: {e}"),
                    }
                );
            }
        } else {
            let xs = list_of(v, x);
            if xs.first().map(|&k| str_of(v, k)).as_deref() == Some("cmd") {
                let key = xs.get(1).copied().unwrap_or(ValueId::UNIT);
                let serial = xs
                    .get(2)
                    .map_or(0, |&s| u64::from_ne_bytes(int_of(v, s).to_ne_bytes()));
                let Some(c) = serials.get(&serial) else {
                    return none;
                };
                set_writer_params(&mut st, c.wparams);
                let out = {
                    let mut g = c.f.lock().unwrap_or_else(PoisonError::into_inner);
                    (*g)(&mut st, &mut forms, &c.fonts)
                };
                got.push(v.list(vec![key, out.value(v)]));
            } else {
                apply_doc_op(v, &mut st, x);
            }
        }
        if let Some(c) = st.flush_chunk() {
            bytes.push(c);
        }
    }
    // the fonts loaded after the last page named after them
    tags.end(f.fonts);
    if let Some(t) = tags.names() {
        st.set_font_tags(t);
    }
    let ftags = {
        let mut m: Vec<(FontId, FontId)> = tags.map.iter().map(|(&a, &b)| (a, b)).collect();
        m.sort_unstable();
        v.list(
            m.into_iter()
                .map(|(a, b)| v.list(vec![v.int(i64::from(a)), v.int(i64::from(b))]))
                .collect(),
        )
    };
    if let Some(c) = st.flush_chunk() {
        bytes.push(c);
    }
    st.params_mut().info_omit_date = f.info_omit_date;
    st.params_mut().suppress_ptex_info = f.suppress_ptex_info;
    // the end of the file: its objects (the fonts embedded, the pages
    // tree, ...) a function of the state alone, kept by its digest (an
    // edit that changes no font, page count or table writes them once);
    // then the chunks placed by their lengths' prefix sums and the
    // cross-reference section
    let key = st.digest();
    let t1 = std::time::Instant::now();
    let kept = tails()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned();
    let tail = match kept {
        Some(t) => Ok(Some(t)),
        None => st.tail().map(|t| {
            let t = t.map(Arc::new);
            if let Some(t) = &t {
                tails()
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(key, t.clone());
            }
            t
        }),
    };
    let t2 = std::time::Instant::now();
    let out = tail
        .and_then(|t| t.map_or(Ok(Vec::new()), |t| t.link(&bytes)))
        .unwrap_or_else(|e| {
            if crate::exec::env_flag!("PHITEX_TEX_TIME") {
                eprintln!("pdf: {e}");
            }
            Vec::new()
        });
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "pdf link: {} records, {placed} pages placed, {shipped} shipped again, {} commands; \
             line {:?}, tail {:?}, file {} bytes in {:?}",
            xpages.len(),
            got.len(),
            t1 - t0,
            t2 - t1,
            out.len(),
            t0.elapsed()
        );
    }
    Linked {
        file: out,
        objnums: v.list(got),
        ftags,
    }
}
