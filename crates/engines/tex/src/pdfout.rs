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

use crate::exec::LockWait as _;
use std::collections::{BTreeMap, BTreeSet};

use phitex_defuse::FxHashMap;
use std::sync::{Arc, Mutex, OnceLock};

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
        let m = pieces().locked();
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

/// A line record for a trace: a page's number, the node that shipped
/// it and its break; a command's node; a marker; another record as it is.
pub(crate) fn line_show(v: &Values, x: ValueId) -> String {
    if let Some(k) = v.as_int(x) {
        let m = pieces().locked();
        return match m.get(&u64::from_ne_bytes(k.to_ne_bytes())) {
            Some(pc) => format!(
                "page {} node {} brk {:?}",
                pc.pj,
                pc.node.cast_unsigned(),
                pc.brk.map(|(p, c)| (p.cast_unsigned(), c))
            ),
            None => "page ?".to_owned(),
        };
    }
    if let Some((p, c)) = line_marker(v, x) {
        return format!("marker piece {} cand {c}", p.cast_unsigned());
    }
    let xs = list_of(v, x);
    if xs.first().map(|&k| str_of(v, k)).as_deref() == Some("cmd") {
        let serial = xs
            .get(2)
            .map_or(0, |&s| u64::from_ne_bytes(int_of(v, s).to_ne_bytes()));
        return format!("cmd serial {serial}");
    }
    v.render(x).chars().take(80).collect()
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
    /// When it was made ([`CMD_BORN`]): the link drops only the
    /// commands made before it looked.
    born: u64,
    /// The object number TeX gave the object the command makes, at read
    /// time ([`note_read_time`]; 0: none): the link maps it to the
    /// number its writer gives where it stands in the line.
    rt: std::sync::atomic::AtomicU64,
}

/// A read-time allocation `n` of the command in progress: the number
/// TeX read back for the object its writer command makes (the first
/// allocation after the command was recorded, [`Run::pdf_cmd`]).
pub(crate) fn note_read_time(r: &mut Run<'_, '_>, n: i64) {
    let serial = std::mem::take(&mut r.pdf_cmd);
    if serial == 0 {
        return;
    }
    if let Some(c) = r.ops.pdf_cmds.locked().get(&serial) {
        c.rt.store(n.cast_unsigned(), std::sync::atomic::Ordering::Relaxed);
    }
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
    /// The page as its PDF side wrote it, for the link to place, and the
    /// fonts used on the state it was written on: set once, by the
    /// page's [`PdfPage`] (final, after the loop-backs settle).
    made: OnceLock<(phitex_pdf::PageRecord, BTreeSet<FontId>, usize)>,
    /// The page's index in the page chain (for traces).
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
    // (the writer's parameters where it runs: one that writes now, an
    // open action's object, fixes the file's version and compression
    // with them, §792, as a command's step does)
    let wp = writer_params(r)?;
    let mut w = vec![v.str(DOC_WP)];
    w.extend(wp.iter().map(|&p| v.int(i64::from(p))));
    xs.push(v.list(w));
    line_push(r, v.list(xs));
    Ok(true)
}

/// The tag of a document-level command's writer parameters, its record's
/// last element ([`doc_op`]).
const DOC_WP: &str = "wp";

