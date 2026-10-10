//! The PDF file as a fold over the shipped pages and the PDF events
//! between them (pdftex.web parts 32d and 32f: `pdf_ship_out`'s objects,
//! part 53's commands that write or number objects, "Finish the PDF
//! file"): the fold receives the events and the pages in document order —
//! pdfTeX's order of execution — and numbers every object as pdfTeX does
//! while it writes them, so the numbers, the offsets and the
//! cross-reference table come out as pdfTeX's. Every table here (the
//! objects, the fonts used, the font descriptors, the encodings, the
//! forms and images, the outlines, the destinations' names, the threads,
//! the map) is the fold's own, built in order; none is touched by a
//! node. What a command reads back of the file is the fold's answer to
//! its event ([`Written::answers`]).
//!
//! The parts: [`image`] writes the images (PNG, JPEG, a PDF file's page),
//! [`marks`] the annotations, links, destinations, beads, outlines,
//! threads and the destinations' name tree.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::dvi::Numbers;
use crate::page::{Page, PdfEvents};
use crate::tfm::Font;

use super::content::{self, Event as Mark, FormRef, Glyphs, ImageRef, Objs, PageBody, WalkState};
use super::font::enc::NOTDEF;
use super::font::map::{F_INCLUDED, F_PK, F_SUBSETTED, F_TYPE1, Map, MapEntry, Mode};
use super::font::t1::{self, FONT_KEYS, T1Job};
use super::font::unicode::{self, Uni};
use super::num::{Out, divide_scaled};
use super::what::{self, ANSWER_FIELDS, Action, ECHO, Event, Id, Params, Text};
use super::{Fixed, Ship};

mod image;
mod marks;

/// What the end of the job reads (the values reaching it).
#[derive(Clone, Debug, Default)]
pub struct End {
    /// `\pdfcompresslevel` (the font files' streams, the object streams,
    /// the cross-reference stream).
    pub compress_level: i32,
    pub gen_tounicode: i32,
    pub omit_charset: i32,
    pub info_omit_date: i32,
    pub suppress_ptex_info: i32,
    pub ptex_use_underscore: i32,
    /// `\pdfomitinfodict`.
    pub omit_info_dict: i32,
    /// `\pdfpagesattr` as shown (`None`: empty).
    pub pages_attr: Option<Arc<[u8]>>,
    /// The creation date as pdfTeX prints it (`D:19700101000000Z`).
    pub date: String,
    /// The output file's name (`JOB.pdf`): the `/ID`'s.
    pub file_name: String,
    /// `\pdftexbanner`.
    pub banner: String,
    /// The parameters there (the version checked again, `\pdfpagewidth`,
    /// `\pdfpageheight` for a thread fixed).
    pub params: Arc<Params>,
    pub page_width: i32,
    pub page_height: i32,
}

/// The files the fold reads (the map file, font files, encodings): their
/// bytes and the path TeX would show, by name.
pub trait Files {
    fn get(&mut self, name: &str) -> Option<(Arc<str>, Arc<[u8]>)>;
    /// `\pdfglyphtounicode`'s entry of a name reaching the end of the job
    /// (`Key::GlyphUnicode`); the empty name's says whether there is any.
    fn glyph_unicode(&mut self, _name: &str) -> Option<Uni> {
        None
    }
}

/// The file, and what the fold had to say.
#[derive(Default)]
pub struct Written {
    pub bytes: Vec<u8>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Pages whose stream was made again (their guesses did not hold).
    pub again: usize,
    /// The answers to the events (`Key::PdfAns`).
    pub answers: crate::build::PdfAnswers,
}

const OBJ_TYPE_OTHERS: u8 = 0;
const OBJ_TYPE_PAGE: u8 = 1;
const OBJ_TYPE_PAGES: u8 = 2;
const OBJ_TYPE_FONT: u8 = 3;
const OBJ_TYPE_OUTLINE: u8 = 4;
const OBJ_TYPE_DEST: u8 = 5;
const OBJ_TYPE_STRUCT_DEST: u8 = 6;
const OBJ_TYPE_OBJ: u8 = 7;
const OBJ_TYPE_XFORM: u8 = 8;
const OBJ_TYPE_XIMAGE: u8 = 9;
const OBJ_TYPE_THREAD: u8 = 10;
const HEAD_TAB_MAX: usize = 10;
const PAGES_TREE_KIDS_MAX: i64 = 6;
const NAME_TREE_KIDS_MAX: usize = 6;
const PDF_OS_MAX_OBJS: usize = 100;

/// An entry of `obj_tab` (`aux` nonzero: `obj_aux` set — data, a
/// destination shipped, an annotation's node, a page made).
#[derive(Clone, Copy, Debug)]
struct Obj {
    info: i64,
    link: u32,
    /// -2 fresh, -1 scheduled; else the offset in the file (or the number
    /// of the object stream holding it).
    offset: i64,
    os_idx: i32,
    aux: i64,
    /// The list (`head_tab`) it was made in.
    t: u8,
}

/// What an object holds (`obj_data_ptr`'s memory, `pdf_mem`).
#[derive(Clone, Debug)]
enum Data {
    Raw { stream: bool, attr: Option<Text>, file: bool, bytes: Option<Arc<[u8]>>, data: Text },
    Xform { b: Arc<crate::item::BoxNode>, attr: Option<Text>, resources: Option<Text>, index: i64 },
    Ximage { image: Arc<super::image::Image>, w: i32, h: i32, d: i32, attr: Option<Text>, index: i64, group: i32 },
    Outline(Outline),
    Bead(Bead),
    /// A destination shipped: the page's object and where.
    Dest { kind: u8, zoom: Option<i32>, struct_num: Option<i32>, named: bool },
}

/// An outline entry (`obj_outline_*`).
#[derive(Clone, Debug, Default)]
struct Outline {
    action: u32,
    count: i32,
    title: u32,
    prev: u32,
    next: u32,
    first: u32,
    last: u32,
    parent: u32,
    attr: Option<Text>,
}

/// A bead (`obj_bead_*`).
#[derive(Clone, Debug, Default)]
struct Bead {
    attr: Option<Text>,
    page: u32,
    rect: u32,
    next: u32,
    prev: u32,
}

/// A font used (`font_used[f]`, `pdf_font_num[f]`, `pdf_char_used[f]`,
/// `pdf_font_map[f]`).
struct UsedFont {
    font: Arc<Font>,
    /// The object number (positive), or minus the font sharing it.
    num: i64,
    /// `pdf_font_size[f]`.
    size: i32,
    marks: Glyphs,
    map: Option<Arc<MapEntry>>,
}

/// A font descriptor (`fd_entry`).
struct Fd {
    fm: Arc<MapEntry>,
    fontname: Vec<u8>,
    subset_tag: Option<[u8; 6]>,
    objnum: u32,
    /// The font name's object (an included PDF's font refers to it).
    fn_objnum: u32,
    font_dim: [(i32, bool); 11],
    tx_tree: BTreeSet<u8>,
    gl_tree: BTreeSet<Vec<u8>>,
    /// `builtin_glyph_names` (writet1's, of a subsetted font).
    builtin: Option<Vec<Vec<u8>>>,
    /// An included PDF's font embedded whole.
    all_glyphs: bool,
}

/// A font dictionary (`fo_entry`).
struct Fo {
    objnum: u32,
    tex_font: u32,
    fd: (Vec<u8>, i32, i32),
    fe: Option<Vec<u8>>,
    first_char: i32,
    last_char: i32,
    cw_objnum: u32,
}

/// An encoding (`fe_entry`).
struct Fe {
    names: Arc<Vec<Vec<u8>>>,
    objnum: u32,
    tx_tree: BTreeSet<u8>,
}

/// The font map as the map items made it (mapfile.c): the default map
/// file read at the first use unless an item flushed it first, and the
/// entries the items added, replaced or deleted over it.
#[derive(Default)]
struct Maps {
    /// The default map file: pending (`mitem->line`), read.
    pending: bool,
    base: Option<Map>,
    /// The items' entries by TFM name (`None`: deleted).
    over: HashMap<Vec<u8>, Option<Arc<MapEntry>>>,
    /// In the order the items made them (for the PostScript names).
    order: Vec<Arc<MapEntry>>,
    /// Entries looked up for a font (`in_use`): not replaced nor deleted.
    used: HashSet<Vec<u8>>,
}

/// An included PDF file open (`PdfDocument`): images of it read and not
/// written (`occurences`), the objects of it copied.
struct EpdfDoc {
    name: Vec<u8>,
    occurrences: i32,
    objs: Vec<image::InObj>,
}

/// The resource lists of a page or form being shipped.
#[derive(Default, Clone)]
struct Lists {
    fonts: Vec<u32>,
    objs: Vec<u32>,
    xforms: Vec<u32>,
    ximages: Vec<u32>,
    text_procset: bool,
    image_procset: i32,
}

/// The PDF marks of the page being shipped.
#[derive(Default)]
struct MarkLists {
    /// (object, rectangle, text) of each annotation.
    annots: Vec<(u32, content::Rect, Text)>,
    /// (object, the page's link) of each link annotation.
    links: Vec<(u32, usize)>,
    /// (object, rectangle) of each destination.
    dests: Vec<(u32, content::Rect)>,
    /// (bead object, the page's bead).
    beads: Vec<(u32, usize)>,
}

/// The fold's state: pdfTeX's globals of PDF output, the fold's own.
struct F<'a> {
    files: &'a mut dyn Files,
    out: Vec<u8>,
    objs: Vec<Obj>,
    data: HashMap<u32, Data>,
    head_tab: [u32; HEAD_TAB_MAX + 1],
    obj_ptr: u32,
    sys_obj_ptr: u32,
    /// `avl_find_obj`: the objects of each type by identifier.
    ids: HashMap<(u8, Id), u32>,
    /// `dest_names`: the named destinations, in the order made.
    dest_names: Vec<(Text, u32)>,
    obj_count: i64,
    xform_count: i64,
    ximage_count: i64,
    // (object streams)
    os_enable: bool,
    os_mode: bool,
    os_buf: Vec<u8>,
    os_cur_objnum: u32,
    os_objidx: usize,
    os_objnum: Vec<u32>,
    os_objoff: Vec<usize>,
    fixed_objcompresslevel: i32,
    /// `pdf_compress_level` where the fold is.
    compress_level: i32,
    /// `check_pdfversion`'s fixed values (`None` until the first object).
    version: Option<Params>,
    /// `init_pdf_output`'s (the first ship).
    fixed: Option<Fixed>,
    /// The parameters where the fold is (an object written now reads them).
    cur: Arc<Params>,
    /// `cur_page_height` (a form's text begins there too).
    cur_page_height: i32,
    // (pages)
    total_pages: i64,
    last_pages: u32,
    last_page: u32,
    page_group: i32,
    // (fonts)
    fonts: BTreeMap<u32, UsedFont>,
    maps: Maps,
    fd_tree: BTreeMap<(Vec<u8>, i32, i32), Fd>,
    fo_tree: BTreeMap<Vec<u8>, Fo>,
    fe_tree: BTreeMap<Vec<u8>, Fe>,
    tags: BTreeSet<[u8; 6]>,
    last_arg_other_subr3: i32,
    font_attr: HashMap<u32, Text>,
    nobuiltin: HashSet<u32>,
    /// `fixed_gen_tounicode`.
    gen_tounicode: i32,
    // (what the walk carries from page to page, and its marks)
    walk: Arc<WalkState>,
    lists: Lists,
    marks: MarkLists,
    // (outlines, document-level texts)
    first_outline: u32,
    last_outline: u32,
    parent_outline: u32,
    info: Option<Vec<u8>>,
    catalog: Option<Vec<u8>>,
    openaction: u32,
    names: Option<Vec<u8>>,
    trailer: Option<Vec<u8>>,
    trailer_id: Option<Vec<u8>>,
    // (images)
    epdf: Vec<EpdfDoc>,
    png_group: u32,
    png_group_written: bool,
    numbers: &'a Numbers,
    answers: crate::build::PdfAnswers,
    /// `\pdfpagewidth`, `\pdfpageheight` at the end; `\pdfptexuseunderscore`.
    end_page_dims: (i32, i32),
    ptex_use_underscore: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
}

