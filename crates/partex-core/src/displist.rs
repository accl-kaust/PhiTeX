//! Display lists (DESIGN 4.6): what each PDF page draws, as items in its
//! content stream's order, for a renderer that draws pages itself (the
//! `PhiTeX` Overleaf extension's preview), made without the PDF linked.
//!
//! Use: [`Tex::set_display_lists`]`(true)` before the cold build (off by
//! default; then nothing is kept, and on or off the output is the same
//! bytes); after each build or rebuild, with no link:
//! - [`Tex::display_pages`]: how many pages were shipped;
//!   [`Tex::display_hashes`]: one hash per page of everything its drawing
//!   depends on (its stream and box, its fonts, the forms and images it
//!   draws): after a rebuild, redraw the pages whose hash changed;
//! - [`Tex::display_list`]`(page)`: the page's [`PageList`] (`page`
//!   0-based, in shipping order, as for [`Tex::origins`]);
//! - [`Tex::display_glyphs`]`(page)`: the codes the page shows, forms
//!   walked where they are drawn: the `i`-th is `origins(page)[i]`'s;
//! - [`Tex::display_form`]`(id)`, [`Tex::display_font`]`(id)` and
//!   [`Tex::display_image`]`(id)`: what the lists name.
//!
//! - **Coordinates.** Points (bp, 1/72 in) in the page's default user
//!   space: `x` rightwards, `y` upwards, from the origin of the media box,
//!   which pdfTeX writes as `[0 0 w h]`, its lower left corner the page's
//!   (`\pdfpageattr` can give a `/MediaBox` of its own: then
//!   [`PageList::media`] is that one, and its corner need not be the
//!   origin). A form's list is in the form's own space (`[0 0 w h]`, `h`
//!   its height plus depth); an [`Item::XObject`]'s matrix maps it to the
//!   page.
//! - **Order.** Items come in the stream's order. A page's glyphs in
//!   glyph origins' order (DESIGN 4.4) are its list's [`Item::Glyph`]s,
//!   with a form's put in at each [`Item::XObject`] that draws it, and a
//!   literal's [`Item::Literal`]`::codes` and an included PDF page's
//!   ([`ImageKind::Pdf`]) codes as that many places with no glyph of
//!   TeX's: what [`Tex::display_glyphs`] gives.
//! - **Exact.** A list is the walk of the stream's bytes, the bytes the
//!   PDF holds, by `partex_engine::pdftext` (glyph origins' and `partex
//!   outline`'s walk), its fonts' widths what the PDF's `/Widths` give: a
//!   glyph is where a reader of the PDF puts it, to the bit.
//! - **Incremental.** With a recorder (SSA) the stream is the ship step's
//!   effect (`Effect::Display`): a step reused keeps its stream, one run
//!   again makes it again, and a list is made when it is asked for, from
//!   what the steps hold then (each stream's list made once). A font's id
//!   ([`FontId`]) is kept for the engine's life: the same font at the
//!   same size has the same id in every rebuild. A form's or image's id
//!   is its object number, which an edit making objects before it can
//!   change: recognize an image by its contents ([`ImageInfo::data`]).
//! - **Fonts.** partex embeds Type 1 fonts only (TrueType, OpenType, PK
//!   and unembedded fonts stop the build: "not implemented in partex
//!   yet"), so every font a list names has a Type 1 program: the whole
//!   file the map entry names (not pdfTeX's subset), its encoding and the
//!   TFM's widths ([`FontInfo`]).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::pdftext::{self, Codes, Font, FormStream, GlyphRes, ListRes, Widths};
pub use partex_engine::pdftext::{FontId, Item, LiteralMode, LiteralSpan, Matrix, Placed, XKind};
use partex_engine::scaled::{divide_scaled, round_xn_over_d};
use partex_engine::stablehash::StableHasher;

use crate::arith::Scaled;
use crate::fontmap::MapEntry;
use crate::host::{FileKind, Host};
use crate::pdf::image::Image;
use crate::tex::Tex;
use crate::track::Tracker;
use crate::web::{INT_BASE, MAG_CODE};

/// A content stream's bytes, and where its literals' texts are in it
/// ([`Tex::display_stream`]).
pub type StreamBytes = (Arc<[u8]>, Arc<[LiteralSpan]>);

/// A page's or a form's display list.
#[derive(Clone, Debug, PartialEq)]
pub struct PageList {
    /// The page's media box (a form's bounding box), `[x0 y0 x1 y1]`, as
    /// the PDF has it: pdfTeX's `[0 0 w h]`, or `\pdfpageattr`'s.
    pub media: [f64; 4],
    /// Its width and height.
    pub w: f64,
    pub h: f64,
    /// `/Rotate` from `\pdfpageattr` (0 to 270), which a viewer turns the
    /// page by, clockwise; the coordinates are not turned.
    pub rotate: i32,
    /// What it draws, in its content stream's order.
    pub items: Arc<[Item]>,
}

