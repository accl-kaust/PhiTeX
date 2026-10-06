//! spc_pdfm.c, spc_pdfm.h: dvipdfmx's `pdf:` specials.
//!
//! C's single `struct spc_pdf_ _pdf_stat` is this module's `State`
//! (`self.pdfm`); the `void *dp` of its init/clean is dropped.
//! `pdf_foreach_dict` callbacks are functions taking the key, the value
//! and what C's `dp` really is.

use crate::dpxutil::{max4, min4, parse_c_ident};
use crate::fmt::Buf;
use crate::fontmap::{
    FONTMAP_RMODE_APPEND, FONTMAP_RMODE_REMOVE, FONTMAP_RMODE_REPLACE, FontmapRec, is_pdfm_mapline,
    pdf_clear_fontmap_record, pdf_init_fontmap_record, pdf_read_fontmap_line,
};
use crate::obj::{PDF_ARRAY, PDF_DICT, PDF_INDIRECT, PDF_STREAM, PDF_STRING, STREAM_COMPRESS};
use crate::parse::{parse_ident, parse_opt_ident, parse_val_ident, skip_white};
use crate::pdfcolor::PdfColor;
use crate::pdfdev::{
    INFO_DO_HIDE, INFO_HAS_HEIGHT, INFO_HAS_USER_BBOX, INFO_HAS_WIDTH, PdfCoord, PdfRect,
    PdfTmatrix, TransformInfo,
};
use crate::pdfdraw::{pdf_copymatrix, pdf_setmatrix};
use crate::pdfximage::{LoadOptions, PDF_XOBJECT_TYPE_IMAGE};
use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler, THEBUFFLENGTH, cstr};
use crate::unicode::{
    UC_UTF8_decode_char, UC_UTF8_is_valid_string, UC_UTF16BE_encode_char, UC_is_valid,
};

/// `SPC_PDFM_SUPPORT_ANNOT_TRANS`.
pub const SPC_PDFM_SUPPORT_ANNOT_TRANS: bool = true;

/// `STRING_STREAM`, the `type` of `spc_handler_pdfm_stream_with_type`.
pub const STRING_STREAM: i32 = 0;
/// `FILE_STREAM`.
pub const FILE_STREAM: i32 = 1;

/// `struct tounicode`: for the to-UTF16-BE conversion of text strings.
#[derive(Clone, Debug)]
pub struct Tounicode {
    pub cmap_id: i32,
    pub unescape_backslash: i32,
    /// An array of PDF names.
    pub taintkeys: Option<Obj>,
}

impl Default for Tounicode {
    fn default() -> Self {
        Tounicode {
            cmap_id: -1,
            unescape_backslash: 0,
            taintkeys: None,
        }
    }
}

/// `struct spc_pdf_` (`_pdf_stat`).
#[derive(Clone, Debug)]
pub struct State {
    /// Pending annotation dict.
    pub annot_dict: Option<Obj>,
    /// Current min level of outlines.
    pub lowest_level: i32,
    pub cd: Tounicode,
    /// Added to all page resource dicts.
    pub pageresources: Option<Obj>,
}

impl Default for State {
    fn default() -> Self {
        State {
            annot_dict: None,
            lowest_level: 255,
            cd: Tounicode::default(),
            pageresources: None,
        }
    }
}

/// `default_taintkeys` of `spc_handler_pdfm__init`.
pub const DEFAULT_TAINTKEYS: [&[u8]; 11] = [
    b"Title",
    b"Author",
    b"Subject",
    b"Keywords",
    b"Creator",
    b"Producer",
    b"Contents",
    b"Subj",
    b"TU",
    b"T",
    b"TM",
];

/// `pdf_foreach_dict`: `proc` on each entry in order, until one returns
/// nonzero (that value is returned). C's `dp` is what `proc` captures.
pub(crate) fn foreach_dict(
    dpx: &mut Dpx,
    dict: Obj,
    mut proc: impl FnMut(&mut Dpx, Obj, Option<Obj>) -> Result<i32>,
) -> Result<i32> {
    if !dpx.o.is_dict(Some(dict)) {
        crate::fatal!("typecheck: Invalid object type");
    }
    let mut error = 0;
    for (k, v) in dpx.o.dict_entries(dict)? {
        if error != 0 {
            break;
        }
        error = proc(dpx, k, v)?;
    }
    Ok(error)
}

/// `strstr(h, n) != NULL`.
fn contains(h: &[u8], n: &[u8]) -> bool {
    h.windows(n.len()).any(|w| w == n)
}

/// `calculate_size_utf16` (static): the UTF-16 size of UTF-8 `p`.
fn calculate_size_utf16(p: &[u8]) -> Result<usize> {
    let mut len = 0;
    let mut i = 0;
    while i < p.len() {
        let c = p[i];
        if c < 0x80 {
            len += 2;
            i += 1;
        } else if c < 0xE0 {
            len += 2;
            i += 2;
        } else if c < 0xF0 {
            len += 2;
            i += 3;
        } else if c < 0xF8 {
            len += 4; // Surrogate
            i += 4;
        } else if c < 0xFC {
            len += 4; // Surrogate
            i += 5;
        } else if c < 0xFE {
            len += 4; // Surrogate
            i += 6;
        } else {
            // C loops forever here; never reached, the string being valid
            // UTF-8.
            crate::fatal!("calculate_size_utf16: invalid UTF-8");
        }
    }
    Ok(len)
}

impl Dpx {
    /// `spc_pdfm_at_begin_document`.
    pub fn spc_pdfm_at_begin_document(&mut self) -> Result<i32> {
        spc_handler_pdfm__init(self)
    }
    /// `spc_pdfm_at_end_document`.
    pub fn spc_pdfm_at_end_document(&mut self) -> Result<i32> {
        spc_handler_pdfm__clean(self)
    }
    /// `spc_pdfm_at_end_page`: adds `pageresources` to the page.
    pub fn spc_pdfm_at_end_page(&mut self) -> Result<i32> {
        if let Some(pr) = self.pdfm.pageresources {
            foreach_dict(self, pr, |dpx, k, v| {
                forallresourcecategory(dpx, k, v.expect("vp"))
            })?;
        }
        Ok(0)
    }
}

/// `parse_pdf_reference` (static): the parser's `@name` callback.
fn parse_pdf_reference(dpx: &mut Dpx, s: &[u8], pp: &mut usize) -> Result<Option<Obj>> {
    skip_white(s, pp);
    match parse_opt_ident(s, pp) {
        Some(name) => {
            let result = dpx.spc_lookup_reference(&name)?;
            if result.is_none() {
                crate::warn!("Could not find the named reference (@{:?}).", name);
            }
            Ok(result)
        }
        None => {
            crate::warn!("Could not find a reference name.");
            Ok(None)
        }
    }
}

/// `parse_pdf_object_extended(&ap->curptr, ap->endptr, NULL,
/// parse_pdf_reference, spe)`.
fn parse_ext(dpx: &mut Dpx, args: &mut SpcArg) -> Result<Option<Obj>> {
    let (s, pp) = args.parts();
    dpx.parse_pdf_object_extended(s, pp, parse_pdf_reference)
}

/// `spc_handler_pdfm__init`.
fn spc_handler_pdfm__init(dpx: &mut Dpx) -> Result<i32> {
    // The following dictionary entry keys are considered as keys for
    // text strings. Be sure that string object is NOT always a text
    // string.
    dpx.pdfm.annot_dict = None;
    dpx.pdfm.lowest_level = 255;
    let taintkeys = dpx.o.new_array();
    dpx.pdfm.cd.taintkeys = Some(taintkeys);
    for k in DEFAULT_TAINTKEYS {
        let n = dpx.o.new_name(k);
        dpx.o.add_array(taintkeys, n)?;
    }
    dpx.pdfm.pageresources = None;
    Ok(0)
}

/// `spc_handler_pdfm__clean`.
fn spc_handler_pdfm__clean(dpx: &mut Dpx) -> Result<i32> {
    if let Some(a) = dpx.pdfm.annot_dict {
        crate::warn!("Unbalanced bann and eann found.");
        dpx.o.release(a)?;
    }
    dpx.pdfm.lowest_level = 255;
    dpx.pdfm.annot_dict = None;
    if let Some(t) = dpx.pdfm.cd.taintkeys.take() {
        dpx.o.release(t)?;
    }
    if let Some(p) = dpx.pdfm.pageresources.take() {
        dpx.o.release(p)?;
    }
    Ok(0)
}

/// `safeputresdent` (foreach callback; `dp` a dict).
fn safeputresdent(dpx: &mut Dpx, kp: Obj, vp: Obj, dp: Obj) -> Result<i32> {
    let key = dpx.o.name_value(kp)?.to_vec();
    if dpx.o.lookup_dict(dp, &key)?.is_some() {
        crate::warn!("Object {:?} already defined in dict! (ignored)", key);
    } else {
        let k = dpx.o.link(kp)?;
        let v = dpx.o.link(vp)?;
        dpx.o.add_dict(dp, k, Some(v))?;
    }
    Ok(0)
}

