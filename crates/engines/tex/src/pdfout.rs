//! The exact PDF file (pdfTeX §750–§815) as the link of the line's
//! records (spec-redesign §1, `link`): no node reads the writer's state.
//!
//! - A page is written apart by the node that ships it ([`ship_apart`]:
//!   the box decoded, the fonts, the content stream compressed, as a
//!   record, `phitex_pdf::PageRecord`), on a writer of its own as the
//!   link's stands between two pages ([`base_fold`]), and its key goes
//!   to the line's accumulator ([`XPAGES`]).
//! - A read-time command (`\pdfobj`, `\pdfxform`, `\pdfximage`,
//!   `\pdfannot`, ...) records itself in the same accumulator where it
//!   runs ([`command`]): the writer's step it stands for, kept by the
//!   build ([`Cmd`]), and the key it is known by. What TeX reads back of
//!   it (an object number, `\pdffontobjnum`, a stack's number) is TeX's
//!   own count, as pdfTeX's counters give it: the read-time allocations
//!   before it plus the objects the pages shipped before it made, a fold
//!   of their write sides' allocations over the line ([`ALLOCS`]); no
//!   loop-back, no round.
//! - A page's PDF side ([`PdfPage`]: the content stream, its
//!   compression, the record) and the link run once, after the
//!   loop-backs settle (`final_only`); nothing on that side feeds a
//!   loop-back.
//! - The end node links ([`finish_exact`]): one writer state from the
//!   start, every record of the line put on it in order (a page by
//!   `PdfState::replay_page`, its objects numbered as pdfTeX numbers
//!   them; shipped again where the replay cannot give the walk's bytes;
//!   a command's step applied), then the tail and the cross-reference section by
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

/// Record `e` put in the line ([`XPAGES`]) where this run is; a piece's
/// record after one of its page-builder candidates goes after a marker
/// `["cand", piece, candidates passed]`: a page firing at an earlier
/// candidate of the piece is placed (and numbered) before it by the
/// link, as pdfTeX ships it there ([`order_line`]).
pub(crate) fn line_push(r: &mut Run<'_, '_>, e: ValueId) {
    let v = r.v;
    let op = r.cx.op();
    if (op == r.ops.exec || op == r.ops.resume) && r.cand > 0 {
        let pid = i64::from_ne_bytes(r.cx.node().raw().to_ne_bytes());
        let m = v.list(vec![v.str("cand"), v.int(pid), v.int(i64::from(r.cand))]);
        r.contribute(XPAGES, v.seq_unit(m));
    }
    r.contribute(XPAGES, v.seq_unit(e));
}

/// The line in pdfTeX's order: a page whose break is at candidate `k`
/// of a piece the piece goes on after ([`PagePiece::brk`]) comes after
/// the piece's records made before it and before those made after it
/// (the record after each marker `["cand", piece, c]` with `c > k`,
/// [`line_push`]), which the line holds before the page (the page's
/// finisher is placed after the whole piece). Nothing is dropped.
///
/// Example: piece P with records a (before its candidates), b (after
/// candidate 0), its page at candidate 0: the line `a, m0, b, page`
/// becomes `a, page, m0, b`.
pub(crate) fn order_line(v: &Values, xpages: &[ValueId]) -> Vec<ValueId> {
    let brks: Vec<(ValueId, i64, i64)> = {
        let m = pieces().lock().unwrap_or_else(PoisonError::into_inner);
        xpages
            .iter()
            .filter_map(|&x| {
                let k = v.as_int(x)?;
                let (pid, c) = m.get(&u64::from_ne_bytes(k.to_ne_bytes()))?.brk?;
                Some((x, pid, c))
            })
            .collect()
    };
    // (one pass: a marker of piece `pid` past candidate `c`, with the
    // record after it, goes after the last page breaking inside `pid` at
    // a candidate below `c` that follows it in the line; the pages of a
    // piece come in the line in the order of their candidates, so moving
    // after each in turn, as the pages are taken, ends there)
    let mut pages_of: FxHashMap<i64, Vec<(usize, i64)>> = FxHashMap::default();
    let pos: FxHashMap<u64, usize> = xpages
        .iter()
        .enumerate()
        .filter(|(_, x)| v.as_int(**x).is_some())
        .map(|(i, x)| (x.raw(), i))
        .collect();
    for &(page, pid, k) in &brks {
        if let Some(&p) = pos.get(&page.raw()) {
            pages_of.entry(pid).or_default().push((p, k));
        }
    }
    if pages_of.is_empty() {
        return xpages.to_vec();
    }
    // (each record's anchor: the page it goes after, if any)
    let mut anchor: Vec<Option<usize>> = vec![None; xpages.len()];
    let mut i = 0;
    while i < xpages.len() {
        let Some((mp, c)) = line_marker(v, xpages[i]) else {
            i += 1;
            continue;
        };
        let to = pages_of.get(&mp).and_then(|ps| {
            ps.iter()
                .filter(|&&(p, k)| p > i && k < c)
                .map(|&(p, _)| p)
                .max()
        });
        anchor[i] = to;
        if let Some(n) = xpages.get(i + 1)
            && line_marker(v, *n).is_none()
            && to.is_some()
        {
            anchor[i + 1] = to;
            i += 2;
            continue;
        }
        i += 1;
    }
    let mut after: FxHashMap<usize, Vec<ValueId>> = FxHashMap::default();
    for (i, a) in anchor.iter().enumerate() {
        if let Some(p) = a {
            after.entry(*p).or_default().push(xpages[i]);
        }
    }
    let mut line = Vec::with_capacity(xpages.len());
    for (i, &x) in xpages.iter().enumerate() {
        if anchor[i].is_some() {
            continue;
        }
        line.push(x);
        if let Some(m) = after.remove(&i) {
            line.extend(m);
        }
    }
    line
}

/// A line marker's piece and candidates passed ([`line_push`]).
fn line_marker(v: &Values, x: ValueId) -> Option<(i64, i64)> {
    if v.as_int(x).is_some() {
        return None;
    }
    let xs = list_of(v, x);
    (xs.len() == 3 && xs.first().map(|&k| str_of(v, k)).as_deref() == Some("cand"))
        .then(|| (int_of(v, xs[1]), int_of(v, xs[2])))
}

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
}

/// A read-time command's step on the writer, as the link applies it.
pub(crate) type CmdFn = Box<dyn FnMut(&mut PdfState, &mut Forms, &dyn FontSource) -> CmdOut + Send>;

