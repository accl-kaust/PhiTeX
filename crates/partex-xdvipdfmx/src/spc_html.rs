//! spc_html.c, spc_html.h: `html:` specials (hypertex anchors, `<img>`,
//! `<base>`).
//!
//! C's single `struct spc_html_ _html_state` is this module's `State`
//! (`self.html`); the C functions' `sd` parameter is dropped.
//! Attribute values are PDF strings holding C's trailing NUL, as in C;
//! they are read back as C strings ([`cstr`]).

use crate::dpxutil::{parse_c_ident, parse_float_decimal};
use crate::fmt::is_c_space;
use crate::pdfdev::{INFO_HAS_HEIGHT, INFO_HAS_WIDTH, PdfCoord, PdfTmatrix, TransformInfo};
use crate::pdfdraw::{pdf_concatmatrix, pdf_setmatrix};
use crate::pdfximage::LoadOptions;
use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler, cstr};

/// `ENABLE_HTML_IMG_SUPPORT` etc. are all on.
pub const ANCHOR_TYPE_HREF: i32 = 0;
pub const ANCHOR_TYPE_NAME: i32 = 1;

/// `HTML_TAG_NAME_MAX`.
pub const HTML_TAG_NAME_MAX: usize = 127;
/// `HTML_TAG_TYPE_EMPTY` (the same as `HTML_TAG_TYPE_OPEN`, as in C).
pub const HTML_TAG_TYPE_EMPTY: i32 = 1;
/// `HTML_TAG_TYPE_OPEN`.
pub const HTML_TAG_TYPE_OPEN: i32 = 1;
/// `HTML_TAG_TYPE_CLOSE`.
pub const HTML_TAG_TYPE_CLOSE: i32 = 2;

/// `M_PI`.
const M_PI: f64 = core::f64::consts::PI;

/// `struct spc_html_` (`_html_state`).
#[derive(Clone, Debug)]
pub struct State {
    /// `opts.extensions`.
    pub extensions: i32,
    pub link_dict: Option<Obj>,
    pub baseurl: Option<Vec<u8>>,
    /// -1 when no anchor is pending.
    pub pending_type: i32,
}

impl Default for State {
    fn default() -> Self {
        State {
            extensions: 0,
            link_dict: None,
            baseurl: None,
            pending_type: -1,
        }
    }
}

/// `downcasify`.
fn downcasify(s: &mut [u8]) {
    for c in s.iter_mut() {
        if *c == 0 {
            break;
        }
        if c.is_ascii_uppercase() {
            *c = *c - b'A' + b'a';
        }
    }
}

/// `parse_key_val` (static): status, key and value (`key="value"`).
fn parse_key_val(s: &[u8], pp: &mut usize) -> (i32, Option<Vec<u8>>, Option<Vec<u8>>) {
    let mut error = 0;
    let mut p = *pp;
    while p < s.len() && is_c_space(s[p]) {
        p += 1;
    }
    let mut v = None;
    let q = p;
    while p < s.len()
        && (s[p].is_ascii_lowercase()
            || s[p].is_ascii_uppercase()
            || s[p].is_ascii_digit()
            || s[p] == b'-'
            || s[p] == b':')
    {
        p += 1;
    }
    if p == q {
        return (-1, None, None);
    }
    let mut k = Some(s[q..p].to_vec());
    if p + 2 >= s.len() || s[p] != b'=' || (s[p + 1] != b'"' && s[p + 1] != b'\'') {
        k = None;
        *pp = p;
        error = -1;
    } else {
        let qchr = s[p + 1];
        p += 2; // skip '="'
        let q = p;
        while p < s.len() && s[p] != qchr {
            p += 1;
        }
        if p == s.len() || s[p] != qchr {
            error = -1;
        } else {
            v = Some(s[q..p].to_vec());
            p += 1;
        }
    }

    *pp = p;
    (error, k, v)
}