/// The forms and images the fold knows, for a walk.
struct View<'x> {
    objs: &'x [Obj],
    data: &'x HashMap<u32, Data>,
}

impl Objs for View<'_> {
    fn form(&self, n: i32) -> Option<FormRef> {
        let n = u32::try_from(n).ok()?;
        match self.data.get(&n)? {
            Data::Xform { b, index, .. } => Some(FormRef { index: *index, depth: b.d }),
            _ => None,
        }
        .filter(|_| self.objs.get(n as usize).is_some())
    }
    fn image(&self, n: i32) -> Option<ImageRef> {
        let n = u32::try_from(n).ok()?;
        match self.data.get(&n)? {
            Data::Ximage { image, index, .. } => Some(ImageRef { index: *index, image: image.clone() }),
            _ => None,
        }
    }
}

impl F<'_> {
    // ---- the object table (§695–§701)

    /// `pdf_create_obj`.
    fn create_obj(&mut self, t: u8, i: i64) -> u32 {
        self.sys_obj_ptr += 1;
        self.obj_ptr = self.sys_obj_ptr;
        let n = self.obj_ptr;
        let e = Obj { info: i, link: 0, offset: -2, os_idx: 0, aux: 0, t };
        if self.objs.len() <= n as usize {
            self.objs.resize(n as usize + 1, e);
        }
        self.objs[n as usize] = e;
        let tu = usize::from(t);
        if t == OBJ_TYPE_PAGE {
            let mut p = self.head_tab[tu];
            if p == 0 || self.objs[p as usize].info < i {
                self.objs[n as usize].link = p;
                self.head_tab[tu] = n;
            } else {
                let mut q = p;
                while p != 0 {
                    if self.objs[p as usize].info < i {
                        break;
                    }
                    q = p;
                    p = self.objs[p as usize].link;
                }
                self.objs[q as usize].link = n;
                self.objs[n as usize].link = p;
            }
        } else if t != OBJ_TYPE_OTHERS {
            self.objs[n as usize].link = self.head_tab[tu];
            self.head_tab[tu] = n;
        }
        n
    }

    /// `pdf_new_objnum`.
    fn new_objnum(&mut self) -> u32 {
        self.create_obj(OBJ_TYPE_OTHERS, 0)
    }

    fn is_written(&self, k: u32) -> bool {
        self.objs.get(k as usize).is_some_and(|o| o.offset > -1)
    }

    /// `is_obj_scheduled`, `set_obj_scheduled`.
    fn is_scheduled(&self, k: u32) -> bool {
        self.objs.get(k as usize).is_some_and(|o| o.offset > -2)
    }

    fn set_scheduled(&mut self, k: u32) {
        if let Some(o) = self.objs.get_mut(k as usize)
            && o.offset == -2
        {
            o.offset = -1;
        }
    }

    /// `find_obj`.
    fn find_obj(&self, t: u8, id: &Id) -> Option<u32> {
        self.ids.get(&(t, id.clone())).copied()
    }

    /// `get_obj`: the object of type `t` with identifier `id`, made if
    /// there is none (a named destination's name kept for the name tree).
    fn get_obj(&mut self, t: u8, id: &Id) -> u32 {
        if let Some(k) = self.find_obj(t, id) {
            return k;
        }
        let k = match id {
            Id::Num(n) => self.create_obj(t, i64::from(*n)),
            Id::Name(s) => {
                let k = self.create_obj(t, -1);
                if t == OBJ_TYPE_DEST {
                    self.dest_names.push((s.clone(), k));
                }
                k
            }
        };
        self.ids.insert((t, id.clone()), k);
        k
    }

    /// `pdf_check_obj(t, n)`: whether `n` is in the list of type `t`.
    fn check_obj(&self, t: u8, n: i32) -> bool {
        u32::try_from(n).ok().and_then(|n| self.objs.get(n as usize).filter(|_| n > 0)).is_some_and(|o| o.t == t)
    }

    /// An event's answer field.
    fn answer(&mut self, piece: u32, tag: what::Tag, field: u32, v: i32) {
        let [s, f, n, i] = tag;
        self.answers.insert((piece, [s, f, n, i * ANSWER_FIELDS + field]), v);
    }

    // ---- writing (§685, §696–§699)

    fn buf(&mut self) -> &mut Vec<u8> {
        if self.os_mode { &mut self.os_buf } else { &mut self.out }
    }

    fn print_ln(&mut self, s: &[u8]) {
        let b = self.buf();
        b.extend_from_slice(s);
        b.push(b'\n');
    }

    fn print_out(&mut self, o: &Out) {
        self.buf().extend_from_slice(&o.b);
    }

    /// `check_pdfversion` with the parameters where the fold is: the
    /// version and the image parameters fixed, the header, at the first
    /// object; after it, a version changed is an error.
    fn check_pdfversion(&mut self) {
        let p = self.cur.clone();
        if let Some(v) = &self.version {
            if v.major != p.major.max(1) && p.major >= 1 || v.minor != p.minor && (0..=9).contains(&p.minor) {
                self.errors
                    .push("pdfTeX error (setup): PDF version cannot be changed after data is written to the PDF file".into());
            }
            return;
        }
        let mut v = (*p).clone();
        if v.major < 1 {
            self.errors.push("pdfTeX error (invalid pdfmajorversion)".into());
            v.major = 1;
        }
        if !(0..=9).contains(&v.minor) {
            self.errors.push("pdfTeX error (invalid pdfminorversion)".into());
            v.minor = 4;
        }
        v.gamma = v.gamma.clamp(0, 1_000_000);
        v.image_gamma = v.image_gamma.clamp(0, 1_000_000);
        v.image_hicolor = v.image_hicolor.clamp(0, 1);
        v.image_apply_gamma = v.image_apply_gamma.clamp(0, 1);
        v.obj_compress_level = v.obj_compress_level.clamp(0, 3);
        v.draftmode = v.draftmode.clamp(0, 1);
        v.copy_fonts = v.copy_fonts.clamp(0, 1);
        if (v.major > 1 || v.minor >= 5) && v.obj_compress_level > 0 {
            self.os_enable = true;
        } else {
            if v.obj_compress_level > 0 {
                self.warnings.push(
                    "pdfTeX warning (Object streams): \\pdfobjcompresslevel > 0 requires PDF-1.5 or greater. Object streams disabled now."
                        .into(),
                );
                v.obj_compress_level = 0;
            }
            self.os_enable = false;
        }
        self.fixed_objcompresslevel = v.obj_compress_level;
        let mut o = Out::new();
        o.print(b"%PDF-");
        o.int(i64::from(v.major));
        o.out(b'.');
        o.int_ln(i64::from(v.minor));
        o.out(b'%');
        o.print(&[208, 212, 197, 216]);
        o.nl();
        self.out.extend_from_slice(&o.b);
        self.version = Some(v);
    }

    /// The fixed version.
    fn fixed_version(&self) -> (i32, i32) {
        self.version.as_ref().map_or((1, 4), |v| (v.major, v.minor))
    }

    /// `pdf_begin_stream` … `pdf_end_stream` of `raw`, compressed at
    /// `\pdfcompresslevel` where the fold is.
    fn stream_raw(&mut self, raw: &[u8]) {
        let level = self.compress_level;
        if level > 0 {
            let z = super::zlib::compress(raw, level);
            self.stream(&z, level);
        } else {
            self.stream(raw, 0);
        }
    }

    /// `pdf_print` of bytes where the fold writes.
    fn puts(&mut self, s: &[u8]) {
        self.buf().extend_from_slice(s);
    }

    /// `pdf_os_switch`.
    fn os_switch(&mut self, os: bool) {
        self.os_mode = os && self.os_enable;
    }

    /// `pdf_os_prepare_obj`.
    fn os_prepare_obj(&mut self, i: u32, level: i32) {
        self.os_switch(level > 0 && self.fixed_objcompresslevel >= level);
        if self.os_mode {
            if self.os_cur_objnum == 0 {
                self.os_cur_objnum = self.new_objnum();
                self.obj_ptr -= 1;
                self.os_objidx = 0;
                self.os_buf.clear();
                self.os_objnum.clear();
                self.os_objoff.clear();
            } else {
                self.os_objidx += 1;
            }
            let e = &mut self.objs[i as usize];
            e.os_idx = self.os_objidx as i32;
            e.offset = i64::from(self.os_cur_objnum);
            self.os_objnum.push(i);
            self.os_objoff.push(self.os_buf.len());
        } else {
            let at = self.out.len() as i64;
            let e = &mut self.objs[i as usize];
            e.offset = at;
            e.os_idx = -1;
        }
    }

    /// `pdf_begin_obj`.
    fn begin_obj(&mut self, i: u32, level: i32) {
        self.check_pdfversion();
        self.os_prepare_obj(i, level);
        let mut o = Out::new();
        if !self.os_mode {
            o.int(i64::from(i));
            o.print_ln(b" 0 obj");
        } else if self.compress_level == 0 {
            o.print(b"% ");
            o.int(i64::from(i));
            o.print_ln(b" 0 obj");
        }
        self.print_out(&o);
    }

    /// `pdf_end_obj`.
    fn end_obj(&mut self) {
        if self.os_mode {
            if self.os_objidx == PDF_OS_MAX_OBJS - 1 {
                self.os_write_objstream();
            }
        } else {
            self.print_ln(b"endobj");
        }
    }

    /// `pdf_begin_dict`.
    fn begin_dict(&mut self, i: u32, level: i32) {
        self.begin_obj(i, level);
        self.print_ln(b"<<");
    }

    /// `pdf_new_dict`.
    fn new_dict(&mut self, t: u8, i: i64, level: i32) -> u32 {
        let n = self.create_obj(t, i);
        self.begin_dict(n, level);
        n
    }

    /// `pdf_end_dict`.
    fn end_dict(&mut self) {
        if self.os_mode {
            self.print_ln(b">>");
            if self.os_objidx == PDF_OS_MAX_OBJS - 1 {
                self.os_write_objstream();
            }
        } else {
            self.print_ln(b">>");
            self.print_ln(b"endobj");
        }
    }

    /// `pdf_begin_stream`, the stream's bytes (`data`, compressed already
    /// if `level` is positive), `pdf_end_stream`.
    fn stream(&mut self, data: &[u8], level: i32) {
        // (`/Length` and blanks, the length written over them at the end)
        self.out.extend_from_slice(b"/Length           \n");
        let at = self.out.len() - 11;
        if level > 0 {
            self.out.extend_from_slice(b"/Filter /FlateDecode\n");
        }
        self.out.extend_from_slice(b">>\nstream\n");
        self.out.extend_from_slice(data);
        let len = data.len().to_string();
        self.out[at..at + len.len()].copy_from_slice(len.as_bytes());
        self.out.push(b'\n');
        self.out.extend_from_slice(b"endstream\n");
        self.end_obj();
    }

    /// `pdf_os_write_objstream`.
    fn os_write_objstream(&mut self) {
        if self.os_cur_objnum == 0 {
            return;
        }
        let p = self.os_buf.len();
        let mut o = Out::new();
        let mut j = 0;
        for i in 0..=self.os_objidx {
            o.int(i64::from(self.os_objnum[i]));
            o.out(b' ');
            o.int(self.os_objoff[i] as i64);
            if j == 9 {
                o.nl();
                j = 0;
            } else {
                o.out(b' ');
                j += 1;
            }
        }
        if let Some(l) = o.b.last_mut() {
            *l = b'\n';
        }
        let pairs = o.b;
        let q = p + pairs.len();
        let cur = self.os_cur_objnum;
        self.begin_dict(cur, 0);
        let mut o = Out::new();
        o.print_ln(b"/Type /ObjStm");
        o.print(b"/N ");
        o.int_ln(self.os_objidx as i64 + 1);
        o.print(b"/First ");
        o.int_ln((q - p) as i64);
        self.print_out(&o);
        let mut data = pairs;
        data.extend_from_slice(&self.os_buf);
        let level = self.compress_level;
        let z = if level > 0 { super::zlib::compress(&data, level) } else { data };
        self.stream(&z, level);
        self.os_cur_objnum = 0;
    }

    // ---- fonts (§729–§734, writefont.c, mapfile.c)

    // ---- the map (mapfile.c)

    /// `fm_read_info` of the default map file, if it is pending.
    fn read_default_map(&mut self) {
        if !self.maps.pending {
            return;
        }
        self.maps.pending = false;
        match self.files.get("pdftex.map") {
            Some((_, data)) => self.maps.base = Some(Map::new(data)),
            None => self.warnings.push("pdfTeX warning: cannot open font map file".into()),
        }
    }

    /// The map's entry of TFM name `tfm` now.
    fn map_lookup(&self, tfm: &[u8]) -> Option<Arc<MapEntry>> {
        if let Some(e) = self.maps.over.get(tfm) {
            return e.clone();
        }
        self.maps.base.as_ref()?.lookup(tfm)
    }

    /// The map's entry of an included PDF's font named `ps_name`
    /// (`lookup_fontmap`): its subset tag and slant/extend suffixes read,
    /// and only if its font file is there.
    fn map_lookup_ps(&mut self, ps_name: &[u8]) -> Option<Arc<MapEntry>> {
        self.read_default_map();
        let mut s = ps_name;
        if ps_name.len() > 7 && ps_name[..6].iter().all(u8::is_ascii_uppercase) && ps_name[6] == b'+' {
            s = &ps_name[7..];
        }
        let key = slant_extend(s);
        let over = self.maps.order.iter().rev().find(|e| {
            e.ps_key().as_ref() == Some(&key)
                && e.is_t1fontfile()
                && e.is(F_INCLUDED)
                && self.maps.over.get(&e.tfm_name).is_some_and(|x| x.as_ref().is_some_and(|x| Arc::ptr_eq(x, e)))
        });
        let fm = match over {
            Some(e) => e.clone(),
            None => {
                let e = self.maps.base.as_ref()?.lookup_ps(&key)?;
                if self.maps.over.contains_key(&e.tfm_name) {
                    return None;
                }
                e
            }
        };
        let ff = String::from_utf8_lossy(fm.ff_name.as_ref()?).into_owned();
        self.files.get(&ff)?;
        Some(fm)
    }

    /// `process_map_item`: `\pdfmapfile` (`file`) or `\pdfmapline`.
    fn map_item(&mut self, s: &[u8], file: bool) {
        let (mode, flush, item) = super::font::map::map_item(s, file);
        if flush {
            self.maps.pending = false;
        } else {
            self.read_default_map();
        }
        if item.is_empty() {
            return;
        }
        let mut lines = Vec::new();
        if file {
            let name = String::from_utf8_lossy(item).into_owned();
            let found = self.files.get(&name).or_else(|| if name.contains('.') { None } else { self.files.get(&format!("{name}.map")) });
            match found {
                Some((_, data)) => super::font::map::for_map_file_lines(&data, |l| lines.push(l.to_vec())),
                None => {
                    self.warnings.push("pdfTeX warning: cannot open font map file".into());
                    return;
                }
            }
        } else {
            super::font::map::for_map_file_lines(item, |l| lines.push(l.to_vec()));
        }
        for l in lines {
            let mut warn = Vec::new();
            let Some(fm) = super::font::map::scan_line(&l, &mut warn) else {
                self.warnings.extend(warn.iter().map(|w| String::from_utf8_lossy(w).into_owned()));
                continue;
            };
            let fm = Arc::new(fm);
            let tfm = fm.tfm_name.clone();
            if tfm == b"<nontfm>" {
                if mode != Mode::Delete {
                    self.maps.order.push(fm);
                }
                continue;
            }
            let exists = self.map_lookup(&tfm).is_some();
            match mode {
                Mode::DupIgnore if exists => {}
                Mode::Replace | Mode::Delete if exists && self.maps.used.contains(&tfm) => {
                    self.warnings.push(format!(
                        "pdfTeX warning: fontmap entry for `{}' has been used, replace/delete not allowed",
                        String::from_utf8_lossy(&tfm)
                    ));
                }
                Mode::Delete => {
                    self.maps.over.insert(tfm, None);
                }
                _ => {
                    self.maps.order.push(fm.clone());
                    self.maps.over.insert(tfm, Some(fm));
                }
            }
        }
    }

    fn map_entry(&mut self, f: &Font) -> Option<Arc<MapEntry>> {
        self.read_default_map();
        let e = self.map_lookup(f.name.as_bytes());
        if e.is_some() {
            self.maps.used.insert(f.name.as_bytes().to_vec());
        }
        e
    }

    fn is_scalable(u: &UsedFont) -> bool {
        u.map.as_ref().is_some_and(|m| !m.is(F_PK))
    }

    /// `pdf_init_font`: font `f` used for the first time.
    fn init_font(&mut self, f: u32, font: &Arc<Font>) {
        let map = self.map_entry(font);
        let size = divide_scaled(font.size, super::num::ONE_HUNDRED_BP, 6).map_or(0, |r| r.1);
        let mut u = UsedFont { font: font.clone(), num: 0, size, marks: [0; 4], map };
        if Self::is_scalable(&u) {
            let mut i = self.head_tab[usize::from(OBJ_TYPE_FONT)];
            while i != 0 {
                let k = self.objs[i as usize].info as u32;
                if let Some(ku) = self.fonts.get(&k)
                    && Self::is_scalable(ku)
                    && ku.map.as_deref() == u.map.as_deref()
                    && ku.font.name == font.name
                {
                    u.num = if ku.num < 0 { ku.num } else { -i64::from(k) };
                    self.fonts.insert(f, u);
                    return;
                }
                i = self.objs[i as usize].link;
            }
        }
        u.num = i64::from(self.create_obj(OBJ_TYPE_FONT, i64::from(f)));
        self.fonts.insert(f, u);
    }

    /// `set_ff`.
    fn ff(&self, f: u32) -> u32 {
        match self.fonts.get(&f) {
            Some(u) if u.num < 0 => (-u.num) as u32,
            _ => f,
        }
    }

    /// `preset_fontmetrics`.
    fn preset_fontmetrics(u: &UsedFont) -> [(i32, bool); 11] {
        let font = &u.font;
        let size = u.size;
        let dims = |c: u8| font.glyph(i32::from(c)).map_or((0, 0, 0), |g| (g.width, g.height, g.depth));
        let ds = |s: i32| divide_scaled(s, size, 3).map_or(0, |r| r.0);
        let mut d = [(0, true); 11];
        let angle = (-t1::atan(f64::from(font.param(1)) / 65536.0) * (180.0 / std::f64::consts::PI)) as i32;
        d[t1::ITALIC_ANGLE].0 = ds(angle);
        d[t1::ASCENT].0 = ds(dims(b'h').1);
        d[t1::CAPHEIGHT].0 = ds(dims(b'H').1);
        d[t1::DESCENT].0 = (-ds(dims(b'y').2)).min(0);
        d[t1::STEMV].0 = ds(dims(b'.').0 / 3);
        d[t1::XHEIGHT].0 = ds(font.param(5));
        let bb = t1::FONTBBOX1;
        d[bb].0 = 0;
        d[bb + 1].0 = d[t1::DESCENT].0;
        d[bb + 2].0 = ds(font.param(6));
        d[bb + 3].0 = d[t1::CAPHEIGHT].0.max(d[t1::ASCENT].0);
        d[t1::FONTNAME] = (0, false);
        d
    }

    /// `get_fe_entry`: an encoding by name, read at its first use.
    fn fe_names(&mut self, name: &[u8]) -> Option<Arc<Vec<Vec<u8>>>> {
        if let Some(e) = self.fe_tree.get(name) {
            return Some(e.names.clone());
        }
        let n = String::from_utf8_lossy(name).into_owned();
        let Some((_, data)) = self.files.get(&n) else {
            self.warnings.push(format!("cannot open encoding file `{n}'"));
            return None;
        };
        match super::font::enc::parse_enc(&data) {
            Ok(v) => {
                let names = Arc::new(v);
                self.fe_tree.insert(name.to_vec(), Fe { names: names.clone(), objnum: 0, tx_tree: BTreeSet::new() });
                Some(names)
            }
            Err(m) => {
                self.errors.push(String::from_utf8_lossy(&m).into_owned());
                None
            }
        }
    }

    /// `dopdffont` (with `create_fontdictionary`).
    fn do_pdf_font(&mut self, font_objnum: u32, f: u32) {
        let Some(u) = self.fonts.get(&f) else { return };
        let font = u.font.clone();
        let marked = |c: i32| (0..256).contains(&c) && u.marks[(c >> 6) as usize] & (1 << (c & 63)) != 0;
        let first = (font.bc..=font.ec).find(|&c| marked(c));
        let last = (font.bc..=font.ec).rev().find(|&c| marked(c));
        let (Some(first), Some(last)) = (first, last) else { return };
        let codes: Vec<u8> = (first..=last).filter(|&c| marked(c)).map(|c| c as u8).collect();
        let Some(fm) = u.map.clone().filter(|m| !m.is(F_PK)) else {
            self.errors.push(format!("font {}: Type 3 (PK) fonts are not supported yet", font.name));
            return;
        };
        if !fm.is(F_TYPE1) {
            self.errors.push(format!("font {}: TrueType and OpenType fonts are not supported yet", font.name));
            return;
        }
        let Some(ff_name) = fm.ff_name.clone() else {
            self.errors.push(format!("font {}: non-embedded fonts are not supported yet", font.name));
            return;
        };
        let font_dim = Self::preset_fontmetrics(u);
        let size = u.size;
        let mut fe = None;
        if let Some(enc) = fm.encname.clone()
            && let Some(names) = self.fe_names(&enc)
        {
            if self.fe_tree.get(&enc).is_some_and(|e| e.objnum == 0) {
                let o = self.new_objnum();
                if let Some(e) = self.fe_tree.get_mut(&enc) {
                    e.objnum = o;
                }
            }
            if let Some(e) = self.fe_tree.get_mut(&enc) {
                e.tx_tree.extend(codes.iter().copied());
            }
            fe = Some((enc, names));
        }
        let key = (ff_name, fm.slant, fm.extend);
        self.fd_tree.entry(key.clone()).or_insert_with(|| Fd {
            fm: fm.clone(),
            fontname: fm.ps_name.clone().unwrap_or_else(|| fm.tfm_name.clone()),
            subset_tag: None,
            objnum: 0,
            fn_objnum: 0,
            all_glyphs: false,
            font_dim,
            tx_tree: BTreeSet::new(),
            gl_tree: BTreeSet::new(),
            builtin: None,
        });
        // (the /Widths array)
        let cw_objnum = self.new_objnum();
        self.begin_obj(cw_objnum, 1);
        let mut o = Out::new();
        o.print(b"[");
        for c in first..=last {
            let w = divide_scaled(font.glyph(c).map_or(0, |g| g.width), size, 4).map_or(0, |r| r.0);
            o.int(i64::from(w / 10));
            if w % 10 != 0 {
                o.out(b'.');
                o.int(i64::from(w % 10));
            }
            if c != last {
                o.out(b' ');
            }
        }
        o.print_ln(b"]");
        self.print_out(&o);
        self.end_obj();
        let fd = self.fd_tree.get_mut(&key).expect("registered above");
        match &fe {
            Some((_, names)) => {
                if fm.is(F_SUBSETTED) {
                    for &c in &codes {
                        let g = &names[usize::from(c)];
                        if g != NOTDEF {
                            fd.gl_tree.insert(g.clone());
                        }
                    }
                }
            }
            None => fd.tx_tree.extend(codes.iter().copied()),
        }
        self.fo_tree.insert(
            fm.tfm_name.clone(),
            Fo { objnum: font_objnum, tex_font: f, fd: key, fe: fe.map(|(n, _)| n), first_char: first, last_char: last, cw_objnum },
        );
    }

    /// `write_fontname`.
    fn fontname(o: &mut Out, fd: &Fd, key: &[u8]) {
        o.print(b"/");
        o.print(key);
        o.print(b" /");
        Self::fontname_rest(o, fd);
    }

    /// The name, after its slash (`write_fontname` without a key).
    fn fontname_rest(o: &mut Out, fd: &Fd) {
        if let Some(t) = &fd.subset_tag {
            o.print(t);
            o.print(b"+");
        }
        o.print_ln(&fd.fontname);
    }

    /// `write_fontdescriptor` (with `write_fontfile`, `writet1`).
    fn write_fontdescriptor(&mut self, key: &(Vec<u8>, i32, i32), omit_charset: i32) {
        let Some(fd) = self.fd_tree.get(key) else { return };
        let fm = fd.fm.clone();
        if !fm.is(F_INCLUDED) {
            self.errors.push("non-embedded fonts are not supported yet".into());
            return;
        }
        let name = String::from_utf8_lossy(&key.0).into_owned();
        let Some((_, data)) = self.files.get(&name) else {
            self.errors.push(format!("{name}: cannot open Type 1 font file for reading"));
            return;
        };
        let mut job = T1Job {
            data: &data,
            subsetted: fm.is(F_SUBSETTED),
            slant: fm.slant,
            extend: fm.extend,
            glyphs: fd.gl_tree.clone(),
            codes: fd.tx_tree.clone(),
            all_glyphs: fd.all_glyphs,
            fontname: fd.fontname.clone(),
            font_dim: fd.font_dim,
            tags: &mut self.tags,
            last_arg_other_subr3: &mut self.last_arg_other_subr3,
        };
        let result = t1::writet1(&mut job);
        let (glyphs, fontname, font_dim) = (job.glyphs, job.fontname, job.font_dim);
        let t1o = match result {
            Ok(o) => o,
            Err((m, w)) => {
                self.warnings.extend(w.iter().map(|w| String::from_utf8_lossy(w).into_owned()));
                self.errors.push(format!("{name}: {}", String::from_utf8_lossy(&m)));
                return;
            }
        };
        self.warnings.extend(t1o.warnings.iter().map(|w| String::from_utf8_lossy(w).into_owned()));
        {
            let fd = self.fd_tree.get_mut(key).expect("a descriptor");
            fd.gl_tree.clone_from(&glyphs);
            fd.fontname = fontname;
            fd.font_dim = font_dim;
            fd.subset_tag = t1o.subset_tag;
            fd.builtin = t1o.builtin;
        }
        let ff_objnum = self.new_objnum();
        self.begin_dict(ff_objnum, 0);
        let mut o = Out::new();
        o.int_entry_ln(b"Length1", t1o.length1 as i64);
        o.int_entry_ln(b"Length2", t1o.length2 as i64);
        o.int_entry_ln(b"Length3", 0);
        self.print_out(&o);
        let level = self.compress_level;
        let z = if level > 0 { super::zlib::compress(&t1o.bytes, level) } else { t1o.bytes };
        self.stream(&z, level);
        // (the font name's object, an included PDF's fonts refer to)
        let fn_objnum = self.fd_tree[key].fn_objnum;
        if fn_objnum != 0 {
            self.begin_obj(fn_objnum, 1);
            let mut o = Out::new();
            o.print(b"/");
            Self::fontname_rest(&mut o, &self.fd_tree[key]);
            self.print_out(&o);
            self.end_obj();
        }
        let objnum = if self.fd_tree[key].objnum == 0 { self.new_objnum() } else { self.fd_tree[key].objnum };
        let fd = self.fd_tree.get_mut(key).expect("a descriptor");
        fd.objnum = objnum;
        let mut o = Out::new();
        o.print_ln(b"/Type /FontDescriptor");
        let fd = &self.fd_tree[key];
        Self::fontname(&mut o, fd, b"FontName");
        let flags = if fm.fd_flags == -1 { 4 } else { fm.fd_flags };
        o.int_entry_ln(b"Flags", i64::from(flags));
        // (`write_fontmetrics`, with `fix_fontmetrics`)
        let mut p = fd.font_dim;
        let bb = t1::FONTBBOX1;
        if p[bb].1 && p[bb + 1].1 && p[bb + 2].1 && p[bb + 3].1 {
            for (k, from) in [(t1::ASCENT, bb + 3), (t1::DESCENT, bb + 1), (t1::CAPHEIGHT, bb + 3)] {
                if !p[k].1 {
                    p[k] = (p[from].0, true);
                }
            }
            o.print(b"/FontBBox [");
            for i in 0..4 {
                o.int(i64::from(p[bb + i].0));
                o.print(if i < 3 { b" " } else { b"]\n" });
            }
        }
        for (&(v, set), k) in p.iter().zip(FONT_KEYS).take(t1::XHEIGHT + 1) {
            if set {
                o.int_entry_ln(k.0.as_bytes(), i64::from(v));
            }
        }
        if omit_charset == 0 && fm.is(F_SUBSETTED) {
            o.print(b"/CharSet (");
            for g in &glyphs {
                o.print(b"/");
                o.print(g);
            }
            o.print_ln(b")");
        }
        o.indirect_ln(b"FontFile", ff_objnum);
        self.begin_dict(objnum, 1);
        self.print_out(&o);
        self.end_dict();
    }

    /// `write_enc` of encoding `name`.
    fn write_enc(&mut self, name: &[u8]) {
        let fe = &self.fe_tree[name];
        if fe.objnum == 0 {
            return;
        }
        let mut o = Out::new();
        o.print_ln(b"/Type /Encoding");
        o.print(b"/Differences [");
        let mut old = -2;
        for &c in &fe.tx_tree {
            let c = i32::from(c);
            if c != old + 1 {
                if old != -2 {
                    o.out(b' ');
                }
                o.int(i64::from(c));
            }
            o.out(b'/');
            o.print(&fe.names[c as usize]);
            old = c;
        }
        o.print_ln(b"]");
        let n = fe.objnum;
        self.begin_dict(n, 1);
        self.print_out(&o);
        self.end_dict();
    }

    /// `write_fontdictionary`.
    fn write_fontdictionary(&mut self, tfm: &[u8]) {
        // (the ToUnicode map first, `write_tounicode`: a fresh object)
        let mut tounicode = 0;
        let tex_font = self.fo_tree[tfm].tex_font;
        if self.gen_tounicode > 0 && !self.nobuiltin.contains(&tex_font) {
            if self.files.glyph_unicode("").is_none() {
                self.warnings.push("no GlyphToUnicode entry has been inserted yet!".into());
                self.gen_tounicode = 0;
            } else {
                let fo = &self.fo_tree[tfm];
                let names = match &fo.fe {
                    Some(e) => self.fe_tree.get(e).map(|x| (x.names.to_vec(), Some(e.clone()))),
                    None => self.fd_tree[&fo.fd].builtin.clone().map(|n| (n, None)),
                };
                match names {
                    Some((names, enc)) => tounicode = self.write_tounicode(&names, tfm, enc.as_deref()),
                    None => self.errors.push("builtin glyph names is empty".into()),
                }
            }
        }
        let fo = &self.fo_tree[tfm];
        let fd = &self.fd_tree[&fo.fd];
        let mut o = Out::new();
        o.print_ln(b"/Type /Font");
        o.print_ln(b"/Subtype /Type1");
        Self::fontname(&mut o, fd, b"BaseFont");
        o.indirect_ln(b"FontDescriptor", fd.objnum);
        o.int_entry_ln(b"FirstChar", i64::from(fo.first_char));
        o.int_entry_ln(b"LastChar", i64::from(fo.last_char));
        o.indirect_ln(b"Widths", fo.cw_objnum);
        if let Some(e) = &fo.fe
            && let Some(n) = self.fe_tree.get(e).map(|e| e.objnum)
            && n != 0
        {
            o.indirect_ln(b"Encoding", n);
        }
        if tounicode != 0 {
            o.indirect_ln(b"ToUnicode", tounicode);
        }
        if let Some(a) = self.font_attr.get(&tex_font) {
            o.print(a);
            o.nl();
        }
        let n = fo.objnum;
        self.begin_dict(n, 1);
        self.print_out(&o);
        self.end_dict();
    }

    /// `write_tounicode`: the CMap of a font whose codes are glyphs
    /// `names` (its encoding `enc`'s, or the font's own).
    fn write_tounicode(&mut self, names: &[Vec<u8>], tfm: &[u8], enc: Option<&[u8]>) -> u32 {
        let tfm = String::from_utf8_lossy(tfm).into_owned();
        let enc = enc.map(|e| String::from_utf8_lossy(e).into_owned());
        let (buf, w) = unicode::cmap_name(&tfm, enc.as_deref());
        self.warnings.extend(w);
        let files = &mut *self.files;
        let data = unicode::cmap(names, &tfm, &buf, &mut |n| files.glyph_unicode(n));
        let objnum = self.new_objnum();
        self.begin_dict(objnum, 0);
        let level = self.compress_level;
        let z = if level > 0 { super::zlib::compress(&data, level) } else { data };
        self.stream(&z, level);
        objnum
    }

    /// epdf.c's `epdf_create_fontdescriptor`: the descriptor of an
    /// included PDF's font replaced by the map's entry `fm` (made, its
    /// object numbered now, if there is none).
    fn epdf_fontdescriptor(&mut self, fm: &Arc<MapEntry>, stem_v: i32) -> (Vec<u8>, i32, i32) {
        let key = (fm.ff_name.clone().unwrap_or_default(), fm.slant, fm.extend);
        if !self.fd_tree.contains_key(&key) {
            self.maps.used.insert(fm.tfm_name.clone());
            let objnum = self.new_objnum();
            let mut font_dim = [(0, false); 11];
            font_dim[t1::STEMV] = (stem_v, true);
            self.fd_tree.insert(
                key.clone(),
                Fd {
                    fm: fm.clone(),
                    fontname: fm.ps_name.clone().unwrap_or_default(),
                    subset_tag: None,
                    objnum,
                    fn_objnum: 0,
                    font_dim,
                    tx_tree: BTreeSet::new(),
                    gl_tree: BTreeSet::new(),
                    builtin: None,
                    all_glyphs: false,
                },
            );
        }
        key
    }

    /// epdf.c's `epdf_mark_glyphs`: the glyphs of an included PDF's
    /// `/CharSet` marked in descriptor `key`.
    fn epdf_mark_glyphs(&mut self, key: &(Vec<u8>, i32, i32), charset: &[u8]) {
        let space = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c);
        let cs = &charset[..charset.iter().position(|&c| c == 0).unwrap_or(charset.len())];
        let cs = &cs[cs.iter().take_while(|&&c| space(c)).count()..];
        let Some(fd) = self.fd_tree.get_mut(key) else { return };
        let mut s = 1;
        while s < cs.len() {
            let mut p = s;
            while p < cs.len() && cs[p] != b'/' && !space(cs[p]) {
                p += 1;
            }
            let name = &cs[s..p];
            if p < cs.len() && space(cs[p]) {
                p += 1;
                while p < cs.len() && space(cs[p]) {
                    p += 1;
                }
            }
            fd.gl_tree.insert(name.to_vec());
            s = p + 1;
        }
    }

    /// `get_fn_objnum`: the font name's object, numbered at the first ask.
    fn fn_objnum(&mut self, key: &(Vec<u8>, i32, i32)) -> u32 {
        let n = self.fd_tree.get(key).map_or(0, |fd| fd.fn_objnum);
        if n != 0 {
            return n;
        }
        let n = self.new_objnum();
        if let Some(fd) = self.fd_tree.get_mut(key) {
            fd.fn_objnum = n;
        }
        n
    }

    // ---- the events (pdftex.web part 53)

    /// The font number of `font` (TeX's order of loading).
    fn font_num(&self, font: &Font) -> Option<u32> {
        self.numbers.get(&font.key).copied()
    }

    /// The font `f` is used (`pdf_init_font` unless it was).
    fn use_font(&mut self, f: u32, font: &Arc<Font>) {
        if !self.fonts.contains_key(&f) {
            self.init_font(f, font);
        }
    }

    /// The fixed values of the first ship (`init_pdf_output`).
    fn init_output(&mut self, decimal_digits: i32) {
        if self.fixed.is_none() {
            self.fixed = Some(Fixed { decimal_digits: decimal_digits.clamp(0, 4) });
        }
    }

    /// `pdf_new_obj(t, 0, level)`.
    fn new_obj(&mut self, t: u8, level: i32) -> u32 {
        let n = self.create_obj(t, 0);
        self.begin_obj(n, level);
        n
    }

    /// A PDF event of the piece `piece` (an id), done; its answers kept.
    #[allow(clippy::too_many_lines)]
    fn step(&mut self, piece: u32, it: &what::PdfItem) {
        let tag = it.tag;
        match &it.event {
            Event::Obj { reserve, use_num, stream, attr, file, bytes, data, immediate, params } => {
                self.cur = params.clone();
                if *reserve {
                    self.obj_count += 1;
                    let k = self.create_obj(OBJ_TYPE_OBJ, self.obj_count);
                    self.answer(piece, tag, 0, k as i32);
                    return;
                }
                let mut k = None;
                if let Some(n) = *use_num {
                    let ok = n > 0 && n as u32 <= self.obj_ptr && self.objs[n as usize].aux == 0;
                    if ok {
                        k = Some(n as u32);
                    } else {
                        self.warnings.push("pdfTeX warning (\\pdfobj): invalid object number being ignored".into());
                        self.answer(piece, tag, 1, 1);
                    }
                }
                let k = k.unwrap_or_else(|| {
                    self.obj_count += 1;
                    self.create_obj(OBJ_TYPE_OBJ, self.obj_count)
                });
                self.objs[k as usize].aux = 1;
                self.data.insert(
                    k,
                    Data::Raw { stream: *stream, attr: attr.clone(), file: *file, bytes: bytes.clone(), data: data.clone() },
                );
                self.answer(piece, tag, 0, k as i32);
                if *immediate {
                    self.compress_level = params.compress_level;
                    self.write_obj(k);
                }
            }
            Event::Xform { attr, resources, b, immediate, params } => {
                self.cur = params.clone();
                self.xform_count += 1;
                let k = self.create_obj(OBJ_TYPE_XFORM, self.xform_count);
                self.objs[k as usize].aux = 1;
                self.data.insert(
                    k,
                    Data::Xform { b: b.clone(), attr: attr.clone(), resources: resources.clone(), index: self.xform_count },
                );
                self.answer(piece, tag, 0, k as i32);
                if *immediate {
                    self.compress_level = params.compress_level;
                    self.init_output(params.decimal_digits);
                    self.ship_form(k);
                }
            }
            Event::Ximage { image, w, h, d, attr, immediate, params } => {
                self.cur = params.clone();
                self.check_pdfversion();
                self.ximage_count += 1;
                let k = self.create_obj(OBJ_TYPE_XIMAGE, self.ximage_count);
                self.objs[k as usize].aux = 1;
                let mut group = 0;
                if image.png_group {
                    // (writepng.c's `transparent_page_group`: one object, the
                    // page group's if none is set)
                    if self.png_group == 0 {
                        self.png_group = self.new_objnum();
                    }
                    if self.page_group == 0 {
                        self.page_group = self.png_group as i32;
                    }
                    group = self.page_group;
                }
                if let Some(p) = &image.pdf {
                    if p.group {
                        group = -1;
                    }
                    // (`find_add_document`)
                    match self.epdf.iter_mut().find(|e| e.name == image.name) {
                        Some(e) => e.occurrences += 1,
                        None => self.epdf.push(EpdfDoc { name: image.name.clone(), occurrences: 0, objs: Vec::new() }),
                    }
                }
                self.data.insert(
                    k,
                    Data::Ximage {
                        image: image.clone(),
                        w: *w,
                        h: *h,
                        d: *d,
                        attr: attr.clone(),
                        index: self.ximage_count,
                        group,
                    },
                );
                self.answer(piece, tag, 0, k as i32);
                if *immediate {
                    self.compress_level = params.compress_level;
                    self.write_image(k);
                }
            }
            Event::Check { kind, n } => {
                let ok = self.check_obj(*kind, *n);
                self.answer(piece, tag, ECHO, *n);
                self.answer(piece, tag, 0, i32::from(ok));
                if !ok {
                    return;
                }
                let dims = match self.data.get(&(*n as u32)) {
                    Some(Data::Xform { b, index, .. }) => Some((b.w, b.h, b.d, *index)),
                    Some(Data::Ximage { w, h, d, index, .. }) => Some((*w, *h, *d, *index)),
                    _ => None,
                };
                if let Some((w, h, d, i)) = dims {
                    self.answer(piece, tag, 1, w);
                    self.answer(piece, tag, 2, h);
                    self.answer(piece, tag, 3, d);
                    self.answer(piece, tag, 4, i as i32);
                }
            }
            Event::Annot { reserve, use_num } => {
                if let Some(n) = use_num {
                    self.answer(piece, tag, ECHO, *n);
                }
                let k = match use_num {
                    Some(n) if !*reserve => {
                        let ok = *n > 0 && *n as u32 <= self.obj_ptr && self.objs[*n as usize].aux == 0;
                        if !ok {
                            self.answer(piece, tag, 1, 1);
                            return;
                        }
                        *n as u32
                    }
                    _ => self.new_objnum(),
                };
                self.answer(piece, tag, 0, k as i32);
            }
            Event::DestCheck { id, structd } => {
                let t = if *structd { OBJ_TYPE_STRUCT_DEST } else { OBJ_TYPE_DEST };
                let dup = self.find_obj(t, id).is_some_and(|k| self.objs[k as usize].aux != 0);
                self.answer(piece, tag, 0, i32::from(dup));
            }
            Event::Outline { attr, action, count, title, params } => {
                self.cur = params.clone();
                self.compress_level = params.compress_level;
                self.outline(attr.clone(), action, *count, title);
            }
            Event::Catalog { toks, open, params } => {
                self.cur = params.clone();
                self.catalog.get_or_insert_with(Vec::new).extend_from_slice(toks);
                if let Some(a) = open {
                    if self.openaction != 0 {
                        self.errors.push("pdfTeX error (ext1): duplicate of openaction".into());
                    } else {
                        self.compress_level = params.compress_level;
                        let n = self.new_obj(OBJ_TYPE_OTHERS, 1);
                        self.openaction = n;
                        self.write_action(a);
                        self.end_obj();
                    }
                }
            }
            Event::Info(t) => self.info.get_or_insert_with(Vec::new).extend_from_slice(t),
            Event::Names(t) => self.names.get_or_insert_with(Vec::new).extend_from_slice(t),
            Event::Trailer(t) => self.trailer.get_or_insert_with(Vec::new).extend_from_slice(t),
            Event::TrailerId(t) => self.trailer_id.get_or_insert_with(Vec::new).extend_from_slice(t),
            Event::ColorStackInit { page, mode, data } => {
                let w = Arc::make_mut(&mut self.walk);
                let n = w.stacks.new_stack(data, i32::from(*mode), *page).unwrap_or(-1);
                self.answer(piece, tag, 0, n);
            }
            Event::ColorStacks => {
                let w = Arc::make_mut(&mut self.walk);
                let n = w.stacks.used() as i32;
                self.answer(piece, tag, 0, n);
            }
            Event::FontInit(font) => {
                let Some(f) = self.font_num(font) else { return };
                self.use_font(f, font);
                let ff = self.ff(f);
                let num = self.fonts.get(&ff).map_or(0, |u| u.num);
                self.answer(piece, tag, 0, num as i32);
                self.answer(piece, tag, 1, ff as i32);
            }
            Event::IncludeChars(font, chars) => {
                let Some(f) = self.font_num(font) else { return };
                self.use_font(f, font);
                if let Some(u) = self.fonts.get_mut(&f) {
                    for &c in chars {
                        u.marks[usize::from(c >> 6)] |= 1 << (c & 63);
                    }
                }
            }
            Event::FontAttr(font, t) => {
                if let Some(f) = self.font_num(font) {
                    self.font_attr.insert(f, t.clone());
                }
            }
            Event::NoBuiltinToUnicode(font) => {
                if let Some(f) = self.font_num(font) {
                    self.nobuiltin.insert(f);
                }
            }
            Event::MapFile(t) => self.map_item(t, true),
            Event::MapLine(t) => self.map_item(t, false),
            Event::PageRef(n) => {
                let k = self.get_obj(OBJ_TYPE_PAGE, &Id::Num(*n));
                self.answer(piece, tag, 0, k as i32);
            }
            Event::XimageBbox(n, j) => {
                let v = match self.data.get(&(*n as u32)) {
                    Some(Data::Ximage { image, .. }) => image.bbox(*j),
                    _ => 0,
                };
                self.answer(piece, tag, 0, v);
            }
            Event::LastPos => {
                let (x, y) = self.walk.last_pos;
                self.answer(piece, tag, 0, x);
                self.answer(piece, tag, 1, y);
            }
        }
    }

    /// `\pdfoutline`'s objects (its action, the entry numbered, its
    /// title) and the outline's tree as pdfTeX keeps it.
    fn outline(&mut self, attr: Option<Text>, action: &Action, count: i32, title: &[u8]) {
        let j = self.new_obj(OBJ_TYPE_OTHERS, 1);
        self.write_action(action);
        self.end_obj();
        let k = self.create_obj(OBJ_TYPE_OUTLINE, 0);
        self.objs[k as usize].aux = 1;
        let t = self.new_obj(OBJ_TYPE_OTHERS, 1);
        let mut o = Out::new();
        o.str(title);
        o.nl();
        self.print_out(&o);
        self.end_obj();
        let ol = Outline { action: j, count, title: t, parent: self.parent_outline, attr, ..Outline::default() };
        self.data.insert(k, Data::Outline(ol));
        if self.first_outline == 0 {
            self.first_outline = k;
        }
        if self.last_outline == 0 {
            if self.parent_outline != 0 {
                self.ol_mut(self.parent_outline).first = k;
            }
        } else {
            let last = self.last_outline;
            self.ol_mut(last).next = k;
            self.ol_mut(k).prev = last;
        }
        self.last_outline = k;
        if count != 0 {
            self.parent_outline = k;
            self.last_outline = 0;
        } else if self.parent_outline != 0 && self.outline_list_count(k) == self.ol(self.parent_outline).count.abs() {
            let mut j = self.last_outline;
            loop {
                let p = self.parent_outline;
                self.ol_mut(p).last = j;
                j = p;
                self.parent_outline = self.ol(p).parent;
                if self.parent_outline == 0 || self.outline_list_count(j) < self.ol(self.parent_outline).count.abs() {
                    break;
                }
            }
            self.last_outline = if self.parent_outline == 0 { self.first_outline } else { self.ol(self.parent_outline).first };
            while self.ol(self.last_outline).next != 0 {
                self.last_outline = self.ol(self.last_outline).next;
            }
        }
    }

    fn ol(&self, k: u32) -> Outline {
        match self.data.get(&k) {
            Some(Data::Outline(o)) => o.clone(),
            _ => Outline::default(),
        }
    }

    fn ol_mut(&mut self, k: u32) -> &mut Outline {
        let e = self.data.entry(k).or_insert_with(|| Data::Outline(Outline::default()));
        if !matches!(e, Data::Outline(_)) {
            *e = Data::Outline(Outline::default());
        }
        match e {
            Data::Outline(o) => o,
            _ => unreachable!("an outline"),
        }
    }

    /// `outline_list_count`.
    fn outline_list_count(&self, mut p: u32) -> i32 {
        let mut k = 1;
        while self.ol(p).prev != 0 {
            k += 1;
            p = self.ol(p).prev;
        }
        k
    }

    /// `pdf_write_obj`: a raw object.
    fn write_obj(&mut self, n: u32) {
        let Some(Data::Raw { stream, attr, file, bytes, data }) = self.data.get(&n).cloned() else { return };
        if stream {
            self.begin_dict(n, 0);
            if let Some(a) = attr.filter(|a| !a.is_empty()) {
                self.print_ln(&a);
            }
        } else {
            self.begin_obj(n, 1);
        }
        let mut content: Vec<u8> = Vec::new();
        if file {
            let Some(b) = bytes else {
                self.errors.push(format!(
                    "! {} not found.\npdfTeX error (ext5): cannot open file for embedding",
                    String::from_utf8_lossy(&data)
                ));
                return;
            };
            content.extend_from_slice(&b);
            if !stream && content.last().is_some_and(|&c| c != 10) {
                content.push(10);
            }
        } else {
            content.extend_from_slice(&data);
            if !stream {
                content.push(10);
            }
        }
        if stream {
            self.stream_raw(&content);
        } else {
            self.puts(&content);
            self.end_obj();
        }
        if let Some(Data::Raw { data, .. }) = self.data.get_mut(&n) {
            *data = Arc::from(&b""[..]);
        }
    }

    /// The font sharing font `k`'s object, as the fold knows it now, or
    /// the guess `g` (a font not used yet: an earlier font of its name).
    fn ff_truth(&self, k: u32, name: Option<&str>, g: u32) -> u32 {
        if self.fonts.contains_key(&k) {
            return self.ff(k);
        }
        let Some(name) = name else { return g };
        self.fonts.iter().find(|(_, u)| u.font.name == name && u.num > 0).map_or(g, |(&n, _)| n)
    }

    /// A walk's marks and references done (pdfTeX does them as the walk
    /// meets them: `pdf_init_font`, `do_annot`, `do_link`, `append_link`,
    /// `do_dest`, `append_bead`, `\pdfrefobj`, `out_form`, `out_image`).
    fn walk_events(&mut self, body: &PageBody) {
        for e in &body.events {
            match e {
                Mark::Font(f) => {
                    if let Some(u) = body.fonts.iter().find(|u| u.num == *f) {
                        let font = u.font.clone();
                        self.use_font(*f, &font);
                    }
                    let k = self.ff(*f);
                    if !self.lists.fonts.iter().any(|&g| self.ff(g) == k) {
                        self.lists.fonts.push(*f);
                    }
                }
                Mark::Annot { objnum, rect, data } => {
                    let mut k = *objnum as u32;
                    if k == 0 || k > self.obj_ptr || self.is_scheduled(k) {
                        k = self.new_objnum();
                    }
                    self.objs[k as usize].aux = 1;
                    self.marks.annots.push((k, *rect, data.clone()));
                    self.set_scheduled(k);
                }
                Mark::Link { objnum, link } => {
                    let mut k = *objnum as u32;
                    if k == 0 || k > self.obj_ptr || self.is_scheduled(k) {
                        k = self.new_objnum();
                    }
                    self.objs[k as usize].aux = 1;
                    self.marks.links.push((k, *link));
                    self.set_scheduled(k);
                }
                Mark::AppendLink { link } => {
                    let k = self.create_obj(OBJ_TYPE_OTHERS, 0);
                    self.objs[k as usize].aux = 1;
                    self.marks.links.push((k, *link));
                }
                Mark::Dest { id, struct_num, kind, zoom, rect } => {
                    let t = if struct_num.is_some() { OBJ_TYPE_STRUCT_DEST } else { OBJ_TYPE_DEST };
                    let k = self.get_obj(t, id);
                    if self.objs[k as usize].aux != 0 {
                        if self.cur.suppress_dup_dest <= 0 {
                            let shown = match id {
                                Id::Num(n) => format!("num{n}"),
                                Id::Name(s) => format!("name{{{}}}", String::from_utf8_lossy(s)),
                            };
                            self.warnings.push(format!(
                                "pdfTeX warning (ext4): destination with the same identifier ({shown}) has been already used, duplicate ignored"
                            ));
                        }
                        continue;
                    }
                    self.objs[k as usize].aux = 1;
                    let named = matches!(id, Id::Name(_));
                    self.data.insert(k, Data::Dest { kind: *kind, zoom: *zoom, struct_num: *struct_num, named });
                    self.marks.dests.push((k, *rect));
                }
                Mark::Bead { id, bead, attr } => {
                    let t = self.get_obj(OBJ_TYPE_THREAD, id);
                    let b = self.new_objnum();
                    let mut bd = Bead { attr: attr.clone().filter(|a| !a.is_empty()), page: self.last_page, ..Bead::default() };
                    let first = self.objs[t as usize].aux as u32;
                    if first == 0 {
                        self.objs[t as usize].aux = i64::from(b);
                        bd.next = b;
                        bd.prev = b;
                    } else {
                        let a = first;
                        let c = self.bead(a).prev;
                        bd.prev = c;
                        bd.next = a;
                        self.bead_mut(a).prev = b;
                        self.bead_mut(c).next = b;
                    }
                    self.data.insert(b, Data::Bead(bd));
                    self.marks.beads.push((b, *bead));
                }
                Mark::RefObj(n) => self.lists.objs.push(*n as u32),
                Mark::Form(n) => {
                    let n = *n as u32;
                    if !self.lists.xforms.contains(&n) {
                        self.lists.xforms.push(n);
                    }
                }
                Mark::Image(n) => {
                    let n = *n as u32;
                    self.image_group(n);
                    if !self.lists.ximages.contains(&n) {
                        self.lists.ximages.push(n);
                    }
                }
            }
        }
        for u in &body.fonts {
            if let Some(x) = self.fonts.get_mut(&u.num) {
                for (a, b) in x.marks.iter_mut().zip(u.glyphs) {
                    *a |= b;
                }
            }
        }
        self.warnings.extend(body.warnings.iter().cloned());
        self.errors.extend(body.errors.iter().cloned());
    }

    fn bead(&self, k: u32) -> Bead {
        match self.data.get(&k) {
            Some(Data::Bead(b)) => b.clone(),
            _ => Bead::default(),
        }
    }

    fn bead_mut(&mut self, k: u32) -> &mut Bead {
        let e = self.data.entry(k).or_insert_with(|| Data::Bead(Bead::default()));
        if !matches!(e, Data::Bead(_)) {
            *e = Data::Bead(Bead::default());
        }
        match e {
            Data::Bead(b) => b,
            _ => unreachable!("a bead"),
        }
    }

    /// `out_image`'s page group: a PNG's transparency group, a PDF page's
    /// group (numbered at the image's first page), the page's if it has
    /// none yet.
    fn image_group(&mut self, n: u32) {
        let Some(Data::Ximage { image, group, .. }) = self.data.get(&n) else { return };
        let (png, pdf, group) = (image.png, image.pdf.is_some(), *group);
        if !pdf {
            if png && group > 0 && self.page_group == 0 {
                self.page_group = group;
            }
            return;
        }
        if group == 0 || self.page_group != 0 {
            return;
        }
        if group == -1 {
            let g = self.new_objnum() as i32;
            self.page_group = g;
            if let Some(Data::Ximage { group, .. }) = self.data.get_mut(&n) {
                *group = g;
            }
        } else {
            self.page_group = group;
        }
    }

    /// "Write out resource lists": the raw objects, images and forms the
    /// page or form refers to and not written yet (forms last: shipping
    /// one resets what the images need).
    fn write_resource_lists(&mut self) {
        for n in self.lists.objs.clone() {
            if !self.is_written(n) && matches!(self.data.get(&n), Some(Data::Raw { .. })) {
                self.write_obj(n);
            }
        }
        for n in self.lists.ximages.clone() {
            if !self.is_written(n) {
                self.write_image(n);
            }
        }
        for n in self.lists.xforms.clone() {
            if !self.is_written(n) && matches!(self.data.get(&n), Some(Data::Xform { .. })) {
                let saved = std::mem::take(&mut self.lists);
                self.ship_form(n);
                self.lists = saved;
            }
        }
    }

    /// "Write out resources dictionary".
    fn write_resources(&mut self, n: u32, extra: Option<&[u8]>) {
        let mut o = Out::new();
        if let Some(r) = extra.filter(|r| !r.is_empty()) {
            o.print_ln(r);
        }
        let fonts = self.lists.fonts.clone();
        if !fonts.is_empty() {
            o.print(b"/Font << ");
            for f in fonts {
                let ff = self.ff(f);
                o.print(b"/F");
                o.int(i64::from(ff));
                o.out(b' ');
                o.int(self.fonts.get(&ff).map_or(0, |u| u.num));
                o.print(b" 0 R ");
            }
            o.print_ln(b">>");
            self.lists.text_procset = true;
        }
        if !self.lists.xforms.is_empty() || !self.lists.ximages.is_empty() {
            o.print(b"/XObject << ");
            for &k in &self.lists.xforms {
                let i = match self.data.get(&k) {
                    Some(Data::Xform { index, .. }) => *index,
                    _ => 0,
                };
                o.print(b"/Fm");
                o.int(i);
                o.out(b' ');
                o.int(i64::from(k));
                o.print(b" 0 R ");
            }
            for &k in &self.lists.ximages {
                let (i, c) = match self.data.get(&k) {
                    Some(Data::Ximage { index, image, .. }) => (*index, image.color),
                    _ => (0, 0),
                };
                o.print(b"/Im");
                o.int(i);
                o.out(b' ');
                o.int(i64::from(k));
                o.print(b" 0 R ");
                self.lists.image_procset |= c;
            }
            o.print_ln(b">>");
        }
        let p = &self.cur;
        if p.omit_procset < 0 || (p.omit_procset == 0 && p.major < 2) {
            use super::image::{IMAGE_COLOR_B, IMAGE_COLOR_C, IMAGE_COLOR_I};
            o.print(b"/ProcSet [ /PDF");
            if self.lists.text_procset {
                o.print(b" /Text");
            }
            if self.lists.image_procset & IMAGE_COLOR_B != 0 {
                o.print(b" /ImageB");
            }
            if self.lists.image_procset & IMAGE_COLOR_C != 0 {
                o.print(b" /ImageC");
            }
            if self.lists.image_procset & IMAGE_COLOR_I != 0 {
                o.print(b" /ImageI");
            }
            o.print_ln(b" ]");
        }
        self.begin_dict(n, 1);
        self.print_out(&o);
        self.end_dict();
    }

    /// Form box `b`'s stream and marks where the fold is (`ff_of`: the
    /// fonts' sharing).
    fn form_render(&self, b: &crate::item::BoxNode, ff_of: &dyn Fn(u32) -> Option<u32>) -> PageBody {
        let view = View { objs: &self.objs, data: &self.data };
        let fixed = self.fixed.unwrap_or(Fixed { decimal_digits: 3 });
        let g = content::Given { numbers: self.numbers, extra: &[], ff_of, fixed, objs: &view, entry: &self.walk };
        content::form_body(b, self.cur_page_height, self.compress_level, content::Margins::default(), &g)
    }

    /// Page `p`'s stream and marks where the fold is.
    fn page_render(&self, p: &Page, ship: &Ship, ff_of: &dyn Fn(u32) -> Option<u32>) -> PageBody {
        let view = View { objs: &self.objs, data: &self.data };
        let fixed = self.fixed.unwrap_or(Fixed { decimal_digits: 3 });
        let g = content::Given { numbers: self.numbers, extra: &[], ff_of, fixed, objs: &view, entry: &self.walk };
        content::page_body(p, ship, &g)
    }

    // ---- a page (§750–§752, "Finish shipping")

    #[allow(clippy::too_many_lines)]
    fn ship(&mut self, p: &Page, ship: &Ship, body: &PageBody) {
        self.cur = ship.params.clone();
        self.compress_level = ship.compress_level;
        let last_resources = self.new_objnum();
        self.page_group = 0;
        self.lists = Lists::default();
        self.cur_page_height = body.height;
        let last_page = self.get_obj(OBJ_TYPE_PAGE, &Id::Num((self.total_pages + 1) as i32));
        self.objs[last_page as usize].aux = 1;
        self.last_page = last_page;
        let last_stream = self.new_dict(OBJ_TYPE_OTHERS, 0, 0);
        self.marks = MarkLists::default();
        self.stream(&body.data, ship.compress_level);
        self.walk_events(body);
        self.walk = body.exit.clone();
        self.total_pages += 1;
        let dd = self.fixed.map_or(3, |f| f.decimal_digits);
        // ("Write out page object")
        let mut o = Out::new();
        o.print_ln(b"/Type /Page");
        o.indirect_ln(b"Contents", last_stream);
        o.indirect_ln(b"Resources", last_resources);
        let attr = ship.page_attr.as_deref();
        let mediabox_given = attr.is_some_and(|s| substr_of_str(b"/MediaBox", s));
        if !mediabox_given {
            o.print(b"/MediaBox [0 0 ");
            o.mag_bp(body.width, dd, p.ship.mag);
            o.out(b' ');
            o.mag_bp(body.height, dd, p.ship.mag);
            o.print_ln(b"]");
        }
        if let Some(a) = attr {
            o.print_ln(a);
        }
        // (the dictionary is begun, which may begin an object stream, before
        // the parent Pages object is made)
        self.begin_dict(last_page, 1);
        if self.total_pages % PAGES_TREE_KIDS_MAX == 1 {
            self.last_pages = self.create_obj(OBJ_TYPE_PAGES, PAGES_TREE_KIDS_MAX);
        }
        o.indirect_ln(b"Parent", self.last_pages);
        if self.page_group > 0 {
            o.print(b"/Group ");
            o.int(i64::from(self.page_group));
            o.print_ln(b" 0 R");
        }
        if !self.marks.annots.is_empty() || !self.marks.links.is_empty() {
            o.print(b"/Annots [ ");
            for &(k, ..) in &self.marks.annots {
                o.int(i64::from(k));
                o.print(b" 0 R ");
            }
            for &(k, _) in &self.marks.links {
                o.int(i64::from(k));
                o.print(b" 0 R ");
            }
            o.print_ln(b"]");
        }
        if !self.marks.beads.is_empty() {
            o.print(b"/B [ ");
            for &(k, _) in &self.marks.beads {
                o.int(i64::from(k));
                o.print(b" 0 R ");
            }
            o.print_ln(b"]");
        }
        self.print_out(&o);
        self.end_dict();
        self.write_resource_lists();
        // (the page's parameters again: a form shipped reads its own)
        self.cur = ship.params.clone();
        self.compress_level = ship.compress_level;
        self.write_marks(body, p.ship.mag, dd);
        self.write_resources(last_resources, ship.page_resources.as_deref());
    }

    /// `pdf_ship_out(obj_xform_box(n), false)`: form `n` written.
    fn ship_form(&mut self, n: u32) {
        let Some(Data::Xform { b, attr, resources, .. }) = self.data.get(&n).cloned() else { return };
        let last_resources = self.new_objnum();
        self.page_group = 0;
        self.lists = Lists::default();
        let dd = self.fixed.map_or(3, |f| f.decimal_digits);
        self.begin_dict(n, 0);
        let mut o = Out::new();
        o.print_ln(b"/Type /XObject");
        o.print_ln(b"/Subtype /Form");
        if let Some(a) = attr.filter(|a| !a.is_empty()) {
            o.print_ln(&a);
        }
        o.print(b"/BBox [");
        o.print(b"0 0 ");
        o.bp(b.w, dd);
        o.out(b' ');
        o.bp(b.h + b.d, dd);
        o.print_ln(b"]");
        o.print_ln(b"/FormType 1");
        o.print_ln(b"/Matrix [1 0 0 1 0 0]");
        o.indirect_ln(b"Resources", last_resources);
        self.print_out(&o);
        // (the walk with the fonts' sharing as the fold knows it, made
        // again if a font it uses first shares another's object)
        let body = {
            let guess = |k: u32| -> Option<u32> { self.fonts.contains_key(&k).then(|| self.ff(k)).filter(|&o| o != k) };
            self.form_render(&b, &guess)
        };
        self.walk_events(&body);
        let ff: Vec<(u32, u32)> = body.ff.iter().map(|&(k, _)| (k, self.ff(k))).collect();
        let data = if ff == body.ff {
            body.data.clone()
        } else {
            let ff_of = |k: u32| ff.iter().find(|x| x.0 == k).map(|x| x.1).filter(|&o| o != k);
            self.form_render(&b, &ff_of).data
        };
        self.stream(&data, self.compress_level);
        self.walk = body.exit.clone();
        self.write_resource_lists();
        self.write_resources(last_resources, resources.as_deref());
        if let Some(Data::Xform { attr, resources, .. }) = self.data.get_mut(&n) {
            *attr = None;
            *resources = None;
        }
    }

    // ---- the end ("Finish the PDF file")

    #[allow(clippy::too_many_lines)]
    fn finish(&mut self, end: &End) {
        self.cur = end.params.clone();
        self.compress_level = end.compress_level;
        self.end_page_dims = (end.page_width, end.page_height);
        if self.total_pages == 0 {
            // ("No pages of output.")
            self.out.clear();
            return;
        }
        if self.total_pages % PAGES_TREE_KIDS_MAX != 0 {
            let lp = self.last_pages as usize;
            self.objs[lp].info = self.total_pages % PAGES_TREE_KIDS_MAX;
        }
        // ("Check for non-existing pages")
        let mut k = self.head_tab[usize::from(OBJ_TYPE_PAGE)];
        while k != 0 && self.objs[k as usize].aux == 0 {
            self.warnings.push(format!(
                "pdfTeX warning (dest): Page {} has been referenced but does not exist!",
                self.objs[k as usize].info
            ));
            k = self.objs[k as usize].link;
        }
        self.head_tab[usize::from(OBJ_TYPE_PAGE)] = k;
        // ("Reverse the linked list of Page and Pages objects")
        for t in [OBJ_TYPE_PAGE, OBJ_TYPE_PAGES] {
            let mut k = self.head_tab[usize::from(t)];
            let mut l = 0;
            while k != 0 {
                let i = self.objs[k as usize].link;
                self.objs[k as usize].link = l;
                l = k;
                k = i;
            }
            self.head_tab[usize::from(t)] = l;
        }
        let mut pages_tail = {
            // (`pages_tail` is the first Pages object before the reversal:
            // the last one now)
            let mut k = self.head_tab[usize::from(OBJ_TYPE_PAGES)];
            while k != 0 && self.objs[k as usize].link != 0 {
                k = self.objs[k as usize].link;
            }
            k
        };
        self.fix_dests();
        // ("Output fonts definition")
        let nums: Vec<u32> = self.fonts.keys().copied().collect();
        for k in nums {
            let (num, marks, mapped) = {
                let u = &self.fonts[&k];
                (u.num, u.marks, u.map.is_some())
            };
            if mapped && num < 0 {
                let i = (-num) as u32;
                if let Some(o) = self.fonts.get_mut(&i) {
                    for (a, b) in o.marks.iter_mut().zip(marks) {
                        *a |= b;
                    }
                }
                match (self.font_attr.get(&i).cloned(), self.font_attr.get(&k).cloned()) {
                    (None, Some(a)) => {
                        self.font_attr.insert(i, a);
                    }
                    (Some(a), None) => {
                        self.font_attr.insert(k, a);
                    }
                    (Some(a), Some(b)) if a != b => {
                        self.warnings.push("pdfTeX warning (\\pdffontattr): fonts have conflicting attributes".into());
                    }
                    _ => {}
                }
            }
        }
        self.gen_tounicode = end.gen_tounicode;
        let mut k = self.head_tab[usize::from(OBJ_TYPE_FONT)];
        while k != 0 {
            let f = self.objs[k as usize].info as u32;
            self.do_pdf_font(k, f);
            k = self.objs[k as usize].link;
        }
        let keys: Vec<_> = self.fd_tree.keys().cloned().collect();
        for key in keys {
            self.write_fontdescriptor(&key, end.omit_charset);
        }
        let encs: Vec<_> = self.fe_tree.keys().cloned().collect();
        for e in encs {
            self.write_enc(&e);
        }
        let fos: Vec<_> = self.fo_tree.keys().cloned().collect();
        for t in fos {
            self.write_fontdictionary(&t);
        }
        // ("Output pages tree")
        let a = i64::from(self.sys_obj_ptr) + 1;
        let mut l = self.head_tab[usize::from(OBJ_TYPE_PAGES)];
        let mut k = self.head_tab[usize::from(OBJ_TYPE_PAGE)];
        let mut b = 0u32;
        loop {
            let mut i: i64 = 0;
            let mut c = 0u32;
            let is_root = self.objs[l as usize].link == 0;
            loop {
                if !is_root {
                    if i % PAGES_TREE_KIDS_MAX == 0 {
                        self.last_pages = self.new_objnum();
                        if c == 0 {
                            c = self.last_pages;
                        }
                        self.objs[pages_tail as usize].link = self.last_pages;
                        pages_tail = self.last_pages;
                        let lp = self.last_pages as usize;
                        self.objs[lp].link = 0;
                        self.objs[lp].info = self.objs[l as usize].info;
                    } else {
                        let lp = self.last_pages as usize;
                        self.objs[lp].info += self.objs[l as usize].info;
                    }
                }
                // ("Output the current Pages object in this level")
                let mut o = Out::new();
                o.print_ln(b"/Type /Pages");
                o.int_entry_ln(b"Count", self.objs[l as usize].info);
                if !is_root {
                    o.indirect_ln(b"Parent", self.last_pages);
                }
                o.print(b"/Kids [");
                let mut j: i64 = 0;
                loop {
                    o.int(i64::from(k));
                    o.print(b" 0 R ");
                    k = self.objs[k as usize].link;
                    j += 1;
                    if (i64::from(l) < a && j == self.objs[l as usize].info)
                        || k == 0
                        || (k == b && b != 0)
                        || j == PAGES_TREE_KIDS_MAX
                    {
                        break;
                    }
                }
                o.remove_last_space();
                o.print_ln(b"]");
                if k == 0 {
                    k = self.head_tab[usize::from(OBJ_TYPE_PAGES)];
                    self.head_tab[usize::from(OBJ_TYPE_PAGES)] = 0;
                }
                if is_root && let Some(pa) = &end.pages_attr {
                    o.print_ln(pa);
                }
                self.begin_dict(l, 1);
                self.print_out(&o);
                self.end_dict();
                i += 1;
                l = self.objs[l as usize].link;
                if l == c {
                    break;
                }
            }
            b = c;
            if l == 0 {
                break;
            }
        }
        let outlines = self.output_outlines();
        let names_tree = self.output_name_tree();
        let threads = self.output_threads();
        // ("Output the catalog object")
        let root = self.create_obj(OBJ_TYPE_OTHERS, 0);
        self.begin_dict(root, 1);
        let mut o = Out::new();
        o.print_ln(b"/Type /Catalog");
        o.indirect_ln(b"Pages", self.last_pages);
        if threads != 0 {
            o.indirect_ln(b"Threads", threads);
        }
        if outlines != 0 {
            o.indirect_ln(b"Outlines", outlines);
        }
        if names_tree != 0 {
            o.indirect_ln(b"Names", names_tree);
        }
        if let Some(c) = self.catalog.take().filter(|c| !c.is_empty()) {
            o.print_ln(&c);
        }
        if self.openaction != 0 {
            o.indirect_ln(b"OpenAction", self.openaction);
        }
        self.print_out(&o);
        self.end_dict();
        // (`pdf_print_info`)
        if end.omit_info_dict == 0 {
            let info = self.create_obj(OBJ_TYPE_OTHERS, 0);
            self.begin_dict(info, 3);
            let s = self.info.take();
            let given = |k: &[u8]| s.as_ref().is_some_and(|s| substr_of_str(k, s));
            let mut o = Out::new();
            if !given(b"/Producer") {
                o.print_ln(b"/Producer (pdfTeX-1.40.29)");
            }
            if let Some(s) = s.as_ref().filter(|s| !s.is_empty()) {
                o.print_ln(s);
            }
            if !given(b"/Creator") {
                o.str_entry_ln(b"Creator", b"TeX");
            }
            if end.info_omit_date == 0 {
                if !given(b"/CreationDate") {
                    o.print(b"/CreationDate (");
                    o.print(end.date.as_bytes());
                    o.print_ln(b")");
                }
                if !given(b"/ModDate") {
                    o.print(b"/ModDate (");
                    o.print(end.date.as_bytes());
                    o.print_ln(b")");
                }
            }
            if !given(b"/Trapped") {
                o.print_ln(b"/Trapped /False");
            }
            if end.suppress_ptex_info % 2 == 0 {
                let key: &[u8] = if end.ptex_use_underscore > 0 { b"PTEX_Fullbanner" } else { b"PTEX.Fullbanner" };
                o.str_entry_ln(key, end.banner.as_bytes());
            }
            self.print_out(&o);
            self.end_dict();
        }
        let id = {
            let mut d = end.date.as_bytes().to_vec();
            d.extend_from_slice(end.file_name.as_bytes());
            let digest = super::md5::md5(&d);
            let hex: String = digest.iter().fold(String::new(), |mut h, b| {
                use std::fmt::Write as _;
                let _ = write!(h, "{b:02X}");
                h
            });
            format!("/ID [<{hex}> <{hex}>]")
        };
        // (`\pdftrailerid`'s instead: `print_ID_alt`, nothing for an empty one)
        let id = match &self.trailer_id {
            None => id.into_bytes(),
            Some(t) => {
                let t: Vec<u8> = t.iter().copied().take_while(|&c| c != 0).collect();
                if t.is_empty() {
                    Vec::new()
                } else {
                    let digest = super::md5::md5(&t);
                    let hex: String = digest.iter().fold(String::new(), |mut h, b| {
                        use std::fmt::Write as _;
                        let _ = write!(h, "{b:02X}");
                        h
                    });
                    format!("/ID [<{hex}> <{hex}>]").into_bytes()
                }
            }
        };
        let trailer = self.trailer.take().filter(|t| !t.is_empty());
        if self.os_enable {
            self.os_switch(true);
            self.os_write_objstream();
            self.os_switch(false);
            // ("Output the cross-reference stream dictionary")
            let xref = self.create_obj(OBJ_TYPE_OTHERS, 0);
            self.begin_dict(xref, 0);
            let last = self.objs[self.sys_obj_ptr as usize].offset;
            let w = if last / 256 > 16_777_215 {
                5
            } else if last > 16_777_215 {
                4
            } else if last > 65535 {
                3
            } else {
                2
            };
            self.free_list();
            let mut o = Out::new();
            o.print_ln(b"/Type /XRef");
            o.print(b"/Index [0 ");
            o.int(i64::from(self.obj_ptr) + 1);
            o.print_ln(b"]");
            o.int_entry_ln(b"Size", i64::from(self.obj_ptr) + 1);
            o.print(b"/W [1 ");
            o.int(w);
            o.print_ln(b" 1]");
            o.indirect_ln(b"Root", root);
            if end.omit_info_dict == 0 {
                o.indirect_ln(b"Info", self.obj_ptr - 1);
            }
            if let Some(t) = &trailer {
                o.print_ln(t);
            }
            o.print(&id);
            o.nl();
            self.print_out(&o);
            let mut d = Out::new();
            for k in 0..=self.sys_obj_ptr {
                let e = self.objs[k as usize];
                if !self.is_written(k) {
                    d.out(0);
                    d.bytes(i64::from(e.link), w as usize);
                    d.out(255);
                } else if e.os_idx == -1 {
                    d.out(1);
                    d.bytes(e.offset, w as usize);
                    d.out(0);
                } else {
                    d.out(2);
                    d.bytes(e.offset, w as usize);
                    d.out(e.os_idx as u8);
                }
            }
            let level = self.compress_level;
            let z = if level > 0 { super::zlib::compress(&d.b, level) } else { d.b };
            self.stream(&z, level);
            let mut o = Out::new();
            o.print_ln(b"startxref");
            o.int_ln(self.objs[self.sys_obj_ptr as usize].offset);
            o.print_ln(b"%%EOF");
            self.out.extend_from_slice(&o.b);
        } else {
            // ("Output the obj_tab")
            self.free_list();
            let save_offset = self.out.len();
            let mut o = Out::new();
            o.print_ln(b"xref");
            o.print(b"0 ");
            o.int_ln(i64::from(self.obj_ptr) + 1);
            o.fw_int(i64::from(self.objs[0].link), 10);
            o.print_ln(b" 65535 f ");
            for k in 1..=self.obj_ptr {
                if self.is_written(k) {
                    o.fw_int(self.objs[k as usize].offset, 10);
                    o.print_ln(b" 00000 n ");
                } else {
                    o.fw_int(i64::from(self.objs[k as usize].link), 10);
                    o.print_ln(b" 00000 f ");
                }
            }
            // ("Output the trailer")
            o.print_ln(b"trailer");
            o.print(b"<< ");
            o.int_entry_ln(b"Size", i64::from(self.sys_obj_ptr) + 1);
            o.indirect_ln(b"Root", root);
            if end.omit_info_dict == 0 {
                o.indirect_ln(b"Info", self.sys_obj_ptr);
            }
            if let Some(t) = &trailer {
                o.print_ln(t);
            }
            o.print(&id);
            o.print_ln(b" >>");
            o.print_ln(b"startxref");
            o.int_ln(save_offset as i64);
            o.print_ln(b"%%EOF");
            self.out.extend_from_slice(&o.b);
        }
    }

    /// "Build a linked list of free objects".
    fn free_list(&mut self) {
        let mut l = 0usize;
        self.objs[0].offset = -2;
        for k in 1..=self.sys_obj_ptr {
            if !self.is_written(k) {
                self.objs[l].link = k;
                l = k as usize;
            }
        }
        self.objs[l].link = 0;
    }
}