/// A read-time command as the build keeps it for the link ([`command`]):
/// its step, the fonts it names, the writer's parameters where it ran.
pub(crate) struct Cmd {
    f: Mutex<CmdFn>,
    fonts: Fonts,
    wparams: WParams,
    /// The node that ran it (for the link's trace).
    node: u64,
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
    /// The page's box, for its PDF side ([`PdfPage`]).
    bx: Arc<BoxNode>,
    /// What the writer it is written on starts from ([`base_fold`]).
    base_in: BaseIn,
    /// The page's `/PageAttr`-like attribute text, hashed into what it
    /// shows.
    attr: String,
    /// The page as its PDF side wrote it, for the link to place, and the
    /// fonts used on the state it was written on: set once, by the
    /// page's [`PdfPage`] (final, after the loop-backs settle).
    made: OnceLock<(phitex_pdf::PageRecord, BTreeSet<FontId>)>,
    /// The page's index in the page chain (for traces and the key).
    pj: i64,
    /// The node that shipped it, for the link's errors ([`Linked`]).
    node: i64,
    /// Its break inside a piece that goes on after it (`[piece,
    /// candidate]`), for the line's order ([`order_line`]).
    brk: Option<(i64, i64)>,
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
    if r.param("pdfoutput")? <= 0 {
        return Ok(false);
    }
    let v = r.v;
    let mut xs = vec![v.str(kind)];
    xs.extend_from_slice(args);
    line_push(r, v.list(xs));
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
type Map = phitex_pdf::map::MapIndex;

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

/// The fonts as pdfTeX numbers them, a fold over the line (spec-final
/// A3): every font load is a record where it ran (`fontload`,
/// [`font_load`]: the text's, an output routine's), numbered in turn; a
/// page's virtual fonts' local fonts, which its step knows by name and
/// size only, are resolved where the walk reads them in (`do_vf`, in
/// the record's order of its walk: [`PageRecord::font_loads`]) against
/// the fonts loaded so far (`tfm_lookup`), else numbered as new ones
/// (`read_font_info`); `pdftexspace` where a faked space first needs
/// it. A text font loaded after a local font of its name and size is
/// that font in pdfTeX (§1260 finds it): it takes its number.
///
/// Example: fonts 1 and 2 loaded, then a page whose virtual font 2 has
/// the local font `cmr10` at 10pt (none so far) and whose walk needs the
/// space font, then `\font\x=cmr10`: pdfTeX's numbers 1, 2, 3 for the
/// local font, 4 for the space font, and the text's `cmr10` is 3.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FontNums {
    /// The numbers given so far.
    next: FontId,
    /// The fonts numbered, in their numbers' order: name, size, the id
    /// the writer knows it by, whether a virtual font loaded it.
    known: Vec<(Arc<str>, i32, FontId, bool)>,
    /// Ids numbered (a writer's id, or one a load named).
    done: BTreeSet<FontId>,
    /// The numbers where they differ from the ids.
    map: BTreeMap<FontId, FontId>,
    /// The local fonts this fold gave ids ([`LINK_LOCALS`] on).
    locals: FontId,
}

impl FontNums {
    fn put(&mut self, id: FontId, name: &str, size: i32, local: bool) {
        if self.done.insert(id) {
            self.next += 1;
            if id != self.next {
                self.map.insert(id, self.next);
            }
            self.known.push((Arc::from(name), size, id, local));
        }
    }

    /// The fonts a format brought, numbered from 1.
    fn seed(&mut self, fonts: &[(String, i32)]) {
        for (i, (n, sz)) in fonts.iter().enumerate() {
            self.put(FontId::try_from(i + 1).unwrap_or(0), n, *sz, false);
        }
    }

    /// A `fontload` record: font `id` of `name` at `size`.
    fn load(&mut self, id: FontId, name: &str, size: i32) {
        if self.done.contains(&id) {
            return;
        }
        let local = self
            .known
            .iter()
            .find(|k| k.3 && &*k.0 == name && k.1 == size)
            .map(|k| k.2);
        match local {
            Some(d) => {
                let n = self.tag(d);
                self.done.insert(id);
                if id != n {
                    self.map.insert(id, n);
                }
            }
            None => self.put(id, name, size, false),
        }
    }

    /// A virtual font's local font `name` at `size`: the font loaded so
    /// far of that name and size, else a new one; the writer's id.
    fn local(&mut self, name: &str, size: i32) -> FontId {
        if let Some(k) = self.known.iter().find(|k| &*k.0 == name && k.1 == size) {
            return k.2;
        }
        let id = LINK_LOCALS + self.locals;
        self.locals += 1;
        self.put(id, name, size, true);
        id
    }

    /// The fonts loaded after the last record, `t` in all (none but a
    /// load without its record).
    fn end(&mut self, t: i64) {
        let mut f: FontId = 1;
        while i64::from(f) <= t {
            if !self.done.contains(&f) {
                self.put(f, "", 0, false);
            }
            f += 1;
        }
    }

    /// Font `id`'s number.
    fn tag(&self, id: FontId) -> FontId {
        self.map.get(&id).copied().unwrap_or(id)
    }

    /// The local fonts of a step's fonts resolved: by its record's walk
    /// (`loads`, [`PageRecord::font_loads`]), else every virtual font's
    /// in order (a command, written by the link). The step's handle →
    /// the writer's id.
    fn resolve(
        &mut self,
        fonts: &Fonts,
        loads: Option<Vec<(bool, FontId)>>,
    ) -> FxHashMap<FontId, FontId> {
        let mut fwd = FxHashMap::default();
        let loads = loads.unwrap_or_else(|| {
            let mut vfs: Vec<FontId> = fonts.vfs.keys().copied().collect();
            vfs.sort_unstable();
            vfs.into_iter().map(|f| (true, f)).collect()
        });
        for (vf, f) in loads {
            if vf {
                if let Some(x) = fonts.vfs.get(&f) {
                    for &(_, g) in &x.fonts {
                        if let Some((n, sz)) = fonts.local(g)
                            && !fwd.contains_key(&g)
                        {
                            let d = self.local(n, sz);
                            fwd.insert(g, d);
                        }
                    }
                }
            } else if f == SPACE_FONT {
                self.put(SPACE_FONT, "pdftexspace", 0, false);
            }
        }
        fwd
    }
}

/// A step's fonts as the link's writer asks for them: by the writer's
/// ids (a local font's handle mapped, [`FontNums::resolve`]), named as
/// pdfTeX numbers them ([`FontNums`]).
struct Mapped<'a> {
    fonts: &'a Fonts,
    nums: &'a FontNums,
    fwd: &'a FxHashMap<FontId, FontId>,
    back: FxHashMap<FontId, FontId>,
}

impl<'a> Mapped<'a> {
    fn new(fonts: &'a Fonts, nums: &'a FontNums, fwd: &'a FxHashMap<FontId, FontId>) -> Self {
        Mapped {
            fonts,
            nums,
            fwd,
            back: fwd.iter().map(|(&a, &b)| (b, a)).collect(),
        }
    }

    fn step(&self, f: FontId) -> FontId {
        self.back.get(&f).copied().unwrap_or(f)
    }

    fn doc(&self, f: FontId) -> FontId {
        self.fwd.get(&f).copied().unwrap_or(f)
    }
}

impl FontSource for Mapped<'_> {
    fn metrics(&self, f: FontId) -> &ScaledFont {
        self.fonts.metrics(self.step(f))
    }
    fn name(&self, f: FontId) -> &[u8] {
        self.fonts.name(self.step(f))
    }
    fn map_entry(&self, f: FontId) -> Option<MapEntry> {
        self.fonts.map_entry(self.step(f))
    }
    fn font_file(&self, f: FontId) -> Option<Vec<u8>> {
        self.fonts.font_file(self.step(f))
    }
    fn encoding_file(&self, f: FontId) -> Option<Vec<u8>> {
        self.fonts.encoding_file(self.step(f))
    }
    fn file(&self, name: &[u8]) -> Option<Vec<u8>> {
        self.fonts.file(name)
    }
    fn map_by_ps_name(&self, ps_name: &[u8], slant: i32, extend: i32) -> Option<MapEntry> {
        self.fonts.map_by_ps_name(ps_name, slant, extend)
    }
    fn space_font(&self) -> Option<FontId> {
        self.fonts.space_font()
    }
    fn virtual_font(&self, f: FontId) -> Option<VirtualFont> {
        self.fonts.virtual_font(self.step(f)).map(|mut x| {
            for e in &mut x.fonts {
                e.1 = self.doc(e.1);
            }
            x
        })
    }
    fn tag(&self, f: FontId) -> Option<FontId> {
        Some(self.nums.tag(f))
    }
}

