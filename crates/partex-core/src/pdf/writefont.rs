//! writefont.c and writeenc.c: font dictionaries, font descriptors,
//! `/Widths` arrays and `/Encoding` differences, and the Type 1 file
//! streams from [`super::writet1`].
//!
//! pdfTeX keeps these in AVL trees (`fo_tree` by TFM name, `fd_tree` by
//! font file, slant and extend, `fe_tree` by encoding name) and writes
//! them in tree order at the end; `BTreeMap`s with the same keys give the
//! same order.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::enc::{GlyphNames, NOTDEF};
use super::writet1::{self, FONT_KEYS, T1Job};
use crate::fontmap::{F_INCLUDED, F_SUBSETTED, F_TRUETYPE, F_TYPE1, MapEntry};
use crate::host::{FileKind, Host};
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;
use partex_engine::scaled::Scaled;
use partex_ssa::Version;

/// A font descriptor (`fd_entry`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Fd {
    fm: Arc<MapEntry>,
    fontname: Vec<u8>,
    subset_tag: Option<[u8; 6]>,
    ff_found: bool,
    objnum: i32,
    font_dim: [(i32, bool); 11],
    /// Character codes of non-reencoded fonts.
    tx_tree: BTreeSet<u8>,
    /// Glyph names of reencoded (and, later, all) characters.
    gl_tree: BTreeSet<Vec<u8>>,
    /// The font file's own encoding (`builtin_glyph_names`).
    builtin: Option<Vec<Vec<u8>>>,
}

partex_engine::persist_struct!(Fd {
    fm,
    fontname,
    subset_tag,
    ff_found,
    objnum,
    font_dim,
    tx_tree,
    gl_tree,
    builtin
});

/// A font dictionary (`fo_entry`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Fo {
    fm: Arc<MapEntry>,
    objnum: i32,
    tex_font: i32,
    fd: (Vec<u8>, i32, i32),
    fe: Option<Vec<u8>>,
    first_char: i32,
    last_char: i32,
    cw_objnum: i32,
}

partex_engine::persist_struct!(Fo {
    fm,
    objnum,
    tex_font,
    fd,
    fe,
    first_char,
    last_char,
    cw_objnum
});

/// An encoding to write (`fe_entry` with its `fe_objnum`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Fe {
    names: GlyphNames,
    objnum: i32,
    tx_tree: BTreeSet<u8>,
}

partex_engine::persist_struct!(Fe {
    names,
    objnum,
    tx_tree
});

super::val::record_by_hash!(FontWriter);

/// The font trees (the `FONTW` field).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct FontWriter {
    fo_tree: BTreeMap<Vec<u8>, Fo>,
    fd_tree: BTreeMap<(Vec<u8>, i32, i32), Fd>,
    fe_tree: BTreeMap<Vec<u8>, Fe>,
    /// `st_tree`: subset tags given.
    tags: BTreeSet<[u8; 6]>,
    /// writet1.c's `lastargOtherSubr3`.
    last_arg_other_subr3: i32,
    /// `fixed_gen_tounicode`.
    pub gen_tounicode: i32,
}

partex_engine::persist_struct!(FontWriter {
    fo_tree,
    fd_tree,
    fe_tree,
    tags,
    last_arg_other_subr3,
    gen_tounicode
});

