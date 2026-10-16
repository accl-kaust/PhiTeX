//! The PDF file as a fold over the document (pdfTeX §750–§815): the
//! writer's state (`phitex_pdf::PdfState`) after each step that touches
//! it, a step being a PDF command when TeX reads it or a page's
//! placement. A finisher writes its page apart ([`ship_apart`]: the
//! box decoded, the fonts, the content stream compressed, on the state
//! reaching the finisher, as a record) and places a [`Place`] node after
//! itself; that node puts the record on the state in the line's order
//! (`PdfState::replay_page`), so the page's objects are numbered after
//! every writer command before it and before every one after, as
//! pdfTeX numbers them, while the pages' expensive part runs apart.
//!
//! The field `pdf:state` holds a key: the digest of the state after the
//! step (`PdfState::digest`, the bytes so far, the forms' table), so an
//! equal state gives an equal key however it was made. The key names a memo entry, the state after the step and the
//! file's bytes so far (`PdfBytes`, append-only: chunks shared between
//! entries). What TeX reads back (`\pdfcolorstackinit`'s number,
//! `\pdffontobjnum`, ...) comes from the state the step made. At the end
//! the `Dvi` node finishes the state that reaches it.

use std::collections::{BTreeMap, BTreeSet};

use phitex_defuse::FxHashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{Ctx, Effect, Field, Locality, MayDefine, Operation, Stop, ValueId, Values};
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
use crate::ops::{Shared, ops, run_node_parking};
use crate::run::{R, Run};
use crate::val::{int_of, list_of, str_of};

/// The accumulator of the exact writer's pages, in document order: each
/// entry the key of a [`PagePiece`] ([`pieces`]), for the fonts' names
/// over the pages ([`font_tags`]), with the document-level commands the
/// output routines ran ([`doc_op`]).
pub(crate) const XPAGES: &str = "pdf:xpages";

/// One page as the exact writer ships it, made by the page's own step
/// with nothing read from another page's: the box, the parameters
/// `\shipout` read, the whatsits' data, the fonts. The page's [`Place`]
/// node puts it on the writer's state in the line's order; the step has
/// already written the page once, apart, so that the placement finds
/// its content stream compressed.
pub(crate) struct PagePiece {
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
    /// [`font_tags`].
    pj: i64,
    fcount: i64,
}

type PieceMemo = Mutex<FxHashMap<u64, Arc<PagePiece>>>;

/// The pages by key: a pure function's memo.
fn pieces() -> &'static PieceMemo {
    static P: OnceLock<PieceMemo> = OnceLock::new();
    P.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// Whether the exact writer folds the pages (`pdf:fold`): a document
/// whose output routine runs a PDF command.
fn folding(r: &mut Run<'_, '_>) -> R<bool> {
    Ok(r.cint(crate::fields::cell::PDF_FOLD)? != 0)
}

/// Whether this run is a finisher.
fn in_page_step(r: &Run<'_, '_>) -> bool {
    r.cx.op() == r.ops.finish
}

/// Asks for the fold (`pdf:fold`): the build is evaluated again with the
/// pages in the writer's `pdf:state` fold.
fn fold_back(r: &mut Run<'_, '_>, why: &str) {
    let v = r.v;
    let id = v.int(i64::from_ne_bytes(r.cx.node().raw().to_ne_bytes()));
    let e = v.list(vec![v.str("pdf:fold"), id, v.str(why)]);
    r.contribute("needs", v.seq_unit(e));
}

