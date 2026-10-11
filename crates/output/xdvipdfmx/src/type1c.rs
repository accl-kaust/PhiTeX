//! type1c.c, type1c.h: OpenType (CFF) simple fonts.
//!
//! `pdf_font *` is the font's id in `self.font.fonts`.

use crate::cff::*;
use crate::cff_dict::{CFF_DEFAULTWIDTHX_DEFAULT, CFF_NOMINALWIDTHX_DEFAULT, cff_dict_update};
use crate::cs_type2::CsGinfo;
use crate::dpxfile::ResType;
use crate::fmt::round_acc;
use crate::obj::STREAM_COMPRESS;
use crate::pdffont::{PDF_FONT_FLAG_NOEMBED, PDF_FONT_FONTTYPE_TYPE1C};
use crate::prelude::*;
use crate::sfnt::{SFNT_TYPE_POSTSCRIPT, SFNT_TYPE_TTC, Sfnt, ULONG};
use crate::type1::subset_fullname;

/// `WORK_BUFFER_SIZE` (mfileio.h; C packs into its `work_buffer`).
const WORK_BUFFER_SIZE: usize = 1024;

impl Dpx {
    /// `pdf_font_open_type1c`: 0 ok, -1 not an OpenType/CFF font.
    pub fn pdf_font_open_type1c(
        &mut self,
        font_id: i32,
        ident: &[u8],
        index: i32,
        encoding_id: i32,
        embedding: i32,
    ) -> Result<i32> {
        let fid = font_id as usize;
        let mut embedding = embedding;
        let mut offset: ULONG = 0;

        let mut fp = self.dpx_open_file(ident, ResType::OtFont)?;
        if fp.is_none() {
            fp = self.dpx_open_file(ident, ResType::TtFont)?;
        }
        let Some(fp) = fp else {
            return Ok(-1);
        };

        let Some(mut sfont) = Sfnt::sfnt_open(fp)? else {
            return Ok(-1);
        };

        if sfont.type_ == SFNT_TYPE_TTC {
            offset = sfont.ttc_read_offset(index as ULONG)?;
        }

        if (sfont.type_ != SFNT_TYPE_TTC && sfont.type_ != SFNT_TYPE_POSTSCRIPT)
            || sfont.sfnt_read_table_directory(offset)? < 0
            || {
                offset = sfont.sfnt_find_table_pos(b"CFF ");
                offset == 0
            }
        {
            sfont.sfnt_close();
            return Ok(-1);
        }

        // (C's cff_font reads the sfnt's FILE.)
        let Some(cffont) = CffFont::cff_open(sfont.stream.clone(), offset as i32, 0)? else {
            warn!("Could not read CFF font data.");
            sfont.sfnt_close();
            return Ok(-1);
        };

        if (cffont.flag & FONTTYPE_CIDFONT) != 0 {
            cffont.cff_close();
            sfont.sfnt_close();
            return Ok(-1);
        }

        // (cff_get_name never gives NULL.)
        let fontname = cffont.cff_get_name();
        self.font.fonts[fid].fontname = Some(fontname.clone());

        cffont.cff_close();

        if embedding == 0 {
            warn!("Ignoring no-embed option for Type1C font.");
            embedding = 1;
            self.font.fonts[fid].flags &= !PDF_FONT_FLAG_NOEMBED;
        }
        // Font like AdobePiStd does not have meaningful built-in encoding.
        // Some software generate CFF/OpenType font with incorrect encoding.
        if encoding_id < 0 {
            warn!("Built-in encoding used for CFF/OpenType font.");
            warn!("CFF font in OpenType font sometimes have strange built-in encoding.");
            warn!("If you find text is not encoded properly in the generated PDF file,");
            warn!("please specify appropriate \".enc\" file in your fontmap.");
        }
        self.font.fonts[fid].subtype = PDF_FONT_FONTTYPE_TYPE1C;

        let descriptor = self.pdf_font_get_descriptor(font_id)?;
        // Create font descriptor from OpenType tables.
        // We can also use CFF TOP DICT/Private DICT for this.
        let Some(tmp) = self.tt_get_fontdesc(&mut sfont, &mut embedding, -1, 1, &fontname)? else {
            fatal!("Could not obtain neccesary font info from OpenType table.");
        };
        self.o.merge_dict(descriptor, tmp)?; // copy
        self.o.release(tmp)?;
        if embedding == 0 {
            // tt_get_fontdesc may have changed this
            warn!("Font embedding disallowed.");
            sfont.sfnt_close();
            return Ok(-1);
        }

        sfont.sfnt_close();

        Ok(0)
    }
    /// Glyph runs: the glyph name and id `pdf_font_load_type1c` draws each
    /// code of OpenType (CFF) font `ident` with, by code: the name in
    /// `encoding`, else in the CFF's built-in encoding, and the glyph of
    /// that name in the charset (0 where there is none). None if the font
    /// does not open as one.
    pub fn t1c_code_glyphs(
        &mut self,
        ident: &[u8],
        encoding: Option<&[Option<Vec<u8>>]>,
    ) -> Result<Option<(Vec<Option<Vec<u8>>>, Vec<u16>)>> {
        let fp = some!(self.dpx_open_file(ident, ResType::OtFont)?);
        let mut sfont = some!(Sfnt::sfnt_open(fp)?);
        if sfont.sfnt_read_table_directory(0)? < 0 || sfont.type_ != SFNT_TYPE_POSTSCRIPT {
            return Ok(None);
        }
        let offset = sfont.sfnt_find_table_pos(b"CFF ") as i32;
        if offset == 0 {
            return Ok(None);
        }
        let mut cffont = some!(CffFont::cff_open(sfont.stream.clone(), offset, 0)?);
        if (cffont.flag & FONTTYPE_CIDFONT) != 0 {
            return Ok(None);
        }
        cffont.cff_read_charsets()?;
        let names: Vec<Option<Vec<u8>>> = match encoding {
            Some(e) => (0..256).map(|c| e.get(c).cloned().flatten()).collect(),
            None => {
                cffont.cff_read_encoding()?;
                let mut names = vec![None; 256];
                for (code, n) in names.iter_mut().enumerate() {
                    let gid = cffont.cff_encoding_lookup(code as Card8)?;
                    *n = Some(cffont.cff_get_string(cffont.cff_charsets_lookup_inverse(gid)?));
                }
                names
            }
        };
        let mut gids = vec![0u16; 256];
        for (g, n) in gids.iter_mut().zip(&names) {
            if let Some(name) = n.as_deref().filter(|n| *n != b".notdef") {
                let sid = cffont.cff_get_sid(name) as SSid;
                *g = cffont.cff_charsets_lookup(sid)?;
            }
        }
        cffont.cff_close();
        Ok(Some((names, gids)))
    }

