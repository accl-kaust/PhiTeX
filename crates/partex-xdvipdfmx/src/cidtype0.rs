//! cidtype0.c, cidtype0.h: CIDFontType0 (CFF CIDFonts, OpenType/CFF, and
//! Type1 / Type1C converted to CFF CIDFonts).
//!
//! `pdf_font *` is a `font_id: i32` into `self.font.fonts`; `cff_font *`
//! is `&mut CffFont` (owned by the caller, closed with `cff_close`);
//! `sfnt *` is `&mut Sfnt`. `CIDToGIDMap`, `used_chars` are byte slices.

#![allow(non_snake_case)]

use crate::cff::CffFont;
use crate::cmap::Cid;
use crate::pdffont::CidOpt;
use crate::prelude::*;
use crate::sfnt::Sfnt;
use crate::tt_table::{TtHeadTable, TtLongMetrics, TtMaxpTable};

/// `TYPE1_NAME_LEN_MAX`.
pub const TYPE1_NAME_LEN_MAX: usize = 127;
/// `WBUF_SIZE` (`create_ToUnicode_stream`).
pub const WBUF_SIZE: usize = 1024;

/// `FONT_FLAG_FIXEDPITCH`: fixed-width font.
pub const FONT_FLAG_FIXEDPITCH: i32 = 1 << 0;
/// `FONT_FLAG_SERIF`.
pub const FONT_FLAG_SERIF: i32 = 1 << 1;
/// `FONT_FLAG_SYMBOLIC`.
pub const FONT_FLAG_SYMBOLIC: i32 = 1 << 2;
/// `FONT_FLAG_SCRIPT`.
pub const FONT_FLAG_SCRIPT: i32 = 1 << 3;
/// `FONT_FLAG_STANDARD`: Adobe standard character set.
pub const FONT_FLAG_STANDARD: i32 = 1 << 5;
/// `FONT_FLAG_ITALIC`.
pub const FONT_FLAG_ITALIC: i32 = 1 << 6;
/// `FONT_FLAG_ALLCAP`.
pub const FONT_FLAG_ALLCAP: i32 = 1 << 16;
/// `FONT_FLAG_SMALLCAP`.
pub const FONT_FLAG_SMALLCAP: i32 = 1 << 17;
/// `FONT_FLAG_FORCEBOLD`.
pub const FONT_FLAG_FORCEBOLD: i32 = 1 << 18;

/// `get_font_attr`'s `L_c`: glyphs for the cap height.
pub const L_C: &[&[u8]] = &[b"H", b"P", b"Pi", b"Rho"];
/// `get_font_attr`'s `L_d`: glyphs for the descent.
pub const L_D: &[&[u8]] = &[b"p", b"q", b"mu", b"eta"];
/// `get_font_attr`'s `L_a`: glyphs for the ascent.
pub const L_A: &[&[u8]] = &[b"b", b"h", b"lambda"];

use crate::agl::{agl_chop_suffix, agl_name_convert_unicode, agl_name_is_unicode};
use crate::cff::{
    CS_STR_LEN_MAX, CffCharsets, CffFdselect, CffIndex, CffRange3, FONTTYPE_CIDFONT, SSid,
};
use crate::cff_dict::{
    CFF_DEFAULTWIDTHX_DEFAULT, CFF_NOMINALWIDTHX_DEFAULT, cff_dict_update, cff_new_dict,
};
use crate::cid::{CIDFONT_FORCE_FIXEDPITCH, CSI_IDENTITY, CSI_UNICODE};
use crate::cmap::{CID_MAX, CMAP_TYPE_CODE_TO_CID, CMAP_TYPE_TO_UNICODE, CMap};
use crate::dpxfile::ResType;
use crate::fmt::round_acc;
use crate::obj::STREAM_COMPRESS;
use crate::pdffont::{
    CIDFONT_FLAG_TYPE1, CIDFONT_FLAG_TYPE1C, CidSysInfo, FONT_STYLE_BOLD, FONT_STYLE_BOLDITALIC,
    FONT_STYLE_ITALIC, FONT_STYLE_NONE, PDF_FONT_FLAG_BASEFONT, PDF_FONT_FONTTYPE_CIDTYPE0,
    PdfFont, add_to_used_chars2, is_used_char2,
};
use crate::sfnt::{SFNT_TYPE_POSTSCRIPT, SFNT_TYPE_TTC};
use crate::t1_char::T1Ginfo;

/// mfileio.h's `WORK_BUFFER_SIZE` (`write_fontfile` packs into
/// `work_buffer`).
const WORK_BUFFER_SIZE: usize = 1024;

/// `PDFUNIT(v)`: `ROUND((1000.0*(v))/(head->unitsPerEm),1)`.
fn pdfunit(v: f64, head: &TtHeadTable) -> f64 {
    round_acc((1000.0 * v) / f64::from(head.units_per_em), 1.0)
}

/// The `pdf_font` of `font_id`.
fn fnt(dpx: &mut Dpx, font_id: i32) -> &mut PdfFont {
    &mut dpx.font.fonts[font_id as usize]
}

/// `DPXFOPEN(name, DPX_RES_TYPE_OTFONT)`, else `DPX_RES_TYPE_TTFONT`.
fn open_ot_or_tt(dpx: &mut Dpx, name: &[u8]) -> Result<Option<MemFile>> {
    match dpx.dpx_open_file(name, ResType::OtFont)? {
        Some(fp) => Ok(Some(fp)),
        None => dpx.dpx_open_file(name, ResType::TtFont),
    }
}

/// The CFF table of an OpenType font, as the `dofont`/`open` functions
/// find it: `offset` is C's (the TTC entry, then the `CFF ` table), 0 if
/// it is not a CFF/OpenType font.
fn find_cff_table(sfont: &mut Sfnt, index: u32) -> Result<u32> {
    let mut offset: u32 = 0;
    if sfont.type_ == SFNT_TYPE_TTC {
        offset = sfont.ttc_read_offset(index)?;
    }
    if (sfont.type_ != SFNT_TYPE_TTC && sfont.type_ != SFNT_TYPE_POSTSCRIPT)
        || sfont.sfnt_read_table_directory(offset)? < 0
    {
        return Ok(0);
    }
    Ok(sfont.sfnt_find_table_pos(b"CFF "))
}

/// `strstr(hay, needle) != NULL`.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

/// `"%s+%s"` of a unique tag and a font name.
fn tagged_name(tag: &[u8; 7], fontname: &[u8]) -> Vec<u8> {
    let t = tag.iter().position(|&c| c == 0).unwrap_or(7);
    let mut v = tag[..t].to_vec();
    v.push(b'+');
    v.extend_from_slice(fontname);
    v
}

impl Dpx {
    /// `pdf_font_make_uniqueTag(font->uniqueID)`.
    fn cidtype0_make_unique_tag(&mut self, font_id: i32) {
        let tag = self.pdf_font_make_uniqueTag();
        let f = fnt(self, font_id);
        f.unique_id[..6].copy_from_slice(&tag);
        f.unique_id[6] = 0;
    }

    /// The `CIDSystemInfo` dict of `csi` into `resource`.
    fn cidtype0_add_csi(&mut self, resource: Obj, csi: &CidSysInfo) {
        let csi_dict = self.o.new_dict();
        let r = csi.registry.clone().unwrap_or_default();
        let o = csi.ordering.clone().unwrap_or_default();
        self.o.put_string(csi_dict, b"Registry", &r);
        self.o.put_string(csi_dict, b"Ordering", &o);
        self.o
            .put_number(csi_dict, b"Supplement", f64::from(csi.supplement));
        self.o.put(resource, b"CIDSystemInfo", csi_dict);
    }

    /// `add_CIDHMetrics` (static): `/W` from `hmtx`.
    fn add_CIDHMetrics(
        &mut self,
        sfont: &mut Sfnt,
        fontdict: Obj,
        cid_to_gid_map: Option<&[u8]>,
        last_cid: u16,
        maxp: &TtMaxpTable,
        head: &TtHeadTable,
        hmtx: &[TtLongMetrics],
    ) {
        let _ = sfont; // unsed
        let mut an_array: Option<Obj> = None;
        let mut start: i32 = 0;
        let mut prev: i32 = 0;
        let mut empty = true;

        let default_advance_width = pdfunit(f64::from(hmtx[0].advance), head);
        // We alway use format: c [w_1 w_2 ... w_n]
        let w_array = self.o.new_array();
        for cid in 0..=i32::from(last_cid) {
            let gid: u16 = match cid_to_gid_map {
                Some(m) => {
                    (u16::from(m[2 * cid as usize]) << 8) | u16::from(m[2 * cid as usize + 1])
                }
                None => cid as u16,
            };
            if gid >= maxp.num_glyphs || (cid != 0 && gid == 0) {
                continue;
            }
            let advance_width = pdfunit(f64::from(hmtx[gid as usize].advance), head);
            if advance_width == default_advance_width {
                if let Some(a) = an_array.take() {
                    let n = self.o.new_number(f64::from(start));
                    self.o.add_array(w_array, n);
                    self.o.add_array(w_array, a);
                    empty = false;
                }
            } else {
                if cid != prev + 1 && an_array.is_some() {
                    let a = an_array.take().unwrap();
                    let n = self.o.new_number(f64::from(start));
                    self.o.add_array(w_array, n);
                    self.o.add_array(w_array, a);
                    empty = false;
                }
                if an_array.is_none() {
                    an_array = Some(self.o.new_array());
                    start = cid;
                }
                let n = self.o.new_number(advance_width);
                self.o.add_array(an_array.unwrap(), n);
                prev = cid;
            }
        }

        if let Some(a) = an_array {
            let n = self.o.new_number(f64::from(start));
            self.o.add_array(w_array, n);
            self.o.add_array(w_array, a);
            empty = false;
        }

        // We always write DW for older MacOS X's preview app. PDF Reference
        // 2nd. ed, wrongly described default value of DW as 0, and MacOS X's
        // (up to 10.2.8) preview app. implements this wrong description.
        self.o.put_number(fontdict, b"DW", default_advance_width);
        if !empty {
            let r = self.o.ref_obj(w_array);
            self.o.put(fontdict, b"W", r);
        }
        self.o.release(w_array);
    }

