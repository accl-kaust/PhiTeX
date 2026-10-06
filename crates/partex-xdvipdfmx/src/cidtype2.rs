//! cidtype2.c, cidtype2.h: CIDFontType2 (TrueType CIDFonts).
//!
//! `pdf_font *` is a `font_id: i32` into `self.font.fonts`; a `CMap *`
//! from the cache is a cmap id (-1 for C's NULL), passed on as
//! `Option<&CMap>` (`CMap_cache_get`) to the decoding helpers.

#![allow(non_snake_case)]

use crate::cid::CIDFONT_FORCE_FIXEDPITCH;
use crate::cmap::{CMap, Cid};
use crate::dpxfile::ResType;
use crate::fmt::round_acc;
use crate::obj::STREAM_COMPRESS;
use crate::pdffont::{
    CidOpt, FONT_STYLE_BOLD, FONT_STYLE_BOLDITALIC, FONT_STYLE_ITALIC, FONT_STYLE_NONE,
    PDF_FONT_FLAG_BASEFONT, PDF_FONT_FONTTYPE_CIDTYPE2, add_to_used_chars2, is_used_char2,
};
use crate::prelude::*;
use crate::sfnt::{SFNT_TYPE_DFONT, SFNT_TYPE_TRUETYPE, SFNT_TYPE_TTC, Sfnt};
use crate::tt_cmap::{TtCmap, tt_cmap_uvs_lookup};
use crate::tt_glyf::TtGlyphs;
use crate::tt_gsub::OtlGsub;
use crate::unicode::{UC_Combine_CJK_compatibility_ideograph, UC_UTF16BE_decode_char};

/// `PDF_NAME_LEN_MAX` (cidtype2.c).
pub const PDF_NAME_LEN_MAX: usize = 255;

/// `required_table`: `(tag, must_exist)` for the embedded font.
pub const REQUIRED_TABLE: &[(&[u8; 4], bool)] = &[
    (b"OS/2", false),
    (b"head", true),
    (b"hhea", true),
    (b"loca", true),
    (b"maxp", true),
    (b"name", true),
    (b"glyf", true),
    (b"hmtx", true),
    (b"fpgm", false),
    (b"cvt ", false),
    (b"prep", false),
];

/// `validate_name`'s `badstrlist`.
pub const BADSTRLIST: &[&[u8]] = &[
    b"-WIN-RKSJ-H",
    b"-WINP-RKSJ-H",
    b"-WING-RKSJ-H",
    b"-90pv-RKSJ-H",
];

/// `WIN_UCS_INDEX_MAX`.
pub const WIN_UCS_INDEX_MAX: i32 = 1;
/// `KNOWN_ENCODINGS_MAX`.
pub const KNOWN_ENCODINGS_MAX: i32 = 10;

/// `known_encodings`: `(platform, encoding, pdfnames)` (TT_WIN = 3,
/// TT_MAC = 1; without C's NULL end).
pub const KNOWN_ENCODINGS: &[(u16, u16, &[&[u8]])] = &[
    (3, 10, &[b"UCSms-UCS4", b"UCSms-UCS2", b"UCS4", b"UCS2"]), // TT_WIN, TT_WIN_UCS4
    (3, 1, &[b"UCSms-UCS4", b"UCSms-UCS2", b"UCS4", b"UCS2"]),  // TT_WIN, TT_WIN_UNICODE
    (3, 2, &[b"90ms-RKSJ"]),                                    // TT_WIN, TT_WIN_SJIS
    (3, 3, &[b"GBK-EUC"]),                                      // TT_WIN, TT_WIN_RPC
    (3, 4, &[b"ETen-B5"]),                                      // TT_WIN, TT_WIN_BIG5
    (3, 5, &[b"KSCms-UHC"]),                                    // TT_WIN, TT_WIN_WANSUNG
    (1, 1, &[b"90pv-RKSJ"]),                                    // TT_MAC, TT_MAC_JAPANESE
    (1, 2, &[b"B5pc"]),      // TT_MAC, TT_MAC_TRADITIONAL_CHINESE
    (1, 25, &[b"GBpc-EUC"]), // TT_MAC, TT_MAC_SIMPLIFIED_CHINESE
    (1, 3, &[b"KSCpc-EUC"]), // TT_MAC, TT_MAC_KOREAN
    (0, 4, &[b"UCSms-UCS4", b"UCSms-UCS2", b"UCS4", b"UCS2"]),
];

/// `FIX_CJK_UNIOCDE_SYMBOLS`.
pub const FIX_CJK_UNIOCDE_SYMBOLS: i32 = 1;

/// `fix_CJK_symbols`'s `CJK_Uni_symbols`: `(alt1, alt2)` Microsoft/Apple
/// Unicode mapping differences (the last is C's EOD, kept: C loops over
/// all of them).
pub const CJK_UNI_SYMBOLS: &[(u16, u16)] = &[
    (0x2014, 0x2015),
    (0x2016, 0x2225),
    (0x203E, 0xFFE3),
    (0x2026, 0x22EF),
    (0x2212, 0xFF0D),
    (0x301C, 0xFF5E),
    (0xFFE0, 0x00A2),
    (0xFFE1, 0x00A3),
    (0xFFE2, 0x00AC),
    (0xFFE5, 0x00A5),
    (0xFFFF, 0xFFFF),
];

