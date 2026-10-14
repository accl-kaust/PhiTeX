//! The relaxed PDF mode (`crates/output/pdf/RELAXED.md`): every page,
//! and every object TeX makes when it reads a command, written on its
//! own by `phitex_pdf::relaxed`, under handles; the `Dvi` node merges
//! the pieces and numbers the objects. No `pdf:state` fold: what passes
//! from page to page is the carry (`pdf:carry`: the colour stacks, the
//! open links, the two switches, the fonts used), and what TeX reads
//! back (`\pdflastobj`, `\pdfpageref`, ...) is a handle.
//!
//! Whatever the mode does not cover asks for exact mode (a `pdf:exact`
//! need): the build is then evaluated again with the fold.

use phitex_defuse::FxHashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{Ctx, Stop, Value, ValueId, Values};
use phitex_pdf::relaxed::{self, Carry, DocCmd, PageEnv, Piece};
use phitex_pdf::{PdfParams, Whatsit};
use phitex_typeset::node::BoxNode;

use crate::fonts::bytes_of;
use crate::pdfout::{BANNER, Fonts};
use crate::run::{R, Run};
use crate::val::{int_of, list_of, str_of};

/// The accumulator of pieces: `[0, key]` for a page, `[1, key]` for an
/// object made at read time, in document order.
pub(crate) const PIECES: &str = "pdf:pieces";

type Memo<T> = Mutex<FxHashMap<u64, Arc<T>>>;

/// The pieces by digest: a pure function's memo.
fn pieces() -> &'static Memo<Piece> {
    static P: OnceLock<Memo<Piece>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// The carry and the pages shipped before it, by digest.
fn carries() -> &'static Memo<(Carry, usize)> {
    static C: OnceLock<Memo<(Carry, usize)>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// The pages written, by the digest of their inputs.
fn written() -> &'static Memo<Piece> {
    static W: OnceLock<Memo<Piece>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// The pages the relaxed writer wrote, all builds together (gate 3's
/// counter).
static PAGES_WRITTEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Pages written in relaxed mode so far in this process.
#[must_use]
pub fn pages_written() -> u64 {
    PAGES_WRITTEN.load(std::sync::atomic::Ordering::Relaxed)
}

fn key_int(v: &Values, k: u64) -> ValueId {
    v.int(i64::from_ne_bytes(k.to_ne_bytes()))
}

fn int_key(v: &Values, x: ValueId) -> u64 {
    u64::from_ne_bytes(int_of(v, x).to_ne_bytes())
}

/// Whether this build writes the PDF file in relaxed mode.
pub(crate) fn on(r: &mut Run<'_, '_>) -> R<bool> {
    Ok(r.param("pdfoutput")? > 0
        && r.cint(crate::fields::cell::PDF_RELAXED)? == 1
        && r.cint(crate::fields::cell::PDF_EXACT)? == 0)
}

/// Asks for exact mode: the build is evaluated again with the fold.
pub(crate) fn fall_back(r: &mut Run<'_, '_>, why: &str) {
    let v = r.v;
    let id = key_int(v, r.cx.node().raw());
    let e = v.list(vec![v.str("pdf:exact"), id, v.str(why)]);
    r.contribute("needs", v.seq_unit(e));
}

/// The owner key of the next object this run makes at read time.
fn next_key(r: &mut Run<'_, '_>) -> u64 {
    let k = relaxed::owner_key(r.cx.node().raw(), r.pdf_occ);
    r.pdf_occ += 1;
    k
}

/// A new handle (`pdf_new_objnum`, `reserveobjnum`).
pub(crate) fn new_handle(r: &mut Run<'_, '_>) -> i64 {
    let k = next_key(r);
    i64::try_from(relaxed::read_handle(k, 0)).unwrap_or(0)
}

/// The parameters a write reads, with the ones the first write fixed
/// (`pdf:fixed`, set by the first).
fn params(r: &mut Run<'_, '_>) -> R<PdfParams> {
    let v = r.v;
    let date = r
        .get_loc(crate::fields::cell::PDFDATE)?
        .map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
    let jobname = r
        .get_loc(crate::fields::cell::JOBNAME)?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    let mut p = PdfParams {
        date,
        banner: BANNER.to_owned(),
        output_file_name: format!("{jobname}.pdf"),
        ..PdfParams::default()
    };
    p.compress_level = r.param("pdfcompresslevel")?.clamp(0, 9);
    p.obj_compress_level = r.param("pdfobjcompresslevel")?;
    p.decimal_digits = r.param("pdfdecimaldigits")?;
    p.minor_version = r.param("pdfminorversion")?;
    p.major_version = r.param("pdfmajorversion")?;
    p.draftmode = r.param("pdfdraftmode")?;
    p.image_hicolor = r.param("pdfimagehicolor")?;
    p.inclusion_copy_fonts = r.param("pdfinclusioncopyfonts")?;
    Ok(p)
}

