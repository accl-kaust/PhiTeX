//! The PDF file as a fold over the shipped pages (pdftex.web parts 32d
//! and 32f: `pdf_ship_out`'s objects, "Finish the PDF file"): the fold
//! receives the pages in document order — pdfTeX's order of execution —
//! and numbers every object as pdfTeX does while it writes them, so the
//! numbers, the offsets and the cross-reference table come out as
//! pdfTeX's. Every table here (the objects, the fonts used, the font
//! descriptors, the encodings) is the fold's own, built from the pages in
//! order; none is touched by a node.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::dvi::Numbers;
use crate::page::Page;
use crate::tfm::Font;

use super::content::{Event, Glyphs, PageBody};
use super::font::enc::NOTDEF;
use super::font::map::{F_INCLUDED, F_PK, F_SUBSETTED, F_TYPE1, Map, MapEntry};
use super::font::t1::{self, FONT_KEYS, T1Job};
use super::num::{Out, divide_scaled};
use super::{Fixed, Ship};

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
}

/// The files the fold reads (the map file, font files, encodings): their
/// bytes and the path TeX would show, by name.
pub trait Files {
    fn get(&mut self, name: &str) -> Option<(Arc<str>, Arc<[u8]>)>;
}

/// The file, and what the fold had to say.
#[derive(Default)]
pub struct Written {
    pub bytes: Vec<u8>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Pages whose stream was made again (their guesses did not hold).
    pub again: usize,
}

const OBJ_TYPE_OTHERS: usize = 0;
const OBJ_TYPE_PAGE: usize = 1;
const OBJ_TYPE_PAGES: usize = 2;
const OBJ_TYPE_FONT: usize = 3;
const HEAD_TAB_MAX: usize = 10;
const PAGES_TREE_KIDS_MAX: i64 = 6;
const PDF_OS_MAX_OBJS: usize = 100;

/// An entry of `obj_tab`.
#[derive(Clone, Copy, Debug)]
struct Obj {
    info: i64,
    link: u32,
    /// -2 fresh, -1 scheduled; else the offset in the file (or the number
    /// of the object stream holding it).
    offset: i64,
    os_idx: i32,
    aux: i64,
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
    font_dim: [(i32, bool); 11],
    tx_tree: BTreeSet<u8>,
    gl_tree: BTreeSet<Vec<u8>>,
}

/// A font dictionary (`fo_entry`).
struct Fo {
    objnum: u32,
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

/// The fold's state: pdfTeX's globals of PDF output, the fold's own.
struct F<'a> {
    files: &'a mut dyn Files,
    out: Vec<u8>,
    objs: Vec<Obj>,
    head_tab: [u32; HEAD_TAB_MAX + 1],
    obj_ptr: u32,
    sys_obj_ptr: u32,
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
    version_written: bool,
    fixed_major: i32,
    fixed_minor: i32,
    // (pages)
    total_pages: i64,
    last_pages: u32,
    // (fonts)
    fonts: BTreeMap<u32, UsedFont>,
    map: Option<Map>,
    map_read: bool,
    fd_tree: BTreeMap<(Vec<u8>, i32, i32), Fd>,
    fo_tree: BTreeMap<Vec<u8>, Fo>,
    fe_tree: BTreeMap<Vec<u8>, Fe>,
    tags: BTreeSet<[u8; 6]>,
    last_arg_other_subr3: i32,
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl F<'_> {
    // ---- the object table (§695–§701)