/// `substr_of_str(s, t)`: whether `s` occurs in `t` (not at its very
/// end: pdfTeX's loop stops `length(s)` before it).
fn substr_of_str(s: &[u8], t: &[u8]) -> bool {
    if t.len() <= s.len() {
        return false;
    }
    (0..t.len() - s.len()).any(|k| &t[k..k + s.len()] == s)
}

/// `lookup_fontmap`'s reading of `<name>-Slant_<n>`, `-Extend_<n>`: the
/// name, slant and extend.
fn slant_extend(s: &[u8]) -> (Vec<u8>, i32, i32) {
    fn strtol(s: &[u8]) -> (i32, usize) {
        let neg = s.first() == Some(&b'-');
        let from = usize::from(neg || s.first() == Some(&b'+'));
        let n = s[from..].iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return (0, 0);
        }
        let v = s[from..from + n].iter().fold(0i64, |v, &c| v * 10 + i64::from(c - b'0'));
        ((if neg { -v } else { v }) as i32, from + n)
    }
    let find = |h: &[u8], n: &[u8]| h.windows(n.len()).position(|w| w == n);
    if let Some(a) = find(s, b"-Slant_") {
        let b = a + 7;
        let (sl, n) = strtol(&s[b..]);
        if n != 0 && b + n == s.len() {
            return (s[..a].to_vec(), sl, 0);
        }
        if n != 0
            && let Some(c) = find(&s[b + n..], b"-Extend_")
        {
            let d = b + n + c + 8;
            let (ex, m) = strtol(&s[d..]);
            if m != 0 && d + m == s.len() {
                return (s[..a].to_vec(), sl, ex);
            }
        }
    } else if let Some(a) = find(s, b"-Extend_") {
        let b = a + 8;
        let (ex, n) = strtol(&s[b..]);
        if n != 0 && b + n == s.len() {
            return (s[..a].to_vec(), 0, ex);
        }
    }
    (s.to_vec(), 0, 0)
}