/// A font a list names ([`Tex::display_font`]): a PDF font at a size.
#[derive(Clone, Debug, PartialEq)]
pub struct FontInfo {
    /// The PostScript name: the map entry's (the PDF's `/BaseFont`
    /// without pdfTeX's subset tag), else the TFM's name.
    pub name: String,
    /// The TFM file's name.
    pub tfm: String,
    /// The size, points (`Tf`'s).
    pub size: f64,
    /// The font program: the file the map entry names, and its bytes as
    /// the host reads them (the whole file, `.pfb` or `.pfa`, not
    /// pdfTeX's subset); `None` if the host cannot read it now.
    pub file: String,
    pub program: Option<Arc<[u8]>>,
    /// Each code's glyph name (256, `.notdef` where none): the map
    /// entry's encoding file's (`reencoded`: the PDF's `/Differences`),
    /// else the program's own `/Encoding` (`StandardEncoding` spelled
    /// out); empty if neither can be read.
    pub encoding: Arc<[String]>,
    pub reencoded: bool,
    /// Each code's advance, in thousandths of the size: the TFM's width
    /// as the PDF's `/Widths` gives it (to a tenth), 0 for a code the TFM
    /// lacks. What placed the glyphs.
    pub widths: Arc<[f64]>,
    /// The map entry's `SlantFont` and `ExtendFont` (0 and 1 if none),
    /// which pdfTeX puts in the embedded program's `/FontMatrix`: a
    /// renderer of `program` applies them itself.
    pub slant: f64,
    pub extend: f64,
}

/// What an image is.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageKind {
    /// A PNG file's: drawn as the unit square.
    Png,
    /// A JPEG file's: drawn as the unit square.
    Jpeg,
    /// Page `page` (from 1) of a PDF file, a form in the PDF: `bbox` (its
    /// `/BBox`, the page's box in the page's own space) and `matrix` (its
    /// `/Matrix`: a turned page's turn, else the identity) as the PDF has
    /// them; its content is drawn by `matrix` then by the item's matrix,
    /// clipped to `bbox`. `codes`: the character codes its text shows,
    /// each a place in glyph origins' order with no source.
    Pdf {
        page: i32,
        bbox: [f64; 4],
        matrix: Matrix,
        codes: u32,
    },
}

/// An image a list names ([`Tex::display_image`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ImageInfo {
    pub kind: ImageKind,
    /// The file as found, and its bytes as they were read (whole).
    pub name: String,
    pub data: Arc<[u8]>,
    /// Its size: in pixels (a PDF page's: its box's, in scaled points).
    pub width: i32,
    pub height: i32,
}

/// A PDF font as a stream names it (`/F<n>`): the TeX font that made it
/// (its TFM's name, its map entry, its size) and the TFM's widths as
/// pdfTeX writes them in `/Widths`: each code's, in ten-thousandths of
/// the size (`divide_scaled` to 4 places).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontRec {
    pub(crate) tfm: Vec<u8>,
    pub(crate) map: Option<Arc<MapEntry>>,
    pub(crate) size: Scaled,
    pub(crate) widths: Arc<[i32]>,
}

partex_engine::persist_struct!(FontRec {
    tfm,
    map,
    size,
    widths
});

/// A page's or form's content stream as shipped: what its display list
/// is made from (the effect `Effect::Display`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shipped {
    /// 0 for a page, else the form's object number.
    pub(crate) form: i32,
    /// The stream's bytes, uncompressed: the PDF's.
    pub(crate) bytes: Arc<[u8]>,
    /// Where its literals' texts are, in order.
    pub(crate) literals: Arc<[LiteralSpan]>,
    /// Its resources: each `/F<n>`'s `n` and font; each `/Fm<n>`'s `n`
    /// and form; each `/Im<n>`'s `n`, its object and its image.
    pub(crate) fonts: Arc<[(i32, Arc<FontRec>)]>,
    pub(crate) forms: Arc<[(i32, i32)]>,
    pub(crate) images: Arc<[(i32, i32, Arc<Image>)]>,
    /// The box as written, its four numbers: a page's `/MediaBox` (unless
    /// `attr` gives one), a form's `/BBox`.
    pub(crate) bbox: Vec<u8>,
    /// A page's `\pdfpageattr`.
    pub(crate) attr: Vec<u8>,
}

partex_engine::persist_struct!(Shipped {
    form,
    bytes,
    literals,
    fonts,
    forms,
    images,
    bbox,
    attr
});

impl Shipped {
    /// One of each part (the effects' round trip test).
    #[cfg(test)]
    pub(crate) fn example() -> Self {
        let rec = FontRec {
            tfm: b"cmr10".to_vec(),
            map: None,
            size: 655_360,
            widths: alloc::vec![0, 5000].into(),
        };
        Shipped {
            form: 0,
            bytes: Arc::from(&b"BT /F1 10 Tf (a) Tj ET"[..]),
            literals: alloc::vec![(0, 2, 1)].into(),
            fonts: alloc::vec![(1, Arc::new(rec))].into(),
            forms: alloc::vec![(1, 12)].into(),
            images: alloc::vec![].into(),
            bbox: b"0 0 612 792".to_vec(),
            attr: Vec::new(),
        }
    }