/// `validate_name` (static): removes NULs and a bad suffix from
/// `fontname[..len]` in place; 0, or -1 if nothing is left. On return
/// `fontname` is what C's buffer holds as a C string (what the caller's
/// `strlen` sees).
fn validate_name(fontname: &mut Vec<u8>, len: i32) -> i32 {
    let mut len = len.max(0) as usize;
    // C's buffer: the name, its NUL, and more NULs.
    let mut buf = fontname.clone();
    if buf.len() < len + 2 {
        buf.resize(len + 2, 0);
    }

    let mut count = 0;
    let mut i = 0usize;
    while i < len {
        if buf[i] == 0 {
            // memmove(fontname + i, fontname + i + 1, len - i)
            buf.copy_within(i + 1..i + 1 + (len - i), i);
            count += 1;
            len -= 1;
        }
        i += 1;
    }
    if count > 0 {
        warn!("Removed {} null character(s) from fontname", count);
    }
    buf[len] = 0;

    // For some fonts that have bad PS name. ad hoc. remove me.
    let cstr_len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    for bad in BADSTRLIST {
        let s = &buf[..cstr_len];
        if let Some(p) = s.windows(bad.len()).position(|w| w == *bad) {
            if p > 0 {
                warn!("Removed bad string from fontname.");
                buf[p] = 0;
                len = p;
                break;
            }
        }
    }

    let cstr_len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    buf.truncate(cstr_len);
    *fontname = buf;

    if len < 1 {
        warn!("No valid character found in fontname string.");
        return -1;
    }

    0
}

/// `fix_CJK_symbols` (static).
fn fix_CJK_symbols(code: u16) -> u16 {
    let mut alt_code = code;
    for &(alt1, alt2) in CJK_UNI_SYMBOLS {
        if alt1 == code {
            alt_code = alt2;
            break;
        } else if alt2 == code {
            alt_code = alt1;
            break;
        }
    }
    alt_code
}

/// `PDFUNIT(v)` (cidtype2.c): `v` font units in 1/1000 em.
fn pdfunit(v: f64, emsize: u16) -> f64 {
    round_acc(1000.0 * v / f64::from(emsize), 1.0)
}

/// `glyph_ordering`, `via_cid_to_code`, `via_cid_to_gid`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MapType {
    GlyphOrdering,
    ViaCidToCode,
    ViaCidToGid,
}

impl Dpx {
    /// `find_tocode_cmap` (static): `<reg>-<ord>-<pdfname>` from
    /// `KNOWN_ENCODINGS[select]`; a cache id, or -1 (C's NULL).
    fn find_tocode_cmap(
        &mut self,
        reg: Option<&[u8]>,
        ord: Option<&[u8]>,
        select: i32,
    ) -> Result<i32> {
        let mut cmap_id = -1;

        let (Some(reg), Some(ord)) = (reg, ord) else {
            warn!("Character set unknown.");
            return Ok(-1);
        };
        if select < 0 || select > KNOWN_ENCODINGS_MAX {
            warn!("Character set unknown.");
            return Ok(-1);
        }

        let pdfnames = KNOWN_ENCODINGS[select as usize].2;
        let mut i = 0;
        while cmap_id < 0 && i < 5 {
            let Some(append) = pdfnames.get(i) else {
                break;
            };
            let mut cmap_name = reg.to_vec();
            cmap_name.push(b'-');
            cmap_name.extend_from_slice(ord);
            cmap_name.push(b'-');
            cmap_name.extend_from_slice(append);
            cmap_id = self.CMap_cache_find(&cmap_name)?;
            i += 1;
        }
        if cmap_id < 0 {
            warn!("Could not find CID-to-Code mapping.");
            return Ok(-1);
        }

        Ok(cmap_id)
    }

