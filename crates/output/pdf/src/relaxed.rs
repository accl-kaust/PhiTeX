//! The relaxed mode (RELAXED.md): every page, and every object TeX makes
//! when it reads a command, is written on its own, with symbolic object
//! numbers (handles); [`finish_relaxed`] merges the pieces, numbers the
//! objects in the order they are defined and writes the cross-reference
//! table. The writer's methods are the exact mode's: each piece is
//! written by a fresh [`PdfState`] whose own object numbers are mapped
//! to handles afterwards.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;
use phitex_typeset::node::{BoxNode, Node};

use crate::color::Stacks;
use crate::marks::LinkLevel;
use crate::objects::ObjBody;
const KIDS: usize = 6;

use crate::{
    Action, ColorStackCmd, FontSource, FontState, Ident, OBJ_TYPE_DEST, OBJ_TYPE_FONT,
    OBJ_TYPE_OBJ, OBJ_TYPE_PAGE, OBJ_TYPE_PAGES, OBJ_TYPE_THREAD, OBJ_TYPE_XFORM, OBJ_TYPE_XIMAGE,
    PAGES_TREE_KIDS_MAX, PdfError, PdfParams, PdfState, Whatsit, WhatsitSource,
};

/// The first handle.
pub const HANDLE_BASE: usize = 1 << 30;
/// The read-time objects' half.
const READ_BASE: usize = HANDLE_BASE + (1 << 29);
/// Objects per page.
pub const PAGE_SLOTS: usize = 1 << 12;
/// Pages (the read-time half begins past them).
pub const MAX_PAGES: usize = (1 << 17) - 1;
/// Objects per read-time owner.
const READ_SLOTS: usize = 64;
/// The finishing state's own objects (never seen by TeX).
const FINISH_BASE: usize = 1 << 31;

/// The handle of page `n`'s `i`-th object (`i` 0: the page object).
///
/// # Errors
/// Past `MAX_PAGES` pages or `PAGE_SLOTS` objects on a page.
pub fn page_handle(n: usize, i: usize) -> Result<usize, PdfError> {
    if n == 0 || n > MAX_PAGES || i >= PAGE_SLOTS {
        return Err(PdfError::Unsupported(format!(
            "relaxed mode: page {n}, object {i} past the handles' room"
        )));
    }
    Ok(HANDLE_BASE + (n - 1) * PAGE_SLOTS + i)
}

/// The handle of a read-time owner's `i`-th object; `key` is the
/// owner's identity.
#[must_use]
pub fn read_handle(key: u64, i: usize) -> usize {
    let room = 1u64 << 29;
    let base = usize::try_from(key % room).unwrap_or(0) & !(READ_SLOTS - 1);
    READ_BASE + base + i.min(READ_SLOTS - 1)
}

/// The owner key of a named thing (a font, a destination, a pages-tree
/// node).
fn key_of(kind: &str, data: &[u8]) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    std::hash::Hasher::write(&mut h, kind.as_bytes());
    std::hash::Hasher::write_u8(&mut h, 0);
    std::hash::Hasher::write(&mut h, data);
    std::hash::Hasher::finish(&h)
}

fn ident_bytes(id: &Ident) -> Vec<u8> {
    match id {
        Ident::Num(n) => format!("num {n}").into_bytes(),
        Ident::Name(s) => [b"name ".as_slice(), s].concat(),
    }
}

/// A destination's handle.
#[must_use]
pub fn dest_handle(id: &Ident) -> usize {
    read_handle(key_of("dest", &ident_bytes(id)), 0)
}

/// The handle of the pages-tree node over pages `6j+1 ..= 6j+6`.
fn leaf_handle(j: usize) -> usize {
    read_handle(key_of("pages", j.to_string().as_bytes()), 0)
}

/// A font's handle: fonts sharing one PDF font share it.
/// The handle of the transparent PNGs' group object.
fn png_group_handle() -> usize {
    read_handle(key_of("png group", b""), 0)
}

/// The group object of transparent PNGs, as pdfTeX writes it
/// (`write_additional_png_objects`), under its handle.
fn png_group_piece(params: &PdfParams) -> Result<Piece, PdfError> {
    let mut st = node_state(params);
    let g = st.new_objnum_();
    st.begin_obj(g, 2);
    if st.p.compress_level == 0 {
        st.print(b"%PTEX Group needed for transparent pngs\n");
    }
    st.print(b"<</Type/Group /S/Transparency /CS/DeviceRGB /I true>>\n");
    st.end_obj();
    let fixed = HashMap::from([(g, png_group_handle())]);
    let mut other = |k: usize| -> Result<usize, PdfError> { Ok(k) };
    piece(
        &st,
        "the transparent PNGs' group".to_owned(),
        &Imports::default(),
        &fixed,
        &mut other,
        None,
        None,
    )
}

fn font_handle(key: u64) -> usize {
    read_handle(key, 0)
}

/// What a piece needs from a font to merge it: its state with the
/// characters used.
#[derive(Clone, Debug)]
pub struct FontUse {
    key: u64,
    state: Arc<FontState>,
}

/// A form or an image as the pages that reference it see it.
#[derive(Clone, Debug)]
enum Made {
    Form(crate::objects::XForm),
    Image(crate::image::XImage),
    Obj,
}

/// What one page, or one object TeX makes when it reads a command, wrote.
#[derive(Clone, Debug)]
pub struct Piece {
    /// The bytes, objects numbered by handle.
    bytes: Vec<u8>,
    /// The objects defined, in order: handle and offset.
    defs: Vec<(usize, usize)>,
    /// Who wrote it (for a collision's report).
    owner: String,
    /// The fonts used and their characters.
    fonts: Vec<(FontId, FontUse)>,
    /// Named destinations defined: name and handle.
    dest_names: Vec<(Vec<u8>, usize)>,
    /// Destinations referenced (to be made if none is defined).
    dest_refs: BTreeSet<(Vec<u8>, usize)>,
    /// The object a read-time command made (`\pdflastobj` ...), for the
    /// pages that reference it.
    made: Option<(usize, Made)>,
    /// Why the document cannot be written in relaxed mode, if it cannot.
    fallback: Option<String>,
    /// A page's `cur_page_height`; for a form written before the page
    /// that references it, the one it was written under (a form's
    /// origin is the page's: pdfTeX writes it after that page).
    height: Option<Scaled>,
}

