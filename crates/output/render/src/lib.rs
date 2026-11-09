//! The live viewer's pages drawn as strips (docs/spec-display-list.md):
//! each page cut into bands, a band the drawing of one line (or of lines
//! and figures that touch), each band a bitmap of its own keyed by what it
//! draws relative to its own top. A build's page is then its bands' keys
//! and places: a band whose key the viewer has is not drawn again, and a
//! band that only moved (the lines below an edit that added a line) is the
//! same bitmap at another place. Only the bands whose drawing changed are
//! drawn, by hayro (every operator, colour space, shading, image filter
//! and font type a PDF holds), into a bitmap the band's size.
//!
//! The demo's input is the PDF the build wrote; the engine's display tap
//! (the spec's §1) replaces reading it back.
//!
//! - **Probe.** The page is interpreted once with a device that draws
//!   nothing: each drawing's box on the page (glyphs by their outlines'
//!   boxes, paths and images by theirs) and its identity (the glyphs and
//!   fonts, the paint, the path's points, the image, the clips and groups
//!   around it), its place the translation of its transform. The page's
//!   text and links come from the same pass.
//! - **Bands.** The drawings' vertical extents, merged where they come
//!   within [`GAP`] pixels of each other: a band holds whole drawings and
//!   no other band's pixels reach into its rows. A band's key hashes its
//!   drawings in order, each place relative to the band's top (x as it
//!   is), with its height, the page's width and the scale.
//! - **Strips.** The bands not drawn before are drawn, consecutive ones
//!   in one pass of the page over their rows (the pixels where nothing is
//!   drawn left clear), each band its rows of it from its top rounded to
//!   a pixel, where the viewer puts it (a band drawn at one place and
//!   shown, unchanged, at a place a fraction of a pixel away is off by
//!   less than a pixel).

#![allow(
    clippy::many_single_char_names,
    reason = "page geometry: x, y, w, h, k as the PDF names them"
)]

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::font::GlyphRun;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{
    BlendMode, CacheKey, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Object, ObjectIdentifier};
use hayro::hayro_syntax::page::Page;
use hayro::kurbo::{Affine, BezPath, PathEl, Rect, Shape};
use hayro::vello_cpu::{self, Pixmap, RenderContext};
use hayro::{RenderCache, RenderSettings, render_into};

/// Bands closer than this (pixels) are one: a drawing's anti-aliased edge
/// reaches a pixel beyond its box, a band's bitmap a pixel beyond its
/// drawings.
const GAP: f64 = 4.0;

/// Columns closer than this (points) are one: wider than a word space,
/// narrower than the space between two columns (LaTeX's `\columnsep`, 10
/// points; ACM's 2 picas). A two-column page's line is a band of its
/// own, not a band of every line beside a figure in the other column
/// (a keystroke in Grail's two-column text drew 3.4 MB of pixels again,
/// the whole column beside a figure).
const COLUMN_GAP: f64 = 6.0;

/// The strips kept for the viewer, at most this many bytes of pixels
/// (the oldest dropped first): 256 MiB natively; 64 MiB in WebAssembly,
/// whose 4 GiB memory the engine's threads share.
#[cfg(not(target_family = "wasm"))]
const KEEP_BYTES: usize = 256 << 20;
#[cfg(target_family = "wasm")]
const KEEP_BYTES: usize = 64 << 20;

/// A band's bitmap: `w` by `h` pixels, RGBA (not premultiplied), clear
/// where the band draws nothing.
pub struct Strip {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// A band of a page (a cell of its drawings: a line, or the part of a
/// line in one column): its key (its [`Strip`]'s), its top (pixels from
/// the page's top) and height, its left and width (its strip's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    pub key: u64,
    pub y: i32,
    pub h: u32,
    pub x: i32,
    pub w: u32,
}

/// A line of the page's text, in points from the page's top left: its
/// start and baseline, its width, its font's size, its characters and
/// each one's left and right edge (one a UTF-16 unit).
#[derive(Clone, Debug, Default)]
pub struct TextLine {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub size: f64,
    pub text: String,
    pub xs: Vec<f64>,
    pub rs: Vec<f64>,
}

/// Where a link goes.
#[derive(Clone, Debug)]
pub enum Target {
    Uri(String),
    /// A page (from 0) and a place on it (points from its top), if given.
    Page(usize, Option<f64>),
}

/// A link: its box (points from the page's top left: left, top, right,
/// bottom) and its target.
#[derive(Clone, Debug)]
pub struct Link {
    pub rect: [f64; 4],
    pub target: Target,
}

/// A page as the viewer shows it: its size in pixels and in points, its
/// bands top to bottom, its text and links; how many bands were drawn
/// now and how many kept, and the time each step took (microseconds).
#[derive(Debug, Default)]
pub struct Frame {
    pub pw: u32,
    pub ph: u32,
    pub w: f64,
    pub h: f64,
    pub bands: Vec<Band>,
    pub text: Vec<TextLine>,
    pub links: Vec<Link>,
    pub drawn: usize,
    pub kept: usize,
    /// The part of the page drawn ([`Renderer::frame_in`]'s window), in
    /// pixels: left, top, width, height; the whole page by default. Each
    /// band's strip is the window's width, from its left.
    pub win: [u32; 4],
    pub open_us: u64,
    pub probe_us: u64,
    pub draw_us: u64,
}

/// The PDF's bytes as hayro takes them.
struct Bytes(Arc<[u8]>);

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// The document open: its bytes (the build's) and hayro's reading of it.
struct Doc {
    bytes: Arc<[u8]>,
    pdf: Pdf,
    /// Each page's text, by page (a frame's probe's, or the text's own).
    texts: HashMap<usize, Vec<TextLine>>,
    /// The outline, read once a PDF.
    outline: Option<Vec<Entry>>,
    /// Its named destinations, each found once.
    names: Names,
}