/// A font load where it runs (spec-final A3): font `id` of `name` at
/// `size`, a record of the line ([`XPAGES`]) the link numbers the fonts
/// by ([`FontNums`]).
pub(crate) fn font_load(r: &mut Run<'_, '_>, name: &str, size: i32, id: i64) {
    let v = r.v;
    let e = v.list(vec![
        v.str("fontload"),
        v.str(name),
        v.int(i64::from(size)),
        v.int(id),
    ]);
    line_push(r, e);
}

/// What the writer a page is written on starts from ([`BaseIn`]), read
/// where the page ships: the replay places the page where the link's
/// writer's variables are these ([`PdfState::replay_page`]); a page whose
/// are not (a link or a color running across, a form) is shipped again
/// by the link, the same bytes either way. No other node's output is
/// read.
fn base_inputs(r: &mut Run<'_, '_>) -> R<BaseIn> {
    let v = r.v;
    let date = r
        .get_loc(crate::fields::cell::PDFDATE)?
        .map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
    let jobname = r
        .get_loc(crate::fields::cell::JOBNAME)?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    let g2u = r.get("pdf:glyphtounicode")?;
    Ok(BaseIn {
        date,
        jobname,
        g2u,
        first: r.page_index() == 0,
    })
}

/// What a page's own writer starts from ([`base_fold`]): the file's date
/// and job name, the glyph-to-unicode table, whether it is the first page.
#[derive(Clone)]
pub(crate) struct BaseIn {
    date: String,
    jobname: String,
    g2u: Option<ValueId>,
    first: bool,
}

/// The writer a page is written on apart ([`BaseIn`]): a fresh one for
/// the first page; for a later one, a fresh one that has shipped an
/// empty page, as the link's writer stands between two pages (the header
/// written, the version fixed).
fn base_fold(v: &Values, b: &BaseIn, params: &[ValueId], wparams: WParams) -> Fold {
    let mut st = fresh(v, b.date.clone(), &b.jobname, b.g2u);
    if !b.first {
        set_writer_params(&mut st, wparams);
        page_params(v, st.params_mut(), params);
        let empty = BoxNode::default();
        let mut wh = |_: u32, _: bool| -> Option<Whatsit> { None };
        let _ = st.ship_out_with(&empty, &Fonts::default(), &mut wh);
        let _ = st.flush_chunk();
    }
    Fold {
        st,
        forms: Forms::default(),
    }
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

/// The files the images made so far ask for where they are written (an
/// included PDF's fonts replaced by the map's, `copyFont` off), found by
/// each `\pdfximage` ([`note_image_files`]): every writer that may write
/// an image (a page shipped, an `\immediate` image) is given them with
/// the map.
pub(crate) fn image_fonts(r: &mut Run<'_, '_>, fonts: &mut Fonts) -> R<()> {
    let v = r.v;
    let names: Vec<String> = r
        .get_loc(crate::fields::cell::PDF_IMAGEFILES)?
        .map(|x| list_of(v, x))
        .unwrap_or_default()
        .iter()
        .map(|&n| str_of(v, n))
        .collect();
    if crate::exec::obs_flag!("PHITEX_PDF_TRACE") {
        eprintln!("  image fonts for node {}: {names:?}", r.cx.node().raw());
    }
    if names.is_empty() {
        return Ok(());
    }
    fonts.ps = default_ps_map(r)?;
    for n in names {
        if let Some(b) = file_bytes(r, &n)? {
            fonts.files.insert(n.into_bytes(), b);
        }
    }
    Ok(())
}

/// The files image `spec` asks for where it is written: dry writes on a
/// writer of its own with the map, the files asked for and not given
/// (each found given to the next: a file one font asks for may name
/// another), added to the images' (`pdf:imagefiles`, [`image_fonts`]).
pub(crate) fn note_image_files(
    r: &mut Run<'_, '_>,
    spec: &phitex_pdf::ImageSpec,
    set: impl Fn(&mut PdfState),
) -> R<()> {
    let v = r.v;
    let mut fonts = Fonts {
        ps: default_ps_map(r)?,
        ..Fonts::default()
    };
    let mut have: BTreeSet<Vec<u8>> = BTreeSet::new();
    for _ in 0..4 {
        let mut st = PdfState::new(PdfParams::default());
        set(&mut st);
        if let Ok(n) = st.ximage(spec.clone()) {
            let _ = st.write_ximage(n, &fonts);
        }
        let missed =
            std::mem::take(&mut *fonts.missed.lock().unwrap_or_else(PoisonError::into_inner));
        let new: Vec<Vec<u8>> = missed.into_iter().filter(|n| !have.contains(n)).collect();
        if new.is_empty() {
            break;
        }
        for n in new {
            if let Some(b) = file_bytes(r, &String::from_utf8_lossy(&n))? {
                fonts.files.insert(n.clone(), b);
            }
            have.insert(n);
        }
    }
    if crate::exec::obs_flag!("PHITEX_PDF_TRACE") {
        eprintln!(
            "  image files: {:?} (map {})",
            have.iter()
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .collect::<Vec<_>>(),
            fonts.ps.is_some()
        );
    }
    if have.is_empty() {
        return Ok(());
    }
    let mut all: BTreeSet<String> = r
        .get_loc(crate::fields::cell::PDF_IMAGEFILES)?
        .map(|x| list_of(v, x))
        .unwrap_or_default()
        .iter()
        .map(|&n| str_of(v, n))
        .collect();
    let n0 = all.len();
    all.extend(have.iter().map(|n| String::from_utf8_lossy(n).into_owned()));
    if all.len() != n0 {
        let x = v.list(all.iter().map(|n| v.str(n)).collect());
        r.set_cell(crate::fields::cell::PDF_IMAGEFILES, x, true);
    }
    Ok(())
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
    /// The virtual fonts `do_vf` reads, by font.
    vfs: FxHashMap<FontId, VirtualFont>,
    /// The virtual fonts' local fonts that are none of the step's own
    /// (by name and size, never by number: [`LOCAL_FONTS`] + the index
    /// is the step's handle of one, which the link resolves against the
    /// fonts loaded before it, [`FontNums`]).
    locals: Vec<(String, i32)>,
}

/// The first handle of a virtual font's local font in a step's fonts
/// ([`Fonts::locals`]); the link's own for the local fonts it numbers
/// ([`FontNums::local`]) begin at [`LINK_LOCALS`]. Both above any font
/// the engine numbers and below [`SPACE_FONT`].
const LOCAL_FONTS: FontId = 0x6000_0000;
const LINK_LOCALS: FontId = 0x7000_0000;

impl Fonts {
    /// Local font `g`'s name and size, if `g` is a handle of one.
    fn local(&self, g: FontId) -> Option<(&str, i32)> {
        let i = usize::try_from(g.checked_sub(LOCAL_FONTS)?).ok()?;
        self.locals.get(i).map(|(n, s)| (n.as_str(), *s))
    }

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
        Arc::new(phitex_pdf::map::MapIndex::new(Arc::from(bytes_of(
            &str_of(v, m),
        ))))
    })))
}