impl Piece {
    /// The handle a read-time command's object has.
    #[must_use]
    pub fn handle(&self) -> Option<usize> {
        self.made.as_ref().map(|m| m.0)
    }

    /// A digest of everything the finish reads (a memo key: equal
    /// pieces give equal keys).
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut b = self.bytes.clone();
        let fonts: Vec<(FontId, u64, &[bool; 256])> = self
            .fonts
            .iter()
            .map(|(f, u)| (*f, u.key, &u.state.chars))
            .collect();
        let made = self.made.as_ref().map(|m| m.0);
        b.extend_from_slice(
            format!(
                "{:?}|{}|{fonts:?}|{:?}|{:?}|{made:?}|{:?}|{:?}",
                self.defs, self.owner, self.dest_names, self.dest_refs, self.fallback, self.height
            )
            .as_bytes(),
        );
        key_of("piece", &b)
    }

    /// Whether the piece's bytes refer to handle `h`.
    fn refs_handle(&self, h: usize) -> bool {
        let mut found = false;
        let _ = rewrite(&self.bytes, |n| {
            found |= n == h;
            None
        });
        found
    }

    /// A page's `cur_page_height`.
    #[must_use]
    pub fn page_height(&self) -> Option<Scaled> {
        self.height
    }

    /// Why the document falls back to exact mode, if it does.
    #[must_use]
    pub fn fallback(&self) -> Option<&str> {
        self.fallback.as_deref()
    }

    /// A form's size, an image's size and what TeX reads of it.
    #[must_use]
    pub fn ximage_info(&self) -> Option<crate::XImageInfo> {
        match &self.made {
            Some((_, Made::Image(x))) => Some(x.info()),
            _ => None,
        }
    }
}

/// What persists from page to page outside the objects: the colour
/// stacks, the links still open, `\pdfinterwordspaceon` and
/// `\pdfrunninglinkoff`. The page builder's fold carries it
/// ([`carry_after`]); given it, the pages are independent.
#[derive(Clone, Debug, Default)]
pub struct Carry {
    stacks: Stacks,
    links: Vec<LinkLevel>,
    faked_space: bool,
    running_link_off: bool,
    /// The fonts pages so far set (`font_used` decides font switches
    /// while the interword switch is on), kept once the switch has been
    /// on: the digest leaves them out until then, so that documents
    /// without the switch do not tie a page to the fonts before it.
    used: BTreeSet<FontId>,
    faked_ever: bool,
}

impl Carry {
    /// The colour stacks `\pdfcolorstackinit` made so far (page, direct,
    /// initial text), in order: the ones the carry does not have yet are
    /// made.
    pub fn sync_stacks(&mut self, inits: &[(bool, bool, Vec<u8>)]) {
        let have = self.stacks.used() - 1;
        for (page, direct, init) in inits.iter().skip(have) {
            let mode = if *direct {
                crate::LiteralMode::DirectAlways
            } else {
                crate::LiteralMode::SetOrigin
            };
            let _ = self.stacks.new_stack(init, mode, *page);
        }
    }

    /// A digest for the caller's memo keys.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut c = self.clone();
        if !c.faked_ever {
            c.used.clear();
        }
        key_of("carry", format!("{c:?}").as_bytes())
    }
}

/// The carry after page `page` (the writer's effect on the colour
/// stacks, the link stack and the two switches, without writing):
/// `wh` gives the page's whatsits as the writer sees them.
#[must_use]
pub fn carry_after(prev: &Carry, page: &BoxNode, wh: &mut WhatsitSource<'_>) -> Carry {
    let mut c = prev.clone();
    c.stacks.ship_begin(true);
    walk(&mut c, page, 0, wh);
    c
}

fn walk(c: &mut Carry, b: &BoxNode, level: i32, wh: &mut WhatsitSource<'_>) {
    for n in &b.list {
        match n {
            Node::Box(x) => walk(c, x, level + 1, wh),
            Node::Whatsit(h, _) => {
                let Some(w) = wh(*h, false) else { continue };
                match w {
                    Whatsit::ColorStack(i, cmd) => {
                        if i < c.stacks.used() {
                            match cmd {
                                ColorStackCmd::Set(d) => {
                                    c.stacks.set(i, &d);
                                }
                                ColorStackCmd::Push(d) => {
                                    c.stacks.push(i, &d);
                                }
                                ColorStackCmd::Pop => {
                                    c.stacks.pop(i);
                                }
                                ColorStackCmd::Current => {
                                    c.stacks.current(i);
                                }
                            }
                        }
                    }
                    Whatsit::StartLink {
                        width,
                        height,
                        depth,
                        attr,
                        action,
                        ..
                    } => c
                        .links
                        .push(LinkLevel::new(level, [width, height, depth], attr, action)),
                    Whatsit::EndLink => {
                        c.links.pop();
                    }
                    Whatsit::InterwordSpaceOn => {
                        c.faked_space = true;
                        c.faked_ever = true;
                    }
                    Whatsit::InterwordSpaceOff => c.faked_space = false,
                    Whatsit::RunningLinkOff => c.running_link_off = true,
                    Whatsit::RunningLinkOn => c.running_link_off = false,
                    _ => {}
                }
            }
            Node::Char { font, .. } => {
                c.used.insert(*font);
            }
            Node::Lig(l) => {
                c.used.insert(l.font);
            }
            Node::Glue(g) => {
                if let Some(l) = &g.leader
                    && let Node::Box(x) = &**l
                {
                    walk(c, x, level + 1, wh);
                }
            }
            _ => {}
        }
    }
}