/// The strips of a viewer's pages, across builds.
#[derive(Default)]
pub struct Renderer {
    doc: Option<Doc>,
    /// The strips kept, each with when it was last drawn or listed in a
    /// frame (a tick of [`Renderer::tick`]).
    strips: HashMap<u64, (Arc<Strip>, u64)>,
    /// Keys by when they were drawn or listed, oldest first; an entry whose
    /// tick is not its strip's now is stale (the strip was listed since).
    order: VecDeque<(u64, u64)>,
    tick: u64,
    bytes: usize,
    /// Each glyph's outline's box and the outline's hash (by hayro's glyph
    /// key, its font object and id: a subset made again for new
    /// characters is a new font object, its glyphs the same outlines).
    boxes: HashMap<u128, (Rect, u64)>,
}

fn hash_of(f: impl FnOnce(&mut DefaultHasher)) -> u64 {
    let mut h = DefaultHasher::new();
    f(&mut h);
    h.finish()
}

/// A length on the page, to a 64th of a pixel (positions computed from
/// the same numbers moved by the same amount hash the same).
#[allow(clippy::cast_possible_truncation)]
fn q(x: f64) -> i64 {
    (x * 64.0).round() as i64
}

/// A transform's linear part, to 1e-6.
#[allow(clippy::cast_possible_truncation)]
fn lin(a: Affine, h: &mut DefaultHasher) {
    for c in &a.as_coeffs()[..4] {
        ((c * 1e6).round() as i64).hash(h);
    }
}

/// What a drawing's paint and mode are, for its identity.
fn props_id(props: &DrawProps<'_>, mode: &DrawMode, h: &mut DefaultHasher) {
    props.paint.cache_key().hash(h);
    props.soft_mask.as_ref().map(CacheKey::cache_key).hash(h);
    format!("{:?}{mode:?}", props.blend_mode).hash(h);
    lin(props.transform, h);
}

/// A drawing on the probed page: its box (pixels), its identity, its
/// place.
struct Item {
    bbox: Rect,
    id: u64,
    ax: f64,
    ay: f64,
}

/// A glyph of the page's text: its origin, advance and size (pixels) and
/// its characters.
struct Ch {
    x: f64,
    y: f64,
    w: f64,
    size: f64,
    text: String,
}

/// The device of the probe: draws nothing, notes each drawing.
struct Probe<'b> {
    items: Vec<Item>,
    /// The clips and groups around what is drawn now, hashed.
    around: u64,
    saved: Vec<u64>,
    boxes: &'b mut HashMap<u128, (Rect, u64)>,
    chars: Vec<Ch>,
}

impl Probe<'_> {
    fn push(&mut self, bbox: Rect, id: u64, at: Affine) {
        if bbox.is_finite() && bbox.width() >= 0.0 && bbox.height() >= 0.0 {
            let t = at.translation();
            self.items.push(Item {
                bbox,
                id,
                ax: t.x,
                ay: t.y,
            });
        }
    }

    fn nest(&mut self, what: u64) {
        self.saved.push(self.around);
        self.around = hash_of(|h| (self.around, what).hash(h));
    }

    fn unnest(&mut self) {
        self.around = self.saved.pop().unwrap_or(0);
    }
}

/// A path's points relative to its first, for its identity (a path drawn
/// with absolute coordinates moved is the same path), and that first
/// point.
fn path_id(path: &BezPath, h: &mut DefaultHasher) -> hayro::kurbo::Point {
    let origin = path
        .elements()
        .iter()
        .find_map(|el| match el {
            PathEl::MoveTo(a)
            | PathEl::LineTo(a)
            | PathEl::QuadTo(a, _)
            | PathEl::CurveTo(a, _, _) => Some(*a),
            PathEl::ClosePath => None,
        })
        .unwrap_or_default();
    for el in path.elements() {
        let pts: &[_] = match el {
            PathEl::MoveTo(a) | PathEl::LineTo(a) => &[*a],
            PathEl::QuadTo(a, b) => &[*a, *b],
            PathEl::CurveTo(a, b, c) => &[*a, *b, *c],
            PathEl::ClosePath => &[],
        };
        std::mem::discriminant(el).hash(h);
        for pt in pts {
            (q(pt.x - origin.x), q(pt.y - origin.y)).hash(h);
        }
    }
    origin
}

