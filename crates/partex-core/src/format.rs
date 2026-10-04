//! Part 50: Dumping and undumping the tables (§1299–§1329).
//!
//! The format file is partex's own layout (little-endian, sections in
//! tex.web's order), not web2c's: only what TeX *prints* while dumping, and
//! the state it leaves behind, must match the reference. Arrays are stored
//! whole where tex.web compresses them (`eqtb` runs, free `mem` blocks), so
//! loading is a straight copy.

use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::codec::{Dec, Enc};
use partex_engine::font::Font;

use crate::cmds::*;
use crate::error::{BATCH_MODE, ERROR_STOP_MODE};
use crate::host::{FileKind, Host};
use crate::hyph::HYPH_PRIME;
use crate::input::ux;
use crate::mem::{MAX_HALFWORD, MemoryWord};
use crate::objs::Shaped;
use crate::pdfconv::ToUnicode;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// Identifies a partex format file (and its layout version).
const MAGIC: &[u8; 8] = b"PTXFMT04";
/// §1326: the closing check word.
const CHECK_WORD: i32 = 69069;

/// A little-endian writer.
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn int(&mut self, x: i32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn ints(&mut self, xs: &[i32]) {
        self.int(i32::try_from(xs.len()).unwrap_or(i32::MAX));
        for &x in xs {
            self.int(x);
        }
    }
    fn words(&mut self, ws: &[MemoryWord]) {
        self.int(i32::try_from(ws.len()).unwrap_or(i32::MAX));
        for w in ws {
            self.0.extend_from_slice(&w.to_bits().to_le_bytes());
        }
    }
    fn bytes(&mut self, bs: &[u8]) {
        self.int(i32::try_from(bs.len()).unwrap_or(i32::MAX));
        self.0.extend_from_slice(bs);
    }
}

/// The matching reader; `None` means a bad format file.
struct In<'a> {
    data: &'a [u8],
    pos: usize,
}

impl In<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    fn int(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn len(&mut self) -> Option<usize> {
        usize::try_from(self.int()?).ok()
    }
    fn ints(&mut self) -> Option<Vec<i32>> {
        let n = self.len()?;
        (0..n).map(|_| self.int()).collect()
    }
    fn words(&mut self) -> Option<Vec<MemoryWord>> {
        let n = self.len()?;
        (0..n)
            .map(|_| {
                let b = self.take(8)?;
                Some(MemoryWord::from_bits(u64::from_le_bytes(
                    b.try_into().ok()?,
                )))
            })
            .collect()
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.len()?;
        Some(self.take(n)?.to_vec())
    }
    /// `undump(lo)(hi)(x)`.
    fn int_in(&mut self, lo: i32, hi: i32) -> Option<i32> {
        let x = self.int()?;
        (lo..=hi).contains(&x).then_some(x)
    }
}