/// The fonts `ids` as the writer asks for them, with the files of the
/// map lines read so far; `space`: with `pdftexspace` too.
#[allow(clippy::too_many_lines)]
pub(crate) fn fonts_for(r: &mut Run<'_, '_>, ids: &[u32], space: bool) -> R<Fonts> {
    let v = r.v;
    let map = default_map(r)?;
    let mut out = Fonts::default();
    // the fonts' fields, defined once at their loads (`[id, name, size,
    // file, size asked]`), then the virtual fonts' local fonts
    // (`vf_def_font`): a local font of a name and size this step has is
    // that font, else a handle of its own ([`Fonts::locals`]); nothing of
    // the document's font table is read, the link numbers the local fonts
    let mut known: Vec<(u32, String, i32, ValueId, i32)> = Vec::new();
    for &f in ids {
        if let Some(i) = crate::fonts::info(v, r.font_get(f, crate::fields::FontCell::Info)?)
            && let Some(file) = i.file
            && !known.iter().any(|x| x.0 == f)
        {
            known.push((f, i.name, i.size, file, i.s));
        }
    }
    // (every step asks for its fonts' virtual files itself: no memo of
    // another step's answer, so a file appearing runs every step that
    // asked)
    let mut k = 0;
    while k < known.len() {
        let (f, name, size) = (known[k].0, known[k].1.clone(), known[k].2);
        k += 1;
        let Some(b) = file_bytes(r, &format!("{name}.vf"))? else {
            continue;
        };
        let Ok(vf) = phitex_tfm::parse_vf(&b) else {
            continue;
        };
        let mut fonts = Vec::new();
        for lf in &vf.fonts {
            let fs = store_scaled_f(lf.scaled, size);
            let lname = String::from_utf8_lossy(&lf.name).into_owned();
            let g = if let Some(x) = known.iter().find(|x| x.1 == lname && x.2 == fs) {
                x.0
            } else if let Some(file) = r.file(&format!("{lname}.tfm"))? {
                let g = LOCAL_FONTS + u32::try_from(out.locals.len()).unwrap_or(0);
                out.locals.push((lname.clone(), fs));
                known.push((g, lname, fs, file, fs));
                g
            } else {
                // (`read_font_info` failing: the null font)
                0
            };
            fonts.push((lf.number, g));
        }
        let packets = vf
            .chars
            .iter()
            .filter_map(|c| Some((u8::try_from(c.code).ok()?, c.dvi.to_vec())))
            .collect();
        out.vfs.insert(f, VirtualFont { fonts, packets });
    }
    let mut wanted: Vec<(u32, String, ValueId, i32)> = known
        .into_iter()
        .map(|(f, name, _, file, s)| (f, name, file, s))
        .collect();
    if space && let Some(x) = r.file("pdftexspace.tfm")? {
        // pdfTeX loads the space font at the shipout that first needs
        // it, as the next internal font number (the engine's
        // `fontcount` does not count it)
        let id = // a page written apart: one id for every page, outside the
            // fonts' range, numbered by the link where it is first
            // loaded ([`FontNums`])
            SPACE_FONT;
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
            map.as_ref().and_then(|m| m.get(name.as_bytes()))
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
/// kept for the link ([`Cmd`]). Nothing is read back: what TeX reads of
/// it is the caller's own count, as pdfTeX's counters give it
/// ([`page_offset`]); always `None`.
pub(crate) fn command(
    r: &mut Run<'_, '_>,
    input: ValueId,
    fonts: &[u32],
    mut f: impl FnMut(&mut PdfState, &dyn FontSource) -> CmdOut + Send + 'static,
) -> R<Option<CmdOut>> {
    command_forms(r, input, fonts, move |st, _, fs| f(st, fs))
}

/// [`command`], the forms' table in reach.
pub(crate) fn command_forms(
    r: &mut Run<'_, '_>,
    input: ValueId,
    fonts: &[u32],
    f: impl FnMut(&mut PdfState, &mut Forms, &dyn FontSource) -> CmdOut + Send + 'static,
) -> R<Option<CmdOut>> {
    if r.param("pdfoutput")? <= 0 {
        return Ok(None);
    }
    let v = r.v;
    let mut fs = fonts_for(r, fonts, false)?;
    // (an image the command writes: the files its fonts ask for)
    image_fonts(r, &mut fs)?;
    let wparams = writer_params(r)?;
    // (the command's key: its node and its place among the node's
    // commands, the same in every round the node runs as this run)
    let _ = input;
    r.muts += 1;
    r.pdf_occ += 1;
    let key = phitex_defuse::hash_words(
        0x6364_6d6b_6579_0001,
        &[r.cx.node().raw(), r.pdf_occ as u64],
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
                node: r.cx.node().raw(),
            }),
        );
    let e = v.list(vec![
        v.str("cmd"),
        v.int(i64::from_ne_bytes(key.to_ne_bytes())),
        v.int(i64::from_ne_bytes(serial.to_ne_bytes())),
    ]);
    line_push(r, e);
    Ok(None)
}

/// The commands' serials ([`command`]): one per run of a command.
static CMD_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
) -> R<()> {
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
    let space = resolved.values().any(|&d| {
        matches!(
            list_of(v, d).first().map(|&k| str_of(v, k)).as_deref(),
            Some("iwon" | "fake")
        )
    });
    // the fonts of the forms the page may ship too
    let mut all: BTreeSet<u32> = ids.iter().copied().collect();
    all.extend(form_fonts(r)?);
    let all: Vec<u32> = all.into_iter().collect();
    let mut fonts = fonts_for(r, &all, space)?;
    // an included PDF's fonts replaced by the map's (`copyFont` off):
    // the files the images ask for, found where they were made
    image_fonts(r, &mut fonts)?;
    let margins = [
        r.param("pdflinkmargin")?,
        r.param("pdfdestmargin")?,
        r.param("pdfthreadmargin")?,
    ];
    page_allocs(r, ids, &fonts, &side.whats)?;
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

/// The page's write side (spec: what TeX needs runs every round): its
/// [`PagePiece`] (the box, its fonts, whatsits and parameters) kept for
/// its PDF side, its key put in the line where it shipped, and that PDF
/// side placed as a child ([`PdfPage`], run once the loop-backs settle):
/// the box decoded into the content stream, compressed, the record the
/// link places.
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
) -> R<()> {
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
    let base_in = base_inputs(r)?;
    let pc = PagePiece {
        wparams,
        boxv: list_of(v, page_v).first().copied().unwrap_or(ValueId::UNIT),
        params: p.to_vec(),
        margins,
        whats,
        fonts,
        bx: Arc::new(bx.clone()),
        attr: p.get(15).map(|&a| str_of(v, a)).unwrap_or_default(),
        made: OnceLock::new(),
        pj: i64::try_from(r.page_index()).unwrap_or(i64::MAX),
        node: i64::from_ne_bytes(r.cx.node().raw().to_ne_bytes()),
        brk: r
            .fin
            .as_ref()
            .and_then(|c| c.brk)
            .filter(|b| b.2)
            .map(|b| (b.0, b.1)),
        base_in,
    };
    // the key: what the piece is made of, by the values' identities
    // (interned: equal values, equal ids), not by their bytes
    let maps = r
        .get_loc(crate::fields::cell::PDF_MAPS)?
        .map_or(0, ValueId::raw);
    let pdfmap = r.file("pdftex.map")?.map_or(0, ValueId::raw);
    let b = &pc.base_in;
    let mut words = vec![
        page_v.raw(),
        u64::from(!b.first),
        phitex_defuse::hash_bytes(0, b.date.as_bytes()),
        phitex_defuse::hash_bytes(0, b.jobname.as_bytes()),
        b.g2u.map_or(0, ValueId::raw),
        maps,
        pdfmap,
        u64::from(pc.fonts.space.is_some()),
        pc.pj.cast_unsigned(),
        pc.brk.map_or(0, |b| b.0.cast_unsigned()),
        pc.brk.map_or(u64::MAX, |b| b.1.cast_unsigned()),
        // (the fonts as the writer gets them, the files the images ask
        // for among them: a run that read other ones writes another page)
        pc.fonts.digest(),
        u64::from(pc.fonts.ps.is_some()),
    ];
    words.extend(pc.margins.iter().map(|&m| u64::from(m.cast_unsigned())));
    words.extend(pc.wparams.iter().map(|&m| u64::from(m.cast_unsigned())));
    let key = phitex_defuse::hash_words(0x7067_7069_6563_6531, &words).max(1);
    pieces()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key)
        .or_insert_with(|| Arc::new(pc));
    // the page's place in the file: its record in the line, where it
    // shipped; the link numbers its objects after every record before
    // it and before every one after
    let k = v.int(i64::from_ne_bytes(key.to_ne_bytes()));
    line_push(r, k);
    let op = r.ops.pdfpage;
    r.cx.child(op, vec![k]);
    Ok(())
}