/// `FD_FLAGS_DEFAULT_EMBED`.
const FD_FLAGS_DEFAULT_EMBED: i32 = 4;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `getcharwidth` and friends: 0 for a missing character.
    fn glyph_dims(&self, f: i32, c: i32) -> (Scaled, Scaled, Scaled) {
        self.fonts
            .get(f)
            .glyph(c)
            .map_or((0, 0, 0), |g| (g.width, g.height, g.depth))
    }

    fn font_used_range(&self, f: i32) -> Option<(i32, i32)> {
        let font = self.fonts.get(f);
        let pf = &self.pdf.ship.fonts[crate::fonts::fx(f)];
        let marked = |c: i32| u8::try_from(c).is_ok_and(|c| pf.marked(c));
        let first = (font.bc..=font.ec).find(|&c| marked(c))?;
        let last = (font.bc..=font.ec).rev().find(|&c| marked(c))?;
        Some((first, last))
    }

    fn marked_in(&self, f: i32, first: i32, last: i32) -> impl Iterator<Item = u8> + '_ {
        let pf = &self.pdf.ship.fonts[crate::fonts::fx(f)];
        (first..=last)
            .filter_map(|c| u8::try_from(c).ok())
            .filter(move |&c| pf.marked(c))
    }

    /// `preset_fontmetrics`.
    fn preset_fontmetrics(&mut self, f: i32) -> Result<[(i32, bool); 11], Jump> {
        let size = self.pdf_font_ref(f).size;
        let font = self.fonts.get(f);
        let (slant, xh, quad) = (font.param(1), font.param(5), font.param(6));
        let mut d = [(0, true); 11];
        #[expect(
            clippy::cast_possible_truncation,
            reason = "C's conversion to an integer"
        )]
        let angle = (-atan(f64::from(slant) / 65536.0) * (180.0 / core::f64::consts::PI)) as i32;
        d[writet1::ITALIC_ANGLE].0 = self.divide_scaled(angle, size, 3)?.0;
        let h = self.glyph_dims(f, i32::from(b'h')).1;
        d[writet1::ASCENT].0 = self.divide_scaled(h, size, 3)?.0;
        let hh = self.glyph_dims(f, i32::from(b'H')).1;
        d[writet1::CAPHEIGHT].0 = self.divide_scaled(hh, size, 3)?.0;
        let i = -self
            .divide_scaled(self.glyph_dims(f, i32::from(b'y')).2, size, 3)?
            .0;
        d[writet1::DESCENT].0 = i.min(0);
        let w = self.glyph_dims(f, i32::from(b'.')).0;
        d[writet1::STEMV].0 = self.divide_scaled(w / 3, size, 3)?.0;
        d[writet1::XHEIGHT].0 = self.divide_scaled(xh, size, 3)?.0;
        let bb = writet1::FONTBBOX1;
        d[bb].0 = 0;
        d[bb + 1].0 = d[writet1::DESCENT].0;
        d[bb + 2].0 = self.divide_scaled(quad, size, 3)?.0;
        d[bb + 3].0 = d[writet1::CAPHEIGHT].0.max(d[writet1::ASCENT].0);
        d[writet1::FONTNAME] = (0, false);
        Ok(d)
    }

    /// writefont.c's `dopdffont`.
    pub(crate) fn do_pdf_font(&mut self, font_objnum: i32, f: i32) -> Result<(), Jump> {
        let Some((first, last)) = self.font_used_range(f) else {
            return Ok(());
        };
        let fm = match self.fm_entry(f) {
            Some(fm) if !fm.is(crate::fontmap::F_PK) => fm,
            _ => {
                return self.pdf_error(
                    b"font",
                    b"Type 3 (PK) fonts are not implemented in partex yet",
                );
            }
        };
        self.create_fontdictionary(&fm, font_objnum, f, first, last)
    }

    /// `create_fontdictionary`.
    fn create_fontdictionary(
        &mut self,
        fm: &Arc<MapEntry>,
        font_objnum: i32,
        f: i32,
        first: i32,
        last: i32,
    ) -> Result<(), Jump> {
        if !fm.is(F_TYPE1) {
            let what: &[u8] = if fm.is(F_TRUETYPE) {
                b"TrueType"
            } else {
                b"OpenType"
            };
            let mut m = what.to_vec();
            m.extend_from_slice(b" fonts are not implemented in partex yet");
            return self.pdf_error(b"font", &m);
        }
        let Some(ff_name) = fm.ff_name.clone() else {
            return self.pdf_error(
                b"font",
                b"non-embedded fonts are not implemented in partex yet",
            );
        };
        let codes: Vec<u8> = self.marked_in(f, first, last).collect();
        let mut fe = None;
        if let Some(enc) = fm.encname.clone() {
            let names = self.get_fe_entry(&enc)?;
            let need = self
                .pdf
                .fontw
                .fe_tree
                .get(&enc)
                .is_none_or(|e| e.objnum == 0);
            if need {
                let o = self.pdf_new_objnum()?;
                self.pdf.fontw.fe_tree.insert(
                    enc.clone(),
                    Fe {
                        names: names.clone(),
                        objnum: o,
                        tx_tree: BTreeSet::new(),
                    },
                );
            }
            if let Some(e) = self.pdf.fontw.fe_tree.get_mut(&enc) {
                e.tx_tree.extend(codes.iter().copied());
            }
            fe = Some((enc, names));
        }
        let key = (ff_name, fm.slant, fm.extend);
        if !self.pdf.fontw.fd_tree.contains_key(&key) {
            let fontname = fm.ps_name.clone().unwrap_or_else(|| fm.tfm_name.clone());
            let font_dim = self.preset_fontmetrics(f)?;
            let fd = Fd {
                fm: fm.clone(),
                fontname,
                subset_tag: None,
                ff_found: false,
                objnum: 0,
                font_dim,
                tx_tree: BTreeSet::new(),
                gl_tree: BTreeSet::new(),
                builtin: None,
            };
            self.pdf.fontw.fd_tree.insert(key.clone(), fd);
        }
        // the /Widths array
        let size = self.pdf_font_ref(f).size;
        let cw_objnum = self.pdf_new_objnum()?;
        self.pdf_begin_obj(cw_objnum, 1)?;
        self.pdf.out.print(b"[");
        for c in first..=last {
            let w = self.divide_scaled(self.glyph_dims(f, c).0, size, 4)?.0;
            // C's `/` and `%`: toward zero
            self.pdf.out.print_int(i64::from(w / 10));
            if w % 10 != 0 {
                self.pdf.out.print(b".");
                self.pdf.out.print_int(i64::from(w % 10));
            }
            if c != last {
                self.pdf.out.print(b" ");
            }
        }
        self.pdf.out.print_ln(b"]");
        self.pdf_end_obj();
        let fd = self
            .pdf
            .fontw
            .fd_tree
            .get_mut(&key)
            .expect("registered above");
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
        let fo = Fo {
            fm: fm.clone(),
            objnum: font_objnum,
            tex_font: f,
            fd: key,
            fe: fe.map(|(n, _)| n),
            first_char: first,
            last_char: last,
            cw_objnum,
        };
        self.pdf.fontw.fo_tree.insert(fm.tfm_name.clone(), fo);
        Ok(())
    }

    /// `writefontstuff`.
    ///
    /// Each entry written is a call whose hit is applied (DESIGN 7.17.3,
    /// "Hits applied inside a step that runs again", item 5), named by
    /// its tree's key and the entry.
    pub(crate) fn write_fontstuff(&mut self) -> Result<(), Jump> {
        use super::val::{WRITING, bit, field};
        use crate::ssa::Func;
        let fontw = bit(field::FONTW);
        let name = |entry: &dyn Fn() -> u128| if T::VALUES { entry() } else { 0 };
        let keys: Vec<_> = self.pdf.fontw.fd_tree.keys().cloned().collect();
        for k in keys {
            let n = name(&|| Version::of(&(&k, &self.pdf.fontw.fd_tree[&k])).0);
            self.applied_call(Func::FontFile, n, fontw, fontw | WRITING, |t| {
                t.write_fontdescriptor(&k)
            })?;
        }
        let encs: Vec<_> = self.pdf.fontw.fe_tree.keys().cloned().collect();
        for e in encs {
            let n = name(&|| Version::of(&(&e, &self.pdf.fontw.fe_tree[&e])).0);
            self.applied_call(Func::Encoding, n, fontw, WRITING, |t| t.write_enc(&e))?;
        }
        let fos: Vec<_> = self.pdf.fontw.fo_tree.values().cloned().collect();
        let dict_reads =
            fontw | bit(field::TOUNICODE) | bit(field::NOBUILTIN_TOUNICODE) | bit(field::FONT_ATTR);
        for fo in fos {
            let n = name(&|| Version::of(&(&fo.fm.tfm_name, &fo)).0);
            self.applied_call(Func::FontDict, n, dict_reads, WRITING, |t| {
                t.write_fontdictionary(&fo)
            })?;
        }
        Ok(())
    }

    /// `write_fontname`.
    fn write_fontname(&mut self, fd: &Fd, key: &[u8]) {
        self.pdf.out.print(b"/");
        self.pdf.out.print(key);
        self.pdf.out.print(b" /");
        if let Some(t) = &fd.subset_tag {
            self.pdf.out.print(t);
            self.pdf.out.print(b"+");
        }
        self.pdf.out.print_ln(&fd.fontname);
    }

    /// `write_fontfile` for Type 1: `writet1` (memoized, [`T1Memo`]) and
    /// the stream.
    fn write_fontfile(
        &mut self,
        key: &(Vec<u8>, i32, i32),
    ) -> Result<(i32, BTreeSet<Vec<u8>>), Jump> {
        let fd = self.pdf.fontw.fd_tree[key].clone();
        let found = self.host.read_file(&key.0, FileKind::Type1);
        if T::VALUES {
            self.tracker
                .load(&key.0, FileKind::Type1, found.as_ref().map(|f| &f.contents));
        }
        let Some(file) = found else {
            let name = key.0.clone();
            return self.pdftex_fail(Some(&name), b"cannot open Type 1 font file for reading");
        };
        let subsetted = fd.fm.is(F_SUBSETTED);
        self.print_str(if subsetted { b"<" } else { b"<<" });
        self.print_str(&file.name);
        // (content-keyed: the same font file, glyphs and tags give the
        // same subset, which a host may keep across rebuilds)
        let memo_key = {
            use core::hash::Hash;
            let mut h = partex_engine::stablehash::StableHasher::new();
            b"writet1".hash(&mut h);
            file.contents.hash(&mut h);
            (
                subsetted,
                fd.fm.slant,
                fd.fm.extend,
                &fd.gl_tree,
                &fd.tx_tree,
            )
                .hash(&mut h);
            (&fd.fontname, &fd.font_dim).hash(&mut h);
            let fw = &self.pdf.fontw;
            (&fw.tags, fw.last_arg_other_subr3).hash(&mut h);
            h.finish128()
        };
        let memo = self
            .host
            .cached(memo_key)
            .and_then(|m| m.downcast::<T1Memo>().ok());
        let memo = if let Some(m) = memo {
            m
        } else {
            let fw = &mut *self.pdf.fontw;
            let mut tags = fw.tags.clone();
            let mut last_arg = fw.last_arg_other_subr3;
            let mut job = T1Job {
                data: &file.contents,
                subsetted,
                slant: fd.fm.slant,
                extend: fd.fm.extend,
                glyphs: fd.gl_tree.clone(),
                codes: fd.tx_tree.clone(),
                fontname: fd.fontname.clone(),
                font_dim: fd.font_dim,
                tags: &mut tags,
                last_arg_other_subr3: &mut last_arg,
            };
            let result = writet1::writet1(&mut job);
            let m = alloc::sync::Arc::new(T1Memo {
                result,
                glyphs: job.glyphs,
                fontname: job.fontname,
                font_dim: job.font_dim,
                tags,
                last_arg,
            });
            self.host.cache(memo_key, m.clone());
            m
        };
        let fw = &mut *self.pdf.fontw;
        fw.tags.clone_from(&memo.tags);
        fw.last_arg_other_subr3 = memo.last_arg;
        let result = memo.result.clone();
        let (glyphs, fontname, font_dim) =
            (memo.glyphs.clone(), memo.fontname.clone(), memo.font_dim);
        let out = match result {
            Ok(o) => o,
            Err((m, warnings)) => {
                for w in warnings {
                    self.pdftex_warn(&w);
                }
                return self.pdftex_fail(Some(&file.name), &m);
            }
        };
        for w in &out.warnings {
            self.pdftex_warn(w);
        }
        self.print_str(if subsetted { b">" } else { b">>" });
        {
            let fd = self.pdf.fontw.fd_tree.get_mut(key).expect("a descriptor");
            fd.ff_found = true;
            fd.gl_tree.clone_from(&glyphs);
            fd.fontname = fontname;
            fd.font_dim = font_dim;
            fd.subset_tag = out.subset_tag;
            fd.builtin.clone_from(&out.builtin);
        }
        let ff_objnum = self.pdf_new_objnum()?;
        self.pdf_begin_dict(ff_objnum, 0)?;
        self.pdf.out.int_entry_ln(b"Length1", len64(out.length1));
        self.pdf.out.int_entry_ln(b"Length2", len64(out.length2));
        self.pdf.out.int_entry_ln(b"Length3", 0);
        self.pdf_begin_stream();
        for &b in &out.bytes {
            self.pdf.out.out(b);
        }
        self.pdf_end_stream();
        Ok((ff_objnum, glyphs))
    }

    /// `write_fontdescriptor`.
    fn write_fontdescriptor(&mut self, key: &(Vec<u8>, i32, i32)) -> Result<(), Jump> {
        let fm = self.pdf.fontw.fd_tree[key].fm.clone();
        let (ff_objnum, glyphs) = if fm.is(F_INCLUDED) {
            let (o, g) = self.write_fontfile(key)?;
            (Some(o), g)
        } else {
            return self.pdf_error(
                b"font",
                b"non-embedded fonts are not implemented in partex yet",
            );
        };
        let mut fd = self.pdf.fontw.fd_tree[key].clone();
        if fd.objnum == 0 {
            fd.objnum = self.pdf_new_objnum()?;
            self.pdf
                .fontw
                .fd_tree
                .get_mut(key)
                .expect("a descriptor")
                .objnum = fd.objnum;
        }
        self.pdf_begin_dict(fd.objnum, 1)?;
        self.pdf.out.print_ln(b"/Type /FontDescriptor");
        self.write_fontname(&fd, b"FontName");
        let flags = if fm.fd_flags == -1 {
            FD_FLAGS_DEFAULT_EMBED
        } else {
            fm.fd_flags
        };
        self.pdf.out.int_entry_ln(b"Flags", i64::from(flags));
        self.write_fontmetrics(&mut fd);
        if let Some(o) = ff_objnum {
            if self.int_par(PDF_OMIT_CHARSET_CODE) == 0 && fm.is(F_SUBSETTED) {
                self.pdf.out.print(b"/CharSet (");
                for g in &glyphs {
                    self.pdf.out.print(b"/");
                    self.pdf.out.print(g);
                }
                self.pdf.out.print_ln(b")");
            }
            self.pdf.out.indirect_ln(b"FontFile", o);
        }
        self.pdf_end_dict();
        Ok(())
    }

    /// `write_fontmetrics` (with `fix_fontmetrics`).
    fn write_fontmetrics(&mut self, fd: &mut Fd) {
        let p = &mut fd.font_dim;
        let bb = writet1::FONTBBOX1;
        if p[bb].1 && p[bb + 1].1 && p[bb + 2].1 && p[bb + 3].1 {
            for (k, from) in [
                (writet1::ASCENT, bb + 3),
                (writet1::DESCENT, bb + 1),
                (writet1::CAPHEIGHT, bb + 3),
            ] {
                if !p[k].1 {
                    p[k] = (p[from].0, true);
                }
            }
            let out = &mut *self.pdf.out;
            out.print(b"/FontBBox [");
            for i in 0..4 {
                out.print_int(i64::from(p[bb + i].0));
                out.print(if i < 3 { b" " } else { b"]\n" });
            }
        } else {
            let mut m = b"font `".to_vec();
            m.extend_from_slice(fd.fm.ff_name.as_deref().unwrap_or_default());
            m.extend_from_slice(b"' doesn't have a BoundingBox");
            self.pdftex_warn(&m);
        }
        for (&(v, set), key) in fd.font_dim.iter().zip(FONT_KEYS).take(writet1::XHEIGHT + 1) {
            if set {
                self.pdf.out.int_entry_ln(key.0.as_bytes(), i64::from(v));
            }
        }
    }

    /// writeenc.c's `write_enc`.
    fn write_enc(&mut self, name: &[u8]) -> Result<(), Jump> {
        let fe = self.pdf.fontw.fe_tree[name].clone();
        if fe.objnum == 0 {
            return Ok(());
        }
        self.pdf_begin_dict(fe.objnum, 1)?;
        self.pdf.out.print_ln(b"/Type /Encoding");
        self.pdf.out.print(b"/Differences [");
        let mut old = -2;
        for &c in &fe.tx_tree {
            let c = i32::from(c);
            if c != old + 1 {
                if old != -2 {
                    self.pdf.out.print(b" ");
                }
                self.pdf.out.print_int(i64::from(c));
            }
            self.pdf.out.print(b"/");
            self.pdf.out.print(&fe.names[crate::input::ux(c)]);
            old = c;
        }
        self.pdf.out.print_ln(b"]");
        self.pdf_end_dict();
        Ok(())
    }

    /// `write_fontdictionary`.
    fn write_fontdictionary(&mut self, fo: &Fo) -> Result<(), Jump> {
        let fd = self.pdf.fontw.fd_tree[&fo.fd].clone();
        let mut tounicode = 0;
        if (self.pdf.fontw.gen_tounicode > 0
            && !self.pdf.nobuiltin_tounicode.contains(&fo.tex_font))
            || fo.fm.tfm_name == b"dummy-space"
        {
            if let Some(e) = &fo.fe {
                let names = self.pdf.fontw.fe_tree[e].names.clone();
                tounicode = self.write_tounicode(&names, &fo.fm.tfm_name, Some(e))?;
            } else {
                let Some(names) = fd.builtin.clone() else {
                    return self.pdftex_fail(None, b"builtin glyph names is empty");
                };
                tounicode = self.write_tounicode(&names, &fo.fm.tfm_name, None)?;
            }
        }
        self.pdf_begin_dict(fo.objnum, 1)?;
        self.pdf.out.print_ln(b"/Type /Font");
        self.pdf.out.print_ln(b"/Subtype /Type1");
        self.write_fontname(&fd, b"BaseFont");
        self.pdf.out.indirect_ln(b"FontDescriptor", fd.objnum);
        self.pdf
            .out
            .int_entry_ln(b"FirstChar", i64::from(fo.first_char));
        self.pdf
            .out
            .int_entry_ln(b"LastChar", i64::from(fo.last_char));
        self.pdf.out.indirect_ln(b"Widths", fo.cw_objnum);
        if let Some(e) = &fo.fe
            && let Some(o) = self.pdf.fontw.fe_tree.get(e).map(|e| e.objnum)
            && o != 0
        {
            self.pdf.out.indirect_ln(b"Encoding", o);
        }
        if tounicode != 0 {
            self.pdf.out.indirect_ln(b"ToUnicode", tounicode);
        }
        if let Some(a) = self.pdf.font_attr.get(&fo.tex_font).cloned()
            && !a.is_empty()
        {
            self.pdf.out.print(&a);
            self.pdf.out.print(b"\n");
        }
        self.pdf_end_dict();
        Ok(())
    }
}

fn len64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// `atan` for `preset_fontmetrics` (no `std`).
fn atan(x: f64) -> f64 {
    writet1::atan(x)
}

/// What `writet1` made of a font (see [`Host::cached`]): its result, and
/// what it changed of the descriptor and the writer.
///
/// [`Host::cached`]: crate::host::Host::cached
struct T1Memo {
    result: Result<writet1::T1Out, (writet1::Fail, Vec<Vec<u8>>)>,
    glyphs: BTreeSet<Vec<u8>>,
    fontname: Vec<u8>,
    font_dim: [(i32, bool); 11],
    tags: BTreeSet<[u8; 6]>,
    last_arg: i32,
}