    /// `add_SimpleMetrics` (static, prefixed: Dpx methods share a
    /// namespace): `Widths`, `FirstChar`, `LastChar` (`widths` scaled in
    /// place).
    #[allow(non_snake_case)]
    fn type1c_add_SimpleMetrics(
        &mut self,
        font_id: i32,
        cffont: &CffFont,
        widths: &mut [f64],
        num_glyphs: Card16,
    ) -> Result<()> {
        let firstchar: i32;
        let lastchar: i32;

        let fontdict = self.pdf_font_get_resource(font_id)?;
        let usedchars = self.font.fonts[font_id as usize]
            .usedchars
            .clone()
            .expect("usedchars");
        let usedchars = usedchars.borrow().clone();

        // The widhts array in the font dictionary must be given relative
        // to the default scaling of 1000:1, not relative to the scaling
        // given by the font matrix.
        let td = cffont.topdict.as_ref().expect("topdict");
        let scaling = if td.cff_dict_known(b"FontMatrix") != 0 {
            1000.0 * td.cff_dict_get(b"FontMatrix", 0)?
        } else {
            1.0
        };

        let array = self.o.new_array();
        if num_glyphs <= 1 {
            // This should be error.
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
                    widths[code as usize] *= scaling;
                }
            }
            firstchar = fc;
            lastchar = lc;
            if firstchar > lastchar {
                fatal!("No glyphs used at all!");
            }
            let ident = self.font.fonts[font_id as usize]
                .ident
                .clone()
                .unwrap_or_default();
            self.pdf_check_tfm_widths(&ident, widths, firstchar, lastchar, &usedchars)?;

