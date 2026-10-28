//! `writefont.c` and `writeenc.c`: font dictionaries, descriptors,
//! `/Widths` arrays, `/Encoding` differences, `/ToUnicode` `CMaps`, and the
//! font files of `writet1.c` and `writettf.c`; with the font commands
//! read before shipping (`\pdffontattr`, `\pdfincludechars`,
//! `\pdffontobjnum`, `\pdfmapline`, `\pdfmapfile`,
//! `\pdfnobuiltintounicode`).
//!
//! pdfTeX keeps these in AVL trees (`fo_tree` by TFM name, `fd_tree` by
//! font file, slant and extend, `fe_tree` by encoding name) and writes
//! them in tree order at the end; `BTreeMap`s with the same keys give the
//! same order.

use std::collections::{BTreeMap, BTreeSet};

use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;

use crate::enc::{self, GlyphNames};
use crate::map::{self, MapEntry};
use crate::num::divide_scaled;
use crate::t1::{self, FontDesc, dim};
use crate::{FontSource, FontType, OBJ_TYPE_FONT, PdfError, PdfState, int, pdf_error};

/// `fo_entry`.
struct Fo {
    objnum: usize,
    tex_font: FontId,
    first_char: usize,
    last_char: usize,
    cw_objnum: usize,
    fd: usize,
    fe: Option<Vec<u8>>,
    fm: MapEntry,
}

/// `fd_entry`.
struct Fd {
    objnum: usize,
    fm: MapEntry,
    desc: FontDesc,
    file: Option<std::sync::Arc<[u8]>>,
    ff_found: bool,
    ff_objnum: usize,
    /// `write_ttf_glyph_names`.
    ttf_glyph_names: bool,
    /// `fn_objnum`: the font name object of an included PDF's font.
    fn_objnum: usize,
    /// `fe->glyph_names` of a reencoded TrueType font.
    enc: Option<GlyphNames>,
}

/// `fe_entry`.
struct Fe {
    objnum: usize,
    names: GlyphNames,
    tx_tree: BTreeSet<usize>,
}

/// The trees of `writefont.c` and `writeenc.c` while fonts are written.
#[derive(Default)]
struct FontTables {
    fds: Vec<Fd>,
    /// `fd_tree`: by font file, slant and extend.
    fd_tree: BTreeMap<(Vec<u8>, i32, i32), usize>,
    /// `fe_tree`: by encoding name.
    fe_tree: BTreeMap<Vec<u8>, Fe>,
    /// `fo_tree`: by TFM name.
    fo_tree: BTreeMap<Vec<u8>, Fo>,
}

/// A map item's mode (`FM_DUPIGNORE`, `FM_REPLACE`, `FM_DELETE`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum MapMode {
    DupIgnore,
    Replace,
    Delete,
}

impl PdfState {
    // ---- the TeX side's font commands ----

    /// `\pdffontattr f {...}`.
    pub fn font_attr(&mut self, f: FontId, attr: &[u8], src: &dyn FontSource) {
        self.load_font(f, src);
        self.font_mut(f).attr = attr.to_vec();
    }

    /// `\pdfnobuiltintounicode f`.
    pub fn no_builtin_tounicode(&mut self, f: FontId, src: &dyn FontSource) {
        self.load_font(f, src);
        self.font_mut(f).nobuiltin_tounicode = true;
    }

    /// `pdf_check_vf_cur_val` and `pdf_init_font` for the commands that
    /// name a font.
    pub(crate) fn font_command(&mut self, f: FontId, src: &dyn FontSource) -> Result<(), PdfError> {
        if f == 0 {
            return Err(pdf_error("font", "invalid font identifier"));
        }
        self.load_font(f, src);
        if self.font(f).kind == FontType::New {
            self.do_vf(f, src);
        }
        if self.font(f).kind == FontType::Virtual {
            return Err(pdf_error(
                "font",
                "command cannot be used with virtual font",
            ));
        }
        if !self.font(f).used {
            self.init_font(f, src);
        }
        Ok(())
    }

    /// `\pdfincludechars f {...}`.
    ///
    /// # Errors
    /// A virtual font or `\nullfont`.
    pub fn include_chars(
        &mut self,
        f: FontId,
        chars: &[u8],
        src: &dyn FontSource,
    ) -> Result<(), PdfError> {
        self.font_command(f, src)?;
        for &c in chars {
            self.font_mut(f).chars[usize::from(c)] = true;
        }
        Ok(())
    }

