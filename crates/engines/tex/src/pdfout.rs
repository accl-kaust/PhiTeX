//! The PDF file as a fold over the document (pdfTeX §750–§815): the
//! writer's state (`phitex_pdf::PdfState`) after each step that touches
//! it, a step being a PDF command when TeX reads it or a `\shipout`.
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

use phitex_defuse::{Ctx, Stop, Value, ValueId, Values};
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

/// `pdftex_banner` of the oracle's pdfTeX.
pub(crate) const BANNER: &str = "This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux) kpathsea version 6.4.2";

/// A font map by TFM name.
type Map = BTreeMap<Vec<u8>, MapEntry>;

type MapCache = Mutex<FxHashMap<u64, Arc<Map>>>;

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
    pub(crate) missed: std::cell::RefCell<BTreeSet<Vec<u8>>>,
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

type PsCache = Mutex<FxHashMap<u64, Arc<PsMap>>>;

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
    let key = m.raw();
    if let Some(c) = ps_maps()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return Ok(Some(c.clone()));
    }
    let parsed = Arc::new(parse_map_ps(&bytes_of(&str_of(v, m))));
    ps_maps()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, parsed.clone());
    Ok(Some(parsed))
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
            self.missed.borrow_mut().insert(name.to_vec());
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
/// local fonts (`[k, font]`), empty for a font that is not virtual.
pub(crate) fn do_vf(r: &mut Run<'_, '_>, f: u32) -> R<()> {
    let v = r.v;
    let key = format!("pdf:vf:{f}");
    if r.get(&key)?.is_some() {
        return Ok(());
    }
    let info = crate::fonts::info(v, r.get(&format!("fontinfo:{f}"))?);
    let Some(i) = info.filter(|i| i.file.is_some()) else {
        r.set(&key, v.list(Vec::new()), true);
        return Ok(());
    };
    let Some(bytes) = file_bytes(r, &format!("{}.vf", i.name))? else {
        r.set(&key, v.list(Vec::new()), true);
        return Ok(());
    };
    let Ok(vf) = phitex_tfm::parse_vf(&bytes) else {
        r.set(&key, v.list(Vec::new()), true);
        return Ok(());
    };
    let mut locals = Vec::new();
    for lf in &vf.fonts {
        let fs = store_scaled_f(lf.scaled, i.size);
        let name = String::from_utf8_lossy(&lf.name).into_owned();
        let count = r.int("fontcount")?;
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
    let v = r.v;
    Ok(r.file(name)?.and_then(|x| {
        list_of(v, x)
            .get(1)
            .map(|&t| Arc::from(bytes_of(&str_of(v, t))))
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
    let key = m.raw();
    if let Some(c) = maps()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return Ok(Some(c.clone()));
    }
    let parsed = Arc::new(parse_map(&bytes_of(&str_of(v, m))));
    maps()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, parsed.clone());
    Ok(Some(parsed))
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
        // REDUCE-GAP: pdfTeX loads the space font at the shipout that
        // first needs it, as the next internal font number; here the
        // number after the fonts so far (the engine's `fontcount` does
        // not count it)
        let n = r.int("fontcount")?;
        let id = u32::try_from(n + 1).unwrap_or(u32::MAX - 1);
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
        .get("pdf:maps")?
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
    let k0 = u64::from_ne_bytes(r.int("pdf:state")?.to_ne_bytes());
    let a = states()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&k0)
        .cloned();
    Ok((k0, a))
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
    let (_, before) = before(r)?;
    let (mut st, mut bytes, mut forms, bytes_hash) = if let Some(a) = before {
        (a.st.clone(), a.bytes.clone(), a.forms.clone(), a.bytes_hash)
    } else {
        {
            let date = r
                .get("pdfdate")?
                .map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
            let jobname = r.get("jobname")?.map(|x| str_of(v, x)).unwrap_or_default();
            let p = PdfParams {
                date,
                banner: BANNER.to_owned(),
                output_file_name: format!("{jobname}.pdf"),
                ..PdfParams::default()
            };
            (PdfState::new(p), PdfBytes::new(), Forms::default(), 0)
        }
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
    states()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(k1)
        .or_insert_with(|| {
            Arc::new(Fold {
                st,
                bytes,
                forms,
                bytes_hash,
            })
        });
    r.set(
        "pdf:state",
        v.int(i64::from_ne_bytes(k1.to_ne_bytes())),
        true,
    );
    Ok(out)
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
        let shown = crate::pdfrelaxed::ship(r, &p, bx, &fonts, &mut wh)?;
        let attr = p.get(15).map(|&a| str_of(v, a)).unwrap_or_default();
        return Ok(phitex_defuse::hash_bytes(shown, attr.as_bytes()));
    }
    // the fonts of the forms the page may ship too
    let mut all: BTreeSet<u32> = ids.iter().copied().collect();
    all.extend(forms_here(r)?.fonts);
    let all: Vec<u32> = all.into_iter().collect();
    let mut fonts = fonts_for(r, &all, space)?;
    let whats = side.whats.clone();
    let images = resolved
        .values()
        .any(|&d| list_of(v, d).first().map(|&k| str_of(v, k)).as_deref() == Some("pdf:refximage"))
        || forms_here(r)?
            .whats
            .values()
            .any(|w| matches!(w, Whatsit::RefXImage(_)));
    if images {
        // an included PDF's fonts replaced by the map's (`copyFont`
        // off): the files a dry run of the page asks for, read now
        fonts.ps = default_ps_map(r)?;
        if let (_, Some(a)) = before(r)? {
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
        for n in std::mem::take(&mut *fonts.missed.borrow_mut()) {
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

/// The PDF file: the state that reaches the end, finished.
pub(crate) fn finish(cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
    let v = cx.values();
    let k = cx
        .use_field(v.field("pdf:state"))?
        .map_or(0, |x| u64::from_ne_bytes(int_of(v, x).to_ne_bytes()));
    let Some(a) = states()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&k)
        .cloned()
    else {
        return Ok(v.str(""));
    };
    let mut st = a.st.clone();
    let int = |cx: &mut Ctx<'_>, f: &str| -> Result<i32, Stop> {
        Ok(cx
            .use_field(v.field(f))?
            .map_or(0, |x| i32::try_from(int_of(v, x)).unwrap_or(0)))
    };
    st.params_mut().info_omit_date = int(cx, "param:pdfinfoomitdate")?;
    st.params_mut().suppress_ptex_info = int(cx, "param:pdfsuppressptexinfo")?;
    let bytes = phitex_pdf::finish(st, &a.bytes).unwrap_or_else(|e| {
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!("pdf: {e}");
        }
        Vec::new()
    });
    let s: String = bytes.iter().map(|&b| char::from(b)).collect();
    Ok(v.intern(Value::Str(s.into_boxed_str())))
}
