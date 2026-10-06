//! type1.c, type1.h: Type 1 fonts, embedded as CFF (FontFile3/Type1C).
//!
//! `pdf_font *` is the font's id in `self.font.fonts`.

use crate::cff::*;
use crate::cff_dict::cff_dict_update;
use crate::dpxfile::ResType;
use crate::fmt::round_acc;
use crate::obj::STREAM_COMPRESS;
use crate::pdffont::{PDF_FONT_FLAG_BASEFONT, PDF_FONT_FLAG_NOEMBED, PDF_FONT_FONTTYPE_TYPE1};
use crate::prelude::*;
use crate::t1_char::T1Ginfo;
use crate::t1_load::{is_pfb, t1_get_fontname, t1_get_standard_glyph};

/// Fixed-width font.
pub const FONT_FLAG_FIXEDPITCH: i32 = 1 << 0;
/// Serif font.
pub const FONT_FLAG_SERIF: i32 = 1 << 1;
/// Symbolic font.
pub const FONT_FLAG_SYMBOLIC: i32 = 1 << 2;
/// Script font.
pub const FONT_FLAG_SCRIPT: i32 = 1 << 3;
/// Adobe Standard Character Set.
pub const FONT_FLAG_STANDARD: i32 = 1 << 5;
/// Italic.
pub const FONT_FLAG_ITALIC: i32 = 1 << 6;
/// All-cap font.
pub const FONT_FLAG_ALLCAP: i32 = 1 << 16;
/// Small-cap font.
pub const FONT_FLAG_SMALLCAP: i32 = 1 << 17;
/// Force bold at small text sizes.
pub const FONT_FLAG_FORCEBOLD: i32 = 1 << 18;

/// `basefonts` of `is_basefont`: the 14 standard fonts.
pub static BASEFONTS: [&[u8]; 14] = [
    b"Courier",
    b"Courier-Bold",
    b"Courier-Oblique",
    b"Courier-BoldOblique",
    b"Helvetica",
    b"Helvetica-Bold",
    b"Helvetica-Oblique",
    b"Helvetica-BoldOblique",
    b"Symbol",
    b"Times-Roman",
    b"Times-Bold",
    b"Times-Italic",
    b"Times-BoldItalic",
    b"ZapfDingbats",
];

/// `is_basefont` (static).
fn is_basefont(name: &[u8]) -> bool {
    BASEFONTS.iter().any(|&b| b == name)
}

/// `MAX_GLYPHS` (pdf_font_load_type1).
const MAX_GLYPHS: usize = 1024;
/// `WBUF_SIZE` (write_fontfile).
const WBUF_SIZE: usize = 1024;

/// `strstr(s, t) != NULL`.
pub(crate) fn strstr(s: &[u8], t: &[u8]) -> bool {
    t.is_empty() || s.windows(t.len()).any(|w| w == t)
}

/// `cff_glyph_lookup(cff, glyph)` with C's NULL glyph (.notdef, 0).
pub(crate) fn glyph_lookup(cff: &CffFont, glyph: Option<&[u8]>) -> Result<Card16> {
    match glyph {
        None => Ok(0),
        Some(g) => cff.cff_glyph_lookup(g),
    }
}

/// `sprintf(fullname, "%6s+%s", uniqueTag, fontname)`.
pub(crate) fn subset_fullname(unique_tag: &[u8], fontname: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(fontname.len() + 8);
    for _ in unique_tag.len()..6 {
        v.push(b' ');
    }
    v.extend_from_slice(unique_tag);
    v.push(b'+');
    v.extend_from_slice(fontname);
    v
}

/// `font->topdict` (present in every font these files handle).
fn topdict(cffont: &mut CffFont) -> &mut CffDict {
    cffont.topdict.as_mut().expect("topdict")
}

/// The charstring of `gid` in `cs` (C's `data + offset[gid] - 1`, its
/// length `offset[gid + 1] - offset[gid]`).
fn charstring(cs: &CffIndex, gid: usize) -> &[u8] {
    let start = cs.offset[gid] as usize - 1;
    let len = cs.offset[gid + 1].wrapping_sub(cs.offset[gid]) as usize;
    &cs.data[start..start + len]
}