    /// `add_CIDVMetrics` (static): `/DW2`, `/W2` (VORG, vmtx).
    fn add_CIDVMetrics(
        &mut self,
        sfont: &mut Sfnt,
        fontdict: Obj,
        cid_to_gid_map: Option<&[u8]>,
        last_cid: u16,
        maxp: &TtMaxpTable,
        head: &TtHeadTable,
        hmtx: &[TtLongMetrics],
    ) -> Result<()> {
        let mut vhea = None;
        let mut vmtx: Option<Vec<TtLongMetrics>> = None;
        let mut empty = true;

        // No accurate vertical metrics can be obtained by simple way if the
        // font does not have VORG table. Only CJK fonts may have VORG.
        if sfont.sfnt_find_table_pos(b"VORG") == 0 {
            return Ok(());
        }

        let vorg = sfont
            .tt_read_VORG_table()?
            .expect("tt_read_VORG_table(): no VORG table");
        let mut default_vert_origin_y = pdfunit(f64::from(vorg.default_vert_origin_y), head);
        if sfont.sfnt_find_table_pos(b"vhea") > 0 {
            vhea = Some(sfont.tt_read_vhea_table()?);
        }
        if let Some(vh) = vhea.as_ref() {
            if sfont.sfnt_find_table_pos(b"vmtx") > 0 {
                sfont.sfnt_locate_table(b"vmtx")?;
                vmtx = Some(sfont.tt_read_longMetrics(
                    maxp.num_glyphs,
                    vh.num_of_long_ver_metrics,
                    vh.num_of_ex_side_bearings,
                )?);
            }
        }

        let default_advance_height;
        if sfont.sfnt_find_table_pos(b"OS/2") == 0 {
            // OpenType font must have OS/2 table.
            let os2 = sfont.tt_read_os2__table()?;
            default_vert_origin_y = pdfunit(f64::from(os2.s_typo_ascender), head);
            default_advance_height = pdfunit(
                f64::from(i32::from(os2.s_typo_ascender) - i32::from(os2.s_typo_descender)),
                head,
            );
        } else {
            // Some TrueType fonts used in Macintosh does not have OS/2 table.
            default_advance_height = 1000.0;
        }

        let w2_array = self.o.new_array();
        for cid in 0..=i32::from(last_cid) {
            let gid: u16 = match cid_to_gid_map {
                Some(m) => {
                    (u16::from(m[2 * cid as usize]) << 8) | u16::from(m[2 * cid as usize + 1])
                }
                None => cid as u16,
            };
            if gid >= maxp.num_glyphs || (cid != 0 && gid == 0) {
                continue;
            }
            let advance_height = match vmtx.as_ref() {
                Some(v) => pdfunit(f64::from(v[gid as usize].advance), head),
                None => default_advance_height,
            };
            let vert_origin_x = pdfunit(f64::from(hmtx[gid as usize].advance) * 0.5, head);
            let mut vert_origin_y = default_vert_origin_y;
            let mut i = 0usize;
            while i < vorg.num_vert_origin_y_metrics as usize
                && gid >= vorg.vert_origin_y_metrics[i].glyph_index
            {
                if gid == vorg.vert_origin_y_metrics[i].glyph_index {
                    vert_origin_y =
                        pdfunit(f64::from(vorg.vert_origin_y_metrics[i].vert_origin_y), head);
                }
                i += 1;
            }
            // c_first c_last w1_y v_x v_y
            // This form may hit Acrobat's implementation limit of array
            // element size, 8192. AFPL GhostScript 8.11 stops with
            // rangecheck error with this. Maybe GS's bug?
            if vert_origin_y != default_vert_origin_y || advance_height != default_advance_height {
                for v in [
                    f64::from(cid),
                    f64::from(cid),
                    -advance_height,
                    vert_origin_x,
                    vert_origin_y,
                ] {
                    let n = self.o.new_number(v);
                    self.o.add_array(w2_array, n);
                }
                empty = false;
            }
        }

        if default_vert_origin_y != 880.0 || default_advance_height != 1000.0 {
            let an_array = self.o.new_array();
            let n = self.o.new_number(default_vert_origin_y);
            self.o.add_array(an_array, n);
            let n = self.o.new_number(-default_advance_height);
            self.o.add_array(an_array, n);
            self.o.put(fontdict, b"DW2", an_array);
        }
        if !empty {
            let r = self.o.ref_obj(w2_array);
            self.o.put(fontdict, b"W2", r);
        }
        self.o.release(w2_array);
        Ok(())
    }

    /// `add_CIDMetrics` (static).
    fn add_CIDMetrics(
        &mut self,
        sfont: &mut Sfnt,
        fontdict: Obj,
        cid_to_gid_map: Option<&[u8]>,
        last_cid: u16,
        need_vmetrics: i32,
    ) -> Result<()> {
        // Read head, hhea, maxp:
        //   unitsPerEm --> head, numHMetrics --> hhea, numGlyphs --> maxp
        let head = sfont.tt_read_head_table()?;
        let maxp = sfont.tt_read_maxp_table()?;
        let hhea = sfont.tt_read_hhea_table()?;

        sfont.sfnt_locate_table(b"hmtx")?;
        let hmtx = sfont.tt_read_longMetrics(
            maxp.num_glyphs,
            hhea.num_of_long_hor_metrics,
            hhea.num_of_ex_side_bearings,
        )?;

        self.add_CIDHMetrics(
            sfont,
            fontdict,
            cid_to_gid_map,
            last_cid,
            &maxp,
            &head,
            &hmtx,
        );
        if need_vmetrics != 0 {
            self.add_CIDVMetrics(
                sfont,
                fontdict,
                cid_to_gid_map,
                last_cid,
                &maxp,
                &head,
                &hmtx,
            )?;
        }
        Ok(())
    }