/// A fresh writer for one piece: the header not written, objects not
/// in object streams.
/// A page's parameters with the ones pdfTeX fixes when the file is
/// opened (`check_pdfversion` and `fix_pdfoutput`, pdfTeX §683) taken
/// from the first page's: the PDF version, the object compression
/// level, the decimal digits, draft mode, the image and font inclusion
/// switches.
#[must_use]
pub fn fixed_params(first: &PdfParams, p: &PdfParams) -> PdfParams {
    let mut q = p.clone();
    q.major_version = first.major_version;
    q.minor_version = first.minor_version;
    q.obj_compress_level = first.obj_compress_level;
    q.decimal_digits = first.decimal_digits;
    q.draftmode = first.draftmode;
    q.image_hicolor = first.image_hicolor;
    q.inclusion_copy_fonts = first.inclusion_copy_fonts;
    q
}

fn node_state(params: &PdfParams) -> PdfState {
    let mut st = PdfState::new(params.clone());
    st.check_pdfversion();
    st.out.clear();
    st.os.enable = false;
    st.fixed_ocl = 0;
    st
}

/// The local objects a piece's writer imported, by handle.
#[derive(Default)]
struct Imports {
    by_handle: HashMap<usize, usize>,
    local: HashMap<usize, usize>,
}

impl Imports {
    fn bind(&mut self, local: usize, h: usize) {
        self.by_handle.insert(h, local);
        self.local.insert(local, h);
    }
}

impl PdfState {
    /// A form made elsewhere, as a written object of this writer.
    fn import_form(&mut self, h: usize, x: &crate::objects::XForm, im: &mut Imports) -> usize {
        if let Some(&k) = im.by_handle.get(&h) {
            return k;
        }
        let k = self.create_obj(OBJ_TYPE_XFORM, form_name(h));
        self.objs[k].offset = 0;
        let mut x = x.clone();
        x.boxed = None;
        self.aux.xforms.insert(k, x);
        im.bind(k, h);
        k
    }

    /// An image made elsewhere.
    fn import_image(&mut self, h: usize, x: &crate::image::XImage, im: &mut Imports) -> usize {
        if let Some(&k) = im.by_handle.get(&h) {
            return k;
        }
        let k = self.create_obj(OBJ_TYPE_XIMAGE, form_name(h));
        self.objs[k].offset = 0;
        self.aux.ximages.insert(k, x.clone());
        im.bind(k, h);
        k
    }

    /// The group object of transparent PNGs (pdfTeX writes one for the
    /// document, with the first such image): a local object for its
    /// handle, never written by a piece; the finish writes it.
    fn png_group_here(&mut self, im: &mut Imports) -> usize {
        if self.png_group == 0 {
            let g = self.new_objnum_();
            self.png_group = g;
            self.png_group_written = true;
            im.bind(g, png_group_handle());
        }
        self.png_group
    }

    /// An object made elsewhere (`\pdfrefobj`).
    fn import_obj(&mut self, h: usize, im: &mut Imports) -> usize {
        if let Some(&k) = im.by_handle.get(&h) {
            return k;
        }
        let k = self.create_obj(OBJ_TYPE_OBJ, 0);
        self.objs[k].offset = 0;
        im.bind(k, h);
        k
    }

    /// An annotation's or link's object number given when TeX read it.
    fn import_mark(&mut self, h: usize, im: &mut Imports) -> usize {
        if let Some(&k) = im.by_handle.get(&h) {
            return k;
        }
        let k = self.new_objnum_();
        im.bind(k, h);
        k
    }
}

/// `/Fm` and `/Im` names from a handle.
fn form_name(h: usize) -> i64 {
    i64::try_from(h % (1 << 20)).unwrap_or(0) + 1
}

/// Why the writer's whatsit cannot be written in relaxed mode.
fn unsupported(w: &Whatsit) -> Option<&'static str> {
    match w {
        Whatsit::Thread { .. } | Whatsit::EndThread => Some("threads and beads"),
        Whatsit::Dest {
            struct_num: Some(_),
            ..
        } => Some("structure destinations"),
        _ => None,
    }
}

/// The whatsits of a list and the boxes in it (leaders included), as
/// the writer will meet them.
fn collect(b: &BoxNode, wh: &mut WhatsitSource<'_>, out: &mut BTreeMap<u32, Whatsit>) {
    for n in &b.list {
        match n {
            Node::Box(x) => collect(x, wh, out),
            Node::Whatsit(h, _) => {
                if !out.contains_key(h)
                    && let Some(w) = wh(*h, false)
                {
                    out.insert(*h, w);
                }
            }
            Node::Glue(g) => {
                if let Some(l) = &g.leader
                    && let Node::Box(x) = &**l
                {
                    collect(x, wh, out);
                }
            }
            _ => {}
        }
    }
}