    /// `\pdffontobjnum f`: the font's object number.
    ///
    /// # Errors
    /// A virtual font or `\nullfont`.
    pub fn font_objnum(&mut self, f: FontId, src: &dyn FontSource) -> Result<i64, PdfError> {
        self.font_command(f, src)?;
        Ok(self.font(self.ff(f)).num)
    }

    /// `\pdffontname f`: the number in `/F<n>`.
    ///
    /// # Errors
    /// A virtual font or `\nullfont`.
    pub fn font_name(&mut self, f: FontId, src: &dyn FontSource) -> Result<i64, PdfError> {
        let n = self.font_objnum(f, src)?;
        Ok(usize::try_from(n).map_or(0, |n| self.objs[n].info))
    }

    /// `\pdfmapline{...}` (`process_map_item`).
    pub fn map_line(&mut self, s: &[u8]) {
        let (mode, rest) = self.map_mode(s);
        if rest.is_empty() {
            return;
        }
        if let Some(e) = map::parse_line(rest) {
            self.map_item(mode, e);
        }
    }

    /// `\pdfmapfile{...}`: the file's lines as `\pdfmapline`s.
    pub fn map_file(&mut self, s: &[u8], src: &dyn FontSource) {
        let (mode, rest) = self.map_mode(s);
        let name = rest.split(|&c| c == b' ').next().unwrap_or_default();
        if name.is_empty() {
            return;
        }
        let Some(bytes) = src.file(name) else {
            // "cannot open font map file" is a warning
            return;
        };
        for e in map::parse_map(&bytes).into_values() {
            self.map_item(mode, e);
        }
    }