    /// `write_fontfile` (static): the embedded `/FontFile3` (CIDFontType0C).
    fn write_fontfile(&mut self, font_id: i32, cffont: &mut CffFont) -> Result<i32> {
        let mut work_buffer = vec![0u8; WORK_BUFFER_SIZE];
        let num_fds = cffont.num_fds as usize;

        // DICT sizes (offset set to long int)
        let mut topdict = CffIndex::cff_new_index(1);
        let mut fdarray = CffIndex::cff_new_index(u16::from(cffont.num_fds));
        let mut private = CffIndex::cff_new_index(u16::from(cffont.num_fds));

        {
            let td = cffont.topdict.as_mut().expect("Top DICT");
            td.cff_dict_remove(b"UniqueID");
            td.cff_dict_remove(b"XUID");
            td.cff_dict_remove(b"Private"); // some bad font may have
            td.cff_dict_remove(b"Encoding"); // some bad font may have

            topdict.offset[1] = (td.cff_dict_pack(&mut work_buffer)? + 1) as u32;
        }
        for i in 0..num_fds {
            let mut size = 0;
            if let Some(Some(p)) = cffont.private.get(i) {
                size = p.cff_dict_pack(&mut work_buffer)?;
                if size < 1 {
                    // Private had contained only Subr
                    cffont.fdarray[i]
                        .as_mut()
                        .expect("Font DICT")
                        .cff_dict_remove(b"Private");
                }
            }
            private.offset[i + 1] = private.offset[i].wrapping_add(size as u32);
            fdarray.offset[i + 1] = fdarray.offset[i].wrapping_add(
                cffont.fdarray[i]
                    .as_ref()
                    .expect("Font DICT")
                    .cff_dict_pack(&mut work_buffer)? as u32,
            );
        }

        let mut destlen: i32 = 4; // header size
        let fontname = fnt(self, font_id).fontname.clone().unwrap_or_default();
        destlen += cffont.cff_set_name(&fontname)?;
        destlen += topdict.cff_index_size();
        destlen += cffont
            .string
            .as_ref()
            .expect("String INDEX")
            .cff_index_size();
        destlen += cffont
            .gsubr
            .as_ref()
            .expect("Global Subr INDEX")
            .cff_index_size();
        destlen += i32::from(cffont.charsets.as_ref().unwrap().num_entries) * 2 + 1; // charset format 0
        destlen += i32::from(cffont.fdselect.as_ref().unwrap().num_entries) * 3 + 5; // fdselect format 3
        destlen += cffont.cstrings.as_ref().unwrap().cff_index_size();
        destlen += fdarray.cff_index_size();
        destlen += private.offset[private.count as usize] as i32 - 1; // Private is not INDEX

        let mut dest = vec![0u8; destlen as usize];

        let mut offset: usize = 0;
        // Header
        offset += cffont.cff_put_header(&mut dest[offset..])? as usize;
        // Name
        offset += cffont
            .name
            .as_ref()
            .unwrap()
            .cff_pack_index(&mut dest[offset..])? as usize;
        // Top DICT
        let topdict_offset = offset;
        offset += topdict.cff_index_size() as usize;
        // Strings
        offset += cffont
            .string
            .as_ref()
            .unwrap()
            .cff_pack_index(&mut dest[offset..])? as usize;
        // Global Subrs
        offset += cffont
            .gsubr
            .as_ref()
            .unwrap()
            .cff_pack_index(&mut dest[offset..])? as usize;

        // charset
        cffont
            .topdict
            .as_mut()
            .unwrap()
            .cff_dict_set(b"charset", 0, offset as f64)?;
        offset += cffont.cff_pack_charsets(&mut dest[offset..])? as usize;

        // FDSelect
        cffont
            .topdict
            .as_mut()
            .unwrap()
            .cff_dict_set(b"FDSelect", 0, offset as f64)?;
        offset += cffont.cff_pack_fdselect(&mut dest[offset..])? as usize;

        // CharStrings
        cffont
            .topdict
            .as_mut()
            .unwrap()
            .cff_dict_set(b"CharStrings", 0, offset as f64)?;
        {
            let cs = cffont.cstrings.as_ref().unwrap();
            let n = cs.cff_index_size() as usize;
            offset += cs.cff_pack_index(&mut dest[offset..offset + n])? as usize;
        }
        cffont.cstrings = None; // Charstrings cosumes huge memory

        // FDArray and Private
        cffont
            .topdict
            .as_mut()
            .unwrap()
            .cff_dict_set(b"FDArray", 0, offset as f64)?;
        let fdarray_offset = offset;
        offset += fdarray.cff_index_size() as usize;

        fdarray.data = vec![0u8; fdarray.offset[fdarray.count as usize] as usize - 1];
        for i in 0..num_fds {
            let size = private.offset[i + 1].wrapping_sub(private.offset[i]) as usize;
            if let Some(Some(p)) = cffont.private.get(i) {
                if size > 0 {
                    p.cff_dict_pack(&mut dest[offset..offset + size])?;
                    let fd = cffont.fdarray[i].as_mut().unwrap();
                    fd.cff_dict_set(b"Private", 0, size as f64)?;
                    fd.cff_dict_set(b"Private", 1, offset as f64)?;
                }
            }
            let a = fdarray.offset[i] as usize - 1;
            cffont.fdarray[i]
                .as_ref()
                .unwrap()
                .cff_dict_pack(&mut fdarray.data[a..])?;
            offset += size;
        }

        {
            let n = fdarray.cff_index_size() as usize;
            fdarray.cff_pack_index(&mut dest[fdarray_offset..fdarray_offset + n])?;
        }
        drop(fdarray);
        drop(private);

        // Finally Top DICT
        topdict.data = vec![0u8; topdict.offset[topdict.count as usize] as usize - 1];
        cffont
            .topdict
            .as_ref()
            .unwrap()
            .cff_dict_pack(&mut topdict.data)?;
        {
            let n = topdict.cff_index_size() as usize;
            topdict.cff_pack_index(&mut dest[topdict_offset..topdict_offset + n])?;
        }
        drop(topdict);

        // FontFile
        {
            let descriptor = fnt(self, font_id).descriptor.expect("font descriptor");
            let fontfile = self.o.new_stream(STREAM_COMPRESS);
            let stream_dict = self.o.stream_dict(fontfile);
            let r = self.o.ref_obj(fontfile);
            self.o.put(descriptor, b"FontFile3", r);
            self.o.put_name(stream_dict, b"Subtype", b"CIDFontType0C");
            self.o.add_stream(fontfile, &dest[..offset]);
            self.o.release(fontfile);
        }

        Ok(destlen)
    }

    /// `CIDFont_type0_add_CIDSet` (static): PDF/A `/CIDSet`. (Length of
    /// CIDSet stream is not clear. Must be 8192 bytes long?)
    fn CIDFont_type0_add_CIDSet(&mut self, font_id: i32, used_chars: &[u8], last_cid: u16) {
        let descriptor = fnt(self, font_id).descriptor.expect("font descriptor");
        let cidset = self.o.new_stream(STREAM_COMPRESS);
        self.o
            .add_stream(cidset, &used_chars[..(last_cid / 8) as usize + 1]);
        let r = self.o.ref_obj(cidset);
        self.o.put(descriptor, b"CIDSet", r);
        self.o.release(cidset);
    }

    /// `CIDFont_type0_dofont`: 0 or an error.
    pub fn CIDFont_type0_dofont(&mut self, font_id: i32) -> Result<i32> {
        let (reference, resource, descriptor, flags, embed, filename, index, need_vmetrics) = {
            let f = fnt(self, font_id);
            (
                f.reference,
                f.resource,
                f.descriptor,
                f.flags,
                f.cid.options.embed,
                f.filename.clone().unwrap_or_default(),
                f.index,
                f.cid.need_vmetrics,
            )
        };

        if reference.is_none() {
            return Ok(0);
        }
        let resource = resource.expect("font resource");
        let descriptor = descriptor.expect("font descriptor");

        let r = self.o.ref_obj(descriptor);
        self.o.put(resource, b"FontDescriptor", r);

        if (flags & PDF_FONT_FLAG_BASEFONT) != 0 {
            return Ok(0);
        } else if embed == 0 && (self.cid.opt_flags_cidfont & CIDFONT_FORCE_FIXEDPITCH) != 0 {
            // No metrics needed.
            self.o.put_number(resource, b"DW", 1000.0);
            return Ok(0);
        }

        let used_chars_rc = fnt(self, font_id).usedchars.clone().expect("usedchars");

        let Some(fp) = open_ot_or_tt(self, &filename)? else {
            warn!("Could not open file: {:?}", filename);
            return Ok(-1);
        };

        let Some(mut sfont) = Sfnt::sfnt_open(fp)? else {
            warn!("Failed to read font file: {:?}", filename);
            return Ok(-1);
        };

        let offset = find_cff_table(&mut sfont, index)?;
        if offset == 0 {
            warn!("Not a CFF/OpenType font: {:?}", filename);
            return Ok(-1);
        }

        let Some(mut cffont) = CffFont::cff_open(sfont.stream.clone(), offset as i32, 0)? else {
            warn!("Failed to read CFF font data: {:?}", filename);
            return Ok(-1);
        };

        if (cffont.flag & FONTTYPE_CIDFONT) == 0 {
            warn!("Unexpected type (CIDFont expected): {:?}", filename);
            return Ok(-1);
        }

        cffont.cff_read_charsets()?;

        let mut num_glyphs: u16 = 0;
        let mut last_cid: u16 = 0;
        let mut cid_to_gid_map;
        {
            let td = cffont.topdict.as_ref().unwrap();
            let cid_count: i32 = if td.cff_dict_known(b"CIDCount") != 0 {
                td.cff_dict_get(b"CIDCount", 0)? as i32
            } else {
                CID_MAX + 1
            };

            cid_to_gid_map = vec![0u8; 2 * cid_count as usize];
            let mut used_chars = used_chars_rc.borrow_mut();
            add_to_used_chars2(&mut used_chars, 0); // .notdef
            for cid in 0..=CID_MAX as u32 {
                if is_used_char2(&used_chars, cid) {
                    let gid = cffont.cff_charsets_lookup(cid as u16)?;
                    if cid != 0 && gid == 0 {
                        warn!("Glyph for CID {} missing in font \"{:?}\".", cid, filename);
                        used_chars[(cid / 8) as usize] &= !(1 << (7 - (cid % 8)));
                        continue;
                    }
                    cid_to_gid_map[2 * cid as usize] = ((gid >> 8) & 0xff) as u8;
                    cid_to_gid_map[2 * cid as usize + 1] = (gid & 0xff) as u8;
                    last_cid = cid as u16;
                    num_glyphs = num_glyphs.wrapping_add(1);
                }
            }
        }

        // DW, W, DW2 and W2: Those values are obtained from OpenType table
        // (not TFM).
        if (self.cid.opt_flags_cidfont & CIDFONT_FORCE_FIXEDPITCH) != 0 {
            self.o.put_number(resource, b"DW", 1000.0);
        } else {
            self.add_CIDMetrics(
                &mut sfont,
                resource,
                Some(&cid_to_gid_map),
                last_cid,
                i32::from(need_vmetrics != 0),
            )?;
        }

        if embed == 0 {
            return Ok(0);
        }

        // Embed font subset.
        cffont.cff_read_fdselect()?;
        cffont.cff_read_fdarray()?;
        cffont.cff_read_private()?;

        cffont.cff_read_subrs()?;

        let offset = cffont
            .topdict
            .as_ref()
            .unwrap()
            .cff_dict_get(b"CharStrings", 0)? as i32;
        cffont.cff_seek_set(offset as usize);
        let idx = cffont.cff_get_index_header()?;
        // offset is now absolute offset ... bad
        let offset = cffont.cff_tell();

        let cs_count = idx.count;
        if cs_count < 2 {
            warn!("No valid charstring data found: {:?}", filename);
            return Ok(-1);
        }

        // New Charsets data
        let mut charset = CffCharsets {
            format: 0,
            num_entries: 0,
            glyphs: vec![0; num_glyphs as usize],
            ..CffCharsets::default()
        };

        // New FDSelect data
        let mut fdselect = CffFdselect {
            format: 3,
            num_entries: 0,
            ranges: vec![CffRange3::default(); num_glyphs as usize],
            ..CffFdselect::default()
        };

        // New CharStrings INDEX
        let mut charstrings = CffIndex::cff_new_index(num_glyphs.wrapping_add(1));
        let mut max_len = 2 * CS_STR_LEN_MAX;
        charstrings.data = vec![0u8; max_len as usize];
        let mut charstring_len: i32 = 0;

        // TODO: Re-assign FD number.
        let mut prev_fd: i32 = -1;
        let mut gid: u16 = 0;
        let mut data = vec![0u8; CS_STR_LEN_MAX as usize];
        let used_chars = used_chars_rc.borrow().clone();
        for cid in 0..=u32::from(last_cid) {
            if !is_used_char2(&used_chars, cid) {
                continue;
            }

            let gid_org = (usize::from(cid_to_gid_map[2 * cid as usize]) << 8)
                | usize::from(cid_to_gid_map[2 * cid as usize + 1]);
            let size = idx.offset[gid_org + 1].wrapping_sub(idx.offset[gid_org]) as i32;
            if size > CS_STR_LEN_MAX {
                warn!("Charstring too long: {:?} (gid={})", filename, gid_org);
                return Ok(-1);
            }
            if charstring_len + CS_STR_LEN_MAX >= max_len {
                max_len = charstring_len + 2 * CS_STR_LEN_MAX;
                charstrings.data.resize(max_len as usize, 0);
            }
            charstrings.offset[gid as usize] = (charstring_len + 1) as u32;
            cffont.cff_seek(offset + idx.offset[gid_org] as usize - 1);
            let rd = cffont.cff_read_data(size as usize);
            data[..rd.len()].copy_from_slice(&rd);
            let fd = cffont.cff_fdselect_lookup(gid_org as u16)?;
            charstring_len += self.cs_copy_charstring(
                &mut charstrings.data[charstring_len as usize..max_len as usize],
                &data[..size as usize],
                cffont.gsubr.as_ref(),
                cffont.subrs[fd as usize].as_ref(),
                0.0,
                0.0,
                None,
            )?;
            if cid > 0 && gid_org > 0 {
                charset.glyphs[charset.num_entries as usize] = cid as u16;
                charset.num_entries += 1;
            }
            if i32::from(fd) != prev_fd {
                fdselect.ranges[fdselect.num_entries as usize].first = gid;
                fdselect.ranges[fdselect.num_entries as usize].fd = fd;
                fdselect.num_entries += 1;
                prev_fd = i32::from(fd);
            }
            gid = gid.wrapping_add(1);
        }
        if gid != num_glyphs {
            warn!("Unexpeced error: {:?}", filename);
            return Ok(-1);
        }
        drop(data);
        drop(idx);
        drop(cid_to_gid_map);

        charstrings.offset[num_glyphs as usize] = (charstring_len + 1) as u32;
        charstrings.count = num_glyphs;
        cffont.num_glyphs = num_glyphs;
        cffont.cstrings = Some(charstrings);

        // discard old one, set new data
        cffont.charsets = Some(charset);
        cffont.fdselect = Some(fdselect);

        // no Global subr
        cffont.gsubr = Some(CffIndex::cff_new_index(0));

        for fd in 0..cffont.num_fds as usize {
            if let Some(s) = cffont.subrs.get_mut(fd) {
                *s = None;
            }
            if let Some(Some(p)) = cffont.private.get_mut(fd) {
                p.cff_dict_remove(b"Subrs"); // no Subrs
            }
        }

        let _destlen = self.write_fontfile(font_id, &mut cffont)?;

        cffont.cff_close();
        sfont.sfnt_close();

        if self.o.check_version(2, 0) < 0 {
            let used_chars = used_chars_rc.borrow().clone();
            self.CIDFont_type0_add_CIDSet(font_id, &used_chars, last_cid);
        }

        Ok(0)
    }