/// A whatsit with its handles replaced by this writer's local objects
/// (imported as written ones).
fn translate(
    st: &mut PdfState,
    w: Whatsit,
    pieces: &dyn Fn(usize) -> Option<Arc<Piece>>,
    im: &mut Imports,
    fallback: &mut Option<String>,
) -> Whatsit {
    if let Some(why) = unsupported(&w) {
        fallback.get_or_insert_with(|| why.to_owned());
    }
    let piece = |h: usize, fallback: &mut Option<String>| -> Option<Arc<Piece>> {
        let p = pieces(h);
        match &p {
            None => {
                fallback.get_or_insert_with(|| format!("object {h} not made in relaxed mode"));
            }
            Some(p) => {
                if let Some(f) = &p.fallback {
                    fallback.get_or_insert_with(|| f.clone());
                }
            }
        }
        p
    };
    match w {
        Whatsit::RefXForm(h) if h >= HANDLE_BASE => {
            let p = piece(h, fallback);
            if let Some(ph) = p.as_ref().and_then(|p| p.height)
                && ph != st.cur_page_height
            {
                fallback
                    .get_or_insert_with(|| "a form written under another page height".to_owned());
            }
            match p.and_then(|p| p.made.clone()) {
                Some((_, Made::Form(x))) => Whatsit::RefXForm(st.import_form(h, &x, im)),
                _ => Whatsit::RefXForm(0),
            }
        }
        Whatsit::RefXImage(h) if h >= HANDLE_BASE => {
            match piece(h, fallback).and_then(|p| p.made.clone()) {
                Some((_, Made::Image(mut x))) => {
                    if x.group_ref < 0 {
                        fallback.get_or_insert_with(|| {
                            "an included PDF page with a transparency group".to_owned()
                        });
                    } else if x.group_ref > 0 {
                        // a PNG's: the document's one group object
                        x.group_ref = i32::try_from(st.png_group_here(im)).unwrap_or(0);
                    }
                    Whatsit::RefXImage(st.import_image(h, &x, im))
                }
                _ => Whatsit::RefXImage(0),
            }
        }
        Whatsit::RefObj(h) if h >= HANDLE_BASE => Whatsit::RefObj(st.import_obj(h, im)),
        Whatsit::Annot {
            objnum,
            width,
            height,
            depth,
            data,
        } if objnum >= HANDLE_BASE => Whatsit::Annot {
            objnum: st.import_mark(objnum, im),
            width,
            height,
            depth,
            data,
        },
        Whatsit::StartLink {
            objnum,
            width,
            height,
            depth,
            attr,
            action,
        } if objnum >= HANDLE_BASE => {
            if matches!(action, Action::Thread { .. }) {
                fallback.get_or_insert_with(|| "thread actions".to_owned());
            }
            Whatsit::StartLink {
                objnum: st.import_mark(objnum, im),
                width,
                height,
                depth,
                attr,
                action,
            }
        }
        w => w,
    }
}

/// What page `n`'s writer is given besides the page.
pub struct PageEnv<'a> {
    /// The pieces by handle.
    pub pieces: &'a dyn Fn(usize) -> Option<Arc<Piece>>,
    /// `\pdfmapline` (false) and `\pdfmapfile` (true) texts so far.
    pub maps: &'a [(bool, Vec<u8>)],
    /// The carry at the page's start.
    pub carry: &'a Carry,
}

/// Page `n` written on its own.
///
/// # Errors
/// What the writer stops at, or a page past the handles' room.
pub fn page(
    params: &PdfParams,
    n: usize,
    page: &BoxNode,
    fonts: &dyn FontSource,
    wh: &mut WhatsitSource<'_>,
    env: &PageEnv<'_>,
) -> Result<Piece, PdfError> {
    let mut st = node_state(params);
    for (file, t) in env.maps {
        if *file {
            st.map_file(t, fonts);
        } else {
            st.map_line(t);
        }
    }
    let c = env.carry;
    st.stacks = c.stacks.clone();
    st.link_stack.clone_from(&c.links);
    st.faked_space = c.faked_space;
    if c.faked_ever || c.faked_space {
        // `font_used` is document-wide: the fonts earlier pages used are
        // initialized (their objects are written at the finish anyway)
        for &f in &c.used {
            st.load_font(f, fonts);
            if !st.font(f).used {
                st.init_font(f, fonts);
            }
        }
    }
    st.running_link = !c.running_link_off;
    st.total_pages = i64::try_from(n - 1).unwrap_or(0);
    let mut leaves = Vec::new();
    if !(n - 1).is_multiple_of(KIDS) {
        st.last_pages = st.create_obj(OBJ_TYPE_PAGES, PAGES_TREE_KIDS_MAX);
        leaves.push(st.last_pages);
    }
    let mut im = Imports::default();
    let mut fallback = None;
    // the open links go on here with new objects
    for l in &mut st.link_stack {
        l.objnum = 0;
    }
    // `cur_page_height` as `pdf_ship_out` sets it: the forms
    // the page references must have been written under it
    let ph = if params.page_height != 0 {
        params.page_height
    } else {
        page.height + page.depth + 2 * (params.v_origin + params.v_offset)
    };
    st.cur_page_height = ph;
    let mut ws = BTreeMap::new();
    collect(page, wh, &mut ws);
    let ws: BTreeMap<u32, Whatsit> = ws
        .into_iter()
        .map(|(h, w)| (h, translate(&mut st, w, env.pieces, &mut im, &mut fallback)))
        .collect();
    st.ship_out_with(page, fonts, &mut |h, _| ws.get(&h).cloned())?;
    if st.total_pages % PAGES_TREE_KIDS_MAX == 1 {
        leaves.push(st.last_pages);
    }
    let owner = format!("page {n}");
    let mut next = 1;
    let mut other = |_: usize| -> Result<usize, PdfError> {
        let h = page_handle(n, next)?;
        next += 1;
        Ok(h)
    };
    let leaf = leaf_handle((n - 1) / KIDS);
    let mut p = piece(
        &st,
        owner,
        &im,
        &leaves.iter().map(|&k| (k, leaf)).collect(),
        &mut other,
        None,
        fallback,
    )?;
    p.height = Some(ph);
    Ok(p)
}

/// The key a font's PDF font is shared by (`init_font`'s test: a
/// scalable font with the same map entry and name).
fn font_key(f: FontId, s: &FontState) -> u64 {
    if s.scalable() {
        key_of("font", format!("{:?}|{:?}", s.name, s.map).as_bytes())
    } else {
        key_of("pkfont", format!("{f}|{}", s.metrics.size).as_bytes())
    }
}