/// `safeputresdict` (foreach callback; `dp` a dict).
fn safeputresdict(dpx: &mut Dpx, kp: Obj, vp: Obj, dp: Obj) -> Result<i32> {
    let key = dpx.o.name_value(kp)?.to_vec();
    let mut dict = dpx.o.lookup_dict(dp, &key)?;

    // Not sure what is the best way to handle this situation
    if dpx.o.is_indirect(dict) && dpx.o.is_indirect(Some(vp)) {
        // If two indirect objects are pointing the same object, do nothing.
        if !dpx.o.compare_reference(dict.expect("dict"), vp) {
            return Ok(0);
        }
        // otherwise merge the content of old one (see below)
        dict = dpx.o.deref_obj(dict)?;
        dpx.o.release_opt(dict)?; // decrement link count
    }

    if dpx.o.type_of(Some(vp)) == PDF_INDIRECT {
        // Copy the content of old resource category dict (if exists)
        if let Some(dict) = dict {
            if let Some(dst) = dpx.o.deref_obj(Some(vp))? {
                if dpx.o.type_of(Some(dst)) == PDF_DICT {
                    foreach_dict(dpx, dict, |dpx, k, v| {
                        safeputresdent(dpx, k, v.expect("vp"), dst)
                    })?;
                    dpx.o.release(dst)?;
                } else {
                    crate::warn!(
                        "Invalid type (not DICT) for page/form resource dict entry: key={:?}",
                        key
                    );
                    dpx.o.release(dst)?;
                    return Ok(-1);
                }
            }
        }
        let v = dpx.o.link(vp)?;
        dpx.o.put(dp, &key, v)?;
    } else if dpx.o.type_of(Some(vp)) == PDF_DICT {
        if let Some(dict) = dict {
            foreach_dict(dpx, vp, |dpx, k, v| {
                safeputresdent(dpx, k, v.expect("vp"), dict)
            })?;
        } else {
            let v = dpx.o.link(vp)?;
            dpx.o.put(dp, &key, v)?;
        }
    } else {
        crate::warn!(
            "Invalid type (not DICT) for page/form resource dict entry: key={:?}",
            key
        );
        return Ok(-1);
    }
    Ok(0)
}

/// `putpageresources` (foreach callback; `dp` the category name).
fn putpageresources(dpx: &mut Dpx, kp: Obj, vp: Obj, category: &[u8]) -> Result<i32> {
    let resource_name = dpx.o.name_value(kp)?.to_vec();
    let v = dpx.o.link(vp)?;
    dpx.pdf_doc_add_page_resource(category, &resource_name, v)?;
    Ok(0)
}

/// `forallresourcecategory` (foreach callback; `dp` unused).
fn forallresourcecategory(dpx: &mut Dpx, kp: Obj, vp: Obj) -> Result<i32> {
    let mut r = -1;
    let category = dpx.o.name_value(kp)?.to_vec();
    match dpx.o.type_of(Some(vp)) {
        PDF_DICT => {
            r = foreach_dict(dpx, vp, |dpx, k, v| {
                putpageresources(dpx, k, v.expect("vp"), &category)
            })?;
        }
        PDF_INDIRECT => {
            // In case pdf:pageresouces << /Category @res >>
            let obj = dpx.o.deref_obj(Some(vp))?;
            match obj {
                None => {
                    crate::warn!("Can't deref object for page resource: {:?}", category);
                    r = -1;
                }
                Some(obj) if dpx.o.type_of(Some(obj)) != PDF_DICT => {
                    crate::warn!("Invalid object type for page resource: {:?}", category);
                    r = -1;
                    // (C does not release `obj` here.)
                }
                Some(obj) => {
                    let res_dict = dpx
                        .pdf_doc_current_page_resources()
                        .expect("page resources");
                    let dict = dpx.o.lookup_dict(res_dict, &category)?;
                    match dict {
                        None => {
                            let v = dpx.o.link(vp)?;
                            dpx.o.put(res_dict, &category, v)?;
                        }
                        Some(mut dict) => {
                            if dpx.o.type_of(Some(dict)) == PDF_INDIRECT {
                                let d = dpx.o.deref_obj(Some(dict))?;
                                // FIXME: jus to decrement link counter
                                dpx.o.release_opt(d)?;
                                dict = d.expect("deref");
                            }
                            // With the below code resource dictionary is
                            // replaced by user supplied one, @res.
                            foreach_dict(dpx, dict, |dpx, k, v| {
                                safeputresdent(dpx, k, v.expect("vp"), obj)
                            })?;
                            let v = dpx.o.link(vp)?;
                            dpx.o.put(res_dict, &category, v)?;
                        }
                    }
                    dpx.o.release(obj)?;
                }
            }
        }
        _ => {
            crate::warn!(
                "Invalid object type for page resource specified for {:?}",
                category
            );
        }
    }
    Ok(r)
}

/// `reencode_string_from_utf8_to_utf16be`.
fn reencode_string_from_utf8_to_utf16be(dpx: &mut Dpx, instring: Obj) -> Result<i32> {
    let mut error = 0;
    let strptr = dpx.o.string_value(instring)?.to_vec();

    // check if the input string is strictly ASCII
    let non_ascii = strptr.iter().filter(|&&c| c > 127).count();
    if non_ascii == 0 {
        return Ok(0); // no need to reencode ASCII strings
    }

    if !UC_UTF8_is_valid_string(&strptr) {
        error = -1;
    } else {
        let mut p = 0;
        // Rough estimate of output length.
        let len = calculate_size_utf16(&strptr)? + 2;
        let mut buf = vec![0u8; len];
        buf[0] = 0xfe;
        buf[1] = 0xff;
        let mut q = 2;
        while p < strptr.len() && q < len && error == 0 {
            let ucv = UC_UTF8_decode_char(&strptr, &mut p);
            if !UC_is_valid(ucv) {
                error = -1;
            } else {
                let count = UC_UTF16BE_encode_char(ucv, &mut buf, &mut q);
                if count == 0 {
                    error = -1;
                }
            }
        }
        if error == 0 {
            dpx.o.set_string(instring, &buf[..q])?;
        }
    }
    Ok(error)
}

/// `reencode_string`: `cmap_id` is the CMap cache id (C's `CMap *`, none
/// when negative).
fn reencode_string(dpx: &mut Dpx, cmap_id: i32, instring: Obj) -> Result<i32> {
    let mut error = 0;

    if !dpx.o.is_string(Some(instring)) {
        return Ok(-1);
    }

    if cmap_id >= 0 {
        let inbuf = dpx.o.string_value(instring)?.to_vec();
        let mut inbufleft = inbuf.len() as i32;
        let mut inbufcur = 0;

        let obufsize = inbufleft * 4 + 2;
        let mut obuf = vec![0u8; obufsize as usize];
        obuf[0] = 0xfe;
        obuf[1] = 0xff;
        let mut obufcur = 2;
        let mut obufleft = obufsize - 2;

        let cmap = dpx.CMap_cache_get(cmap_id)?;
        dpx.CMap_decode(
            cmap,
            &inbuf,
            &mut inbufcur,
            &mut inbufleft,
            &mut obuf,
            &mut obufcur,
            &mut obufleft,
        )?;

        if inbufleft > 0 {
            error = -1;
        }
        if error == 0 {
            dpx.o
                .set_string(instring, &obuf[..(obufsize - obufleft) as usize])?;
        }
    }
    Ok(error)
}

/// `need_reencode`.
fn need_reencode(dpx: &mut Dpx, kp: Obj, vp: Obj, cd: &Tounicode) -> Result<bool> {
    let mut r = false;
    let taintkeys = cd.taintkeys.expect("taintkeys");
    for i in 0..dpx.o.array_length(taintkeys)? {
        let tk = dpx.o.get_array(taintkeys, i as i32)?.expect("taint key");
        if dpx.o.name_value(kp)? == dpx.o.name_value(tk)? {
            r = true;
            break;
        }
    }
    if r {
        // Check UTF-16BE BOM.
        let v = dpx.o.string_value(vp)?;
        if v.len() >= 2 && &v[..2] == b"\xfe\xff" {
            r = false;
        }
    }
    Ok(r)
}

/// `modify_strings` (foreach callback; `dp` the `struct tounicode`).
fn modify_strings(dpx: &mut Dpx, kp: Obj, vp: Obj, cd: Option<&Tounicode>) -> Result<i32> {
    let mut r = 0; // continue

    match dpx.o.type_of(Some(vp)) {
        PDF_STRING => {
            if let Some(cd) = cd.filter(|cd| cd.cmap_id >= 0 && cd.taintkeys.is_some()) {
                if need_reencode(dpx, kp, vp, cd)? {
                    r = reencode_string(dpx, cd.cmap_id, vp)?;
                }
            } else if let Some(cd) = cd.filter(|cd| {
                (dpx.conf.compat_mode == crate::ctx::CompatMode::Xdv || dpx.conf.pdfm_str_utf8)
                    && cd.taintkeys.is_some()
            }) {
                if need_reencode(dpx, kp, vp, cd)? {
                    r = reencode_string_from_utf8_to_utf16be(dpx, vp)?;
                }
            }
            if r < 0 {
                // error occured...
                crate::warn!(
                    "Input string conversion (to UTF16BE) failed for {:?}...",
                    dpx.o.name_value(kp)?
                );
            }
        }
        // Array elements are also checked.
        PDF_ARRAY => {
            for i in 0..dpx.o.array_length(vp)? {
                if let Some(obj) = dpx.o.get_array(vp, i as i32)? {
                    r = modify_strings(dpx, kp, obj, cd)?;
                }
                if r < 0 {
                    break;
                }
            }
        }
        PDF_DICT => {
            r = foreach_dict(dpx, vp, |dpx, k, v| match v {
                Some(v) => modify_strings(dpx, k, v, cd),
                None => Ok(0),
            })?;
        }
        PDF_STREAM => {
            let d = dpx.o.stream_dict(vp)?;
            r = foreach_dict(dpx, d, |dpx, k, v| match v {
                Some(v) => modify_strings(dpx, k, v, cd),
                None => Ok(0),
            })?;
        }
        _ => {}
    }
    Ok(r)
}

/// `parse_pdf_dict_with_tounicode`.
fn parse_pdf_dict_with_tounicode(
    dpx: &mut Dpx,
    s: &[u8],
    pp: &mut usize,
    cd: &Tounicode,
) -> Result<Option<Obj>> {
    let mut dict;

    // disable this test for XDV files, as we do UTF8 reencoding with no
    // cmap
    if dpx.conf.compat_mode != crate::ctx::CompatMode::Xdv
        && !dpx.conf.pdfm_str_utf8
        && cd.cmap_id < 0
    {
        dict = dpx.parse_pdf_object_extended(s, pp, parse_pdf_reference)?;
        if let Some(d) = dict
            && !dpx.o.is_dict(Some(d))
        {
            crate::warn!("Dictionary type object expected but non-dictionary type found.");
            dpx.o.release(d)?;
            dict = None;
        }
    } else {
        // :(
        if cd.unescape_backslash != 0 {
            dict = dpx.parse_pdf_tainted_dict_extended(s, pp, parse_pdf_reference)?;
        } else {
            dict = dpx.parse_pdf_object_extended(s, pp, parse_pdf_reference)?;
        }
        if let Some(d) = dict {
            if !dpx.o.is_dict(Some(d)) {
                crate::warn!("Dictionary type object expected but non-dictionary type found.");
                dpx.o.release(d)?;
                dict = None;
            } else {
                foreach_dict(dpx, d, |dpx, k, v| match v {
                    Some(v) => modify_strings(dpx, k, v, Some(cd)),
                    None => Ok(0),
                })?;
            }
        }
    }
    Ok(dict)
}