    /// `CIDFont_type0_open_from_t1`: 0, or -1 if `name` is not a Type 1 font.
    pub fn CIDFont_type0_open_from_t1(
        &mut self,
        font_id: i32,
        name: &[u8],
        index: i32,
        opt: &mut CidOpt,
    ) -> Result<i32> {
        let _ = index;
        let Some(mut fp) = self.dpx_open_file(name, ResType::T1Font)? else {
            return Ok(-1);
        };

        let Some(cffont) = self.t1_load_font(None, 1, &mut fp)? else {
            return Ok(-1);
        };

        // Mangled name requires more 7 bytes.
        let fontname = cffont.cff_get_name();

        let csi = CidSysInfo {
            registry: Some(b"Adobe".to_vec()),
            ordering: Some(b"Identity".to_vec()),
            supplement: 0,
        };
        if opt.style != FONT_STYLE_NONE {
            warn!(",Bold, ,Italic, ... not supported for this type of font...");
            opt.style = FONT_STYLE_NONE;
        }

        {
            let f = fnt(self, font_id);
            f.fontname = Some(fontname.clone());
            f.subtype = PDF_FONT_FONTTYPE_CIDTYPE0;
            f.cid.csi = csi.clone();
            f.flags |= CIDFONT_FLAG_TYPE1;
        }

        let resource = self.o.new_dict();
        fnt(self, font_id).resource = Some(resource);
        self.o.put_name(resource, b"Type", b"Font");
        self.o.put_name(resource, b"Subtype", b"CIDFontType0");

        self.cidtype0_make_unique_tag(font_id);

        let descriptor = self.o.new_dict();
        fnt(self, font_id).descriptor = Some(descriptor);
        {
            let tag = fnt(self, font_id).unique_id;
            let tmp = tagged_name(&tag, &fontname);
            self.o.put_name(descriptor, b"FontName", &tmp);
            self.o.put_name(resource, b"BaseFont", &tmp);
        }
        self.cidtype0_add_csi(resource, &csi);

        Ok(0)
    }

    /// `CIDFont_type0_open`: 0, or -1 if `name` is not a CFF CIDFont
    /// (OpenType).
    pub fn CIDFont_type0_open(
        &mut self,
        font_id: i32,
        name: &[u8],
        index: i32,
        opt: &mut CidOpt,
    ) -> Result<i32> {
        let Some(fp) = open_ot_or_tt(self, name)? else {
            return Ok(-1);
        };

        let Some(mut sfont) = Sfnt::sfnt_open(fp)? else {
            return Ok(-1);
        };

        let offset = find_cff_table(&mut sfont, index as u32)?;
        if offset == 0 {
            return Ok(-1);
        }

        let Some(cffont) = CffFont::cff_open(sfont.stream.clone(), offset as i32, 0)? else {
            warn!("Cannot read CFF font data");
            return Ok(-1);
        };

        if (cffont.flag & FONTTYPE_CIDFONT) == 0 {
            return Ok(-1);
        }

        // Mangled name requires more 7 bytes. Style requires more 11 bytes.
        let mut fontname = cffont.cff_get_name();
        let td = cffont.topdict.as_ref().unwrap();
        let csi = CidSysInfo {
            registry: cffont.cff_get_string_opt(td.cff_dict_get(b"ROS", 0)? as SSid),
            ordering: cffont.cff_get_string_opt(td.cff_dict_get(b"ROS", 1)? as SSid),
            supplement: td.cff_dict_get(b"ROS", 2)? as i32,
        };

        cffont.cff_close();

        if opt.embed != 0 && opt.style != FONT_STYLE_NONE {
            warn!("Embedding disabled due to style option for {:?}.", name);
            opt.embed = 0;

            match opt.style {
                FONT_STYLE_BOLD => fontname.extend_from_slice(b",Bold"),
                FONT_STYLE_ITALIC => fontname.extend_from_slice(b",Italic"),
                FONT_STYLE_BOLDITALIC => fontname.extend_from_slice(b",BoldItalic"),
                _ => {}
            }
        }

        // getting font info. from TrueType tables
        let Some(descriptor) =
            self.tt_get_fontdesc(&mut sfont, &mut opt.embed, opt.stemv, 0, name)?
        else {
            warn!("Could not obtain necessary font info: {:?}", name);
            return Ok(-1);
        };
        fnt(self, font_id).descriptor = Some(descriptor);

        {
            let f = fnt(self, font_id);
            f.fontname = Some(fontname.clone());
            f.subtype = PDF_FONT_FONTTYPE_CIDTYPE0;
            f.cid.csi = csi.clone();
        }

        let resource = self.o.new_dict();
        fnt(self, font_id).resource = Some(resource);
        self.o.put_name(resource, b"Type", b"Font");
        self.o.put_name(resource, b"Subtype", b"CIDFontType0");

        if opt.embed != 0 {
            self.cidtype0_make_unique_tag(font_id);
            let tag = fnt(self, font_id).unique_id;
            let tmp = tagged_name(&tag, &fontname);
            self.o.put_name(descriptor, b"FontName", &tmp);
            self.o.put_name(resource, b"BaseFont", &tmp);
        } else {
            self.o.put_name(descriptor, b"FontName", &fontname);
            self.o.put_name(resource, b"BaseFont", &fontname);
        }
        self.cidtype0_add_csi(resource, &csi);
        self.o.put_number(resource, b"DW", 1000.0); // not sure

        sfont.sfnt_close();

        Ok(0)
    }