/// A document-level command (`\pdfinfo`, `\pdfcatalog`, `\pdfnames`,
/// `\pdftrailer`, `\pdftrailerid`) an output routine runs: kept in the
/// pages' accumulator ([`XPAGES`]) where it was run, and applied by the
/// finish there, so that the step reads no writer state. True when it
/// was kept (the caller does nothing more).
pub(crate) fn doc_op(r: &mut Run<'_, '_>, kind: &str, args: &[ValueId]) -> R<bool> {
    if r.param("pdfoutput")? <= 0 || crate::pdfrelaxed::on(r)? || !in_page_step(r) || folding(r)? {
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

/// One state of the fold: the writer, the file so far, the forms.
pub(crate) struct Fold {
    st: PdfState,
    bytes: PdfBytes,
    forms: Forms,
    /// A hash of `bytes`, chunk by chunk.
    bytes_hash: u64,
}

type States = Mutex<FxHashMap<u64, Arc<Fold>>>;

/// The writer's states by key: a pure function's memo (the key is the
/// state's digest: [`step_forms`]).
fn states() -> &'static States {
    static S: OnceLock<States> = OnceLock::new();
    S.get_or_init(|| Mutex::new(FxHashMap::default()))
}

type PreStates = Mutex<FxHashMap<u64, Arc<Fold>>>;

/// A writer state by its page-base digest (`pdf:pre`,
/// `PdfState::page_base_digest` with the forms'): the latest state a
/// step left with it. A finisher writes its page apart on it, reading
/// only the digest, not the whole state's key (`pdf:state`): an earlier
/// page placed anew changes the key, seldom the digest.
fn pre_states() -> &'static PreStates {
    static S: OnceLock<PreStates> = OnceLock::new();
    S.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// The writer's state a finisher writes its page apart on: one with the
/// page-base digest reaching it (`pdf:pre`), else the state reaching it.
/// The digest stands for the state's key in the page's memo key.
fn page_base(r: &mut Run<'_, '_>) -> R<(u64, Option<Arc<Fold>>)> {
    if in_page_step(r) && !folding(r)? {
        if let Some(x) = r.get("pdf:pre")? {
            let k = u64::from_ne_bytes(int_of(r.v, x).to_ne_bytes());
            let a = pre_states()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&k)
                .cloned();
            if a.is_some() {
                return Ok((k, a));
            }
        }
    }
    before(r)
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
                h = phitex_defuse::hash_bytes(h, b.as_deref().unwrap_or_default());
            }
        }
        let mut files: Vec<_> = self.files.iter().collect();
        files.sort_unstable_by(|a, b| a.0.cmp(b.0));
        for (n, b) in files {
            h = phitex_defuse::hash_bytes(phitex_defuse::hash_bytes(h, n), b);
        }
        phitex_defuse::hash_words(h, &[u64::from(self.space.unwrap_or(0))])
    }
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
    let info = crate::fonts::info(v, r.get(&format!("fontinfo:{f}"))?);
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
            if let Some(j) = crate::fonts::info(v, r.get(&format!("fontinfo:{g}"))?)
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
                Node::Char { font, .. } => out.push(*font),
                Node::Lig(l) => out.push(l.font),
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
        let name = crate::fonts::info(v, r.get(&format!("fontinfo:{f}"))?).map(|i| i.name);
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
        if let Some(i) = crate::fonts::info(v, r.get(&format!("fontinfo:{f}"))?)
            && let Some(file) = i.file
        {
            wanted.push((f, i.name, file, i.s));
        }
    }
    if space && let Some(x) = r.file("pdftexspace.tfm")? {
        // pdfTeX loads the space font at the shipout that first needs
        // it, as the next internal font number (the engine's
        // `fontcount` does not count it)
        let id = if in_page_step(r) && !folding(r)? && !crate::pdfrelaxed::on(r)? {
            // a page written apart: one id for every page, outside the
            // fonts' range, numbered by the finish where it is first
            // loaded ([`font_tags`])
            SPACE_FONT
        } else {
            let n = r.cint(crate::fields::cell::FONTCOUNT)?;
            u32::try_from(n + 1).unwrap_or(u32::MAX - 1)
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

/// The fold's state where this node is (`pdf:state`), if a step made
/// one.
fn before(r: &mut Run<'_, '_>) -> R<(u64, Option<Arc<Fold>>)> {
    let k0 = u64::from_ne_bytes(r.cint(crate::fields::cell::PDF_STATE)?.to_ne_bytes());
    if let Some((k, a)) = &r.pdf_fold
        && *k == k0
    {
        return Ok((k0, Some(a.clone())));
    }
    let a = states()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&k0)
        .cloned();
    Ok((k0, a))
}

/// The writer's state a run's last step made, where other nodes find
/// it: the run ends (finishes or parks) with `pdf:state` naming it.
pub(crate) fn publish(fold: Option<(u64, Arc<Fold>)>) {
    if let Some((k, a)) = fold {
        states()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(k)
            .or_insert(a);
    }
}

/// The forms' part of the fold's state where this node is.
pub(crate) fn forms_here(r: &mut Run<'_, '_>) -> R<Forms> {
    Ok(before(r)?.1.map(|a| a.forms.clone()).unwrap_or_default())
}

/// A step of the fold: the state after the previous step (a fresh one
/// at the start), `f` applied, the bytes it wrote flushed; `input` is
/// everything the step depends on.
pub(crate) fn step<T>(
    r: &mut Run<'_, '_>,
    input: ValueId,
    fonts: &Fonts,
    f: impl FnOnce(&mut PdfState, &Fonts) -> T,
) -> R<T> {
    step_forms(r, input, fonts, |st, _, fs| f(st, fs))
}

/// [`step`], the forms' table in reach.
pub(crate) fn step_forms<T>(
    r: &mut Run<'_, '_>,
    input: ValueId,
    fonts: &Fonts,
    f: impl FnOnce(&mut PdfState, &mut Forms, &Fonts) -> T,
) -> R<T> {
    // the inputs were read where they were made; the key is the state
    let _ = input;
    let v = r.v;
    let (k0, before) = before(r)?;
    // (the state this run's previous step left is the run's own: taken,
    // not copied, when nothing else holds it)
    let before = before.map(|a| {
        if r.pdf_fold.as_ref().is_some_and(|(k, _)| *k == k0) {
            r.pdf_fold = None;
        }
        Arc::try_unwrap(a).unwrap_or_else(|a| Fold {
            st: a.st.clone(),
            bytes: a.bytes.clone(),
            forms: a.forms.clone(),
            bytes_hash: a.bytes_hash,
        })
    });
    let (mut st, mut bytes, mut forms, bytes_hash) = if let Some(a) = before {
        (a.st, a.bytes, a.forms, a.bytes_hash)
    } else {
        let date = r
            .get_loc(crate::fields::cell::PDFDATE)?
            .map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
        let jobname = r
            .get_loc(crate::fields::cell::JOBNAME)?
            .map(|x| str_of(v, x))
            .unwrap_or_default();
        let g2u = r.get("pdf:glyphtounicode")?;
        (
            fresh(v, date, &jobname, g2u),
            PdfBytes::new(),
            Forms::default(),
            0,
        )
    };
    // the parameters a write reads (the version, compression: fixed
    // at the first write, pdfTeX §792), as they are at this step
    {
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
        let pm = st.params_mut();
        pm.compress_level = ps[0].clamp(0, 9);
        pm.obj_compress_level = ps[1];
        pm.decimal_digits = ps[2];
        pm.minor_version = ps[3];
        pm.major_version = ps[4];
    }
    let out = f(&mut st, &mut forms, fonts);
    let n0 = bytes.chunks().len();
    st.flush(&mut bytes);
    // the key is the state the step made (the writer's, the file's
    // bytes so far, the forms'), not how it was made: a step that leaves
    // it equal leaves `pdf:state` equal, and what reads it is not run
    let mut bytes_hash = bytes_hash;
    for c in &bytes.chunks()[n0..] {
        bytes_hash = phitex_defuse::hash_bytes(bytes_hash, &c.bytes);
    }
    let fh = phitex_defuse::hash_bytes(0, format!("{:?}", (&forms.whats, &forms.fonts)).as_bytes());
    let k1 = phitex_defuse::hash_words(st.digest(), &[bytes_hash, fh]).max(1);
    if crate::exec::env_flag!("PHITEX_PDF_TRACE") {
        eprintln!(
            "  pdf step {} ({:?}): {} -> {} (digest {}, {} bytes) {}",
            r.cx.node().raw(),
            r.cx.op(),
            k0 % 100_000,
            k1 % 100_000,
            st.digest() % 100_000,
            bytes.chunks().iter().map(|c| c.bytes.len()).sum::<usize>(),
            v.render(input).chars().take(50).collect::<String>()
        );
    }
    // (kept in the run: only the state the run ends with can be read
    // elsewhere, [`publish`]; a run of a thousand writer commands, a
    // format's `\pdfglyphtounicode` table, keeps one state, not a
    // thousand copies)
    let pre = phitex_defuse::hash_words(st.page_base_digest(), &[fh]).max(1);
    let fold = Arc::new(Fold {
        st,
        bytes,
        forms,
        bytes_hash,
    });
    pre_states()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(pre, fold.clone());
    r.pdf_fold = Some((k1, fold));
    r.set(
        "pdf:state",
        v.int(i64::from_ne_bytes(k1.to_ne_bytes())),
        true,
    );
    r.set("pdf:pre", v.int(i64::from_ne_bytes(pre.to_ne_bytes())), true);
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

/// A read-time command on the writer (`pdf:state` fold step) in PDF
/// mode, with the fonts it names.
pub(crate) fn command<T: Default>(
    r: &mut Run<'_, '_>,
    input: ValueId,
    fonts: &[u32],
    f: impl FnOnce(&mut PdfState, &Fonts) -> T,
) -> R<T> {
    if r.param("pdfoutput")? <= 0 {
        return Ok(T::default());
    }
    if crate::pdfrelaxed::on(r)? {
        crate::pdfrelaxed::fall_back(r, "a command the relaxed mode does not cover");
        return Ok(T::default());
    }
    if in_page_step(r) && !folding(r)? {
        // its state would be the pages' before it: stage B's loop-back
        let what = r.v.render(input).chars().take(60).collect::<String>();
        fold_back(r, &format!("a PDF command in an output routine: {what}"));
        return Ok(T::default());
    }
    let fs = fonts_for(r, fonts, false)?;
    step(r, input, &fs, f)
}

/// [`command`], the forms' table in reach.
pub(crate) fn command_forms<T: Default>(
    r: &mut Run<'_, '_>,
    input: ValueId,
    fonts: &[u32],
    f: impl FnOnce(&mut PdfState, &mut Forms, &Fonts) -> T,
) -> R<T> {
    if r.param("pdfoutput")? <= 0 {
        return Ok(T::default());
    }
    if crate::pdfrelaxed::on(r)? {
        crate::pdfrelaxed::fall_back(r, "a command the relaxed mode does not cover");
        return Ok(T::default());
    }
    if in_page_step(r) && !folding(r)? {
        // its state would be the pages' before it: stage B's loop-back
        let what = r.v.render(input).chars().take(60).collect::<String>();
        fold_back(r, &format!("a PDF command in an output routine: {what}"));
        return Ok(T::default());
    }
    let fs = fonts_for(r, fonts, false)?;
    step_forms(r, input, &fs, f)
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
    vf_for(r, bx)?;
    let space = resolved.values().any(|&d| {
        matches!(
            list_of(v, d).first().map(|&k| str_of(v, k)).as_deref(),
            Some("iwon" | "fake")
        )
    });
    if crate::pdfrelaxed::on(r)? {
        let fonts = fonts_for(r, ids, space)?;
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
    let base_forms = page_base(r)?.1.map(|a| a.forms.clone()).unwrap_or_default();
    all.extend(base_forms.fonts.iter().copied());
    let all: Vec<u32> = all.into_iter().collect();
    let mut fonts = fonts_for(r, &all, space)?;
    let whats = side.whats.clone();
    let images = resolved
        .values()
        .any(|&d| list_of(v, d).first().map(|&k| str_of(v, k)).as_deref() == Some("pdf:refximage"))
        || base_forms
            .whats
            .values()
            .any(|w| matches!(w, Whatsit::RefXImage(_)));
    if images {
        // an included PDF's fonts replaced by the map's (`copyFont`
        // off): the files a dry run of the page asks for, read now
        fonts.ps = default_ps_map(r)?;
        if let (_, Some(a)) = page_base(r)? {
            let mut st = a.st.clone();
            let forms = &a.forms;
            page_params(v, st.params_mut(), &p);
            let mut wh = |h: u32, _l: bool| -> Option<Whatsit> {
                if h >= FORM_HANDLES {
                    return forms.whats.get(&h).cloned();
                }
                resolved.get(&h).and_then(|&d| whatsit(v, d))
            };
            let _ = st.ship_out_with(bx, &fonts, &mut wh);
        }
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
    if in_page_step(r) && !folding(r)? {
        return ship_apart(r, page, &p, bx, &side.whats, &resolved, fonts, margins);
    }
    let (err, shown) = step_forms(r, page, &fonts, |st, forms, fonts| {
        page_params(v, st.params_mut(), &p);
        let pm = st.params_mut();
        pm.link_margin = margins[0];
        pm.dest_margin = margins[1];
        pm.thread_margin = margins[2];
        let mut wh = |h: u32, _leaders: bool| -> Option<Whatsit> {
            if h >= FORM_HANDLES {
                return forms.whats.get(&h).cloned();
            }
            if let Some(&d) = resolved.get(&h) {
                return whatsit(v, d);
            }
            let w = *whats.get(h as usize)?;
            let ys = list_of(v, w);
            if ys.first().map(|&k| str_of(v, k)).as_deref() == Some("special") {
                Some(Whatsit::Special(bytes_of(
                    &ys.get(1).map(|&s| str_of(v, s)).unwrap_or_default(),
                )))
            } else {
                None
            }
        };
        let e = st.ship_out_with(bx, fonts, &mut wh).err();
        let (content, w, h) = st.last_page_shown();
        let words = [
            u64::from_ne_bytes(i64::from(w).to_ne_bytes()),
            u64::from_ne_bytes(i64::from(h).to_ne_bytes()),
        ];
        (
            e,
            phitex_defuse::hash_words(phitex_defuse::hash_bytes(0, content), &words),
        )
    })?;
    // the rotation and the rest of the page's dictionary: `\pdfpageattr`
    let attr = p.get(15).map(|&a| str_of(v, a)).unwrap_or_default();
    let shown = phitex_defuse::hash_bytes(shown, attr.as_bytes());
    if let Some(e) = err {
        // PDF-BACKEND-GAP: the writer's errors are pdfTeX's fatal errors;
        // they go to the log, the job going on
        r.log("error", &format!("pdfTeX error: {e}"))?;
    }
    Ok(shown)
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
    let mut pc = PagePiece {
        boxv: list_of(v, page_v).first().copied().unwrap_or(ValueId::UNIT),
        params: p.to_vec(),
        margins,
        whats,
        fonts,
        rec: phitex_pdf::PageRecord::default(),
        base: BTreeSet::new(),
        pj: i64::try_from(r.page_index()).unwrap_or(i64::MAX),
        fcount: r.cint(crate::fields::cell::FONTCOUNT)?,
    };
    // the key: what the piece is made of, by the values' identities
    // (interned: equal values, equal ids), not by their bytes
    let (k0, _) = page_base(r)?;
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
    let key = phitex_defuse::hash_words(0x7067_7069_6563_6531, &words).max(1);
    // the page written once, apart, on a state with the page-base digest
    // reaching the finisher (`page_base`)
    let (_, a) = page_base(r)?;
    let (mut st, forms) = if let Some(a) = a {
        (a.st.clone(), a.forms.clone())
    } else {
        let date = r
            .get_loc(crate::fields::cell::PDFDATE)?
            .map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
        let jobname = r
            .get_loc(crate::fields::cell::JOBNAME)?
            .map(|x| str_of(v, x))
            .unwrap_or_default();
        let g2u = r.get("pdf:glyphtounicode")?;
        (fresh(v, date, &jobname, g2u), Forms::default())
    };
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
    if let Some(e) = err {
        // PDF-BACKEND-GAP: the writer's errors are pdfTeX's fatal errors;
        // they go to the log, the job going on
        r.log("error", &format!("pdfTeX error: {e}"))?;
    }
    let pj = pc.pj;
    pieces()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key)
        .or_insert_with(|| Arc::new(pc));
    let k = v.int(i64::from_ne_bytes(key.to_ne_bytes()));
    r.contribute(XPAGES, v.seq_unit(k));
    // the page's place in the file: a node after this finisher that puts
    // the page on the writer's state where the finisher stands
    // ([`Place`]); its objects then come after every writer command
    // before it in the line, and before every one after
    r.cx.follow(r.ops.place, vec![k, v.int(pj)]);
    r.placed_page = true;
    Ok(shown)
}

/// A page's placement: arguments `[key, j]`, the [`PagePiece`] the
/// finisher wrote apart and the page's index. The node reads the
/// writer's state reaching it (`pdf:state`: the read-time commands' and
/// the pages' before it, a chain the finishers and the commands thread
/// by def-use), puts the page on it (`PdfState::replay_page`: the
/// recorded page renumbered, or shipped again where it cannot be
/// placed) and defines the next state. The expensive part of a page
/// (its box decoded, its fonts, its content stream compressed) was
/// done apart by the finisher; this step is the cheap, serial one.
///
/// Example: pieces `A B` with a page finishing at the end of `A`, `B`
/// running `\pdfobj`: the line is `A, finish, place, B`; `B`'s object
/// is numbered after the page's, as pdfTeX numbers it.
pub(crate) struct Place(pub Shared);

impl Operation for Place {
    fn name(&self) -> &'static str {
        "place"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, _: &[ValueId], v: &Values) -> MayDefine {
        MayDefine::Names(vec![v.field("pdf:state"), v.field("pdf:pre")])
    }
    /// The writer's state: the node waits for it instead of starting
    /// and stopping.
    fn reads(&self, _: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(vec![v.field("pdf:state")])
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args();
        let o = ops(&self.0);
        run_node_parking(cx, o, true, |r| place_node(r, &args))
    }
}

/// A [`Place`] node's run.
fn place_node(r: &mut Run<'_, '_>, args: &[ValueId]) -> R<()> {
    let v = r.v;
    let Some(&kx) = args.first() else { return Ok(()) };
    let key = u64::from_ne_bytes(int_of(v, kx).to_ne_bytes());
    let Some(pc) = pieces()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned()
    else {
        r.engine_diag("page placement: the page's piece is not in the memo");
        return Ok(());
    };
    // the fonts' names as pdfTeX numbers them, over the pages placed so
    // far (the accumulator before this node, in order: `font_tags` is
    // cumulative, a page's numbers never change by a later page)
    let xp = v.seq_to_vec(r.cx.acc_range(v.field(XPAGES), None)?);
    let placed: Vec<Arc<PagePiece>> = {
        let m = pieces().lock().unwrap_or_else(PoisonError::into_inner);
        xp.iter()
            .filter(|&&x| v.as_int(x).is_some())
            .filter_map(|&x| m.get(&u64::from_ne_bytes(int_of(v, x).to_ne_bytes())).cloned())
            .collect()
    };
    let fcount = placed.last().map_or(pc.fcount, |p| p.fcount.max(pc.fcount));
    let tags = font_tags(fcount, &placed);
    // (a stop at the state's read parks the run: it resumes here)
    r.begin_command();
    let t0 = std::time::Instant::now();
    let res = step_forms(r, kx, &pc.fonts, |st, forms, fonts| {
        if let Some(t) = tags {
            st.set_font_tags(t);
        }
        set_page_params(v, st, &pc.params, pc.margins);
        let n0 = st.objects_numbered();
        let res = st.replay_page(&pc.rec, fonts, &pc.base, None);
        if crate::exec::env_flag!("PHITEX_PDF_TRACE") {
            eprintln!(
                "  place page {}: objects {n0} -> {} ({})",
                pc.pj,
                st.objects_numbered(),
                match &res {
                    Ok(_) => "replayed".to_owned(),
                    Err(e) => format!("shipped again: {e}"),
                }
            );
        }
        if res.is_err() {
            // (the finisher logged the writer's error when it wrote the
            // page)
            let mut wh = |h: u32, _leaders: bool| page_whatsit(forms, &pc.whats, h);
            let mut side = crate::nodes::Side::default();
            if let Some(phitex_typeset::node::Node::Box(bx)) =
                crate::nodes::decode(v, pc.boxv, &mut side)
            {
                let _ = st.ship_out_with(&bx, fonts, &mut wh);
            }
        }
        res
    })?;
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        match &res {
            Err(why) => eprintln!("place: page {} shipped again ({:?}): {why}", pc.pj, t0.elapsed()),
            Ok(Some(why)) => eprintln!("place: page {} ({:?}): {why}", pc.pj, t0.elapsed()),
            Ok(None) => eprintln!("place: page {} placed ({:?})", pc.pj, t0.elapsed()),
        }
    }
    Ok(())
}


/// The loop-back of the fonts' names the finish gave
/// ([`font_tags`]): `[[engine number, name number], ...]` where they
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

/// The fonts' numbers as pdfTeX gives them, where they differ from the
/// engine's (`None`: nowhere). The engine numbers the fonts in the
/// order the line loads them (`fontcount`, a field every load defines
/// for what follows it: the text's and a finisher's output routine's
/// alike, a finisher being a node in the line), which is pdfTeX's
/// order except where a `\shipout` loads fonts as its walk reads them
/// in: a virtual font's local fonts at its `do_vf` (the engine loads
/// them before the walk, [`vf_for`]) and `pdftexspace`
/// ([`SPACE_FONT`]) where a faked space first needs it. `pages` are the
/// placed pages in order; `t` the fonts loaded in all.
///
/// Example: text fonts 1 and 2, then a page whose virtual font 2 loads
/// its local font 3 and whose walk needs the space font, then a text
/// font 4: pdfTeX's numbers 1, 2, 3, space 4, then 5 for font 4.
fn font_tags(t: i64, pages: &[Arc<PagePiece>]) -> Option<std::collections::HashMap<FontId, FontId>> {
    let mut map = std::collections::HashMap::new();
    let mut done: BTreeSet<FontId> = BTreeSet::new();
    let mut next: FontId = 0;
    let mut put = |f: FontId| {
        if done.insert(f) {
            next += 1;
            if f != next {
                map.insert(f, next);
            }
        }
    };
    let id = |f: i64| FontId::try_from(f).unwrap_or(0);
    // the fonts loaded before the pages placed so far
    let mut steps = 0i64;
    for pc in pages {
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
                put(id(f));
            }
        }
        // then as the walk reads them in
        for (vf, f) in pc.rec.font_loads() {
            if vf {
                if let Some(x) = pc.fonts.vfs.get(&f) {
                    for &(_, g) in &x.fonts {
                        if mine(g) {
                            put(g);
                        }
                    }
                }
            } else if f == SPACE_FONT {
                put(f);
            }
        }
        for f in steps + 1..=pc.fcount {
            put(id(f));
        }
        steps = steps.max(pc.fcount);
    }
    for f in steps + 1..=t {
        put(id(f));
    }
    (!map.is_empty()).then_some(map)
}

