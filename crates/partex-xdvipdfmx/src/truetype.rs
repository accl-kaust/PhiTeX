//! truetype.c, truetype.h: simple (non-CID) TrueType fonts.
//!
//! The glyph name resolvers keep C's out-parameter `USHORT *gid`
//! (`&mut USHORT`): C writes it only on some paths, and
//! `do_custom_encoding` reads it even when resolving failed.

use crate::agl::{
    agl_chop_suffix, agl_name_convert_unicode, agl_name_is_unicode, agl_suffix_to_otltag,
};
use crate::dpxfile::ResType;
use crate::fmt::round_acc;
use crate::pdffont::{PDF_FONT_FLAG_NOEMBED, PDF_FONT_FONTTYPE_TRUETYPE};
use crate::prelude::*;
use crate::sfnt::{
    SFNT_TYPE_DFONT, SFNT_TYPE_TRUETYPE, SFNT_TYPE_TTC, Sfnt, ULONG, USHORT, sfnt_put_short,
    sfnt_put_ulong, sfnt_put_ushort,
};
use crate::tt_cmap::{
    TT_MAC, TT_MAC_ROMAN, TT_WIN, TT_WIN_SYMBOL, TT_WIN_UCS4, TT_WIN_UNICODE, TtCmap,
};
use crate::tt_glyf::TtGlyphs;
use crate::tt_gsub::OtlGsub;
use crate::tt_post::TtPostTable;

/// `required_table`: the tables kept in an embedded TrueType font, and
/// whether each must exist.
pub const REQUIRED_TABLE: [(&[u8], i32); 12] = [
    (b"OS/2", 0),
    (b"head", 1),
    (b"hhea", 1),
    (b"loca", 1),
    (b"maxp", 1),
    (b"name", 1),
    (b"glyf", 1),
    (b"hmtx", 1),
    (b"fpgm", 0),
    (b"cvt ", 0),
    (b"prep", 0),
    (b"cmap", 1),
];

/// `struct glyph_mapper`: glyph name to gid lookups, borrowing the font.
#[derive(Debug)]
pub struct GlyphMapper<'a> {
    pub codetogid: Option<TtCmap>,
    pub gsub: Option<OtlGsub>,
    pub sfont: &'a mut Sfnt,
    pub nametogid: Option<TtPostTable>,
}