    /// `CIDFont_type0_open_from_t1c`: 0, or -1 if `name` is not a bare
    /// CFF (Type1C in OpenType).
    pub fn CIDFont_type0_open_from_t1c(
        &mut self,
        font_id: i32,
        name: &[u8],
        index: i32,
        opt: &mut CidOpt,
    ) -> Result<i32> {
        let Some(fp) = open_ot_or_tt(self, name)? else {
            return Ok(-1);
        };

        let Some(mut sfont) = Sfnt::sfnt_open(fp)? else {
            // C warns and goes on with a NULL sfnt (and crashes).
            fatal!("Not a CFF/OpenType font: {:?}", name);
        };

        let offset = find_cff_table(&mut sfont, index as u32)?;
        if offset == 0 {
            return Ok(-1);
        }

        let Some(cffont) = CffFont::cff_open(sfont.stream.clone(), offset as i32, 0)? else {
            warn!("Cannot read CFF font data: {:?}", name);
            return Ok(-1);
        };

        if (cffont.flag & FONTTYPE_CIDFONT) != 0 {
            return Ok(-1);
        }

        // Mangled name requires more 7 bytes.
        let fontname = cffont.cff_get_name();
        let csi = CidSysInfo {
            registry: Some(b"Adobe".to_vec()),
            ordering: Some(b"Identity".to_vec()),
            supplement: 0,
        };

        cffont.cff_close();

        opt.embed = 1;
        // getting font info. from TrueType tables
        let Some(descriptor) =
            self.tt_get_fontdesc(&mut sfont, &mut opt.embed, opt.stemv, 0, name)?
        else {
            warn!("Could not obtain necessary font info: {:?}", name);
            return Ok(-1);
        };
        fnt(self, font_id).descriptor = Some(descriptor);
        if opt.embed == 0 {
            warn!("Cannot embed font due to font license: {:?}", name);
            return Ok(-1);
        }

        {
            let f = fnt(self, font_id);
            f.fontname = Some(fontname.clone());
            f.subtype = PDF_FONT_FONTTYPE_CIDTYPE0;
            f.cid.csi = csi.clone();
            f.flags |= CIDFONT_FLAG_TYPE1C;
        }

        let resource = self.o.new_dict();
        fnt(self, font_id).resource = Some(resource);
        self.o.put_name(resource, b"Type", b"Font");
        self.o.put_name(resource, b"Subtype", b"CIDFontType0");

        if opt.embed != 0 {
            self.cidtype0_make_unique_tag(font_id);
            let tag = fnt(self, font_id).unique_id;
            let tmp = tagged_name(&tag, &fontname);
            self.o.put_name(descriptor, b"FontName", &tmp);
            self.o.put_name(resource, b"BaseFont", &tmp);
        } else {
            self.o.put_name(descriptor, b"FontName", &fontname);
            self.o.put_name(resource, b"BaseFont", &fontname);
        }
        self.cidtype0_add_csi(resource, &csi);

        sfont.sfnt_close();

        Ok(0)
    }

    /// `CIDFont_type0_t1cdofont`.
    pub fn CIDFont_type0_t1cdofont(&mut self, font_id: i32) -> Result<i32> {
        let (reference, resource, descriptor, filename, index, need_vmetrics, fontname) = {
            let f = fnt(self, font_id);
            (
                f.reference,
                f.resource,
                f.descriptor,
                f.filename.clone().unwrap_or_default(),
                f.index,
                f.cid.need_vmetrics,
                f.fontname.clone().unwrap_or_default(),
            )
        };

        if reference.is_none() {
            return Ok(0);
        }
        let resource = resource.expect("font resource");
        let descriptor = descriptor.expect("font descriptor");

        let r = self.o.ref_obj(descriptor);
        self.o.put(resource, b"FontDescriptor", r);

        let used_chars_rc = fnt(self, font_id).usedchars.clone().expect("usedchars");

        let Some(fp) = open_ot_or_tt(self, &filename)? else {
            warn!("Could not open file: {:?}", filename);
            return Ok(-1);
        };

        let Some(mut sfont) = Sfnt::sfnt_open(fp)? else {
            warn!("Failed to read font data: {:?}", filename);
            return Ok(-1);
        };

        let offset = find_cff_table(&mut sfont, index)?;
        if offset == 0 {
            warn!("Not a CFF/OpenType font: {:?}", filename);
            return Ok(-1);
        }

        let Some(mut cffont) = CffFont::cff_open(sfont.stream.clone(), offset as i32, 0)? else {
            warn!("Failed to read CFF font data: {:?}", filename);
            return Ok(-1);
        };

        if (cffont.flag & FONTTYPE_CIDFONT) != 0 {
            warn!("Unxpected type (CIDFont) found: {:?}", filename);
            return Ok(-1);
        }

        cffont.cff_read_private()?;
        cffont.cff_read_subrs()?;

        let mut default_width = CFF_DEFAULTWIDTHX_DEFAULT;
        let mut nominal_width = CFF_NOMINALWIDTHX_DEFAULT;
        if let Some(p) = cffont.private[0].as_ref() {
            if p.cff_dict_known(b"StdVW") != 0 {
                let stemv = p.cff_dict_get(b"StdVW", 0)?;
                self.o.put_number(descriptor, b"StemV", stemv);
            }
            if p.cff_dict_known(b"defaultWidthX") != 0 {
                default_width = p.cff_dict_get(b"defaultWidthX", 0)?;
            }
            if p.cff_dict_known(b"nominalWidthX") != 0 {
                nominal_width = p.cff_dict_get(b"nominalWidthX", 0)?;
            }
        }

        let mut num_glyphs: u16 = 0;
        let mut last_cid: u16 = 0;
        {
            let mut used_chars = used_chars_rc.borrow_mut();
            add_to_used_chars2(&mut used_chars, 0); // .notdef
            for i in 0..(i32::from(cffont.num_glyphs) + 7) / 8 {
                let c = i32::from(used_chars[i as usize]);
                for j in (0..=7).rev() {
                    if (c & (1 << j)) != 0 {
                        num_glyphs = num_glyphs.wrapping_add(1);
                        last_cid = ((i + 1) * 8 - j - 1) as u16;
                    }
                }
            }
        }
        let used_chars = used_chars_rc.borrow().clone();

        cffont.fdselect = Some(CffFdselect {
            format: 3,
            num_entries: 1,
            ranges: vec![CffRange3 { first: 0, fd: 0 }],
            ..CffFdselect::default()
        });

        {
            let mut charset = CffCharsets {
                format: 0,
                num_entries: num_glyphs.wrapping_sub(1),
                glyphs: vec![0; num_glyphs.wrapping_sub(1) as usize],
                ..CffCharsets::default()
            };

            let mut gid: u16 = 0;
            for cid in 0..=u32::from(last_cid) {
                if is_used_char2(&used_chars, cid) {
                    if gid > 0 {
                        charset.glyphs[gid as usize - 1] = cid as u16;
                    }
                    gid = gid.wrapping_add(1);
                }
            }
            // cff_release_charsets(cffont->charsets);
            cffont.charsets = Some(charset);
        }

        {
            let td = cffont.topdict.as_mut().unwrap();
            td.cff_dict_add(b"CIDCount", 1)?;
            td.cff_dict_set(b"CIDCount", 0, f64::from(last_cid) + 1.0)?;
        }

        {
            let mut fd0 = cff_new_dict();
            fd0.cff_dict_add(b"FontName", 1)?;
            // FIXME: Skip XXXXXX+
            let sid = cffont.cff_add_string(fontname.get(7..).unwrap_or(&[]), 1);
            fd0.cff_dict_set(b"FontName", 0, f64::from(sid))?;
            fd0.cff_dict_add(b"Private", 2)?;
            fd0.cff_dict_set(b"Private", 0, 0.0)?;
            fd0.cff_dict_set(b"Private", 0, 0.0)?;
            cffont.fdarray = vec![Some(fd0)];
        }
        {
            let td = cffont.topdict.as_mut().unwrap();
            // FDArray - index offset, not known yet
            td.cff_dict_add(b"FDArray", 1)?;
            td.cff_dict_set(b"FDArray", 0, 0.0)?;
            // FDSelect - offset, not known yet
            td.cff_dict_add(b"FDSelect", 1)?;
            td.cff_dict_set(b"FDSelect", 0, 0.0)?;

            td.cff_dict_remove(b"UniqueID");
            td.cff_dict_remove(b"XUID");
            td.cff_dict_remove(b"Private");
            td.cff_dict_remove(b"Encoding");
        }

        let offset = cffont
            .topdict
            .as_ref()
            .unwrap()
            .cff_dict_get(b"CharStrings", 0)? as i32;
        cffont.cff_seek_set(offset as usize);
        let idx = cffont.cff_get_index_header()?;
        // offset is now absolute offset ... bad
        let offset = cffont.cff_tell();

        if idx.count < 2 {
            warn!("No valid charstring data found: {:?}", filename);
            return Ok(-1);
        }

        // New CharStrings INDEX
        let mut charstrings = CffIndex::cff_new_index(num_glyphs.wrapping_add(1));
        let mut max_len = 2 * CS_STR_LEN_MAX;
        charstrings.data = vec![0u8; max_len as usize];
        let mut charstring_len: i32 = 0;

        let mut gid: u16 = 0;
        let mut data = vec![0u8; CS_STR_LEN_MAX as usize];
        for cid in 0..=usize::from(last_cid) {
            if !is_used_char2(&used_chars, cid as u32) {
                continue;
            }

            let size = idx.offset[cid + 1].wrapping_sub(idx.offset[cid]) as i32;
            if size > CS_STR_LEN_MAX {
                warn!("Charstring too long:{:?} (gid={})", filename, cid);
                return Ok(-1);
            }
            if charstring_len + CS_STR_LEN_MAX >= max_len {
                max_len = charstring_len + 2 * CS_STR_LEN_MAX;
                charstrings.data.resize(max_len as usize, 0);
            }
            charstrings.offset[gid as usize] = (charstring_len + 1) as u32;
            cffont.cff_seek(offset + idx.offset[cid] as usize - 1);
            let rd = cffont.cff_read_data(size as usize);
            data[..rd.len()].copy_from_slice(&rd);
            charstring_len += self.cs_copy_charstring(
                &mut charstrings.data[charstring_len as usize..max_len as usize],
                &data[..size as usize],
                cffont.gsubr.as_ref(),
                cffont.subrs[0].as_ref(),
                default_width,
                nominal_width,
                None,
            )?;
            gid = gid.wrapping_add(1);
        }
        if gid != num_glyphs {
            warn!("Unexpeced error: {:?}", filename);
            return Ok(-1);
        }
        drop(data);
        drop(idx);

        charstrings.offset[num_glyphs as usize] = (charstring_len + 1) as u32;
        charstrings.count = num_glyphs;
        cffont.num_glyphs = num_glyphs;
        cffont.cstrings = Some(charstrings);

        // no Global subr
        cffont.gsubr = Some(CffIndex::cff_new_index(0));

        if let Some(s) = cffont.subrs.get_mut(0) {
            *s = None;
        }
        if let Some(Some(p)) = cffont.private.get_mut(0) {
            p.cff_dict_remove(b"Subrs"); // no Subrs
        }

        cffont.cff_add_string(b"Adobe", 1);
        cffont.cff_add_string(b"Identity", 1);

        let mut td = cffont.topdict.take().unwrap();
        cff_dict_update(&mut td, &mut cffont);
        cffont.topdict = Some(td);

        if let Some(Some(mut p)) = cffont.private.get_mut(0).map(Option::take) {
            cff_dict_update(&mut p, &mut cffont);
            cffont.private[0] = Some(p);
        }

        cffont.cff_update_string();

        // CFF code need to be rewrote...
        {
            let adobe = cffont.cff_get_sid(b"Adobe");
            let identity = cffont.cff_get_sid(b"Identity");
            let td = cffont.topdict.as_mut().unwrap();
            td.cff_dict_add(b"ROS", 3)?;
            td.cff_dict_set(b"ROS", 0, f64::from(adobe))?;
            td.cff_dict_set(b"ROS", 1, f64::from(identity))?;
            td.cff_dict_set(b"ROS", 2, 0.0)?;
        }

        let _destlen = self.write_fontfile(font_id, &mut cffont)?;

        // DW, W, DW2 and W2: Those values are obtained from OpenType table
        // (not TFM).
        {
            let mut cid_to_gid_map = vec![0u8; 2 * (usize::from(last_cid) + 1)];
            for cid in 0..=usize::from(last_cid) {
                if is_used_char2(&used_chars, cid as u32) {
                    cid_to_gid_map[2 * cid] = ((cid >> 8) & 0xff) as u8;
                    cid_to_gid_map[2 * cid + 1] = (cid & 0xff) as u8;
                }
            }
            self.add_CIDMetrics(
                &mut sfont,
                resource,
                Some(&cid_to_gid_map),
                last_cid,
                i32::from(need_vmetrics != 0),
            )?;
        }

        cffont.cff_close();
        sfont.sfnt_close();

        if self.o.check_version(2, 0) < 0 {
            self.CIDFont_type0_add_CIDSet(font_id, &used_chars, last_cid);
        }

        Ok(0)
    }