impl<'a> Device<'a> for Probe<'_> {
    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, mode: &DrawMode) {
        if matches!(mode, DrawMode::Invisible) {
            return;
        }
        let mut bbox = (props.transform * path.clone()).bounding_box();
        if let DrawMode::Stroke(s) | DrawMode::FillAndStroke(_, s) = mode {
            let k = props.transform.determinant().abs().sqrt();
            bbox = bbox.inflate(
                f64::from(s.line_width) * k + 1.0,
                f64::from(s.line_width) * k + 1.0,
            );
        }
        let mut origin = hayro::kurbo::Point::ZERO;
        let id = hash_of(|h| {
            self.around.hash(h);
            props_id(&props, mode, h);
            origin = path_id(path, h);
        });
        self.push(
            bbox,
            id,
            props.transform * Affine::translate(origin.to_vec2()),
        );
    }

    fn push_clip_path(&mut self, clip: &ClipPath) {
        self.nest(hash_of(|h| clip.cache_key().hash(h)));
    }

    fn push_transparency_group(
        &mut self,
        opacity: f32,
        mask: Option<SoftMask<'a>>,
        blend: BlendMode,
    ) {
        self.nest(hash_of(|h| {
            opacity.to_bits().hash(h);
            mask.as_ref().map(CacheKey::cache_key).hash(h);
            format!("{blend:?}").hash(h);
        }));
    }

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, mode: &DrawMode) {
        let visible = !matches!(mode, DrawMode::Invisible);
        let mut bbox: Option<Rect> = None;
        let mut first: Option<Affine> = None;
        let mut h = DefaultHasher::new();
        self.around.hash(&mut h);
        props_id(&props, mode, &mut h);
        for g in run.glyphs() {
            let full = props.transform * g.transform();
            let (key, local, text, adv) = match &**g {
                Glyph::Outline(o) => {
                    let (local, key) = *self
                        .boxes
                        .entry(o.identifier().cache_key())
                        .or_insert_with(|| {
                            let outline = o.outline();
                            let mut h = DefaultHasher::new();
                            path_id(&outline, &mut h);
                            (outline.bounding_box(), h.finish())
                        });
                    let key = u128::from(key);
                    let text = o.as_unicode().map(|b| bf(&b)).unwrap_or_default();
                    let adv = o.advance_width().map_or(0.0, f64::from);
                    (key, local, text, adv)
                }
                Glyph::Type3(t) => {
                    let text = t.as_unicode().map(|b| bf(&b)).unwrap_or_default();
                    // (what its procedure draws, in the font's 1000 units
                    // to the em, once per glyph: its box, and its right
                    // edge as its advance, the font's widths being out of
                    // reach. As an advance of none, every letter of a
                    // Type 3 font, pdfTeX's bitmap ones, was a word apart)
                    let key = t.cache_key();
                    let (local, _) = *self.boxes.entry(key).or_insert_with(|| {
                        let mut inner = HashMap::new();
                        let mut sub = Probe {
                            items: Vec::new(),
                            around: 0,
                            saved: Vec::new(),
                            boxes: &mut inner,
                            chars: Vec::new(),
                        };
                        t.interpret(&mut sub, Affine::IDENTITY, Affine::IDENTITY, &props.paint);
                        let ink = sub.items.iter().map(|i| i.bbox).reduce(|a, b| a.union(b));
                        (ink.unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0)), 0)
                    });
                    let adv = local.x1.max(0.0);
                    (key, local, text, adv)
                }
            };
            let gb = full.transform_rect_bbox(local);
            let at = *first.get_or_insert(full);
            key.hash(&mut h);
            lin(full, &mut h);
            let (d, o) = (full.translation(), at.translation());
            (q(d.x - o.x), q(d.y - o.y)).hash(&mut h);
            if visible {
                bbox = Some(bbox.map_or(gb, |b| b.union(gb)));
            }
            // (the em: outlines are 1000 units to it)
            let em = full.as_coeffs()[3].abs() * 1000.0;
            let wx = full.as_coeffs()[0].abs() * adv;
            self.chars.push(Ch {
                x: d.x,
                y: d.y,
                w: wx,
                size: em,
                text,
            });
        }
        if let (Some(b), Some(at)) = (bbox, first) {
            self.push(b, h.finish(), at);
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        // (the image's transform takes its samples, `w` by `h`, to the
        // page: hayro puts the `1/w`, `-1/h` scale of the unit square in
        // front of the CTM. As the unit square, an image's box was its
        // corner sample, a pixel or less: no band held its place, and
        // every raster figure, PNG or JPEG, and every bitmap glyph a
        // Type 3 font draws as an image mask, was left blank)
        let bbox = props.transform.transform_rect_bbox(Rect::new(
            0.0,
            0.0,
            f64::from(image.width()),
            f64::from(image.height()),
        ));
        let id = hash_of(|h| {
            self.around.hash(h);
            image.cache_key().hash(h);
            props.soft_mask.as_ref().map(CacheKey::cache_key).hash(h);
            format!("{:?}", props.blend_mode).hash(h);
            lin(props.transform, h);
        });
        self.push(bbox, id, props.transform);
    }

    fn pop_clip(&mut self) {
        self.unnest();
    }

    fn pop_transparency_group(&mut self) {
        self.unnest();
    }
}

/// A `ToUnicode` value as text.
fn bf(b: &hayro::hayro_interpret::hayro_cmap::BfString) -> String {
    use hayro::hayro_interpret::hayro_cmap::BfString;
    match b {
        BfString::Char(c) => c.to_string(),
        BfString::String(s) => s.clone(),
    }
}

/// The page's glyphs as lines of text (points): a glyph starts a line
/// where its baseline moves or it goes back left.
fn lines_of(chars: &[Ch], scale: f64) -> Vec<TextLine> {
    let mut out: Vec<TextLine> = Vec::new();
    let mut cur: Option<TextLine> = None;
    let mut last_r = 0.0;
    for c in chars {
        let (x, y, w, size) = (c.x / scale, c.y / scale, c.w / scale, c.size / scale);
        let new = match &cur {
            None => true,
            Some(l) => (y - l.y).abs() > l.size * 0.3 || x < last_r - l.size,
        };
        if new {
            if let Some(l) = cur.take().filter(|l| !l.text.trim().is_empty()) {
                out.push(l);
            }
            cur = Some(TextLine {
                x,
                y,
                size,
                ..TextLine::default()
            });
        } else if let Some(l) = cur.as_mut() {
            // (a word space TeX leaves as a gap: a space, as the PDF's text
            // has it. A tenth of the em, as poppler's `pdftotext` takes
            // it: a justified line's spaces shrink below a fifth, Grail's
            // Libertine lines lost theirs, while kerns stay below a tenth)
            if x - last_r > size * 0.1 && !l.text.ends_with(' ') {
                l.text.push(' ');
                l.xs.push(last_r);
                l.rs.push(x);
            }
        }
        if let Some(l) = cur.as_mut() {
            for u in c.text.encode_utf16() {
                let _ = u;
                l.xs.push(x);
                l.rs.push(x + w);
            }
            l.text.push_str(&c.text);
            l.w = x + w - l.x;
        }
        last_r = x + w;
    }
    if let Some(l) = cur.filter(|l| !l.text.trim().is_empty()) {
        out.push(l);
    }
    out
}