/// The field a page's PDF side defines when its record is written
/// (`pdf:rec:<key>`, [`PdfPage`]: the hash of what the page shows): the
/// link reads it for each page of the line, so it runs after them.
pub(crate) fn rec_field(v: &Values, key: i64) -> phitex_defuse::Field {
    v.field(&format!("pdf:rec:{}", key.cast_unsigned()))
}

/// A page's PDF side (`[key]`, the [`PagePiece`] its finisher kept):
/// the page written once, apart, on a writer of its own as the link's
/// stands between two pages ([`base_fold`]), so that its content stream
/// is compressed in parallel with the other pages' and the hash of what
/// it shows is known ([`rec_field`]). Final (`Operation::final_only`):
/// it runs once the loop-backs settle, and nothing it does feeds one.
pub(crate) struct PdfPage;

impl phitex_defuse::Operation for PdfPage {
    fn name(&self) -> &'static str {
        "pdfpage"
    }
    fn defs(
        &self,
        args: &[ValueId],
        v: &Values,
    ) -> Option<Vec<(phitex_defuse::Field, phitex_defuse::Locality)>> {
        Some(
            args.first()
                .and_then(|&x| v.as_int(x))
                .map(|k| vec![(rec_field(v, k), phitex_defuse::Locality::Global)])
                .unwrap_or_default(),
        )
    }
    /// Nothing: a range read before the loop-backs settle never waits
    /// for it.
    fn static_contribs(
        &self,
        _: &[ValueId],
        _: &Values,
    ) -> Option<Vec<(phitex_defuse::Field, ValueId)>> {
        Some(Vec::new())
    }
    fn final_only(&self, _: &[ValueId], _: &Values) -> bool {
        true
    }
    fn run(&self, cx: &mut phitex_defuse::Ctx<'_>) -> Result<ValueId, phitex_defuse::Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let v = cx.values();
        let Some(k) = cx.args().first().and_then(|&x| v.as_int(x)) else {
            return Ok(ValueId::UNIT);
        };
        let key = u64::from_ne_bytes(k.to_ne_bytes());
        let pc = pieces()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        let Some(pc) = pc else {
            cx.define_with(rec_field(v, k), v.int(0), phitex_defuse::Locality::Global);
            return Ok(ValueId::UNIT);
        };
        let Fold { mut st, forms } = base_fold(v, &pc.base_in, &pc.params, pc.wparams);
        set_writer_params(&mut st, pc.wparams);
        set_page_params(v, &mut st, &pc.params, pc.margins);
        let base = st.fonts_used();
        let (res, rec) = {
            let mut wh = |h: u32, _leaders: bool| page_whatsit(&forms, &pc.whats, h);
            st.ship_recorded(&pc.bx, &pc.fonts, &mut wh)
        };
        // (the writer's error here is this writer's, not the file's: a
        // page whose record is not placed as it is is shipped again by
        // the link, whose error is the job's, [`Linked::errors`])
        let _ = res.err();
        let (content, w, h) = st.last_page_shown();
        let words = [
            u64::from_ne_bytes(i64::from(w).to_ne_bytes()),
            u64::from_ne_bytes(i64::from(h).to_ne_bytes()),
        ];
        let shown = phitex_defuse::hash_words(phitex_defuse::hash_bytes(0, content), &words);
        let shown = phitex_defuse::hash_bytes(shown, pc.attr.as_bytes());
        let _ = pc.made.set((rec, base));
        cx.define_with(
            rec_field(v, k),
            v.int(i64::from_ne_bytes(shown.to_ne_bytes())),
            phitex_defuse::Locality::Global,
        );
        Ok(ValueId::UNIT)
    }
}

/// Each shipped page's allocations as pdfTeX makes them at `\\shipout`
/// (pdfTeX §752–§768), contributed by the page's write side (its
/// finisher, every round) where it ships: `[fixed, [font names],
/// [destination names], [shipout fonts], compressible]`: the objects the
/// page always takes (its object unless a `\\pdfpageref` made it, its
/// contents, its resources), the fonts it uses by name (an object for
/// each name first used), the destinations it makes or links to (an
/// object for each first named), the fonts the shipout itself loads (a
/// virtual font's local fonts, `pdftexspace`: pdfTeX's font numbers after
/// them), and the objects it writes into object streams (a stream's own
/// object every 100, `\\pdfobjcompresslevel` above 0). A read-time
/// command's object number is TeX's count of the read-time allocations
/// before it plus the fold of this over the line before it
/// ([`page_offset`]): an in-round value, no loop-back.
pub(crate) const ALLOCS: &str = "pdf:alloc";

/// The page's allocations ([`ALLOCS`]), from the write side: its box's
/// fonts (`fonts`, the writer's view), its whatsits and the parameters
/// in force.
pub(crate) fn page_allocs(
    r: &mut Run<'_, '_>,
    ids: &[u32],
    fonts: &Fonts,
    side_whats: &[ValueId],
) -> R<()> {
    let v = r.v;
    let j = i64::try_from(r.page_index()).unwrap_or(0);
    let pre = r.get(&format!("pdf:pageref:{}", j + 1))?.is_some();
    let fixed = if pre { 2 } else { 3 };
    let mut names: Vec<ValueId> = Vec::new();
    for &f in ids {
        let id = FontId::from(f);
        if fonts.by_id.contains_key(&id) {
            names.push(v.str(&String::from_utf8_lossy(phitex_pdf::FontSource::name(
                fonts, id,
            ))));
        }
    }
    let mut dests: Vec<ValueId> = Vec::new();
    let mut annots = 0i64;
    for &w in side_whats {
        let xs = list_of(v, w);
        match xs.first().map(|&k| str_of(v, k)).as_deref() {
            Some("pdf:dest") => {
                if let Some(&id) = xs.get(1) {
                    dests.push(id);
                }
            }
            Some("pdf:annot" | "pdf:startlink") => annots += 1,
            _ => {}
        }
    }
    let mut ship_fonts: Vec<ValueId> = fonts
        .locals
        .iter()
        .map(|(n, sz)| v.str(&format!("{n}@{sz}")))
        .collect();
    if fonts.space.is_some() {
        ship_fonts.push(v.str("pdftexspace@0"));
    }
    // (the page's dictionary and resources, and each annotation it
    // writes, go into an object stream where they compress)
    let compressible = if r.param("pdfobjcompresslevel")? > 0 {
        2 + annots
    } else {
        0
    };
    let e = v.list(vec![
        v.int(fixed),
        v.list(names),
        v.list(dests),
        v.list(ship_fonts),
        v.int(compressible),
    ]);
    r.contribute(ALLOCS, v.seq_unit(e));
    Ok(())
}