    /// `add_TTCIDHMetrics` (static): `/DW`, `/W`.
    fn add_TTCIDHMetrics(
        &mut self,
        fontdict: Obj,
        g: &TtGlyphs,
        used_chars: &[u8],
        cidtogidmap: Option<&[u8]>,
        last_cid: u16,
    ) -> Result<()> {
        let mut start: i32 = 0;
        let mut prev: i32 = 0;
        let mut an_array: Option<Obj> = None;
        let mut empty = true;

        let w_array = self.o.new_array();
        let dw = if g.dw != 0 && g.dw <= g.emsize {
            pdfunit(f64::from(g.dw), g.emsize)
        } else {
            pdfunit(f64::from(g.gd[0].advw), g.emsize)
        };
        for cid in 0..=i32::from(last_cid) {
            if !is_used_char2(used_chars, cid as u32) {
                continue;
            }
            let gid: u16 = match cidtogidmap {
                Some(m) => {
                    (u16::from(m[2 * cid as usize]) << 8) | u16::from(m[2 * cid as usize + 1])
                }
                None => cid as u16,
            };
            let idx = g.tt_get_index(gid);
            if cid != 0 && idx == 0 {
                continue;
            }
            let width = pdfunit(f64::from(g.gd[idx as usize].advw), g.emsize);
            if width == dw {
                if let Some(a) = an_array.take() {
                    let n = self.o.new_number(f64::from(start));
                    self.o.add_array(w_array, n)?;
                    self.o.add_array(w_array, a)?;
                    empty = false;
                }
            } else {
                if cid != prev + 1 {
                    if let Some(a) = an_array.take() {
                        let n = self.o.new_number(f64::from(start));
                        self.o.add_array(w_array, n)?;
                        self.o.add_array(w_array, a)?;
                        empty = false;
                    }
                }
                let a = match an_array {
                    Some(a) => a,
                    None => {
                        start = cid;
                        let a = self.o.new_array();
                        an_array = Some(a);
                        a
                    }
                };
                let n = self.o.new_number(width);
                self.o.add_array(a, n)?;
                prev = cid;
            }
        }

        if let Some(a) = an_array {
            let n = self.o.new_number(f64::from(start));
            self.o.add_array(w_array, n)?;
            self.o.add_array(w_array, a)?;
            empty = false;
        }

        self.o.put_number(fontdict, b"DW", dw)?;
        if !empty {
            let r = self.o.ref_obj(w_array)?;
            self.o.put(fontdict, b"W", r)?;
        }
        self.o.release(w_array)?;
        Ok(())
    }

    /// `add_TTCIDVMetrics` (static): `/DW2`, `/W2`.
    fn add_TTCIDVMetrics(
        &mut self,
        fontdict: Obj,
        g: &TtGlyphs,
        used_chars: &[u8],
        last_cid: u16,
    ) -> Result<()> {
        let mut empty = true;

        let default_vert_origin_y = pdfunit(
            f64::from(i32::from(g.default_advh) - i32::from(g.default_tsb)),
            g.emsize,
        );
        let default_advance_height = pdfunit(f64::from(g.default_advh), g.emsize);

        let w2_array = self.o.new_array();
        for cid in 0..=i32::from(last_cid) {
            if !is_used_char2(used_chars, cid as u32) {
                continue;
            }
            let idx = g.tt_get_index(cid as u16);
            if cid != 0 && idx == 0 {
                continue;
            }
            let d = &g.gd[idx as usize];
            let advance_height = pdfunit(f64::from(d.advh), g.emsize);
            let vert_origin_x = pdfunit(0.5 * f64::from(d.advw), g.emsize);
            let vert_origin_y = pdfunit(f64::from(i32::from(d.tsb) + i32::from(d.ury)), g.emsize);
            // c_first c_last w1_y v_x v_y
            if vert_origin_y != default_vert_origin_y || advance_height != default_advance_height {
                for v in [
                    f64::from(cid),
                    f64::from(cid),
                    -advance_height,
                    vert_origin_x,
                    vert_origin_y,
                ] {
                    let n = self.o.new_number(v);
                    self.o.add_array(w2_array, n)?;
                }
                empty = false;
            }
        }

        if default_vert_origin_y != 880.0 || default_advance_height != 1000.0 {
            let an_array = self.o.new_array();
            let n = self.o.new_number(default_vert_origin_y);
            self.o.add_array(an_array, n)?;
            let n = self.o.new_number(-default_advance_height);
            self.o.add_array(an_array, n)?;
            self.o.put(fontdict, b"DW2", an_array)?;
        }
        if !empty {
            let r = self.o.ref_obj(w2_array)?;
            self.o.put(fontdict, b"W2", r)?;
        }
        self.o.release(w2_array)?;
        Ok(())
    }