    /// `pdf_create_obj`.
    fn create_obj(&mut self, t: usize, i: i64) -> u32 {
        self.sys_obj_ptr += 1;
        self.obj_ptr = self.sys_obj_ptr;
        let n = self.obj_ptr;
        let e = Obj { info: i, link: 0, offset: -2, os_idx: 0, aux: 0 };
        if self.objs.len() <= n as usize {
            self.objs.resize(n as usize + 1, e);
        }
        self.objs[n as usize] = e;
        if t == OBJ_TYPE_PAGE {
            let mut p = self.head_tab[t];
            if p == 0 || self.objs[p as usize].info < i {
                self.objs[n as usize].link = p;
                self.head_tab[t] = n;
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
            self.objs[n as usize].link = self.head_tab[t];
            self.head_tab[t] = n;
        }
        n
    }

    /// `pdf_new_objnum`.
    fn new_objnum(&mut self) -> u32 {
        self.create_obj(OBJ_TYPE_OTHERS, 0)
    }

    fn is_written(&self, k: u32) -> bool {
        self.objs[k as usize].offset > -1
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

    /// `check_pdfversion`: the header, before the first object.
    fn check_pdfversion(&mut self) {
        if self.version_written {
            return;
        }
        self.version_written = true;
        let mut o = Out::new();
        o.print(b"%PDF-");
        o.int(i64::from(self.fixed_major));
        o.out(b'.');
        o.int_ln(i64::from(self.fixed_minor));
        o.out(b'%');
        o.print(&[208, 212, 197, 216]);
        o.nl();
        self.out.extend_from_slice(&o.b);
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
    fn new_dict(&mut self, t: usize, i: i64, level: i32) -> u32 {
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

    fn map_entry(&mut self, f: &Font) -> Option<Arc<MapEntry>> {
        if !self.map_read {
            self.map_read = true;
            match self.files.get("pdftex.map") {
                Some((_, data)) => self.map = Some(Map::new(data)),
                None => self.warnings.push("cannot open font map file".into()),
            }
        }
        self.map.as_ref()?.lookup(f.name.as_bytes())
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
            let mut i = self.head_tab[OBJ_TYPE_FONT];
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
            font_dim,
            tx_tree: BTreeSet::new(),
            gl_tree: BTreeSet::new(),
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
            Fo { objnum: font_objnum, fd: key, fe: fe.map(|(n, _)| n), first_char: first, last_char: last, cw_objnum },
        );
    }

    /// `write_fontname`.
    fn fontname(o: &mut Out, fd: &Fd, key: &[u8]) {
        o.print(b"/");
        o.print(key);
        o.print(b" /");
        if let Some(t) = &fd.subset_tag {
            o.print(t);
            o.print(b"+");
        }
        o.print_ln(&fd.fontname);
    }

    /// `write_fontdescriptor` (with `write_fontfile`, `writet1`).
    fn write_fontdescriptor(&mut self, key: &(Vec<u8>, i32, i32), end: &End) {
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
            all_glyphs: false,
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
        let objnum = self.new_objnum();
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
        if end.omit_charset == 0 && fm.is(F_SUBSETTED) {
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
    fn write_fontdictionary(&mut self, tfm: &[u8], end: &End) {
        let fo = &self.fo_tree[tfm];
        if end.gen_tounicode > 0 {
            self.errors.push("\\pdfgentounicode is not supported yet".into());
        }
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
        let n = fo.objnum;
        self.begin_dict(n, 1);
        self.print_out(&o);
        self.end_dict();
    }

    // ---- a page (§750–§752, "Finish shipping")

    fn ship(&mut self, p: &Page, ship: &Ship, body: &PageBody) {
        self.compress_level = ship.compress_level;
        let last_resources = self.new_objnum();
        // (`get_obj(obj_type_page, total_pages + 1)`: no page is referred
        // to before it is shipped yet)
        let last_page = self.create_obj(OBJ_TYPE_PAGE, self.total_pages + 1);
        self.objs[last_page as usize].aux = 1;
        let last_stream = self.new_dict(OBJ_TYPE_OTHERS, 0, 0);
        self.stream(&body.data, ship.compress_level);
        // (the objects the page made, in the order its stream made them)
        for e in &body.events {
            match e {
                Event::Font(f) => {
                    if !self.fonts.contains_key(f)
                        && let Some(u) = body.fonts.iter().find(|u| u.num == *f)
                    {
                        let font = u.font.clone();
                        self.init_font(*f, &font);
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
        self.total_pages += 1;
        // ("Write out page object")
        let mut o = Out::new();
        o.print_ln(b"/Type /Page");
        o.indirect_ln(b"Contents", last_stream);
        o.indirect_ln(b"Resources", last_resources);
        let attr = ship.page_attr.as_deref();
        let mediabox_given = attr.is_some_and(|s| substr_of_str(b"/MediaBox", s));
        if !mediabox_given {
            o.print(b"/MediaBox [0 0 ");
            o.mag_bp(body.width, body.fixed.decimal_digits, p.ship.mag);
            o.out(b' ');
            o.mag_bp(body.height, body.fixed.decimal_digits, p.ship.mag);
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
        self.print_out(&o);
        self.end_dict();
        // ("Write out resources dictionary")
        let mut o = Out::new();
        if let Some(r) = &ship.page_resources {
            o.print_ln(r);
        }
        let mut list: Vec<u32> = Vec::new();
        for u in &body.fonts {
            let k = self.ff(u.num);
            if !list.iter().any(|&g| self.ff(g) == k) {
                list.push(u.num);
            }
        }
        if !list.is_empty() {
            o.print(b"/Font << ");
            for f in list {
                let ff = self.ff(f);
                o.print(b"/F");
                o.int(i64::from(ff));
                o.out(b' ');
                o.int(self.fonts.get(&ff).map_or(0, |u| u.num));
                o.print(b" 0 R ");
            }
            o.print_ln(b">>");
        }
        // (`pdf_major_version` is the parameter where the page is shipped)
        if ship.omit_procset < 0 || (ship.omit_procset == 0 && ship.major_version < 2) {
            o.print(b"/ProcSet [ /PDF");
            if !body.fonts.is_empty() {
                o.print(b" /Text");
            }
            o.print_ln(b" ]");
        }
        self.begin_dict(last_resources, 1);
        self.print_out(&o);
        self.end_dict();
    }

    // ---- the end ("Finish the PDF file")

    fn finish(&mut self, end: &End) {
        self.compress_level = end.compress_level;
        if self.total_pages % PAGES_TREE_KIDS_MAX != 0 {
            let lp = self.last_pages as usize;
            self.objs[lp].info = self.total_pages % PAGES_TREE_KIDS_MAX;
        }
        // ("Reverse the linked list of Page and Pages objects")
        for t in [OBJ_TYPE_PAGE, OBJ_TYPE_PAGES] {
            let mut k = self.head_tab[t];
            let mut l = 0;
            while k != 0 {
                let i = self.objs[k as usize].link;
                self.objs[k as usize].link = l;
                l = k;
                k = i;
            }
            self.head_tab[t] = l;
        }
        let mut pages_tail = {
            // (`pages_tail` is the first Pages object before the reversal:
            // the last one now)
            let mut k = self.head_tab[OBJ_TYPE_PAGES];
            while k != 0 && self.objs[k as usize].link != 0 {
                k = self.objs[k as usize].link;
            }
            k
        };
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
            }
        }
        let mut k = self.head_tab[OBJ_TYPE_FONT];
        while k != 0 {
            let f = self.objs[k as usize].info as u32;
            self.do_pdf_font(k, f);
            k = self.objs[k as usize].link;
        }
        let keys: Vec<_> = self.fd_tree.keys().cloned().collect();
        for key in keys {
            self.write_fontdescriptor(&key, end);
        }
        let encs: Vec<_> = self.fe_tree.keys().cloned().collect();
        for e in encs {
            self.write_enc(&e);
        }
        let fos: Vec<_> = self.fo_tree.keys().cloned().collect();
        for t in fos {
            self.write_fontdictionary(&t, end);
        }
        // ("Output pages tree")
        let a = i64::from(self.sys_obj_ptr) + 1;
        let mut l = self.head_tab[OBJ_TYPE_PAGES];
        let mut k = self.head_tab[OBJ_TYPE_PAGE];
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
                    k = self.head_tab[OBJ_TYPE_PAGES];
                    self.head_tab[OBJ_TYPE_PAGES] = 0;
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
        // ("Output the catalog object")
        let root = self.create_obj(OBJ_TYPE_OTHERS, 0);
        self.begin_dict(root, 1);
        let mut o = Out::new();
        o.print_ln(b"/Type /Catalog");
        o.indirect_ln(b"Pages", self.last_pages);
        self.print_out(&o);
        self.end_dict();
        // (`pdf_print_info`)
        if end.omit_info_dict == 0 {
            let info = self.create_obj(OBJ_TYPE_OTHERS, 0);
            self.begin_dict(info, 3);
            let mut o = Out::new();
            o.print_ln(b"/Producer (pdfTeX-1.40.29)");
            o.str_entry_ln(b"Creator", b"TeX");
            if end.info_omit_date == 0 {
                o.print(b"/CreationDate (");
                o.print(end.date.as_bytes());
                o.print_ln(b")");
                o.print(b"/ModDate (");
                o.print(end.date.as_bytes());
                o.print_ln(b")");
            }
            o.print_ln(b"/Trapped /False");
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
            let hex: String = digest.iter().map(|b| format!("{b:02X}")).collect();
            format!("/ID [<{hex}> <{hex}>]")
        };
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
            o.print(id.as_bytes());
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
                if !self.is_written(k) {
                    o.fw_int(i64::from(self.objs[k as usize].link), 10);
                    o.print_ln(b" 00000 f ");
                } else {
                    o.fw_int(self.objs[k as usize].offset, 10);
                    o.print_ln(b" 00000 n ");
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
            o.print(id.as_bytes());
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

/// The PDF file of `pages` (each with its body, made again here where its
/// guess about the pages before does not hold), fonts numbered by
/// `numbers`, the job ending as `end` says.
pub fn assemble(pages: &[Page], bodies: &mut [Arc<PageBody>], numbers: &Numbers, end: &End, files: &mut dyn Files) -> Written {
    let Some(first) = pages.first().and_then(|p| p.ship.pdf.clone()) else {
        return Written::default();
    };
    // (`check_pdfversion` and `init_pdf_output` at the first page)
    let minor = if (0..=9).contains(&first.minor_version) { first.minor_version } else { 4 };
    let major = first.major_version.max(1);
    let fixed_objcompresslevel = first.obj_compress_level.clamp(0, 3);
    let os_enable = (major > 1 || minor >= 5) && fixed_objcompresslevel > 0;
    let fixed = Fixed::of(&first);
    let mut f = F {
        files,
        out: Vec::new(),
        objs: vec![Obj { info: 0, link: 0, offset: -2, os_idx: 0, aux: 0 }],
        head_tab: [0; HEAD_TAB_MAX + 1],
        obj_ptr: 0,
        sys_obj_ptr: 0,
        os_enable,
        os_mode: false,
        os_buf: Vec::new(),
        os_cur_objnum: 0,
        os_objidx: 0,
        os_objnum: Vec::new(),
        os_objoff: Vec::new(),
        fixed_objcompresslevel: if os_enable { fixed_objcompresslevel } else { 0 },
        compress_level: first.compress_level,
        version_written: false,
        fixed_major: major,
        fixed_minor: minor,
        total_pages: 0,
        last_pages: 0,
        fonts: BTreeMap::new(),
        map: None,
        map_read: false,
        fd_tree: BTreeMap::new(),
        fo_tree: BTreeMap::new(),
        fe_tree: BTreeMap::new(),
        tags: BTreeSet::new(),
        last_arg_other_subr3: 0,
        errors: Vec::new(),
        warnings: Vec::new(),
    };
    let mut again = 0;
    for (p, body) in pages.iter().zip(bodies.iter_mut()) {
        let Some(ship) = p.ship.pdf.clone() else {
            f.errors.push("pdfTeX error (setup): \\pdfoutput can only be changed before anything is written to the output".into());
            continue;
        };
        // (the page's guesses: the fixed values, and which font each font
        // shares its object with — a font used before, the one it shares
        // with; one not used before, an earlier page's font of its name,
        // else the page's own guess)
        let truth = |f: &F<'_>, k: u32, g: u32| -> u32 {
            if f.fonts.contains_key(&k) {
                return f.ff(k);
            }
            let Some(name) = body.fonts.iter().find(|u| u.num == k).map(|u| u.font.name.clone()) else { return g };
            f.fonts.iter().find(|(_, u)| u.font.name == name && u.num > 0).map_or(g, |(&n, _)| n)
        };
        let ff_holds = body.ff.iter().all(|&(k, g)| truth(&f, k, g) == g);
        if body.fixed != fixed || !ff_holds {
            let ff: Vec<(u32, u32)> = body.ff.iter().map(|&(k, g)| (k, truth(&f, k, g))).collect();
            let ff_of = |k: u32| -> Option<u32> { ff.iter().find(|x| x.0 == k).map(|x| x.1).filter(|&o| o != k) };
            *body = Arc::new(super::content::page_body(p, &ship, numbers, &ff_of, fixed));
            again += 1;
        }
        f.ship(p, &ship, body);
    }
    f.finish(end);
    Written { bytes: f.out, errors: f.errors, warnings: f.warnings, again }
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