/// `fqurl` (static): `name` resolved against `baseurl`.
fn fqurl(baseurl: Option<&[u8]>, name: &[u8]) -> Vec<u8> {
    let mut q = Vec::new();
    if let Some(baseurl) = baseurl
        && !baseurl.is_empty()
    {
        q.extend_from_slice(baseurl);
        if q.last() == Some(&b'/') {
            q.pop();
        }
        if !name.is_empty() && name[0] != b'/' {
            q.push(b'/');
        }
    }
    q.extend_from_slice(name);
    q
}

/// `atopt` (static): a length with its unit, in bp.
fn atopt(a: &[u8]) -> f64 {
    let a = cstr(a);
    let mut p = 0;
    let mut u = 1.0;

    let Some(q) = parse_float_decimal(a, &mut p) else {
        crate::warn!("Invalid length value: {:?} ({:?})", a, a.get(p));
        return 0.0;
    };

    let v = crate::fmt::atof(&q);

    if let Some(q) = parse_c_ident(a, &mut p) {
        match cstr(&q) {
            b"pt" => u *= 72.0 / 72.27,
            b"in" => u *= 72.0,
            b"cm" => u *= 72.0 / 2.54,
            b"mm" => u *= 72.0 / 25.4,
            b"bp" => u *= 1.0,
            b"pc" => u *= 12.0 * 72.0 / 72.27,
            b"dd" => u *= 1238.0 / 1157.0 * 72.0 / 72.27,
            b"cc" => u *= 12.0 * 1238.0 / 1157.0 * 72.0 / 72.27,
            b"sp" => u *= 72.0 / (72.27 * 65536.0),
            b"px" => u *= 1.0, // 72dpi
            _ => crate::warn!("Unknown unit of measure: {:?}", q),
        }
    }

    v * u
}

/// `*p` of a C string: 0 at the end.
fn at(s: &[u8], p: usize) -> u8 {
    s.get(p).copied().unwrap_or(0)
}

/// `cvt_a_to_tmatrix` (static): an SVG `transform` item into `m`
/// (`pp` is C's `nextptr`, moved only on success); the status.
/// `translate wsp* '(' wsp* number (comma-wsp number)? wsp* ')'`.
fn cvt_a_to_tmatrix(m: &mut PdfTmatrix, s: &[u8], pp: &mut usize) -> i32 {
    const TKEYS: [&[u8]; 6] = [
        b"matrix",    // a b c d e f
        b"translate", // tx [ty] : dflt. tf = 0
        b"scale",     // sx [sy] : dflt. sy = sx
        b"rotate",    // ang [cx cy] : dflt. cx, cy = 0
        b"skewX",     // ang
        b"skewY",     // ang
    ];
    let mut p = *pp;
    let mut v = [0.0f64; 6];

    while is_c_space(at(s, p)) && at(s, p) != 0 {
        p += 1;
    }

    let Some(q) = parse_c_ident(s, &mut p) else {
        return -1;
    };
    // parsed transformation key
    let k = TKEYS
        .iter()
        .position(|t| *t == cstr(&q))
        .unwrap_or(TKEYS.len());

    // handle args
    while at(s, p) != 0 && is_c_space(at(s, p)) {
        p += 1;
    }
    if at(s, p) != b'(' || at(s, p + 1) == 0 {
        return -1;
    }
    p += 1;
    while at(s, p) != 0 && is_c_space(at(s, p)) {
        p += 1;
    }
    let mut n = 0;
    while n < 6 && at(s, p) != 0 && at(s, p) != b')' {
        let Some(q) = parse_float_decimal(s, &mut p) else {
            break;
        };
        v[n] = crate::fmt::atof(&q);
        if at(s, p) == b',' {
            p += 1;
        }
        while at(s, p) != 0 && is_c_space(at(s, p)) {
            p += 1;
        }
        if at(s, p) == b',' {
            p += 1;
            while at(s, p) != 0 && is_c_space(at(s, p)) {
                p += 1;
            }
        }
        n += 1;
    }
    if at(s, p) != b')' {
        return -1;
    }
    p += 1;

    match k {
        0 => {
            if n != 6 {
                return -1;
            }
            m.a = v[0];
            m.c = v[1];
            m.b = v[2];
            m.d = v[3];
            m.e = v[4];
            m.f = v[5];
        }
        1 => {
            if n != 1 && n != 2 {
                return -1;
            }
            m.a = 1.0;
            m.d = 1.0;
            m.c = 0.0;
            m.b = 0.0;
            m.e = v[0];
            m.f = if n == 2 { v[1] } else { 0.0 };
        }
        2 => {
            if n != 1 && n != 2 {
                return -1;
            }
            m.a = v[0];
            m.d = if n == 2 { v[1] } else { v[0] };
            m.c = 0.0;
            m.b = 0.0;
            m.e = 0.0;
            m.f = 0.0;
        }
        3 => {
            if n != 1 && n != 3 {
                return -1;
            }
            m.a = libm::cos(v[0] * M_PI / 180.0);
            m.c = libm::sin(v[0] * M_PI / 180.0);
            m.b = -m.c;
            m.d = m.a;
            m.e = if n == 3 { v[1] } else { 0.0 };
            m.f = if n == 3 { v[2] } else { 0.0 };
        }
        4 => {
            if n != 1 {
                return -1;
            }
            m.a = 1.0;
            m.d = 1.0;
            m.c = 0.0;
            m.b = libm::tan(v[0] * M_PI / 180.0);
        }
        5 => {
            if n != 1 {
                return -1;
            }
            m.a = 1.0;
            m.d = 1.0;
            m.c = libm::tan(v[0] * M_PI / 180.0);
            m.b = 0.0;
        }
        _ => {}
    }

    *pp = p;
    0
}