/// The page index of a page object.
fn page_index(pages: &[Page<'_>], id: ObjectIdentifier) -> Option<usize> {
    pages.iter().position(|p| p.raw().obj_id() == Some(id))
}

/// A destination (`[page /XYZ x y z]` and its kin) as a page and a top.
fn dest_of(pages: &[Page<'_>], arr: &Array<'_>) -> Option<(usize, Option<f64>)> {
    let mut it = arr.raw_iter();
    let page = match it.next()? {
        MaybeRef::Ref(r) => page_index(pages, r.into())?,
        MaybeRef::NotRef(Object::Number(n)) => usize::try_from(n.as_i64()).ok()?,
        MaybeRef::NotRef(_) => return None,
    };
    let kind = it.next().and_then(|o| match o {
        MaybeRef::NotRef(Object::Name(n)) => Some(n.as_str().to_owned()),
        _ => None,
    });
    let top = match kind.as_deref() {
        Some("XYZ") => {
            let _left = it.next();
            it.next().and_then(|o| match o {
                MaybeRef::NotRef(Object::Number(n)) => Some(n.as_f64()),
                _ => None,
            })
        }
        Some("FitH" | "FitBH") => it.next().and_then(|o| match o {
            MaybeRef::NotRef(Object::Number(n)) => Some(n.as_f64()),
            _ => None,
        }),
        _ => None,
    };
    let h = f64::from(pages[page].render_dimensions().1);
    let y0 = pages[page].media_box().y0;
    Some((page, top.map(|t| h - (t - y0))))
}

/// A destination's value (an array, or a dictionary with one under `/D`).
fn as_dest(o: Object<'_>) -> Option<Array<'_>> {
    match o {
        Object::Array(a) => Some(a),
        Object::Dict(d) => d.get::<Array<'_>>(b"D"),
        _ => None,
    }
}

/// A name of the `/Names` `/Dests` tree: its value's object, or its place
/// (a value written inside its node).
enum NameVal {
    Ref(ObjectIdentifier),
    At(Option<(usize, Option<f64>)>),
}

/// A document's named destinations (the catalog's `/Dests` dictionary,
/// its `/Names` `/Dests` tree), each name's place found once: the tree's
/// nodes read once a document and a name's value only when it is asked.
/// (A search of the tree for every link read every value of each leaf it
/// passed, each in an object stream whose offsets hayro reads again for
/// every object: two thirds of a frame of a page with citations and
/// references, Grail's 6.7 ms of 23 in WebAssembly.)
#[derive(Default)]
struct Names {
    /// The tree's names (none: not read yet).
    tree: Option<HashMap<Vec<u8>, NameVal>>,
    /// Each name asked, and its place.
    asked: HashMap<Vec<u8>, Option<(usize, Option<f64>)>>,
}

impl Names {
    /// Named destination `name`'s page and top.
    fn place(
        &mut self,
        pdf: &Pdf,
        pages: &[Page<'_>],
        name: &[u8],
    ) -> Option<(usize, Option<f64>)> {
        if let Some(p) = self.asked.get(name) {
            return *p;
        }
        let p = self.find(pdf, pages, name);
        self.asked.insert(name.to_vec(), p);
        p
    }

    fn find(&mut self, pdf: &Pdf, pages: &[Page<'_>], name: &[u8]) -> Option<(usize, Option<f64>)> {
        let xref = pdf.xref();
        let root: Dict<'_> = xref.get(xref.root_id())?;
        if let Some(d) = root.get::<Dict<'_>>(b"Dests")
            && let Some(o) = d.get::<Object<'_>>(name)
        {
            return as_dest(o).and_then(|a| dest_of(pages, &a));
        }
        let tree = self.tree.get_or_insert_with(|| names_of(pdf, &root, pages));
        match tree.get(name)? {
            NameVal::At(p) => *p,
            NameVal::Ref(id) => xref
                .get::<Object<'_>>(*id)
                .and_then(as_dest)
                .and_then(|a| dest_of(pages, &a)),
        }
    }
}

/// The `/Names` `/Dests` tree's names, every node read once (its values
/// not: a reference kept, resolved when asked), the first of a name kept.
fn names_of(pdf: &Pdf, root: &Dict<'_>, pages: &[Page<'_>]) -> HashMap<Vec<u8>, NameVal> {
    let mut out = HashMap::new();
    let Some(tree) = root
        .get::<Dict<'_>>(b"Names")
        .and_then(|n| n.get::<Dict<'_>>(b"Dests"))
    else {
        return out;
    };
    let xref = pdf.xref();
    let mut stack = vec![tree];
    // (at most this many nodes: a cycle of /Kids ends there)
    let mut budget = 100_000usize;
    while let Some(node) = stack.pop() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        if let Some(names) = node.get::<Array<'_>>(b"Names") {
            let mut it = names.raw_iter();
            while let (Some(k), Some(v)) = (it.next(), it.next()) {
                let key = match k {
                    MaybeRef::NotRef(Object::String(s)) => s.as_bytes().to_vec(),
                    MaybeRef::Ref(r) => match xref.get::<Object<'_>>(r.into()) {
                        Some(Object::String(s)) => s.as_bytes().to_vec(),
                        _ => continue,
                    },
                    MaybeRef::NotRef(_) => continue,
                };
                out.entry(key).or_insert_with(|| match v {
                    MaybeRef::Ref(r) => NameVal::Ref(r.into()),
                    MaybeRef::NotRef(o) => NameVal::At(as_dest(o).and_then(|a| dest_of(pages, &a))),
                });
            }
        }
        if let Some(kids) = node.get::<Array<'_>>(b"Kids") {
            // (in reverse: the first kid's names read first)
            let kids: Vec<Dict<'_>> = kids.iter::<Dict<'_>>().collect();
            stack.extend(kids.into_iter().rev());
        }
    }
    out
}

/// A page's links (points from its top left).
fn links_of(pdf: &Pdf, pages: &[Page<'_>], page: &Page<'_>, names: &mut Names) -> Vec<Link> {
    let mut out = Vec::new();
    let Some(annots) = page.raw().get::<Array<'_>>(b"Annots") else {
        return out;
    };
    let h = f64::from(page.render_dimensions().1);
    let mb = page.media_box();
    for a in annots.iter::<Dict<'_>>() {
        if a.get::<Object<'_>>(b"Subtype")
            .and_then(Object::into_name)
            .is_none_or(|n| n.as_str() != "Link")
        {
            continue;
        }
        let Some(r) = a.get::<Array<'_>>(b"Rect") else {
            continue;
        };
        let v: Vec<f64> = r.iter::<f64>().collect();
        if v.len() < 4 {
            continue;
        }
        let rect = [
            v[0].min(v[2]) - mb.x0,
            h - (v[1].max(v[3]) - mb.y0),
            v[0].max(v[2]) - mb.x0,
            h - (v[1].min(v[3]) - mb.y0),
        ];
        let mut dest = |o: Object<'_>| -> Option<Target> {
            match o {
                Object::Array(a) => dest_of(pages, &a),
                Object::String(s) => names.place(pdf, pages, s.as_bytes()),
                Object::Name(n) => names.place(pdf, pages, n.as_str().as_bytes()),
                _ => None,
            }
            .map(|(p, y)| Target::Page(p, y))
        };
        let target = if let Some(act) = a.get::<Dict<'_>>(b"A") {
            match act
                .get::<Object<'_>>(b"S")
                .and_then(Object::into_name)
                .map(|n| n.as_str().to_owned())
                .as_deref()
            {
                Some("URI") => act
                    .get::<Object<'_>>(b"URI")
                    .and_then(Object::into_string)
                    .map(|s| Target::Uri(String::from_utf8_lossy(s.as_bytes()).into_owned())),
                Some("GoTo") => act.get::<Object<'_>>(b"D").and_then(&mut dest),
                _ => None,
            }
        } else {
            a.get::<Object<'_>>(b"Dest").and_then(&mut dest)
        };
        if let Some(target) = target {
            out.push(Link { rect, target });
        }
    }
    out
}