    /// This stream with its form's and its resources' objects named by
    /// pdfTeX's numbers `n` (virtual object numbers), shared if none moves.
    fn numbered(self: &Arc<Self>, n: &crate::pdf::vnum::Numbering) -> Arc<Self> {
        let moved = n.of(self.form) != self.form
            || self.forms.iter().any(|&(_, k)| n.of(k) != k)
            || self.images.iter().any(|(_, k, _)| n.of(*k) != *k);
        if !moved {
            return self.clone();
        }
        let mut s = (**self).clone();
        s.form = n.of(s.form);
        s.forms = s.forms.iter().map(|&(m, k)| (m, n.of(k))).collect();
        s.images = s
            .images
            .iter()
            .map(|(m, k, i)| (*m, n.of(*k), i.clone()))
            .collect();
        Arc::new(s)
    }

    /// The box and the turn the PDF gives this stream's page (or form).
    pub(crate) fn media(&self) -> ([f64; 4], i32) {
        let given = (!self.attr.is_empty())
            .then(|| after(&self.attr, b"/MediaBox"))
            .flatten();
        let nums = numbers(given.unwrap_or(&self.bbox));
        let mut b = [0.0; 4];
        for (x, v) in b.iter_mut().zip(&nums) {
            *x = *v;
        }
        // (as a reader takes a box: its corners in order)
        if b[0] > b[2] {
            b.swap(0, 2);
        }
        if b[1] > b[3] {
            b.swap(1, 3);
        }
        let rotate = after(&self.attr, b"/Rotate")
            .and_then(|t| numbers(t).first().copied())
            .map_or(0, |r| {
                #[expect(clippy::cast_possible_truncation, reason = "an integer read")]
                let r = r as i32;
                r.rem_euclid(360)
            });
        (b, rotate)
    }
}

/// Whether a stream's images are all drawn from it alone (a page's own
/// PDF holds JPEG images only, `pagepdf::page_pdf`).
fn images_whole(s: &Shipped) -> bool {
    s.images
        .iter()
        .all(|(_, _, im)| im.pdf.is_none() && !im.png)
}

/// The text after `key` in `s`, if `key` is there.
fn after<'a>(s: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let at = s.windows(key.len()).position(|w| w == key)?;
    Some(&s[at + key.len()..])
}

/// The numbers at the start of `s` (a box's, past its `[`), as a reader
/// of the PDF reads them.
fn numbers(s: &[u8]) -> Vec<f64> {
    let mut out = Vec::new();
    for t in s
        .split(|c| c.is_ascii_whitespace() || *c == b'[')
        .filter(|t| !t.is_empty())
    {
        let t = t.split(|&c| c == b']').next().unwrap_or_default();
        match partex_engine::pdfread::number_value(t) {
            Some(v) if !t.is_empty() => out.push(v),
            _ => break,
        }
        if out.len() == 4 {
            break;
        }
    }
    out
}

/// The value a reader of the PDF gives a `/Widths` entry pdfTeX prints
/// from `w` (ten-thousandths: writefont.c's `%i` and `.%i`).
fn width_value(w: i32) -> f64 {
    use core::fmt::Write;
    let mut t = alloc::format!("{}", w / 10);
    if w % 10 != 0 {
        let _ = write!(t, ".{}", w % 10);
    }
    partex_engine::pdfread::number_value(t.as_bytes()).unwrap_or(0.0)
}

/// pdfTeX's printed number of points for `s` (`pdf_print_bp` at `dd`
/// decimal digits), as text.
fn bp_text(s: Scaled, dd: i32) -> Vec<u8> {
    let q = divide_scaled(s, crate::pdf::ONE_HUNDRED_BP, dd + 2).map_or(0, |r| r.0);
    let mut o = crate::pdf::out::PdfOut::default();
    o.print_real(q, dd);
    o.take_buf()
}

/// The pages' streams in shipping order, and the forms' by object
/// number.
type Streams = (Vec<Arc<Shipped>>, BTreeMap<i32, Arc<Shipped>>);

/// A font of the lists: its PDF font, its size, and what was found.
#[derive(Clone)]
struct FontEntry {
    rec: Arc<FontRec>,
    size: f64,
    info: Option<FontInfo>,
}

/// What display lists keep (`Tex::dl`).
#[derive(Clone)]
pub(crate) struct DlState {
    /// The open stream's literals.
    spans: Vec<LiteralSpan>,
    /// A page's record, until its page object is written.
    page: Option<Shipped>,
    /// The PDF fonts' records, by the font (its slot, identity, size and
    /// map entry): made once.
    font_recs: BTreeMap<u128, Arc<FontRec>>,
    /// Without a recorder: the pages shipped, and the forms.
    pages: Vec<Arc<Shipped>>,
    forms: BTreeMap<i32, Arc<Shipped>>,
    /// The streams as last collected, with what they were collected from
    /// (an SSA build's: the steps' chunks' versions).
    cached: Option<(u128, Arc<Streams>)>,
    /// The fonts by id, and each id by its font's key.
    fonts: Vec<FontEntry>,
    font_ids: BTreeMap<u128, FontId>,
    /// Each PDF font's widths for the walk, by its record.
    widths: BTreeMap<usize, (Arc<FontRec>, Arc<Widths>)>,
    /// Each stream's list, and its hash, by its record (kept with them).
    lists: BTreeMap<usize, (Arc<Shipped>, Arc<[Item]>)>,
    hashes: BTreeMap<usize, (Arc<Shipped>, u128)>,
    /// The codes each included PDF page shows: its file's bytes, the
    /// page.
    image_codes: Vec<(Arc<[u8]>, i32, usize)>,
}