impl Dpx {
    /// `spc_html_at_begin_page`.
    pub fn spc_html_at_begin_page(&mut self) -> i32 {
        spc_handler_html__bophook(self, None)
    }
    /// `spc_html_at_end_page`.
    pub fn spc_html_at_end_page(&mut self) -> i32 {
        spc_handler_html__eophook(self, None)
    }
    /// `spc_html_at_begin_document`.
    pub fn spc_html_at_begin_document(&mut self) -> i32 {
        spc_handler_html__init(self)
    }
    /// `spc_html_at_end_document`.
    pub fn spc_html_at_end_document(&mut self) -> i32 {
        spc_handler_html__clean(self, None)
    }
}

/// `ISDELIM` of `read_html_tag`.
fn is_delim(c: u8) -> bool {
    c == b'>' || c == b'/' || is_c_space(c)
}

/// `read_html_tag` (static): status, the tag name (downcased), and
/// its type (`HTML_TAG_TYPE_*`); attributes go into `attr`.
fn read_html_tag(dpx: &mut Dpx, attr: Obj, s: &[u8], pp: &mut usize) -> (i32, Vec<u8>, i32) {
    let mut p = *pp;
    let mut error = 0;
    let mut name = Vec::new();
    let mut ty = 0;

    while p < s.len() && is_c_space(s[p]) {
        p += 1;
    }
    if p >= s.len() || s[p] != b'<' {
        return (-1, name, ty);
    }

    ty = HTML_TAG_TYPE_OPEN;
    p += 1;
    while p < s.len() && is_c_space(s[p]) {
        p += 1;
    }
    if p < s.len() && s[p] == b'/' {
        ty = HTML_TAG_TYPE_CLOSE;
        p += 1;
        while p < s.len() && is_c_space(s[p]) {
            p += 1;
        }
    }

    while p < s.len() && name.len() < HTML_TAG_NAME_MAX && !is_delim(s[p]) {
        name.push(s[p]);
        p += 1;
    }
    if name.is_empty() || p == s.len() || !is_delim(s[p]) {
        *pp = p;
        return (-1, name, ty);
    }

    while p < s.len() && is_c_space(s[p]) {
        p += 1;
    }
    while p < s.len() && error == 0 && s[p] != b'/' && s[p] != b'>' {
        let (e, kp, vp) = parse_key_val(s, &mut p);
        error = e;
        if error == 0 {
            let mut kp = kp.expect("key");
            downcasify(&mut kp);
            // include trailing NULL here!!!
            let mut v = cstr(&vp.expect("value")).to_vec();
            v.push(0);
            let k = dpx.o.new_name(cstr(&kp));
            let v = dpx.o.new_string(&v);
            dpx.o.add_dict(attr, k, Some(v));
        }
        while p < s.len() && is_c_space(s[p]) {
            p += 1;
        }
    }
    if error != 0 {
        *pp = p;
        return (error, name, ty);
    }

    if p < s.len() && s[p] == b'/' {
        ty = HTML_TAG_TYPE_EMPTY;
        p += 1;
        while p < s.len() && is_c_space(s[p]) {
            p += 1;
        }
    }
    if p == s.len() || s[p] != b'>' {
        *pp = p;
        return (-1, name, ty);
    }
    p += 1;

    downcasify(&mut name);
    *pp = p;
    (0, name, ty)
}