    /// `cid_to_code` (static): the code (or -1) and C's `*puvs` (a
    /// variation selector, or -1). `cmap` none returns `cid`.
    fn cid_to_code(&self, cmap: Option<&CMap>, cid: Cid, unicode_cmap: i32) -> Result<(i32, i32)> {
        let mut puvs = -1;

        let Some(cmap) = cmap else {
            return Ok((i32::from(cid), puvs));
        };

        let inbuf = [(cid >> 8) as u8, (cid & 0xff) as u8];
        let mut outbuf = [0u8; 32];
        let mut inpos = 0usize;
        let mut inbytesleft = 2;
        let mut outpos = 0usize;
        let mut outbytesleft = 32;

        self.CMap_decode_char(
            cmap,
            &inbuf,
            &mut inpos,
            &mut inbytesleft,
            &mut outbuf,
            &mut outpos,
            &mut outbytesleft,
        )?;

        if inbytesleft != 0 {
            return Ok((-1, puvs));
        } else if outbytesleft == 31 {
            return Ok((i32::from(outbuf[0]), puvs));
        } else if outbytesleft == 30 {
            return Ok(((i32::from(outbuf[0]) << 8) | i32::from(outbuf[1]), puvs));
        } else if outbytesleft == 28 && unicode_cmap == 0 {
            return Ok((
                ((u32::from(outbuf[0]) << 24)
                    | (u32::from(outbuf[1]) << 16)
                    | (u32::from(outbuf[2]) << 8)
                    | u32::from(outbuf[3])) as i32,
                puvs,
            ));
        } else if (outbytesleft == 28 || outbytesleft == 26 || outbytesleft == 24)
            && unicode_cmap != 0
        {
            // We assume the output encoding is UTF-16.
            let endptr = (32 - outbytesleft) as usize;
            let s = &outbuf[..endptr];
            let mut p = 0usize;
            let uc = UC_UTF16BE_decode_char(s, &mut p);
            if p == endptr {
                return Ok((uc, puvs)); /* single Unicode characters */
            }
            // Check following Variation Selectors.
            let uvs = UC_UTF16BE_decode_char(s, &mut p);
            if p == endptr {
                if (0xfe00..=0xfe0f).contains(&uvs) {
                    // Standardized Variation Sequence.
                    puvs = uvs;
                    return Ok((uc, puvs));
                } else if (0xe0100..=0xe01ef).contains(&uvs) {
                    // Ideographic Variation Sequence.
                    puvs = uvs;
                    return Ok((uc, puvs));
                } else if uvs == 0x3099 || uvs == 0x309a {
                    // Combining Katakana-Hiragana (Semi-)Voiced Sound Mark.
                    puvs = uvs;
                    return Ok((uc, puvs));
                }
            }
            warn!("CID={} mapped to non-single Unicode characters...", cid);
            return Ok((-1, puvs));
        }

        Ok((-1, puvs))
    }

    /// `cid_to_gid` (static): `cmap` none returns `cid`.
    fn cid_to_gid(&self, cmap: Option<&CMap>, cid: Cid) -> Result<u16> {
        let Some(cmap) = cmap else {
            return Ok(cid);
        };

        let inbuf = [(cid >> 8) as u8, (cid & 0xff) as u8];
        let mut outbuf = [0u8; 2];
        let mut inpos = 0usize;
        let mut inbytesleft = 2;
        let mut outpos = 0usize;
        let mut outbytesleft = 2;

        self.CMap_decode_char(
            cmap,
            &inbuf,
            &mut inpos,
            &mut inbytesleft,
            &mut outbuf,
            &mut outpos,
            &mut outbytesleft,
        )?;

        if inbytesleft != 0 || outbytesleft != 0 {
            return Ok(0);
        }

        Ok((u16::from(outbuf[0]) << 8) | u16::from(outbuf[1]))
    }

    /// The gid of `cid` (and its code, for C's warnings), the part of the
    /// horizontal and vertical loops of `CIDFont_type2_dofont` before
    /// `tt_add_glyph`.
    fn cidtype2_map_cid(
        &self,
        maptype: MapType,
        cmap_id: i32,
        ttcmap: Option<&TtCmap>,
        ttcmap_uvs: Option<&TtCmap>,
        unicode_cmap: i32,
        cid: Cid,
    ) -> Result<(i32, u16)> {
        let cmap = if cmap_id >= 0 {
            Some(self.CMap_cache_get(cmap_id)?)
        } else {
            None
        };
        let mut gid: u16 = 0;
        let code;
        match maptype {
            MapType::GlyphOrdering => {
                gid = cid;
                code = i32::from(cid);
            }
            MapType::ViaCidToGid => {
                gid = self.cid_to_gid(cmap, cid)?;
                code = i32::from(cid);
            }
            MapType::ViaCidToCode => {
                let (c, uvs) = self.cid_to_code(cmap, cid, unicode_cmap)?;
                code = c;
                if code < 0 {
                    warn!("Unable to map CID to code: CID={}", cid);
                } else {
                    let ttcmap = ttcmap.expect("CIDFont_type2_dofont: no cmap");
                    if let Some(ttcmap_uvs) = ttcmap_uvs {
                        if uvs > 0 {
                            gid = tt_cmap_uvs_lookup(
                                ttcmap_uvs,
                                Some(ttcmap),
                                code as u32,
                                uvs as u32,
                            );
                        }
                    }
                    if gid == 0 && (0xfe00..=0xfe0f).contains(&uvs) {
                        // Standardized Variation Sequence: combine CJK
                        // compatibility ideograph.
                        let code2 = UC_Combine_CJK_compatibility_ideograph(code, uvs);
                        if code2 > 0 {
                            gid = ttcmap.tt_cmap_lookup(code2 as u32);
                        }
                    }
                    if gid == 0 {
                        gid = ttcmap.tt_cmap_lookup(code as u32);
                        if gid > 0 && uvs > 0 {
                            warn!("Ignored Variation Selector: CID={}", cid);
                        }
                    }
                    if FIX_CJK_UNIOCDE_SYMBOLS != 0
                        && gid == 0
                        && unicode_cmap != 0
                        && code <= 0xFFFF
                    {
                        let alt_code = i32::from(fix_CJK_symbols(code as u16));
                        if alt_code != code {
                            gid = ttcmap.tt_cmap_lookup(alt_code as u32);
                            if gid != 0 {
                                warn!("Unicode char replaced.");
                            }
                        }
                    }
                }
            }
        }
        if gid == 0 && code >= 0 {
            warn!("Glyph missing in font. (CID={}, code=0x{:04x})", cid, code);
        }
        Ok((code, gid))
    }