impl DlState {
    fn new() -> Self {
        DlState {
            spans: Vec::new(),
            page: None,
            font_recs: BTreeMap::new(),
            pages: Vec::new(),
            forms: BTreeMap::new(),
            cached: None,
            fonts: Vec::new(),
            font_ids: BTreeMap::new(),
            widths: BTreeMap::new(),
            lists: BTreeMap::new(),
            hashes: BTreeMap::new(),
            image_codes: Vec::new(),
        }
    }

    /// The id of PDF font `rec` at size `size`, made if new.
    fn font_id(&mut self, rec: &Arc<FontRec>, size: f64) -> FontId {
        let key = StableHasher::of(&(&**rec, size.to_bits()));
        if let Some(&id) = self.font_ids.get(&key) {
            return id;
        }
        let id = FontId(u32::try_from(self.fonts.len()).unwrap_or(u32::MAX - 1));
        self.fonts.push(FontEntry {
            rec: rec.clone(),
            size,
            info: None,
        });
        self.font_ids.insert(key, id);
        id
    }

    /// The walk's widths of PDF font `rec`: what the PDF's `/Widths`
    /// give, in text space.
    fn widths(&mut self, rec: &Arc<FontRec>) -> Arc<Widths> {
        let at = Arc::as_ptr(rec) as usize;
        if let Some((_, w)) = self.widths.get(&at) {
            return w.clone();
        }
        // (`DocText`'s: each `/Widths` number read, times 1/1000)
        let w = Arc::new(Widths::Simple {
            first: 0,
            widths: rec.widths.iter().map(|&w| width_value(w) * 0.001).collect(),
            missing: 0.0,
        });
        self.widths.insert(at, (rec.clone(), w.clone()));
        w
    }

    /// What the queries keep of streams no longer shipped, let go.
    fn prune(&mut self, s: &Streams) {
        let live: alloc::collections::BTreeSet<usize> =
            s.0.iter()
                .chain(s.1.values())
                .map(|r| Arc::as_ptr(r) as usize)
                .collect();
        self.lists.retain(|k, _| live.contains(k));
        self.hashes.retain(|k, _| live.contains(k));
    }
}

/// A stream's resources for the walks: the streams whose resources are in
/// force (forms entered, innermost last), and every stream.
struct Res<'a> {
    dl: &'a mut DlState,
    stack: Vec<Arc<Shipped>>,
    streams: &'a Streams,
}

impl Res<'_> {
    /// The PDF font that resource `name` (`F<n>`) names in the stream.
    fn font_rec(&self, name: &[u8]) -> Option<Arc<FontRec>> {
        let n = num(name.strip_prefix(b"F")?)?;
        let top = self.stack.last()?;
        top.fonts
            .iter()
            .find(|(m, _)| *m == n)
            .map(|(_, r)| r.clone())
    }

    /// The image `id`, among those the stream draws.
    fn image(&self, id: u32) -> Option<Arc<Image>> {
        let id = i32::try_from(id).ok()?;
        self.stack
            .last()?
            .images
            .iter()
            .find(|(_, k, _)| *k == id)
            .map(|(_, _, i)| i.clone())
    }
}

/// The number a resource's name ends with (`F12`'s `12`).
fn num(digits: &[u8]) -> Option<i32> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    core::str::from_utf8(digits).ok()?.parse().ok()
}

impl ListRes for Res<'_> {
    fn font(&mut self, name: &[u8]) -> Font {
        let mut f = Font {
            name: name.to_vec(),
            ..Font::default()
        };
        // (as `DocText::font` finds a Type 1 font: one byte a code)
        if let Some(rec) = self.font_rec(name) {
            f.codes = Codes::Fixed(1);
            f.widths = self.dl.widths(&rec);
        }
        f
    }

    fn font_id(&mut self, name: &[u8], size: f64) -> FontId {
        match self.font_rec(name) {
            Some(rec) => self.dl.font_id(&rec, size),
            None => FontId::UNKNOWN,
        }
    }

    fn xobject(&mut self, name: &[u8]) -> Option<(XKind, u32)> {
        let top = self.stack.last()?;
        if let Some(n) = name.strip_prefix(b"Fm").and_then(num) {
            let k = top.forms.iter().find(|(m, _)| *m == n)?.1;
            return Some((XKind::Form, u32::try_from(k).ok()?));
        }
        let n = name.strip_prefix(b"Im").and_then(num)?;
        let k = top.images.iter().find(|(m, _, _)| *m == n)?.1;
        Some((XKind::Image, u32::try_from(k).ok()?))
    }
}