/// `spc_handler_html__init`.
fn spc_handler_html__init(dpx: &mut Dpx) -> i32 {
    dpx.html.link_dict = None;
    dpx.html.baseurl = None;
    dpx.html.pending_type = -1;
    0
}

/// `spc_handler_html__clean` (`spe` may be none).
fn spc_handler_html__clean(dpx: &mut Dpx, spe: Option<&mut SpcEnv>) -> i32 {
    dpx.html.baseurl = None;

    if dpx.html.pending_type >= 0 || dpx.html.link_dict.is_some() {
        crate::warn!("Unclosed html anchor found.");
    }

    if let Some(l) = dpx.html.link_dict {
        dpx.o.release(l);
    }

    dpx.html.pending_type = -1;
    dpx.html.baseurl = None;
    dpx.html.link_dict = None;

    0
}

/// `spc_handler_html__bophook`.
fn spc_handler_html__bophook(dpx: &mut Dpx, spe: Option<&mut SpcEnv>) -> i32 {
    if dpx.html.pending_type >= 0 {
        crate::warn!("...html anchor continues from previous page processed...");
    }
    0
}

/// `spc_handler_html__eophook`.
fn spc_handler_html__eophook(dpx: &mut Dpx, spe: Option<&mut SpcEnv>) -> i32 {
    if dpx.html.pending_type >= 0 {
        crate::warn!("Unclosed html anchor at end-of-page!");
    }
    0
}

/// `html_open_link`.
fn html_open_link(dpx: &mut Dpx, spe: &mut SpcEnv, name: &[u8]) -> i32 {
    // Should be checked somewhere else
    assert!(dpx.html.link_dict.is_none());

    let link_dict = dpx.o.new_dict();
    dpx.html.link_dict = Some(link_dict);
    dpx.o.put_name(link_dict, b"Type", b"Annot");
    dpx.o.put_name(link_dict, b"Subtype", b"Link");

    let color = dpx.o.new_array();
    for v in [0.0, 0.0, 1.0] {
        let n = dpx.o.new_number(v);
        dpx.o.add_array(color, n);
    }
    dpx.o.put(link_dict, b"C", color);

    let url = fqurl(dpx.html.baseurl.as_deref(), name);
    if url.first() == Some(&b'#') {
        dpx.o.put_string(link_dict, b"Dest", &url[1..]);
    } else {
        // Assume this is URL
        let action = dpx.o.new_dict();
        dpx.o.put_name(action, b"Type", b"Action");
        dpx.o.put_name(action, b"S", b"URI");
        dpx.o.put_string(action, b"URI", &url);
        let l = dpx.o.link(action);
        dpx.o.put(link_dict, b"A", l);
        dpx.o.release(action);
    }

    let l = dpx.o.link(link_dict);
    dpx.spc_begin_annot(spe, l);

    dpx.html.pending_type = ANCHOR_TYPE_HREF;

    0
}