/// `p` with the parameters the first write fixed; the first write
/// records them.
fn fix(r: &mut Run<'_, '_>, p: &PdfParams) -> R<PdfParams> {
    let v = r.v;
    let first = if let Some(x) = r.get_loc(crate::fields::cell::PDF_FIXED)? {
        fixed_of(v, x, p)
    } else {
        {
            let xs = [
                p.major_version,
                p.minor_version,
                p.obj_compress_level,
                p.decimal_digits,
                p.draftmode,
                p.image_hicolor,
                p.inclusion_copy_fonts,
            ];
            let l = v.list(xs.iter().map(|&x| v.int(i64::from(x))).collect());
            r.set_cell(crate::fields::cell::PDF_FIXED, l, true);
            p.clone()
        }
    };
    Ok(relaxed::fixed_params(&first, p))
}

fn fixed_of(v: &Values, x: ValueId, p: &PdfParams) -> PdfParams {
    let xs = list_of(v, x);
    let g = |k: usize| {
        xs.get(k)
            .map_or(0, |&y| i32::try_from(int_of(v, y)).unwrap_or(0))
    };
    let mut q = p.clone();
    q.major_version = g(0);
    q.minor_version = g(1);
    q.obj_compress_level = g(2);
    q.decimal_digits = g(3);
    q.draftmode = g(4);
    q.image_hicolor = g(5);
    q.inclusion_copy_fonts = g(6);
    q
}

/// `\pdfmapfile` (true) and `\pdfmapline` (false) texts read so far.
fn maps(r: &mut Run<'_, '_>) -> R<Vec<(bool, Vec<u8>)>> {
    let v = r.v;
    let Some(x) = r.get_loc(crate::fields::cell::PDF_MAPS)? else {
        return Ok(Vec::new());
    };
    Ok(list_of(v, x)
        .into_iter()
        .map(|e| {
            let ys = list_of(v, e);
            let file = ys.first().map(|&k| str_of(v, k)).as_deref() == Some("file");
            let t = ys
                .get(1)
                .map(|&t| bytes_of(&str_of(v, t)))
                .unwrap_or_default();
            (file, t)
        })
        .collect())
}

/// The pieces of the objects `ws` refers to, read where they were made
/// (`pdf:h:<handle>`), so that a page or form depends on them.
fn referenced(
    r: &mut Run<'_, '_>,
    ws: impl Iterator<Item = Whatsit>,
) -> R<FxHashMap<usize, Arc<Piece>>> {
    let v = r.v;
    let mut out = FxHashMap::default();
    for w in ws {
        let (Whatsit::RefXForm(h) | Whatsit::RefXImage(h)) = w else {
            continue;
        };
        if h < relaxed::HANDLE_BASE || out.contains_key(&h) {
            continue;
        }
        if let Some(x) = r.get(&format!("pdf:h:{h}"))? {
            let k = int_key(v, x);
            let p = pieces()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&k)
                .cloned();
            if let Some(p) = p {
                out.insert(h, p);
            }
        }
    }
    Ok(out)
}

/// A piece kept: by its digest, contributed in document order; a
/// read-time object's also under its handle.
fn keep(r: &mut Run<'_, '_>, page: bool, p: Piece) -> u64 {
    let v = r.v;
    let k = p.digest();
    if let Some(h) = p.handle().filter(|_| !page) {
        r.set(&format!("pdf:h:{h}"), key_int(v, k), true);
    }
    pieces()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(k)
        .or_insert_with(|| Arc::new(p));
    let e = v.list(vec![v.int(i64::from(!page)), key_int(v, k)]);
    r.contribute(PIECES, v.seq_unit(e));
    k
}

/// The carry where this node is, with the pages shipped before.
fn carry_here(r: &mut Run<'_, '_>) -> R<(Carry, usize)> {
    let v = r.v;
    let got = r.get_loc(crate::fields::cell::PDF_CARRY)?.and_then(|x| {
        carries()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&int_key(v, x))
            .cloned()
    });
    let (mut c, n) = got.map(|a| (*a).clone()).unwrap_or_default();
    // the colour stacks `\pdfcolorstackinit` made so far
    let inits: Vec<(bool, bool, Vec<u8>)> = r
        .get_loc(crate::fields::cell::PDF_COLORSTACKS)?
        .map(|x| list_of(v, x))
        .unwrap_or_default()
        .into_iter()
        .map(|e| {
            let ys = list_of(v, e);
            let g = |k: usize| ys.get(k).map_or(0, |&y| int_of(v, y));
            let t = ys
                .get(2)
                .map(|&t| bytes_of(&str_of(v, t)))
                .unwrap_or_default();
            (g(0) != 0, g(1) == 2, t)
        })
        .collect();
    c.sync_stacks(&inits);
    Ok((c, n))
}