impl GlyphRes for Res<'_> {
    fn enter(&mut self, id: u32) -> Option<FormStream> {
        let rec = self.streams.1.get(&i32::try_from(id).ok()?)?.clone();
        let out = (rec.bytes.clone(), rec.literals.clone(), pdftext::IDENTITY);
        self.stack.push(rec);
        Some(out)
    }

    fn leave(&mut self) {
        self.stack.pop();
    }

    fn image_codes(&mut self, id: u32) -> usize {
        let Some(image) = self.image(id) else {
            return 0;
        };
        image.pdf.as_ref().map_or(0, |p| {
            codes_of(&mut self.dl.image_codes, &image.data, p.page)
        })
    }
}

/// How many codes page `page` of PDF file `data` shows (counted once).
fn codes_of(memo: &mut Vec<(Arc<[u8]>, i32, usize)>, data: &Arc<[u8]>, page: i32) -> usize {
    if let Some(&(_, _, n)) = memo
        .iter()
        .find(|(d, p, _)| Arc::ptr_eq(d, data) && *p == page)
    {
        return n;
    }
    let n = pdftext::page_code_count(data, usize::try_from(page).unwrap_or(0));
    memo.push((data.clone(), page, n));
    n
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Keep display lists (DESIGN 4.6) from now on, or stop: before a
    /// build begins. Off by default, and then free.
    pub fn set_display_lists(&mut self, on: bool) {
        self.dl = on.then(|| alloc::boxed::Box::new(DlState::new()));
    }

    /// Whether display lists are kept.
    #[must_use]
    #[inline]
    pub fn display_lists_on(&self) -> bool {
        self.dl.is_some()
    }

    /// How many pages the last build or rebuild shipped (0 with display
    /// lists off).
    pub fn display_pages(&mut self) -> usize {
        self.display_streams().map_or(0, |s| s.0.len())
    }

    /// Page `page`'s display list (0-based, in shipping order), after the
    /// last build or rebuild; `None` if there is no such page or display
    /// lists are off.
    pub fn display_list(&mut self, page: usize) -> Option<PageList> {
        let s = self.display_streams()?;
        let rec = s.0.get(page)?.clone();
        Some(self.display_page_list(&rec, &s))
    }

    /// Form `id`'s (its object number's) own display list, in its own
    /// space; `None` if no list has it.
    pub fn display_form(&mut self, id: u32) -> Option<PageList> {
        let s = self.display_streams()?;
        let rec = s.1.get(&i32::try_from(id).ok()?)?.clone();
        Some(self.display_page_list(&rec, &s))
    }

    /// Each code page `page` shows, in glyph origins' order (forms walked
    /// where they are drawn, see the module's documentation): a glyph of
    /// TeX's placed in the page's space, a literal's or an included PDF
    /// page's `None`. Empty if there is no such page.
    pub fn display_glyphs(&mut self, page: usize) -> Vec<Option<Placed>> {
        let Some(s) = self.display_streams() else {
            return Vec::new();
        };
        let Some(rec) = s.0.get(page).cloned() else {
            return Vec::new();
        };
        let Some(dl) = self.dl.as_deref_mut() else {
            return Vec::new();
        };
        let mut res = Res {
            dl,
            stack: alloc::vec![rec.clone()],
            streams: &s,
        };
        pdftext::glyphs(&rec.bytes, &rec.literals, &mut res)
    }

    /// A hash of each page's drawing (its stream and box, its fonts, the
    /// forms and images it draws), in shipping order: a page whose hash
    /// a rebuild left alone draws the same.
    pub fn display_hashes(&mut self) -> Vec<u128> {
        let Some(s) = self.display_streams() else {
            return Vec::new();
        };
        let Some(dl) = self.dl.as_deref_mut() else {
            return Vec::new();
        };
        s.0.iter().map(|r| stream_hash(dl, r, &s, 0)).collect()
    }

    /// What page `page`'s list is made from: its content stream as the
    /// PDF has it (uncompressed) and where its literals' texts are in it
    /// (with their modes), for checking a list against a PDF; form `id`'s
    /// with `form`.
    pub fn display_stream(&mut self, page: usize, form: Option<u32>) -> Option<StreamBytes> {
        let s = self.display_streams()?;
        let rec = match form {
            Some(id) => s.1.get(&i32::try_from(id).ok()?)?,
            None => s.0.get(page)?,
        };
        Some((rec.bytes.clone(), rec.literals.clone()))
    }

    /// The forms the lists draw, by object number.
    pub fn display_forms(&mut self) -> Vec<u32> {
        self.display_streams().map_or_else(Vec::new, |s| {
            s.1.keys().filter_map(|&k| u32::try_from(k).ok()).collect()
        })
    }

    /// Font `id` of the lists; `None` if no list named it.
    pub fn display_font(&mut self, id: FontId) -> Option<FontInfo> {
        let i = usize::try_from(id.0).ok()?;
        let (rec, size) = {
            let e = self.dl.as_deref()?.fonts.get(i)?;
            if let Some(info) = &e.info {
                return Some(info.clone());
            }
            (e.rec.clone(), e.size)
        };
        let info = self.font_info(&rec, size);
        if let Some(e) = self.dl.as_deref_mut().and_then(|d| d.fonts.get_mut(i)) {
            e.info = Some(info.clone());
        }
        Some(info)
    }

    /// Image `id` (its object number) of the lists; `None` if no list
    /// draws it.
    pub fn display_image(&mut self, id: u32) -> Option<ImageInfo> {
        let s = self.display_streams()?;
        let k = i32::try_from(id).ok()?;
        let image =
            s.0.iter()
                .chain(s.1.values())
                .find_map(|r| r.images.iter().find(|(_, i, _)| *i == k))
                .map(|(_, _, im)| im.clone())?;
        let kind = match &image.pdf {
            Some(p) => {
                let (bbox, matrix) =
                    crate::pdf::epdf::included_form_box(&image.data, p.page, p.page_box)
                        .unwrap_or(([0.0; 4], pdftext::IDENTITY));
                let dl = self.dl.as_deref_mut()?;
                let codes = codes_of(&mut dl.image_codes, &image.data, p.page);
                ImageKind::Pdf {
                    page: p.page,
                    bbox,
                    matrix,
                    codes: u32::try_from(codes).unwrap_or(u32::MAX),
                }
            }
            None if image.png => ImageKind::Png,
            None => ImageKind::Jpeg,
        };
        Some(ImageInfo {
            kind,
            name: String::from_utf8_lossy(&image.name).into_owned(),
            data: image.data.clone(),
            width: image.width,
            height: image.height,
        })
    }

    /// The streams of the last build or rebuild: an SSA build's collected
    /// from its steps' effects (again only when they changed).
    fn display_streams(&mut self) -> Option<Arc<Streams>> {
        self.dl.as_ref()?;
        let Some(ssa) = self.tracker.ssa() else {
            // (without a recorder: as kept, collected again after a ship)
            let d = self.dl.as_deref_mut()?;
            if let Some((_, s)) = &d.cached {
                return Some(s.clone());
            }
            let s = Arc::new((d.pages.clone(), d.forms.clone()));
            d.cached = Some((0, s.clone()));
            return Some(s);
        };
        let rec = ssa.rec.borrow();
        let chunks = crate::ssa::step_effects(&rec);
        let key =
            partex_ssa::Version::of(&chunks.iter().map(|(k, e)| (*k, e.0.0)).collect::<Vec<_>>()).0;
        let d = self.dl.as_deref_mut()?;
        if let Some((k, s)) = &d.cached
            && *k == key
        {
            return Some(s.clone());
        }
        // (virtual object numbers: forms and images named by pdfTeX's
        // numbers, which the steps' numbering events give, as the link's)
        let slices: Vec<&[crate::effects::Effect]> = chunks.iter().map(|(_, e)| &e.1[..]).collect();
        let numbering = crate::effects::numbering_of(&slices).map(|(n, _)| n);
        let (mut pages, mut forms) = (Vec::new(), BTreeMap::new());
        for (_, c) in &chunks {
            for e in c.1.iter() {
                if let crate::effects::Effect::Display(s) = e {
                    let s = match &numbering {
                        Some(n) => s.numbered(n),
                        None => s.clone(),
                    };
                    if s.form == 0 {
                        pages.push(s);
                    } else {
                        forms.insert(s.form, s);
                    }
                }
            }
        }
        let s = Arc::new((pages, forms));
        d.prune(&s);
        d.cached = Some((key, s.clone()));
        Some(s)
    }

    /// The list of stream `rec` (made once).
    fn display_page_list(&mut self, rec: &Arc<Shipped>, s: &Streams) -> PageList {
        let (media, rotate) = rec.media();
        let items = self.dl.as_deref_mut().map_or_else(
            || Arc::from(&[][..]),
            |dl| {
                let at = Arc::as_ptr(rec) as usize;
                if let Some((_, items)) = dl.lists.get(&at) {
                    return items.clone();
                }
                let mut res = Res {
                    dl: &mut *dl,
                    stack: alloc::vec![rec.clone()],
                    streams: s,
                };
                let items: Arc<[Item]> = pdftext::list(&rec.bytes, &rec.literals, &mut res).into();
                dl.lists.insert(at, (rec.clone(), items.clone()));
                items
            },
        );
        PageList {
            media,
            w: media[2] - media[0],
            h: media[3] - media[1],
            rotate,
            items,
        }
    }

    /// What font `rec` at `size` is: its program and encoding found
    /// through the host (reads only: nothing printed, nothing tracked).
    fn font_info(&mut self, rec: &FontRec, size: f64) -> FontInfo {
        let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let map = rec.map.as_deref();
        let file = map.and_then(|m| m.ff_name.clone()).unwrap_or_default();
        let program = (!file.is_empty())
            .then(|| self.host.read_file(&file, FileKind::Type1))
            .flatten()
            .map(|f| f.contents);
        let enc = map.and_then(|m| m.encname.clone());
        let names = match &enc {
            Some(e) => self
                .host
                .read_file(e, FileKind::Enc)
                .and_then(|f| crate::pdf::enc::parse_enc(&f.contents).ok()),
            None => program
                .as_deref()
                .and_then(crate::pdf::writet1::builtin_encoding),
        };
        let thousandths = |m: Option<i32>, none: f64| {
            m.filter(|&v| v != 0)
                .map_or(none, |v| f64::from(v) / 1000.0)
        };
        FontInfo {
            name: s(map.and_then(|m| m.ps_name.as_deref()).unwrap_or(&rec.tfm)),
            tfm: s(&rec.tfm),
            size,
            file: s(&file),
            program,
            encoding: names.unwrap_or_default().iter().map(|n| s(n)).collect(),
            reencoded: enc.is_some(),
            widths: rec.widths.iter().map(|&w| width_value(w)).collect(),
            slant: thousandths(map.map(|m| m.slant), 0.0),
            extend: thousandths(map.map(|m| m.extend), 1.0),
        }
    }

    // ---- recording ----

    /// A content stream begins (a page's or a form's).
    pub(crate) fn display_stream_begin(&mut self) {
        self.tap = None;
        if let Some(d) = self.dl.as_deref_mut() {
            d.spans.clear();
            d.page = None;
        }
    }

    /// The encoder writes a literal's text, `len` bytes from where the
    /// stream is now, in mode `mode`: before it writes them.
    pub(crate) fn display_literal(&mut self, len: usize, mode: i32) {
        let at = self.pdf.out.stream_pos();
        if let Some(d) = self.dl.as_deref_mut() {
            let (a, e) = (
                u32::try_from(at).unwrap_or(u32::MAX),
                u32::try_from(at + len).unwrap_or(u32::MAX),
            );
            d.spans.push((a, e, u8::try_from(mode).unwrap_or(0)));
        }
    }

    /// The content stream ends (before `pdf_end_stream`): a page's
    /// (`form` 0, of size `w` by `h`), or form `form`'s (its width `w`
    /// and its height plus depth `h`). Its record is made: a form's kept
    /// now, a page's once its page object is written.
    pub(crate) fn display_stream_end(&mut self, form: i32, w: Scaled, h: Scaled) {
        let tap = self.host.wants_streams();
        if self.dl.is_none() && !tap {
            return;
        }
        let Some(bytes) = self.pdf.out.stream_bytes() else {
            return;
        };
        // (what the resources dictionary names, as `pdf_ship_resources`
        // writes it; read quietly: what the ship read already)
        let s = &self.pdf.ship;
        let (font_list, xforms, ximages) = (
            s.font_list.clone(),
            s.xform_list.clone(),
            s.ximage_list.clone(),
        );
        let mut fonts = Vec::new();
        for f in font_list {
            let n = s_ff(self, f);
            let rank = self
                .fonts
                .rank
                .get(crate::fonts::fx(n))
                .copied()
                .unwrap_or(n);
            fonts.push((rank, self.display_font_rec(n)));
        }
        let forms: Vec<(i32, i32)> = xforms
            .iter()
            .map(|&k| (self.pdf.objs.get(k).info.num(), k))
            .collect();
        let mut images = Vec::new();
        for k in ximages {
            let n = self.pdf.objs.get(k).info.num();
            if let crate::pdf::objtab::Aux::XImage(x) = &self.pdf.objs.get(k).aux
                && let Some(im) = &x.image
            {
                images.push((n, k, im.clone()));
            }
        }
        // (the box as `write_page_object` or the form's dictionary prints
        // it: a page's scaled by `\mag`)
        let dd = self.pdf.out.fixed_decimal_digits;
        let (w, h) = if form == 0 {
            let mag = self.peek_eqtb(INT_BASE + MAG_CODE).int();
            let m = |x: Scaled| {
                if mag == 1000 {
                    x
                } else {
                    round_xn_over_d(x, mag, 1000)
                }
            };
            (m(w), m(h))
        } else {
            (w, h)
        };
        let mut bbox = b"0 0 ".to_vec();
        bbox.extend_from_slice(&bp_text(w, dd));
        bbox.push(b' ');
        bbox.extend_from_slice(&bp_text(h, dd));
        let rec = Shipped {
            form,
            bytes: bytes.into(),
            literals: self
                .dl
                .as_deref_mut()
                .map(|d| core::mem::take(&mut d.spans))
                .unwrap_or_default()
                .into(),
            fonts: fonts.into(),
            forms: forms.into(),
            images: images.into(),
            bbox,
            attr: Vec::new(),
        };
        // (a viewer's: a form at once, a page with its attributes)
        if tap {
            if form == 0 {
                self.tap = Some(alloc::boxed::Box::new(rec.clone()));
            } else {
                // (whole: its images drawn from it, no `/Resources` of
                // its own that its stream alone does not give)
                let own = match &self.pdf.objs.get(self.pdf.ship.cur_form).aux {
                    crate::pdf::objtab::Aux::XForm(x) => {
                        x.resources.as_ref().is_some_and(|t| !t.is_empty())
                    }
                    _ => false,
                };
                let whole = !own && images_whole(&rec);
                let uni = self.shipped_tounicode();
                let whole = whole && uni.1;
                let s = crate::pagepdf::ShippedStream(Arc::new(rec.clone()), whole, uni.0);
                self.host.stream_shipped(None, s);
            }
        }
        let Some(d) = self.dl.as_deref_mut() else {
            return;
        };
        if form == 0 {
            d.page = Some(rec);
        } else {
            self.display_keep(rec);
        }
    }

    /// The page object is written, with `\pdfpageattr`'s text `attr`: the
    /// page's record is kept.
    pub(crate) fn display_page_object(&mut self, attr: Option<&[u8]>) {
        if let Some(mut rec) = self.tap.take() {
            rec.attr = attr.unwrap_or_default().to_vec();
            let page = usize::try_from(self.pdf.ship.total_pages - 1).unwrap_or(0);
            // (whole: drawn from its stream alone as from the PDF, no
            // `\pdfpageresources` (graphics states), no annotations or
            // links, its images drawn from it)
            let res = self
                .equiv_toks(crate::web::PDF_PAGE_RESOURCES_LOC)
                .is_some_and(|t| !t.is_empty());
            let marks = self.pdf.ship.has_marks();
            let whole = !res && !marks && images_whole(&rec);
            let uni = self.shipped_tounicode();
            let whole = whole && uni.1;
            let s = crate::pagepdf::ShippedStream(Arc::new(*rec), whole, uni.0);
            self.host.stream_shipped(Some(page), s);
        }
        let Some(mut rec) = self.dl.as_deref_mut().and_then(|d| d.page.take()) else {
            return;
        };
        rec.attr = attr.unwrap_or_default().to_vec();
        self.display_keep(rec);
    }

    /// The `\pdfglyphtounicode` entries the job's fonts will have their
    /// `/ToUnicode` maps made from, if `\pdfgentounicode` is on (read as
    /// it is now: the job's end reads it again), and whether a page's own
    /// PDF makes them as the job will (no font is one of
    /// `\pdfnobuiltintounicode`'s). Read quietly: a viewer's.
    fn shipped_tounicode(&self) -> (Option<crate::pagepdf::Uni>, bool) {
        let on = self
            .peek_eqtb(INT_BASE + crate::web::PDF_GEN_TOUNICODE_CODE)
            .int();
        if on <= 0 || self.tounicode.is_empty() {
            return (None, true);
        }
        (
            Some(crate::pagepdf::Uni(self.tounicode.clone())),
            self.pdf.nobuiltin_tounicode.len() == 0,
        )
    }

    /// Stream record `rec` kept: the step's effect with a recorder, else
    /// the engine's.
    fn display_keep(&mut self, rec: Shipped) {
        let rec = Arc::new(rec);
        if T::VALUES {
            if let Some(e) = &mut self.effects {
                e.push(crate::effects::Effect::Display(rec));
            }
        } else if let Some(d) = self.dl.as_deref_mut() {
            if rec.form == 0 {
                d.pages.push(rec);
            } else {
                d.forms.insert(rec.form, rec);
            }
            d.cached = None;
        }
    }

    /// The record of the PDF font TeX font `k` made (shared: made once
    /// per font).
    fn display_font_rec(&mut self, k: i32) -> Arc<FontRec> {
        let i = crate::fonts::fx(k);
        let pf = self.pdf.ship.fonts.get(i).cloned().unwrap_or_default();
        let map = pf.map.clone().flatten();
        let idv = self.fonts.idv.get(i).copied().unwrap_or(0);
        let key = StableHasher::of(&(k, idv, pf.size, map.as_deref()));
        if let Some(r) = self.dl.as_deref().and_then(|d| d.font_recs.get(&key)) {
            return r.clone();
        }
        let font = self.fonts.get(k);
        let widths: Vec<i32> = (0..=255)
            .map(|c| {
                let w = font.glyph(c).map_or(0, |g| g.width);
                divide_scaled(w, pf.size, 4).map_or(0, |r| r.0)
            })
            .collect();
        let rec = Arc::new(FontRec {
            tfm: self.font_name_bytes(k),
            map,
            size: pf.size,
            widths: widths.into(),
        });
        if let Some(d) = self.dl.as_deref_mut() {
            d.font_recs.insert(key, rec.clone());
        }
        rec
    }
}