/// `html_open_dest`.
fn html_open_dest(dpx: &mut Dpx, spe: &mut SpcEnv, name: &[u8]) -> i32 {
    let mut cp = PdfCoord {
        x: spe.x_user,
        y: spe.y_user,
    };
    dpx.pdf_dev_transform(&mut cp, None);

    // Otherwise must be bug
    let page_ref = dpx.pdf_doc_this_page_ref().expect("page_ref");

    let array = dpx.o.new_array();
    dpx.o.add_array(array, page_ref);
    let xyz = dpx.o.new_name(b"XYZ");
    dpx.o.add_array(array, xyz);
    let null = dpx.o.new_null();
    dpx.o.add_array(array, null);
    let top = dpx.o.new_number(cp.y + 24.0);
    dpx.o.add_array(array, top);
    let null = dpx.o.new_null();
    dpx.o.add_array(array, null);

    let error = dpx.pdf_doc_add_names(b"Dests", name, array);

    if error != 0 {
        dpx.spc_warn(
            spe,
            format_args!("Failed to add named destination: {:?}", name),
        );
    }

    dpx.html.pending_type = ANCHOR_TYPE_NAME;

    error
}

/// `ANCHOR_STARTED`.
fn anchor_started(dpx: &Dpx) -> bool {
    dpx.html.pending_type >= 0 || dpx.html.link_dict.is_some()
}

/// `spc_html__anchor_open`.
fn spc_html__anchor_open(dpx: &mut Dpx, spe: &mut SpcEnv, attr: Obj) -> i32 {
    if anchor_started(dpx) {
        dpx.spc_warn(spe, format_args!("Nested html anchors found!"));
        return -1;
    }

    let href = dpx.o.lookup_dict(attr, b"href");
    let name = dpx.o.lookup_dict(attr, b"name");
    match (href, name) {
        (Some(_), Some(_)) => {
            dpx.spc_warn(
                spe,
                format_args!("Sorry, you can't have both \"href\" and \"name\" in anchor tag..."),
            );
            -1
        }
        (Some(href), None) => {
            let v = cstr(dpx.o.string_value(href)).to_vec();
            html_open_link(dpx, spe, &v)
        }
        (None, Some(name)) => {
            // name
            let v = cstr(dpx.o.string_value(name)).to_vec();
            html_open_dest(dpx, spe, &v)
        }
        (None, None) => {
            dpx.spc_warn(
                spe,
                format_args!("You should have \"href\" or \"name\" in anchor tag!"),
            );
            -1
        }
    }
}

/// `spc_html__anchor_close`.
fn spc_html__anchor_close(dpx: &mut Dpx, spe: &mut SpcEnv) -> i32 {
    let mut error = 0;

    match dpx.html.pending_type {
        ANCHOR_TYPE_HREF => {
            if let Some(l) = dpx.html.link_dict {
                dpx.spc_end_annot(spe);
                dpx.o.release(l);
                dpx.html.link_dict = None;
                dpx.html.pending_type = -1;
            } else {
                dpx.spc_warn(
                    spe,
                    format_args!("Closing html anchor (link) without starting!"),
                );
                error = -1;
            }
        }
        ANCHOR_TYPE_NAME => {
            dpx.html.pending_type = -1;
        }
        _ => {
            dpx.spc_warn(
                spe,
                format_args!("No corresponding opening tag for html anchor."),
            );
            error = -1;
        }
    }

    error
}

/// `spc_html__base_empty`.
fn spc_html__base_empty(dpx: &mut Dpx, spe: &mut SpcEnv, attr: Obj) -> i32 {
    let Some(href) = dpx.o.lookup_dict(attr, b"href") else {
        dpx.spc_warn(spe, format_args!("\"href\" not found for \"base\" tag!"));
        return -1;
    };

    let vp = cstr(dpx.o.string_value(href)).to_vec();
    if let Some(old) = &dpx.html.baseurl {
        crate::warn!("\"baseurl\" changed: {:?} --> {:?}", old, vp);
    }
    dpx.html.baseurl = Some(vp);

    0
}