/// `pdf_ship_out` in relaxed mode: page `n` written on its own, the
/// carry folded on. `p` is the page's parameters as `\shipout` read
/// them; the result is the page's hash for `pagehash`.
pub(crate) fn ship(
    r: &mut Run<'_, '_>,
    p: &[ValueId],
    bx: &BoxNode,
    fonts: &Fonts,
    wh: &mut dyn FnMut(u32) -> Option<Whatsit>,
) -> R<u64> {
    let v = r.v;
    let mut pm = params(r)?;
    crate::pdfout::page_params(v, &mut pm, p);
    pm.link_margin = r.param("pdflinkmargin")?;
    pm.dest_margin = r.param("pdfdestmargin")?;
    pm.thread_margin = r.param("pdfthreadmargin")?;
    let pm = fix(r, &pm)?;
    let (carry, n) = carry_here(r)?;
    // the whatsits once, for the objects they refer to
    let mut seen: FxHashMap<u32, Option<Whatsit>> = FxHashMap::default();
    let mut look = |h: u32| -> Option<Whatsit> { seen.entry(h).or_insert_with(|| wh(h)).clone() };
    let mut all = Vec::new();
    collect_whats(bx, &mut |h| all.push(look(h)));
    let refs = referenced(r, all.into_iter().flatten())?;
    let by = |h: usize| refs.get(&h).cloned();
    let maps = maps(r)?;
    let env = PageEnv {
        pieces: &by,
        maps: &maps,
        carry: &carry,
    };
    let mut whf = |h: u32, _: bool| look(h);
    // the writer is a pure function of these: a page whose inputs are
    // unchanged is not written again
    let mut ref_keys: Vec<(usize, u64)> = refs.iter().map(|(&h, p)| (h, p.digest())).collect();
    ref_keys.sort_unstable();
    let mut whats: Vec<(u32, Option<Whatsit>)> = Vec::new();
    collect_whats(bx, &mut |h| whats.push((h, whf(h, false))));
    let input = phitex_defuse::hash_bytes(
        phitex_defuse::hash_words(
            fonts.digest(),
            &[carry.digest(), u64::try_from(n).unwrap_or(0)],
        ),
        format!("{pm:?}|{bx:?}|{whats:?}|{maps:?}|{ref_keys:?}").as_bytes(),
    );
    let memo = written()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&input)
        .cloned();
    let piece = if let Some(p) = memo {
        (*p).clone()
    } else {
        PAGES_WRITTEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match relaxed::page(&pm, n + 1, bx, fonts, &mut whf, &env) {
            Ok(p) => {
                written()
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(input, Arc::new(p.clone()));
                p
            }
            Err(e) => {
                fall_back(r, &format!("page {}: {e}", n + 1));
                return Ok(0);
            }
        }
    };
    let next = relaxed::carry_after(&carry, bx, &mut whf);
    let ck = phitex_defuse::hash_words(next.digest(), &[u64::try_from(n + 1).unwrap_or(0)]);
    carries()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(ck)
        .or_insert_with(|| Arc::new((next, n + 1)));
    r.set_cell(crate::fields::cell::PDF_CARRY, key_int(v, ck), true);
    if let Some(h) = piece.page_height()
        && i64::from(h) != r.cint(crate::fields::cell::PDF_LASTPH)?
    {
        r.set_cell(crate::fields::cell::PDF_LASTPH, v.int(i64::from(h)), true);
    }
    Ok(keep(r, true, piece))
}

/// Calls `f` with every whatsit handle of `b`, leaders' boxes included.
fn collect_whats(b: &BoxNode, f: &mut dyn FnMut(u32)) {
    use phitex_typeset::node::Node;
    for n in &b.list {
        match n {
            Node::Whatsit(h, _) => f(*h),
            Node::Box(x) => collect_whats(x, f),
            Node::Glue(g) => {
                if let Some(l) = &g.leader
                    && let Node::Box(x) = &**l
                {
                    collect_whats(x, f);
                }
            }
            _ => {}
        }
    }
}