/// `set_ff` read quietly: the font whose PDF font font `f` uses.
fn s_ff<H: Host, T: Tracker>(t: &Tex<H, T>, f: i32) -> i32 {
    let n = t
        .pdf
        .ship
        .fonts
        .get(crate::fonts::fx(f))
        .map_or(0, |p| p.num);
    if n < 0 { -n } else { f }
}

/// The hash of stream `rec`'s drawing: its record, its fonts, and the
/// forms (theirs, `depth` deep) and images it draws (made once).
fn stream_hash(dl: &mut DlState, rec: &Arc<Shipped>, s: &Streams, depth: usize) -> u128 {
    use core::hash::Hash;
    let at = Arc::as_ptr(rec) as usize;
    if let Some((_, h)) = dl.hashes.get(&at) {
        return *h;
    }
    let mut h = StableHasher::new();
    // (its record holds its bytes, literals, fonts with their map entries
    // and widths, images with their files, box and page attributes)
    rec.hash(&mut h);
    if depth < 32 {
        for (_, k) in rec.forms.iter() {
            match s.1.get(k) {
                Some(f) => stream_hash(dl, f, s, depth + 1).hash(&mut h),
                None => 0u128.hash(&mut h),
            }
        }
    }
    let v = h.finish128();
    dl.hashes.insert(at, (rec.clone(), v));
    v
}