    /// `load_base_CMap` (static): the Unicode to CID CMap built from the
    /// glyph names; its cache id or -1.
    fn load_base_CMap(
        &mut self,
        font_name: &[u8],
        wmode: i32,
        cffont: &mut CffFont,
    ) -> Result<i32> {
        let range_min: [u8; 4] = [0x00, 0x00, 0x00, 0x00];
        let range_max: [u8; 4] = [0x7f, 0xff, 0xff, 0xff];

        let mut cmap_name = font_name.to_vec();
        if wmode != 0 {
            cmap_name.extend_from_slice(b"-UCS4-V");
        } else {
            cmap_name.extend_from_slice(b"-UCS4-H");
        }

        let cmap_id = self.CMap_cache_find(&cmap_name)?;
        if cmap_id >= 0 {
            return Ok(cmap_id);
        }

        let mut cmap = CMap::CMap_new();
        cmap.CMap_set_name(&cmap_name);
        cmap.CMap_set_type(CMAP_TYPE_CODE_TO_CID);
        cmap.CMap_set_wmode(wmode);
        cmap.CMap_add_codespacerange(&range_min, &range_max);
        cmap.CMap_set_CIDSysInfo(Some(&CSI_IDENTITY()));

        for gid in 1..cffont.num_glyphs {
            let sid = cffont.cff_charsets_lookup_inverse(gid)?;
            let glyph = cffont.cff_get_string(sid);

            let (name, suffix) = agl_chop_suffix(&glyph);
            let Some(name) = name else {
                continue;
            };

            if suffix.is_some() {
                continue;
            }

            if agl_name_is_unicode(&name) {
                let ucv = agl_name_convert_unicode(&name);
                let src_code = ucv.to_be_bytes();
                cmap.CMap_add_cidchar(&src_code, gid);
            } else {
                let agln = self.agl_lookup_list(&name);
                if agln.is_none() {
                    warn!("Glyph \"{:?}\" inaccessible (no Unicode mapping)", glyph);
                }
                let mut cur = agln.as_ref();
                while let Some(a) = cur {
                    if a.n_components > 1 {
                        warn!("Glyph \"{:?}\" inaccessible (composite character)", glyph);
                    } else if a.n_components == 1 {
                        let ucv = a.unicodes[0];
                        let src_code = ucv.to_be_bytes();
                        cmap.CMap_add_cidchar(&src_code, gid);
                    }
                    cur = a.alternate.as_deref();
                }
            }
        }
        self.CMap_cache_add(cmap)
    }

    /// `t1_load_UnicodeCMap`: a cache id, or -1 (`otl_tags` not supported).
    pub fn t1_load_UnicodeCMap(
        &mut self,
        font_name: &[u8],
        otl_tags: Option<&[u8]>,
        wmode: i32,
    ) -> Result<i32> {
        let Some(mut fp) = self.dpx_open_file(font_name, ResType::T1Font)? else {
            return Ok(-1);
        };

        let Some(mut cffont) = self.t1_load_font(None, 1, &mut fp)? else {
            return Ok(-1);
        };

        let cmap_id = self.load_base_CMap(font_name, wmode, &mut cffont)?;

        cffont.cff_close();

        if cmap_id < 0 {
            warn!(
                "Failed to create Unicode charmap for font \"{:?}\".",
                font_name
            );
            return Ok(-1);
        }

        if otl_tags.is_some() {
            warn!("Glyph substitution not supported for Type1 font yet...");
        }

        Ok(cmap_id)
    }

    /// `create_ToUnicode_stream` (static).
    fn create_ToUnicode_stream(
        &mut self,
        cffont: &mut CffFont,
        font_name: &[u8],
        used_glyphs: &[u8],
    ) -> Result<Option<Obj>> {
        let range_min: [u8; 2] = [0x00, 0x00];
        let range_max: [u8; 2] = [0xff, 0xff];
        let mut wbuf = [0u8; WBUF_SIZE];

        let mut cmap = CMap::CMap_new();

        let mut cmap_name = font_name.to_vec();
        cmap_name.extend_from_slice(b"-UTF16");
        cmap.CMap_set_name(&cmap_name);

        cmap.CMap_set_wmode(0);
        cmap.CMap_set_type(CMAP_TYPE_TO_UNICODE);
        cmap.CMap_set_CIDSysInfo(Some(&CSI_UNICODE()));

        cmap.CMap_add_codespacerange(&range_min, &range_max);

        let mut glyph_count: i32 = 0;
        let mut total_fail_count: i32 = 0;
        for cid in 1..cffont.num_glyphs {
            // Skip .notdef
            if is_used_char2(used_glyphs, u32::from(cid)) {
                wbuf[0] = ((cid >> 8) & 0xff) as u8;
                wbuf[1] = (cid & 0xff) as u8;

                let mut p = 2usize;
                let gid = cffont.cff_charsets_lookup_inverse(cid)?;
                if gid == 0 {
                    continue;
                }
                if let Some(glyph) = cffont.cff_get_string_opt(gid) {
                    let (len, fail_count) = self.agl_sput_UTF16BE(&glyph, &mut wbuf, &mut p);
                    if len < 1 || fail_count != 0 {
                        total_fail_count += fail_count;
                    } else {
                        cmap.CMap_add_bfchar(&wbuf[..2], &wbuf[2..2 + len as usize]);
                    }
                }
                glyph_count += 1;
            }
        }

        let mut stream = None;
        if total_fail_count != 0 && total_fail_count >= glyph_count / 10 {
            warn!(
                "{} glyph names (out of {}) missing Unicode mapping.",
                total_fail_count, glyph_count
            );
            warn!("ToUnicode CMap \"{:?}-UTF16\" removed.", font_name);
        } else {
            stream = self.CMap_create_stream(&cmap)?;
        }
        drop(cmap);

        Ok(stream)
    }