/// A piece from a writer that wrote it: local objects mapped to
/// handles, and what the finish merges.
fn piece(
    st: &PdfState,
    owner: String,
    im: &Imports,
    fixed: &HashMap<usize, usize>,
    other: &mut dyn FnMut(usize) -> Result<usize, PdfError>,
    made: Option<(usize, Made)>,
    mut fallback: Option<String>,
) -> Result<Piece, PdfError> {
    let mut map: HashMap<usize, usize> = fixed.clone();
    for (&k, &h) in &im.local {
        map.insert(k, h);
    }
    let mut dest_refs = BTreeSet::new();
    for ((t, id), &k) in st.index.iter() {
        let h = match *t {
            OBJ_TYPE_PAGE => match id {
                Ident::Num(m) => page_handle(usize::try_from(*m).unwrap_or(0), 0)?,
                Ident::Name(_) => continue,
            },
            OBJ_TYPE_DEST => {
                if !st.aux.dests.contains_key(&k) {
                    dest_refs.insert((id.clone(), dest_handle(id)));
                }
                dest_handle(id)
            }
            OBJ_TYPE_THREAD => {
                fallback.get_or_insert_with(|| "threads".to_owned());
                continue;
            }
            _ => {
                fallback.get_or_insert_with(|| "structure destinations".to_owned());
                continue;
            }
        };
        map.entry(k).or_insert(h);
    }
    let mut fonts = Vec::new();
    for (&f, s) in st.fonts.iter() {
        if !s.used {
            continue;
        }
        if s.vf.is_some() {
            // a virtual font has no objects: its packets set its local
            // fonts, which are the piece's own fonts
            continue;
        }
        if s.expand.is_some() {
            fallback.get_or_insert_with(|| "expanded fonts".to_owned());
        }
        let rep = st.ff(f);
        let key = font_key(rep, st.font(rep));
        if s.num > 0 {
            map.insert(usize::try_from(s.num).unwrap_or(0), font_handle(key));
        }
        fonts.push((
            f,
            FontUse {
                key,
                state: s.clone(),
            },
        ));
    }
    if !st.epdf_fds.is_empty() {
        fallback.get_or_insert_with(|| "fonts of included PDF files replaced".to_owned());
    }
    if made.is_none() && st.page_group_val != 0 {
        // a group made by the page itself is the page's object
    }
    for k in 1..=st.sys_obj_ptr() {
        if let std::collections::hash_map::Entry::Vacant(e) = map.entry(k) {
            e.insert(other(k)?);
        }
    }
    let dest_names = st
        .dest_names
        .iter()
        .filter(|(_, k)| st.aux.dests.contains_key(k))
        .map(|(s, k)| (s.clone(), map[k]))
        .collect();
    let (bytes, defs) = rewrite(&st.out, |n| {
        (n < HANDLE_BASE).then(|| map.get(&n).copied()).flatten()
    });
    let made = made.map(|(k, m)| (map.get(&k).copied().unwrap_or(k), m));
    Ok(Piece {
        bytes,
        defs,
        owner,
        fonts,
        dest_names,
        dest_refs: dest_refs
            .into_iter()
            .map(|(id, h)| (ident_bytes(&id), h))
            .collect(),
        made,
        fallback,
        height: None,
    })
}

/// `bytes` with every `n 0 R` and `n 0 obj` whose `n` `map` gives
/// replaced, stream data skipped; the objects defined (handle or number
/// after the replacement, and offset).
fn rewrite(
    bytes: &[u8],
    mut map: impl FnMut(usize) -> Option<usize>,
) -> (Vec<u8>, Vec<(usize, usize)>) {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 8);
    let mut defs = Vec::new();
    let mut length: Option<usize> = None;
    let mut i = 0;
    let delim =
        |c: u8| !(c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'+' || c == b'_');
    while i < bytes.len() {
        let c = bytes[i];
        // a stream's data, by the length its dictionary gave
        if c == b's' && bytes[i..].starts_with(b"stream\n") && (i == 0 || bytes[i - 1] == b'\n') {
            let start = i + 7;
            let end = length.map_or(start, |l| (start + l).min(bytes.len()));
            out.extend_from_slice(&bytes[i..end]);
            i = end;
            length = None;
            continue;
        }
        if c == b'/' && bytes[i..].starts_with(b"/Length ") {
            let s = i + 8;
            let mut e = s;
            while e < bytes.len() && bytes[e].is_ascii_digit() {
                e += 1;
            }
            length = std::str::from_utf8(&bytes[s..e])
                .ok()
                .and_then(|t| t.parse().ok());
        }
        if c.is_ascii_digit() && (i == 0 || delim(bytes[i - 1])) {
            let mut e = i;
            while e < bytes.len() && bytes[e].is_ascii_digit() {
                e += 1;
            }
            let rest = &bytes[e..];
            let r = rest.starts_with(b" 0 R") && rest.get(4).is_none_or(|&d| delim(d));
            let o = rest.starts_with(b" 0 obj") && rest.get(6).is_none_or(|&d| delim(d));
            if r || o {
                let n: Option<usize> = std::str::from_utf8(&bytes[i..e])
                    .ok()
                    .and_then(|t| t.parse().ok());
                let m = n.and_then(&mut map);
                let num = m.or(n).unwrap_or(0);
                if o && (i == 0 || bytes[i - 1] == b'\n') {
                    defs.push((num, out.len()));
                }
                match m {
                    Some(m) => out.extend_from_slice(m.to_string().as_bytes()),
                    None => out.extend_from_slice(&bytes[i..e]),
                }
                i = e;
                continue;
            }
            out.extend_from_slice(&bytes[i..e]);
            i = e;
            continue;
        }
        out.push(c);
        i += 1;
    }
    (out, defs)
}

/// The owner key of a read-time command: the reading node's identity
/// and the command's occurrence in it.
#[must_use]
pub fn owner_key(node: u64, occurrence: usize) -> u64 {
    key_of("owner", format!("{node}|{occurrence}").as_bytes())
}

/// `\pdfobj` (not `reserveobjnum`) made on its own: `given` is the
/// handle `useobjnum` gave, else the owner's first.
///
/// # Errors
/// What the writer stops at.
pub fn obj(
    params: &PdfParams,
    key: u64,
    given: Option<usize>,
    stream: bool,
    attr: Option<Vec<u8>>,
    body: ObjBody,
) -> Result<Piece, PdfError> {
    let mut st = node_state(params);
    let mut fallback = None;
    if stream {
        let text = match &body {
            ObjBody::Text(t) | ObjBody::File(t) => t,
        };
        if rewrite(text, |n| (n >= HANDLE_BASE).then_some(n))
            .1
            .is_empty()
            && has_handle(text)
        {
            fallback = Some("a stream object referring to other objects".to_owned());
        }
    }
    let (k, _) = st.obj(None, stream, attr, body);
    st.pdf_write_obj(k);
    let h = given.unwrap_or_else(|| read_handle(key, 0));
    let mut next = 1;
    let mut other = |_: usize| -> Result<usize, PdfError> {
        let h = read_handle(key, next);
        next += 1;
        Ok(h)
    };
    let fixed = HashMap::from([(k, h)]);
    piece(
        &st,
        format!("object {h} (owner key {key:016x})"),
        &Imports::default(),
        &fixed,
        &mut other,
        Some((k, Made::Obj)),
        fallback,
    )
}