fn to_i32(x: usize) -> i32 {
    i32::try_from(x).unwrap_or(i32::MAX)
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §1302: `\dump` (INITEX).
    pub(crate) fn store_fmt_file(&mut self) -> Result<(), Jump> {
        // §1304: if dumping is not allowed, abort.
        if self.save_ptr() != 0 {
            self.print_err(b"You can't dump inside a group");
            self.help(&[b"`{...\\dump}' is a no-no."]);
            return self.succumb();
        }
        // §1328: create the `format_ident`, open the format file, and
        // inform the user that dumping has begun.
        self.set_selector(NEW_STRING);
        self.print_str(b" (preloaded format=");
        self.print(self.job_name());
        self.print_char(b' ');
        // (the job's start, the host's answer: a `Clock` read where it is
        // printed, DESIGN 7.17.12's `sys_*`)
        let day = (self.sys_year, self.sys_month, self.sys_day);
        self.clock_read(crate::track::Query::Now, &day);
        self.print_int(self.sys_year);
        self.print_char(b'.');
        self.print_int(self.sys_month);
        self.print_char(b'.');
        self.print_int(self.sys_day);
        self.print_char(b')');
        self.set_selector(if self.interaction() == BATCH_MODE {
            LOG_ONLY
        } else {
            TERM_AND_LOG
        });
        self.str_room(1)?;
        self.format_ident = to_i32(self.make_string()?);
        self.pack_job_name(b".fmt");
        let id = loop {
            let name = self.name_of_file.clone();
            if let Some((id, printed)) = self.host.open_write(&name, FileKind::Fmt) {
                self.name_of_file = printed;
                break id;
            }
            self.prompt_file_name(b"format file name", b".fmt")?;
        };
        self.print_nl(b"Beginning to dump on file ");
        let s = self.make_name_string()?; // `w_make_name_string`
        self.slow_print(s);
        self.flush_string();
        self.print_nl(b"");
        self.slow_print(self.format_ident);

        let mut out = Out::default();
        out.0.extend_from_slice(MAGIC);
        // §1307: constants for a consistency check.
        for x in [
            MAX_HALFWORD,
            self.hash_high,
            EQTB_SIZE,
            HASH_PRIME,
            HYPH_PRIME,
        ] {
            out.int(x);
        }
        // pdfTeX §1654: the e-TeX mode; the optional features are off in
        // every format.
        out.int(i32::from(self.etex_mode));
        for j in 0..ETEX_STATES {
            self.set_int_par(ETEX_STATE_CODE + j, 0);
        }
        out.int(i32::from(self.mltex_enabled_p));
        out.bytes(&self.xord);
        out.bytes(&self.xchr);
        out.bytes(&self.xprn.iter().map(|&b| u8::from(b)).collect::<Vec<_>>());

        // §1309: the string pool.
        out.bytes(&self.str_pool[..self.pool_ptr]);
        out.ints(
            &self.str_start[..=self.str_ptr]
                .iter()
                .map(|&s| to_i32(s))
                .collect::<Vec<_>>(),
        );
        self.print_ln();
        self.print_int(to_i32(self.str_ptr));
        self.print_str(b" strings of total length ");
        self.print_int(to_i32(self.pool_ptr));

        // §1311: the dynamic memory: the values eqtb's entries hold beside
        // their words (`objs.rs`), by location: token lists, glue, boxes
        // and shapes.
        let mut e = Enc::default();
        let mut held: Vec<(i32, crate::objs::Obj)> = self
            .eqtb_obj
            .iter()
            .enumerate()
            .filter_map(|(p, o)| Some((i32::try_from(p).ok()?, o.clone()?)))
            .collect();
        held.extend(self.xregs.objs().map(|(p, o)| (p, o.clone())));
        e.count(held.len());
        for (p, o) in &held {
            e.i32(*p);
            match o {
                crate::objs::Obj::Toks(t) => {
                    e.u8(0);
                    e.u8(u8::from(t.protected()));
                    e.ints(t.tokens());
                }
                crate::objs::Obj::Glue(g) => {
                    e.u8(1);
                    e.glue(&g.spec);
                }
                crate::objs::Obj::Box(b) => {
                    e.u8(2);
                    e.box_node(b);
                }
                crate::objs::Obj::Shape(sh) => {
                    e.u8(3);
                    match &**sh {
                        Shaped::Lines(s) => {
                            e.i32(0);
                            e.ints(&s.iter().flat_map(|&(i, l)| [i, l]).collect::<Vec<_>>());
                        }
                        Shaped::Penalties(p) => {
                            e.i32(1);
                            e.ints(p);
                        }
                    }
                }
            }
        }
        out.bytes(&e.0);
        let (var_used, dyn_used) = self.memory_usage();
        self.print_ln();
        self.print_int(var_used + dyn_used);
        self.print_str(b" memory locations dumped; current usage is ");
        self.print_int(var_used);
        self.print_char(b'&');
        self.print_int(dyn_used);

        // §1313: the table of equivalents and the hash table.
        out.words(&self.eqtb.to_vec(0..self.eqtb.len()));
        // e-TeX's registers above 255 that differ from their default.
        let cells: Vec<_> = self.xregs.cells().collect();
        out.ints(
            &cells
                .iter()
                .flat_map(|&(l, _, x)| [l, x])
                .collect::<Vec<_>>(),
        );
        out.words(&cells.iter().map(|c| c.1).collect::<Vec<_>>());
        out.int(self.par_loc);
        out.int(self.write_loc);
        out.int(self.hash_used);
        self.cs_count = FROZEN_CONTROL_SEQUENCE - 1 - self.hash_used + self.hash_high;
        for p in HASH_BASE..=self.hash_used {
            if self.text(p) != 0 {
                self.cs_count += 1;
            }
        }
        out.words(&self.hash.to_vec(0..self.hash.len()));
        out.int(self.cs_count);
        // pdfTeX §1318: the primitive table (with e-TeX's primitives).
        out.ints(&self.prims.next);
        out.ints(&self.prims.text);
        out.int(self.prims.used);
        self.print_ln();
        self.print_int(self.cs_count);
        self.print_str(b" multiletter control sequences");

        // §1320: the font information: each font as its TFM file, size and
        // (possibly changed) parameters.
        out.int(self.font_ptr);
        out.int(self.fmem_ptr);
        let mut e = Enc::default();
        for k in NULL_FONT..=self.font_ptr {
            let f = &self.fonts;
            let i = ux(k);
            e.bytes(&f.tfm[i]);
            e.i32(f.metrics[i].size);
            e.ints(&f.metrics[i].params);
            match &f.glue[i] {
                Some(g) => {
                    e.u8(1);
                    e.glue(g);
                }
                None => e.u8(0),
            }
            for x in [f.hyphen_char[i], f.skew_char[i], f.name[i], f.area[i]] {
                e.i32(x);
            }
        }
        out.bytes(&e.0);
        for k in NULL_FONT..=self.font_ptr {
            // §1322
            self.print_nl(b"\\font");
            self.print_esc_num(self.font_id_text(k));
            self.print_char(b'=');
            let (nm, ar) = (self.fonts.name[ux(k)], self.fonts.area[ux(k)]);
            self.print_file_name(nm, ar, self.pool_str(b""));
            let font = self.fonts.get(k);
            let (size, design_size) = (font.size, font.design_size);
            if size != design_size {
                self.print_str(b" at ");
                self.print_scaled(size);
                self.print_str(b"pt");
            }
        }

        self.print_ln();
        self.print_int(self.fmem_ptr - 7);
        self.print_str(b" words of font info for ");
        self.print_int(self.font_ptr - FONT_BASE);
        if self.font_ptr == FONT_BASE + 1 {
            self.print_str(b" preloaded font");
        } else {
            self.print_str(b" preloaded fonts");
        }

        // §1324: the hyphenation tables.
        self.exceptions_read();
        self.patterns_read();
        let h = &mut self.hyph;
        out.int(h.hyph_count);
        if h.hyph_next <= HYPH_PRIME {
            h.hyph_next = self.params.hyph_size;
        }
        out.int(h.hyph_next);
        out.ints(&h.hyph_word);
        out.ints(&h.hyph_link);
        let mut e = Enc::default();
        e.count(h.exceptions.len());
        for (word, positions) in h.exceptions.sorted() {
            e.bytes(word);
            e.bytes(positions);
        }
        self.exceptions_wrote();
        out.bytes(&e.0);
        self.print_ln();
        self.print_int(self.hyph.hyph_count);
        if self.hyph.hyph_count == 1 {
            self.print_str(b" hyphenation exception");
        } else {
            self.print_str(b" hyphenation exceptions");
        }
        if self.hyph.trie_not_ready {
            self.init_trie()?;
        }
        let h = &self.hyph;
        let tm = ux(h.trie_max) + 1;
        let ops = ux(h.trie_op_ptr) + 1;
        out.int(h.trie_max);
        out.ints(&h.patterns.link[..tm]);
        out.ints(&h.patterns.op[..tm]);
        out.ints(&h.patterns.ch[..tm]);
        out.int(h.trie_op_ptr);
        out.ints(&h.patterns.distance[..ops]);
        out.ints(&h.patterns.num[..ops]);
        out.ints(&h.patterns.next[..ops]);
        out.ints(&h.trie_used);
        out.ints(&h.patterns.op_start);
        // e-TeX: the saved hyphenation codes.
        out.int(i32::try_from(h.patterns.hyph_codes.len()).unwrap_or(0));
        for (&lang, codes) in &h.patterns.hyph_codes {
            out.int(i32::from(lang));
            out.ints(&codes.iter().map(|&c| i32::from(c)).collect::<Vec<_>>());
        }
        self.print_nl(b"Hyphenation trie of length ");
        self.print_int(self.hyph.trie_max);
        self.print_str(b" has ");
        self.print_int(self.hyph.trie_op_ptr);
        if self.hyph.trie_op_ptr == 1 {
            self.print_str(b" op");
        } else {
            self.print_str(b" ops");
        }
        self.print_str(b" out of ");
        self.print_int(crate::hyph::TRIE_OP_SIZE);
        for k in (0..=255).rev() {
            let used = self.hyph.trie_used[ux(k)];
            if used > MIN_QUARTERWORD {
                self.print_nl(b"  ");
                self.print_int(used);
                self.print_str(b" for language ");
                self.print_int(k);
            }
        }

        if self.params.flavor == crate::params::Flavor::PdfTex {
            // pdfTeX §1325: its (so far empty) PDF memory and objects.
            self.print_ln();
            self.print_int(0);
            self.print_str(b" words of pdfTeX memory");
            self.print_ln();
            self.print_int(0);
            self.print_str(b" indirect objects");
        }

        // pdfTeX §1587: LaTeX loads glyphtounicode.tex while making its
        // format; the resulting mappings must survive the dump.
        out.int(to_i32(self.tounicode.len()));
        for (glyph, value) in self.tounicode.sorted() {
            out.bytes(glyph);
            match value {
                ToUnicode::Code(code) => {
                    out.int(0);
                    out.int(i32::try_from(*code).unwrap_or(i32::MAX));
                }
                ToUnicode::Text(text) => {
                    out.int(1);
                    out.bytes(text);
                }
                ToUnicode::Undefined => out.int(2),
            }
        }

        // §1326: a couple more things and the closing check word.
        out.int(self.interaction);
        out.int(self.format_ident);
        out.int(CHECK_WORD);
        self.set_int_par(TRACING_STATS_CODE, 0);
        // §1329: close the format file.
        self.host.write(id, &out.0);
        self.host.close(id);
        Ok(())
    }

    /// §1303: load the format file `data`; `false` (after the message) if
    /// it is unacceptable.
    pub(crate) fn load_fmt_file(&mut self, data: &[u8]) -> bool {
        let loaded = self.undump(data);
        // (the format's fonts are numbered by the table: the run's are made
        // after them)
        self.fonts.count = self.font_ptr;
        // (the tables and the token lists were stored wholesale, past their
        // accessors: their versions, as the accessors make them at a write)
        self.version_tables();
        if loaded.is_none() {
            self.term_bytes(b"(Fatal format file error; I'm stymied)\n");
            return false;
        }
        true
    }

    fn undump(&mut self, data: &[u8]) -> Option<()> {
        let mut r = In { data, pos: 0 };
        if r.take(8)? != MAGIC {
            return None;
        }
        // §1308: constants for a consistency check.
        if r.int()? != MAX_HALFWORD {
            return None;
        }
        let hash_high = r.int_in(0, self.params.hash_extra)?;
        if r.int()? != EQTB_SIZE || r.int()? != HASH_PRIME || r.int()? != HYPH_PRIME {
            return None;
        }
        self.hash_high = hash_high;
        // pdfTeX §1655
        self.etex_mode = match r.int()? {
            0 => false,
            1 => true,
            _ => return None,
        };
        if self.etex_mode {
            self.init_etex_extended();
        } else {
            self.init_etex_compat();
        }
        self.mltex_enabled_p = r.int()? != 0;
        // tex.ch: a TCX file of the command line wins over the format's
        // tables; `-8bit` makes every character printable
        let (xord, xchr, xprn) = (r.bytes()?, r.bytes()?, r.bytes()?);
        if self.params.translation.is_none() {
            self.xord.copy_from_slice(xord.get(..256)?);
            self.xchr.copy_from_slice(xchr.get(..256)?);
            for (d, s) in self.xprn.iter_mut().zip(xprn) {
                *d = s != 0 || self.params.eight_bit;
            }
        }
        // §1310: the string pool.
        let pool = r.bytes()?;
        let starts = r.ints()?;
        if pool.len() + ux(self.params.pool_free) > self.str_pool.len() {
            self.str_pool
                .resize(pool.len() + ux(self.params.pool_free), 0);
        }
        self.str_pool[..pool.len()].copy_from_slice(&pool);
        self.pool_ptr = pool.len();
        self.str_ptr = starts.len().checked_sub(1)?;
        if self.str_start.len() < starts.len() + ux(self.params.strings_free) {
            self.str_start
                .resize(starts.len() + ux(self.params.strings_free), 0);
        }
        for (d, &s) in self.str_start.iter_mut().zip(&starts) {
            *d = usize::try_from(s).ok().filter(|&s| s <= pool.len())?;
        }
        self.init_str_ptr = self.str_ptr;
        self.init_pool_ptr = self.pool_ptr;
        self.str_index.clear();
        self.pool_ready();

        // §1312: the dynamic memory: the values entries hold, put in
        // place once eqtb is loaded (each glue a new lineage, as each was
        // its entry's own).
        let objs = r.bytes()?;
        let mut d = Dec::new(&objs);
        let n = d.count()?;
        let mut held: Vec<(i32, crate::objs::Obj)> = Vec::with_capacity(n);
        for _ in 0..n {
            let p = d.i32()?;
            let o = match d.u8()? {
                0 => {
                    let protected = d.u8()? == 1;
                    let toks = d.ints()?;
                    crate::objs::Obj::Toks(Arc::new(crate::tok::TokenList::new(toks, protected)))
                }
                1 => {
                    let spec = d.glue()?;
                    crate::objs::Obj::Glue(self.new_glue_value(spec, None))
                }
                2 => crate::objs::Obj::Box(d.box_node()?.share()),
                3 => {
                    let kind = d.i32()?;
                    let v = d.ints()?;
                    crate::objs::Obj::Shape(Arc::new(if kind == 0 {
                        Shaped::Lines(v.as_chunks::<2>().0.iter().map(|&[i, l]| (i, l)).collect())
                    } else {
                        Shaped::Penalties(Arc::from(v))
                    }))
                }
                _ => return None,
            };
            if p <= 0 {
                return None;
            }
            held.push((p, o));
        }

        // §1314: the table of equivalents and the hash table.
        let eqtb = r.words()?;
        if eqtb.len() > self.eqtb.len() {
            return None;
        }
        self.eqtb.copy_from(0, &eqtb);
        let (cells, words) = (r.ints()?, r.words()?);
        if cells.len() != 2 * words.len() {
            return None;
        }
        self.xregs = crate::xregs::ExtRegs::new(self.params.fast & crate::params::FAST_XREGS != 0);
        for (c, w) in cells.chunks(2).zip(words) {
            if c[0] < crate::xregs::EXT_BASE {
                return None;
            }
            self.xregs.set(c[0], w);
            self.xregs.set_level(c[0], c[1]);
        }
        self.eqtb_obj = alloc::vec![None; self.eqtb.len()];
        for (p, o) in held {
            if p >= crate::xregs::EXT_BASE {
                self.xregs.set_obj(p, Some(o));
            } else {
                *self.eqtb_obj.get_mut(ux(p))? = Some(o);
            }
        }
        self.par_loc = r.int_in(HASH_BASE, self.hash_top)?;
        self.par_token = CS_TOKEN_FLAG + self.par_loc;
        self.write_loc = r.int_in(HASH_BASE, self.hash_top)?;
        self.hash_used = r.int_in(HASH_BASE, FROZEN_CONTROL_SEQUENCE)?;
        let hash = r.words()?;
        if hash.len() > self.hash.len() {
            return None;
        }
        self.hash.copy_from(0, &hash);
        self.cs_count = r.int()?;
        let (next, text) = (r.ints()?, r.ints()?);
        if next.len() != self.prims.next.len() || text.len() != self.prims.text.len() {
            return None;
        }
        self.prims.next = alloc::sync::Arc::new(next);
        self.prims.text = alloc::sync::Arc::new(text);
        self.prims.used = r.int()?;
        self.prims.by_meaning = None;

        // §1321: the font information.
        self.font_ptr = r.int_in(FONT_BASE, self.params.font_max)?;
        self.fmem_ptr = r.int()?;
        let fonts = r.bytes()?;
        let mut d = Dec::new(&fonts);
        self.fonts.clear();
        for _ in NULL_FONT..=self.font_ptr {
            let tfm: Arc<[u8]> = Arc::from(d.bytes()?);
            let size = d.i32()?;
            let params = d.ints()?;
            let glue = match d.u8()? {
                0 => None,
                _ => Some(d.glue()?),
            };
            let [hyphen_char, skew_char, name, area] = [d.i32()?, d.i32()?, d.i32()?, d.i32()?];
            let mut font = if tfm.is_empty() {
                Font::null()
            } else {
                Font::from_tfm(&tfm, Some(size)).ok()?
            };
            font.params = params;
            let k = self.fonts.metrics.len();
            let f = i32::try_from(k).unwrap_or(0);
            // (the metrics by identity, for a tracker that keeps versions:
            // `Tex::version_tables` versions the fonts once they are in)
            let idv = if T::VALUES {
                crate::fonts::FontIdent::Tfm {
                    tfm: &tfm,
                    name: self.str_bytes(ux(name)),
                    area: self.str_bytes(ux(area)),
                    size,
                }
                .content(&self.fonts)
            } else {
                0
            };
            self.fonts.place(f, font, tfm, name, area, 0, idv);
            self.font_loaded(f);
            // (a machine's: the fonts of this name changed, as loading one
            // changes them; a region that begins with loading the format
            // and searches them later would otherwise read them as they
            // were at its start)
            if f > NULL_FONT {
                self.font_named(name);
            }
            self.fonts.glue[k] = glue;
            self.fonts.hyphen_char[k] = hyphen_char;
            self.fonts.skew_char[k] = skew_char;
        }

        // §1325: the hyphenation tables.
        let h = &mut self.hyph;
        h.hyph_count = r.int()?;
        let _dumped_next = r.int()?;
        let words = r.ints()?;
        let links = r.ints()?;
        if words.len() != h.hyph_word.len() || links.len() != h.hyph_link.len() {
            return None;
        }
        alloc::sync::Arc::make_mut(&mut h.hyph_word).copy_from_slice(&words);
        alloc::sync::Arc::make_mut(&mut h.hyph_link).copy_from_slice(&links);
        let exceptions = r.bytes()?;
        let mut d = Dec::new(&exceptions);
        let n = d.count()?;
        h.exceptions.clear();
        for _ in 0..n {
            let word = d.bytes()?.to_vec();
            let positions = d.bytes()?.to_vec();
            h.exceptions.insert(word, positions);
        }
        // `hyph_next`: one past the largest occupied slot, as in §1325.
        let mut j = words
            .iter()
            .rposition(|&w| w != 0)
            .map_or(0, |j| to_i32(j) + 1);
        if j < HYPH_PRIME {
            j = HYPH_PRIME;
        }
        h.hyph_next = j;
        if h.hyph_next >= self.params.hyph_size {
            h.hyph_next = HYPH_PRIME;
        } else if h.hyph_next >= HYPH_PRIME {
            h.hyph_next += 1;
        }
        h.trie_max = r.int()?;
        let (trl, tro, trc) = (r.ints()?, r.ints()?, r.ints()?);
        h.pat().link[..trl.len()].copy_from_slice(&trl);
        h.pat().op[..tro.len()].copy_from_slice(&tro);
        h.pat().ch[..trc.len()].copy_from_slice(&trc);
        h.trie_op_ptr = r.int()?;
        let (d, nn, nx) = (r.ints()?, r.ints()?, r.ints()?);
        h.pat().distance[..d.len()].copy_from_slice(&d);
        h.pat().num[..nn.len()].copy_from_slice(&nn);
        h.pat().next[..nx.len()].copy_from_slice(&nx);
        h.trie_used.copy_from_slice(&r.ints()?);
        let op_start = r.ints()?;
        if op_start.len() != h.patterns.op_start.len() {
            return None;
        }
        h.pat().op_start = op_start;
        let mut hyph_codes = alloc::collections::BTreeMap::new();
        for _ in 0..r.int_in(0, 256)? {
            let lang = u8::try_from(r.int_in(0, 255)?).ok()?;
            let codes = r.ints()?;
            if codes.len() != 256 {
                return None;
            }
            hyph_codes.insert(
                lang,
                codes
                    .iter()
                    .map(|&c| u8::try_from(c).unwrap_or(0))
                    .collect(),
            );
        }
        h.pat().hyph_codes = hyph_codes;
        h.trie_not_ready = false;
        // (the patterns' version is made from them, `Tex::version_hyph`)
        h.pat_ver = 0;

        self.tounicode.clear();
        for _ in 0..r.len()? {
            let glyph = r.bytes()?;
            let value = match r.int()? {
                0 => ToUnicode::Code(u32::try_from(r.int_in(0, 0x10_FFFF)?).ok()?),
                1 => ToUnicode::Text(r.bytes()?),
                2 => ToUnicode::Undefined,
                _ => return None,
            };
            if self.tounicode.insert(glyph, value).is_some() {
                return None;
            }
        }

        // §1327: a couple more things and the closing check word.
        self.interaction = r.int_in(BATCH_MODE, ERROR_STOP_MODE)?;
        if let Some(i) = self.params.interaction {
            self.interaction = i;
        }
        self.format_ident = r.int_in(0, to_i32(self.str_ptr))?;
        (r.int()? == CHECK_WORD).then_some(())
    }
}