/// `parse_pdf_dict_with_tounicode(&args->curptr, args->endptr, &sd->cd)`.
fn parse_dict_tounicode(dpx: &mut Dpx, args: &mut SpcArg) -> Result<Option<Obj>> {
    let cd = dpx.pdfm.cd.clone();
    let (s, pp) = args.parts();
    parse_pdf_dict_with_tounicode(dpx, s, pp, &cd)
}

/// `set_rect_for_annot`.
fn set_rect_for_annot(dpx: &mut Dpx, spe: &mut SpcEnv, ti: TransformInfo) -> PdfRect {
    let cp = dpx.spc_get_current_point(spe);
    let (mut cp1, mut cp2, mut cp3, mut cp4);

    if ti.flags & INFO_HAS_USER_BBOX != 0 {
        cp1 = PdfCoord {
            x: cp.x + ti.bbox.llx,
            y: cp.y + ti.bbox.lly,
        };
        cp2 = PdfCoord {
            x: cp.x + ti.bbox.urx,
            y: cp.y + ti.bbox.lly,
        };
        cp3 = PdfCoord {
            x: cp.x + ti.bbox.urx,
            y: cp.y + ti.bbox.ury,
        };
        cp4 = PdfCoord {
            x: cp.x + ti.bbox.llx,
            y: cp.y + ti.bbox.ury,
        };
    } else {
        cp1 = PdfCoord {
            x: cp.x,
            y: cp.y - spe.mag * ti.depth,
        };
        cp2 = PdfCoord {
            x: cp.x + spe.mag * ti.width,
            y: cp.y - spe.mag * ti.depth,
        };
        cp3 = PdfCoord {
            x: cp.x + spe.mag * ti.width,
            y: cp.y + spe.mag * ti.height,
        };
        cp4 = PdfCoord {
            x: cp.x,
            y: cp.y + spe.mag * ti.height,
        };
    }
    dpx.pdf_dev_transform(&mut cp1, None);
    dpx.pdf_dev_transform(&mut cp2, None);
    dpx.pdf_dev_transform(&mut cp3, None);
    dpx.pdf_dev_transform(&mut cp4, None);
    PdfRect {
        llx: min4(cp1.x, cp2.x, cp3.x, cp4.x),
        lly: min4(cp1.y, cp2.y, cp3.y, cp4.y),
        urx: max4(cp1.x, cp2.x, cp3.x, cp4.x),
        ury: max4(cp1.y, cp2.y, cp3.y, cp4.y),
    }
}

/// `spc_handler_pdfm_stream_with_type` (`STRING_STREAM`/`FILE_STREAM`).
fn spc_handler_pdfm_stream_with_type(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    args: &mut SpcArg,
    ty: i32,
) -> Result<i32> {
    args.skip_white();

    let Some(ident) = ({
        let (s, pp) = args.parts();
        parse_opt_ident(s, pp)
    }) else {
        dpx.spc_warn(spe, format_args!("Missing objname for pdf:(f)stream."));
        return Ok(-1);
    };

    args.skip_white();

    let tmp = {
        let (s, pp) = args.parts();
        dpx.o.parse_pdf_object(s, pp, None)?
    };
    let Some(tmp) = tmp else {
        dpx.spc_warn(spe, format_args!("Missing input string for pdf:(f)stream."));
        return Ok(-1);
    };
    if !dpx.o.is_string(Some(tmp)) {
        dpx.spc_warn(
            spe,
            format_args!("Invalid type of input string for pdf:(f)stream."),
        );
        dpx.o.release(tmp)?;
        return Ok(-1);
    }

    let instring = dpx.o.string_value(tmp)?.to_vec();

    let fstream;
    match ty {
        FILE_STREAM => {
            // An empty string's value is C's NULL.
            if instring.is_empty() {
                dpx.spc_warn(spe, format_args!("Missing filename for pdf:fstream."));
                dpx.o.release(tmp)?;
                return Ok(-1);
            }
            let instring = cstr(&instring);
            let Some(fullname) = dpx
                .files
                .find(instring, crate::io::Format::Pict, b"dvipdfmx")
            else {
                dpx.spc_warn(spe, format_args!("File {:?} not found.", instring));
                dpx.o.release(tmp)?;
                return Ok(-1);
            };
            let Some(mut fp) = dpx.dpx_open_file(&fullname, crate::dpxfile::ResType::Binary)?
            else {
                dpx.spc_warn(spe, format_args!("Could not open file: {:?}", instring));
                dpx.o.release(tmp)?;
                return Ok(-1);
            };
            fstream = dpx.o.new_stream(STREAM_COMPRESS);
            let mut work_buffer = vec![0u8; crate::pdfdev::FORMAT_BUF_SIZE];
            loop {
                let nb_read = fp.read_into(&mut work_buffer);
                if nb_read == 0 {
                    break;
                }
                dpx.o.add_stream(fstream, &work_buffer[..nb_read])?;
            }
        }
        STRING_STREAM => {
            fstream = dpx.o.new_stream(STREAM_COMPRESS);
            dpx.o.add_stream(fstream, &instring)?;
        }
        _ => {
            dpx.o.release(tmp)?;
            return Ok(-1);
        }
    }
    dpx.o.release(tmp)?;

    // Optional dict.
    //
    //  TODO: check Length, Filter...
    args.skip_white();

    if args.cur() == b'<' {
        let stream_dict = dpx.o.stream_dict(fstream)?;

        let tmp = parse_ext(dpx, args)?;
        let Some(tmp) = tmp else {
            dpx.spc_warn(spe, format_args!("Parsing dictionary failed."));
            dpx.o.release(fstream)?;
            return Ok(-1);
        };
        if !dpx.o.is_dict(Some(tmp)) {
            dpx.spc_warn(
                spe,
                format_args!("Expecting dictionary type object but non-dictionary type found."),
            );
            dpx.o.release(fstream)?;
            dpx.o.release(tmp)?;
            return Ok(-1);
        }
        if dpx.o.lookup_dict(tmp, b"Length")?.is_some() {
            dpx.o.remove_dict(tmp, b"Length")?;
        } else if dpx.o.lookup_dict(tmp, b"Filter")?.is_some() {
            dpx.o.remove_dict(tmp, b"Filter")?;
        }
        dpx.o.merge_dict(stream_dict, tmp)?;
        dpx.o.release(tmp)?;
    }

    // Users should explicitly close this.
    dpx.spc_push_object(spe, &ident, fstream)?;

    Ok(0)
}

/// `spc_handler_pdfm_bop`.
fn spc_handler_pdfm_bop(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    if args.curptr < args.endptr {
        let content = args.rest().to_vec();
        dpx.pdf_doc_set_bop_content(&content)?;
    }
    args.curptr = args.endptr;
    Ok(0)
}

/// `spc_handler_pdfm_eop`.
fn spc_handler_pdfm_eop(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    if args.curptr < args.endptr {
        let content = args.rest().to_vec();
        dpx.pdf_doc_set_eop_content(&content)?;
    }
    args.curptr = args.endptr;
    Ok(0)
}