            for code in firstchar..=lastchar {
                let v = if usedchars[code as usize] != 0 {
                    round_acc(widths[code as usize], 0.1)
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
    /// `pdf_font_load_type1c`.
    pub fn pdf_font_load_type1c(&mut self, font_id: i32) -> Result<i32> {
        let fid = font_id as usize;
        let mut widths = [0.0f64; 256];
        let mut ginfo = CsGinfo::default();

        if self.font.fonts[fid].reference.is_none() {
            return Ok(0);
        }

        if (self.font.fonts[fid].flags & PDF_FONT_FLAG_NOEMBED) != 0 {
            fatal!("Only embedded font supported for CFF/OpenType font.");
        }

        let font = &self.font.fonts[fid];
        let usedchars = font.usedchars.clone().expect("usedchars");
        let fontname = font.fontname.clone().expect("fontname");
        let ident = font.filename.clone().expect("filename");
        let unique_tag = self.pdf_font_get_uniqueTag(font_id);

        let fontdict = self.pdf_font_get_resource(font_id)?;
        let descriptor = self.pdf_font_get_descriptor(font_id)?;
        let encoding_id = self.font.fonts[fid].encoding_id;

        let Some(fp) = self.dpx_open_file(&ident, ResType::OtFont)? else {
            fatal!(
                "Could not open OpenType font: {}",
                String::from_utf8_lossy(&ident)
            );
        };

        let Some(mut sfont) = Sfnt::sfnt_open(fp)? else {
            fatal!(
                "Could not open OpenType font: {}",
                String::from_utf8_lossy(&ident)
            );
        };
        if sfont.sfnt_read_table_directory(0)? < 0 {
            fatal!(
                "Could not read OpenType table directory: {}",
                String::from_utf8_lossy(&ident)
            );
        }
        let mut offset: i32 = 0;
        if sfont.type_ != SFNT_TYPE_POSTSCRIPT || {
            offset = sfont.sfnt_find_table_pos(b"CFF ") as i32;
            offset == 0
        } {
            fatal!("Not a CFF/OpenType font (or variable font?) (11)?");
        }

        let Some(mut cffont) = CffFont::cff_open(sfont.stream.clone(), offset, 0)? else {
            fatal!("Could not open CFF font.");
        };
        if (cffont.flag & FONTTYPE_CIDFONT) != 0 {
            fatal!("This is CIDFont...");
        }

        let fullname = subset_fullname(&unique_tag, &fontname);

        // Offsets from DICTs
        cffont.cff_read_charsets()?;
        if encoding_id < 0 {
            cffont.cff_read_encoding()?;
        }
        cffont.cff_read_private()?;
        cffont.cff_read_subrs()?;

        // FIXME
        cffont._string = Some(CffIndex::cff_new_index(0));

        // New Charsets data
        let mut charset = CffCharsets {
            format: 0,
            num_entries: 0,
            glyphs: vec![0; 256],
            ..CffCharsets::default()
        };

        // Encoding related things.
        let enc_vec: Vec<Option<Vec<u8>>> = if encoding_id >= 0 {
            self.pdf_encoding_get_encoding(encoding_id)?
        } else {
            // Create enc_vec and ToUnicode CMap for built-in encoding.
            let used = usedchars.borrow().clone();
            let mut enc_vec: Vec<Option<Vec<u8>>> = vec![None; 256];
            for code in 0..256usize {
                if used[code] != 0 {
                    let gid = cffont.cff_encoding_lookup(code as Card8)?;
                    enc_vec[code] =
                        Some(cffont.cff_get_string(cffont.cff_charsets_lookup_inverse(gid)?));
                } else {
                    enc_vec[code] = None;
                }
            }
            if self.o.lookup_dict(fontdict, b"ToUnicode")?.is_none() {
                let tounicode = self.pdf_create_ToUnicode_CMap(&fullname, &enc_vec, Some(&used))?;
                if let Some(tounicode) = tounicode {
                    let r = self.o.ref_obj(tounicode)?;
                    self.o.put(fontdict, b"ToUnicode", r)?;
                    self.o.release(tounicode)?;
                }
            }
            enc_vec
        };

        // New Encoding data:
        //
        //  We should not use format 0 here.
        //  The number of encoded glyphs (num_entries) is limited to 255 in
        //  format 0, and hence it causes problem for encodings that uses
        //  full 256 code-points. As we always sort glyphs by encoding, we
        //  can avoid this problem simply by using format 1; Using full
        //  range result in a single range, 0 255.
        //
        //  Creating actual encoding date is delayed to eliminate character
        //  codes to be mapped to .notdef and to handle multiply-encoded
        //  glyphs.
        let mut encoding = CffEncoding {
            format: 1,
            num_entries: 0,
            range1: vec![CffRange1::default(); 255],
            num_supps: 0,
            supp: vec![CffMap::default(); 255],
            ..CffEncoding::default()
        };

        // Charastrings.
        let td = cffont.topdict.as_ref().expect("topdict");
        let mut offset = td.cff_dict_get(b"CharStrings", 0)? as i32;
        cffont.cff_seek_set(offset as usize);
        let cs_idx = cffont.cff_get_index_header()?;

        // Offset is now absolute offset ... fixme
        offset = cffont.cff_tell() as i32;
        let cs_count = cs_idx.count;
        if cs_count < 2 {
            fatal!("No valid charstring data found.");
        }

        // New CharStrings INDEX
        let mut charstrings = CffIndex::cff_new_index(257); // 256 + 1 for ".notdef" glyph
        let mut max_len: i32 = 2 * CS_STR_LEN_MAX;
        charstrings.data = vec![0; max_len as usize];
        let mut charstring_len: i32 = 0;

        // Information from OpenType table is rough estimate. Replace with
        // accurate value.
        if let Some(pd) = cffont.private.first().and_then(Option::as_ref)
            && pd.cff_dict_known(b"StdVW") != 0
        {
            let stemv = pd.cff_dict_get(b"StdVW", 0)?;
            self.o.put_number(descriptor, b"StemV", stemv)?;
        }

        // Widths
        let pd = cffont.private.first().and_then(Option::as_ref);
        let default_width = match pd {
            Some(pd) if pd.cff_dict_known(b"defaultWidthX") != 0 => {
                pd.cff_dict_get(b"defaultWidthX", 0)?
            }
            _ => CFF_DEFAULTWIDTHX_DEFAULT,
        };
        let nominal_width = match pd {
            Some(pd) if pd.cff_dict_known(b"nominalWidthX") != 0 => {
                pd.cff_dict_get(b"nominalWidthX", 0)?
            }
            _ => CFF_NOMINALWIDTHX_DEFAULT,
        };

        // First we add .notdef glyph.
        // All Type 1 font requires .notdef glyph to be present.
        let size = cs_idx.offset[1] as i32 - cs_idx.offset[0] as i32;
        if size > CS_STR_LEN_MAX {
            fatal!("Charstring too long: gid={}, {} bytes", 0, size);
        }
        charstrings.offset[0] = (charstring_len + 1) as LOffset;
        cffont.cff_seek((offset + cs_idx.offset[0] as i32 - 1) as usize);
        let data = cffont.cff_read_data(size as usize);
        charstring_len += self.cs_copy_charstring(
            &mut charstrings.data[charstring_len as usize..max_len as usize],
            &data,
            cffont.gsubr.as_ref(),
            cffont.subrs.first().and_then(Option::as_ref),
            default_width,
            nominal_width,
            Some(&mut ginfo),
        )?;
        let notdef_width = ginfo.wx;

        // Subset font
        let mut num_glyphs: Card16 = 1;
        let pdfcharset = self.o.new_stream(0);
        for code in 0..256usize {
            widths[code] = notdef_width;

            let name = match enc_vec[code].as_deref() {
                Some(n) if usedchars.borrow()[code] != 0 && n != b".notdef" => n,
                _ => continue,
            };

            // FIXME:
            //  cff_get_sid() obtain SID from original String INDEX.
            //  It should be cff_string_get_sid(string, ...).
            //  cff_add_string(cff, ...) -> cff_string_add(string, ...).
            let sid_orig = cffont.cff_get_sid(name) as SSid;
            let sid = if i32::from(sid_orig) < CFF_STDSTR_MAX {
                sid_orig
            } else {
                cffont.cff_add_string(name, 0)
            };
            // We use "unique = 0" because duplicate strings are impossible
            // at this stage unless the original font already had
            // duplicates.

            // Check if multiply-encoded glyph.
            let mut j: Card16 = 0;
            while j < charset.num_entries {
                if sid == charset.glyphs[usize::from(j)] {
                    // Already have this glyph.
                    let n = usize::from(encoding.num_supps);
                    encoding.supp[n].code = code as Card8;
                    encoding.supp[n].glyph = sid;
                    usedchars.borrow_mut()[code] = 0; // Used but multiply-encoded.
                    encoding.num_supps = encoding.num_supps.wrapping_add(1);
                    break;
                }
                j += 1;
            }
            if j < charset.num_entries {
                continue; // Prevent duplication.
            }

            // This is new encoding entry.
            let gid = cffont.cff_charsets_lookup(sid_orig)?; // FIXME
            if gid == 0 {
                warn!("Glyph missing in font.");
                warn!("Maybe incorrect encoding specified.");
                usedchars.borrow_mut()[code] = 0; // Set unused for writing correct encoding
                continue;
            }
            self.o.add_stream(pdfcharset, b"/")?;
            self.o.add_stream(pdfcharset, name)?;

            let g = usize::from(gid);
            let size = cs_idx.offset[g + 1] as i32 - cs_idx.offset[g] as i32;
            if size > CS_STR_LEN_MAX {
                fatal!("Charstring too long: gid={}, {} bytes", gid, size);
            }

            if charstring_len + CS_STR_LEN_MAX >= max_len {
                max_len = charstring_len + 2 * CS_STR_LEN_MAX;
                charstrings.data.resize(max_len as usize, 0);
            }
            charstrings.offset[usize::from(num_glyphs)] = (charstring_len + 1) as LOffset;
            cffont.cff_seek((offset + cs_idx.offset[g] as i32 - 1) as usize);
            let data = cffont.cff_read_data(size as usize);
            charstring_len += self.cs_copy_charstring(
                &mut charstrings.data[charstring_len as usize..max_len as usize],
                &data,
                cffont.gsubr.as_ref(),
                cffont.subrs.first().and_then(Option::as_ref),
                default_width,
                nominal_width,
                Some(&mut ginfo),
            )?;
            widths[code] = ginfo.wx;
            charset.glyphs[usize::from(charset.num_entries)] = sid;
            charset.num_entries += 1;
            num_glyphs += 1;
        }

        // Now we create encoding data.
        if encoding.num_supps > 0 {
            encoding.format |= 0x80; // Have supplemantary data.
        } else {
            encoding.supp = Vec::new(); // FIXME
        }
        {
            let used = usedchars.borrow();
            let is_enc = |code: usize| {
                used[code] != 0 && enc_vec[code].as_deref().is_some_and(|n| n != b".notdef")
            };
            let mut code = 0usize;
            while code < 256 {
                if !is_enc(code) {
                    code += 1;
                    continue;
                }
                let n = usize::from(encoding.num_entries);
                encoding.range1[n].first = code as SSid;
                encoding.range1[n].n_left = 0;
                code += 1;
                while code < 256 && is_enc(code) {
                    encoding.range1[n].n_left = encoding.range1[n].n_left.wrapping_add(1);
                    code += 1;
                }
                encoding.num_entries = encoding.num_entries.wrapping_add(1);
                // The above while() loop stopped at unused char or
                // code == 256.
                code += 1;
            }
        }

        // cleanup
        drop(enc_vec);

        cff_release_index(cs_idx);

        charstrings.offset[usize::from(num_glyphs)] = (charstring_len + 1) as LOffset;
        charstrings.count = num_glyphs;
        // (C's arrays keep their allocated sizes; only these parts are
        // read.)
        charstrings.offset.truncate(usize::from(num_glyphs) + 1);
        charstrings.data.truncate(charstring_len as usize);
        let charstring_len = charstrings.cff_index_size();
        cffont.num_glyphs = num_glyphs;

        // Discard old one, set new data.
        if let Some(c) = cffont.charsets.take() {
            cff_release_charsets(c);
        }
        let charset_num_entries = charset.num_entries;
        cffont.charsets = Some(charset);
        if let Some(e) = cffont.encoding.take() {
            cff_release_encoding(e);
        }
        let (enc_num_entries, enc_num_supps) = (encoding.num_entries, encoding.num_supps);
        cffont.encoding = Some(encoding);
        // We don't use subroutines at all.
        if let Some(g) = cffont.gsubr.take() {
            cff_release_index(g);
        }
        cffont.gsubr = Some(CffIndex::cff_new_index(0));
        if let Some(s) = cffont.subrs[0].take() {
            cff_release_index(s);
        }
        cffont.subrs[0] = None;

        // Flag must be reset since cff_pack_encoding(charset) does not
        // write encoding(charset) if HAVE_STANDARD_ENCODING(CHARSET) is
        // set. We are re-encoding font.
        cffont.flag = FONTTYPE_FONT;

        // FIXME:
        //  Update String INDEX to delete unused strings.
        let mut td = cffont.topdict.take().expect("topdict");
        cff_dict_update(&mut td, &mut cffont);
        cffont.topdict = Some(td);
        if let Some(mut pd) = cffont.private[0].take() {
            cff_dict_update(&mut pd, &mut cffont);
            cffont.private[0] = Some(pd);
        }
        cffont.cff_update_string();

        // Calculate sizes of Top DICT and Private DICT.
        // All offset values in DICT are set to long (32-bit) integer in
        // cff_dict_pack(), those values are updated later.
        let mut topdict = CffIndex::cff_new_index(1);
        let mut work_buffer = [0u8; WORK_BUFFER_SIZE];

        let td = cffont.topdict.as_mut().unwrap();
        td.cff_dict_remove(b"UniqueID");
        td.cff_dict_remove(b"XUID");

        // Force existence of Encoding.
        if td.cff_dict_known(b"Encoding") == 0 {
            td.cff_dict_add(b"Encoding", 1)?;
        }
        topdict.offset[1] = (td.cff_dict_pack(&mut work_buffer)? + 1) as LOffset;
        let mut private_size: i32 = 0;
        if let Some(pd) = cffont.private[0].as_mut() {
            pd.cff_dict_remove(b"Subrs"); // no Subrs
            private_size = pd.cff_dict_pack(&mut work_buffer)?;
        }

        // Estimate total size of fontfile.
        let mut stream_data_len: i32 = 4; // header size

        stream_data_len += cffont.cff_set_name(&fullname)?;

        stream_data_len += topdict.cff_index_size();
        stream_data_len += cffont.string.as_ref().expect("string").cff_index_size();
        stream_data_len += cffont.gsubr.as_ref().unwrap().cff_index_size();

        // We are using format 1 for Encoding and format 0 for charset.
        // TODO: Should implement cff_xxx_size().
        stream_data_len += 2 + i32::from(enc_num_entries) * 2 + 1 + i32::from(enc_num_supps) * 3;
        stream_data_len += 1 + i32::from(charset_num_entries) * 2;
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
        offset += topdict.cff_index_size();
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
        let td = cffont.topdict.as_mut().unwrap();
        td.cff_dict_set(b"Encoding", 0, f64::from(offset))?;
        offset += cffont.cff_pack_encoding(&mut stream_data[offset as usize..])?;
        // charset
        let td = cffont.topdict.as_mut().unwrap();
        td.cff_dict_set(b"charset", 0, f64::from(offset))?;
        offset += cffont.cff_pack_charsets(&mut stream_data[offset as usize..])?;
        // CharStrings
        let td = cffont.topdict.as_mut().unwrap();
        td.cff_dict_set(b"CharStrings", 0, f64::from(offset))?;
        offset += charstrings.cff_pack_index(
            &mut stream_data[offset as usize..(offset + charstring_len) as usize],
        )?;
        cff_release_index(charstrings);
        // Private
        let td = cffont.topdict.as_mut().unwrap();
        td.cff_dict_set(b"Private", 1, f64::from(offset))?;
        if let Some(pd) = cffont.private[0].as_ref()
            && private_size > 0
        {
            private_size = pd.cff_dict_pack(
                &mut stream_data[offset as usize..(offset + private_size) as usize],
            )?;
        }
        let td = cffont.topdict.as_mut().unwrap();
        td.cff_dict_set(b"Private", 0, f64::from(private_size))?;
        offset += private_size;

        // Finally Top DICT
        let n = topdict.offset[1] as usize - 1;
        topdict.data = vec![0u8; n];
        cffont
            .topdict
            .as_ref()
            .unwrap()
            .cff_dict_pack(&mut topdict.data[..n])?;
        let size = topdict.cff_index_size();
        topdict.cff_pack_index(
            &mut stream_data[topdict_offset as usize..(topdict_offset + size) as usize],
        )?;
        cff_release_index(topdict);

        // Copyright and Trademark Notice ommited.

        // Handle Widths in fontdict.
        self.type1c_add_SimpleMetrics(font_id, &cffont, &mut widths, num_glyphs)?;

        // Close font
        cffont.cff_close();
        sfont.sfnt_close();

        // CharSet
        if self.o.check_version(2, 0) < 0 {
            let data = self.o.stream_data(pdfcharset)?.to_vec();
            self.o.put_string(descriptor, b"CharSet", &data)?;
        }
        self.o.release(pdfcharset)?;
        // Write PDF FontFile data.
        let fontfile = self.o.new_stream(STREAM_COMPRESS);
        let stream_dict = self.o.stream_dict(fontfile)?;
        let r = self.o.ref_obj(fontfile)?;
        self.o.put(descriptor, b"FontFile3", r)?;
        self.o.put_name(stream_dict, b"Subtype", b"Type1C")?;
        self.o
            .add_stream(fontfile, &stream_data[..offset as usize])?;
        self.o.release(fontfile)?;

        Ok(0)
    }
}