/// `create_xgstate`: an ExtGState with `ca` = `a` (20260113 sets no
/// `CA`).
fn create_xgstate(dpx: &mut Dpx, a: f64) -> Obj {
    let dict = dpx.o.new_dict();
    dpx.o.put_name(dict, b"Type", b"ExtGState");
    dpx.o.put_number(dict, b"ca", a);
    dict
}

/// `spc_html__img_empty` (the `ENABLE_HTML_IMG_SUPPORT` one).
fn spc_html__img_empty(dpx: &mut Dpx, spe: &mut SpcEnv, attr: Obj) -> i32 {
    let options = LoadOptions {
        page_no: 1,
        bbox_type: 0,
        dict: None,
        page_name: None,
    };
    let mut error = 0;
    let mut alpha = 1.0; // meaning fully opaque

    dpx.spc_warn(
        spe,
        format_args!("html \"img\" tag found (not completed, plese don't use!)."),
    );

    let Some(src) = dpx.o.lookup_dict(attr, b"src") else {
        dpx.spc_warn(
            spe,
            format_args!("\"src\" attribute not found for \"img\" tag!"),
        );
        return -1;
    };

    let mut ti = TransformInfo::default();
    ti.transform_info_clear();
    if let Some(obj) = dpx.o.lookup_dict(attr, b"width") {
        ti.width = atopt(dpx.o.string_value(obj));
        ti.flags |= INFO_HAS_WIDTH;
    }
    if let Some(obj) = dpx.o.lookup_dict(attr, b"height") {
        ti.height = atopt(dpx.o.string_value(obj));
        ti.flags |= INFO_HAS_HEIGHT;
    }

    if let Some(obj) = dpx.o.lookup_dict(attr, b"svg:opacity") {
        alpha = crate::fmt::atof(cstr(dpx.o.string_value(obj)));
        if !(0.0..=1.0).contains(&alpha) {
            crate::warn!("Invalid opacity value: {:?}", dpx.o.string_value(obj));
            alpha = 1.0;
        }
    }

    if let Some(obj) = dpx.o.lookup_dict(attr, b"svg:transform") {
        let s = cstr(dpx.o.string_value(obj)).to_vec();
        let mut p = 0;
        while p < s.len() && is_c_space(s[p]) {
            p += 1;
        }
        while p < s.len() && error == 0 {
            let mut n = PdfTmatrix::default();
            pdf_setmatrix(&mut n, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            error = cvt_a_to_tmatrix(&mut n, &s, &mut p);
            if error == 0 {
                n.f = -n.f;
                pdf_concatmatrix(&mut ti.matrix, &n);
                while p < s.len() && is_c_space(s[p]) {
                    p += 1;
                }
                if at(&s, p) == b',' {
                    p += 1;
                    while p < s.len() && is_c_space(s[p]) {
                        p += 1;
                    }
                }
            }
        }
    }

    if error != 0 {
        dpx.spc_warn(spe, format_args!("Error in html \"img\" tag attribute."));
        return error;
    }

    let filename = cstr(dpx.o.string_value(src)).to_vec();
    let id = dpx.pdf_ximage_load_image(None, &filename, options);
    if id < 0 {
        dpx.spc_warn(
            spe,
            format_args!("Could not find/load image: {:?}", filename),
        );
        error = -1;
    } else {
        if alpha != 0.0 {
            let dict = create_xgstate(dpx, alpha);
            dpx.pdf_dev_xgstate_push(dict);
        }
        let (x, y) = (spe.x_user, spe.y_user);
        dpx.spc_put_image(spe, id, &mut ti, x, y);
        if alpha != 0.0 {
            dpx.pdf_dev_xgstate_pop();
        }
    }

    error
}

/// `spc_handler_html_default`.
fn spc_handler_html_default(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> i32 {
    if ap.curptr >= ap.endptr {
        return 0;
    }

    let attr = dpx.o.new_dict();
    let (mut error, name, ty) = {
        let (s, pp) = ap.parts();
        read_html_tag(dpx, attr, s, pp)
    };
    if error != 0 {
        dpx.o.release(attr);
        return error;
    }
    match cstr(&name) {
        b"a" => match ty {
            HTML_TAG_TYPE_OPEN => error = spc_html__anchor_open(dpx, spe, attr),
            HTML_TAG_TYPE_CLOSE => error = spc_html__anchor_close(dpx, spe),
            _ => {
                dpx.spc_warn(spe, format_args!("Empty html anchor tag???"));
                error = -1;
            }
        },
        b"base" => {
            if ty == HTML_TAG_TYPE_CLOSE {
                dpx.spc_warn(spe, format_args!("Close tag for \"base\"???"));
                error = -1;
            } else {
                // treat "open" same as "empty"
                error = spc_html__base_empty(dpx, spe, attr);
            }
        }
        b"img" => {
            if ty == HTML_TAG_TYPE_CLOSE {
                dpx.spc_warn(spe, format_args!("Close tag for \"img\"???"));
                error = -1;
            } else {
                // treat "open" same as "empty"
                error = spc_html__img_empty(dpx, spe, attr);
            }
        }
        _ => {}
    }
    dpx.o.release(attr);

    while ap.curptr < ap.endptr && is_c_space(ap.cur()) {
        ap.curptr += 1;
    }

    error
}

/// `spc_html_check_special`.
pub fn spc_html_check_special(buffer: &[u8]) -> bool {
    let mut p = 0;
    while p < buffer.len() && is_c_space(buffer[p]) {
        p += 1;
    }
    buffer[p..].starts_with(b"html:")
}

/// `spc_html_setup_handler`.
pub fn spc_html_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> i32 {
    while ap.curptr < ap.endptr && is_c_space(ap.cur()) {
        ap.curptr += 1;
    }
    if ap.curptr + 5 > ap.endptr || !ap.rest().starts_with(b"html:") {
        return -1;
    }

    ap.command = Some(b"");

    sph.key = b"html:";
    sph.exec = spc_handler_html_default;

    ap.curptr += 5;
    while ap.curptr < ap.endptr && is_c_space(ap.cur()) {
        ap.curptr += 1;
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_val() {
        let s = b" href=\"#sec1\" name='x'";
        let mut p = 0;
        let (e, k, v) = parse_key_val(s, &mut p);
        assert_eq!(e, 0);
        assert_eq!(k.as_deref(), Some(&b"href"[..]));
        assert_eq!(v.as_deref(), Some(&b"#sec1"[..]));
        assert_eq!(p, 13);
        let (e, k, v) = parse_key_val(s, &mut p);
        assert_eq!(
            (e, k.as_deref(), v.as_deref()),
            (0, Some(&b"name"[..]), Some(&b"x"[..]))
        );
        let mut p = 0;
        assert_eq!(parse_key_val(b"=x", &mut p).0, -1);
        assert_eq!(p, 0);
        let mut p = 0;
        let (e, k, _) = parse_key_val(b"href x", &mut p);
        assert_eq!((e, k, p), (-1, None, 4));
    }

    #[test]
    fn url() {
        assert_eq!(fqurl(None, b"#a"), b"#a".to_vec());
        assert_eq!(fqurl(Some(b""), b"x.html"), b"x.html".to_vec());
        assert_eq!(
            fqurl(Some(b"http://h/"), b"x.html"),
            b"http://h/x.html".to_vec()
        );
        assert_eq!(fqurl(Some(b"http://h"), b"/x"), b"http://h/x".to_vec());
        assert_eq!(fqurl(Some(b"http://h"), b""), b"http://h".to_vec());
    }

    #[test]
    fn downcase() {
        let mut s = b"HrEf\0AB".to_vec();
        downcasify(&mut s);
        assert_eq!(s, b"href\0AB".to_vec());
    }

    #[test]
    fn check() {
        assert!(spc_html_check_special(b" html:<a href=\"x\">"));
        assert!(!spc_html_check_special(b"htm:"));
    }
}