/// The fixed parts of the exact file the end node reads: the fields
/// reaching it.
pub(crate) struct EndFields {
    /// `pdf:state`: the key of the writer's state the read-time
    /// commands left (none: no command touched it).
    pub state: Option<u64>,
    pub date: String,
    pub jobname: String,
    pub g2u: Option<ValueId>,
    /// `fontcount`: the fonts loaded in all.
    pub fonts: i64,
    pub info_omit_date: i32,
    pub suppress_ptex_info: i32,
}

/// The exact PDF file (the end node's): the writer's state reaching the
/// end (the read-time commands' steps and the pages' placements, in the
/// line's order), the document-level commands the output routines ran
/// applied ([`apply_doc_op`], from the pages' accumulator [`XPAGES`]),
/// the fonts' names in pdfTeX's order over all the pages
/// ([`font_tags`]), then `phitex_pdf::finish`. The empty string: no
/// file.
pub(crate) fn finish_exact(v: &Values, f: &EndFields, xpages: &[ValueId]) -> Vec<u8> {
    let start = f.state.and_then(|k| {
        states()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&k)
            .cloned()
    });
    let a = match start {
        Some(a) => a,
        None if xpages.is_empty() => return Vec::new(),
        None => Arc::new(Fold {
            st: fresh(v, f.date.clone(), &f.jobname, f.g2u),
            bytes: PdfBytes::new(),
            forms: Forms::default(),
            bytes_hash: 0,
        }),
    };
    let t0 = std::time::Instant::now();
    let mut st = a.st.clone();
    let mut bytes = a.bytes.clone();
    // the pages are on the state, each placed by its node in the line
    // ([`Place`]); the accumulator gives the pages' pieces (for the
    // fonts' names over all of them) and the document-level commands
    let placed: Vec<Arc<PagePiece>> = {
        let m = pieces().lock().unwrap_or_else(PoisonError::into_inner);
        xpages
            .iter()
            .filter(|&&x| v.as_int(x).is_some())
            .filter_map(|&x| m.get(&u64::from_ne_bytes(int_of(v, x).to_ne_bytes())).cloned())
            .collect()
    };
    if let Some(tags) = font_tags(f.fonts, &placed) {
        st.set_font_tags(tags);
    }
    for &x in xpages {
        if v.as_int(x).is_some() {
            continue;
        }
        apply_doc_op(v, &mut st, x);
    }
    st.flush(&mut bytes);
    st.params_mut().info_omit_date = f.info_omit_date;
    st.params_mut().suppress_ptex_info = f.suppress_ptex_info;
    let out = phitex_pdf::finish(st, &bytes).unwrap_or_else(|e| {
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!("pdf: {e}");
        }
        Vec::new()
    });
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "pdf pages: {} placed in the line; file {} bytes, {:?}",
            placed.len(),
            out.len(),
            t0.elapsed()
        );
    }
    out
}