/// Whether a text has `n 0 R` with `n` a handle.
fn has_handle(t: &[u8]) -> bool {
    let mut found = false;
    let _ = rewrite(t, |n| {
        if n >= HANDLE_BASE {
            found = true;
        }
        None
    });
    found
}

/// `\pdfxform` made on its own: the box shipped as a form now, under
/// `cur_page_height` `height` (the form's origin): for an `\immediate`
/// one the last page's, which is what pdfTeX had; else the one the
/// pages that reference it must have (`checked`), as pdfTeX writes it
/// after the first.
///
/// # Errors
/// What the writer stops at.
#[allow(clippy::too_many_arguments)]
pub fn xform(
    params: &PdfParams,
    key: u64,
    attr: Option<Vec<u8>>,
    resources: Option<Vec<u8>>,
    b: &BoxNode,
    fonts: &dyn FontSource,
    wh: &mut WhatsitSource<'_>,
    env: &PageEnv<'_>,
    (height, checked): (Scaled, bool),
) -> Result<Piece, PdfError> {
    let mut st = node_state(params);
    for (file, t) in env.maps {
        if *file {
            st.map_file(t, fonts);
        } else {
            st.map_line(t);
        }
    }
    st.stacks = env.carry.stacks.clone();
    st.cur_page_height = height;
    let mut im = Imports::default();
    let mut fallback = None;
    let mut ws = BTreeMap::new();
    collect(b, wh, &mut ws);
    if ws.values().any(|w| matches!(w, Whatsit::ColorStack(..))) {
        fallback = Some("a form with colour stack commands".to_owned());
    }
    let ws: BTreeMap<u32, Whatsit> = ws
        .into_iter()
        .map(|(h, w)| (h, translate(&mut st, w, env.pieces, &mut im, &mut fallback)))
        .collect();
    let k = st.xform(attr, resources, b.clone());
    let made = st.aux.xforms.get(&k).cloned();
    st.objs[k].info = form_name(read_handle(key, 0));
    st.write_xform(k, fonts, &mut |h, _| ws.get(&h).cloned())?;
    let made = made.map(|x| (k, Made::Form(x)));
    let mut next = 1;
    let mut other = |_: usize| -> Result<usize, PdfError> {
        let h = read_handle(key, next);
        next += 1;
        Ok(h)
    };
    let fixed = HashMap::from([(k, read_handle(key, 0))]);
    let mut p = piece(
        &st,
        format!("form {} (owner key {key:016x})", read_handle(key, 0)),
        &im,
        &fixed,
        &mut other,
        made,
        fallback,
    )?;
    p.height = checked.then_some(height);
    Ok(p)
}

/// `\pdfximage` made on its own: the image read and written now.
///
/// # Errors
/// What the writer stops at.
pub fn ximage(
    params: &PdfParams,
    key: u64,
    spec: crate::ImageSpec,
    fonts: &dyn FontSource,
) -> Result<Piece, PdfError> {
    let mut st = node_state(params);
    let mut im = Imports::default();
    st.png_group_here(&mut im);
    let k = st.ximage(spec)?;
    st.objs[k].info = form_name(read_handle(key, 0));
    let made = st.aux.ximages.get(&k).cloned();
    st.write_ximage(k, fonts)?;
    let mut fallback = None;
    if made.as_ref().is_some_and(|x| x.group_ref < 0) {
        fallback = Some("an included PDF page with a transparency group".to_owned());
    }
    let made = made.map(|x| (k, Made::Image(x)));
    let mut next = 1;
    let mut other = |_: usize| -> Result<usize, PdfError> {
        let h = read_handle(key, next);
        next += 1;
        Ok(h)
    };
    let fixed = HashMap::from([(k, read_handle(key, 0))]);
    piece(
        &st,
        format!("image {} (owner key {key:016x})", read_handle(key, 0)),
        &im,
        &fixed,
        &mut other,
        made,
        fallback,
    )
}

/// The document-level commands, in the order TeX read them: replayed
/// by [`finish_relaxed`].
#[derive(Clone, Debug)]
pub enum DocCmd {
    /// `\pdfinfo`.
    Info(Vec<u8>),
    /// `\pdfcatalog` with `openaction`.
    Catalog(Vec<u8>, Option<Action>),
    /// `\pdfnames`.
    Names(Vec<u8>),
    /// `\pdftrailer`.
    Trailer(Vec<u8>),
    /// `\pdftrailerid`.
    TrailerId(Vec<u8>),
    /// `\pdfoutline`.
    Outline(Option<Vec<u8>>, Action, i32, Vec<u8>),
    /// `\pdfglyphtounicode`.
    GlyphToUnicode(Vec<u8>, Vec<u8>),
    /// `\pdffontattr f` (the last one TeX read for the font).
    FontAttr(FontId, Vec<u8>),
    /// `\pdfincludechars f`.
    IncludeChars(FontId, Vec<u8>),
    /// `\pdfnobuiltintounicode f`.
    NoBuiltinToUnicode(FontId),
}

/// `\pdffontobjnum f` made on its own: the handle of the font's PDF
/// font (the finish writes it, with the pages' characters).
///
/// # Errors
/// A virtual font or `\nullfont` (`pdf_check_vf_cur_val`).
pub fn font_objnum(
    params: &PdfParams,
    maps: &[(bool, Vec<u8>)],
    f: FontId,
    fonts: &dyn FontSource,
) -> Result<usize, PdfError> {
    let mut st = node_state(params);
    for (file, t) in maps {
        if *file {
            st.map_file(t, fonts);
        } else {
            st.map_line(t);
        }
    }
    st.font_command(f, fonts)?;
    let rep = st.ff(f);
    Ok(font_handle(font_key(rep, st.font(rep))))
}