/// The writer's parameters a document-level command's record carries
/// ([`doc_op`]).
fn doc_op_params(v: &Values, e: ValueId) -> Option<WParams> {
    let xs = list_of(v, *list_of(v, e).last()?);
    if xs.first().map(|&k| str_of(v, k)).as_deref() != Some(DOC_WP) {
        return None;
    }
    let mut ps = [0; 5];
    for (p, &x) in ps.iter_mut().zip(xs.get(1..)?) {
        *p = i32::try_from(int_of(v, x)).unwrap_or(0);
    }
    Some(ps)
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
    let cell = m.locked().entry(key).or_default().clone();
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

    /// The fonts a format brought, by key, numbered from 1 in the order
    /// it loaded them.
    fn seed(&mut self, fonts: &[(FontId, String, i32)]) {
        for (k, n, sz) in fonts {
            self.put(*k, n, *sz, false);
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

    /// The fonts loaded after the last record, by key in the order of
    /// their loads (none but a load without its record).
    fn end(&mut self, order: &[FontId]) {
        for &f in order {
            if !self.done.contains(&f) {
                self.put(f, "", 0, false);
            }
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
            // (in the order of the fonts' numbers: their keys say nothing
            // of it)
            let mut vfs: Vec<FontId> = fonts.vfs.keys().copied().collect();
            vfs.sort_unstable_by_key(|&f| (self.tag(f), f));
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
    fn font_file_shared(&self, f: FontId) -> Option<Arc<[u8]>> {
        self.fonts.font_file_shared(self.step(f))
    }
    fn encoding_file_shared(&self, f: FontId) -> Option<Arc<[u8]>> {
        self.fonts.encoding_file_shared(self.step(f))
    }
    fn file_shared(&self, name: &[u8]) -> Option<Arc<[u8]>> {
        self.fonts.file_shared(name)
    }
    fn metrics_hash(&self, f: FontId) -> u64 {
        self.fonts.metrics_hash(self.step(f))
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
/// The font key of a `fontload` record ([`font_load`]), if `x` is one.
pub(crate) fn fontload_key(v: &Values, x: ValueId) -> Option<i64> {
    let xs = list_of(v, x);
    (xs.len() == 4 && str_of(v, xs[0]) == "fontload").then(|| int_of(v, xs[3]))
}

pub(crate) fn font_load(r: &mut Run<'_, '_>, name: &str, size: i32, id: i64) {
    let v = r.v;
    r.contribute(FONT_LOADS, v.seq_unit(v.int(id)));
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
        let missed = std::mem::take(&mut *fonts.missed.locked());
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
    /// The font as loaded ([`crate::fonts::load`], shared: its metrics
    /// not copied for every page that uses the font).
    loaded: Arc<crate::fonts::Loaded>,
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
    /// The fonts' numbers as the step sees them, by key (TeX's: their
    /// ranks in the order of the loads before it, [`FONT_LOADS`]): the
    /// names its content gives them, which the link rewrites only where
    /// the fonts the shipouts loaded moved them ([`FontNums`]).
    nums: FxHashMap<FontId, FontId>,
}

/// The fonts' loads, by key, where they ran (a sequence): the order a
/// step reads its fonts' provisional numbers from ([`Fonts::nums`]); the
/// line's `fontload` records ([`font_load`]) are the exact order, in
/// TeX's order of runs ([`order_line`]).
pub(crate) const FONT_LOADS: &str = "fontloads";

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
                u64::from_ne_bytes(i64::from(x.loaded.at.size).to_ne_bytes()),
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
        let mut nums: Vec<(FontId, FontId)> = self.nums.iter().map(|(&k, &n)| (k, n)).collect();
        nums.sort_unstable();
        for (k, n) in nums {
            h = phitex_defuse::hash_words(h, &[u64::from(k), u64::from(n)]);
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
    if let Some((w, h)) = m.locked().get(&key)
        && w.upgrade().is_some_and(|a| Arc::ptr_eq(&a, b))
    {
        return *h;
    }
    let h = phitex_defuse::hash_bytes(0, b).max(1);
    let mut m = m.locked();
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
            |x| &x.loaded.at,
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
        self.file_shared(name).map(|b| b.to_vec())
    }
    fn font_file_shared(&self, f: FontId) -> Option<Arc<[u8]>> {
        self.by_id.get(&f)?.ff.clone()
    }
    fn encoding_file_shared(&self, f: FontId) -> Option<Arc<[u8]>> {
        self.by_id.get(&f)?.enc.clone()
    }
    fn file_shared(&self, name: &[u8]) -> Option<Arc<[u8]>> {
        let f = self.files.get(name).cloned();
        if f.is_none() {
            self.missed.locked().insert(name.to_vec());
        }
        f
    }
    fn metrics_hash(&self, f: FontId) -> u64 {
        self.by_id.get(&f).map_or_else(
            || phitex_pdf::metrics_hash(self.metrics(f)),
            |x| {
                *x.loaded
                    .at_hash
                    .get_or_init(|| phitex_pdf::metrics_hash(&x.loaded.at))
            },
        )
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
    fn tag(&self, f: FontId) -> Option<FontId> {
        self.nums.get(&f).copied()
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
    // (the writer's lookup: a file absent here is no read-back point)
    let was = std::mem::replace(&mut r.writer_lookup, true);
    let found = r.file(name);
    r.writer_lookup = was;
    Ok(found?.and_then(|x| {
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
    let nums = load_numbers(r)?;
    for &f in ids {
        if let Some(&n) = nums.get(&i64::from(f)) {
            out.nums.insert(f, FontId::try_from(n).unwrap_or(0));
        }
    }
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
        // it, as the next internal font number (the engine numbers no
        // font for it)
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
                loaded: l.clone(),
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
    // the `\pdfglyphtounicode` table a format carries (`dumptounicode`,
    // pdfTeX §1502; `undumptounicode`, §1503): a fresh writer state, a
    // format loaded, starts from it. Made once per table (its value in
    // its values table) and shared by every writer started from it: the
    // writer's tables are shared until changed, so a page written apart
    // or a link starts from it without copying or hashing it again.
    let mut st = match g2u {
        None => PdfState::new(PdfParams::default()),
        Some(t) => {
            let key = (v.instance(), t.raw());
            let mut m = g2u_states().locked();
            if let Some(k) = m.iter().position(|e| e.0 == key) {
                m[k].1.clone()
            } else {
                let mut st = PdfState::new(PdfParams::default());
                for e in list_of(v, t) {
                    let ab = list_of(v, e);
                    if let [a, b] = ab[..] {
                        st.glyph_to_unicode(&bytes_of(&str_of(v, a)), &bytes_of(&str_of(v, b)));
                    }
                }
                // (its sums taken here, once: a clone's are up to date)
                let _ = st.digest();
                if m.len() >= 4 {
                    m.remove(0);
                }
                m.push((key, st.clone()));
                st
            }
        }
    };
    *st.params_mut() = p;
    st
}

/// The fresh writers of [`fresh`] by their glyph table (values instance,
/// value), the latest few.
type G2uStates = Mutex<Vec<((u64, u64), PdfState)>>;

/// [`G2uStates`], the process's.
fn g2u_states() -> &'static G2uStates {
    static M: OnceLock<G2uStates> = OnceLock::new();
    M.get_or_init(|| Mutex::new(Vec::new()))
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
    r.muts += 1;
    r.pdf_occ += 1;
    let key = phitex_defuse::hash_words(
        0x6364_6d6b_6579_0001,
        &[r.cx.node().raw(), r.pdf_occ as u64],
    )
    .max(1);
    // (the record's handle on the command: what the command is (its key,
    // its input, the fonts and parameters it runs with), never a count
    // of the commands made so far, which the threads' schedule orders;
    // two runs that make the same handle made the same command)
    let mut words = vec![key, input.raw(), fs.digest()];
    words.extend(wparams.iter().map(|&p| u64::from(p.cast_unsigned())));
    let serial = phitex_defuse::hash_words(0x6364_6d73_6572_0001, &words);
    let born = CMD_BORN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    r.ops.pdf_cmds.locked().insert(
        serial,
        Arc::new(Cmd {
            f: Mutex::new(Box::new(f)),
            fonts: fs,
            wparams,
            node: r.cx.node().raw(),
            born,
            rt: std::sync::atomic::AtomicU64::new(0),
        }),
    );
    r.pdf_cmd = serial;
    let e = v.list(vec![
        v.str("cmd"),
        v.int(i64::from_ne_bytes(key.to_ne_bytes())),
        v.int(i64::from_ne_bytes(serial.to_ne_bytes())),
    ]);
    line_push(r, e);
    Ok(None)
}

/// The commands made so far ([`command`]), for [`Cmd::born`] only: a
/// count the schedule orders, never put in a value.
static CMD_BORN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    bx: BoxNode,
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
    let marks = mark_events(v, &bx, &resolved);
    page_allocs(r, ids, &fonts, &side.whats, marks)?;
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
    bx: BoxNode,
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
        bx: Arc::new(bx),
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
        // (not the page's index in the chain: the record is placed by
        // the link, its objects numbered there, so a page an earlier
        // page moved is the same page and keeps its record)
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
    pieces().locked().entry(key).or_insert_with(|| Arc::new(pc));
    // the page's place in the file: its record in the line, where it
    // shipped; the link numbers its objects after every record before
    // it and before every one after
    let k = v.int(i64::from_ne_bytes(key.to_ne_bytes()));
    line_push(r, k);
    let op = r.ops.pdfpage;
    r.kid(op, vec![k]);
    Ok(())
}

/// Page `pc`'s PDF side ([`PdfPage`]): written once, apart, on a writer
/// of its own as the link's stands between two pages ([`base_fold`]),
/// its record kept for the link (a page whose record is not placed as
/// it is is shipped again by the link, whose error is the job's,
/// [`Linked::errors`]). The record is made once, by whichever asks first.
fn write_page(v: &Values, pc: &PagePiece) {
    pc.made.get_or_init(|| {
        let Fold { mut st, forms } = base_fold(v, &pc.base_in, &pc.params, pc.wparams);
        set_writer_params(&mut st, pc.wparams);
        set_page_params(v, &mut st, &pc.params, pc.margins);
        let base = st.fonts_used();
        // (the objects TeX numbered before the page that its whatsits
        // name, a link's, an annotation's: numbered here first, so that
        // the page's own objects come after them and the record keeps
        // their numbers as names, mapping only its own; on a writer that
        // starts from fewer objects a link's number was taken for one of
        // the page's own, and the replay renumbered the link)
        // (the read-time numbers only its bytes name: named after
        // those, [`apart_names`])
        // (named past the objects the writer it starts from made too,
        // [`base_fold`]: a name one of those has is scheduled there, and
        // the line's number of the same object is not)
        let floor = st.objects_numbered();
        let (named, aliases) = apart_names(&pc.whats, floor);
        while st.objects_numbered() < (named.max(floor) + aliases.len()).min(1 << 24) {
            st.new_objnum();
        }
        let local: Reloc = aliases.iter().map(|&(p, t)| (t, p)).collect();
        let (res, rec) = {
            let mut wh = |h: u32, _leaders: bool| {
                page_whatsit(&forms, &pc.whats, h).map(|w| relocated(w, &local))
            };
            st.ship_recorded(&pc.bx, &pc.fonts, &mut wh)
        };
        let _ = res.err();
        (rec, base, floor)
    });
}

/// A page's PDF side (`[key]`, the [`PagePiece`] its finisher kept):
/// [`write_page`], so that the pages' content streams are compressed in
/// parallel before the link places them (the link writes a page whose
/// record is not made yet itself: it never waits for these). Final
/// (`Operation::final_only`): it runs once the loop-backs settle, and
/// nothing it does feeds one.
pub(crate) struct PdfPage;

/// The keys of the pages a run shipped ([`PagePiece`]s, in its shipouts'
/// order), defined by that run and read by its [`PdfPage`] children,
/// whose argument is their place in the list: a child's identity is
/// the shipout's place, never the page's content.
pub(crate) const PAGE_KEYS: &str = "pdf:pagekeys";

impl phitex_defuse::Operation for PdfPage {
    fn name(&self) -> &'static str {
        "pdfpage"
    }
    fn defs(
        &self,
        _: &[ValueId],
        _: &Values,
    ) -> Option<Vec<(phitex_defuse::Field, phitex_defuse::Locality)>> {
        Some(Vec::new())
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
        let Some(i) = cx
            .args()
            .first()
            .and_then(|&x| v.as_int(x))
            .and_then(|i| usize::try_from(i).ok())
        else {
            return Ok(ValueId::UNIT);
        };
        // (its own key only: a shipout of the same run elsewhere in the
        // list changing leaves it standing)
        let p = cx.path(&[phitex_defuse::Step::Index(u32::try_from(i).unwrap_or(u32::MAX))]);
        let Some(k) = cx
            .use_field_path(v.field(PAGE_KEYS), p)?
            .and_then(|x| v.as_int(x))
        else {
            return Ok(ValueId::UNIT);
        };
        let pc = pieces()
            .locked()
            .get(&u64::from_ne_bytes(k.to_ne_bytes()))
            .cloned();
        if let Some(pc) = pc {
            write_page(v, &pc);
        }
        Ok(ValueId::UNIT)
    }
}

/// Each shipped page's allocations as pdfTeX makes them at `\\shipout`
/// (pdfTeX §752–§768), contributed by the page's write side (its
/// finisher, every round) where it ships: `[fixed, [font names],
/// [destination names], [shipout fonts], compressible, read-time count,
/// [read-time fonts], [read-time destinations], read-time object stream
/// writes, [mark events], node, [read-time pages]`: the objects the page
/// always takes (its object unless a `\\pdfpageref` or an action made it
/// first, by its place in the line, its contents, its resources),
/// the fonts it uses by name (an object for each name first used), the
/// destinations it defines (an object for each first named; each first
/// defined written into an object stream), the fonts the shipout itself
/// loads (a virtual font's local fonts, `pdftexspace`: pdfTeX's font
/// numbers after them), and the objects it writes into object streams
/// besides its destinations (its dictionary, its resources, its
/// annotations; a stream's own object every 100, `\\pdfobjcompresslevel`
/// above 0). A read-time command's object number is TeX's count of the
/// read-time allocations before it plus the fold of this over the line
/// before it ([`page_offset`]): an in-round value, no loop-back.
///
/// The read-time count before the page is in the entry only where the
/// run that ships it has read it already (its own read-time commands
/// before the `\\shipout`); otherwise it is -1, and the one reader that
/// needs it (`\\pdfpageref` of a shipped page, [`shipped_page_obj`])
/// reads it before the shipping node: a finisher does not read the
/// count, so an edit that changes it does not run the finishers after
/// it again.
pub(crate) const ALLOCS: &str = "pdf:alloc";

/// The page's allocations ([`ALLOCS`]), from the write side: its box's
/// fonts (`fonts`, the writer's view), its whatsits and the parameters
/// in force.
pub(crate) fn page_allocs(
    r: &mut Run<'_, '_>,
    ids: &[u32],
    fonts: &Fonts,
    side_whats: &[ValueId],
    marks: ValueId,
) -> R<()> {
    let v = r.v;
    // (its object unless a reference or a link's action made it first:
    // the fold knows the page's number, [`RT_PAGES`])
    let fixed = 3;
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
    // writes, go into an object stream where they compress; its
    // destinations too, counted by the fold where first defined)
    let compressible = if r.param("pdfobjcompresslevel")? > 0 {
        2 + annots
    } else {
        0
    };
    // (the read-time objects made before the page: its own object's
    // number, after its resources', counts them; counted here only
    // where this run made or read one before the shipout, else by the
    // reader that needs it, before this node)
    let read_time = if r.rt_made > 0 || r.st.is_loaded(crate::fields::cell::PDF_OBJNUM) {
        let me = r.cx.node().raw();
        crate::pdfprim::rt_before(r, me)? + r.rt_made
    } else {
        -1
    };
    let rt_fonts = r.get(RT_FONTS)?.unwrap_or_else(|| v.list(Vec::new()));
    let rt_dests = r.get(RT_DESTS)?.unwrap_or_else(|| v.list(Vec::new()));
    let rt_os = r.get(RT_OS)?.unwrap_or_else(|| v.int(0));
    let rt_pages = r.get(RT_PAGES)?.unwrap_or_else(|| v.list(Vec::new()));
    // (where the page broke, for a page firing inside a piece: the
    // piece and its candidate, [`page_offset_at`])
    let brk = r
        .fin
        .as_ref()
        .and_then(|c| c.brk)
        .filter(|b| b.2)
        .map_or(ValueId::UNIT, |(pid, k, _)| v.list(vec![v.int(pid), v.int(k)]));
    let e = v.list(vec![
        v.int(fixed),
        v.list(names),
        v.list(dests),
        v.list(ship_fonts),
        v.int(compressible),
        v.int(read_time),
        rt_fonts,
        rt_dests,
        rt_os,
        marks,
        v.int(i64::from_ne_bytes(r.cx.node().raw().to_ne_bytes())),
        rt_pages,
        brk,
    ]);
    if crate::exec::obs_flag!("PHITEX_ALLOC_TRACE") {
        eprintln!("alloc: page (node {}): {}", r.cx.node().raw(), v.render(e));
    }
    r.contribute(ALLOCS, v.seq_unit(e));
    Ok(())
}

/// The events of a page's shipout that make objects past its fixed
/// ones, in the order the walk meets them (pdfTeX §729, §738, §1612ff):
/// `[0, level, n]` n hboxes entered at `level` since the mark before
/// (the state the same for all of them; a link running at that
/// level continues in each, `append_link`: an object a piece);
/// `[1, level, kind, target]` a link started (`do_link`; its action's
/// target, got when each piece is written: `kind` 1 a destination, 2 a
/// thread, 3 a page by number, 0 none); `[2]` a link ended; `[3, on]`
/// running links on or off; `[4, level, n]` n vboxes entered at `level`
/// (a thread continues in each, `append_thread`); `[5, start, level,
/// running, id]` a thread's bead (`do_thread`: the bead and its
/// rectangle's object, the thread's own where first named); `[6]` a
/// thread ended. The state they act on (the link stack, the thread)
/// runs on over the pages, so the fold of the line ([`fold_allocs`])
/// counts them.
fn mark_events(v: &Values, bx: &BoxNode, resolved: &FxHashMap<u32, ValueId>) -> ValueId {
    struct Walk<'a> {
        v: &'a Values,
        resolved: &'a FxHashMap<u32, ValueId>,
        cur_s: i64,
        seg: BTreeMap<(i64, i64), i64>,
        ev: Vec<Vec<ValueId>>,
    }
    impl Walk<'_> {
        fn count(&mut self, code: i64, level: i64) {
            *self.seg.entry((code, level)).or_insert(0) += 1;
        }
        /// The boxes entered since the last mark (the state is the
        /// same for all of them), before the next.
        fn flush(&mut self) {
            let v = self.v;
            for ((code, level), n) in std::mem::take(&mut self.seg) {
                self.ev.push(vec![v.int(code), v.int(level), v.int(n)]);
            }
        }
        fn push(&mut self, e: Vec<ValueId>) {
            self.flush();
            self.ev.push(e);
        }
        fn what(&mut self, h: u32, vertical: bool) {
            let v = self.v;
            let Some(&d) = self.resolved.get(&h) else {
                return;
            };
            let ys = list_of(v, d);
            let kind = ys.first().map(|&k| str_of(v, k)).unwrap_or_default();
            let int = |k: usize| ys.get(k).map_or(0, |&n| int_of(v, n));
            match kind.as_str() {
                "pdf:startlink" if !vertical => {
                    let a = ys.get(6).map(|&a| list_of(v, a)).unwrap_or_default();
                    let ak = a.first().map(|&k| str_of(v, k)).unwrap_or_default();
                    let local = a.get(1).is_none_or(|&f| str_of(v, f).is_empty());
                    let page = a.get(2).copied().filter(|&p| p != ValueId::UNIT);
                    let id = a.get(3).copied().unwrap_or(ValueId::UNIT);
                    let named = !list_of(v, id).is_empty();
                    let (target, what) = match (ak.as_str(), page) {
                        ("goto", Some(p)) if local => {
                            (3, v.int(list_of(v, p).first().map_or(0, |&n| int_of(v, n))))
                        }
                        ("goto", None) if local && named => (1, id),
                        ("thread", _) if local && named => (2, id),
                        _ => (0, ValueId::UNIT),
                    };
                    self.push(vec![v.int(1), v.int(self.cur_s), v.int(target), what]);
                }
                "pdf:endlink" if !vertical => self.push(vec![v.int(2)]),
                "rloff" => self.push(vec![v.int(3), v.int(0)]),
                "rlon" => self.push(vec![v.int(3), v.int(1)]),
                "pdf:thread" | "pdf:startthread" => {
                    let start = kind == "pdf:startthread";
                    if start && !vertical {
                        return;
                    }
                    let running = i32::try_from(int(4)).ok() == Some(phitex_typeset::node::RUNNING);
                    self.push(vec![
                        v.int(5),
                        v.int(i64::from(start)),
                        v.int(self.cur_s),
                        v.int(i64::from(running)),
                        ys.get(1).copied().unwrap_or(ValueId::UNIT),
                    ]);
                }
                "pdf:endthread" if vertical => self.push(vec![v.int(6)]),
                _ => {}
            }
        }
        fn nodes(&mut self, list: &[phitex_typeset::node::Node], vertical: bool) {
            use phitex_typeset::node::Node;
            for p in list {
                match p {
                    Node::Box(b) => self.boxed(b),
                    Node::Whatsit(h, _) => self.what(*h, vertical),
                    Node::Disc(d) if !vertical => self.nodes(&d.replace, false),
                    _ => {}
                }
            }
        }
        fn boxed(&mut self, b: &BoxNode) {
            use phitex_typeset::node::BoxKind;
            self.cur_s += 1;
            let vertical = b.kind == BoxKind::V;
            self.count(if vertical { 4 } else { 0 }, self.cur_s);
            self.nodes(&b.list, vertical);
            self.cur_s -= 1;
        }
    }
    let mut w = Walk {
        v,
        resolved,
        cur_s: -1,
        seg: BTreeMap::new(),
        ev: Vec::new(),
    };
    w.boxed(bx);
    w.flush();
    v.list(w.ev.into_iter().map(|e| v.list(e)).collect())
}

/// The fonts a read-time write uses (an `\\immediate\\pdfxform`'s):
/// an object made for each name no page shipped before and no read-time
/// write before used (`pdf:rtfonts`, which the pages' allocations carry,
/// [`ALLOCS`]).
pub(crate) fn fonts_first_used(r: &mut Run<'_, '_>, names: &[String]) -> R<()> {
    let v = r.v;
    // (named, not counted: whether a page made the font's object first
    // is the pages' fold's to say, where a read needs the count,
    // [`page_offset`]; the write itself only reaches PDF bytes)
    let mut rt: Vec<ValueId> = r.get(RT_FONTS)?.map(|x| list_of(v, x)).unwrap_or_default();
    let n0 = rt.len();
    for n in names {
        let x = v.str(n);
        if !rt.contains(&x) {
            rt.push(x);
        }
    }
    if rt.len() != n0 {
        r.set(RT_FONTS, v.list(rt), true);
    }
    Ok(())
}

/// The allocations before this node ([`ALLOCS`]); `PHITEX_ALLOCS_TRACE`
/// (observe): which node read them.
fn allocs_range(r: &mut Run<'_, '_>, why: &str) -> R<ValueId> {
    // (what a page firing at a candidate this piece ran through made
    // comes first: `Run::order_after_pages`)
    r.order_after_pages(why)?;
    if crate::exec::obs_flag!("PHITEX_ALLOCS_TRACE") {
        eprintln!("allocs-read: node {} {why}", r.cx.node().raw());
    }
    let v = r.v;
    Ok(r.cx.acc_range(v.field(ALLOCS), None)?)
}

/// The fonts read-time writes used ([`fonts_first_used`]).
const RT_FONTS: &str = "pdf:rtfonts";

/// The destinations read-time commands made ([`dest_first_named`]).
const RT_DESTS: &str = "pdf:rtdests";

/// The objects read-time writes put into object streams ([`os_write`]).
const RT_OS: &str = "pdf:rtos";

/// The pages whose objects read-time commands made before they shipped
/// (`\\pdfpageref`, an action's `goto page`: [`page_made_at_read_time`]).
const RT_PAGES: &str = "pdf:rtpages";

/// Page `n`'s object made by a read-time command before the page
/// shipped: the page then makes no object of its own ([`ALLOCS`]).
pub(crate) fn page_made_at_read_time(r: &mut Run<'_, '_>, n: i64) -> R<()> {
    let v = r.v;
    let mut xs: Vec<ValueId> = r.get(RT_PAGES)?.map(|x| list_of(v, x)).unwrap_or_default();
    xs.push(v.int(n));
    r.set(RT_PAGES, v.list(xs), true);
    Ok(())
}

/// A destination a read-time command names (an outline's `goto`):
/// its object made where no page shipped before and no read-time command
/// before made it (`pdf_get_obj`).
pub(crate) fn dest_first_named(r: &mut Run<'_, '_>, id: ValueId) -> R<()> {
    let v = r.v;
    // (named, not counted, as [`fonts_first_used`])
    let mut rt: Vec<ValueId> = r.get(RT_DESTS)?.map(|x| list_of(v, x)).unwrap_or_default();
    if rt.contains(&id) {
        return Ok(());
    }
    rt.push(id);
    r.set(RT_DESTS, v.list(rt), true);
    Ok(())
}

/// An object a read-time command writes now: with
/// `\\pdfobjcompresslevel` above 0 it goes into an object stream, whose
/// own object is made as its first object goes in (every 100: pdfTeX's
/// `pdf_os_max_objs`), counting the pages' before it.
pub(crate) fn os_write(r: &mut Run<'_, '_>) -> R<()> {
    if r.param("pdfobjcompresslevel")? <= 0 {
        return Ok(());
    }
    let v = r.v;
    let range = allocs_range(r, "os_write")?;
    let pages = fold_allocs(v, range).os_written;
    let rt = r.get(RT_OS)?.and_then(|x| v.as_int(x)).unwrap_or(0);
    if (pages + rt) % 100 == 0 {
        bump_cell(r)?;
    }
    r.set(RT_OS, v.int(rt + 1), true);
    Ok(())
}

/// A read-time object counted in the `pdf:objnum` cell, made by this
/// run ([`crate::pdfprim::rt_before`]).
pub(crate) fn bump_cell(r: &mut Run<'_, '_>) -> R<()> {
    let k = r.cint(crate::fields::cell::PDF_OBJNUM)? + 1;
    let x = r.v.int(k);
    r.set_cell(crate::fields::cell::PDF_OBJNUM, x, true);
    r.rt_cells += 1;
    r.rt_made += 1;
    Ok(())
}

/// Page `n`'s object (`\\pdfpageref n`) where that page shipped before
/// this run and no reference made its object first: pdfTeX makes its
/// resources' object, then the page's (pdfTeX §752). The read-time
/// objects before it are the count the shipping run had where it
/// shipped: in the page's entry when that run read it, else the count
/// reaching the shipping node ([`ALLOCS`]).
pub(crate) fn shipped_page_obj(r: &mut Run<'_, '_>, n: i32) -> R<Option<i64>> {
    if r.param("pdfoutput")? <= 0 {
        return Ok(None);
    }
    let v = r.v;
    let range = allocs_range(r, "shipped_page_obj")?;
    let f = fold_allocs(v, range);
    let Some(&(rt, node, after)) = usize::try_from(n - 1)
        .ok()
        .and_then(|k| f.page_objs.get(k))
        .filter(|o| o.2 > 0)
    else {
        return Ok(None);
    };
    if rt >= 0 {
        return Ok(Some(rt + after));
    }
    let rt = crate::pdfprim::rt_before(r, u64::from_ne_bytes(node.to_ne_bytes()))?;
    Ok(Some(rt + after))
}

/// The fold of the pages' allocations ([`ALLOCS`]) over a range: the
/// objects they made and the fonts the shipouts loaded.
#[derive(Clone, Default)]
struct AllocFold {
    objects: i64,
    ship_fonts: i64,
    /// Each page's object: the read-time count where it shipped (-1:
    /// not in the entry), its shipping node, and the page-side objects
    /// before it plus 2 (0: an object a reference made before it).
    page_objs: Arc<Vec<(i64, i64, i64)>>,
    /// The font names the pages used.
    fonts: Arc<BTreeSet<ValueId>>,
    /// The destinations the pages made.
    dests: Arc<BTreeSet<ValueId>>,
    /// The objects the pages wrote into object streams.
    os_written: i64,
}

/// The writer's link stack and thread over the pages, as the fold of
/// their mark events ([`mark_events`]) runs them.
#[derive(Default)]
struct MarkState {
    /// Each running link's level and its action's target (kind, id).
    links: Vec<(i64, i64, ValueId)>,
    /// Running links off (`\\pdfrunninglinkoff`).
    off: bool,
    thread_level: i64,
    thread_running: bool,
    thread_id: Option<ValueId>,
    /// A bead on this page (`last_thread`).
    thread_last: bool,
    threads: BTreeSet<ValueId>,
    /// The pages whose objects a link's action made before they
    /// shipped (`get_obj(obj_type_page, n)`).
    premade: BTreeSet<i64>,
    /// The page being shipped (from 1): its object and those before
    /// exist.
    page: i64,
}

/// What a page's marks made: objects, and of them those written into an
/// object stream where they compress.
struct MarksMade {
    objects: i64,
    compressible: i64,
}

impl MarkState {
    fn page(&mut self, v: &Values, events: &[ValueId], dests: &mut BTreeSet<ValueId>) -> MarksMade {
        let mut made = MarksMade {
            objects: 0,
            compressible: 0,
        };
        self.thread_last = false;
        for &e in events {
            let ys = list_of(v, e);
            let n = |k: usize| ys.get(k).and_then(|&x| v.as_int(x)).unwrap_or(0);
            let id = |k: usize| ys.get(k).copied().unwrap_or(ValueId::UNIT);
            match n(0) {
                0 if !self.off => {
                    let pieces: Vec<(i64, ValueId)> = self
                        .links
                        .iter()
                        .filter(|l| l.0 == n(1))
                        .map(|l| (l.1, l.2))
                        .collect();
                    for (kind, target) in pieces {
                        made.objects += n(2);
                        made.compressible += n(2);
                        if n(2) > 0 {
                            made.objects += self.target(v, kind, target, dests);
                        }
                    }
                }
                1 => {
                    self.links.push((n(1), n(2), id(3)));
                    made.objects += self.target(v, n(2), id(3), dests);
                }
                2 => {
                    self.links.pop();
                }
                3 => self.off = n(1) == 0,
                4 if self.thread_last && self.thread_running && self.thread_level == n(1) => {
                    made.objects += 2 * n(2);
                    made.compressible += n(2);
                    let t = self.thread_id.unwrap_or(ValueId::UNIT);
                    if self.threads.insert(t) {
                        made.objects += 1;
                    }
                }
                5 => {
                    if n(1) != 0 {
                        self.thread_level = n(2);
                        self.thread_running = n(3) != 0;
                        self.thread_id = Some(id(4));
                    }
                    made.objects += 2;
                    made.compressible += 1;
                    if self.threads.insert(id(4)) {
                        made.objects += 1;
                    }
                    self.thread_last = true;
                }
                6 => self.thread_last = false,
                _ => {}
            }
        }
        made
    }

    /// A link piece's action target got (`get_obj` as the piece is
    /// written): an object where it is named first (a page's where it
    /// has not shipped and nothing made it).
    fn target(&mut self, v: &Values, kind: i64, id: ValueId, dests: &mut BTreeSet<ValueId>) -> i64 {
        let new = match kind {
            1 => dests.insert(id),
            2 => self.threads.insert(id),
            3 => {
                let p = v.as_int(id).unwrap_or(0);
                p > self.page && self.premade.insert(p)
            }
            _ => false,
        };
        i64::from(new)
    }
}

type AllocMemo = Mutex<FxHashMap<(u64, u64), AllocFold>>;

fn fold_allocs(v: &Values, range: ValueId) -> AllocFold {
    static M: OnceLock<AllocMemo> = OnceLock::new();
    let m = M.get_or_init(|| Mutex::new(FxHashMap::default()));
    let k = (v.instance(), range.raw());
    if let Some(x) = m.locked().get(&k) {
        return x.clone();
    }
    let mut out = AllocFold::default();
    let mut page_objs: Vec<(i64, i64, i64)> = Vec::new();
    let mut written = 0i64;
    let mut seen: BTreeSet<ValueId> = BTreeSet::new();
    let mut seen_dest: BTreeSet<ValueId> = BTreeSet::new();
    // (the destinations defined: each written once, where first defined;
    // another definition is ignored, pdfTeX's "duplicate ignored")
    let mut defined: BTreeSet<ValueId> = BTreeSet::new();
    let mut seen_ship: BTreeSet<ValueId> = BTreeSet::new();
    let mut ms = MarkState::default();
    for (pages, e) in v.seq_to_vec(range).into_iter().enumerate() {
        let xs = list_of(v, e);
        let i = |k: usize| xs.get(k).and_then(|&x| v.as_int(x)).unwrap_or(0);
        let l = |k: usize| xs.get(k).map(|&x| list_of(v, x)).unwrap_or_default();
        let pno = i64::try_from(pages).unwrap_or(i64::MAX - 1) + 1;
        ms.page = pno;
        for p in l(11) {
            ms.premade.insert(v.as_int(p).unwrap_or(0));
        }
        // (the fonts and destinations read-time writes named before the
        // page: an object each where none was made before, before the
        // page's own)
        for n in l(6) {
            if seen.insert(n) {
                out.objects += 1;
            }
        }
        for d in l(7) {
            if seen_dest.insert(d) {
                out.objects += 1;
            }
        }
        // (the page's own object, after its resources' one, unless a
        // reference or a link's action made it before)
        let fixed = if ms.premade.contains(&pno) { 2 } else { i(0) };
        page_objs.push((i(5), i(10), if fixed >= 3 { out.objects + 2 } else { 0 }));
        out.objects += fixed;
        // (a pages-tree node every `pages_tree_kids_max` pages)
        if pages % 6 == 0 {
            out.objects += 1;
        }
        for n in l(1) {
            if seen.insert(n) {
                out.objects += 1;
            }
        }
        let mut dests_written = 0i64;
        for d in l(2) {
            if seen_dest.insert(d) {
                out.objects += 1;
            }
            if defined.insert(d) {
                dests_written += 1;
            }
        }
        for f in l(3) {
            if seen_ship.insert(f) {
                out.ship_fonts += 1;
            }
        }
        // (the marks' objects past the fixed ones, `mark_events`: a link
        // piece each, a bead and its rectangle each, a destination,
        // thread or page a piece's action or a bead names first)
        let made = ms.page(v, &l(9), &mut seen_dest);
        out.objects += made.objects;
        // (the read-time writes into object streams before the page
        // count toward a stream's 100; a link piece's, a bead
        // rectangle's and a destination's object go into one too)
        let os = if i(4) > 0 {
            i(4) + made.compressible + dests_written
        } else {
            0
        };
        for _ in 0..os {
            if (written + i(8)) % 100 == 0 {
                out.objects += 1;
            }
            written += 1;
        }
    }
    out.page_objs = Arc::new(page_objs);
    out.fonts = Arc::new(seen);
    out.dests = Arc::new(seen_dest);
    out.os_written = written;
    let mut g = m.locked();
    if g.len() > 1 << 12 {
        g.clear();
    }
    g.insert(k, out.clone());
    out
}

/// The objects the pages shipped before this run made ([`ALLOCS`]): a
/// read-time allocation's number is TeX's count of the read-time ones
/// before it plus this. A fold of the accumulator's range before this
/// node: no round, no loop-back.
pub(crate) fn page_offset(r: &mut Run<'_, '_>) -> R<i64> {
    let v = r.v;
    let range = allocs_range(r, "page_offset")?;
    let f = fold_allocs(v, range);
    let (fonts, dests) = (r.get(RT_FONTS)?, r.get(RT_DESTS)?);
    Ok(f.objects + rt_new(v, &f, fonts, dests))
}

/// The objects read-time writes made for fonts and destinations they
/// named (`fonts`, `dests`: [`RT_FONTS`], [`RT_DESTS`] where the read
/// is) after the pages of fold `f`: those no page and no read-time
/// write before made.
fn rt_new(v: &Values, f: &AllocFold, fonts: Option<ValueId>, dests: Option<ValueId>) -> i64 {
    let new = |x: Option<ValueId>, seen: &BTreeSet<ValueId>| {
        x.map(|x| list_of(v, x))
            .unwrap_or_default()
            .iter()
            .filter(|n| !seen.contains(n))
            .count()
    };
    i64::try_from(new(fonts, &f.fonts) + new(dests, &f.dests)).unwrap_or(0)
}

/// [`page_offset`] where node `at` is, at its candidate count `c` (a
/// [`crate::pdfprim::REL`] number it made, read in another node): the
/// pages' allocations up to and including it, with those of the pages
/// that fired inside it before `c` (their finishers sit after it,
/// pdfTeX ships them before), and not those firing there later.
pub(crate) fn page_offset_at(r: &mut Run<'_, '_>, at: u64, c: i64) -> R<i64> {
    if at == r.cx.node().raw() {
        return page_offset(r);
    }
    let v = r.v;
    let to = phitex_defuse::NodeId::from_raw(at);
    let mut range = r.cx.acc_range_to(v.field(ALLOCS), None, Some(to))?;
    let me = i64::from_ne_bytes(at.to_ne_bytes());
    let after = r.cx.acc_range(v.field(ALLOCS), Some(to))?;
    for u in v.seq_to_vec(after) {
        let brk = list_of(v, u).get(12).map(|&b| list_of(v, b)).unwrap_or_default();
        if let [pid, k] = brk[..]
            && int_of(v, pid) == me
            && int_of(v, k) < c
        {
            range = v.seq_concat(range, v.seq_unit(u));
        }
    }
    let f = fold_allocs(v, range);
    let mut there = |name: &str| -> R<Option<ValueId>> {
        Ok(match r.cx.use_field_at(v.field(name), Some(to))? {
            phitex_defuse::FieldAt::Def(x, _) => Some(x),
            _ => None,
        })
    };
    let (fonts, dests) = (there(RT_FONTS)?, there(RT_DESTS)?);
    Ok(f.objects + rt_new(v, &f, fonts, dests))
}

/// Font `f`'s name number as pdfTeX gives it (`\\pdffontname`): its
/// number in the order of first loads ([`font_number`]) after the fonts
/// the shipouts before this run loaded ([`ALLOCS`]), which pdfTeX
/// numbers as it loads them.
pub(crate) fn font_tag_read(r: &mut Run<'_, '_>, f: i64) -> R<i64> {
    let n = font_number(r, f)?;
    let v = r.v;
    let range = allocs_range(r, "font_tag_read")?;
    Ok(n + fold_allocs(v, range).ship_fonts)
}

/// The fonts' numbers as a step sees them: their ranks in the order of
/// first loads, the format's fonts first ([`crate::api::FONT_ORDER`]),
/// then the loads before this run ([`FONT_LOADS`], in the order of the
/// runs' places) and this run's own. A provisional numbering (a page
/// written apart names its fonts by it): the link numbers by the line's
/// records in TeX's order ([`FontNums`]) and rewrites a name it moved.
pub(crate) fn load_numbers(r: &mut Run<'_, '_>) -> R<FxHashMap<i64, i64>> {
    static M: OnceLock<Once<Arc<FxHashMap<i64, i64>>>> = OnceLock::new();
    let v = r.v;
    let put = |k: i64, out: &mut FxHashMap<i64, i64>| {
        let n = i64::try_from(out.len()).unwrap_or(0) + 1;
        out.entry(k).or_insert(n);
    };
    let order = r.get(crate::api::FONT_ORDER)?;
    let f = v.field(FONT_LOADS);
    let before = r.cx.acc_range(f, None)?;
    // (the numbers of the format's fonts and of the loads before the run:
    // a pure function's memo by those values, every page step asks)
    let key = phitex_defuse::hash_words(
        0x6c6f_6164_6e75_6d73,
        &[order.map_or(0, ValueId::raw), before.raw()],
    );
    let m = M.get_or_init(|| Mutex::new(FxHashMap::default()));
    {
        let mut g = m.locked();
        if g.len() > 1 << 10 {
            g.clear();
        }
    }
    let base = once(m, key, || {
        let mut out: FxHashMap<i64, i64> = FxHashMap::default();
        if let Some(x) = order {
            for k in list_of(v, x) {
                put(int_of(v, k), &mut out);
            }
        }
        for k in v.seq_to_vec(before) {
            put(int_of(v, k), &mut out);
        }
        Arc::new(out)
    });
    let mut out = (*base).clone();
    for &(g, x) in &r.contribs {
        if g == f {
            for k in v.seq_to_vec(x) {
                put(int_of(v, k), &mut out);
            }
        }
    }
    Ok(out)
}

/// The number TeX gives font `f` (a key, [`crate::fonts::key_of`]): its
/// rank in the order of first loads, the format's fonts first
/// ([`crate::api::FONT_ORDER`]), then the line's `fontload` records
/// before this run in TeX's order ([`order_line`]) and this run's own.
/// Only output reads it (`\\pdffontname`; the writers number the fonts
/// of the whole line the same way).
pub(crate) fn font_number(r: &mut Run<'_, '_>, f: i64) -> R<i64> {
    let v = r.v;
    if f == 0 {
        return Ok(0);
    }
    let format_fonts: Vec<i64> = r
        .get(crate::api::FONT_ORDER)?
        .map(|x| list_of(v, x).iter().map(|&k| int_of(v, k)).collect())
        .unwrap_or_default();
    let mut seen: FxHashMap<i64, i64> = FxHashMap::default();
    for (i, &k) in format_fonts.iter().enumerate() {
        seen.entry(k).or_insert(i64::try_from(i).unwrap_or(0) + 1);
    }
    if let Some(&n) = seen.get(&f) {
        return Ok(n);
    }
    let line = v.seq_to_vec(r.cx.acc_range(v.field(XPAGES), None)?);
    let own: Vec<ValueId> = r
        .contribs
        .iter()
        .filter(|(g, _)| *g == v.field(XPAGES))
        .flat_map(|&(_, x)| v.seq_to_vec(x))
        .collect();
    let mut n = i64::try_from(seen.len()).unwrap_or(0);
    for x in order_line(v, &line).into_iter().chain(own) {
        if let Some(k) = fontload_key(v, x)
            && !seen.contains_key(&k)
        {
            n += 1;
            seen.insert(k, n);
            if k == f {
                return Ok(n);
            }
        }
    }
    Ok(0)
}

/// The id of `pdftexspace` in the pages written apart (none of the
/// engine's: its fonts are known by their keys, `crate::fonts::key_of`).
pub(crate) const SPACE_FONT: FontId = FontId::MAX - 15;

/// The fixed parts of the exact file the end node reads: the fields
/// reaching it.
pub(crate) struct EndFields {
    pub date: String,
    pub jobname: String,
    pub g2u: Option<ValueId>,
    /// The fonts loaded in all, by key in the order of their first loads
    /// (the format's, then the line's).
    pub fonts: Vec<FontId>,
    /// The format's fonts, key, name and size in the order it loaded them
    /// ([`FontNums::seed`]).
    pub seed: Vec<(FontId, String, i32)>,
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
    /// The writer's digests ([`PdfState::digest_parts`]) and the
    /// characters it used ([`PdfState::used_chars`]): taken when a later
    /// link first compares a writer with this one (most snapshots are
    /// never compared).
    digests: Option<SnapDigests>,
    nums: FontNums,
    forms: Forms,
    /// The read-time object numbers the link numbered otherwise
    /// ([`Reloc`]).
    reloc: Reloc,
    got: usize,
    errors: usize,
    placed: usize,
    shipped: usize,
}

/// Read-time object numbers whose objects the link's writer numbered
/// otherwise, by the number TeX read back: a command after a candidate
/// of its piece where a page fired, whose finisher sits after the piece
/// (the read-time number leaves that page's objects out; the writer,
/// placing the command after the page as pdfTeX ships it, counts them,
/// [`order_line`]). A page's whatsits naming one are shipped with the
/// writer's number.
/// A link's or an annotation's number only PDF bytes name
/// ([`crate::pdfprim::REL`]) is always mapped: the link numbers its object.
type Reloc = std::collections::BTreeMap<usize, usize>;

/// A form's whatsits (`data`, from the read-time ones `raw`) with the
/// [`crate::pdfprim::REL`] numbers they name as TeX reads them: a form is
/// written by the link's step for its command, which takes its whatsits
/// as they are, not by a page written apart.
pub(crate) fn form_whatsit_nums(
    r: &mut Run<'_, '_>,
    raw: &[ValueId],
    data: &mut [Option<Whatsit>],
) -> R<()> {
    let v = r.v;
    let mut map = Reloc::new();
    for &w in raw {
        let xs = list_of(v, w);
        let at = match xs.first().map(|&k| str_of(v, k)).as_deref() {
            Some("pdf:startlink") => 7,
            Some("pdf:annot") => 6,
            _ => continue,
        };
        // (its count in its node follows the node)
        let k = xs.get(at + 1).map_or(0, |&x| int_of(v, x));
        let t = xs.get(1).map_or(0, |&x| int_of(v, x));
        if t & crate::pdfprim::REL == 0 {
            continue;
        }
        let node = xs
            .get(at)
            .map_or(0, |&x| u64::from_ne_bytes(int_of(v, x).to_ne_bytes()));
        // (as the page's writer numbers it: the pages before the node)
        let n = crate::pdfprim::rel_number(r, node, k, 0)?;
        if let (Ok(t), Ok(n)) = (usize::try_from(t), usize::try_from(n)) {
            map.insert(t, n);
        }
    }
    if !map.is_empty() {
        for d in data.iter_mut() {
            if let Some(w) = d.take() {
                *d = Some(relocated(w, &map));
            }
        }
    }
    Ok(())
}

/// Whatsit `w` with its object numbers as the link numbered them.
fn relocated(w: Whatsit, reloc: &Reloc) -> Whatsit {
    let at = |n: usize| reloc.get(&n).copied().unwrap_or(n);
    match w {
        Whatsit::StartLink {
            objnum,
            width,
            height,
            depth,
            attr,
            action,
        } => Whatsit::StartLink {
            objnum: at(objnum),
            width,
            height,
            depth,
            attr,
            action,
        },
        Whatsit::Annot {
            objnum,
            width,
            height,
            depth,
            data,
        } => Whatsit::Annot {
            objnum: at(objnum),
            width,
            height,
            depth,
            data,
        },
        Whatsit::RefObj(n) => Whatsit::RefObj(at(n)),
        Whatsit::RefXForm(n) => Whatsit::RefXForm(at(n)),
        Whatsit::RefXImage(n) => Whatsit::RefXImage(at(n)),
        w => w,
    }
}

/// The object numbers whatsit `w` names (a link's, an annotation's, a
/// reference's).
fn what_objnum(w: &Whatsit) -> Option<usize> {
    match w {
        Whatsit::StartLink { objnum, .. } | Whatsit::Annot { objnum, .. } => Some(*objnum),
        Whatsit::RefObj(n) | Whatsit::RefXForm(n) | Whatsit::RefXImage(n) => Some(*n),
        _ => None,
    }
    .filter(|&n| !is_rel(n))
}

/// Whether object number `n` is a read-time one only PDF bytes name
/// ([`crate::pdfprim::REL`]).
fn is_rel(n: usize) -> bool {
    i64::try_from(n).is_ok_and(|n| n & crate::pdfprim::REL != 0)
}

/// The [`crate::pdfprim::REL`] numbers whatsit `w` names.
fn rel_objnum(w: &Whatsit) -> Option<usize> {
    match w {
        Whatsit::StartLink { objnum, .. } | Whatsit::Annot { objnum, .. } => Some(*objnum),
        _ => None,
    }
    .filter(|&n| is_rel(n))
}

/// The names a page written apart gives the [`crate::pdfprim::REL`]
/// numbers its whatsits name, `(name, number)`: in order, after every
/// number TeX numbered that it names (the largest of those, first) and
/// every object of the writer it is written on (`floor`, [`base_fold`]:
/// a name one of those had is scheduled there, so the record's link was
/// found scheduled where the line's is not, and the link shipped the
/// page again); the link gives each name the number the line gives its
/// object.
fn apart_names(whats: &BTreeMap<u32, Whatsit>, floor: usize) -> (usize, Vec<(usize, usize)>) {
    let named = whats.values().filter_map(what_objnum).max().unwrap_or(0);
    let from = named.max(floor);
    let mut rel: Vec<usize> = whats.values().filter_map(rel_objnum).collect();
    rel.sort_unstable();
    rel.dedup();
    let aliases = rel
        .into_iter()
        .enumerate()
        .map(|(i, t)| (from + 1 + i, t))
        .collect();
    (named, aliases)
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

/// A snapshot's digests ([`PdfState::digest_parts`]: but for the
/// object stream's bytes, and those bytes) and the characters its
/// writer used ([`LinkSnap`]).
type SnapDigests = (u64, u64, Vec<(FontId, [bool; 256])>);

impl LinkSnap {
    /// [`LinkSnap::digests`], taken now if not yet.
    fn digests(&mut self) -> &SnapDigests {
        if self.digests.is_none() {
            let st = &mut self.st;
            let (core, os) = st.digest_parts();
            self.digests = Some((core, os, st.used_chars()));
        }
        self.digests.as_ref().expect("taken above")
    }
}

/// How a writer compares with the last link's at a snapshot
/// ([`compare_writer`]).
enum SameWriter {
    /// The same writer: the records from there on are the same bytes.
    Same,
    /// The same but for the characters the pages used: the writer has
    /// used every character the snapshot's had (and maybe more). The
    /// records from there on are the same bytes (the characters used
    /// are read at the file's end only, the fonts' subsets), and the
    /// end's writer is the last link's with these characters added.
    ButChars,
    /// The same but for the bytes of the objects in the object stream
    /// being filled, which only the step writing that stream reads: the
    /// two are the same once that stream is written
    /// ([`PdfState::os_stream`]).
    ButObjectStream,
    /// Another writer.
    Other,
}

/// How the writer `st` with `nums`, `forms` and `reloc` compares with
/// the one the last link had at snapshot `sn` ([`finish_exact`]). The
/// forms compared only when there are none (a document with forms runs
/// on).
fn compare_writer(
    st: &mut PdfState,
    nums: &FontNums,
    forms: &Forms,
    reloc: &Reloc,
    sn: &mut LinkSnap,
) -> SameWriter {
    let none = forms.whats.is_empty()
        && forms.fonts.is_empty()
        && sn.forms.whats.is_empty()
        && sn.forms.fonts.is_empty();
    if !none || *nums != sn.nums || *reloc != sn.reloc {
        return SameWriter::Other;
    }
    // (the snapshot's digests taken once: no copy of its writer)
    let (core, os) = st.digest_parts();
    let (sn_core, sn_os, chars) = sn.digests();
    if core != *sn_core {
        return SameWriter::Other;
    }
    let mine = st.used_chars();
    let covers = chars.iter().all(|(f, c)| {
        mine.iter()
            .find(|m| m.0 == *f)
            .is_some_and(|m| c.iter().zip(&m.1).all(|(&a, &b)| !a || b))
    });
    if !covers {
        SameWriter::Other
    } else if os != *sn_os {
        SameWriter::ButObjectStream
    } else if mine == *chars {
        SameWriter::Same
    } else {
        SameWriter::ButChars
    }
}

/// Why the writer at record `i` is not the last link's there
/// (`PHITEX_LINK_WHY`, observe): the writer's fields that differ, with
/// their first differing lines.
#[cold]
fn link_why(i: usize, st: &PdfState, nums: &FontNums, reloc: &Reloc, sn: &LinkSnap) {
    use std::fmt::Write as _;
    // (a field of the writer: a line indented by one level)
    fn fields(s: &str) -> Vec<(String, Vec<&str>)> {
        let mut out: Vec<(String, Vec<&str>)> = Vec::new();
        for l in s.lines() {
            let top = l.starts_with("    ") && !l.starts_with("     ");
            if top || out.is_empty() {
                let name = l.trim().split(':').next().unwrap_or("").to_owned();
                out.push((name, Vec::new()));
            }
            if let Some(f) = out.last_mut() {
                f.1.push(l);
            }
        }
        out
    }
    let a = format!("{st:#?}");
    let b = format!("{:#?}", sn.st);
    let (fa, fb) = (fields(&a), fields(&b));
    let mut msg = format!(
        "link why at record {i}: font numbers {}, reloc {}",
        if *nums == sn.nums { "equal" } else { "differ" },
        if *reloc == sn.reloc {
            "equal".to_owned()
        } else {
            format!("differs: {reloc:?} vs {:?}", sn.reloc)
        }
    );
    for (n, la) in &fa {
        let Some((_, lb)) = fb.iter().find(|x| x.0 == *n) else {
            let _ = write!(msg, "\n  field {n}: not in the last link's");
            continue;
        };
        if la == lb {
            continue;
        }
        let p = la.iter().zip(lb).take_while(|(x, y)| x == y).count();
        let q = la[p..]
            .iter()
            .rev()
            .zip(lb[p..].iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        let _ = write!(
            msg,
            "\n  field {n}: lines {}..{} of {} differ (last link's {}..{} of {})",
            p,
            la.len() - q,
            la.len(),
            p,
            lb.len() - q,
            lb.len()
        );
        for l in la[p..la.len() - q].iter().take(6) {
            let _ = write!(
                msg,
                "\n    now  {}",
                l.chars().take(160).collect::<String>()
            );
        }
        for l in lb[p..lb.len() - q].iter().take(6) {
            let _ = write!(
                msg,
                "\n    last {}",
                l.chars().take(160).collect::<String>()
            );
        }
    }
    eprintln!("{msg}");
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
    /// The pages the link placed as their own steps wrote them, and those
    /// it shipped again ([`PdfState::replay_page`]'s refusals).
    pub placed: u64,
    pub shipped: u64,
}

/// What a link did with the pages ([`Linked::placed`], `shipped`):
/// `[placed, shipped]`, the end node's contribution
/// (`crate::Build::link_pages`).
pub(crate) const LINKED: &str = "pdf:linked";

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
pub(crate) fn finish_exact(
    v: &Values,
    f: &EndFields,
    xpages: &[ValueId],
    cmds: &CmdMemo,
) -> Linked {
    let out = link(v, f, xpages, cmds, true);
    if crate::exec::obs_flag!("PHITEX_LINK_CHECK") {
        // (the file linked from the last link's records, and from the
        // start: the same bytes)
        let full = link(v, f, xpages, cmds, false);
        let at = out
            .file
            .iter()
            .zip(&full.file)
            .take_while(|(a, b)| a == b)
            .count();
        assert!(
            out.file == full.file && out.errors == full.errors,
            "link check: the file linked from the last link's records ({} bytes) is not the \
             file linked from the start ({} bytes): they differ from byte {at}",
            out.file.len(),
            full.file.len()
        );
        eprintln!(
            "link check: {} bytes, the same linked from the start",
            out.file.len()
        );
    }
    out
}

/// [`finish_exact`]'s link, from the last link's records (`memo`, kept
/// for the next) or from the start.
#[allow(clippy::too_many_lines)]
fn link(v: &Values, f: &EndFields, xpages: &[ValueId], cmds: &CmdMemo, memo: bool) -> Linked {
    let none = Linked {
        file: Vec::new(),
        hash: None,
        errors: Vec::new(),
        placed: 0,
        shipped: 0,
    };
    if xpages.is_empty() {
        return none;
    }
    let t0 = std::time::Instant::now();
    let mut st = fresh(v, f.date.clone(), &f.jobname, f.g2u);
    let mut forms = Forms::default();
    let mut reloc = Reloc::new();
    let mut nums = FontNums::default();
    nums.seed(&f.seed);
    let mut bytes = PdfBytes::new();
    let mut got: Vec<ValueId> = Vec::new();
    let mut errors: Vec<(String, i64)> = Vec::new();
    let (mut placed, mut shipped) = (0usize, 0usize);
    let pcs: Vec<Option<Arc<PagePiece>>> = {
        let m = pieces().locked();
        xpages
            .iter()
            .map(|&x| {
                v.as_int(x)
                    .and_then(|k| m.get(&u64::from_ne_bytes(k.to_ne_bytes())).cloned())
            })
            .collect()
    };
    // (the commands made so far: one made after this point is not one
    // this link can have dropped)
    let hi = CMD_BORN.load(std::sync::atomic::Ordering::Relaxed);
    let serials: FxHashMap<u64, Arc<Cmd>> = {
        let mut m = cmds.locked();
        // (the tag compared as a value: no string made per record)
        let cmd_tag = v.str("cmd");
        let serials: FxHashMap<u64, Arc<Cmd>> = xpages
            .iter()
            .filter_map(|&x| {
                let xs = list_of(v, x);
                (xs.first() == Some(&cmd_tag))
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
            m.retain(|s, c| c.born > hi || serials.contains_key(s));
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
    for (k, n, sz) in &f.seed {
        head = phitex_defuse::hash_words(
            phitex_defuse::hash_bytes(head, n.as_bytes()),
            &[u64::from(sz.cast_unsigned()), u64::from(*k)],
        );
    }
    let mut chunks_of: Vec<Vec<phitex_pdf::Chunk>> = Vec::with_capacity(xpages.len());
    let mut snaps: Vec<LinkSnap> = Vec::new();
    let mut start = 0usize;
    // (the last link's memo, taken: an edit's records after the change
    // that are the last link's again, from a writer state equal to the
    // one it had there, are copied too, `resync` below)
    let mut old: Option<LinkMemo> = memo
        .then(|| link_memo().locked().take())
        .flatten()
        .filter(|m| m.head == head);
    if let Some(m) = old.as_mut() {
        let same = m.ids.iter().zip(&ids).take_while(|(a, b)| a == b).count();
        if let Some(sn) = m.snaps.iter().rev().find(|sn| sn.at <= same && sn.at > 0) {
            st = sn.st.clone();
            nums = sn.nums.clone();
            forms = sn.forms.clone();
            reloc = sn.reloc.clone();
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
    let mut resync: Option<(usize, usize)> = None;
    // (where the writer there had used other characters: those of the
    // pages before, added to the last link's end, [`SameWriter::ButChars`])
    let mut chars_added: Option<Vec<(FontId, [bool; 256])>> = None;
    let mut tries = 0usize;
    // (the records the line and the last link end with alike, and how
    // many more records the last link had: record `i` of the line is
    // its record `i + more` there)
    let (suffix, more) = old.as_ref().map_or((0, 0), |m| {
        let n = m
            .ids
            .iter()
            .rev()
            .zip(ids.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        (n, m.ids.len().cast_signed() - ids.len().cast_signed())
    });
    // (a writer the same as the last link's but for the object stream
    // being filled: compared again once that stream is written)
    let mut os_wait: Option<usize> = None;
    let (mut t_try, mut t_snap) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
    for (i, (&x, pc)) in xpages.iter().zip(&pcs).enumerate().skip(start) {
        let tt = std::time::Instant::now();
        if os_wait.is_some_and(|c| st.os_stream() != c) {
            os_wait = None;
        }
        if i > start
            && tries < 8
            && os_wait.is_none()
            && ids.len() - i <= suffix
            && let Some(m) = old.as_mut()
            && let Some(j) = i.checked_add_signed(more)
            && let Some(sn) = m.snaps.iter_mut().find(|sn| sn.at == j)
        {
            match compare_writer(&mut st, &nums, &forms, &reloc, sn) {
                SameWriter::Same => {
                    resync = Some((i, j));
                    t_try += tt.elapsed();
                    break;
                }
                SameWriter::ButChars => {
                    resync = Some((i, j));
                    chars_added = Some(st.used_chars());
                    t_try += tt.elapsed();
                    break;
                }
                SameWriter::ButObjectStream => {
                    os_wait = Some(st.os_stream());
                    if crate::exec::obs_flag!("PHITEX_LINK_WHY") {
                        eprintln!(
                            "link why at record {i}: the same but for object stream {}'s bytes",
                            st.os_stream()
                        );
                    }
                }
                SameWriter::Other => {
                    tries += 1;
                    if crate::exec::obs_flag!("PHITEX_LINK_WHY") {
                        link_why(i, &st, &nums, &reloc, sn);
                    }
                }
            }
        }
        t_try += tt.elapsed();
        let tt = std::time::Instant::now();
        if i > start && (i - start).is_multiple_of(LINK_SNAP) {
            // (the writer's digests' sums brought up to date first: a
            // later link comparing a writer with this snapshot, or going
            // on from it, hashes only what changed after it, not every
            // object written since the link began)
            st.refresh_digests();
            snaps.push(LinkSnap {
                at: i,
                digests: None,
                st: st.clone(),
                nums: nums.clone(),
                forms: forms.clone(),
                reloc: reloc.clone(),
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
            // (a page whose PDF side has not run yet: written here)
            write_page(v, pc);
            let empty = phitex_pdf::PageRecord::default();
            let rec = pc.made.get().map_or(&empty, |m| &m.0);
            let fwd = nums.resolve(&pc.fonts, Some(rec.font_loads()));
            let src = Mapped::new(&pc.fonts, &nums, &fwd);
            let fmap = |g: FontId| fwd.get(&g).copied().unwrap_or(g);
            set_page_params(v, &mut st, &pc.params, pc.margins);
            let n0 = st.objects_numbered();
            // (a page naming an object the link numbered otherwise than
            // TeX read it is shipped with the link's number)
            let moved = pc
                .whats
                .values()
                .filter_map(what_objnum)
                .any(|n| reloc.contains_key(&n));
            // (the names the page was written apart with for the
            // read-time numbers only its bytes name: the line's numbers
            // of their objects)
            let floor = pc.made.get().map_or(0, |m| m.2);
            let (_, aliases) = apart_names(&pc.whats, floor);
            let tex: FxHashMap<usize, usize> = aliases
                .iter()
                .filter_map(|&(p, t)| reloc.get(&t).map(|&m| (p, m)))
                .collect();
            let res = if moved {
                Err("an object numbered otherwise at read time".to_owned())
            } else if tex.len() != aliases.len() {
                Err("an object the line does not number".to_owned())
            } else {
                let at = |n: usize| tex.get(&n).copied().unwrap_or(n);
                st.replay_page(rec, &src, &fmap, &at, None, &mut ps)
            };
            if res.is_err() {
                // (the node that wrote the page logged the writer's
                // error when it wrote it)
                let mut wh = |h: u32, _leaders: bool| {
                    page_whatsit(&forms, &pc.whats, h).map(|w| relocated(w, &reloc))
                };
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
                        let mut g = c.f.locked();
                        (*g)(&mut st, &mut forms, &src)
                    };
                    let rt = c.rt.load(std::sync::atomic::Ordering::Relaxed);
                    if let (Ok(rt @ 1..), Some(&m)) = (usize::try_from(rt), out.nums.first())
                        && let Ok(m) = usize::try_from(m)
                        && m != rt
                    {
                        reloc.insert(rt, m);
                    }
                    if crate::exec::obs_flag!("PHITEX_PDF_TRACE") {
                        eprintln!(
                            "  link command {serial} of node {}: {:?} (objects {})",
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
                _ => {
                    if let Some(wp) = doc_op_params(v, x) {
                        set_writer_params(&mut st, wp);
                    }
                    apply_doc_op(v, &mut st, x);
                }
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
            resync.map_or(xpages.len(), |r| r.0)
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
    if let Some((i, j)) = resync
        && let Some(mut m) = old.take()
        && let Some(sn) = m.snaps.iter().find(|sn| sn.at == j)
    {
        let (sg, se, sp, ss) = (sn.got, sn.errors, sn.placed, sn.shipped);
        resynced = xpages.len() - i;
        chunks_of.extend(m.chunks.drain(j - m.chunk_base..));
        // (the last link's snapshots from there on, at this line's
        // records and counts)
        let (g0, e0, p0, s0) = (got.len(), errors.len(), placed, shipped);
        // (each with the characters this line's records before `i` used
        // and the last link's before `j` did not, as the end's writer
        // below: a later link starting from one of them starts from the
        // characters this line's pages used, not the last link's)
        snaps.extend(m.snaps.into_iter().filter(|x| x.at >= j).map(|mut x| {
            if let Some(c) = &chars_added {
                x.st.add_used_chars(c);
                if let Some(d) = x.digests.as_mut() {
                    d.2 = x.st.used_chars();
                }
            }
            x.at = x.at - j + i;
            x.got = x.got - sg + g0;
            x.errors = x.errors - se + e0;
            x.placed = x.placed - sp + p0;
            x.shipped = x.shipped - ss + s0;
            x
        }));
        got.extend(m.got[sg..].iter().copied());
        errors.extend(m.errors.drain(se..));
        placed += m.end.placed - sp;
        shipped += m.end.shipped - ss;
        st = m.end.st;
        nums = m.end.nums;
        forms = m.end.forms;
        reloc = m.end.reloc;
        if let Some(c) = &chars_added {
            st.add_used_chars(c);
        }
    }
    for c in chunks_of.iter().flatten() {
        bytes.push(c.clone());
    }
    st.refresh_digests();
    let end = LinkSnap {
        at: xpages.len(),
        digests: None,
        st: st.clone(),
        nums: nums.clone(),
        forms: forms.clone(),
        reloc: reloc.clone(),
        got: got.len(),
        errors: errors.len(),
        placed,
        shipped,
    };
    if memo {
        *link_memo().locked() = Some(LinkMemo {
            head,
            ids,
            chunks: chunks_of,
            snaps,
            got: got.clone(),
            errors: errors.clone(),
            end,
            chunk_base: 0,
        });
    }
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
    nums.end(&f.fonts);
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
    let kept = tails().locked().get(&key).cloned();
    let tail = match kept {
        Some(t) => Ok(Some(t)),
        None => st.tail().map(|t| {
            let t = t.map(Arc::new);
            if let Some(t) = &t {
                tails().locked().insert(key, t.clone());
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
        placed: u64::try_from(placed).unwrap_or(0),
        shipped: u64::try_from(shipped).unwrap_or(0),
    }
}