/// Pixels premultiplied by their alpha made straight (as a canvas's
/// `ImageData` takes them).
fn unpremultiply(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a == 0 {
            p[..3].fill(0);
        } else if a < 255 {
            for c in &mut p[..3] {
                *c = u8::try_from(((u32::from(*c) * 255 + a / 2) / a).min(255)).unwrap_or(255);
            }
        }
    }
}

/// Page `page` interpreted by the probe through `base` (to pixels, the
/// page `pw` by `ph` of them): its drawings on the page, their boxes
/// clipped to it, and its glyphs.
fn probe(
    page: &Page<'_>,
    base: Affine,
    pw: f64,
    ph: f64,
    boxes: &mut HashMap<u128, (Rect, u64)>,
) -> (Vec<Item>, Vec<Ch>) {
    let icache = InterpreterCache::new();
    let mut probe = Probe {
        items: Vec::new(),
        around: 0,
        saved: Vec::new(),
        boxes,
        chars: Vec::new(),
    };
    let mut ctx = Context::new(
        base,
        Rect::new(0.0, 0.0, pw, ph),
        &icache,
        page.xref(),
        InterpreterSettings::default(),
    );
    interpret_page(page, &mut ctx, &mut probe);
    let page_rect = Rect::new(0.0, 0.0, pw, ph);
    let mut items: Vec<Item> = probe
        .items
        .into_iter()
        .filter(|i| i.bbox.intersect(page_rect).area() > 0.0 || i.bbox.height() == 0.0)
        .collect();
    for i in &mut items {
        i.bbox = i.bbox.intersect(page_rect.inflate(1.0, 1.0));
    }
    (items, probe.chars)
}

/// An entry of the PDF's outline: its title, its page (from 0) and place
/// (points from the page's top), and its entries.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub title: String,
    pub page: Option<usize>,
    pub y: Option<f64>,
    pub kids: Vec<Entry>,
}

/// A PDF text string: UTF-16BE after its byte order mark, else
/// `PDFDocEncoding` (Latin-1 where TeX's bookmarks use it).
fn text_string(b: &[u8]) -> String {
    if let Some(u) = b.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = u
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    } else if let Some(u) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(u).into_owned()
    } else {
        b.iter().map(|&c| char::from(c)).collect()
    }
}