    /// `CIDFont_type0_t1create_ToUnicode_stream`: a reference, or none.
    pub fn CIDFont_type0_t1create_ToUnicode_stream(
        &mut self,
        filename: &[u8],
        fontname: &[u8],
        used_chars: &[u8],
    ) -> Result<Option<Obj>> {
        let mut r = None;

        let mut fp = some!(self.dpx_open_file(filename, ResType::T1Font)?);

        if let Some(mut cffont) = self.t1_load_font(None, 1, &mut fp)? {
            if let Some(tounicode) =
                self.create_ToUnicode_stream(&mut cffont, fontname, used_chars)?
            {
                r = Some(self.o.ref_obj(tounicode));
                self.o.release(tounicode);
            }
        }

        Ok(r)
    }

    /// `get_font_attr` (static): descriptor entries from the glyphs.
    fn get_font_attr(&mut self, font_id: i32, cffont: &mut CffFont) -> Result<()> {
        let mut capheight;
        let mut ascent;
        let mut descent;
        let italicangle;
        let stemv;
        let defaultwidth;
        let nominalwidth;
        let mut flags: i32 = 0;
        let mut gm = T1Ginfo::default();

        defaultwidth = 500.0;
        nominalwidth = 0.0;

        // CapHeight, Ascent, and Descent is meaningfull only for
        // Latin/Greek/Cyrillic. The BlueValues and OtherBlues also have
        // those information.
        let td = cffont.topdict.as_ref().unwrap();
        if td.cff_dict_known(b"FontBBox") != 0 {
            // Default values
            ascent = td.cff_dict_get(b"FontBBox", 3)?;
            capheight = ascent;
            descent = td.cff_dict_get(b"FontBBox", 1)?;
        } else {
            capheight = 680.0;
            ascent = 690.0;
            descent = -190.0;
        }
        let p0 = cffont.private[0].as_ref().expect("Private DICT");
        if p0.cff_dict_known(b"StdVW") != 0 {
            stemv = p0.cff_dict_get(b"StdVW", 0)?;
        } else {
            // We may use the following values for StemV:
            //  Thin - ExtraLight: <= 50, Light: 71, Regular(Normal): 88,
            //  Medium: 109, SemiBold(DemiBold): 135, Bold - Heavy: >= 166
            stemv = 88.0;
        }
        if td.cff_dict_known(b"ItalicAngle") != 0 {
            italicangle = td.cff_dict_get(b"ItalicAngle", 0)?;
            if italicangle != 0.0 {
                flags |= FONT_FLAG_ITALIC;
            }
        } else {
            italicangle = 0.0;
        }

        // Use "space", "H", "p", and "b" for various values. Those
        // characters should not "seac". (no accent)
        let mut defaultwidth = defaultwidth;
        {
            let gid = i32::from(cffont.cff_glyph_lookup(b"space")?);
            if let Some(s) = glyph_cs(cffont, gid) {
                self.t1char_get_metrics(s, cffont.subrs[0].as_ref(), Some(&mut gm))?;
                defaultwidth = gm.wx;
            }
        }

        for name in L_C {
            let gid = i32::from(cffont.cff_glyph_lookup(name)?);
            if let Some(s) = glyph_cs(cffont, gid) {
                self.t1char_get_metrics(s, cffont.subrs[0].as_ref(), Some(&mut gm))?;
                capheight = gm.bbox.ury;
                break;
            }
        }

        for name in L_D {
            let gid = i32::from(cffont.cff_glyph_lookup(name)?);
            if let Some(s) = glyph_cs(cffont, gid) {
                self.t1char_get_metrics(s, cffont.subrs[0].as_ref(), Some(&mut gm))?;
                descent = gm.bbox.lly;
                break;
            }
        }

        for name in L_A {
            let gid = i32::from(cffont.cff_glyph_lookup(name)?);
            if let Some(s) = glyph_cs(cffont, gid) {
                self.t1char_get_metrics(s, cffont.subrs[0].as_ref(), Some(&mut gm))?;
                ascent = gm.bbox.ury;
                break;
            }
        }

        let p0 = cffont.private[0].as_mut().expect("Private DICT");
        if defaultwidth != 0.0 {
            p0.cff_dict_add(b"defaultWidthX", 1)?;
            p0.cff_dict_set(b"defaultWidthX", 0, defaultwidth)?;
        }
        if nominalwidth != 0.0 {
            p0.cff_dict_add(b"nominalWidthX", 1)?;
            p0.cff_dict_set(b"nominalWidthX", 0, nominalwidth)?;
        }
        if p0.cff_dict_known(b"ForceBold") != 0 && p0.cff_dict_get(b"ForceBold", 0)? != 0.0 {
            flags |= FONT_FLAG_FORCEBOLD;
        }
        if p0.cff_dict_known(b"IsFixedPitch") != 0 && p0.cff_dict_get(b"IsFixedPitch", 0)? != 0.0 {
            flags |= FONT_FLAG_FIXEDPITCH;
        }
        let (fontname, descriptor) = {
            let f = fnt(self, font_id);
            (f.fontname.clone(), f.descriptor.expect("font descriptor"))
        };
        if let Some(fname) = fontname {
            if !contains(&fname, b"Sans") {
                flags |= FONT_FLAG_SERIF;
            }
        }
        flags |= FONT_FLAG_SYMBOLIC;

        self.o.put_number(descriptor, b"CapHeight", capheight);
        self.o.put_number(descriptor, b"Ascent", ascent);
        self.o.put_number(descriptor, b"Descent", descent);
        self.o.put_number(descriptor, b"ItalicAngle", italicangle);
        self.o.put_number(descriptor, b"StemV", stemv);
        self.o.put_number(descriptor, b"Flags", f64::from(flags));
        Ok(())
    }

    /// `add_metrics` (static): `/W` (and `/DW`) from `widths`.
    fn add_metrics(
        &mut self,
        font_id: i32,
        cffont: &mut CffFont,
        cid_to_gid_map: &[u8],
        widths: &[f64],
        default_width: f64,
        last_cid: Cid,
    ) -> Result<()> {
        let (descriptor, resource, used_chars_rc) = {
            let f = fnt(self, font_id);
            (
                f.descriptor.expect("font descriptor"),
                f.resource.expect("font resource"),
                f.usedchars.clone().expect("usedchars"),
            )
        };

        // The original FontBBox of the font is preserved, instead of
        // replacing it with tight bounding box calculated from charstrings,
        // to prevent Acrobat 4 from greeking text as much as possible.
        let td = cffont.topdict.as_ref().unwrap();
        if td.cff_dict_known(b"FontBBox") == 0 {
            warn!("No FontBBox found");
        } else {
            let tmp = self.o.new_array();
            for i in 0..4 {
                let val = td.cff_dict_get(b"FontBBox", i)?;
                let n = self.o.new_number(round_acc(val, 1.0));
                self.o.add_array(tmp, n);
            }
            self.o.put(descriptor, b"FontBBox", tmp);
        }

        let used_chars = used_chars_rc.borrow().clone();

        // FIXME: This writes "CID CID width". I think it's better to handle
        // each 8 char block and to use "CID_start [ w0 w1 ...]".
        let tmp = self.o.new_array();
        for cid in 0..=u32::from(last_cid) {
            if is_used_char2(&used_chars, cid) {
                let gid = (usize::from(cid_to_gid_map[2 * cid as usize]) << 8)
                    | usize::from(cid_to_gid_map[2 * cid as usize + 1]);
                if widths[gid] != default_width {
                    for v in [f64::from(cid), f64::from(cid), round_acc(widths[gid], 1.0)] {
                        let n = self.o.new_number(v);
                        self.o.add_array(tmp, n);
                    }
                }
            }
        }
        self.o.put_number(resource, b"DW", default_width);
        if self.o.array_length(tmp) > 0 {
            let r = self.o.ref_obj(tmp);
            self.o.put(resource, b"W", r);
        }
        self.o.release(tmp);
        Ok(())
    }