/// The file from the pieces: the pages in order, then the objects TeX
/// made when it read commands, then the fonts, the pages tree, the
/// outlines, the names, the catalog and `/Info` the finish writes; the
/// objects numbered in the order they are defined.
///
/// # Errors
/// `Err(reason)` when the document must be written in exact mode
/// instead: a piece that falls back, or two owners of one handle.
#[allow(clippy::too_many_lines)]
pub fn finish_relaxed(
    params: &PdfParams,
    pages: &[Arc<Piece>],
    objs: &[Arc<Piece>],
    cmds: &[DocCmd],
) -> Result<Vec<u8>, String> {
    // the transparent PNGs' group, once, if a piece refers to it
    let g = png_group_handle();
    let group = if pages.iter().chain(objs).any(|p| p.refs_handle(g)) {
        Some(Arc::new(
            png_group_piece(params).map_err(|e| e.to_string())?,
        ))
    } else {
        None
    };
    let all: Vec<&Arc<Piece>> = pages.iter().chain(objs).chain(group.iter()).collect();
    for p in &all {
        if let Some(f) = &p.fallback {
            return Err(format!("{}: {f}", p.owner));
        }
    }
    // a handle defined by two pieces (or twice by one) has two owners
    let mut owners: HashMap<usize, (usize, &str)> = HashMap::new();
    for (i, p) in all.iter().enumerate() {
        for &(h, _) in &p.defs {
            if let Some((_, o)) = owners.insert(h, (i, &p.owner)) {
                return Err(format!("handle {h} has two owners: {o} and {}", p.owner));
            }
        }
    }
    if pages.is_empty() {
        return Ok(Vec::new());
    }
    let mut f = node_state(params);
    let n_pages = pages.len();
    let mut fixed: HashMap<usize, usize> = HashMap::new();
    for n in 1..=n_pages {
        if (n - 1) % KIDS == 0 {
            f.last_pages = f.create_obj(OBJ_TYPE_PAGES, PAGES_TREE_KIDS_MAX);
            fixed.insert(f.last_pages, leaf_handle((n - 1) / KIDS));
        }
        let k = f.get_obj(
            OBJ_TYPE_PAGE,
            &Ident::Num(i32::try_from(n).unwrap_or(i32::MAX)),
        );
        f.aux.pages_made.insert(k);
        f.objs[k].offset = 0;
    }
    f.total_pages = i64::try_from(n_pages).unwrap_or(0);
    // the fonts, one per shared PDF font, with every page's characters
    let mut by_key: BTreeMap<u64, (FontId, FontState)> = BTreeMap::new();
    for p in &all {
        for (id, u) in &p.fonts {
            match by_key.get_mut(&u.key) {
                Some((rep, s)) => {
                    for (b, c) in s.chars.iter_mut().zip(u.state.chars) {
                        *b |= c;
                    }
                    if *id < *rep {
                        let chars = s.chars;
                        *rep = *id;
                        *s = (*u.state).clone();
                        s.chars = chars;
                    }
                }
                None => {
                    by_key.insert(u.key, (*id, (*u.state).clone()));
                }
            }
        }
    }
    // the font commands, on the PDF fonts their fonts share
    let mut key_of_font: HashMap<FontId, u64> = HashMap::new();
    for p in &all {
        for (id, u) in &p.fonts {
            key_of_font.insert(*id, u.key);
        }
    }
    let mut attrs: BTreeMap<FontId, &[u8]> = BTreeMap::new();
    for c in cmds {
        let (DocCmd::FontAttr(id, _)
        | DocCmd::IncludeChars(id, _)
        | DocCmd::NoBuiltinToUnicode(id)) = c
        else {
            continue;
        };
        let Some(key) = key_of_font.get(id) else {
            return Err(format!("a font command on font {id}, which no page shows"));
        };
        let Some((rep, s)) = by_key.get_mut(key) else {
            continue;
        };
        match c {
            DocCmd::FontAttr(_, t) => {
                attrs.insert(*id, t);
            }
            DocCmd::IncludeChars(_, t) => {
                for &ch in t {
                    s.chars[usize::from(ch)] = true;
                }
            }
            DocCmd::NoBuiltinToUnicode(_) if id == rep => s.nobuiltin_tounicode = true,
            _ => {}
        }
    }
    // a shared font's attributes: its first font's, else (`output_fonts`)
    // the first sharer's that has some
    for (key, (rep, s)) in &mut by_key {
        let mine = attrs.get(rep).filter(|a| !a.is_empty());
        let other = || {
            attrs
                .iter()
                .find(|(id, a)| key_of_font.get(id) == Some(key) && !a.is_empty())
                .map(|(_, a)| *a)
        };
        if let Some(a) = mine.copied().or_else(other) {
            s.attr = a.to_vec();
        }
    }
    for (key, (rep, mut s)) in by_key {
        s.used = false;
        s.num = 0;
        f.fonts.insert(rep, Arc::new(s));
        let k = f.create_obj(OBJ_TYPE_FONT, i64::from(rep));
        f.use_font(rep, crate::int(k));
        fixed.insert(k, font_handle(key));
    }
    for c in cmds {
        let r = match c {
            DocCmd::Info(t) => {
                f.info(t);
                Ok(())
            }
            DocCmd::Catalog(t, a) => f.catalog(t, a.as_ref()),
            DocCmd::Names(t) => {
                f.names(t);
                Ok(())
            }
            DocCmd::Trailer(t) => {
                f.trailer(t);
                Ok(())
            }
            DocCmd::TrailerId(t) => {
                f.trailer_id(t);
                Ok(())
            }
            DocCmd::Outline(a, act, n, t) => f.outline(a.clone(), act, *n, t),
            DocCmd::GlyphToUnicode(a, b) => {
                f.glyph_to_unicode(a, b);
                Ok(())
            }
            DocCmd::FontAttr(..) | DocCmd::IncludeChars(..) | DocCmd::NoBuiltinToUnicode(_) => {
                Ok(())
            }
        };
        r.map_err(|e| format!("finish: {e}"))?;
    }
    // destinations referred to and defined nowhere are made here
    let defined: BTreeSet<usize> = all
        .iter()
        .flat_map(|p| p.defs.iter().map(|d| d.0))
        .collect();
    let mut names: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for p in &all {
        for (s, h) in &p.dest_names {
            names.entry(s.clone()).or_insert(*h);
        }
    }
    let mut refs: BTreeMap<usize, Ident> = BTreeMap::new();
    for p in &all {
        for (id, h) in &p.dest_refs {
            refs.insert(*h, parse_ident(id));
        }
    }
    for (_, id) in f
        .index
        .iter()
        .filter(|((t, _), _)| *t == OBJ_TYPE_DEST)
        .map(|((_, id), &k)| (k, id.clone()))
        .collect::<Vec<_>>()
    {
        refs.insert(dest_handle(&id), id);
    }
    for (h, id) in &refs {
        let k = f.get_obj(OBJ_TYPE_DEST, id);
        if defined.contains(h) {
            f.objs[k].offset = 0;
            f.aux.dests.insert(k, crate::marks::Dest::placeholder());
        }
        fixed.insert(k, *h);
    }
    f.dest_names.clear();
    for (s, h) in names {
        f.dest_names.push((s, h));
    }
    for (h, id) in &refs {
        if let Ident::Name(s) = id
            && !f.dest_names.iter().any(|(t, _)| t == s)
        {
            f.dest_names.push((s.clone(), *h));
        }
    }
    let (root, info) = f.tail_objects().map_err(|e| format!("finish: {e}"))?;
    let mut next = 0;
    let mut other = |_: usize| -> Result<usize, PdfError> {
        next += 1;
        Ok(FINISH_BASE + next)
    };
    // the finish's own objects: pages stubs as the pages' handles
    for ((t, id), &k) in f.index.iter() {
        if *t == OBJ_TYPE_PAGE
            && let Ident::Num(m) = id
        {
            fixed.insert(
                k,
                page_handle(usize::try_from(*m).unwrap_or(0), 0).map_err(|e| e.to_string())?,
            );
        }
    }
    let mut map: HashMap<usize, usize> = fixed;
    for k in 1..=f.sys_obj_ptr() {
        if let std::collections::hash_map::Entry::Vacant(e) = map.entry(k) {
            e.insert(other(k).map_err(|e| e.to_string())?);
        }
    }
    let (tail, tail_defs) = rewrite(&f.out, |n| {
        (n < HANDLE_BASE).then(|| map.get(&n).copied()).flatten()
    });
    let root = map.get(&root).copied().unwrap_or(root);
    let info = info.map(|i| map.get(&i).copied().unwrap_or(i));
    // numbers in the order the objects are defined
    let mut number: HashMap<usize, usize> = HashMap::new();
    #[allow(clippy::type_complexity)]
    let chunks: Vec<(&[u8], &[(usize, usize)])> = all
        .iter()
        .map(|p| (p.bytes.as_slice(), p.defs.as_slice()))
        .chain(std::iter::once((tail.as_slice(), tail_defs.as_slice())))
        .collect();
    for (_, defs) in &chunks {
        for &(h, _) in *defs {
            let n = number.len() + 1;
            number.entry(h).or_insert(n);
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(format!("%PDF-{}.{}\n%", f.fixed_major, f.fixed_minor).as_bytes());
    out.extend_from_slice(&[208, 212, 197, 216, b'\n']);
    let mut offsets = vec![0usize; number.len() + 1];
    let mut missing: BTreeSet<usize> = BTreeSet::new();
    for (bytes, _) in &chunks {
        let base = out.len();
        let (b, defs) = rewrite(bytes, |h| {
            if h < HANDLE_BASE {
                return None;
            }
            if let Some(&n) = number.get(&h) {
                Some(n)
            } else {
                missing.insert(h);
                None
            }
        });
        for (n, at) in defs {
            if let Some(o) = offsets.get_mut(n) {
                *o = base + at;
            }
        }
        out.extend_from_slice(&b);
    }
    if let Some(h) = missing.first() {
        return Err(format!("handle {h} referred to and not made"));
    }
    let start = out.len();
    let size = offsets.len();
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for o in &offsets[1..] {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(b"trailer\n<< /Size ");
    out.extend_from_slice(format!("{size}\n/Root {} 0 R\n", number[&root]).as_bytes());
    if let Some(i) = info {
        out.extend_from_slice(format!("/Info {} 0 R\n", number[&i]).as_bytes());
    }
    if let Some(t) = f.trailer_toks.take() {
        out.extend_from_slice(&t);
        out.push(b'\n');
    }
    out.extend_from_slice(&f.id_entry());
    out.extend_from_slice(format!(" >>\nstartxref\n{start}\n%%EOF\n").as_bytes());
    Ok(out)
}

/// An identifier from [`ident_bytes`].
fn parse_ident(b: &[u8]) -> Ident {
    if let Some(n) = b.strip_prefix(b"num ") {
        Ident::Num(
            std::str::from_utf8(n)
                .ok()
                .and_then(|t| t.parse().ok())
                .unwrap_or(0),
        )
    } else {
        Ident::Name(b.strip_prefix(b"name ").unwrap_or(b).to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two owners of one handle: the finish refuses, naming both.
    #[test]
    fn forced_collision() {
        let p = PdfParams::default();
        let body = |t: &str| ObjBody::Text(t.as_bytes().to_vec());
        let a = obj(&p, 7, None, false, None, body("<< /A 1 >>")).unwrap();
        let b = obj(&p, 7, None, false, None, body("<< /B 2 >>")).unwrap();
        let e = finish_relaxed(&p, &[], &[Arc::new(a), Arc::new(b)], &[]).unwrap_err();
        assert!(e.contains("two owners: object "), "{e}");
    }
}