/// The catalog's `/Outlines`: each entry's title and destination, its
/// `/First` its entries, `/Next` the one after it.
fn outline_of(pdf: &Pdf, names: &mut Names) -> Vec<Entry> {
    fn level(
        pdf: &Pdf,
        pages: &[Page<'_>],
        first: Dict<'_>,
        budget: &mut usize,
        names: &mut Names,
    ) -> Vec<Entry> {
        let mut out = Vec::new();
        let mut cur = Some(first);
        while let Some(d) = cur {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            let title = d
                .get::<Object<'_>>(b"Title")
                .and_then(Object::into_string)
                .map(|s| text_string(s.as_bytes()))
                .unwrap_or_default();
            let mut dest = |o: Object<'_>| -> Option<(usize, Option<f64>)> {
                match o {
                    Object::Array(a) => dest_of(pages, &a),
                    Object::String(s) => names.place(pdf, pages, s.as_bytes()),
                    Object::Name(n) => names.place(pdf, pages, n.as_str().as_bytes()),
                    _ => None,
                }
            };
            let place = match d.get::<Dict<'_>>(b"A") {
                Some(a) => a.get::<Object<'_>>(b"D").and_then(&mut dest),
                None => d.get::<Object<'_>>(b"Dest").and_then(&mut dest),
            };
            let kids = d
                .get::<Dict<'_>>(b"First")
                .map(|f| level(pdf, pages, f, budget, names))
                .unwrap_or_default();
            out.push(Entry {
                title,
                page: place.map(|p| p.0),
                y: place.and_then(|p| p.1),
                kids,
            });
            cur = d.get::<Dict<'_>>(b"Next");
        }
        out
    }
    let xref = pdf.xref();
    let Some(root) = xref.get::<Dict<'_>>(xref.root_id()) else {
        return Vec::new();
    };
    let pages = pdf.pages();
    let Some(first) = root
        .get::<Dict<'_>>(b"Outlines")
        .and_then(|o| o.get::<Dict<'_>>(b"First"))
    else {
        return Vec::new();
    };
    // (at most this many entries: a cycle of /Next ends there)
    let mut budget = 100_000usize;
    level(pdf, pages, first, &mut budget, names)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn micros(t: Instant) -> u64 {
    t.elapsed().as_micros() as u64
}

impl Renderer {
    #[must_use]
    pub fn new() -> Renderer {
        Renderer::default()
    }

    /// A strip drawn for a frame (`None`: unknown, or dropped since).
    #[must_use]
    pub fn strip(&self, key: u64) -> Option<Arc<Strip>> {
        self.strips.get(&key).map(|(s, _)| s.clone())
    }

    /// `s` kept as `key`'s strip, the newest; the oldest dropped past
    /// [`KEEP_BYTES`]. A key drawn again replaces its strip and its bytes:
    /// counted twice (a page's identical bands are one key, each drawn),
    /// the count grew past the budget for good, and every strip was then
    /// dropped as soon as it was drawn, the viewer's pages blank.
    fn keep(&mut self, key: u64, s: Strip) {
        self.tick += 1;
        self.bytes += s.rgba.len();
        if let Some((old, _)) = self.strips.insert(key, (Arc::new(s), self.tick)) {
            self.bytes -= old.rgba.len();
        }
        self.order.push_back((key, self.tick));
        self.evict();
    }

    /// `key`'s strip made the newest: a frame lists it, and the viewer
    /// asks for it next. Kept in the order it was first drawn, a band a
    /// frame listed (kept from an earlier build or zoom) was among the
    /// first dropped when the frame's new bands were drawn, before the
    /// viewer could fetch it: a blank band.
    fn touch(&mut self, key: u64) {
        self.tick += 1;
        if let Some(e) = self.strips.get_mut(&key) {
            e.1 = self.tick;
            self.order.push_back((key, self.tick));
        }
    }

    /// The oldest strips dropped while they are past [`KEEP_BYTES`]; the
    /// order's stale entries dropped once they outnumber the strips.
    fn evict(&mut self) {
        while self.bytes > KEEP_BYTES {
            let Some((k, t)) = self.order.pop_front() else {
                break;
            };
            if self.strips.get(&k).is_some_and(|e| e.1 == t)
                && let Some((s, _)) = self.strips.remove(&k)
            {
                self.bytes -= s.rgba.len();
            }
        }
        if self.order.len() > 2 * self.strips.len() + 1024 {
            let strips = &self.strips;
            self.order
                .retain(|(k, t)| strips.get(k).is_some_and(|e| e.1 == *t));
        }
    }

    /// Page `k` (from 0) of `pdf` at `scale` pixels a point: its bands,
    /// those not drawn before drawn now; `None` where the PDF does not
    /// open or has no such page.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::too_many_lines
    )]
    pub fn frame(&mut self, pdf: &Arc<[u8]>, k: usize, scale: f32) -> Option<Frame> {
        self.frame_in(pdf, k, scale, None)
    }

    /// [`Renderer::frame`], only the part of the page in `win` (fractions
    /// of its width and height: left, top, right, bottom) drawn: the bands
    /// it crosses, each cut to it. A zoomed-in viewer's (a whole page at
    /// 800% is 86 million pixels, which no browser's memory holds; the
    /// part on screen is a few).
    pub fn frame_in(
        &mut self,
        pdf: &Arc<[u8]>,
        k: usize,
        scale: f32,
        win: Option<[f32; 4]>,
    ) -> Option<Frame> {
        let t0 = Instant::now();
        let mut doc = self.open(pdf)?;
        let open_us = micros(t0);
        let out = self.frame_of(&mut doc, k, scale, win, open_us);
        self.doc = Some(doc);
        out
    }

    /// The document of `pdf`, taken out (put back by the caller): the one
    /// open, or `pdf` opened now.
    fn open(&mut self, pdf: &Arc<[u8]>) -> Option<Doc> {
        match self.doc.take() {
            Some(d) if Arc::ptr_eq(&d.bytes, pdf) => Some(d),
            _ => Some(Doc {
                bytes: pdf.clone(),
                pdf: Pdf::new(Arc::new(Bytes(pdf.clone()))).ok()?,
                texts: HashMap::new(),
                outline: None,
                names: Names::default(),
            }),
        }
    }

    /// How many pages `pdf` has (`None`: it does not open).
    pub fn pages(&mut self, pdf: &Arc<[u8]>) -> Option<usize> {
        let doc = self.open(pdf)?;
        let n = doc.pdf.pages().len();
        self.doc = Some(doc);
        Some(n)
    }

    /// Page `k`'s text, line by line (points from its top left).
    pub fn text(&mut self, pdf: &Arc<[u8]>, k: usize) -> Option<Vec<TextLine>> {
        let mut doc = self.open(pdf)?;
        if !doc.texts.contains_key(&k) {
            let pages = doc.pdf.pages();
            if let Some(page) = pages.get(k) {
                let base = page.initial_transform(true).to_kurbo();
                let (w, h) = page.render_dimensions();
                let (_, chars) = probe(page, base, f64::from(w), f64::from(h), &mut self.boxes);
                let lines = lines_of(&chars, 1.0);
                doc.texts.insert(k, lines);
            }
        }
        let out = doc.texts.get(&k).cloned();
        self.doc = Some(doc);
        out
    }

    /// The PDF's outline (its bookmarks), its entries' pages and places.
    pub fn outline(&mut self, pdf: &Arc<[u8]>) -> Vec<Entry> {
        let Some(mut doc) = self.open(pdf) else {
            return Vec::new();
        };
        let out = doc
            .outline
            .get_or_insert_with(|| outline_of(&doc.pdf, &mut doc.names))
            .clone();
        self.doc = Some(doc);
        out
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap
    )]
    fn frame_of(
        &mut self,
        doc: &mut Doc,
        k: usize,
        scale: f32,
        win: Option<[f32; 4]>,
        open_us: u64,
    ) -> Option<Frame> {
        let pdf = &doc.pdf;
        let pages = pdf.pages();
        let page = pages.get(k)?;
        let (w, h) = page.render_dimensions();
        let s = f64::from(scale);
        let pw = (f64::from(w) * s).ceil().max(1.0) as u32;
        let ph = (f64::from(h) * s).ceil().max(1.0) as u32;
        let base = Affine::scale(s) * page.initial_transform(true).to_kurbo();
        let settings = InterpreterSettings::default();
        // (the window in pixels, at least one pixel of the page)
        let [wx, wy, ww, wh] = win.map_or([0, 0, pw, ph], |[x0, y0, x1, y1]| {
            let at = |f: f32, n: u32, up: bool| {
                let p = f64::from(f.clamp(0.0, 1.0)) * f64::from(n);
                (if up { p.ceil() } else { p.floor() }) as u32
            };
            let (x0, y0) = (at(x0, pw, false).min(pw - 1), at(y0, ph, false).min(ph - 1));
            let (x1, y1) = (at(x1, pw, true).max(x0 + 1), at(y1, ph, true).max(y0 + 1));
            [x0, y0, x1.min(pw) - x0, y1.min(ph) - y0]
        });

        // (the probe: every drawing's box and identity, the text)
        let t1 = Instant::now();
        let (items, chars) = probe(page, base, f64::from(pw), f64::from(ph), &mut self.boxes);
        let text = lines_of(&chars, s);
        doc.texts.insert(k, text.clone());
        let links = links_of(pdf, pages, page, &mut doc.names);

        // (the bands: the drawings cut where they leave a gap, rows, then
        // in a row its columns, then their rows, ...)
        let mut cells: Vec<Vec<usize>> = Vec::new();
        cut(
            &items,
            (0..items.len()).collect(),
            true,
            false,
            &mut cells,
            s,
        );
        let probe_us = micros(t1);

        let t2 = Instant::now();
        // (each band's place in pixels and key: its drawings in order,
        // placed from its top left, cut to the window, its rows and columns
        // from its own top left too)
        let (wtop, wbottom) = (wy as i32, (wy + wh) as i32);
        let (wleft, wright) = (wx as i32, (wx + ww) as i32);
        let mut bands: Vec<Band> = cells
            .iter()
            .filter_map(|c| {
                let r = c.iter().map(|&i| items[i].bbox).reduce(|a, b| a.union(b))?;
                let top = (r.y0 - 1.0).round() as i32;
                let bottom = top + ((r.y1 + 1.0) - (r.y0 - 1.0)).ceil().max(1.0) as i32;
                let left = (r.x0 - 1.0).round() as i32;
                let right = left + ((r.x1 + 1.0) - (r.x0 - 1.0)).ceil().max(1.0) as i32;
                let (from, to) = (top.max(wtop), bottom.min(wbottom));
                let (l, rt) = (left.max(wleft), right.min(wright));
                if from >= to || l >= rt {
                    return None;
                }
                let mut h = DefaultHasher::new();
                (right - left, q(r.y1 - r.y0), scale.to_bits()).hash(&mut h);
                let (x0, y0) = (r.x0 - 1.0, r.y0 - 1.0);
                for &i in c {
                    let it = &items[i];
                    (it.id, q(it.ax - x0), q(it.ay - y0)).hash(&mut h);
                }
                (l - left, rt - l, from - top, to - from).hash(&mut h);
                Some(Band {
                    key: h.finish(),
                    y: from,
                    h: (to - from) as u32,
                    x: l,
                    w: (rt - l) as u32,
                })
            })
            .collect();
        bands.sort_by_key(|b| (b.y, b.x));
        // (a key drawn once a frame: identical bands, a repeated line or
        // rule, are one strip; those kept made the newest, so drawing the
        // new ones never drops them before the viewer fetches them)
        let mut seen = std::collections::HashSet::new();
        let fresh: Vec<bool> = bands
            .iter()
            .map(|b| !self.strips.contains_key(&b.key) && seen.insert(b.key))
            .collect();
        for (b, f) in bands.iter().zip(&fresh) {
            if !f {
                self.touch(b.key);
            }
        }
        let drawn = fresh.iter().filter(|f| **f).count();
        let kept = bands.len() - drawn;
        self.draw(page, base, &settings, &bands, &fresh)?;
        Some(Frame {
            pw,
            ph,
            w: f64::from(w),
            h: f64::from(h),
            bands,
            text,
            links,
            drawn,
            kept,
            win: [wx, wy, ww, wh],
            open_us,
            probe_us,
            draw_us: micros(t2),
        })
    }

    /// The bands of `bands` that are `fresh` drawn and kept: those near
    /// each other in one pass of the page over the rectangle around them
    /// (hayro skipping what falls outside), each band its pixels of it.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        reason = "a page's pixels: a band's size fits an i32"
    )]
    fn draw(
        &mut self,
        page: &Page<'_>,
        base: Affine,
        settings: &InterpreterSettings,
        bands: &[Band],
        fresh: &[bool],
    ) -> Option<()> {
        let cache = RenderCache::new();
        let todo: Vec<&Band> = bands
            .iter()
            .zip(fresh)
            .filter_map(|(b, f)| f.then_some(b))
            .collect();
        let rect = |b: &Band| (b.x, b.y, b.x + b.w as i32, b.y + b.h as i32);
        let area = |(x0, y0, x1, y1): (i32, i32, i32, i32)| i64::from(x1 - x0) * i64::from(y1 - y0);
        let mut i = 0;
        while i < todo.len() {
            // (the next bands drawn with it while the rectangle around them
            // is at most twice their pixels: the lines below an edit in one
            // pass, a line in each column apart)
            let mut around = rect(todo[i]);
            let mut sum = area(around);
            let mut e = i + 1;
            while e < todo.len() {
                let r = rect(todo[e]);
                let u = (
                    around.0.min(r.0),
                    around.1.min(r.1),
                    around.2.max(r.2),
                    around.3.max(r.3),
                );
                if area(u) > 2 * (sum + area(r)) {
                    break;
                }
                around = u;
                sum += area(r);
                e += 1;
            }
            let (x0, y0, x1, y1) = around;
            let (sw, sh) = (u16::try_from(x1 - x0).ok()?, u16::try_from(y1 - y0).ok()?);
            let mut ctx = RenderContext::new(sw, sh);
            render_into(
                page,
                &cache,
                settings,
                &RenderSettings::default(),
                &mut ctx,
                Affine::translate((-f64::from(x0), -f64::from(y0))) * base,
            );
            ctx.flush();
            let mut pix = Pixmap::new(sw, sh);
            ctx.render_with(
                &mut pix,
                &mut vello_cpu::Resources::default(),
                vello_cpu::RasterizerSettings {
                    target_init: vello_cpu::TargetInit::Clear(
                        vello_cpu::color::palette::css::TRANSPARENT,
                    ),
                    ..Default::default()
                },
            );
            let all = pix.data_as_u8_slice();
            let row = usize::from(sw) * 4;
            for band in &todo[i..e] {
                let (dx, dy) = ((band.x - x0) as usize, (band.y - y0) as usize);
                let w = band.w as usize;
                let mut rgba = Vec::with_capacity(w * band.h as usize * 4);
                for y in dy..dy + band.h as usize {
                    rgba.extend_from_slice(&all[y * row + dx * 4..][..w * 4]);
                }
                unpremultiply(&mut rgba);
                self.keep(
                    band.key,
                    Strip {
                        w: band.w,
                        h: band.h,
                        rgba,
                    },
                );
            }
            i = e;
        }
        Some(())
    }
}