/// The fold of the pages' allocations ([`ALLOCS`]) over a range: the
/// objects they made and the fonts the shipouts loaded.
#[derive(Clone, Copy, Default)]
struct AllocFold {
    objects: i64,
    ship_fonts: i64,
}

type AllocMemo = Mutex<FxHashMap<(u64, u64), AllocFold>>;

fn fold_allocs(v: &Values, range: ValueId) -> AllocFold {
    static M: OnceLock<AllocMemo> = OnceLock::new();
    let m = M.get_or_init(|| Mutex::new(FxHashMap::default()));
    let k = (v.instance(), range.raw());
    if let Some(&x) = m.lock().unwrap_or_else(PoisonError::into_inner).get(&k) {
        return x;
    }
    let mut out = AllocFold::default();
    let mut written = 0i64;
    let mut seen: BTreeSet<ValueId> = BTreeSet::new();
    let mut seen_dest: BTreeSet<ValueId> = BTreeSet::new();
    let mut seen_ship: BTreeSet<ValueId> = BTreeSet::new();
    for (pages, e) in v.seq_to_vec(range).into_iter().enumerate() {
        let xs = list_of(v, e);
        let i = |k: usize| xs.get(k).and_then(|&x| v.as_int(x)).unwrap_or(0);
        let l = |k: usize| xs.get(k).map(|&x| list_of(v, x)).unwrap_or_default();
        out.objects += i(0);
        // (a pages-tree node every `pages_tree_kids_max` pages)
        if pages % 6 == 0 {
            out.objects += 1;
        }
        for n in l(1) {
            if seen.insert(n) {
                out.objects += 1;
            }
        }
        for d in l(2) {
            if seen_dest.insert(d) {
                out.objects += 1;
            }
        }
        for f in l(3) {
            if seen_ship.insert(f) {
                out.ship_fonts += 1;
            }
        }
        for _ in 0..i(4) {
            if written % 100 == 0 {
                out.objects += 1;
            }
            written += 1;
        }
    }
    let mut g = m.lock().unwrap_or_else(PoisonError::into_inner);
    if g.len() > 1 << 12 {
        g.clear();
    }
    g.insert(k, out);
    out
}

/// The objects the pages shipped before this run made ([`ALLOCS`]): a
/// read-time allocation's number is TeX's count of the read-time ones
/// before it plus this. A fold of the accumulator's range before this
/// node: no round, no loop-back.
pub(crate) fn page_offset(r: &mut Run<'_, '_>) -> R<i64> {
    let v = r.v;
    let range = r.cx.acc_range(v.field(ALLOCS), None)?;
    Ok(fold_allocs(v, range).objects)
}

/// Font `n`'s name number as pdfTeX gives it (`\\pdffontname`): the
/// engine's number after the fonts the shipouts before this run loaded
/// ([`ALLOCS`]), which pdfTeX numbers as it loads them.
pub(crate) fn font_tag_read(r: &mut Run<'_, '_>, n: i64) -> R<i64> {
    let v = r.v;
    let range = r.cx.acc_range(v.field(ALLOCS), None)?;
    Ok(n + fold_allocs(v, range).ship_fonts)
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
    /// The fonts loaded before the line's first `fontload` record (a
    /// format's), name and size by number from 1 ([`FontNums::seed`]).
    pub seed: Vec<(String, i32)>,
    pub info_omit_date: i32,
    pub suppress_ptex_info: i32,
}

/// The records between two of the link's kept states ([`LinkMemo`]).
const LINK_SNAP: usize = 8;

/// The writer as the link left it after a record (every [`LINK_SNAP`]).
#[derive(Clone)]
struct LinkSnap {
    at: usize,
    st: PdfState,
    /// The writer's digest ([`PdfState::digest`]), taken with it.
    dg: u64,
    nums: FontNums,
    forms: Forms,
    got: usize,
    errors: usize,
    placed: usize,
    shipped: usize,
}

/// The last link of the process: its records (by value), each record's
/// chunks, the writer every [`LINK_SNAP`] records, and what it gave the
/// commands; an edit's link copies the records it shares from the start
/// (spec-final §6.2).
struct LinkMemo {
    /// The values' instance and what the writer starts from, hashed.
    head: u64,
    ids: Vec<u64>,
    chunks: Vec<Vec<phitex_pdf::Chunk>>,
    snaps: Vec<LinkSnap>,
    got: Vec<ValueId>,
    errors: Vec<(String, i64)>,
    /// The writer after the last record (before the file's end).
    end: LinkSnap,
    /// The record `chunks[0]` is (its earlier records moved out).
    chunk_base: usize,
}

/// Whether the writer `st` with `nums` and `forms` is the one the last
/// link had at snapshot `sn`: its records from there on are then the
/// same bytes ([`finish_exact`]). The forms compared only when there are
/// none (a document with forms runs on).
fn same_writer(st: &mut PdfState, nums: &FontNums, forms: &Forms, sn: &LinkSnap) -> bool {
    let none = forms.whats.is_empty()
        && forms.fonts.is_empty()
        && sn.forms.whats.is_empty()
        && sn.forms.fonts.is_empty();
    if !none {
        return false;
    }
    // (the snapshot's digest taken when it was kept: no copy of its
    // writer, no formatting)
    *nums == sn.nums && st.digest() == sn.dg
}

/// [`same_writer`] but for the characters the pages used: `st` has used
/// every character the snapshot's writer had (and maybe more), and is
/// that writer otherwise. The records after it are then the same bytes
/// (the characters used are read at the file's end only, the fonts'
/// subsets), and the end's writer is the last link's with `st`'s
/// characters added.
fn same_writer_but_chars(st: &mut PdfState, nums: &FontNums, forms: &Forms, sn: &LinkSnap) -> bool {
    let none = forms.whats.is_empty()
        && forms.fonts.is_empty()
        && sn.forms.whats.is_empty()
        && sn.forms.fonts.is_empty();
    if !none || *nums != sn.nums {
        return false;
    }
    let mine = st.used_chars();
    let covers = sn.st.used_chars().iter().all(|(f, c)| {
        mine.iter()
            .find(|m| m.0 == *f)
            .is_some_and(|m| c.iter().zip(&m.1).all(|(&a, &b)| !a || b))
    });
    if !covers {
        return false;
    }
    let mut theirs = sn.st.clone();
    st.digest_sans_chars() == theirs.digest_sans_chars()
}