/// The PDF file of `pages` (each with its body, made again here where its
/// guesses about the pages before do not hold) and the PDF events before
/// each page and after the last (`events`, one more than the pages),
/// fonts numbered by `numbers`, the job ending as `end` says.
#[allow(clippy::too_many_lines)]
pub fn assemble(
    pages: &[Page],
    bodies: &mut [Arc<PageBody>],
    events: &[&PdfEvents],
    numbers: &Numbers,
    end: &End,
    files: &mut dyn Files,
) -> Written {
    let Some(first) = pages.first().and_then(|p| p.ship.pdf.clone()) else {
        return Written::default();
    };
    let mut f = F {
        files,
        out: Vec::new(),
        objs: vec![Obj { info: 0, link: 0, offset: -2, os_idx: 0, aux: 0, t: OBJ_TYPE_OTHERS }],
        data: HashMap::new(),
        head_tab: [0; HEAD_TAB_MAX + 1],
        obj_ptr: 0,
        sys_obj_ptr: 0,
        ids: HashMap::new(),
        dest_names: Vec::new(),
        obj_count: 0,
        xform_count: 0,
        ximage_count: 0,
        os_enable: false,
        os_mode: false,
        os_buf: Vec::new(),
        os_cur_objnum: 0,
        os_objidx: 0,
        os_objnum: Vec::new(),
        os_objoff: Vec::new(),
        fixed_objcompresslevel: 0,
        compress_level: first.compress_level,
        version: None,
        fixed: None,
        cur: first.params.clone(),
        cur_page_height: 0,
        total_pages: 0,
        last_pages: 0,
        last_page: 0,
        page_group: 0,
        fonts: BTreeMap::new(),
        maps: Maps { pending: true, ..Maps::default() },
        fd_tree: BTreeMap::new(),
        fo_tree: BTreeMap::new(),
        fe_tree: BTreeMap::new(),
        tags: BTreeSet::new(),
        last_arg_other_subr3: 0,
        font_attr: HashMap::new(),
        nobuiltin: HashSet::new(),
        gen_tounicode: 0,
        walk: Arc::new(WalkState::default()),
        lists: Lists::default(),
        marks: MarkLists::default(),
        first_outline: 0,
        last_outline: 0,
        parent_outline: 0,
        info: None,
        catalog: None,
        openaction: 0,
        names: None,
        trailer: None,
        trailer_id: None,
        epdf: Vec::new(),
        png_group: 0,
        png_group_written: false,
        numbers,
        answers: crate::build::PdfAnswers::default(),
        end_page_dims: (end.page_width, end.page_height),
        ptex_use_underscore: end.ptex_use_underscore > 0,
        errors: Vec::new(),
        warnings: Vec::new(),
    };
    let mut again = 0;
    for (i, (p, body)) in pages.iter().zip(bodies.iter_mut()).enumerate() {
        if let Some(ev) = events.get(i) {
            for ((piece, _), it) in ev.iter() {
                f.step(*piece, it);
            }
        }
        let Some(ship) = p.ship.pdf.clone() else {
            f.errors.push("pdfTeX error (setup): \\pdfoutput can only be changed before anything is written to the output".into());
            continue;
        };
        f.cur = ship.params.clone();
        f.init_output(ship.decimal_digits);
        let fixed = f.fixed.unwrap_or_else(|| Fixed::of(&ship));
        // (the page's guesses: the fixed values; which font each font
        // shares its object with — a font used before, the one it shares
        // with; one not used before, an earlier page's font of its name,
        // else the page's own guess; the walk's state at its start; the
        // forms and images it refers to)
        let numbered = body.fonts.iter().all(|u| numbers.get(&u.font.key) == Some(&u.num));
        let truth = |f: &F<'_>, k: u32, g: u32| -> u32 {
            let name = body.fonts.iter().find(|u| u.num == k).map(|u| u.font.name.clone());
            f.ff_truth(k, name.as_deref(), g)
        };
        let ff_holds = numbered && body.ff.iter().all(|&(k, g)| truth(&f, k, g) == g);
        if body.fixed != fixed || !ff_holds || body.incomplete || *body.entry != *f.walk {
            let guess = if numbered { body.ff.clone() } else { content::guess_ff(p, numbers, &[]) };
            let ff: Vec<(u32, u32)> = guess.iter().map(|&(k, g)| (k, truth(&f, k, g))).collect();
            let ff_of = |k: u32| -> Option<u32> { ff.iter().find(|x| x.0 == k).map(|x| x.1).filter(|&o| o != k) };
            *body = Arc::new(f.page_render(p, &ship, &ff_of));
            again += 1;
        }
        f.ship(p, &ship, body);
    }
    if let Some(ev) = events.get(pages.len()) {
        for ((piece, _), it) in ev.iter() {
            f.step(*piece, it);
        }
    }
    f.finish(end);
    Written { bytes: f.out, errors: f.errors, warnings: f.warnings, again, answers: f.answers }
}

/// Where the pages say each font's glyphs are (for tests): the union of
/// the pages' glyph sets by font number.
pub fn glyphs_used(bodies: &[Arc<PageBody>]) -> HashMap<u32, Glyphs> {
    let mut m: HashMap<u32, Glyphs> = HashMap::new();
    for b in bodies {
        for u in &b.fonts {
            let e = m.entry(u.num).or_insert([0; 4]);
            for (a, x) in e.iter_mut().zip(u.glyphs) {
                *a |= x;
            }
        }
    }
    m
}