    /// `CIDFont_type0_t1dofont`.
    pub fn CIDFont_type0_t1dofont(&mut self, font_id: i32) -> Result<i32> {
        let (reference, resource, descriptor, filename) = {
            let f = fnt(self, font_id);
            (
                f.reference,
                f.resource,
                f.descriptor,
                f.filename.clone().unwrap_or_default(),
            )
        };

        if reference.is_none() {
            return Ok(0);
        }
        let resource = resource.expect("font resource");
        let descriptor = descriptor.expect("font descriptor");

        let r = self.o.ref_obj(descriptor);
        self.o.put(resource, b"FontDescriptor", r);

        let Some(mut fp) = self.dpx_open_file(&filename, ResType::T1Font)? else {
            warn!("Type1: Could not open Type1 font.");
            return Ok(-1);
        };

        let Some(mut cffont) = self.t1_load_font(None, 0, &mut fp)? else {
            warn!("Could not read Type 1 font...");
            return Ok(-1);
        };

        let fontname = fnt(self, font_id).fontname.clone().expect("font->fontname");
        let used_chars_rc = fnt(self, font_id)
            .usedchars
            .clone()
            .expect("font->usedchars");

        cffont.cff_set_name(&fontname)?;

        // defaultWidthX, CapHeight, etc.
        self.get_font_attr(font_id, &mut cffont)?;
        let p0 = cffont.private[0].as_ref().expect("Private DICT");
        let defaultwidth = if p0.cff_dict_known(b"defaultWidthX") != 0 {
            p0.cff_dict_get(b"defaultWidthX", 0)?
        } else {
            0.0
        };
        let nominalwidth = if p0.cff_dict_known(b"nominalWidthX") != 0 {
            p0.cff_dict_get(b"nominalWidthX", 0)?
        } else {
            0.0
        };

        let mut num_glyphs: i32 = 0;
        let mut last_cid: u16 = 0;
        {
            let mut used_chars = used_chars_rc.borrow_mut();
            add_to_used_chars2(&mut used_chars, 0); // .notdef
            for i in 0..(i32::from(cffont.num_glyphs) + 7) / 8 {
                let c = i32::from(used_chars[i as usize]);
                for j in (0..=7).rev() {
                    if (c & (1 << j)) != 0 {
                        num_glyphs += 1;
                        last_cid = ((i + 1) * 8 - j - 1) as u16;
                    }
                }
            }
        }
        let used_chars = used_chars_rc.borrow().clone();

        cffont.fdselect = Some(CffFdselect {
            format: 3,
            num_entries: 1,
            ranges: vec![CffRange3 { first: 0, fd: 0 }],
            ..CffFdselect::default()
        });

        let mut cid_to_gid_map = vec![0u8; 2 * (usize::from(last_cid) + 1)];
        {
            let mut charset = CffCharsets {
                format: 0,
                num_entries: (num_glyphs - 1) as u16,
                glyphs: vec![0; (num_glyphs - 1) as usize],
                ..CffCharsets::default()
            };

            let mut gid: u16 = 0;
            for cid in 0..=usize::from(last_cid) {
                if is_used_char2(&used_chars, cid as u32) {
                    if gid > 0 {
                        charset.glyphs[gid as usize - 1] = cid as u16;
                    }
                    cid_to_gid_map[2 * cid] = ((gid >> 8) & 0xff) as u8;
                    cid_to_gid_map[2 * cid + 1] = (gid & 0xff) as u8;
                    gid = gid.wrapping_add(1);
                }
            }

            cffont.charsets = Some(charset);
        }

        {
            let td = cffont.topdict.as_mut().unwrap();
            td.cff_dict_add(b"CIDCount", 1)?;
            td.cff_dict_set(b"CIDCount", 0, f64::from(last_cid) + 1.0)?;
        }

        {
            let mut fd0 = cff_new_dict();
            fd0.cff_dict_add(b"FontName", 1)?;
            // FIXME: Skip XXXXXX+
            let sid = cffont.cff_add_string(fontname.get(7..).unwrap_or(&[]), 1);
            fd0.cff_dict_set(b"FontName", 0, f64::from(sid))?;
            fd0.cff_dict_add(b"Private", 2)?;
            fd0.cff_dict_set(b"Private", 0, 0.0)?;
            fd0.cff_dict_set(b"Private", 0, 0.0)?;
            cffont.fdarray = vec![Some(fd0)];
        }

        {
            let td = cffont.topdict.as_mut().unwrap();
            // FDArray - index offset, not known yet
            td.cff_dict_add(b"FDArray", 1)?;
            td.cff_dict_set(b"FDArray", 0, 0.0)?;
            // FDSelect - offset, not known yet
            td.cff_dict_add(b"FDSelect", 1)?;
            td.cff_dict_set(b"FDSelect", 0, 0.0)?;

            td.cff_dict_add(b"charset", 1)?;
            td.cff_dict_set(b"charset", 0, 0.0)?;

            td.cff_dict_add(b"CharStrings", 1)?;
            td.cff_dict_set(b"CharStrings", 0, 0.0)?;
        }

        {
            let mut gm = T1Ginfo::default();
            let mut max: i32 = 0;
            let mut widths = vec![0.0f64; num_glyphs as usize];
            let mut w_stat = [0i32; 1001];
            let mut offset: i32 = 0;
            let mut cstring = CffIndex::cff_new_index(num_glyphs as u16);
            cstring.data = Vec::new();
            cstring.offset[0] = 1;
            let mut gid: u16 = 0;
            for cid in 0..=usize::from(last_cid) {
                if !is_used_char2(&used_chars, cid as u32) {
                    continue;
                }

                if offset + CS_STR_LEN_MAX >= max {
                    max += CS_STR_LEN_MAX * 2;
                    cstring.data.resize(max as usize, 0);
                }
                let a = cstring.offset[gid as usize] as usize - 1;
                let cs = cffont.cstrings.as_ref().expect("CharStrings");
                let sa = cs.offset[cid] as usize - 1;
                let sl = cs.offset[cid + 1].wrapping_sub(cs.offset[cid]) as usize;
                offset += self.t1char_convert_charstring(
                    &mut cstring.data[a..a + CS_STR_LEN_MAX as usize],
                    &cs.data[sa..sa + sl],
                    cffont.subrs[0].as_ref(),
                    defaultwidth,
                    nominalwidth,
                    Some(&mut gm),
                )?;
                cstring.offset[gid as usize + 1] = (offset + 1) as u32;
                if gm.use_seac != 0 {
                    warn!(
                        "The \"seac\" command for an accented character found: {:?}",
                        filename
                    );
                    return Ok(-1);
                }
                widths[gid as usize] = gm.wx;
                if gm.wx >= 0.0 && gm.wx <= 1000.0 {
                    w_stat[gm.wx as i32 as usize] += 1;
                }
                gid = gid.wrapping_add(1);
            }

            cffont.cstrings = Some(cstring);

            let mut max_count = 0;
            let mut dw: i32 = -1;
            for i in 0..=1000 {
                if w_stat[i] > max_count {
                    dw = i as i32;
                    max_count = w_stat[i];
                }
            }
            if dw >= 0 {
                self.add_metrics(
                    font_id,
                    &mut cffont,
                    &cid_to_gid_map,
                    &widths,
                    f64::from(dw),
                    last_cid,
                )?;
            } else {
                self.add_metrics(
                    font_id,
                    &mut cffont,
                    &cid_to_gid_map,
                    &widths,
                    defaultwidth,
                    last_cid,
                )?;
            }
        }
        cffont.subrs[0] = None;

        drop(cid_to_gid_map);

        cffont.cff_add_string(b"Adobe", 1);
        cffont.cff_add_string(b"Identity", 1);

        let mut td = cffont.topdict.take().unwrap();
        cff_dict_update(&mut td, &mut cffont);
        cffont.topdict = Some(td);
        let mut p0 = cffont.private[0].take().expect("Private DICT");
        cff_dict_update(&mut p0, &mut cffont);
        cffont.private[0] = Some(p0);

        cffont.cff_update_string();

        // CFF code need to be rewrote...
        {
            let adobe = cffont.cff_get_sid(b"Adobe");
            let identity = cffont.cff_get_sid(b"Identity");
            let td = cffont.topdict.as_mut().unwrap();
            td.cff_dict_add(b"ROS", 3)?;
            td.cff_dict_set(b"ROS", 0, f64::from(adobe))?;
            td.cff_dict_set(b"ROS", 1, f64::from(identity))?;
            td.cff_dict_set(b"ROS", 2, 0.0)?;
        }

        cffont.num_glyphs = num_glyphs as u16;
        let _ = self.write_fontfile(font_id, &mut cffont)?;

        cffont.cff_close();

        if self.o.check_version(2, 0) < 0 {
            self.CIDFont_type0_add_CIDSet(font_id, &used_chars, last_cid);
        }

        Ok(0)
    }
}

/// The charstring of `gid` when `gid >= 0 && gid < cstrings->count`.
fn glyph_cs(cffont: &CffFont, gid: i32) -> Option<&[u8]> {
    let cs = cffont.cstrings.as_ref().expect("CharStrings");
    if gid >= 0 && gid < i32::from(cs.count) {
        let g = gid as usize;
        let a = cs.offset[g] as usize - 1;
        let len = cs.offset[g + 1].wrapping_sub(cs.offset[g]) as usize;
        Some(&cs.data[a..a + len])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert!(contains(b"LMSans10-Regular", b"Sans"));
        assert!(!contains(b"LMRoman10", b"Sans"));
        let mut tag = [0u8; 7];
        tag[..6].copy_from_slice(b"ABCDEF");
        assert_eq!(tagged_name(&tag, b"Foo"), b"ABCDEF+Foo");
        let head = TtHeadTable {
            units_per_em: 2048,
            ..TtHeadTable::default()
        };
        assert_eq!(pdfunit(1229.0, &head), 600.0);
        assert_eq!(pdfunit(1025.0, &head), 500.0);
    }
}