fn link_memo() -> &'static Mutex<Option<LinkMemo>> {
    static M: OnceLock<Mutex<Option<LinkMemo>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

/// What the link made: the file (empty: none).
pub(crate) struct Linked {
    pub file: Vec<u8>,
    /// The file's hash, from its chunks' ([`phitex_pdf::Tail::link_hashed`];
    /// none: to be hashed whole).
    pub hash: Option<u64>,
    /// The writer's errors where the link shipped a page again (pdfTeX's,
    /// `PDF-BACKEND-GAP`: a fatal error there, here logged, the job going
    /// on), with the node that shipped the page.
    pub errors: Vec<(String, i64)>,
}

/// The exact PDF file, linked (spec-redesign §1 `link`): one writer
/// state from the start, the line's records put on it in order
/// ([`XPAGES`]): a font load numbered ([`FontNums`]); a page's record
/// placed (`PdfState::replay_page`, its objects numbered after every
/// record before it, its fonts' handles mapped and their numbers in its
/// content rewritten), or shipped again from its box where the replay
/// cannot give the walk's bytes; a read-time command's step applied
/// ([`Cmd`]); a document-level command applied ([`apply_doc_op`]).
/// Each record's bytes are chunks; a page's content whose font numbers
/// changed is rewritten and compressed on the executor, the pages apart
/// in parallel (spec-final §6.1); then the tail (the fonts embedded,
/// the pages tree: by the final state's digest) and the cross-reference
/// section, by the prefix sums of the chunks' lengths
/// (`phitex_pdf::Tail::link`).
#[allow(clippy::too_many_lines)]
pub(crate) fn finish_exact(
    v: &Values,
    f: &EndFields,
    xpages: &[ValueId],
    cmds: &CmdMemo,
) -> Linked {
    let none = Linked {
        file: Vec::new(),
        hash: None,
        errors: Vec::new(),
    };
    if xpages.is_empty() {
        return none;
    }
    let t0 = std::time::Instant::now();
    let mut st = fresh(v, f.date.clone(), &f.jobname, f.g2u);
    let mut forms = Forms::default();
    let mut nums = FontNums::default();
    nums.seed(&f.seed);
    let mut bytes = PdfBytes::new();
    let mut got: Vec<ValueId> = Vec::new();
    let mut errors: Vec<(String, i64)> = Vec::new();
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
    // (spec-final §6.2: the records the last link of this build placed,
    // the same from the file's start, are not placed again: their chunks
    // are copied, and the writer goes on from the state it had after
    // them, kept every `LINK_SNAP` records)
    let ids: Vec<u64> = xpages.iter().map(|x| x.raw()).collect();
    // (what the writer starts from: the same for the copy to stand)
    let mut head = phitex_defuse::hash_bytes(
        phitex_defuse::hash_words(
            v.instance(),
            &[f.g2u.map_or(0, ValueId::raw), f.date.len() as u64],
        ),
        [f.date.as_bytes(), f.jobname.as_bytes()]
            .concat()
            .as_slice(),
    );
    for (n, sz) in &f.seed {
        head = phitex_defuse::hash_words(
            phitex_defuse::hash_bytes(head, n.as_bytes()),
            &[u64::from(sz.cast_unsigned())],
        );
    }
    let mut chunks_of: Vec<Vec<phitex_pdf::Chunk>> = Vec::with_capacity(xpages.len());
    let mut snaps: Vec<LinkSnap> = Vec::new();
    let mut start = 0usize;
    // (the last link's memo, taken: an edit's records after the change
    // that are the last link's again, from a writer state equal to the
    // one it had there, are copied too, `resync` below)
    let mut old: Option<LinkMemo> = link_memo()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .filter(|m| m.head == head);
    if let Some(m) = old.as_mut() {
        let same = m.ids.iter().zip(&ids).take_while(|(a, b)| a == b).count();
        if let Some(sn) = m.snaps.iter().rev().find(|sn| sn.at <= same && sn.at > 0) {
            st = sn.st.clone();
            nums = sn.nums.clone();
            forms = sn.forms.clone();
            got = m.got[..sn.got].to_vec();
            errors = m.errors[..sn.errors].to_vec();
            (placed, shipped) = (sn.placed, sn.shipped);
            start = sn.at;
            // (moved, not copied: the memo is this link's to replace)
            chunks_of.extend(m.chunks.drain(..start));
            let (before, after): (Vec<LinkSnap>, Vec<LinkSnap>) = std::mem::take(&mut m.snaps)
                .into_iter()
                .partition(|x| x.at <= start);
            snaps.extend(before);
            m.snaps = after;
            m.chunk_base = start;
        }
    }
    let copied = start;
    let t_pre = t0.elapsed();
    // (each record's bytes from `start` on, a page's content still to
    // be rewritten and compressed among them)
    let mut parts: Vec<Vec<phitex_pdf::Part>> = Vec::with_capacity(xpages.len() - start);
    // (where the records are the last link's again from a writer equal
    // to its own there: the rest copied, spec-final §6.2 past the edit)
    let mut resync: Option<usize> = None;
    // (where the writer there had used other characters: those of the
    // pages before, added to the last link's end, [`same_writer_but_chars`])
    let mut chars_added: Option<Vec<(FontId, [bool; 256])>> = None;
    let mut tries = 0usize;
    let (mut t_try, mut t_snap) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
    for (i, (&x, pc)) in xpages.iter().zip(&pcs).enumerate().skip(start) {
        let tt = std::time::Instant::now();
        if i > start
            && tries < 8
            && let Some(m) = old.as_ref()
            && let Some(sn) = m.snaps.iter().find(|sn| sn.at == i)
            && m.ids.len() == ids.len()
            && m.ids[i..] == ids[i..]
        {
            tries += 1;
            if same_writer(&mut st, &nums, &forms, sn) {
                resync = Some(i);
                t_try += tt.elapsed();
                break;
            }
            if same_writer_but_chars(&mut st, &nums, &forms, sn) {
                resync = Some(i);
                chars_added = Some(st.used_chars());
                t_try += tt.elapsed();
                break;
            }
        }
        t_try += tt.elapsed();
        let tt = std::time::Instant::now();
        if i > start && (i - start).is_multiple_of(LINK_SNAP) {
            snaps.push(LinkSnap {
                at: i,
                dg: st.digest(),
                st: st.clone(),
                nums: nums.clone(),
                forms: forms.clone(),
                got: got.len(),
                errors: errors.len(),
                placed,
                shipped,
            });
        }
        t_snap += tt.elapsed();
        let mut ps: Vec<phitex_pdf::Part> = Vec::new();
        if v.as_int(x).is_some() {
            // a page: no piece in the memo is a page written by a run
            // the engine threw away; nothing to place
            let Some(pc) = pc else {
                return none;
            };
            set_writer_params(&mut st, pc.wparams);
            // its virtual fonts' local fonts numbered where its walk
            // reads them in, after every font loaded before it
            // (a page whose PDF side did not run: shipped again below)
            let empty = phitex_pdf::PageRecord::default();
            let rec = pc.made.get().map_or(&empty, |m| &m.0);
            let fwd = nums.resolve(&pc.fonts, Some(rec.font_loads()));
            let src = Mapped::new(&pc.fonts, &nums, &fwd);
            let fmap = |g: FontId| fwd.get(&g).copied().unwrap_or(g);
            set_page_params(v, &mut st, &pc.params, pc.margins);
            let n0 = st.objects_numbered();
            let res = st.replay_page(rec, &src, &fmap, None, &mut ps);
            if res.is_err() {
                // (the node that wrote the page logged the writer's
                // error when it wrote it)
                let mut wh = |h: u32, _leaders: bool| page_whatsit(&forms, &pc.whats, h);
                let mut side = crate::nodes::Side::default();
                if let Some(phitex_typeset::node::Node::Box(bx)) =
                    crate::nodes::decode(v, pc.boxv, &mut side)
                    && let Err(e) = st.ship_out_with(&bx, &src, &mut wh)
                {
                    errors.push((format!("pdfTeX error: {e}"), pc.node));
                }
                shipped += 1;
            } else {
                placed += 1;
            }
            if crate::exec::obs_flag!("PHITEX_PDF_TRACE") {
                eprintln!(
                    "  link page {} (shipped by node {}): objects {n0} -> {} ({})",
                    pc.pj,
                    u64::from_ne_bytes(pc.node.to_ne_bytes()),
                    st.objects_numbered(),
                    match &res {
                        Ok(r) => format!(
                            "replayed{}",
                            r.as_deref().map_or(String::new(), |r| format!(", {r}"))
                        ),
                        Err(e) => format!("shipped again: {e}"),
                    }
                );
            }
        } else {
            let xs = list_of(v, x);
            match xs.first().map(|&k| str_of(v, k)).as_deref() {
                Some("cmd") => {
                    let key = xs.get(1).copied().unwrap_or(ValueId::UNIT);
                    let serial = xs
                        .get(2)
                        .map_or(0, |&s| u64::from_ne_bytes(int_of(v, s).to_ne_bytes()));
                    let Some(c) = serials.get(&serial) else {
                        return none;
                    };
                    set_writer_params(&mut st, c.wparams);
                    let fwd = nums.resolve(&c.fonts, None);
                    let src = Mapped::new(&c.fonts, &nums, &fwd);
                    let out = {
                        let mut g = c.f.lock().unwrap_or_else(PoisonError::into_inner);
                        (*g)(&mut st, &mut forms, &src)
                    };
                    if crate::exec::obs_flag!("PHITEX_PDF_TRACE") {
                        eprintln!(
                            "  link command of node {}: {:?} (objects {})",
                            c.node,
                            out,
                            st.objects_numbered()
                        );
                    }
                    got.push(v.list(vec![key, out.value(v)]));
                }
                Some("fontload") => {
                    let name = xs.get(1).map(|&n| str_of(v, n)).unwrap_or_default();
                    let size = xs.get(2).map_or(0, |&n| int_of(v, n));
                    let id = xs.get(3).map_or(0, |&n| int_of(v, n));
                    nums.load(
                        FontId::try_from(id).unwrap_or(0),
                        &name,
                        i32::try_from(size).unwrap_or(0),
                    );
                }
                _ => apply_doc_op(v, &mut st, x),
            }
        }
        if let Some(c) = st.flush_chunk() {
            ps.push(phitex_pdf::Part::Bytes(c));
        }
        parts.push(ps);
    }
    let t_loop = t0.elapsed();
    crate::timeline::note(
        &format!(
            "link: records {start}..{} replayed ({tries} resync tries)",
            resync.unwrap_or(xpages.len())
        ),
        t_loop.saturating_sub(t_pre),
    );
    crate::timeline::note("link: the resync tries", t_try);
    crate::timeline::note("link: the writer's snapshots", t_snap);
    // (spec-final §6.1: the pages' contents whose font numbers moved,
    // rewritten and compressed apart, on the executor)
    let t_parts = std::time::Instant::now();
    let jobs: Vec<(usize, usize)> = parts
        .iter()
        .enumerate()
        .flat_map(|(i, ps)| {
            ps.iter()
                .enumerate()
                .filter(|(_, p)| matches!(p, phitex_pdf::Part::Content { .. }))
                .map(move |(k, _)| (i, k))
        })
        .collect();
    let done: Vec<phitex_pdf::Chunk> = {
        let mut ex = crate::exec::Inner(crate::exec::threads(None));
        crate::exec::map_on(&mut ex, jobs.len(), &|j| {
            let (i, k) = jobs[j];
            parts[i][k].clone().resolve()
        })
    };
    let mut done = done.into_iter();
    let mut rewritten = 0usize;
    for ps in parts {
        let mut cs = Vec::with_capacity(ps.len());
        for p in ps {
            cs.push(match p {
                phitex_pdf::Part::Bytes(c) => c,
                phitex_pdf::Part::Content { .. } => {
                    rewritten += 1;
                    done.next().unwrap_or_else(|| p.resolve())
                }
            });
        }
        chunks_of.push(cs);
    }
    let t_parts = t_parts.elapsed();
    let mut resynced = 0usize;
    if let Some(i) = resync
        && let Some(mut m) = old.take()
        && let Some(sn) = m.snaps.iter().find(|sn| sn.at == i)
    {
        let (sg, se, sp, ss) = (sn.got, sn.errors, sn.placed, sn.shipped);
        resynced = xpages.len() - i;
        chunks_of.extend(m.chunks.drain(i - m.chunk_base..));
        snaps.extend(m.snaps.into_iter().filter(|x| x.at >= i));
        got.extend(m.got[sg..].iter().copied());
        errors.extend(m.errors.drain(se..));
        placed += m.end.placed - sp;
        shipped += m.end.shipped - ss;
        st = m.end.st;
        nums = m.end.nums;
        forms = m.end.forms;
        if let Some(c) = &chars_added {
            st.add_used_chars(c);
        }
    }
    for c in chunks_of.iter().flatten() {
        bytes.push(c.clone());
    }
    let end = LinkSnap {
        at: xpages.len(),
        dg: 0,
        st: st.clone(),
        nums: nums.clone(),
        forms: forms.clone(),
        got: got.len(),
        errors: errors.len(),
        placed,
        shipped,
    };
    *link_memo().lock().unwrap_or_else(PoisonError::into_inner) = Some(LinkMemo {
        head,
        ids,
        chunks: chunks_of,
        snaps,
        got: got.clone(),
        errors: errors.clone(),
        end,
        chunk_base: 0,
    });
    {
        use crate::timeline::note;
        note("link: linked: before the records", t_pre);
        note("link: linked: the records", t_loop.saturating_sub(t_pre));
        note(
            "link: linked: after them (the tail, the xref, the bytes)",
            t0.elapsed().saturating_sub(t_loop),
        );
    }
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "pdf link: {copied} of {} records copied from the last link, {resynced} more after the change; {rewritten} page contents rewritten in {t_parts:?}; before the records {t_pre:?}, records {:?}, after them {:?}",
            xpages.len(),
            t_loop.saturating_sub(t_pre),
            t0.elapsed().saturating_sub(t_loop)
        );
    }
    // the fonts loaded after the last record (none but a load without
    // its record)
    nums.end(f.fonts);
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
    let (out, hash) = tail
        .and_then(|t| {
            t.map_or(Ok((Vec::new(), None)), |t| {
                t.link_hashed(&bytes).map(|(b, h)| (b, Some(h)))
            })
        })
        .unwrap_or_else(|e| {
            if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
                eprintln!("pdf: {e}");
            }
            (Vec::new(), None)
        });
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
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
        hash,
        errors,
    }
}