/// `spc_handler_pdfm_put`.
fn spc_handler_pdfm_put(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> Result<i32> {
    let mut error = 0;

    ap.skip_white();

    let Some(ident) = ({
        let (s, pp) = ap.parts();
        parse_opt_ident(s, pp)
    }) else {
        dpx.spc_warn(spe, format_args!("Missing object identifier."));
        return Ok(-1);
    };
    let Some(obj1) = dpx.spc_lookup_object(&ident)? else {
        dpx.spc_warn(spe, format_args!("Specified object not exist: {:?}", ident));
        return Ok(-1);
    };
    ap.skip_white();

    let Some(obj2) = parse_ext(dpx, ap)? else {
        dpx.spc_warn(
            spe,
            format_args!("Missing (an) object(s) to put into {:?}!", ident),
        );
        return Ok(-1);
    };

    match dpx.o.type_of(Some(obj1)) {
        PDF_DICT => {
            if dpx.o.type_of(Some(obj2)) != PDF_DICT {
                dpx.spc_warn(
                    spe,
                    format_args!(
                        "Inconsistent object type for \"put\" (expecting DICT): {:?}",
                        ident
                    ),
                );
                error = -1;
            } else if cstr(&ident) == b"resources" {
                error = foreach_dict(dpx, obj2, |dpx, k, v| {
                    safeputresdict(dpx, k, v.expect("vp"), obj1)
                })?;
            } else {
                dpx.o.merge_dict(obj1, obj2)?;
            }
        }
        PDF_STREAM => {
            if dpx.o.type_of(Some(obj2)) == PDF_DICT {
                let d = dpx.o.stream_dict(obj1)?;
                dpx.o.merge_dict(d, obj2)?;
            } else if dpx.o.type_of(Some(obj2)) == PDF_STREAM {
                dpx.spc_warn(
                    spe,
                    format_args!(
                        "\"put\" operation not supported for STREAM <- STREAM: {:?}",
                        ident
                    ),
                );
                error = -1;
            } else {
                dpx.spc_warn(
                    spe,
                    format_args!("Invalid type: expecting a DICT or STREAM: {:?}", ident),
                );
                error = -1;
            }
        }
        PDF_ARRAY => {
            // dvipdfm
            let l = dpx.o.link(obj2)?;
            dpx.o.add_array(obj1, l)?;
            while ap.curptr < ap.endptr {
                let Some(obj3) = parse_ext(dpx, ap)? else {
                    break;
                };
                dpx.o.add_array(obj1, obj3)?;
                ap.skip_white();
            }
        }
        _ => {
            dpx.spc_warn(
                spe,
                format_args!(
                    "Can't \"put\" object into non-DICT/STREAM/ARRAY type object: {:?}",
                    ident
                ),
            );
            error = -1;
        }
    }
    dpx.o.release(obj2)?;

    Ok(error)
}

/// `spc_handler_pdfm_annot`.
fn spc_handler_pdfm_annot(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let mut ident = None;

    args.skip_white();
    if args.cur() == b'@' {
        let (s, pp) = args.parts();
        ident = parse_opt_ident(s, pp);
        args.skip_white();
    }

    let mut ti = TransformInfo::default();
    ti.transform_info_clear();
    if dpx.spc_util_read_dimtrns(spe, &mut ti, args, 0) < 0 {
        return Ok(-1);
    }

    if ti.flags & INFO_HAS_USER_BBOX != 0
        && (ti.flags & INFO_HAS_WIDTH != 0 || ti.flags & INFO_HAS_HEIGHT != 0)
    {
        dpx.spc_warn(
            spe,
            format_args!("You can't specify both bbox and width/height."),
        );
        return Ok(-1);
    }

    let annot_dict = parse_dict_tounicode(dpx, args)?;
    let Some(annot_dict) = annot_dict else {
        dpx.spc_warn(spe, format_args!("Could not find dictionary object."));
        return Ok(-1);
    };
    if !dpx.o.is_dict(Some(annot_dict)) {
        dpx.spc_warn(spe, format_args!("Invalid type: not dictionary object."));
        dpx.o.release(annot_dict)?;
        return Ok(-1);
    }

    let rect = set_rect_for_annot(dpx, spe, ti);

    // Order is important...
    if let Some(ident) = &ident {
        let l = dpx.o.link(annot_dict)?;
        dpx.spc_push_object(spe, ident, l)?;
    }
    // Add this reference.
    let page = dpx.pdf_doc_current_page_number();
    dpx.pdf_doc_add_annot(page as u32, &rect, annot_dict, 1)?;

    dpx.o.release(annot_dict)?;

    Ok(0)
}

/// `spc_handler_pdfm_bann`.
///
/// NOTE: This can't have ident. See "Dvipdfm User's Manual".
/// 1 Jul. 2020: ident allowed (upon request)
/// Only first annotation can be accessed in line break cases.
fn spc_handler_pdfm_bann(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let mut ident = None;

    if dpx.pdfm.annot_dict.is_some() {
        dpx.spc_warn(
            spe,
            format_args!("Can't begin an annotation when one is pending."),
        );
        return Ok(-1);
    }

    args.skip_white();
    if args.cur() == b'@' {
        let (s, pp) = args.parts();
        ident = parse_opt_ident(s, pp);
        args.skip_white();
    }

    dpx.pdfm.annot_dict = parse_dict_tounicode(dpx, args)?;
    let Some(annot_dict) = dpx.pdfm.annot_dict else {
        dpx.spc_warn(
            spe,
            format_args!("Ignoring annotation with invalid dictionary."),
        );
        return Ok(-1);
    };
    if !dpx.o.is_dict(Some(annot_dict)) {
        dpx.spc_warn(spe, format_args!("Invalid type: not a dictionary object."));
        dpx.o.release(annot_dict)?;
        dpx.pdfm.annot_dict = None;
        return Ok(-1);
    }

    let l = dpx.o.link(annot_dict)?;
    let error = dpx.spc_begin_annot(spe, l);
    if let Some(ident) = &ident {
        let l = dpx.o.link(annot_dict)?;
        dpx.spc_push_object(spe, ident, l)?;
    }

    Ok(error)
}

/// `spc_handler_pdfm_eann`.
fn spc_handler_pdfm_eann(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let Some(annot_dict) = dpx.pdfm.annot_dict else {
        dpx.spc_warn(
            spe,
            format_args!("Tried to end an annotation without starting one!"),
        );
        return Ok(-1);
    };

    let error = dpx.spc_end_annot(spe)?;

    dpx.o.release(annot_dict)?;
    dpx.pdfm.annot_dict = None;

    Ok(error)
}

/// `spc_handler_pdfm_xann`: for supporting `\phantom` within bann-eann
/// (`pdf:xann width 50pt height 8pt depth 1pt` extends the current
/// annotation rectangle).
fn spc_handler_pdfm_xann(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    if !dpx.spc_is_tracking_boxes(spe) {
        // Silently ignore
        args.curptr = args.endptr;
        return Ok(0);
    }

    args.skip_white();

    let mut ti = TransformInfo::default();
    ti.transform_info_clear();
    if dpx.spc_util_read_dimtrns(spe, &mut ti, args, 0) < 0 {
        return Ok(-1);
    }

    if ti.flags & INFO_HAS_USER_BBOX != 0
        && (ti.flags & INFO_HAS_WIDTH != 0 || ti.flags & INFO_HAS_HEIGHT != 0)
    {
        dpx.spc_warn(
            spe,
            format_args!("You can't specify both bbox and width/height."),
        );
        return Ok(-1);
    }

    let rect = set_rect_for_annot(dpx, spe, ti);
    dpx.pdf_doc_expand_box(&rect);

    Ok(0)
}

/// The colors of `pdf:bcolor` and `pdf:scolor`: error, stroke, fill.
fn read_bcolor(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> Result<(i32, PdfColor, PdfColor)> {
    let mut error = 0;
    let mut fc = PdfColor::default();
    let mut sc = PdfColor::default();

    ap.skip_white();

    let (psc, pfc) = dpx.pdf_color_get_current();
    if ap.curptr < ap.endptr && (ap.cur() == b'f' || ap.cur() == b's') {
        sc.pdf_color_copycolor(&psc);
        fc.pdf_color_copycolor(&pfc);
        while error == 0 && ap.curptr < ap.endptr {
            if ap.rest().starts_with(b"fill") {
                ap.curptr += 4;
                ap.skip_white();
                error = dpx.spc_util_read_pdfcolor(spe, &mut fc, ap, Some(&pfc))?;
            } else if ap.rest().starts_with(b"stroke") {
                ap.curptr += 6;
                ap.skip_white();
                error = dpx.spc_util_read_pdfcolor(spe, &mut sc, ap, Some(&psc))?;
            } else {
                // Neither: C loops here forever.
                crate::fatal!("pdf:bcolor/scolor: \"fill\" or \"stroke\" expected");
            }
            ap.skip_white();
        }
    } else {
        error = dpx.spc_util_read_pdfcolor(spe, &mut fc, ap, Some(&pfc))?;
        if error == 0 {
            if ap.curptr < ap.endptr {
                error = dpx.spc_util_read_pdfcolor(spe, &mut sc, ap, Some(&psc))?;
            } else {
                sc.pdf_color_copycolor(&fc);
            }
        }
    }
    Ok((error, sc, fc))
}

/// `spc_handler_pdfm_bcolor`.
fn spc_handler_pdfm_bcolor(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> Result<i32> {
    let (error, sc, fc) = read_bcolor(dpx, spe, ap)?;
    if error != 0 {
        dpx.spc_warn(spe, format_args!("Invalid color specification?"));
    } else {
        ap.skip_white();
        dpx.pdf_color_push(&sc, &fc)?; // save currentcolor
    }
    Ok(error)
}

/// `spc_handler_pdfm_scolor`: changes the current color without
/// clearing the color stack (unlike "color rgb 1 0 0").
fn spc_handler_pdfm_scolor(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> Result<i32> {
    let (error, sc, fc) = read_bcolor(dpx, spe, ap)?;
    if error != 0 {
        dpx.spc_warn(spe, format_args!("Invalid color specification?"));
    } else {
        dpx.pdf_color_set(&sc, &fc)?;
    }
    Ok(error)
}

/// `spc_handler_pdfm_ecolor`.
fn spc_handler_pdfm_ecolor(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    dpx.pdf_color_pop()?;
    Ok(0)
}

/// `spc_handler_pdfm_btrans`.
fn spc_handler_pdfm_btrans(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let mut ti = TransformInfo::default();
    ti.transform_info_clear();
    if dpx.spc_util_read_dimtrns(spe, &mut ti, args, 0) < 0 {
        return Ok(-1);
    }

    // btrans inside bcontent-econtent bug fix.
    let cp = dpx.spc_get_current_point(spe);
    // Create transformation matrix
    let mut m = PdfTmatrix::default();
    pdf_copymatrix(&mut m, &ti.matrix);
    m.e += (1.0 - m.a) * cp.x - m.c * cp.y;
    m.f += (1.0 - m.d) * cp.y - m.b * cp.x;

    dpx.pdf_dev_gsave()?;
    dpx.pdf_dev_concat(&m)?;

    Ok(0)
}

/// `spc_handler_pdfm_etrans`.
fn spc_handler_pdfm_etrans(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    dpx.pdf_dev_grestore()?;

    // Unfortunately, the following line is necessary in case of a color
    // change inside of the save/restore pair.
    dpx.pdf_dev_reset_color(0)?;
    dpx.pdf_dev_reset_xgstate(0)?;

    Ok(0)
}

/// `spc_handler_pdfm_outline`.
fn spc_handler_pdfm_outline(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let mut is_open = -1;

    args.skip_white();

    // pdf:outline 1 ... (as DVIPDFM)
    // pdf:outline [] 1 ... (open bookmark)
    // pdf:outline [-] 1 ... (closed bookmark)
    if args.curptr + 3 < args.endptr && args.cur() == b'[' {
        args.curptr += 1;
        if args.cur() == b'-' {
            args.curptr += 1;
        } else {
            is_open = 1;
        }
        args.curptr += 1;
    }
    args.skip_white();

    let tmp = {
        let (s, pp) = args.parts();
        dpx.o.parse_pdf_object(s, pp, None)?
    };
    let Some(tmp) = tmp else {
        dpx.spc_warn(spe, format_args!("Missing number for outline item depth."));
        return Ok(-1);
    };
    if !dpx.o.is_number(Some(tmp)) {
        dpx.o.release(tmp)?;
        dpx.spc_warn(
            spe,
            format_args!("Expecting number for outline item depth."),
        );
        return Ok(-1);
    }

    let mut level = dpx.o.number_value(tmp)? as i32;
    dpx.o.release(tmp)?;

    // Make sure we know where the starting level is
    dpx.pdfm.lowest_level = dpx.pdfm.lowest_level.min(level);

    level += 1 - dpx.pdfm.lowest_level;

    let Some(item_dict) = parse_dict_tounicode(dpx, args)? else {
        dpx.spc_warn(spe, format_args!("Ignoring invalid dictionary."));
        return Ok(-1);
    };
    let mut current_depth = dpx.pdf_doc_bookmarks_depth();
    if current_depth > level {
        while current_depth > level {
            current_depth -= 1;
            dpx.pdf_doc_bookmarks_up();
        }
    } else if current_depth < level {
        while current_depth < level {
            current_depth += 1;
            dpx.pdf_doc_bookmarks_down()?;
        }
    }

    dpx.pdf_doc_bookmarks_add(item_dict, is_open)?;

    Ok(0)
}

/// `spc_handler_pdfm_article`.
fn spc_handler_pdfm_article(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();

    let Some(ident) = ({
        let (s, pp) = args.parts();
        parse_opt_ident(s, pp)
    }) else {
        dpx.spc_warn(spe, format_args!("Article name expected but not found."));
        return Ok(-1);
    };

    let Some(info_dict) = parse_dict_tounicode(dpx, args)? else {
        dpx.spc_warn(
            spe,
            format_args!("Ignoring article with invalid info dictionary."),
        );
        return Ok(-1);
    };

    let l = dpx.o.link(info_dict)?;
    dpx.pdf_doc_begin_article(cstr(&ident), Some(l))?;
    dpx.spc_push_object(spe, &ident, info_dict)?;

    Ok(0)
}

/// `spc_handler_pdfm_bead`.
fn spc_handler_pdfm_bead(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();

    if args.cur() != b'@' {
        dpx.spc_warn(
            spe,
            format_args!("Article identifier expected but not found."),
        );
        return Ok(-1);
    }

    let Some(article_name) = ({
        let (s, pp) = args.parts();
        parse_opt_ident(s, pp)
    }) else {
        dpx.spc_warn(
            spe,
            format_args!("Article reference expected but not found."),
        );
        return Ok(-1);
    };

    // If okay so far, try to get a bounding box
    let mut ti = TransformInfo::default();
    ti.transform_info_clear();
    if dpx.spc_util_read_dimtrns(spe, &mut ti, args, 0) < 0 {
        return Ok(-1);
    }

    if ti.flags & INFO_HAS_USER_BBOX != 0
        && (ti.flags & INFO_HAS_WIDTH != 0 || ti.flags & INFO_HAS_HEIGHT != 0)
    {
        dpx.spc_warn(
            spe,
            format_args!("You can't specify both bbox and width/height."),
        );
        return Ok(-1);
    }

    args.skip_white();
    let article_info = if args.cur() != b'<' {
        dpx.o.new_dict()
    } else {
        let Some(d) = parse_dict_tounicode(dpx, args)? else {
            dpx.spc_warn(spe, format_args!("Error in reading dictionary."));
            return Ok(-1);
        };
        d
    };

    // Does this article exist yet
    if let Some(article) = dpx.spc_lookup_object(&article_name)? {
        dpx.o.merge_dict(article, article_info)?;
        dpx.o.release(article_info)?;
    } else {
        let l = dpx.o.link(article_info)?;
        dpx.pdf_doc_begin_article(cstr(&article_name), Some(l))?;
        dpx.spc_push_object(spe, &article_name, article_info)?;
    }
    let page_no = dpx.pdf_doc_current_page_number();
    let rect = set_rect_for_annot(dpx, spe, ti);
    dpx.pdf_doc_add_bead(cstr(&article_name), None, page_no, &rect)?;

    Ok(0)
}

/// `spc_handler_pdfm_image`.
fn spc_handler_pdfm_image(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let mut ident = None;
    let mut options = LoadOptions {
        page_no: 1,
        bbox_type: 0,
        dict: None,
        page_name: None,
    };

    args.skip_white();
    if args.cur() == b'@' {
        let (s, pp) = args.parts();
        ident = parse_opt_ident(s, pp);
        args.skip_white();
    }

    // 2015/12/29
    // There should not be "page" and "pagebox" in read_dimtrns().
    let mut ti = TransformInfo::default();
    ti.transform_info_clear();
    if dpx.spc_util_read_blahblah(
        spe,
        &mut ti,
        &mut options.page_no,
        &mut options.bbox_type,
        &mut options.page_name,
        args,
    ) < 0
    {
        dpx.spc_warn(
            spe,
            format_args!("Reading option field in pdf:image failed."),
        );
        return Ok(-1);
    }

    args.skip_white();
    let fspec = {
        let (s, pp) = args.parts();
        dpx.o.parse_pdf_object(s, pp, None)?
    };
    let Some(fspec) = fspec else {
        dpx.spc_warn(spe, format_args!("Missing filename string for pdf:image."));
        return Ok(-1);
    };
    if !dpx.o.is_string(Some(fspec)) {
        dpx.spc_warn(spe, format_args!("Missing filename string for pdf:image."));
        dpx.o.release(fspec)?;
        return Ok(-1);
    }

    args.skip_white();
    if args.curptr < args.endptr {
        options.dict = parse_ext(dpx, args)?;
    }

    let filename = cstr(dpx.o.string_value(fspec)?).to_vec();
    let ident_c = ident.as_deref().map(cstr);
    let xobj_id = dpx.pdf_ximage_load_image(ident_c, &filename, options)?;

    if xobj_id < 0 {
        dpx.spc_warn(spe, format_args!("Could not find image resource..."));
        dpx.o.release(fspec)?;
        return Ok(-1);
    }

    if ti.flags & INFO_DO_HIDE == 0 {
        let (x, y) = (spe.x_user, spe.y_user);
        dpx.spc_put_image(spe, xobj_id, &mut ti, x, y)?;
    }

    if ident.is_some()
        && dpx.conf.compat_mode == crate::ctx::CompatMode::Compat
        && dpx.pdf_ximage_get_subtype(xobj_id)? == PDF_XOBJECT_TYPE_IMAGE
    {
        dpx.pdf_ximage_set_attr(xobj_id, 1, 1, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0)?;
    }

    dpx.o.release(fspec)?;

    Ok(0)
}

/// `spc_handler_pdfm_dest`.
fn spc_handler_pdfm_dest(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();

    let name = {
        let (s, pp) = args.parts();
        dpx.o.parse_pdf_object(s, pp, None)?
    };
    let Some(name) = name else {
        dpx.spc_warn(
            spe,
            format_args!("PDF string expected for destination name but not found."),
        );
        return Ok(-1);
    };
    if !dpx.o.is_string(Some(name)) {
        dpx.spc_warn(
            spe,
            format_args!("PDF string expected for destination name but invalid type."),
        );
        dpx.o.release(name)?;
        return Ok(-1);
    }

    let array = parse_ext(dpx, args)?;
    let Some(array) = array else {
        dpx.spc_warn(spe, format_args!("No destination specified for pdf:dest."));
        dpx.o.release(name)?;
        return Ok(-1);
    };
    if !dpx.o.is_array(Some(array)) {
        dpx.spc_warn(
            spe,
            format_args!("Destination not specified as an array object!"),
        );
        dpx.o.release(name)?;
        dpx.o.release(array)?;
        return Ok(-1);
    }

    let key = dpx.o.string_value(name)?.to_vec();
    dpx.pdf_doc_add_names(b"Dests", &key, array)?;
    dpx.o.release(name)?;

    Ok(0)
}

/// `spc_handler_pdfm_names`.
fn spc_handler_pdfm_names(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let category = {
        let (s, pp) = args.parts();
        dpx.o.parse_pdf_object(s, pp, None)?
    };
    let Some(category) = category else {
        dpx.spc_warn(spe, format_args!("PDF name expected but not found."));
        return Ok(-1);
    };
    if !dpx.o.is_name(Some(category)) {
        dpx.spc_warn(spe, format_args!("PDF name expected but not found."));
        dpx.o.release(category)?;
        return Ok(-1);
    }
    let cat = cstr(dpx.o.name_value(category)?).to_vec();

    let tmp = parse_ext(dpx, args)?;
    let Some(tmp) = tmp else {
        dpx.spc_warn(spe, format_args!("PDF object expected but not found."));
        dpx.o.release(category)?;
        return Ok(-1);
    };
    if dpx.o.is_array(Some(tmp)) {
        let size = dpx.o.array_length(tmp)?;
        if size % 2 != 0 {
            dpx.spc_warn(
                spe,
                format_args!("Array size not multiple of 2 for pdf:names."),
            );
            dpx.o.release(category)?;
            dpx.o.release(tmp)?;
            return Ok(-1);
        }

        for i in 0..size / 2 {
            let key = dpx.o.get_array(tmp, (2 * i) as i32)?;
            let value = dpx.o.get_array(tmp, (2 * i + 1) as i32)?;
            if !dpx.o.is_string(key) {
                dpx.spc_warn(spe, format_args!("Name tree key must be string."));
                dpx.o.release(category)?;
                dpx.o.release(tmp)?;
                return Ok(-1);
            }
            let k = dpx.o.string_value(key.expect("key"))?.to_vec();
            let v = dpx.o.link(value.expect("value"))?;
            if dpx.pdf_doc_add_names(&cat, &k, v)? < 0 {
                dpx.spc_warn(spe, format_args!("Failed to add Name tree entry..."));
                dpx.o.release(category)?;
                dpx.o.release(tmp)?;
                return Ok(-1);
            }
        }
        dpx.o.release(tmp)?;
    } else if dpx.o.is_string(Some(tmp)) {
        let key = tmp;
        let Some(value) = parse_ext(dpx, args)? else {
            dpx.o.release(category)?;
            dpx.o.release(key)?;
            dpx.spc_warn(spe, format_args!("PDF object expected but not found."));
            return Ok(-1);
        };
        let k = dpx.o.string_value(key)?.to_vec();
        if dpx.pdf_doc_add_names(&cat, &k, value)? < 0 {
            dpx.spc_warn(spe, format_args!("Failed to add Name tree entry..."));
            dpx.o.release(category)?;
            dpx.o.release(key)?;
            return Ok(-1);
        }
        dpx.o.release(key)?;
    } else {
        dpx.o.release(tmp)?;
        dpx.o.release(category)?;
        dpx.spc_warn(spe, format_args!("Invalid object type for pdf:names."));
        return Ok(-1);
    }
    dpx.o.release(category)?;

    Ok(0)
}

/// `spc_handler_pdfm_docinfo`.
fn spc_handler_pdfm_docinfo(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let Some(dict) = parse_dict_tounicode(dpx, args)? else {
        dpx.spc_warn(
            spe,
            format_args!("Dictionary object expected but not found."),
        );
        return Ok(-1);
    };

    let docinfo = dpx.pdf_doc_docinfo()?;
    dpx.o.merge_dict(docinfo, dict)?;
    dpx.o.release(dict)?;

    Ok(0)
}

/// `spc_handler_pdfm_docview`.
fn spc_handler_pdfm_docview(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let Some(dict) = parse_dict_tounicode(dpx, args)? else {
        dpx.spc_warn(
            spe,
            format_args!("Dictionary object expected but not found."),
        );
        return Ok(-1);
    };

    let catalog = dpx.pdf_doc_catalog()?;
    // Avoid overriding whole ViewerPreferences
    let pref_old = dpx.o.lookup_dict(catalog, b"ViewerPreferences")?;
    let pref_add = dpx.o.lookup_dict(dict, b"ViewerPreferences")?;
    if let (Some(pref_old), Some(pref_add)) = (pref_old, pref_add) {
        dpx.o.merge_dict(pref_old, pref_add)?;
        dpx.o.remove_dict(dict, b"ViewerPreferences")?;
    }
    dpx.o.merge_dict(catalog, dict)?;
    dpx.o.release(dict)?;

    Ok(0)
}

/// `spc_handler_pdfm_close`.
fn spc_handler_pdfm_close(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();
    let ident = {
        let (s, pp) = args.parts();
        parse_opt_ident(s, pp)
    };
    if let Some(ident) = ident {
        dpx.spc_flush_object(spe, &ident)?;
    } else {
        // Close all?
        dpx.spc_warn(
            spe,
            format_args!("pdf:close without an argument no longer supported!"),
        );
        dpx.spc_clear_objects(spe);
    }

    Ok(0)
}

/// `spc_handler_pdfm_object`.
fn spc_handler_pdfm_object(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();
    let Some(ident) = ({
        let (s, pp) = args.parts();
        parse_opt_ident(s, pp)
    }) else {
        dpx.spc_warn(spe, format_args!("Could not find a object identifier."));
        return Ok(-1);
    };

    let Some(object) = parse_ext(dpx, args)? else {
        dpx.spc_warn(
            spe,
            format_args!("Could not find an object definition for {:?}.", ident),
        );
        return Ok(-1);
    };
    dpx.spc_push_object(spe, &ident, object)?;

    Ok(0)
}

/// `spc_handler_pdfm_content`.
fn spc_handler_pdfm_content(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();
    if args.curptr < args.endptr {
        let cp = dpx.spc_get_current_point(spe);
        let mut m = PdfTmatrix::default();
        pdf_setmatrix(&mut m, 1.0, 0.0, 0.0, 1.0, cp.x, cp.y);
        let mut work_buffer = Buf::new();
        work_buffer.extend(b" q ");
        dpx.pdf_sprint_matrix(&mut work_buffer, &m);
        work_buffer.extend(b" cm ");

        dpx.pdf_doc_add_page_content(work_buffer.as_bytes())?; // op: q cm
        let content = args.rest().to_vec();
        dpx.pdf_doc_add_page_content(&content)?; // op: ANY
        dpx.pdf_doc_add_page_content(b" Q")?; // op: Q
    }
    args.curptr = args.endptr;

    Ok(0)
}

/// `spc_handler_pdfm_literal`.
fn spc_handler_pdfm_literal(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let mut direct = false;

    args.skip_white();
    while args.curptr < args.endptr {
        if args.rest().starts_with(b"reverse") {
            args.curptr += 7;
            crate::warn!(
                "The special \"pdf:literal reverse ...\" is no longer supported.\nIgnore the \"reverse\" option."
            );
        } else if args.rest().starts_with(b"direct") {
            direct = true;
            args.curptr += 6;
        } else {
            break;
        }
        args.skip_white();
    }

    if args.curptr < args.endptr {
        let cp = dpx.spc_get_current_point(spe);
        let mut m = PdfTmatrix::default();
        if !direct {
            m.a = 1.0;
            m.d = 1.0;
            m.b = 0.0;
            m.c = 0.0;
            m.e = cp.x;
            m.f = cp.y;
            dpx.pdf_dev_concat(&m)?;
        }
        dpx.pdf_doc_add_page_content(b" ")?; // op:
        let content = args.rest().to_vec();
        dpx.pdf_doc_add_page_content(&content)?; // op: ANY
        if !direct {
            m.e = -cp.x;
            m.f = -cp.y;
            dpx.pdf_dev_concat(&m)?;
        }
    }

    args.curptr = args.endptr;

    Ok(0)
}

/// `spc_handler_pdfm_bcontent`.
fn spc_handler_pdfm_bcontent(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    dpx.pdf_dev_gsave()?;
    let (xpos, ypos) = dpx.spc_get_coord(spe);
    let mut m = PdfTmatrix::default();
    pdf_setmatrix(
        &mut m,
        1.0,
        0.0,
        0.0,
        1.0,
        spe.x_user - xpos,
        spe.y_user - ypos,
    );
    dpx.pdf_dev_concat(&m)?;
    let (x, y) = (spe.x_user, spe.y_user);
    dpx.spc_push_coord(spe, x, y);

    Ok(0)
}

/// `spc_handler_pdfm_econtent`.
fn spc_handler_pdfm_econtent(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    dpx.spc_pop_coord(spe);
    dpx.pdf_dev_grestore()?;
    dpx.pdf_dev_reset_color(0)?;
    dpx.pdf_dev_reset_xgstate(0)?;

    Ok(0)
}

/// `spc_handler_pdfm_code`.
fn spc_handler_pdfm_code(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();

    if args.curptr < args.endptr {
        dpx.pdf_doc_add_page_content(b" ")?; // op:
        let content = args.rest().to_vec();
        dpx.pdf_doc_add_page_content(&content)?; // op: ANY
        args.curptr = args.endptr;
    }

    Ok(0)
}

/// `spc_handler_pdfm_do_nothing`.
fn spc_handler_pdfm_do_nothing(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.curptr = args.endptr;
    Ok(0)
}

/// `spc_handler_pdfm_stream`: `pdf: stream @objname (input_string)
/// [PDF_DICT]`.
fn spc_handler_pdfm_stream(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    spc_handler_pdfm_stream_with_type(dpx, spe, args, STRING_STREAM)
}

/// `spc_handler_pdfm_fstream`: `pdf: fstream @objname (filename)
/// [PDF_DICT]`.
fn spc_handler_pdfm_fstream(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    spc_handler_pdfm_stream_with_type(dpx, spe, args, FILE_STREAM)
}

/// `spc_handler_pdfm_bform`: grabs page content, the box from the
/// dimensions (`width`/`height`/`depth`) or `bbox` around (x_user,
/// y_user).
fn spc_handler_pdfm_bform(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();

    let Some(ident) = ({
        let (s, pp) = args.parts();
        parse_opt_ident(s, pp)
    }) else {
        dpx.spc_warn(spe, format_args!("A form XObject must have name."));
        return Ok(-1);
    };

    let mut ti = TransformInfo::default();
    ti.transform_info_clear();
    if dpx.spc_util_read_dimtrns(spe, &mut ti, args, 0) < 0 {
        return Ok(-1);
    }

    // A XForm with zero dimension results in a non-invertible
    // transformation matrix.
    let cropbox;
    if ti.flags & INFO_HAS_USER_BBOX != 0 {
        if ti.bbox.urx - ti.bbox.llx == 0.0 || ti.bbox.ury - ti.bbox.lly == 0.0 {
            dpx.spc_warn(spe, format_args!("Bounding box has a zero dimension."));
            return Ok(-1);
        }
        cropbox = PdfRect {
            llx: ti.bbox.llx,
            lly: ti.bbox.lly,
            urx: ti.bbox.urx,
            ury: ti.bbox.ury,
        };
    } else {
        if ti.width == 0.0 || ti.depth + ti.height == 0.0 {
            dpx.spc_warn(spe, format_args!("Bounding box has a zero dimension."));
            return Ok(-1);
        }
        cropbox = PdfRect {
            llx: 0.0,
            lly: -ti.depth,
            urx: ti.width,
            ury: ti.height,
        };
    }

    let cp = dpx.spc_get_current_point(spe);
    let error = dpx.spc_begin_form(spe, cstr(&ident), cp, &cropbox)?;

    if error != 0 {
        dpx.spc_warn(spe, format_args!("Couldn't start form object."));
    }

    Ok(error)
}

/// `spc_handler_pdfm_eform`: an extra dictionary after exobj is merged
/// into the form dictionary.
fn spc_handler_pdfm_eform(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let mut attrib = None;

    args.skip_white();

    if args.curptr < args.endptr {
        attrib = parse_ext(dpx, args)?;
        if let Some(a) = attrib
            && !dpx.o.is_dict(Some(a))
        {
            dpx.o.release(a)?;
            attrib = None;
        }
    }
    // pageresources here too
    if let Some(pr) = dpx.pdfm.pageresources {
        foreach_dict(dpx, pr, |dpx, k, v| {
            forallresourcecategory(dpx, k, v.expect("vp"))
        })?;
    }
    dpx.spc_end_form(spe, attrib)
}

/// `spc_handler_pdfm_uxobj`: uses a saved XObject (scaled to the given
/// dimensions or `bbox`).
fn spc_handler_pdfm_uxobj(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();

    let Some(ident) = ({
        let (s, pp) = args.parts();
        parse_opt_ident(s, pp)
    }) else {
        dpx.spc_warn(spe, format_args!("No object identifier given."));
        return Ok(-1);
    };
    let ident = cstr(&ident);

    let mut ti = TransformInfo::default();
    ti.transform_info_clear();
    if args.curptr < args.endptr && dpx.spc_util_read_dimtrns(spe, &mut ti, args, 0) < 0 {
        return Ok(-1);
    }

    let mut xobj_id = dpx.pdf_ximage_findresource(ident);
    if xobj_id < 0 {
        xobj_id = dpx.pdf_ximage_reserve(ident)?;
    }

    let (x, y) = (spe.x_user, spe.y_user);
    dpx.spc_put_image(spe, xobj_id, &mut ti, x, y)?;

    Ok(0)
}

/// `spc_handler_pdfm_link`.
fn spc_handler_pdfm_link(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    Ok(dpx.spc_resume_annot(spe))
}

/// `spc_handler_pdfm_nolink`.
fn spc_handler_pdfm_nolink(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    Ok(dpx.spc_suspend_annot(spe))
}

/// `spc_handler_pdfm_pagesize`: handled at BOP (dvi.c).
fn spc_handler_pdfm_pagesize(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.curptr = args.endptr;
    Ok(0)
}

/// `spc_handler_pdfm_bgcolor`.
fn spc_handler_pdfm_bgcolor(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let mut colorspec = PdfColor::default();

    let error = dpx.spc_util_read_pdfcolor(spe, &mut colorspec, args, None)?;
    if error != 0 {
        dpx.spc_warn(spe, format_args!("No valid color specified?"));
    } else {
        dpx.pdf_doc_set_bgcolor(Some(&colorspec));
    }

    Ok(error)
}

/// The fontmap line of `pdf:mapline` and `x:fontmapline` (C has the same
/// code twice): the error is reported, the special's status is 0.
pub(crate) fn spc_fontmapline(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
    what: &str,
) -> Result<i32> {
    let mut error = 0;

    ap.skip_white();
    if ap.curptr >= ap.endptr {
        dpx.spc_warn(spe, format_args!("Empty {} special?", what));
        return Ok(-1);
    }

    let opchr = ap.cur();
    if opchr == b'-' || opchr == b'+' {
        ap.curptr += 1;
    }

    ap.skip_white();

    if opchr == b'-' {
        let map_name = {
            let (s, pp) = ap.parts();
            parse_ident(s, pp)
        };
        if let Some(map_name) = map_name {
            dpx.pdf_remove_fontmap_record(cstr(&map_name));
        } else {
            dpx.spc_warn(spe, format_args!("Invalid fontmap line: Missing TFM name."));
            error = -1;
        }
    } else {
        let rest = ap.rest();
        if rest.len() >= THEBUFFLENGTH - 1 {
            dpx.spc_warn(spe, format_args!("Invalid fontmap line: Too long a line."));
            return Ok(-1);
        }
        let buffer = rest.to_vec();
        let mut mrec = FontmapRec::default();
        pdf_init_fontmap_record(&mut mrec);
        error = pdf_read_fontmap_line(&mut mrec, &buffer, is_pdfm_mapline(cstr(&buffer)))?;
        if error != 0 {
            dpx.spc_warn(spe, format_args!("Invalid fontmap line."));
        } else {
            let map_name = mrec.map_name.clone().unwrap_or_default();
            if opchr == b'+' {
                dpx.pdf_append_fontmap_record(&map_name, &mrec);
            } else {
                dpx.pdf_insert_fontmap_record(&map_name, &mrec);
            }
        }
        pdf_clear_fontmap_record(&mut mrec);
    }
    if error == 0 {
        ap.curptr = ap.endptr;
    }

    Ok(0)
}

/// `spc_handler_pdfm_mapline`.
fn spc_handler_pdfm_mapline(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> Result<i32> {
    spc_fontmapline(dpx, spe, ap, "mapline")
}

/// The fontmap file of `pdf:mapfile` and `x:fontmapfile`.
pub(crate) fn spc_fontmapfile(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();
    if args.curptr >= args.endptr {
        return Ok(0);
    }

    let mode = match args.cur() {
        b'-' => {
            args.curptr += 1;
            FONTMAP_RMODE_REMOVE
        }
        b'+' => {
            args.curptr += 1;
            FONTMAP_RMODE_APPEND
        }
        _ => FONTMAP_RMODE_REPLACE,
    };

    let mapfile = {
        let (s, pp) = args.parts();
        parse_val_ident(s, pp)
    };
    match mapfile {
        None => {
            dpx.spc_warn(spe, format_args!("No fontmap file specified."));
            Ok(-1)
        }
        Some(mapfile) => dpx.pdf_load_fontmap_file(cstr(&mapfile), mode),
    }
}

/// `spc_handler_pdfm_mapfile`.
fn spc_handler_pdfm_mapfile(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    spc_fontmapfile(dpx, spe, args)
}

/// `spc_handler_pdfm_tounicode`.
fn spc_handler_pdfm_tounicode(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    // First clear
    dpx.pdfm.cd.cmap_id = -1;
    dpx.pdfm.cd.unescape_backslash = 0;

    args.skip_white();
    if args.curptr >= args.endptr {
        dpx.spc_warn(spe, format_args!("Missing CMap name for pdf:tounicode."));
        return Ok(-1);
    }

    // _FIXME_
    // Any valid char allowed for PDF name object should be allowed here.
    let cmap_name = {
        let (s, pp) = args.parts();
        parse_ident(s, pp)
    };
    let Some(cmap_name) = cmap_name else {
        dpx.spc_warn(spe, format_args!("Missing ToUnicode mapping name..."));
        return Ok(-1);
    };
    let cmap_name = cstr(&cmap_name);

    dpx.pdfm.cd.cmap_id = dpx.CMap_cache_find(cmap_name)?;
    if dpx.pdfm.cd.cmap_id < 0 {
        dpx.spc_warn(
            spe,
            format_args!("Failed to load ToUnicode mapping: {:?}", cmap_name),
        );
        return Ok(-1);
    }

    // Shift-JIS like encoding may contain backslash in 2nd byte.
    // WARNING: This will add nasty extension to PDF parser.
    if dpx.pdfm.cd.cmap_id >= 0
        && (contains(cmap_name, b"RKSJ")
            || contains(cmap_name, b"B5")
            || contains(cmap_name, b"GBK")
            || contains(cmap_name, b"KSC"))
    {
        dpx.pdfm.cd.unescape_backslash = 1;
    }

    // Additional "taint key"
    // An array of PDF name objects can be supplied optionally.
    args.skip_white();
    if args.curptr < args.endptr {
        let taint_keys = {
            let (s, pp) = args.parts();
            dpx.o.parse_pdf_object(s, pp, None)?
        };
        if let Some(taint_keys) = taint_keys {
            if dpx.o.is_array(Some(taint_keys)) {
                for i in 0..dpx.o.array_length(taint_keys)? {
                    let key = dpx.o.get_array(taint_keys, i as i32)?;
                    if dpx.o.is_name(key) {
                        let l = dpx.o.link(key.expect("key"))?;
                        let tk = dpx.pdfm.cd.taintkeys.expect("taintkeys");
                        dpx.o.add_array(tk, l)?;
                    } else {
                        dpx.spc_warn(
                            spe,
                            format_args!("Invalid argument specified in pdf:tounicode special."),
                        );
                    }
                }
            } else {
                dpx.spc_warn(
                    spe,
                    format_args!("Invalid argument specified in pdf:unicode special."),
                );
            }
            dpx.o.release(taint_keys)?;
        }
    }

    Ok(0)
}

/// `spc_handler_pdfm_pageresources`.
fn spc_handler_pdfm_pageresources(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    args: &mut SpcArg,
) -> Result<i32> {
    let Some(dict) = parse_ext(dpx, args)? else {
        dpx.spc_warn(
            spe,
            format_args!("Dictionary object expected but not found."),
        );
        return Ok(-1);
    };

    if let Some(old) = dpx.pdfm.pageresources {
        dpx.o.release(old)?;
    }
    dpx.pdfm.pageresources = Some(dict);

    Ok(0)
}

/// `spc_handler_pdfm_bxgstate`.
fn spc_handler_pdfm_bxgstate(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();
    let Some(obj) = parse_ext(dpx, args)? else {
        dpx.spc_warn(spe, format_args!("Could not find an object definition."));
        return Ok(-1);
    };
    if !dpx.o.is_dict(Some(obj)) {
        dpx.spc_warn(
            spe,
            format_args!("Parsed object for ExtGState not a dictionary object!"),
        );
        dpx.o.release(obj)?;
        return Ok(-1);
    }
    dpx.pdf_dev_xgstate_push(obj)?;

    args.skip_white();

    Ok(0)
}

/// `spc_handler_pdfm_exgstate`.
fn spc_handler_pdfm_exgstate(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    dpx.pdf_dev_xgstate_pop()?;
    args.skip_white();
    Ok(0)
}

/// `spc_handler_pdft_compat_page`.
fn spc_handler_pdft_compat_page(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();
    if args.curptr < args.endptr {
        dpx.pdf_doc_add_page_content(b" ")?; // op:
        let content = args.rest().to_vec();
        dpx.pdf_doc_add_page_content(&content)?; // op: ANY
    }

    args.curptr = args.endptr;

    Ok(0)
}
/// `pdfm_handlers`, in C's order (the first match wins).
pub static PDFM_HANDLERS: [SpcHandler; 88] = [
    SpcHandler {
        key: b"annotation",
        exec: spc_handler_pdfm_annot,
    },
    SpcHandler {
        key: b"annotate",
        exec: spc_handler_pdfm_annot,
    },
    SpcHandler {
        key: b"annot",
        exec: spc_handler_pdfm_annot,
    },
    SpcHandler {
        key: b"ann",
        exec: spc_handler_pdfm_annot,
    },
    SpcHandler {
        key: b"outline",
        exec: spc_handler_pdfm_outline,
    },
    SpcHandler {
        key: b"out",
        exec: spc_handler_pdfm_outline,
    },
    SpcHandler {
        key: b"article",
        exec: spc_handler_pdfm_article,
    },
    SpcHandler {
        key: b"art",
        exec: spc_handler_pdfm_article,
    },
    SpcHandler {
        key: b"bead",
        exec: spc_handler_pdfm_bead,
    },
    SpcHandler {
        key: b"thread",
        exec: spc_handler_pdfm_bead,
    },
    SpcHandler {
        key: b"destination",
        exec: spc_handler_pdfm_dest,
    },
    SpcHandler {
        key: b"dest",
        exec: spc_handler_pdfm_dest,
    },
    SpcHandler {
        key: b"object",
        exec: spc_handler_pdfm_object,
    },
    SpcHandler {
        key: b"obj",
        exec: spc_handler_pdfm_object,
    },
    SpcHandler {
        key: b"docinfo",
        exec: spc_handler_pdfm_docinfo,
    },
    SpcHandler {
        key: b"docview",
        exec: spc_handler_pdfm_docview,
    },
    SpcHandler {
        key: b"content",
        exec: spc_handler_pdfm_content,
    },
    SpcHandler {
        key: b"put",
        exec: spc_handler_pdfm_put,
    },
    SpcHandler {
        key: b"close",
        exec: spc_handler_pdfm_close,
    },
    SpcHandler {
        key: b"bop",
        exec: spc_handler_pdfm_bop,
    },
    SpcHandler {
        key: b"eop",
        exec: spc_handler_pdfm_eop,
    },
    SpcHandler {
        key: b"image",
        exec: spc_handler_pdfm_image,
    },
    SpcHandler {
        key: b"img",
        exec: spc_handler_pdfm_image,
    },
    SpcHandler {
        key: b"epdf",
        exec: spc_handler_pdfm_image,
    },
    SpcHandler {
        key: b"link",
        exec: spc_handler_pdfm_link,
    },
    SpcHandler {
        key: b"nolink",
        exec: spc_handler_pdfm_nolink,
    },
    SpcHandler {
        key: b"begincolor",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"bcolor",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"bc",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"setcolor",
        exec: spc_handler_pdfm_scolor,
    },
    SpcHandler {
        key: b"scolor",
        exec: spc_handler_pdfm_scolor,
    },
    SpcHandler {
        key: b"sc",
        exec: spc_handler_pdfm_scolor,
    },
    SpcHandler {
        key: b"endcolor",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"ecolor",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"ec",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"begingray",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"bgray",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"bg",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"endgray",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"egray",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"eg",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"bgcolor",
        exec: spc_handler_pdfm_bgcolor,
    },
    SpcHandler {
        key: b"bgc",
        exec: spc_handler_pdfm_bgcolor,
    },
    SpcHandler {
        key: b"bbc",
        exec: spc_handler_pdfm_bgcolor,
    },
    SpcHandler {
        key: b"bbg",
        exec: spc_handler_pdfm_bgcolor,
    },
    SpcHandler {
        key: b"pagesize",
        exec: spc_handler_pdfm_pagesize,
    },
    SpcHandler {
        key: b"bannot",
        exec: spc_handler_pdfm_bann,
    },
    SpcHandler {
        key: b"beginann",
        exec: spc_handler_pdfm_bann,
    },
    SpcHandler {
        key: b"bann",
        exec: spc_handler_pdfm_bann,
    },
    SpcHandler {
        key: b"eannot",
        exec: spc_handler_pdfm_eann,
    },
    SpcHandler {
        key: b"endann",
        exec: spc_handler_pdfm_eann,
    },
    SpcHandler {
        key: b"eann",
        exec: spc_handler_pdfm_eann,
    },
    SpcHandler {
        key: b"btrans",
        exec: spc_handler_pdfm_btrans,
    },
    SpcHandler {
        key: b"begintransform",
        exec: spc_handler_pdfm_btrans,
    },
    SpcHandler {
        key: b"begintrans",
        exec: spc_handler_pdfm_btrans,
    },
    SpcHandler {
        key: b"bt",
        exec: spc_handler_pdfm_btrans,
    },
    SpcHandler {
        key: b"etrans",
        exec: spc_handler_pdfm_etrans,
    },
    SpcHandler {
        key: b"endtransform",
        exec: spc_handler_pdfm_etrans,
    },
    SpcHandler {
        key: b"endtrans",
        exec: spc_handler_pdfm_etrans,
    },
    SpcHandler {
        key: b"et",
        exec: spc_handler_pdfm_etrans,
    },
    SpcHandler {
        key: b"bform",
        exec: spc_handler_pdfm_bform,
    },
    SpcHandler {
        key: b"beginxobj",
        exec: spc_handler_pdfm_bform,
    },
    SpcHandler {
        key: b"bxobj",
        exec: spc_handler_pdfm_bform,
    },
    SpcHandler {
        key: b"eform",
        exec: spc_handler_pdfm_eform,
    },
    SpcHandler {
        key: b"endxobj",
        exec: spc_handler_pdfm_eform,
    },
    SpcHandler {
        key: b"exobj",
        exec: spc_handler_pdfm_eform,
    },
    SpcHandler {
        key: b"usexobj",
        exec: spc_handler_pdfm_uxobj,
    },
    SpcHandler {
        key: b"uxobj",
        exec: spc_handler_pdfm_uxobj,
    },
    SpcHandler {
        key: b"tounicode",
        exec: spc_handler_pdfm_tounicode,
    },
    SpcHandler {
        key: b"literal",
        exec: spc_handler_pdfm_literal,
    },
    SpcHandler {
        key: b"stream",
        exec: spc_handler_pdfm_stream,
    },
    SpcHandler {
        key: b"fstream",
        exec: spc_handler_pdfm_fstream,
    },
    SpcHandler {
        key: b"names",
        exec: spc_handler_pdfm_names,
    },
    SpcHandler {
        key: b"mapline",
        exec: spc_handler_pdfm_mapline,
    },
    SpcHandler {
        key: b"mapfile",
        exec: spc_handler_pdfm_mapfile,
    },
    SpcHandler {
        key: b"bcontent",
        exec: spc_handler_pdfm_bcontent,
    },
    SpcHandler {
        key: b"econtent",
        exec: spc_handler_pdfm_econtent,
    },
    SpcHandler {
        key: b"code",
        exec: spc_handler_pdfm_code,
    },
    SpcHandler {
        key: b"minorversion",
        exec: spc_handler_pdfm_do_nothing,
    },
    SpcHandler {
        key: b"majorversion",
        exec: spc_handler_pdfm_do_nothing,
    },
    SpcHandler {
        key: b"encrypt",
        exec: spc_handler_pdfm_do_nothing,
    },
    SpcHandler {
        key: b"pageresources",
        exec: spc_handler_pdfm_pageresources,
    },
    SpcHandler {
        key: b"trailerid",
        exec: spc_handler_pdfm_do_nothing,
    },
    SpcHandler {
        key: b"xannot",
        exec: spc_handler_pdfm_xann,
    },
    SpcHandler {
        key: b"extendann",
        exec: spc_handler_pdfm_xann,
    },
    SpcHandler {
        key: b"xann",
        exec: spc_handler_pdfm_xann,
    },
    SpcHandler {
        key: b"bxgstate",
        exec: spc_handler_pdfm_bxgstate,
    },
    SpcHandler {
        key: b"exgstate",
        exec: spc_handler_pdfm_exgstate,
    },
];

/// `pdft_compat_handlers` (`pdf:direct`, `pdf:page`).
pub static PDFT_COMPAT_HANDLERS: [SpcHandler; 2] = [
    SpcHandler {
        key: b"direct",
        exec: spc_handler_pdft_compat_page,
    },
    SpcHandler {
        key: b"page",
        exec: spc_handler_pdft_compat_page,
    },
];

/// `spc_pdfm_check_special`.
pub fn spc_pdfm_check_special(buf: &[u8]) -> bool {
    let mut p = 0;
    skip_white(buf, &mut p);
    buf[p..].starts_with(b"pdf:")
}

/// `spc_pdfm_setup_handler`.
pub fn spc_pdfm_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> Result<i32> {
    let mut error = -1;

    ap.skip_white();
    if ap.curptr + 4 >= ap.endptr || !ap.rest().starts_with(b"pdf:") {
        dpx.spc_warn(spe, format_args!("Not pdf: special???"));
        return Ok(-1);
    }
    ap.curptr += 4;

    ap.skip_white();
    let q = {
        let (s, pp) = ap.parts();
        parse_c_ident(s, pp)
    };
    if let Some(q) = q {
        let q = cstr(&q);
        let mut is_pdft_compat = false;
        if ap.curptr < ap.endptr && ap.cur() == b':' {
            is_pdft_compat = true;
            ap.curptr += 1;
        }
        let table: &'static [SpcHandler] = if is_pdft_compat {
            &PDFT_COMPAT_HANDLERS
        } else {
            &PDFM_HANDLERS
        };
        for h in table {
            if q == h.key {
                ap.command = Some(h.key);
                sph.key = b"pdf:";
                sph.exec = h.exec;
                ap.skip_white();
                error = 0;
                break;
            }
        }
    }

    Ok(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_size() {
        assert_eq!(calculate_size_utf16(b"").unwrap(), 0);
        assert_eq!(calculate_size_utf16(b"abc").unwrap(), 6);
        assert_eq!(calculate_size_utf16("é€".as_bytes()).unwrap(), 4);
        assert_eq!(calculate_size_utf16("a😀".as_bytes()).unwrap(), 6);
    }

    #[test]
    fn check() {
        assert!(spc_pdfm_check_special(b"  pdf:literal 0 g"));
        assert!(spc_pdfm_check_special(b"pdf:"));
        assert!(!spc_pdfm_check_special(b"pdf"));
        assert!(!spc_pdfm_check_special(b"x:gsave"));
    }

    #[test]
    fn strstr() {
        assert!(contains(b"90ms-RKSJ-H", b"RKSJ"));
        assert!(!contains(b"UniJIS-UTF16-H", b"B5"));
    }
}