    /// `CIDFont_type2_dofont`.
    pub fn CIDFont_type2_dofont(&mut self, font_id: i32) -> Result<i32> {
        let fid = font_id as usize;
        let mut unicode_cmap = 0;

        if self.font.fonts[fid].reference.is_none() {
            return Ok(0);
        }

        let resource = self.font.fonts[fid]
            .resource
            .expect("CIDFont_type2_dofont: no resource");
        let descriptor = self.font.fonts[fid]
            .descriptor
            .expect("CIDFont_type2_dofont: no descriptor");
        let r = self.o.ref_obj(descriptor)?;
        self.o.put(resource, b"FontDescriptor", r)?;

        if self.font.fonts[fid].flags & PDF_FONT_FLAG_BASEFONT != 0 {
            return Ok(0);
        }

        // CIDSystemInfo comes here since Supplement can be increased.
        let csi = self.font.fonts[fid].cid.csi.clone();
        {
            let tmp = self.o.new_dict();
            self.o.put_string(
                tmp,
                b"Registry",
                csi.registry.as_deref().unwrap_or_default(),
            )?;
            self.o.put_string(
                tmp,
                b"Ordering",
                csi.ordering.as_deref().unwrap_or_default(),
            )?;
            self.o
                .put_number(tmp, b"Supplement", f64::from(csi.supplement))?;
            self.o.put(resource, b"CIDSystemInfo", tmp)?;
        }

        let embed = self.font.fonts[fid].cid.options.embed;
        // Quick exit for non-embedded & fixed-pitch font.
        if embed == 0 && (self.cid.opt_flags_cidfont & CIDFONT_FORCE_FIXEDPITCH) != 0 {
            self.o.put_number(resource, b"DW", 1000.0)?;
            return Ok(0);
        }

        let filename = self.font.fonts[fid].filename.clone().unwrap_or_default();
        let index = self.font.fonts[fid].index;
        let sfont = if let Some(fp) = self.dpx_open_file(&filename, ResType::TtFont)? {
            Sfnt::sfnt_open(fp)?
        } else if let Some(fp) = self.dpx_open_file(&filename, ResType::DFont)? {
            Sfnt::dfont_open(fp, index as i32)?
        } else {
            warn!("Could not open TTF/dfont file");
            return Ok(-1);
        };
        let Some(mut sfont) = sfont else {
            warn!("Could not open TTF file");
            return Ok(-1);
        };

        let offset = match sfont.type_ {
            SFNT_TYPE_TTC => {
                let offset = sfont.ttc_read_offset(index)?;
                if offset == 0 {
                    warn!("Invalid TTC index");
                    return Ok(-1);
                }
                offset
            }
            SFNT_TYPE_TRUETYPE => {
                if index > 0 {
                    warn!("Found TrueType font file while expecting TTC file");
                    return Ok(-1);
                }
                0
            }
            SFNT_TYPE_DFONT => sfont.offset,
            _ => {
                warn!("Not a TrueType/TTC font?");
                return Ok(-1);
            }
        };

        if sfont.sfnt_read_table_directory(offset)? < 0 {
            warn!("Could not read TrueType table directory");
            return Ok(-1);
        }

        // Adobe-Identity means font's internal glyph ordering here.
        let mut maptype = MapType::ViaCidToCode;
        let mut ttcmap: Option<TtCmap> = None;
        let mut ttcmap_uvs: Option<TtCmap> = None;
        let mut cmap_id: i32 = -1;
        if csi.registry.as_deref() == Some(b"Adobe") && csi.ordering.as_deref() == Some(b"Identity")
        {
            maptype = MapType::GlyphOrdering;
        } else {
            if let (Some(reg), Some(ord)) = (csi.registry.as_deref(), csi.ordering.as_deref()) {
                let mut cmap_name = reg.to_vec();
                cmap_name.push(b'-');
                cmap_name.extend_from_slice(ord);
                cmap_name.push(b'-');
                cmap_name.extend_from_slice(
                    self.font.fonts[fid].fontname.as_deref().unwrap_or_default(),
                );
                let id = self.CMap_cache_find(&cmap_name)?;
                if id >= 0 {
                    cmap_id = id;
                    maptype = MapType::ViaCidToGid;
                }
            }
            if maptype != MapType::ViaCidToGid {
                maptype = MapType::ViaCidToCode;
                // This part contains a bug. It may choose SJIS encoding
                // TrueType cmap table for Adobe-GB1.
                let mut i = 0;
                while i <= KNOWN_ENCODINGS_MAX {
                    let (platform, encoding, _) = KNOWN_ENCODINGS[i as usize];
                    ttcmap = sfont.tt_cmap_read(platform, encoding)?;
                    if ttcmap.is_some() {
                        break;
                    }
                    i += 1;
                }
                if ttcmap.is_none() {
                    warn!("No usable TrueType cmap table found for font.");
                    warn!("Cannot continue without this...");
                    return Ok(-1);
                } else if i <= WIN_UCS_INDEX_MAX {
                    unicode_cmap = 1;
                    // Unicode Variation Sequences.
                    ttcmap_uvs = sfont.tt_cmap_read(0, 5)?;
                } else {
                    unicode_cmap = 0;
                }

                if csi.ordering.as_deref() == Some(b"UCS") && i <= WIN_UCS_INDEX_MAX {
                    cmap_id = -1;
                } else {
                    cmap_id =
                        self.find_tocode_cmap(csi.registry.as_deref(), csi.ordering.as_deref(), i)?;
                    if cmap_id < 0 {
                        return Ok(-1);
                    }
                }
            }
        }

        let mut glyphs = TtGlyphs::tt_build_init()?;

        let mut last_cid: u32 = 0;
        let mut num_glyphs: u16 = 1; /* .notdef */
        let h_used_chars = self.font.fonts[fid].usedchars.clone();
        let v_used_chars: Option<Vec<u8>> = self.font.fonts[fid]
            .cid
            .usedchars_v
            .as_ref()
            .map(|v| v.borrow().clone());
        assert!(h_used_chars.is_some() || v_used_chars.is_some());
        {
            // Quick check of max CID.
            let mut c: i32 = 0;
            if let Some(h) = h_used_chars.as_ref() {
                let h = h.borrow();
                for i in (0..8192usize).rev() {
                    if h[i] != 0 {
                        last_cid = (i * 8 + 7) as u32;
                        c = i32::from(h[i] as i8);
                        break;
                    }
                }
            }
            if let Some(v) = v_used_chars.as_ref() {
                for i in (0..8192usize).rev() {
                    if v[i] != 0 && (i * 8 + 7) as u32 >= last_cid {
                        c = if (i * 8 + 7) as u32 > last_cid {
                            i32::from(v[i] as i8)
                        } else {
                            c | i32::from(v[i] as i8)
                        };
                        last_cid = (i * 8 + 7) as u32;
                        break;
                    }
                }
            }
            if last_cid > 0 {
                for i in 0..8 {
                    if (c >> i) & 1 != 0 {
                        break;
                    }
                    last_cid -= 1;
                }
            }
            assert!(last_cid < 0xFFFF);
        }
        let last_cid = last_cid as u16;

        // NO_GHOSTSCRIPT_BUG is not defined: no CIDToGIDMap.
        let cidtogidmap: Option<Vec<u8>> = None;

        // Map CIDs to GIDs. Horizontal and vertical used_chars are merged.

        // Horizontal.
        if let Some(h) = h_used_chars.as_ref() {
            for cid in 1..=last_cid {
                if !is_used_char2(&h.borrow(), u32::from(cid)) {
                    continue;
                }
                let (_code, gid) = self.cidtype2_map_cid(
                    maptype,
                    cmap_id,
                    ttcmap.as_ref(),
                    ttcmap_uvs.as_ref(),
                    unicode_cmap,
                    cid,
                )?;
                // TODO: duplicated glyph
                glyphs.tt_add_glyph(gid, cid)?;
                num_glyphs = num_glyphs.wrapping_add(1);
            }
        }

        // Vertical.
        if let Some(v) = v_used_chars.as_ref() {
            // Require `vrt2' or `vert'.
            let gsub_list = if maptype != MapType::ViaCidToCode {
                None
            } else {
                let mut gsub_list = OtlGsub::otl_gsub_new();
                if gsub_list.otl_gsub_add_feat(b"*", b"*", b"vrt2", &mut sfont)? < 0 {
                    if gsub_list.otl_gsub_add_feat(b"*", b"*", b"vert", &mut sfont)? < 0 {
                        warn!("GSUB feature vrt2/vert not found.");
                        None
                    } else {
                        gsub_list.otl_gsub_select(b"*", b"*", b"vert");
                        Some(gsub_list)
                    }
                } else {
                    gsub_list.otl_gsub_select(b"*", b"*", b"vrt2");
                    Some(gsub_list)
                }
            };

            for cid in 1..=last_cid {
                if !is_used_char2(v, u32::from(cid)) {
                    continue;
                }

                // There may be conflict of horizontal and vertical glyphs
                // when font is used with /UCS. However, we simply ignore
                // that...
                if let Some(h) = h_used_chars.as_ref() {
                    if is_used_char2(&h.borrow(), u32::from(cid)) {
                        continue;
                    }
                }

                let (_code, mut gid) = self.cidtype2_map_cid(
                    maptype,
                    cmap_id,
                    ttcmap.as_ref(),
                    ttcmap_uvs.as_ref(),
                    unicode_cmap,
                    cid,
                )?;
                if gid != 0 {
                    if let Some(gsub_list) = gsub_list.as_ref() {
                        gsub_list.otl_gsub_apply(&mut gid)?;
                    }
                }

                glyphs.tt_add_glyph(gid, cid)?;

                if let Some(h) = h_used_chars.as_ref() {
                    // Merge vertical used_chars to horizontal.
                    add_to_used_chars2(&mut h.borrow_mut(), u32::from(cid));
                }

                num_glyphs = num_glyphs.wrapping_add(1);
            }
        }

        // used_chars: the horizontal ones (with the vertical merged), else
        // the vertical ones.
        let used_chars: Vec<u8> = match h_used_chars.as_ref() {
            Some(h) => h.borrow().clone(),
            None => v_used_chars.clone().unwrap(),
        };

        drop(ttcmap);
        drop(ttcmap_uvs);

        if embed != 0 {
            if sfont.tt_build_tables(&mut glyphs)? < 0 {
                warn!("Could not created FontFile stream.");
                return Ok(-1);
            }
        } else if sfont.tt_get_metrics(&mut glyphs)? < 0 {
            warn!("Reading glyph metrics failed...");
            return Ok(-1);
        }

        // DW, W, DW2, and W2
        if self.cid.opt_flags_cidfont & CIDFONT_FORCE_FIXEDPITCH != 0 {
            self.o.put_number(resource, b"DW", 1000.0)?;
        } else {
            self.add_TTCIDHMetrics(
                resource,
                &glyphs,
                &used_chars,
                cidtogidmap.as_deref(),
                last_cid,
            )?;
            if self.font.fonts[fid].cid.need_vmetrics != 0 {
                self.add_TTCIDVMetrics(resource, &glyphs, &used_chars, last_cid)?;
            }
        }

        // CIDSet: all glyphs including component glyph and dummy glyph
        // must be listed in CIDSet; .notdef glyph should be omitted.
        if self.o.check_version(2, 0) < 0 {
            let n = glyphs.last_gid as usize / 8 + 1;
            let mut cidset_data = vec![0u8; n];
            for i in 1..=glyphs.last_gid as usize {
                cidset_data[i / 8] |= 1 << (7 - i % 8);
            }
            let cidset = self.o.new_stream(STREAM_COMPRESS);
            self.o.add_stream(cidset, &cidset_data)?;
            let r = self.o.ref_obj(cidset)?;
            self.o.put(descriptor, b"CIDSet", r)?;
            self.o.release(cidset)?;
        }

        glyphs.tt_build_finish();

        // Finish here if not embedded.
        if embed == 0 {
            return Ok(0);
        }

        // Create font file.
        for &(name, must_exist) in REQUIRED_TABLE {
            if sfont.sfnt_require_table(name, i32::from(must_exist)) < 0 {
                warn!("Some required TrueType table does not exist.");
                return Ok(-1);
            }
        }

        // FontFile2
        let fontfile = self.sfnt_create_FontFile_stream(&mut sfont)?;

        drop(sfont);

        let Some(fontfile) = fontfile else {
            warn!("Could not created FontFile stream.");
            return Ok(-1);
        };

        let r = self.o.ref_obj(fontfile)?;
        self.o.put(descriptor, b"FontFile2", r)?;
        self.o.release(fontfile)?;

        // CIDToGIDMap: ISO 32000-1 requires it for Type 2 CIDFonts with
        // embedded font programs.
        match cidtogidmap {
            None => {
                self.o.put_name(resource, b"CIDToGIDMap", b"Identity")?;
            }
            Some(map) => {
                let c2gmstream = self.o.new_stream(STREAM_COMPRESS);
                self.o
                    .add_stream(c2gmstream, &map[..(usize::from(last_cid) + 1) * 2])?;
                let r = self.o.ref_obj(c2gmstream)?;
                self.o.put(resource, b"CIDToGIDMap", r)?;
                self.o.release(c2gmstream)?;
            }
        }

        let _ = num_glyphs;
        Ok(0)
    }