/// `\pdfobj` in relaxed mode: the object made on its own; its handle.
pub(crate) fn obj(
    r: &mut Run<'_, '_>,
    given: Option<i64>,
    stream: bool,
    attr: Option<Vec<u8>>,
    body: phitex_pdf::ObjBody,
) -> R<i64> {
    let key = next_key(r);
    let given = match given {
        Some(n) => match usize::try_from(n) {
            Ok(h) if h >= relaxed::HANDLE_BASE => Some(h),
            _ => {
                fall_back(
                    r,
                    "\\pdfobj useobjnum with a number not given in relaxed mode",
                );
                return Ok(n);
            }
        },
        None => None,
    };
    let p = params(r)?;
    let p = fix(r, &p)?;
    match relaxed::obj(&p, key, given, stream, attr, body) {
        Ok(piece) => {
            let h = piece.handle().unwrap_or(0);
            keep(r, false, piece);
            Ok(i64::try_from(h).unwrap_or(0))
        }
        Err(e) => {
            fall_back(r, &format!("\\pdfobj: {e}"));
            Ok(0)
        }
    }
}

/// `\pdfxform` in relaxed mode: the box shipped as a form on its own;
/// its handle.
pub(crate) fn xform(
    r: &mut Run<'_, '_>,
    bx: &BoxNode,
    data: &[Option<Whatsit>],
    fonts: &Fonts,
    attr: Option<Vec<u8>>,
    resources: Option<Vec<u8>>,
    immediate: bool,
) -> R<Option<usize>> {
    let key = next_key(r);
    // the form's origin: the last page's height for an immediate one,
    // else the height of the pages that will reference it
    let height = if immediate {
        (
            i32::try_from(r.cint(crate::fields::cell::PDF_LASTPH)?).unwrap_or(0),
            false,
        )
    } else {
        (r.param("pdfpageheight")?, true)
    };
    let p = params(r)?;
    let p = fix(r, &p)?;
    let refs = referenced(r, data.iter().flatten().cloned())?;
    let by = |h: usize| refs.get(&h).cloned();
    let maps = maps(r)?;
    let carry = Carry::default();
    let env = PageEnv {
        pieces: &by,
        maps: &maps,
        carry: &carry,
    };
    let mut wh = |h: u32, _: bool| data.get(h as usize).cloned().flatten();
    match relaxed::xform(&p, key, attr, resources, bx, fonts, &mut wh, &env, height) {
        Ok(piece) => {
            let h = piece.handle();
            keep(r, false, piece);
            Ok(h)
        }
        Err(e) => {
            fall_back(r, &format!("\\pdfxform: {e}"));
            Ok(None)
        }
    }
}

/// `\pdfximage` in relaxed mode: the image read and written on its own;
/// its handle and what TeX reads of it.
pub(crate) fn ximage(
    r: &mut Run<'_, '_>,
    spec: phitex_pdf::ImageSpec,
    ps: &[i32],
) -> R<Option<(usize, phitex_pdf::XImageInfo)>> {
    let key = next_key(r);
    let mut p = params(r)?;
    p.image_hicolor = ps[2];
    p.image_apply_gamma = ps[3];
    p.gamma = ps[4];
    p.image_gamma = ps[5];
    p.inclusion_copy_fonts = ps[6];
    let p = fix(r, &p)?;
    // an included PDF's fonts replaced by the map's: the files a dry run
    // asks for, read now
    let mut fonts = Fonts::default();
    fonts.ps = crate::pdfout::default_ps_map(r)?;
    let _ = relaxed::ximage(&p, key, spec.clone(), &fonts);
    for n in std::mem::take(&mut *fonts.missed.lock().unwrap_or_else(PoisonError::into_inner)) {
        if let Some(b) = crate::pdfout::file_bytes(r, &String::from_utf8_lossy(&n))? {
            fonts.files.insert(n, b);
        }
    }
    match relaxed::ximage(&p, key, spec, &fonts) {
        Ok(piece) => {
            let got = piece.handle().zip(piece.ximage_info());
            keep(r, false, piece);
            Ok(got)
        }
        Err(e) => {
            r.log("error", &format!("pdfTeX error: {e}"))?;
            Ok(None)
        }
    }
}

/// `\pdffontobjnum f` in relaxed mode: the handle of the font's PDF
/// font.
pub(crate) fn font_objnum(r: &mut Run<'_, '_>, f: u32) -> R<i64> {
    let p = params(r)?;
    let p = fix(r, &p)?;
    let maps = maps(r)?;
    let fonts = crate::pdfout::fonts_for(r, &[f], false)?;
    match relaxed::font_objnum(&p, &maps, f, &fonts) {
        Ok(h) => Ok(i64::try_from(h).unwrap_or(0)),
        Err(e) => {
            r.log("error", &format!("pdfTeX error: {e}"))?;
            Ok(0)
        }
    }
}