impl Dpx {
    /// `pdf_font_open_type1`: 0 ok, -1 not a Type 1 font.
    pub fn pdf_font_open_type1(
        &mut self,
        font_id: i32,
        ident: &[u8],
        index: i32,
        encoding_id: i32,
        embedding: i32,
    ) -> Result<i32> {
        let fid = font_id as usize;

        if index != 0 {
            warn!("Ignoring non-zero font index.");
        }

        if is_basefont(ident) {
            let font = &mut self.font.fonts[fid];
            font.fontname = Some(ident.to_vec());
            font.subtype = PDF_FONT_FONTTYPE_TYPE1;
            font.flags |= PDF_FONT_FLAG_NOEMBED;
            font.flags |= PDF_FONT_FLAG_BASEFONT;
        } else {
            let Some(mut fp) = self.dpx_open_file(ident, ResType::T1Font)? else {
                return Ok(-1);
            };

            if embedding == 0 {
                warn!("Ignoring no-embed option for Type1 font.");
                self.font.fonts[fid].flags &= !PDF_FONT_FLAG_NOEMBED;
            }
            let mut fontname = Vec::new();
            if is_pfb(&mut fp) == 0 || {
                let (st, name) = t1_get_fontname(&mut fp)?;
                fontname = name;
                st < 0
            } {
                fatal!(
                    "Failed to read Type 1 font \"{}\".",
                    String::from_utf8_lossy(ident)
                );
            }
            drop(fp);

            let font = &mut self.font.fonts[fid];
            font.fontname = Some(fontname);
            font.subtype = PDF_FONT_FONTTYPE_TYPE1;
        }

        Ok(0)
    }
    /// `get_font_attr` (static; prefixed: Dpx methods share a namespace):
    /// the descriptor's attributes (and `defaultWidthX` into the Private
    /// DICT).
    fn type1_get_font_attr(&mut self, font_id: i32, cffont: &mut CffFont) -> Result<()> {
        const L_C: [&[u8]; 4] = [b"H", b"P", b"Pi", b"Rho"];
        const L_D: [&[u8]; 4] = [b"p", b"q", b"mu", b"eta"];
        const L_A: [&[u8]; 3] = [b"b", b"h", b"lambda"];
        let mut flags: i32 = 0;
        let mut gm = T1Ginfo::default();

        let mut defaultwidth = 500.0;
        let nominalwidth = 0.0;

        // CapHeight, Ascent, and Descent is meaningfull only for
        // Latin/Greek/Cyrillic. The BlueValues and OtherBlues also have
        // those information.
        let (mut capheight, mut ascent, mut descent);
        let td = cffont.topdict.as_ref().expect("topdict");
        if td.cff_dict_known(b"FontBBox") != 0 {
            // Default values
            capheight = td.cff_dict_get(b"FontBBox", 3)?;
            ascent = capheight;
            descent = td.cff_dict_get(b"FontBBox", 1)?;
        } else {
            capheight = 680.0;
            ascent = 690.0;
            descent = -190.0;
        }
        let pd = cffont.private[0].as_ref().expect("private");
        let stemv = if pd.cff_dict_known(b"StdVW") != 0 {
            pd.cff_dict_get(b"StdVW", 0)?
        } else {
            // We may use the following values for StemV:
            //  Thin - ExtraLight: <= 50
            //  Light: 71
            //  Regular(Normal): 88
            //  Medium: 109
            //  SemiBold(DemiBold): 135
            //  Bold - Heavy: >= 166
            88.0
        };
        let italicangle = if td.cff_dict_known(b"ItalicAngle") != 0 {
            let a = td.cff_dict_get(b"ItalicAngle", 0)?;
            if a != 0.0 {
                flags |= FONT_FLAG_ITALIC;
            }
            a
        } else {
            0.0
        };

        // Use "space", "H", "p", and "b" for various values. Those
        // characters should not "seac". (no accent)
        // (cff_glyph_lookup is a card16: not found is .notdef, gid 0.)
        let gid = i32::from(cffont.cff_glyph_lookup(b"space")?);
        // (C reads `cffont->cstrings->count` from here on: a font without
        // CharStrings is an error, not a NULL dereference.)
        let Some(cs) = cffont.cstrings.as_ref() else {
            fatal!("No CharStrings found in Type 1 font.");
        };
        let subrs = cffont.subrs.first().and_then(Option::as_ref);
        if gid >= 0 && gid < i32::from(cs.count) {
            self.t1char_get_metrics(charstring(cs, gid as usize), subrs, Some(&mut gm))?;
            defaultwidth = gm.wx;
        }

        for name in L_C {
            let gid = i32::from(cffont.cff_glyph_lookup(name)?);
            if gid >= 0 && gid < i32::from(cs.count) {
                self.t1char_get_metrics(charstring(cs, gid as usize), subrs, Some(&mut gm))?;
                capheight = gm.bbox.ury;
                break;
            }
        }

        for name in L_D {
            let gid = i32::from(cffont.cff_glyph_lookup(name)?);
            if gid >= 0 && gid < i32::from(cs.count) {
                self.t1char_get_metrics(charstring(cs, gid as usize), subrs, Some(&mut gm))?;
                descent = gm.bbox.lly;
                break;
            }
        }

        for name in L_A {
            let gid = i32::from(cffont.cff_glyph_lookup(name)?);
            if gid >= 0 && gid < i32::from(cs.count) {
                self.t1char_get_metrics(charstring(cs, gid as usize), subrs, Some(&mut gm))?;
                ascent = gm.bbox.ury;
                break;
            }
        }

        let pd = cffont.private[0].as_mut().expect("private");
        if defaultwidth != 0.0 {
            pd.cff_dict_add(b"defaultWidthX", 1)?;
            pd.cff_dict_set(b"defaultWidthX", 0, defaultwidth)?;
        }
        if nominalwidth != 0.0 {
            pd.cff_dict_add(b"nominalWidthX", 1)?;
            pd.cff_dict_set(b"nominalWidthX", 0, nominalwidth)?;
        }
        if pd.cff_dict_known(b"ForceBold") != 0 && pd.cff_dict_get(b"ForceBold", 0)? != 0.0 {
            flags |= FONT_FLAG_FORCEBOLD;
        }
        if pd.cff_dict_known(b"IsFixedPitch") != 0 && pd.cff_dict_get(b"IsFixedPitch", 0)? != 0.0 {
            flags |= FONT_FLAG_FIXEDPITCH;
        }

        let fontname = self.font.fonts[font_id as usize].fontname.clone();
        let descriptor = self.pdf_font_get_descriptor(font_id)?;

        if fontname.as_deref().is_some_and(|f| !strstr(f, b"Sans")) {
            flags |= FONT_FLAG_SERIF;
        }
        if fontname.as_deref().is_some_and(|f| strstr(f, b"Caps")) {
            flags |= FONT_FLAG_SMALLCAP;
        }
        flags |= FONT_FLAG_SYMBOLIC; // FIXME

        self.o.put_number(descriptor, b"CapHeight", capheight)?;
        self.o.put_number(descriptor, b"Ascent", ascent)?;
        self.o.put_number(descriptor, b"Descent", descent)?;
        self.o.put_number(descriptor, b"ItalicAngle", italicangle)?;
        self.o.put_number(descriptor, b"StemV", stemv)?;
        self.o.put_number(descriptor, b"Flags", f64::from(flags))?;
        Ok(())
    }
    /// `add_metrics` (static, prefixed): `Widths`, `FirstChar`, `LastChar`.
    fn type1_add_metrics(
        &mut self,
        font_id: i32,
        cffont: &CffFont,
        enc_vec: &[Option<Vec<u8>>],
        widths: &[f64],
        num_glyphs: i32,
    ) -> Result<()> {
        let fontdict = self.pdf_font_get_resource(font_id)?;
        let descriptor = self.pdf_font_get_descriptor(font_id)?;
        let usedchars = self.font.fonts[font_id as usize]
            .usedchars
            .clone()
            .expect("usedchars");
        let usedchars = usedchars.borrow().clone();
        let mut norm_widths = [0.0f64; 256];
        let firstchar: i32;
        let lastchar: i32;
        let td = cffont.topdict.as_ref().expect("topdict");

        // The original FontBBox of the font is preserved, instead of
        // replacing it with tight bounding box calculated from charstrings,
        // to prevent Acrobat 4 from greeking text as much as possible.
        if td.cff_dict_known(b"FontBBox") == 0 {
            fatal!("No FontBBox?");
        }

        // The widhts array in the font dictionary must be given relative
        // to the default scaling of 1000:1, not relative to the scaling
        // given by the font matrix.
        let scaling = if td.cff_dict_known(b"FontMatrix") != 0 {
            1000.0 * td.cff_dict_get(b"FontMatrix", 0)?
        } else {
            1.0
        };

        let array = self.o.new_array();
        for i in 0..4 {
            let val = td.cff_dict_get(b"FontBBox", i)?;
            let n = self.o.new_number(round_acc(val, 1.0));
            self.o.add_array(array, n)?;
        }
        let l = self.o.link(array)?;
        self.o.put(descriptor, b"FontBBox", l)?;
        self.o.release(array)?;

        let array = self.o.new_array();
        if num_glyphs <= 1 {
            // This must be an error.
            firstchar = 0;
            lastchar = 0;
            let n = self.o.new_number(0.0);
            self.o.add_array(array, n)?;
        } else {
            let (mut fc, mut lc) = (255, 0);
            for code in 0..256 {
                if usedchars[code as usize] != 0 {
                    if code < fc {
                        fc = code;
                    }
                    if code > lc {
                        lc = code;
                    }
                    let gid = glyph_lookup(cffont, enc_vec[code as usize].as_deref())?;
                    norm_widths[code as usize] = scaling * widths[usize::from(gid)];
                }
            }
            firstchar = fc;
            lastchar = lc;
            if firstchar > lastchar {
                warn!("No glyphs actually used???");
                self.o.release(array)?;
                return Ok(());
            }

            let ident = self.font.fonts[font_id as usize]
                .ident
                .clone()
                .unwrap_or_default();
            self.pdf_check_tfm_widths(&ident, &mut norm_widths, firstchar, lastchar, &usedchars)?;

            for code in firstchar..=lastchar {
                let v = if usedchars[code as usize] != 0 {
                    round_acc(norm_widths[code as usize], 0.1)
                } else {
                    0.0
                };
                let n = self.o.new_number(v);
                self.o.add_array(array, n)?;
            }
        }

        if self.o.array_length(array)? > 0 {
            let r = self.o.ref_obj(array)?;
            self.o.put(fontdict, b"Widths", r)?;
        }
        self.o.release(array)?;

        self.o
            .put_number(fontdict, b"FirstChar", f64::from(firstchar))?;
        self.o
            .put_number(fontdict, b"LastChar", f64::from(lastchar))?;
        Ok(())
    }
    /// `write_fontfile` (static, prefixed, the non-LIBDPX one): the
    /// FontFile3 stream; `pdfcharset` the CharSet string (a stream).
    fn type1_write_fontfile(
        &mut self,
        font_id: i32,
        cffont: &mut CffFont,
        pdfcharset: Option<Obj>,
    ) -> Result<i32> {
        let mut wbuf = [0u8; WBUF_SIZE];

        let descriptor = self.pdf_font_get_descriptor(font_id)?;

        let mut topdict_idx = CffIndex::cff_new_index(1);
        // Force existence of Encoding.
        let td = topdict(cffont);
        if td.cff_dict_known(b"CharStrings") == 0 {
            td.cff_dict_add(b"CharStrings", 1)?;
        }
        if td.cff_dict_known(b"charset") == 0 {
            td.cff_dict_add(b"charset", 1)?;
        }
        if td.cff_dict_known(b"Encoding") == 0 {
            td.cff_dict_add(b"Encoding", 1)?;
        }
        let mut private_size = cffont.private[0]
            .as_ref()
            .expect("private")
            .cff_dict_pack(&mut wbuf)?;
        // Private dict is required (but may have size 0)
        let td = topdict(cffont);
        if td.cff_dict_known(b"Private") == 0 {
            td.cff_dict_add(b"Private", 2)?;
        }
        topdict_idx.offset[1] = (td.cff_dict_pack(&mut wbuf)? + 1) as LOffset;

        // Estimate total size of fontfile.
        let charstring_len = cffont.cstrings.as_ref().expect("cstrings").cff_index_size();

        let mut stream_data_len: i32 = 4; // header size
        stream_data_len += cffont.name.as_ref().expect("name").cff_index_size();
        stream_data_len += topdict_idx.cff_index_size();
        stream_data_len += cffont.string.as_ref().expect("string").cff_index_size();
        stream_data_len += cffont.gsubr.as_ref().expect("gsubr").cff_index_size();
        // We are using format 1 for Encoding and format 0 for charset.
        // TODO: Should implement cff_xxx_size().
        let enc = cffont.encoding.as_ref().expect("encoding");
        stream_data_len += 2 + i32::from(enc.num_entries) * 2 + 1 + i32::from(enc.num_supps) * 3;
        stream_data_len +=
            1 + i32::from(cffont.charsets.as_ref().expect("charsets").num_entries) * 2;
        stream_data_len += charstring_len;
        stream_data_len += private_size;

        // Now we create FontFile data.
        let mut stream_data = vec![0u8; stream_data_len as usize];
        // Data Layout order as described in CFF spec., sec 2 "Data Layout".
        let mut offset: i32 = 0;
        // Header
        offset += cffont.cff_put_header(&mut stream_data[offset as usize..])?;
        // Name
        offset += cffont
            .name
            .as_ref()
            .unwrap()
            .cff_pack_index(&mut stream_data[offset as usize..])?;
        // Top DICT
        let topdict_offset = offset;
        offset += topdict_idx.cff_index_size();
        // Strings
        offset += cffont
            .string
            .as_ref()
            .unwrap()
            .cff_pack_index(&mut stream_data[offset as usize..])?;
        // Global Subrs
        offset += cffont
            .gsubr
            .as_ref()
            .unwrap()
            .cff_pack_index(&mut stream_data[offset as usize..])?;
        // Encoding
        // TODO: don't write Encoding entry if the font is always used
        // with PDF Encoding information. Applies to type1c.c as well.
        topdict(cffont).cff_dict_set(b"Encoding", 0, f64::from(offset))?;
        offset += cffont.cff_pack_encoding(&mut stream_data[offset as usize..])?;
        // charset
        topdict(cffont).cff_dict_set(b"charset", 0, f64::from(offset))?;
        offset += cffont.cff_pack_charsets(&mut stream_data[offset as usize..])?;
        // CharStrings
        topdict(cffont).cff_dict_set(b"CharStrings", 0, f64::from(offset))?;
        offset += cffont.cstrings.as_ref().unwrap().cff_pack_index(
            &mut stream_data[offset as usize..(offset + charstring_len) as usize],
        )?;
        // Private
        if cffont.private[0].is_some() && private_size > 0 {
            private_size = cffont.private[0].as_ref().unwrap().cff_dict_pack(
                &mut stream_data[offset as usize..(offset + private_size) as usize],
            )?;
            topdict(cffont).cff_dict_set(b"Private", 1, f64::from(offset))?;
            topdict(cffont).cff_dict_set(b"Private", 0, f64::from(private_size))?;
        }
        offset += private_size;

        // Finally Top DICT
        let n = topdict_idx.offset[1] as usize - 1;
        topdict_idx.data = vec![0u8; n];
        topdict(cffont).cff_dict_pack(&mut topdict_idx.data[..n])?;
        let size = topdict_idx.cff_index_size();
        topdict_idx.cff_pack_index(
            &mut stream_data[topdict_offset as usize..(topdict_offset + size) as usize],
        )?;
        cff_release_index(topdict_idx);

        // Copyright and Trademark Notice ommited.

        // Flush Font File
        let fontfile = self.o.new_stream(STREAM_COMPRESS);
        let stream_dict = self.o.stream_dict(fontfile)?;
        let r = self.o.ref_obj(fontfile)?;
        self.o.put(descriptor, b"FontFile3", r)?;
        self.o.put_name(stream_dict, b"Subtype", b"Type1C")?;
        self.o
            .add_stream(fontfile, &stream_data[..offset as usize])?;
        self.o.release(fontfile)?;
        if self.o.check_version(2, 0) < 0 {
            let cs = pdfcharset.expect("pdfcharset");
            let data = self.o.stream_data(cs)?.to_vec();
            self.o.put_string(descriptor, b"CharSet", &data)?;
        }

        Ok(offset)
    }
    /// `pdf_font_load_type1`.
    pub fn pdf_font_load_type1(&mut self, font_id: i32) -> Result<i32> {
        let fid = font_id as usize;

        if self.font.fonts[fid].reference.is_none() {
            return Ok(0);
        }

        let fontdict = self.pdf_font_get_resource(font_id)?;
        let font = &self.font.fonts[fid];
        let encoding_id = font.encoding_id;
        let usedchars = font.usedchars.clone().expect("usedchars");
        let ident = font.filename.clone().expect("filename");
        let fontname = font.fontname.clone().expect("fontname");
        let unique_tag = self.pdf_font_get_uniqueTag(font_id);

        let Some(mut fp) = self.dpx_open_file(&ident, ResType::T1Font)? else {
            fatal!(
                "Type1: Could not open Type1 font: {}",
                String::from_utf8_lossy(&ident)
            );
        };

        let mut gidmap: Vec<Card16> = vec![0; MAX_GLYPHS];
        let mut num_glyphs: Card16;

        let mut builtin: Option<Vec<Option<Vec<u8>>>> = if encoding_id >= 0 {
            None
        } else {
            Some(vec![None; 256])
        };

        let Some(mut cffont) = self.t1_load_font(builtin.as_deref_mut(), 0, &mut fp)? else {
            fatal!(
                "Could not load Type 1 font: {}",
                String::from_utf8_lossy(&ident)
            );
        };
        drop(fp);

        let fullname = subset_fullname(&unique_tag, &fontname);

        // Encoding related things.
        let enc_vec: Vec<Option<Vec<u8>>> = if encoding_id >= 0 {
            self.pdf_encoding_get_encoding(encoding_id)?
        } else {
            let enc_vec = builtin.take().unwrap();
            // Create enc_vec and ToUnicode CMap for built-in encoding.
            if self.o.lookup_dict(fontdict, b"ToUnicode")?.is_none() {
                let used = usedchars.borrow().clone();
                let tounicode = self.pdf_create_ToUnicode_CMap(&fullname, &enc_vec, Some(&used))?;
                if let Some(tounicode) = tounicode {
                    let r = self.o.ref_obj(tounicode)?;
                    self.o.put(fontdict, b"ToUnicode", r)?;
                    self.o.release(tounicode)?;
                }
            }
            enc_vec
        };

        cffont.cff_set_name(&fullname)?;

        // defaultWidthX, CapHeight, etc.
        self.type1_get_font_attr(font_id, &mut cffont)?;
        let pd = cffont.private[0].as_ref().expect("private");
        let defaultwidth = if pd.cff_dict_known(b"defaultWidthX") != 0 {
            pd.cff_dict_get(b"defaultWidthX", 0)?
        } else {
            0.0
        };
        let nominalwidth = if pd.cff_dict_known(b"nominalWidthX") != 0 {
            pd.cff_dict_get(b"nominalWidthX", 0)?
        } else {
            0.0
        };

        // Create CFF encoding, charset, sort glyphs
        let pdfcharset = self.o.new_stream(0);
        let mut charset;
        {
            let mut encoding = CffEncoding {
                format: 1,
                num_entries: 0,
                range1: vec![CffRange1::default(); 256],
                num_supps: 0,
                supp: vec![CffMap::default(); 256],
                ..CffEncoding::default()
            };

            charset = CffCharsets {
                format: 0,
                num_entries: 0,
                glyphs: vec![0; MAX_GLYPHS],
                ..CffCharsets::default()
            };

            let gid = cffont.cff_glyph_lookup(b".notdef")?;
            // (A card16: never < 0.)
            gidmap[0] = gid;
            num_glyphs = 1;
            let mut prev: i32 = -2;
            for code in 0..=0xffusize {
                let glyph = enc_vec[code].as_deref();

                if usedchars.borrow()[code] == 0 {
                    continue;
                }
                if glyph == Some(b".notdef") {
                    warn!("Character mapped to .notdef used in font.");
                    usedchars.borrow_mut()[code] = 0;
                    continue;
                }

                let gid = i32::from(glyph_lookup(&cffont, glyph)?);
                if gid < 1 || gid >= i32::from(cffont.cstrings.as_ref().unwrap().count) {
                    warn!("Glyph missing in font.");
                    usedchars.borrow_mut()[code] = 0;
                    continue;
                }
                let glyph = glyph.unwrap();

                let mut duplicate = 0;
                while duplicate < code {
                    if usedchars.borrow()[duplicate] != 0
                        && enc_vec[duplicate].as_deref() == Some(glyph)
                    {
                        break;
                    }
                    duplicate += 1;
                }

                let sid = cffont.cff_add_string(glyph, 1); // FIXME
                if duplicate < code {
                    // found duplicates
                    let n = encoding.num_supps as usize;
                    encoding.supp[n].code = duplicate as Card8;
                    encoding.supp[n].glyph = sid;
                    encoding.num_supps = encoding.num_supps.wrapping_add(1);
                } else {
                    gidmap[usize::from(num_glyphs)] = gid as Card16;
                    charset.glyphs[usize::from(charset.num_entries)] = sid;
                    charset.num_entries += 1;
                    if code as i32 != prev + 1 {
                        encoding.num_entries = encoding.num_entries.wrapping_add(1);
                        let r = &mut encoding.range1[usize::from(encoding.num_entries) - 1];
                        r.first = code as SSid;
                        r.n_left = 0;
                    } else {
                        let r = &mut encoding.range1[usize::from(encoding.num_entries) - 1];
                        r.n_left = r.n_left.wrapping_add(1);
                    }
                    prev = code as i32;
                    num_glyphs += 1;

                    // CharSet is actually string object.
                    self.o.add_stream(pdfcharset, b"/")?;
                    self.o.add_stream(pdfcharset, glyph)?;
                }
            }
            if encoding.num_supps > 0 {
                encoding.format |= 0x80;
            } else {
                encoding.supp = Vec::new(); // FIXME
            }
            cffont.encoding = Some(encoding);
        }

        // (C leaves the widths of glyphs it skips uninitialized.)
        let mut widths = vec![0.0f64; usize::from(cffont.cstrings.as_ref().unwrap().count)];
        // No more string will be added.
        // The Type 1 seac operator may add another glyph but the glyph
        // name of those glyphs are contained in standard string. The
        // String Index will not be modified after here.
        // BUT: We cannot update the String Index yet because then we
        // wouldn't be able to find the GIDs of the base and accent
        // characters (unless they have been used already).

        {
            let mut gm = T1Ginfo::default();
            let mut offset: i32 = 0;
            let mut dstlen_max: i32 = 0;
            let old = cffont.cstrings.take().unwrap();
            let subrs = cffont.subrs.first_mut().and_then(Option::take);
            let mut cstring = CffIndex::cff_new_index(old.count);
            cstring.data = Vec::new();
            cstring.offset[0] = 1;

            // The num_glyphs increases if "seac" operators are used.
            let mut gid: Card16 = 0;
            while gid < num_glyphs {
                let g = usize::from(gid);
                gid += 1;
                if offset + CS_STR_LEN_MAX >= dstlen_max {
                    dstlen_max += CS_STR_LEN_MAX * 2;
                    cstring.data.resize(dstlen_max as usize, 0);
                }
                let gid_orig = usize::from(gidmap[g]);

                let dst = cstring.offset[g] as usize - 1;
                offset += self.t1char_convert_charstring(
                    &mut cstring.data[dst..dst + CS_STR_LEN_MAX as usize],
                    charstring(&old, gid_orig),
                    subrs.as_ref(),
                    defaultwidth,
                    nominalwidth,
                    Some(&mut gm),
                )?;
                cstring.offset[g + 1] = (offset + 1) as LOffset;
                if gm.use_seac != 0 {
                    // NOTE:
                    //  1. seac.achar and seac.bchar must be contained in
                    //     the CFF standard string.
                    //  2. Those characters need not to be encoded.
                    //  3. num_glyphs == charsets->num_entries + 1.
                    let achar_name = t1_get_standard_glyph(i32::from(gm.seac.achar));
                    let achar_gid = i32::from(glyph_lookup(&cffont, achar_name)?);
                    let bchar_name = t1_get_standard_glyph(i32::from(gm.seac.bchar));
                    let bchar_gid = i32::from(glyph_lookup(&cffont, bchar_name)?);
                    if achar_gid < 0 {
                        warn!("Accent char not found. Invalid use of \"seac\" operator.");
                        continue;
                    }
                    if bchar_gid < 0 {
                        warn!("Base char not found. Invalid use of \"seac\" operator.");
                        continue;
                    }

                    let mut i = 0;
                    while i < num_glyphs {
                        if i32::from(gidmap[usize::from(i)]) == achar_gid {
                            break;
                        }
                        i += 1;
                    }
                    if i == num_glyphs {
                        let name = achar_name.unwrap_or_default();
                        gidmap[usize::from(num_glyphs)] = achar_gid as Card16;
                        num_glyphs += 1;
                        charset.glyphs[usize::from(charset.num_entries)] =
                            cffont.cff_get_seac_sid(name) as SSid;
                        charset.num_entries += 1;
                        // CharSet is actually string object.
                        self.o.add_stream(pdfcharset, b"/")?;
                        self.o.add_stream(pdfcharset, name)?;
                    }

                    let mut i = 0;
                    while i < num_glyphs {
                        if i32::from(gidmap[usize::from(i)]) == bchar_gid {
                            break;
                        }
                        i += 1;
                    }
                    if i == num_glyphs {
                        let name = bchar_name.unwrap_or_default();
                        gidmap[usize::from(num_glyphs)] = bchar_gid as Card16;
                        num_glyphs += 1;
                        charset.glyphs[usize::from(charset.num_entries)] =
                            cffont.cff_get_seac_sid(name) as SSid;
                        charset.num_entries += 1;
                        // CharSet is actually string object.
                        self.o.add_stream(pdfcharset, b"/")?;
                        self.o.add_stream(pdfcharset, name)?;
                    }
                }
                widths[g] = gm.wx;
            }
            cstring.count = num_glyphs;
            // (C's arrays keep their allocated sizes; only these parts
            // are read.)
            cstring.offset.truncate(usize::from(num_glyphs) + 1);
            cstring.data.truncate(offset as usize);

            drop(subrs);
            cffont.subrs = Vec::new();

            cff_release_index(old);
            cffont.cstrings = Some(cstring);

            if let Some(c) = cffont.charsets.take() {
                cff_release_charsets(c);
            }
            cffont.charsets = Some(charset);
        }

        // Now we can update the String Index
        let mut td = cffont.topdict.take().expect("topdict");
        cff_dict_update(&mut td, &mut cffont);
        cffont.topdict = Some(td);
        let mut pd = cffont.private[0].take().expect("private");
        cff_dict_update(&mut pd, &mut cffont);
        cffont.private[0] = Some(pd);
        cffont.cff_update_string();

        self.type1_add_metrics(font_id, &cffont, &enc_vec, &widths, i32::from(num_glyphs))?;

        let _offset = self.type1_write_fontfile(font_id, &mut cffont, Some(pdfcharset))?;

        self.o.release(pdfcharset)?;

        cffont.cff_close();

        Ok(0)
    }
}