/// A C string: the bytes up to the first NUL.
fn c_str(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

/// `PDFUNIT(v)` (truetype.c): `v` font units in 1/1000 em.
fn pdfunit(v: f64, emsize: USHORT) -> f64 {
    round_acc(1000.0 * v / f64::from(emsize), 1.0)
}

/// The `cmap` table C's `do_*_encoding` write (`cmap_old_format` is 0):
/// a (1,0) format 6 and a (3,0) format 4 subtable, 1086 bytes; the
/// glyph indices are filled per code.
fn new_cmap_table() -> Vec<u8> {
    let mut t = vec![0u8; 1086]; /* = 20 + 522 + 544 */
    sfnt_put_ushort(&mut t[0..], 0); /* Version  */
    sfnt_put_ushort(&mut t[2..], 2); /* Number of subtables */
    sfnt_put_ushort(&mut t[4..], TT_MAC); /* Platform ID */
    sfnt_put_ushort(&mut t[6..], TT_MAC_ROMAN); /* Encoding ID */
    sfnt_put_ulong(&mut t[8..], 20); /* Offset   */
    sfnt_put_ushort(&mut t[12..], TT_WIN); /* Platform ID */
    sfnt_put_ushort(&mut t[14..], TT_WIN_SYMBOL); /* Encoding ID */
    sfnt_put_ulong(&mut t[16..], 542); /* Offset   = 20 + 522 */
    // (1, 0) subtable
    sfnt_put_ushort(&mut t[20..], 6); /* format   */
    sfnt_put_ushort(&mut t[22..], 522); /* length   = 10 + 512 */
    sfnt_put_ushort(&mut t[24..], 0); /* language */
    sfnt_put_ushort(&mut t[26..], 0); /* firstCode */
    sfnt_put_ushort(&mut t[28..], 256); /* entryCount */
    // (3, 0) subtable
    sfnt_put_ushort(&mut t[542..], 4); /* format   */
    sfnt_put_ushort(&mut t[544..], 544); /* length   = 16 + 8 * 2 + 512 */
    sfnt_put_ushort(&mut t[546..], 0); /* language */
    sfnt_put_ushort(&mut t[548..], 4); /* segCountX2 */
    sfnt_put_ushort(&mut t[550..], 4); /* searchRange */
    sfnt_put_ushort(&mut t[552..], 1); /* entrySelector */
    sfnt_put_ushort(&mut t[554..], 0); /* rangeShift */
    sfnt_put_ushort(&mut t[556..], 0xf0ff); /* endCode[0] */
    sfnt_put_ushort(&mut t[558..], 0xffff); /* endCode[1] */
    sfnt_put_ushort(&mut t[560..], 0); /* reservedPad */
    sfnt_put_ushort(&mut t[562..], 0xf000); /* startCode[0] */
    sfnt_put_ushort(&mut t[564..], 0xffff); /* startCode[1] */
    sfnt_put_short(&mut t[566..], 0); /* idDelta[0] */
    sfnt_put_short(&mut t[568..], 1); /* idDelta[1] */
    sfnt_put_ushort(&mut t[570..], 4); /* idRangeOffset[0] */
    sfnt_put_ushort(&mut t[572..], 0); /* idRangeOffset[1] */
    t
}

/// `agl_decompose_glyphname` (static): the count, the `_`-separated
/// components (at most `size`; C's `nptrs`, ERROR beyond) and the suffix
/// after the first `.` (C's `*suffix`).
fn agl_decompose_glyphname(
    glyphname: &[u8],
    size: i32,
) -> Result<(i32, Vec<Vec<u8>>, Option<Vec<u8>>)> {
    let glyphname = c_str(glyphname);
    // Chop everything after the *first* dot.
    let (base, suffix) = match glyphname.iter().position(|&c| c == b'.') {
        None => (glyphname, None),
        Some(q) => (&glyphname[..q], Some(glyphname[q + 1..].to_vec())),
    };

    let mut nptrs: Vec<Vec<u8>> = Vec::new();
    // `start` is where nptrs[n - 1] begins; `p` searches from there.
    let mut start = 0usize;
    let mut n: i32 = 1;
    let mut p = 0usize;
    while p < base.len() {
        let Some(u) = base[p..].iter().position(|&c| c == b'_').map(|k| p + k) else {
            break;
        };
        if u + 1 >= base.len() {
            break;
        }
        if n >= size {
            fatal!("Uh ah..."); /* _FIXME_ */
        }
        nptrs.push(base[start..u].to_vec());
        p = u + 1;
        start = p;
        n += 1;
    }
    nptrs.push(base[start..].to_vec());

    Ok((n, nptrs, suffix))
}

/// `select_gsub` (static): loads/selects GSUB feature `feat`; 0, or -1.
fn select_gsub(feat: &[u8], gm: &mut GlyphMapper<'_>) -> Result<i32> {
    let feat = c_str(feat);
    if feat.is_empty() {
        return Ok(-1);
    }
    let Some(gsub) = gm.gsub.as_mut() else {
        return Ok(-1);
    };

    // First treat as is.
    let idx = gsub.otl_gsub_select(b"*", b"*", feat);
    if idx >= 0 {
        return Ok(0);
    }

    let error = gsub.otl_gsub_add_feat(b"*", b"*", feat, gm.sfont)?;
    if error == 0 {
        let idx = gsub.otl_gsub_select(b"*", b"*", feat);
        return if idx >= 0 { Ok(0) } else { Ok(-1) };
    }

    Ok(-1)
}

/// `otl_gsub_apply(gm->gsub, gid)`: -1 without a GSUB.
fn gm_apply(gm: &GlyphMapper<'_>, gid: &mut USHORT) -> Result<i32> {
    Ok(match gm.gsub.as_ref() {
        Some(g) => g.otl_gsub_apply(gid)?,
        None => -1,
    })
}

/// `composeglyph` (static): ligature substitution of `glyphs` (feature
/// `feat`, or the default ones); `gid` written on success.
fn composeglyph(
    glyphs: &[USHORT],
    feat: Option<&[u8]>,
    gm: &mut GlyphMapper<'_>,
    gid: &mut USHORT,
) -> Result<i32> {
    let mut t = *b"    ";

    let mut error = match feat.map(c_str) {
        None | Some(b"") => {
            // Meaning "Unknown".
            select_gsub(b"(?lig|lig?|?cmp|cmp?|frac|afrc)", gm)?
        }
        Some(feat) => {
            if feat.len() > 4 {
                -1
            } else {
                t[..feat.len()].copy_from_slice(feat);
                select_gsub(&t, gm)?
            }
        }
    };

    if error == 0 {
        error = match gm.gsub.as_ref() {
            Some(g) => g.otl_gsub_apply_lig(glyphs, gid)?,
            None => -1,
        };
    }

    Ok(error)
}

/// `composeuchar` (static): `composeglyph` of the gids of `unicodes`.
fn composeuchar(
    unicodes: &[i32],
    feat: Option<&[u8]>,
    gm: &mut GlyphMapper<'_>,
    gid: &mut USHORT,
) -> Result<i32> {
    let Some(codetogid) = gm.codetogid.as_ref() else {
        return Ok(-1);
    };

    let mut error = 0;
    let mut gids = vec![0 as USHORT; unicodes.len()];
    let mut i = 0;
    while error == 0 && i < unicodes.len() {
        gids[i] = codetogid.tt_cmap_lookup(unicodes[i] as ULONG);
        error = if gids[i] == 0 { -1 } else { 0 };
        i += 1;
    }

    if error == 0 {
        error = composeglyph(&gids, feat, gm, gid)?;
    }

    Ok(error)
}

/// `findposttable` (static): `gid` from the `post` table.
fn findposttable(glyph_name: &[u8], gid: &mut USHORT, gm: &mut GlyphMapper<'_>) -> i32 {
    let Some(post) = gm.nametogid.as_ref() else {
        return -1;
    };
    *gid = post.tt_lookup_post_table(glyph_name);
    if *gid == 0 { -1 } else { 0 }
}

/// `setup_glyph_mapper` (static): status and the mapper (C fills `gm`).
fn setup_glyph_mapper(sfont: &mut Sfnt) -> Result<(i32, GlyphMapper<'_>)> {
    let nametogid = sfont.tt_read_post_table()?;
    let mut codetogid = sfont.tt_cmap_read(TT_WIN, TT_WIN_UCS4)?;
    if codetogid.is_none() {
        codetogid = sfont.tt_cmap_read(TT_WIN, TT_WIN_UNICODE)?;
    }

    if nametogid.is_none() && codetogid.is_none() {
        return Ok((
            -1,
            GlyphMapper {
                codetogid,
                gsub: None,
                sfont,
                nametogid,
            },
        ));
    }

    Ok((
        0,
        GlyphMapper {
            codetogid,
            gsub: Some(OtlGsub::otl_gsub_new()),
            sfont,
            nametogid,
        },
    ))
}

/// `clean_glyph_mapper` (static).
fn clean_glyph_mapper(gm: GlyphMapper<'_>) {}

/// `is_comp(n)`: a glyph name with `_` (truetype.c: "This is wrong. We
/// must care about '.'").
fn is_comp(n: &[u8]) -> bool {
    c_str(n).contains(&b'_')
}

impl Dpx {
    /// `pdf_font_open_truetype`: 0, or -1 if not a usable TrueType font.
    pub fn pdf_font_open_truetype(
        &mut self,
        font_id: i32,
        ident: &[u8],
        index: i32,
        encoding_id: i32,
        embedding: i32,
    ) -> Result<i32> {
        let _ = encoding_id;
        let mut embedding = embedding;

        let sfont = if let Some(fp) = self.dpx_open_file(ident, ResType::TtFont)? {
            Sfnt::sfnt_open(fp)?
        } else if let Some(fp) = self.dpx_open_file(ident, ResType::DFont)? {
            Sfnt::dfont_open(fp, index)?
        } else {
            return Ok(-1);
        };
        let Some(mut sfont) = sfont else {
            warn!("Could not open TrueType font");
            return Ok(-1);
        };

        let mut error = if sfont.type_ == SFNT_TYPE_TTC {
            let offset = sfont.ttc_read_offset(index as ULONG)?;
            if offset == 0 {
                warn!("Invalid TTC index");
                -1
            } else {
                sfont.sfnt_read_table_directory(offset)?
            }
        } else {
            let offset = sfont.offset;
            sfont.sfnt_read_table_directory(offset)?
        };
        if error != 0 {
            return Ok(-1); /* Silently */
        }

        // Reading fontdict before checking fonttype conflicts with PKFONT
        // because pdf_font_get_resource() always makes a dictionary.
        let fontdict = self.pdf_font_get_resource(font_id)?;
        let descriptor = self.pdf_font_get_descriptor(font_id)?;
        if embedding == 0 {
            warn!("No-embed option not supported for TrueType font");
            embedding = 1;
            self.font.fonts[font_id as usize].flags &= !PDF_FONT_FLAG_NOEMBED;
        }

        {
            // C's `char fontname[256]`, zeroed.
            let mut fontname = [0u8; 256];
            let name = sfont.tt_get_ps_fontname(255)?;
            let mut length = name.len();
            fontname[..length].copy_from_slice(&name);
            if length < 1 {
                let ident = c_str(ident);
                length = ident.len().min(255);
                fontname[..length].copy_from_slice(&ident[..length]);
            }
            fontname[length] = 0;
            for n in 0..length {
                if fontname[n] == 0 {
                    fontname.copy_within(n + 1..length, n);
                }
            }
            let fontname = c_str(&fontname).to_vec();
            if fontname.is_empty() {
                warn!("Can't find valid fontname");
                error = -1;
            } else {
                self.font.fonts[font_id as usize].fontname = Some(fontname.clone());
                match self.tt_get_fontdesc(&mut sfont, &mut embedding, -1, 1, &fontname)? {
                    None => {
                        warn!("Could not obtain necessary font info");
                        error = -1;
                    }
                    Some(tmp) => {
                        self.o.merge_dict(descriptor, tmp)?;
                        self.o.release(tmp)?;
                    }
                }
            }
        }
        if error != 0 {
            return Ok(-1);
        }

        if embedding == 0 {
            warn!("Font file can't be embedded due to liscence restrictions.");
            error = -1;
        }

        drop(sfont);

        if error == 0 {
            self.o.put_name(fontdict, b"Type", b"Font")?;
            self.o.put_name(fontdict, b"Subtype", b"TrueType")?;
            self.font.fonts[font_id as usize].subtype = PDF_FONT_FONTTYPE_TRUETYPE;
        }

        Ok(error)
    }

    /// `pdf_font_load_truetype`: 0, or -1 on error.
    pub fn pdf_font_load_truetype(&mut self, font_id: i32) -> Result<i32> {
        let (descriptor, ident, encoding_id, index, has_reference) = {
            let font = &self.font.fonts[font_id as usize];
            (
                font.descriptor,
                font.filename.clone().unwrap_or_default(),
                font.encoding_id,
                font.index as i32,
                font.reference.is_some(),
            )
        };

        if !has_reference {
            return Ok(0);
        }
        let usedchars: Vec<u8> = self.font.fonts[font_id as usize]
            .usedchars
            .as_ref()
            .expect("pdf_font_load_truetype: no usedchars")
            .borrow()
            .clone();

        let sfont = if let Some(fp) = self.dpx_open_file(&ident, ResType::TtFont)? {
            Sfnt::sfnt_open(fp)?
        } else if let Some(fp) = self.dpx_open_file(&ident, ResType::DFont)? {
            Sfnt::dfont_open(fp, index)?
        } else {
            fatal!(
                "Unable to open TrueType/dfont font file: {}",
                alloc::string::String::from_utf8_lossy(&ident)
            );
        };

        let Some(mut sfont) = sfont else {
            fatal!(
                "Unable to open TrueType/dfont file: {}",
                alloc::string::String::from_utf8_lossy(&ident)
            );
        };
        if sfont.type_ != SFNT_TYPE_TRUETYPE
            && sfont.type_ != SFNT_TYPE_TTC
            && sfont.type_ != SFNT_TYPE_DFONT
        {
            fatal!(
                "Font \"{}\" not a TrueType/dfont font?",
                alloc::string::String::from_utf8_lossy(&ident)
            );
        }

        let mut error = if sfont.type_ == SFNT_TYPE_TTC {
            let offset = sfont.ttc_read_offset(index as ULONG)?;
            if offset == 0 {
                fatal!("Invalid TTC index");
            }
            sfont.sfnt_read_table_directory(offset)?
        } else {
            let offset = sfont.offset;
            sfont.sfnt_read_table_directory(offset)?
        };

        if error != 0 {
            fatal!("Reading SFND table dir failed... Not a TrueType font?");
        }

        // Create new TrueType cmap table with MacRoman encoding.
        if encoding_id < 0 {
            error = self.do_builtin_encoding(font_id, &usedchars, &mut sfont)?;
        } else {
            let enc_vec = self.pdf_encoding_get_encoding(encoding_id)?;
            error = self.do_custom_encoding(font_id, &enc_vec, &usedchars, &mut sfont)?;
        }
        if error != 0 {
            fatal!("Error occured while creating font subfont");
        }

        for (name, must_exist) in REQUIRED_TABLE {
            if sfont.sfnt_require_table(name, must_exist) < 0 {
                fatal!(
                    "Required TrueType table \"{}\" does not exist in font",
                    alloc::string::String::from_utf8_lossy(name)
                );
            }
        }

        // FontFile2
        let Some(fontfile) = self.sfnt_create_FontFile_stream(&mut sfont)? else {
            fatal!("Could not created FontFile stream");
        };

        drop(sfont);

        let descriptor = descriptor.expect("pdf_font_load_truetype: no descriptor");
        let r = self.o.ref_obj(fontfile)?;
        self.o.put(descriptor, b"FontFile2", r)?; /* XXX */
        self.o.release(fontfile)?;

        Ok(0)
    }

    /// Glyph runs: the glyph id `pdf_font_load_truetype` draws each code
    /// of TrueType font `ident` (face `index`) with, by code: with no
    /// encoding (`encoding` none), the code in the Mac Roman `cmap`
    /// (`do_builtin_encoding`); else its name in `encoding`, through
    /// `post`, the Unicode `cmap` and GSUB (`do_custom_encoding`). 0 where
    /// none is found (C's `do_custom_encoding` keeps the previous code's
    /// glyph then). None if the font does not open.
    pub fn tt_code_gids(
        &mut self,
        ident: &[u8],
        index: i32,
        encoding: Option<&[Option<Vec<u8>>]>,
    ) -> Result<Option<Vec<u16>>> {
        let sfont = if let Some(fp) = self.dpx_open_file(ident, ResType::TtFont)? {
            Sfnt::sfnt_open(fp)?
        } else if let Some(fp) = self.dpx_open_file(ident, ResType::DFont)? {
            Sfnt::dfont_open(fp, index)?
        } else {
            return Ok(None);
        };
        let mut sfont = some!(sfont);
        if sfont.type_ != SFNT_TYPE_TRUETYPE
            && sfont.type_ != SFNT_TYPE_TTC
            && sfont.type_ != SFNT_TYPE_DFONT
        {
            return Ok(None);
        }
        let offset = if sfont.type_ == SFNT_TYPE_TTC {
            sfont.ttc_read_offset(index as ULONG)?
        } else {
            sfont.offset
        };
        if sfont.sfnt_read_table_directory(offset)? != 0 {
            return Ok(None);
        }
        let mut gids = vec![0u16; 256];
        let Some(encoding) = encoding else {
            let ttcm = some!(sfont.tt_cmap_read(TT_MAC, TT_MAC_ROMAN)?);
            for (code, g) in gids.iter_mut().enumerate() {
                *g = ttcm.tt_cmap_lookup(code as ULONG);
            }
            return Ok(Some(gids));
        };
        let (error, mut gm) = setup_glyph_mapper(&mut sfont)?;
        if error != 0 {
            return Ok(None);
        }
        for (code, g) in gids.iter_mut().enumerate() {
            let Some(name) = encoding.get(code).and_then(|e| e.as_deref()) else {
                continue;
            };
            if c_str(name) == b".notdef" {
                continue;
            }
            let mut gid: USHORT = 0;
            let error = if is_comp(name) {
                self.findcomposite(name, &mut gid, &mut gm)?
            } else {
                self.resolve_glyph(name, &mut gid, &mut gm)?
            };
            *g = if error == 0 { gid } else { 0 };
        }
        Ok(Some(gids))
    }

    /// `do_widths` (static): /Widths, /FirstChar, /LastChar of the font.
    fn do_widths(&mut self, font_id: i32, widths: &mut [f64; 256]) -> Result<()> {
        let (fontdict, usedchars, ident) = {
            let font = &self.font.fonts[font_id as usize];
            (
                font.resource.expect("do_widths: no resource"),
                font.usedchars
                    .as_ref()
                    .expect("do_widths: no usedchars")
                    .borrow()
                    .clone(),
                font.ident.clone().unwrap_or_default(),
            )
        };

        let mut firstchar = 255;
        let mut lastchar = 0;
        for code in 0..256 {
            if usedchars[code as usize] != 0 {
                if code < firstchar {
                    firstchar = code;
                }
                if code > lastchar {
                    lastchar = code;
                }
            }
        }
        if firstchar > lastchar {
            warn!("No glyphs actually used???");
            return Ok(());
        }

        self.pdf_check_tfm_widths(&ident, widths, firstchar, lastchar, &usedchars)?;

        let array = self.o.new_array();
        for code in firstchar..=lastchar {
            let v = if usedchars[code as usize] != 0 {
                round_acc(widths[code as usize], 0.1)
            } else {
                0.0
            };
            let n = self.o.new_number(v);
            self.o.add_array(array, n)?;
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

    /// The widths of the used codes (`code_to_idx` their new gids), in
    /// 1/1000 em, as both `do_*_encoding` compute them.
    fn tt_code_widths(
        usedchars: &[u8],
        glyphs: &TtGlyphs,
        code_to_idx: &[USHORT; 256],
    ) -> [f64; 256] {
        let mut widths = [0.0f64; 256];
        for code in 0..256 {
            if usedchars[code] != 0 {
                let idx = glyphs.tt_get_index(code_to_idx[code]);
                widths[code] = pdfunit(f64::from(glyphs.gd[idx as usize].advw), glyphs.emsize);
            } else {
                widths[code] = 0.0;
            }
        }
        widths
    }

    /// `do_builtin_encoding` (static): subset by the (1,0) cmap.
    fn do_builtin_encoding(
        &mut self,
        font_id: i32,
        usedchars: &[u8],
        sfont: &mut Sfnt,
    ) -> Result<i32> {
        let mut code_to_idx: [USHORT; 256] = [0; 256];

        let Some(ttcm) = sfont.tt_cmap_read(TT_MAC, TT_MAC_ROMAN)? else {
            warn!("Could not read Mac-Roman TrueType cmap table...");
            return Ok(-1);
        };

        let mut cmap_table = new_cmap_table();

        let mut glyphs = TtGlyphs::tt_build_init()?;

        let mut count: i32 = 1; /* .notdef */
        for code in 0..256usize {
            if usedchars[code] == 0 {
                continue;
            }

            let gid = ttcm.tt_cmap_lookup(code as ULONG);
            let idx;
            if gid == 0 {
                warn!("Glyph for character code=0x{:02x} missing in font.", code);
                idx = 0;
            } else {
                let mut i = glyphs.tt_find_glyph(gid);
                if i == 0 {
                    i = glyphs.tt_add_glyph(gid, count as USHORT)?; /* count returned. */
                }
                idx = i;
            }
            code_to_idx[code] = idx;
            sfnt_put_ushort(&mut cmap_table[30 + 2 * code..], idx);
            sfnt_put_ushort(&mut cmap_table[574 + 2 * code..], idx);
            count += 1;
        }
        drop(ttcm);

        if sfont.tt_build_tables(&mut glyphs)? < 0 {
            warn!("Packing TrueType font into SFNT failed!");
            return Ok(-1);
        }

        let mut widths = Self::tt_code_widths(usedchars, &glyphs, &code_to_idx);
        self.do_widths(font_id, &mut widths)?;

        glyphs.tt_build_finish();

        sfont.sfnt_set_table(b"cmap", cmap_table);

        Ok(0)
    }

    /// `do_custom_encoding` (static): subset by glyph names; `encoding` is
    /// the 256 glyph names of the encoding (`pdf_encoding_get_encoding`).
    fn do_custom_encoding(
        &mut self,
        font_id: i32,
        encoding: &[Option<Vec<u8>>],
        usedchars: &[u8],
        sfont: &mut Sfnt,
    ) -> Result<i32> {
        let mut code_to_idx: [USHORT; 256] = [0; 256];
        // C's uninitialized `gid`: kept across codes, as in C.
        let mut gid: USHORT = 0;

        let mut glyphs;
        let cmap_table;
        {
            let (error, mut gm) = setup_glyph_mapper(sfont)?;
            if error != 0 {
                warn!("No post table nor Unicode cmap found in font");
                warn!(">> I can't find glyphs without this!");
                return Ok(-1);
            }

            let mut table = new_cmap_table();

            glyphs = TtGlyphs::tt_build_init()?;

            let mut count: i32 = 1; /* +1 for .notdef */
            for code in 0..256usize {
                if usedchars[code] == 0 {
                    continue;
                }

                let idx;
                match encoding.get(code).and_then(|e| e.as_deref()) {
                    Some(name) if c_str(name) != b".notdef" => {
                        let error = if is_comp(name) {
                            self.findcomposite(name, &mut gid, &mut gm)?
                        } else {
                            self.resolve_glyph(name, &mut gid, &mut gm)?
                        };

                        // Older versions of gs had problem with glyphs (other
                        // than .notdef) mapped to gid = 0.
                        if error != 0 {
                            warn!("Glyph not available in font.");
                        }
                        let mut i = glyphs.tt_find_glyph(gid);
                        if i == 0 {
                            i = glyphs.tt_add_glyph(gid, count as USHORT)?; /* count returned. */
                            count += 1;
                        }
                        idx = i;
                    }
                    _ => {
                        warn!(
                            "Character code=\"0x{:02X}\" mapped to \".notdef\" glyph used in font",
                            code
                        );
                        warn!(">> Maybe incorrect encoding specified?");
                        idx = 0;
                    }
                }
                code_to_idx[code] = idx;
                sfnt_put_ushort(&mut table[30 + 2 * code..], idx);
                sfnt_put_ushort(&mut table[574 + 2 * code..], idx);
            }
            clean_glyph_mapper(gm);
            cmap_table = table;
        }

        if sfont.tt_build_tables(&mut glyphs)? < 0 {
            warn!("Packing TrueType font into SFNT file faild...");
            return Ok(-1);
        }

        let mut widths = Self::tt_code_widths(usedchars, &glyphs, &code_to_idx);
        self.do_widths(font_id, &mut widths)?;

        glyphs.tt_build_finish();

        sfont.sfnt_set_table(b"cmap", cmap_table);

        Ok(0)
    }

    /// `selectglyph` (static): the variant of gid `in_` for `suffix`
    /// (`.sc`, `.onum`, …, through GSUB) into `out`; status.
    fn selectglyph(
        &mut self,
        in_: USHORT,
        suffix: &[u8],
        gm: &mut GlyphMapper<'_>,
        out: &mut USHORT,
    ) -> Result<i32> {
        let mut in_ = in_;
        let mut error;
        let mut s = c_str(suffix).to_vec();

        // First try converting suffix to feature tag. agl.c currently only
        // knows less ambiguous cases; e.g., 'sc', 'superior', etc.
        if let Some(r) = agl_suffix_to_otltag(&s) {
            // We found feature tag for 'suffix'.
            error = select_gsub(r, gm)?; /* no fallback for this */
            if error == 0 {
                error = gm_apply(gm, &mut in_)?;
            }
        } else {
            // 'suffix' may represent feature tag. Try loading GSUB only
            // when length of 'suffix' is less than or equal to 4.
            if s.len() > 4 {
                error = -1; /* Uh */
            } else if s.len() == 4 {
                error = select_gsub(&s, gm)?;
            } else {
                // Less than 4: pad ' '.
                let mut t = *b"    ";
                t[..s.len()].copy_from_slice(&s);
                error = select_gsub(&t, gm)?;
            }
            if error == 0 {
                // 'suffix' represents feature tag.
                error = gm_apply(gm, &mut in_)?;
            } else {
                // Other case: alt1, nalt10... (alternates).
                // q = s + strlen(s) - 1; back over the digits, not to s.
                let mut q = s.len() as isize - 1;
                while q > 0 && s[q as usize].is_ascii_digit() {
                    q -= 1;
                }
                if q == 0 {
                    error = -1;
                } else {
                    // Starting at 1 (for an empty suffix C's q is s - 1).
                    let qn = (q + 1) as usize;
                    let n = (crate::fmt::atoi(&s[qn..]) as i32) - 1;
                    s.truncate(qn);
                    if s.len() > 4 {
                        error = -1;
                    } else {
                        // This may be alternate substitution (C pads into
                        // `t` but selects `s`).
                        error = select_gsub(&s, gm)?;
                        if error == 0 {
                            error = match gm.gsub.as_ref() {
                                Some(g) => g.otl_gsub_apply_alt(n as USHORT, &mut in_)?,
                                None => -1,
                            };
                        }
                    }
                }
            }
        }

        *out = in_;
        Ok(error)
    }

    /// `findcomposite` (static): `a_b_c` names through ligatures.
    fn findcomposite(
        &mut self,
        glyphname: &[u8],
        gid: &mut USHORT,
        gm: &mut GlyphMapper<'_>,
    ) -> Result<i32> {
        let mut error = findposttable(glyphname, gid, gm);
        if error == 0 {
            return Ok(0);
        }

        let mut gids: [USHORT; 32] = [0; 32];
        let (n_comp, nptrs, suffix) = agl_decompose_glyphname(glyphname, 32)?;
        error = 0;
        let mut i = 0usize;
        while error == 0 && (i as i32) < n_comp {
            error = self.resolve_glyph(&nptrs[i], &mut gids[i], gm)?;
            if error != 0 {
                warn!("Could not resolve glyph ({}th component).", i);
            }
            i += 1;
        }

        if error == 0 {
            let n = n_comp as usize;
            match suffix.as_deref() {
                Some(s @ (b"liga" | b"dlig" | b"hlig" | b"frac" | b"ccmp" | b"afrc")) => {
                    error = composeglyph(&gids[..n], Some(s), gm, gid)?;
                }
                _ => {
                    // First try composing glyph.
                    error = composeglyph(&gids[..n], None, gm, gid)?;
                    if error == 0 {
                        if let Some(s) = suffix.as_deref() {
                            // a_b_c.vert
                            let g = *gid;
                            error = self.selectglyph(g, s, gm, gid)?;
                        }
                    }
                }
            }
        }

        Ok(error)
    }

    /// `findparanoiac` (static): AGL lookups, alternates, compositions
    /// (`glyphname` has no suffix here).
    fn findparanoiac(
        &mut self,
        glyphname: &[u8],
        gid: &mut USHORT,
        gm: &mut GlyphMapper<'_>,
    ) -> Result<i32> {
        let mut idx: USHORT = 0;
        let mut error;

        let mut agln = self.agl_lookup_list(glyphname);
        while let Some(a) = agln {
            if idx != 0 {
                break;
            }
            if let Some(suffix) = a.suffix.as_deref() {
                let name = a.name.clone().unwrap_or_default();
                error = self.findparanoiac(&name, &mut idx, gm)?;
                if error != 0 {
                    return Ok(error);
                }

                let g = idx;
                error = self.selectglyph(g, suffix, gm, &mut idx)?;
                if error != 0 {
                    warn!("Variant for glyph might not be found.");
                    warn!("Using glyph name without suffix instead...");
                }
            } else if a.n_components == 1 {
                idx = gm
                    .codetogid
                    .as_ref()
                    .expect("findparanoiac: no Unicode cmap")
                    .tt_cmap_lookup(a.unicodes[0] as ULONG);
            } else if a.n_components > 1 {
                let n = a.n_components as usize;
                error = composeuchar(&a.unicodes[..n], None, gm, &mut idx)?;
                if self.conf.verbose_level >= 0 {
                    if error != 0 {
                        warn!("Not found...");
                    } else {
                        warn!(">> Composite glyph found at glyph-id=\"{}\".", idx);
                    }
                }
            } else {
                panic!("findparanoiac: an AGL entry without components"); /* Boooo */
            }
            agln = a.alternate.map(|b| *b);
        }

        *gid = idx;
        if idx == 0 { Ok(-1) } else { Ok(0) }
    }

    /// `resolve_glyph` (static): a glyph name to a gid; status.
    fn resolve_glyph(
        &mut self,
        glyphname: &[u8],
        gid: &mut USHORT,
        gm: &mut GlyphMapper<'_>,
    ) -> Result<i32> {
        // First the post table; then Unicode if the Windows-Unicode cmap is
        // available.
        let mut error = findposttable(glyphname, gid, gm);
        if error == 0 {
            return Ok(0);
        }

        if gm.codetogid.is_none() {
            return Ok(-1);
        }

        let (name, suffix) = agl_chop_suffix(c_str(glyphname));
        match name.as_deref() {
            None => {
                // .notdef, .foo
                error = -1;
            }
            Some(name) if agl_name_is_unicode(name) => {
                let ucv = agl_name_convert_unicode(name);
                *gid = gm.codetogid.as_ref().unwrap().tt_cmap_lookup(ucv as ULONG);
                error = if *gid == 0 { -1 } else { 0 };
            }
            Some(name) => {
                error = self.findparanoiac(name, gid, gm)?;
            }
        }
        if error == 0 {
            if let Some(suffix) = suffix.as_deref() {
                let g = *gid;
                error = self.selectglyph(g, suffix, gm, gid)?;
                if error != 0 {
                    warn!("Variant for glyph might not be found.");
                    warn!("Using glyph name without suffix instead...");
                    error = 0; /* ignore */
                }
            }
        }

        Ok(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &[u8]) -> (i32, Vec<Vec<u8>>, Option<Vec<u8>>) {
        agl_decompose_glyphname(s, 32).unwrap()
    }

    #[test]
    fn decompose() {
        assert_eq!(
            d(b"a_b_c.liga"),
            (
                3,
                vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()],
                Some(b"liga".to_vec())
            )
        );
        assert_eq!(d(b"f_i"), (2, vec![b"f".to_vec(), b"i".to_vec()], None));
        // A trailing '_' is not split.
        assert_eq!(d(b"a_"), (1, vec![b"a_".to_vec()], None));
        assert_eq!(d(b"_a"), (2, vec![b"".to_vec(), b"a".to_vec()], None));
        assert_eq!(d(b""), (1, vec![b"".to_vec()], None));
    }

    #[test]
    fn cmap_table() {
        let t = new_cmap_table();
        assert_eq!(t.len(), 1086);
        assert_eq!(&t[542..544], &[0, 4]);
        assert_eq!(&t[566..570], &[0, 0, 0, 1]);
    }
}