    /// `CIDFont_type2_open`: 0, or -1 if `name` is not a TrueType font.
    pub fn CIDFont_type2_open(
        &mut self,
        font_id: i32,
        name: &[u8],
        index: i32,
        opt: &mut CidOpt,
    ) -> Result<i32> {
        let fid = font_id as usize;

        let sfont = if let Some(fp) = self.dpx_open_file(name, ResType::TtFont)? {
            Sfnt::sfnt_open(fp)?
        } else if let Some(fp) = self.dpx_open_file(name, ResType::DFont)? {
            Sfnt::dfont_open(fp, index)?
        } else {
            return Ok(-1);
        };
        let Some(mut sfont) = sfont else {
            return Ok(-1);
        };

        let offset = match sfont.type_ {
            SFNT_TYPE_TTC => sfont.ttc_read_offset(index as u32)?,
            SFNT_TYPE_TRUETYPE => {
                if index > 0 {
                    warn!("Invalid TTC index (not TTC font)");
                    return Ok(-1);
                }
                0
            }
            SFNT_TYPE_DFONT => sfont.offset,
            _ => return Ok(-1),
        };

        if sfont.sfnt_read_table_directory(offset)? < 0 {
            warn!("Reading TrueType table directory failed");
            return Ok(-1);
        }

        // Ignore TrueType Collection with CFF table.
        if sfont.type_ == SFNT_TYPE_TTC && sfont.sfnt_find_table_pos(b"CFF ") != 0 {
            return Ok(-1);
        }

        let mut fontname;
        {
            // MAC-ROMAN-EN-POSTSCRIPT or WIN-UNICODE-EN(US)-POSTSCRIPT
            let mut shortname = sfont.tt_get_ps_fontname(PDF_NAME_LEN_MAX as u16)?;
            let mut namelen = shortname.len() as i32;
            if namelen == 0 {
                let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
                shortname = name[..n.min(PDF_NAME_LEN_MAX)].to_vec();
                namelen = shortname.len() as i32;
            }
            // For SJIS, UTF-16, ... string.
            let error = validate_name(&mut shortname, namelen);
            if error != 0 {
                return Ok(-1);
            }
            fontname = shortname;
        }

        if opt.embed != 0 && opt.style != FONT_STYLE_NONE {
            warn!("Embedding disabled due to style option.");
            opt.embed = 0;
        }
        match opt.style {
            FONT_STYLE_BOLD => fontname.extend_from_slice(b",Bold"),
            FONT_STYLE_ITALIC => fontname.extend_from_slice(b",Italic"),
            FONT_STYLE_BOLDITALIC => fontname.extend_from_slice(b",BoldItalic"),
            _ => {}
        }
        // CIDSystemInfo is determined from CMap or from map record option.
        {
            let font = &mut self.font.fonts[fid];
            font.fontname = Some(fontname.clone());
            font.subtype = PDF_FONT_FONTTYPE_CIDTYPE2;
            if opt.csi.registry.is_some() && opt.csi.ordering.is_some() {
                font.cid.csi.registry = opt.csi.registry.clone();
                font.cid.csi.ordering = opt.csi.ordering.clone();
                font.cid.csi.supplement = opt.csi.supplement;
            }
        }

        let resource = self.o.new_dict();
        self.font.fonts[fid].resource = Some(resource);
        self.o.put_name(resource, b"Type", b"Font")?;
        self.o.put_name(resource, b"Subtype", b"CIDFontType2")?;

        let descriptor = self.tt_get_fontdesc(&mut sfont, &mut opt.embed, opt.stemv, 0, name)?;
        self.font.fonts[fid].descriptor = descriptor;
        let Some(descriptor) = descriptor else {
            warn!("Could not obtain necessary font info");
            return Ok(-1);
        };

        if opt.embed != 0 {
            let tag = self.pdf_font_make_uniqueTag();
            self.font.fonts[fid].unique_id[..6].copy_from_slice(&tag);
            let mut tmp = tag.to_vec();
            tmp.push(b'+');
            tmp.extend_from_slice(&fontname);
            self.o.put_name(descriptor, b"FontName", &tmp)?;
            self.o.put_name(resource, b"BaseFont", &tmp)?;
        } else {
            self.o.put_name(descriptor, b"FontName", &fontname)?;
            self.o.put_name(resource, b"BaseFont", &fontname)?;
        }

        drop(sfont);

        // Don't write fontdict here: /Supplement in /CIDSystemInfo may
        // change.

        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let mut n = b"Foo\0Bar\0".to_vec();
        assert_eq!(validate_name(&mut n, 7), 0);
        assert_eq!(n, b"FooBar");
        // Consecutive NULs: C's loop skips the second one.
        let mut n = b"A\0\0B\0".to_vec();
        assert_eq!(validate_name(&mut n, 4), 0);
        assert_eq!(n, b"A");
        let mut n = b"MS-Mincho-90pv-RKSJ-H\0".to_vec();
        assert_eq!(validate_name(&mut n, 21), 0);
        assert_eq!(n, b"MS-Mincho");
        let mut n = b"-90pv-RKSJ-H\0".to_vec();
        assert_eq!(validate_name(&mut n, 12), 0);
        assert_eq!(n, b"-90pv-RKSJ-H");
        let mut n = Vec::new();
        assert_eq!(validate_name(&mut n, 0), -1);
    }

    #[test]
    fn cjk_symbols() {
        assert_eq!(fix_CJK_symbols(0x2014), 0x2015);
        assert_eq!(fix_CJK_symbols(0x2015), 0x2014);
        assert_eq!(fix_CJK_symbols(0x41), 0x41);
        assert_eq!(fix_CJK_symbols(0xFFFF), 0xFFFF);
    }
}