    /// The item's mode and text.
    fn map_mode<'a>(&mut self, s: &'a [u8]) -> (MapMode, &'a [u8]) {
        let s = s.split(|&c| c == 0).next().unwrap_or_default();
        let s = s.strip_prefix(b" ").unwrap_or(s);
        let (mode, s) = match s.first() {
            Some(b'+') => (MapMode::DupIgnore, &s[1..]),
            Some(b'=') => (MapMode::Replace, &s[1..]),
            Some(b'-') => (MapMode::Delete, &s[1..]),
            _ => {
                // "flush default map file name": unless read already
                if !self.map_read {
                    self.no_default_map = true;
                }
                (MapMode::DupIgnore, s)
            }
        };
        (mode, s.strip_prefix(b" ").unwrap_or(s))
    }

    /// `fm_read_info` for one entry (`avl_do_entry`): a font already
    /// loaded keeps its entry.
    fn map_item(&mut self, mode: MapMode, e: MapEntry) {
        let name = e.tfm_name.clone();
        let in_use = self.fonts.values().any(|s| s.name == name);
        match mode {
            MapMode::Replace => {
                if !in_use {
                    self.map_add.remove(&name);
                    self.map_over.insert(name, Some(e));
                }
            }
            MapMode::Delete => {
                if !in_use {
                    self.map_add.remove(&name);
                    self.map_over.insert(name, None);
                }
            }
            MapMode::DupIgnore => match self.map_over.get(&name) {
                Some(Some(_)) => {}
                Some(None) => {
                    self.map_over.insert(name, Some(e));
                }
                None => {
                    self.map_add.get_or_insert_with(name, || e);
                }
            },
        }
    }

    // ---- writing (writefont.c) ----

    /// `get_char_range`: the first and last characters used.
    fn char_range(&self, f: FontId) -> (usize, usize) {
        let st = self.font(f);
        let first = (0..256).find(|&i| st.chars[i]);
        let last = (0..256).rev().find(|&i| st.chars[i]);
        match (first, last) {
            (Some(a), Some(b)) => (a, b),
            _ => (1, 0),
        }
    }

    /// `preset_fontmetrics`: the descriptor's values from the TFM.
    fn preset_fontmetrics(&self, f: FontId) -> [(i32, bool); 11] {
        let st = self.font(f);
        let m = &st.metrics;
        let size = st.size;
        let d = |x: Scaled| divide_scaled(x, size, 3).0;
        let h = |c: u8| m.char(c).map_or(0, |x| x.height);
        let mut dims = [(0, false); 11];
        #[allow(clippy::cast_possible_truncation)]
        let angle =
            (-(f64::from(m.slant()) / 65536.0).atan() * (180.0 / std::f64::consts::PI)) as i32;
        dims[dim::ITALIC_ANGLE].0 = d(angle);
        dims[dim::ASCENT].0 = d(h(b'h'));
        dims[dim::CAPHEIGHT].0 = d(h(b'H'));
        let i = -d(m.char(b'y').map_or(0, |x| x.depth));
        dims[dim::DESCENT].0 = if i < 0 { i } else { 0 };
        dims[dim::STEMV].0 = d(m.width(b'.').unwrap_or(0) / 3);
        dims[dim::XHEIGHT].0 = d(m.x_height());
        dims[dim::FONTBBOX1].0 = 0;
        dims[dim::FONTBBOX1 + 1].0 = dims[dim::DESCENT].0;
        dims[dim::FONTBBOX1 + 2].0 = d(m.quad());
        dims[dim::FONTBBOX1 + 3].0 = dims[dim::CAPHEIGHT].0.max(dims[dim::ASCENT].0);
        for x in dims.iter_mut().take(dim::INT_KEYS_NUM) {
            x.1 = true;
        }
        dims
    }

    /// `write_charwidth_array`, after `create_charwidth_array`.
    fn write_widths(&mut self, f: FontId, first: usize, last: usize) -> usize {
        let n = self.new_objnum_();
        self.begin_obj(n, 1);
        self.print(b"[");
        for i in first..=last {
            let st = self.font(f);
            let w = divide_scaled(st.width(u16::try_from(i).unwrap_or(0)), st.size, 4).0;
            self.print_int(i64::from(w / 10));
            if w % 10 != 0 {
                self.pdf_out(b'.');
                self.print_int(i64::from(w % 10));
            }
            if i != last {
                self.pdf_out(b' ');
            }
        }
        self.print(b"]\n");
        self.end_obj();
        n
    }

    /// `write_fontname`.
    fn write_fontname(&mut self, desc: &FontDesc, key: &[u8]) {
        self.pdf_out(b'/');
        self.print(key);
        self.print(b" /");
        if let Some(t) = desc.subset_tag {
            self.print(&t);
            self.pdf_out(b'+');
        }
        self.print(&desc.fontname);
        self.pdf_out(b'\n');
    }

    /// [`t1::write_t1`] of `file` for `fd`, by a memo of the process: the
    /// font program is a function of the file, the map entry, the
    /// descriptor as it comes in, the subset tags taken before and
    /// `last_arg_other_subr3`; it gives the program, the descriptor as it
    /// goes out and those two after (a link writes the same fonts again
    /// for every edit, subsetting each).
    fn write_t1_memo(
        &mut self,
        file: &std::sync::Arc<[u8]>,
        fd: &mut Fd,
    ) -> Result<t1::Embedded, PdfError> {
        type Made = (t1::Embedded, t1::FontDesc, i32);
        static MEMO: std::sync::Mutex<Vec<(u128, std::sync::Arc<Made>)>> =
            std::sync::Mutex::new(Vec::new());
        // (the subset tags taken before are not in the key: the program
        // depends on them only through the tag chosen, chosen again here
        // from what chose it, as `make_subset_tag` would)
        let key = crate::digest::debug_hash(&(
            crate::digest::Blob(file),
            &fd.fm,
            &fd.desc,
            self.last_arg_other_subr3,
        ));
        let hit = MEMO
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|e| e.0 == key)
            .map(|e| e.1.clone());
        if let Some(m) = hit {
            let mut e = m.0.clone();
            fd.desc = m.1.clone();
            if let Some((at, glyphs, name)) = &e.tag_at {
                let tag = t1::make_subset_tag(glyphs, name, &mut self.subset_tags);
                e.bytes[*at..*at + 6].copy_from_slice(&tag);
                fd.desc.subset_tag = Some(tag);
            }
            self.last_arg_other_subr3 = m.2;
            return Ok(e);
        }
        let e = t1::write_t1(
            file,
            &fd.fm,
            &mut fd.desc,
            &mut self.subset_tags,
            &mut self.last_arg_other_subr3,
        )?;
        let made = std::sync::Arc::new((e.clone(), fd.desc.clone(), self.last_arg_other_subr3));
        let mut m = MEMO
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if m.len() >= 256 {
            m.remove(0);
        }
        m.push((key, made));
        Ok(e)
    }

    /// `write_fontfile`.
    fn write_fontfile(&mut self, fd: &mut Fd) -> Result<(), PdfError> {
        let name =
            || String::from_utf8_lossy(fd.fm.ff_name.as_deref().unwrap_or_default()).into_owned();
        let Some(file) = fd.file.take() else {
            let what = if fd.fm.is_truetype() {
                "TrueType"
            } else {
                "Type 1"
            };
            return Err(PdfError::Font(format!(
                "cannot open {what} font file for reading: {}",
                name()
            )));
        };
        if fd.fm.is_type1() {
            let e = self.write_t1_memo(&file, fd)?;
            fd.ff_found = true;
            fd.ff_objnum = self.new_objnum_();
            self.begin_dict(fd.ff_objnum, 0);
            self.print(
                format!(
                    "/Length1 {}\n/Length2 {}\n/Length3 {}\n",
                    e.length1, e.length2, e.length3
                )
                .as_bytes(),
            );
            self.begin_stream();
            self.print(&e.bytes);
            self.end_stream();
        } else if fd.fm.is_truetype() {
            let bytes = crate::ttf::write_ttf(
                &file,
                &fd.fm,
                &mut fd.desc,
                fd.enc.as_ref(),
                fd.ttf_glyph_names,
                &mut self.subset_tags,
            )?;
            fd.ff_found = true;
            fd.ff_objnum = self.new_objnum_();
            self.begin_dict(fd.ff_objnum, 0);
            self.print(format!("/Length1 {}\n", bytes.len()).as_bytes());
            self.begin_stream();
            self.print(&bytes);
            self.end_stream();
        } else {
            return Err(PdfError::Unsupported("OpenType (CFF) font files".into()));
        }
        Ok(())
    }

    /// `write_fontdescriptor`.
    fn write_fontdescriptor(&mut self, fd: &mut Fd) -> Result<(), PdfError> {
        if fd.fm.is_fontfile() {
            self.write_fontfile(fd)?;
        }
        if fd.fn_objnum != 0 {
            // write_fontname_object
            self.begin_obj(fd.fn_objnum, 1);
            self.pdf_out(b'/');
            if let Some(t) = fd.desc.subset_tag {
                self.print(&t);
                self.pdf_out(b'+');
            }
            self.print(&fd.desc.fontname.clone());
            self.pdf_out(b'\n');
            self.end_obj();
        }
        if fd.objnum == 0 {
            fd.objnum = self.new_objnum_();
        }
        self.begin_dict(fd.objnum, 1);
        self.print(b"/Type /FontDescriptor\n");
        self.write_fontname(&fd.desc, b"FontName");
        let flags = match fd.fm.fd_flags {
            Some(fl) => fl,
            None if fd.ff_found => 4,
            None if fd.fm.is_std_t1font() => {
                return Err(PdfError::Unsupported("the 14 standard fonts".into()));
            }
            None => 0x22,
        };
        self.print(format!("/Flags {flags}\n").as_bytes());
        // write_fontmetrics, fix_fontmetrics
        let bb = dim::FONTBBOX1;
        let dims = &mut fd.desc.font_dim;
        if dims[bb].1 && dims[bb + 1].1 && dims[bb + 2].1 && dims[bb + 3].1 {
            if !dims[dim::ASCENT].1 {
                dims[dim::ASCENT] = (dims[bb + 3].0, true);
            }
            if !dims[dim::DESCENT].1 {
                dims[dim::DESCENT] = (dims[bb + 1].0, true);
            }
            if !dims[dim::CAPHEIGHT].1 {
                dims[dim::CAPHEIGHT] = (dims[bb + 3].0, true);
            }
        }
        let dims = fd.desc.font_dim;
        if dims[bb].1 && dims[bb + 1].1 && dims[bb + 2].1 && dims[bb + 3].1 {
            self.print(
                format!(
                    "/FontBBox [{} {} {} {}]\n",
                    dims[bb].0,
                    dims[bb + 1].0,
                    dims[bb + 2].0,
                    dims[bb + 3].0
                )
                .as_bytes(),
            );
        }
        for (i, (key, _)) in t1::FONT_KEY.iter().enumerate().take(dim::GEN_KEY_NUM) {
            if dims[i].1 {
                self.print(format!("/{key} {}\n", dims[i].0).as_bytes());
            }
        }
        if fd.ff_found {
            if self.p.omit_charset == 0 && fd.fm.is_subsetted() && fd.fm.is_type1() {
                self.print(b"/CharSet (");
                for g in &fd.desc.gl_tree {
                    self.pdf_out(b'/');
                    self.print(g);
                }
                self.print(b")\n");
            }
            let key: &[u8] = if fd.fm.is_type1() {
                b"FontFile"
            } else if fd.fm.is_truetype() {
                b"FontFile2"
            } else {
                b"FontFile3"
            };
            self.print(b"/");
            self.print(key);
            self.print(format!(" {} 0 R\n", fd.ff_objnum).as_bytes());
        }
        self.end_dict();
        Ok(())
    }

    /// `create_fontdescriptor`.
    fn new_fd(&self, fm: &MapEntry, f: FontId) -> Fd {
        Fd {
            objnum: 0,
            fm: fm.clone(),
            desc: FontDesc {
                fontname: fm.ps_name.clone().unwrap_or_else(|| fm.tfm_name.clone()),
                subset_tag: None,
                font_dim: self.preset_fontmetrics(f),
                gl_tree: BTreeSet::new(),
                tx_tree: None,
                builtin: None,
                all_glyphs: false,
            },
            file: self.font(f).file.clone(),
            ff_found: false,
            ff_objnum: 0,
            ttf_glyph_names: false,
            fn_objnum: 0,
            enc: None,
        }
    }

    /// `create_fontdictionary`.
    #[allow(clippy::too_many_lines)]
    fn create_fontdictionary(
        &mut self,
        fm: MapEntry,
        objnum: usize,
        f: FontId,
        t: &mut FontTables,
    ) -> Result<(), PdfError> {
        let (first, last) = self.char_range(f);
        let marked: Vec<usize> = (first..=last).filter(|&i| self.font(f).chars[i]).collect();
        let mut fe_key = None;
        if let Some(encname) = fm.encname.clone() {
            if !t.fe_tree.contains_key(&encname) {
                let Some(bytes) = self.font(f).enc.clone() else {
                    return Err(PdfError::Font(format!(
                        "cannot open encoding file for reading: {}",
                        String::from_utf8_lossy(&encname)
                    )));
                };
                let names = enc::load_enc(&bytes)?;
                t.fe_tree.insert(
                    encname.clone(),
                    Fe {
                        objnum: 0,
                        names,
                        tx_tree: BTreeSet::new(),
                    },
                );
            }
            if fm.is_type1() || fm.is_opentype() {
                let need = t.fe_tree[&encname].objnum == 0;
                let n = if need { self.new_objnum_() } else { 0 };
                let fe = t.fe_tree.get_mut(&encname).expect("registered above");
                if need {
                    fe.objnum = n;
                }
                fe.tx_tree.extend(marked.iter().copied());
            }
            fe_key = Some(encname);
        }
        let fd_index;
        let cw_objnum;
        if fm.is_fontfile() {
            if fm.is_type1() {
                let key = (fm.ff_name.clone().unwrap_or_default(), fm.slant, fm.extend);
                fd_index = if let Some(&i) = t.fd_tree.get(&key) {
                    i
                } else {
                    t.fds.push(self.new_fd(&fm, f));
                    t.fd_tree.insert(key, t.fds.len() - 1);
                    t.fds.len() - 1
                };
            } else {
                t.fds.push(self.new_fd(&fm, f));
                fd_index = t.fds.len() - 1;
            }
            cw_objnum = self.write_widths(f, first, last);
            let fd = &mut t.fds[fd_index];
            if let Some(k) = &fe_key {
                // mark_reenc_glyphs
                if fm.is_subsetted() {
                    let g = &t.fe_tree[k].names;
                    for &i in &marked {
                        if let Some(n) = &g[i] {
                            fd.desc.gl_tree.insert(n.clone());
                        }
                    }
                }
                if !fm.is_type1() {
                    fd.desc
                        .tx_tree
                        .get_or_insert_with(BTreeSet::new)
                        .extend(marked.iter().copied());
                    if fm.is_truetype() {
                        fd.ttf_glyph_names = true;
                        fd.enc = Some(t.fe_tree[k].names.clone());
                    }
                }
            } else {
                fd.desc
                    .tx_tree
                    .get_or_insert_with(BTreeSet::new)
                    .extend(marked.iter().copied());
            }
            if !fm.is_type1() {
                let mut fd = std::mem::replace(&mut t.fds[fd_index], self.new_fd(&fm, f));
                let r = self.write_fontdescriptor(&mut fd);
                t.fds[fd_index] = fd;
                r?;
            }
        } else {
            // a builtin font: its widths and descriptor now
            cw_objnum = self.write_widths(f, first, last);
            let mut fd = self.new_fd(&fm, f);
            self.write_fontdescriptor(&mut fd)?;
            t.fds.push(fd);
            fd_index = t.fds.len() - 1;
        }
        let fo = Fo {
            objnum,
            tex_font: f,
            first_char: first,
            last_char: last,
            cw_objnum,
            fd: fd_index,
            fe: fe_key,
            fm,
        };
        if fo.fm.is_type1() {
            t.fo_tree.insert(fo.fm.tfm_name.clone(), fo);
        } else {
            self.write_fontdictionary(&fo, t)?;
        }
        Ok(())
    }

    /// `write_fontdictionary`.
    fn write_fontdictionary(&mut self, fo: &Fo, t: &FontTables) -> Result<(), PdfError> {
        let fd = &t.fds[fo.fd];
        let mut tounicode = 0;
        let st = self.font(fo.tex_font);
        if (self.fixed_gen_tounicode > 0 && !st.nobuiltin_tounicode)
            || fo.fm.tfm_name == b"dummy-space"
        {
            if let Some(fe) = fo.fe.as_ref().and_then(|k| t.fe_tree.get(k)) {
                let enc = fo.fe.clone();
                tounicode = self.write_tounicode(&fe.names, &fo.fm.tfm_name, enc.as_deref());
            } else if fo.fm.is_type1() {
                let Some(b) = &fd.desc.builtin else {
                    return Err(PdfError::Font("builtin glyph names is empty".into()));
                };
                tounicode = self.write_tounicode(b, &fo.fm.tfm_name, None);
            }
        }
        self.begin_dict(fo.objnum, 1);
        self.print(b"/Type /Font\n");
        self.print(if fo.fm.is_truetype() {
            b"/Subtype /TrueType\n".as_slice()
        } else {
            b"/Subtype /Type1\n"
        });
        self.write_fontname(&fd.desc, b"BaseFont");
        self.print(format!("/FontDescriptor {} 0 R\n", fd.objnum).as_bytes());
        self.print(
            format!(
                "/FirstChar {}\n/LastChar {}\n/Widths {} 0 R\n",
                fo.first_char, fo.last_char, fo.cw_objnum
            )
            .as_bytes(),
        );
        if (fo.fm.is_type1() || fo.fm.is_opentype())
            && let Some(fe) = fo.fe.as_ref().and_then(|k| t.fe_tree.get(k))
            && fe.objnum != 0
        {
            self.print(format!("/Encoding {} 0 R\n", fe.objnum).as_bytes());
        }
        if tounicode != 0 {
            self.print(format!("/ToUnicode {tounicode} 0 R\n").as_bytes());
        }
        let attr = self.font(fo.tex_font).attr.clone();
        if !attr.is_empty() {
            self.print(&attr);
            self.print(b"\n");
        }
        self.end_dict();
        Ok(())
    }

    /// `write_enc` (`writeenc.c`).
    fn write_enc(&mut self, fe: &Fe) {
        self.begin_dict(fe.objnum, 1);
        self.print(b"/Type /Encoding\n");
        self.print(b"/Differences [");
        let mut i_old: i64 = -2;
        for &c in &fe.tx_tree {
            let name = fe.names[c].as_deref().unwrap_or(b".notdef").to_vec();
            let c = int(c);
            if c == i_old + 1 {
                self.pdf_out(b'/');
                self.print(&name);
            } else {
                if i_old != -2 {
                    self.pdf_out(b' ');
                }
                self.print_int(c);
                self.pdf_out(b'/');
                self.print(&name);
            }
            i_old = c;
        }
        self.print(b"]\n");
        self.end_dict();
    }

    /// "Output fonts definition" (pdfTeX §801) and `write_fontstuff`.
    pub(crate) fn output_fonts(&mut self) -> Result<(), PdfError> {
        let ids: Vec<FontId> = self.fonts.keys().copied().collect();
        for k in ids {
            let st = self.font(k);
            if st.used && st.map.is_some() && st.num < 0 {
                let i = FontId::try_from(-st.num).unwrap_or(0);
                let chars = st.chars;
                for (b, c) in self.font_mut(i).chars.iter_mut().zip(chars) {
                    *b |= c;
                }
                let (ai, ak) = (self.font(i).attr.clone(), self.font(k).attr.clone());
                if ai.is_empty() && !ak.is_empty() {
                    self.font_mut(i).attr = ak;
                } else if ak.is_empty() && !ai.is_empty() {
                    self.font_mut(k).attr = ai;
                }
            }
        }
        self.fixed_gen_tounicode = self.p.gen_tounicode;
        let mut t = FontTables::default();
        // the descriptors of included PDF files' fonts (`epdf.c`), in
        // `fd_tree` before the TeX fonts'
        for (key, e) in (*self.epdf_fds).clone() {
            let mut font_dim = [(0, false); 11];
            font_dim[t1::dim::STEMV] = (e.stemv, true);
            t.fds.push(Fd {
                objnum: e.objnum,
                fm: e.fm.clone(),
                desc: FontDesc {
                    fontname: e.fm.ps_name.clone().unwrap_or_default(),
                    subset_tag: None,
                    font_dim,
                    gl_tree: e.glyphs.clone(),
                    tx_tree: None,
                    builtin: None,
                    all_glyphs: e.all_glyphs,
                },
                file: e.file.clone(),
                ff_found: false,
                ff_objnum: 0,
                ttf_glyph_names: false,
                fn_objnum: e.fn_objnum,
                enc: None,
            });
            t.fd_tree.insert(key, t.fds.len() - 1);
        }
        let mut k = self.head[OBJ_TYPE_FONT];
        while k != 0 {
            let f = FontId::try_from(self.objs[k].info).unwrap_or(0);
            // do_pdf_font
            let (first, last) = self.char_range(f);
            if first <= last {
                match self.font(f).map.clone() {
                    Some(fm) if !fm.is_pk() => self.create_fontdictionary(fm, k, f, &mut t)?,
                    _ => {
                        return Err(PdfError::Unsupported(format!(
                            "PK (Type 3) font {}",
                            String::from_utf8_lossy(&self.font(f).name)
                        )));
                    }
                }
            }
            k = self.objs[k].link;
        }
        // write_fontdescriptors
        let order: Vec<usize> = t.fd_tree.values().copied().collect();
        for i in order {
            let mut fd = std::mem::replace(
                &mut t.fds[i],
                Fd {
                    objnum: 0,
                    fm: MapEntry::default(),
                    desc: FontDesc {
                        fontname: Vec::new(),
                        subset_tag: None,
                        font_dim: [(0, false); 11],
                        gl_tree: BTreeSet::new(),
                        tx_tree: None,
                        builtin: None,
                        all_glyphs: false,
                    },
                    file: None,
                    ff_found: false,
                    ff_objnum: 0,
                    ttf_glyph_names: false,
                    fn_objnum: 0,
                    enc: None,
                },
            );
            let r = self.write_fontdescriptor(&mut fd);
            t.fds[i] = fd;
            r?;
        }
        // write_fontencodings
        for fe in t.fe_tree.values() {
            if fe.objnum != 0 {
                self.write_enc(fe);
            }
        }
        // write_fontdictionaries
        let keys: Vec<Vec<u8>> = t.fo_tree.keys().cloned().collect();
        for key in keys {
            let fo = t.fo_tree.remove(&key).expect("a key of the tree");
            self.write_fontdictionary(&fo, &t)?;
        }
        Ok(())
    }
}