/// The drawings `idx` of `items` cut into cells, into `out`: where their
/// boxes leave a gap across the page (`rows`: a gap between rows, of
/// [`GAP`] pixels; else between columns, of [`COLUMN_GAP`] points at
/// `scale`), each part cut again the other way, until a part has no gap
/// either way. A cell holds whole drawings, and no other cell's pixels
/// reach into it.
fn cut(
    items: &[Item],
    idx: Vec<usize>,
    rows: bool,
    tried: bool,
    out: &mut Vec<Vec<usize>>,
    scale: f64,
) {
    if idx.is_empty() {
        return;
    }
    let gap = if rows {
        GAP
    } else {
        (COLUMN_GAP * scale).max(GAP)
    };
    let span = |i: usize| {
        let b = items[i].bbox;
        if rows { (b.y0, b.y1) } else { (b.x0, b.x1) }
    };
    let mut by: Vec<usize> = idx;
    by.sort_by(|&a, &b| span(a).0.total_cmp(&span(b).0));
    let mut parts: Vec<Vec<usize>> = Vec::new();
    let mut end = f64::NEG_INFINITY;
    for i in by {
        let (a, b) = span(i);
        match parts.last_mut() {
            Some(p) if a <= end + gap => {
                p.push(i);
                end = end.max(b);
            }
            _ => {
                parts.push(vec![i]);
                end = b;
            }
        }
    }
    if parts.len() == 1 {
        let mut part = parts.pop().unwrap_or_default();
        if tried {
            // (the drawings in the page's order: a cell's key hashes them so)
            part.sort_unstable();
            out.push(part);
        } else {
            cut(items, part, !rows, true, out, scale);
        }
        return;
    }
    for p in parts {
        cut(items, p, !rows, false, out, scale);
    }
}

#[cfg(test)]
mod tests;
