//! spc_misc.c, spc_misc.h: pdfTeX-compatible and other small specials
//! (`postscriptbox`, `pdfcolorstack`, `pdffontattr`, `landscape`, …).

use crate::dpxutil::{DpxStack, dpx_util_read_length, parse_c_ident};
use crate::parse::{parse_ident, parse_number, parse_opt_ident, skip_line, skip_white};
use crate::pdfdev::{
    INFO_HAS_HEIGHT, INFO_HAS_USER_BBOX, INFO_HAS_WIDTH, PdfCoord, PdfRect, PdfTmatrix,
    TransformInfo,
};
use crate::pdfximage::LoadOptions;
use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler, cstr};

/// `PDFCOLORSTACK_MAX_STACK`.
pub const PDFCOLORSTACK_MAX_STACK: usize = 256;

/// `struct stack`: one pdfcolorstack.
#[derive(Clone, Debug, Default)]
pub struct Stack {
    pub page: i32,
    pub direct: i32,
    /// The literal strings (PDF string objects, owned: released on pop).
    pub stack: DpxStack<Obj>,
}

/// `struct fontattr`: a `pdffontattr` applied at the end of the document.
#[derive(Clone, Debug, Default)]
pub struct Fontattr {
    pub ident: Vec<u8>,
    pub size: f64,
    pub attr: Option<Obj>,
}

/// spc_misc.c's `static` state.
#[derive(Clone, Debug)]
pub struct State {
    /// `spc_stack.stacks` (`PDFCOLORSTACK_MAX_STACK` of them).
    pub stacks: Vec<Stack>,
    /// `fontattrs` (none = C's NULL; `num_fontattrs` is its length,
    /// `max_fontattrs` its capacity).
    pub fontattrs: Option<Vec<Fontattr>>,
}

impl Default for State {
    fn default() -> Self {
        let mut stacks = Vec::with_capacity(PDFCOLORSTACK_MAX_STACK);
        stacks.resize_with(PDFCOLORSTACK_MAX_STACK, Stack::default);
        State {
            stacks,
            fontattrs: None,
        }
    }
}

impl Dpx {
    /// `spc_misc_at_begin_document`.
    pub fn spc_misc_at_begin_document(&mut self) -> i32 {
        if self.misc.fontattrs.is_none() {
            self.misc.fontattrs = Some(Vec::with_capacity(256));
        }
        pdfcolorstack__init(self)
    }
    /// `spc_misc_at_end_document`.
    pub fn spc_misc_at_end_document(&mut self) -> i32 {
        if let Some(fontattrs) = self.misc.fontattrs.take() {
            for fa in fontattrs {
                let attr = fa.attr.expect("attr");
                process_fontattr(self, &fa.ident, fa.size, attr);
                self.o.release(attr);
            }
        }
        pdfcolorstack__clean(self)
    }
    /// `spc_misc_at_begin_page`.
    pub fn spc_misc_at_begin_page(&mut self) -> i32 {
        for i in 0..PDFCOLORSTACK_MAX_STACK {
            if self.misc.stacks[i].page != 0 {
                let litstr = self.misc.stacks[i].stack.dpx_stack_top().copied();
                let cp = PdfCoord { x: 0.0, y: 0.0 };
                let direct = self.misc.stacks[i].direct;
                if litstr.is_some() {
                    pdfcolorstack__set_litstr(self, cp, litstr, direct);
                }
            }
        }
        0
    }
    /// `spc_misc_at_begin_form`.
    pub fn spc_misc_at_begin_form(&mut self) -> i32 {
        self.spc_misc_at_begin_page()
    }
    /// `spc_misc_at_end_form`.
    pub fn spc_misc_at_end_form(&mut self) -> i32 {
        self.spc_misc_at_begin_page()
    }
}

/// `pdfcolorstack__get_id`: status and the id.
fn pdfcolorstack__get_id(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> (i32, i32) {
    if args.curptr >= args.endptr {
        dpx.spc_warn(spe, format_args!("Stack ID number expected but not found."));
        return (-1, 0);
    }
    let q = {
        let (s, pp) = args.parts();
        parse_number(s, pp)
    };
    let Some(q) = q else {
        dpx.spc_warn(spe, format_args!("Stack ID number expected but not found."));
        return (-1, 0);
    };
    let id = crate::fmt::atoi(&q) as i32;

    args.skip_white();

    (0, id)
}

/// `pdfcolorstack__init` (on `self.misc.stacks`).
fn pdfcolorstack__init(dpx: &mut Dpx) -> i32 {
    for st in &mut dpx.misc.stacks {
        st.page = 0;
        st.direct = 0;
        st.stack = DpxStack::dpx_stack_init();
    }
    0
}

/// `pdfcolorstack__clean` (releases the strings).
fn pdfcolorstack__clean(dpx: &mut Dpx) -> i32 {
    for i in 0..PDFCOLORSTACK_MAX_STACK {
        while let Some(litstr) = dpx.misc.stacks[i].stack.dpx_stack_pop() {
            dpx.o.release(litstr);
        }
    }
    0
}

/// `pdfcolorstack__set_litstr`.
fn pdfcolorstack__set_litstr(dpx: &mut Dpx, cp: PdfCoord, litstr: Option<Obj>, direct: i32) {
    let Some(litstr) = litstr else {
        return;
    };

    let mut m = PdfTmatrix::default();
    if direct == 0 {
        m.a = 1.0;
        m.d = 1.0;
        m.b = 0.0;
        m.c = 0.0;
        m.e = cp.x;
        m.f = cp.y;
        dpx.pdf_dev_concat(&m);
    }
    dpx.pdf_doc_add_page_content(b" ");
    let s = dpx.o.string_value(litstr).to_vec();
    dpx.pdf_doc_add_page_content(&s);
    if direct == 0 {
        m.e = -cp.x;
        m.f = -cp.y;
        dpx.pdf_dev_concat(&m);
    }
}

/// `parse_pdf_string(&args->curptr, args->endptr)`.
fn parse_litstr(dpx: &mut Dpx, args: &mut SpcArg) -> Option<Obj> {
    let (s, pp) = args.parts();
    dpx.o.parse_pdf_string(s, pp)
}

/// `pdfcolorstack__set`: `st` is the stack's id (index of
/// `self.misc.stacks`).
fn pdfcolorstack__set(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    st: usize,
    cp: PdfCoord,
    args: &mut SpcArg,
) -> i32 {
    args.skip_white();
    if args.curptr >= args.endptr {
        return -1;
    }

    let Some(litstr) = dpx.misc.stacks[st].stack.dpx_stack_pop() else {
        dpx.spc_warn(spe, format_args!("Stack empty!"));
        return -1;
    };
    dpx.o.release(litstr);

    if let Some(litstr) = parse_litstr(dpx, args) {
        dpx.misc.stacks[st].stack.dpx_stack_push(litstr);
        let direct = dpx.misc.stacks[st].direct;
        pdfcolorstack__set_litstr(dpx, cp, Some(litstr), direct);
        args.skip_white();
    }

    0
}

/// `pdfcolorstack__push`.
fn pdfcolorstack__push(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    st: usize,
    cp: PdfCoord,
    args: &mut SpcArg,
) -> i32 {
    args.skip_white();
    if args.curptr >= args.endptr {
        return -1;
    }

    if let Some(litstr) = parse_litstr(dpx, args) {
        dpx.misc.stacks[st].stack.dpx_stack_push(litstr);
        let direct = dpx.misc.stacks[st].direct;
        pdfcolorstack__set_litstr(dpx, cp, Some(litstr), direct);
        args.skip_white();
    }

    0
}

/// `pdfcolorstack__current`.
fn pdfcolorstack__current(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    st: usize,
    cp: PdfCoord,
    args: &mut SpcArg,
) -> i32 {
    let litstr = dpx.misc.stacks[st].stack.dpx_stack_top().copied();
    if litstr.is_some() {
        let direct = dpx.misc.stacks[st].direct;
        pdfcolorstack__set_litstr(dpx, cp, litstr, direct);
        args.skip_white();
    } else {
        dpx.spc_warn(spe, format_args!("Stack empty!"));
        return -1;
    }

    0
}

/// `pdfcolorstack__pop`.
fn pdfcolorstack__pop(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    st: usize,
    cp: PdfCoord,
    args: &mut SpcArg,
) -> i32 {
    let error = 0;

    // "default" at the bottom
    if dpx.misc.stacks[st].stack.dpx_stack_depth() < 2 {
        dpx.spc_warn(spe, format_args!("Stack underflow"));
        return -1;
    }
    if let Some(litstr) = dpx.misc.stacks[st].stack.dpx_stack_pop() {
        dpx.o.release(litstr);
    }
    let litstr = dpx.misc.stacks[st].stack.dpx_stack_top().copied();
    if litstr.is_some() {
        let direct = dpx.misc.stacks[st].direct;
        pdfcolorstack__set_litstr(dpx, cp, litstr, direct);
    }

    error
}

/// `parse_pdf_reference` (static; the `@name` callback of the parser;
/// C's duplicate of spc_pdfm's).
fn parse_pdf_reference(dpx: &mut Dpx, s: &[u8], pp: &mut usize) -> Option<Obj> {
    skip_white(s, pp);
    match parse_opt_ident(s, pp) {
        Some(name) => {
            let result = dpx.spc_lookup_reference(&name);
            if result.is_none() {
                crate::warn!("Could not find the named reference (@{:?}).", name);
            }
            result
        }
        None => {
            crate::warn!("Could not find a reference name.");
            None
        }
    }
}

/// `process_fontattr`: merges `attr` into the font's resource dict.
fn process_fontattr(dpx: &mut Dpx, ident: &[u8], size: f64, attr: Obj) -> i32 {
    let font_id = dpx.pdf_font_findresource(ident, size);
    if font_id < 0 {
        crate::warn!(
            "Could not find specified font resource: {:?} ({}pt)",
            ident,
            size
        );
        return -1;
    }

    let fontdict = dpx.pdf_get_font_resource(font_id);

    dpx.o.merge_dict(fontdict, attr);

    0
}

/// `spc_handler_pdfcolorstackinit`.
fn spc_handler_pdfcolorstackinit(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    let cp = PdfCoord { x: 0.0, y: 0.0 };

    args.skip_white();
    if args.curptr >= args.endptr {
        return -1;
    }

    let (status, id) = pdfcolorstack__get_id(dpx, spe, args);
    if status < 0 {
        return -1;
    }
    if id < 0 || id >= PDFCOLORSTACK_MAX_STACK as i32 {
        dpx.spc_warn(spe, format_args!("Invalid stack number specified: {}", id));
        return -1;
    }
    args.skip_white();

    let st = id as usize;
    if dpx.misc.stacks[st].stack.dpx_stack_depth() > 0 {
        dpx.spc_warn(spe, format_args!("Stadk ID={} already initialized?", id));
        return -1;
    }

    loop {
        let q = {
            let (s, pp) = args.parts();
            parse_c_ident(s, pp)
        };
        let Some(q) = q else {
            break;
        };
        let qs = cstr(&q);
        if qs == b"page" {
            dpx.misc.stacks[st].page = 1;
        } else if qs == b"direct" {
            dpx.misc.stacks[st].direct = 1;
        } else {
            dpx.spc_warn(
                spe,
                format_args!(
                    "Ignoring unknown option for pdfcolorstack special (init): {:?}",
                    q
                ),
            );
        }
        args.skip_white();
    }

    if args.curptr < args.endptr {
        if let Some(litstr) = parse_litstr(dpx, args) {
            dpx.misc.stacks[st].stack.dpx_stack_push(litstr);
            let direct = dpx.misc.stacks[st].direct;
            pdfcolorstack__set_litstr(dpx, cp, Some(litstr), direct);
        }
        args.skip_white();
    } else {
        dpx.spc_warn(spe, format_args!("No valid PDF literal specified."));
        return -1;
    }

    0
}

/// `spc_handler_pdfcolorstack`.
fn spc_handler_pdfcolorstack(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    let mut error = 0;

    args.skip_white();
    if args.curptr >= args.endptr {
        return -1;
    }

    let (status, id) = pdfcolorstack__get_id(dpx, spe, args);
    if status < 0 {
        return -1;
    }
    if id < 0 || id >= PDFCOLORSTACK_MAX_STACK as i32 {
        dpx.spc_warn(spe, format_args!("Invalid stack ID specified: {}", id));
        return -1;
    }
    args.skip_white();

    let st = id as usize;
    if dpx.misc.stacks[st].stack.dpx_stack_depth() < 1 {
        dpx.spc_warn(
            spe,
            format_args!("Stack ID={} not properly initialized?", id),
        );
        return -1;
    }

    let command = {
        let (s, pp) = args.parts();
        parse_c_ident(s, pp)
    };
    let Some(command) = command else {
        return -1;
    };

    let cp = dpx.spc_get_current_point(spe);
    match cstr(&command) {
        b"set" => error = pdfcolorstack__set(dpx, spe, st, cp, args),
        b"push" => error = pdfcolorstack__push(dpx, spe, st, cp, args),
        b"pop" => error = pdfcolorstack__pop(dpx, spe, st, cp, args),
        b"current" => error = pdfcolorstack__current(dpx, spe, st, cp, args),
        _ => dpx.spc_warn(spe, format_args!("Unknown action: {:?}", command)),
    }

    if error != 0 {
        dpx.spc_warn(
            spe,
            format_args!(
                "Error occurred while processing pdfcolorstack: id={} command={:?}",
                id, command
            ),
        );
    }

    error
}

/// `spc_handler_pdffontattr`.
fn spc_handler_pdffontattr(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> i32 {
    let mut size = 0.0;

    ap.skip_white();
    if ap.curptr >= ap.endptr {
        return -1;
    }

    let ident = {
        let (s, pp) = ap.parts();
        parse_ident(s, pp)
    };
    let Some(ident) = ident else {
        dpx.spc_warn(spe, format_args!("Missing a font name."));
        return -1;
    };
    ap.skip_white();

    if ap.curptr < ap.endptr && ap.cur() != b'<' {
        let (error, v) = {
            let (s, pp) = ap.parts();
            dpx_util_read_length(1.0, s, pp)
        };
        if error != 0 {
            dpx.spc_warn(spe, format_args!("Font size expected but not found."));
            return -1;
        }
        size = v;
        ap.skip_white();
    }

    let attr = {
        let (s, pp) = ap.parts();
        dpx.parse_pdf_object_extended(s, pp, parse_pdf_reference)
    };
    let Some(attr) = attr else {
        dpx.spc_warn(
            spe,
            format_args!("Failed to parse a PDF dictionary object: {:?}", ident),
        );
        return -1;
    };
    if !dpx.o.is_dict(Some(attr)) {
        dpx.spc_warn(
            spe,
            format_args!("PDF dict expected but non-dict object found: {:?}", ident),
        );
        dpx.o.release(attr);
        return -1;
    }
    ap.skip_white();

    dpx.misc
        .fontattrs
        .get_or_insert_with(Vec::new)
        .push(Fontattr {
            ident: cstr(&ident).to_vec(),
            size,
            attr: Some(attr),
        });

    0
}

/// `sscanf(buf, "{%lfpt}{%lfpt}{%255[^}]}", &width, &height, filename)`:
/// how many were converted, and the values.
fn scan_postscriptbox(buf: &[u8]) -> (i32, f64, f64, Vec<u8>) {
    let mut p = 0;
    let mut n = 0;
    let (mut w, mut h) = (0.0, 0.0);
    let lit = |p: &mut usize, l: &[u8]| -> bool {
        if buf[*p..].starts_with(l) {
            *p += l.len();
            true
        } else {
            false
        }
    };
    let num = |p: &mut usize| -> Option<f64> {
        let (v, used) = crate::fmt::strtod(&buf[*p..]);
        if used == 0 {
            None
        } else {
            *p += used;
            Some(v)
        }
    };
    if !lit(&mut p, b"{") {
        return (n, w, h, Vec::new());
    }
    let Some(v) = num(&mut p) else {
        return (n, w, h, Vec::new());
    };
    w = v;
    n += 1;
    if !lit(&mut p, b"pt}{") {
        return (n, w, h, Vec::new());
    }
    let Some(v) = num(&mut p) else {
        return (n, w, h, Vec::new());
    };
    h = v;
    n += 1;
    if !lit(&mut p, b"pt}{") {
        return (n, w, h, Vec::new());
    }
    let start = p;
    while p < buf.len() && p - start < 255 && buf[p] != b'}' {
        p += 1;
    }
    if p == start {
        return (n, w, h, Vec::new());
    }
    n += 1;
    (n, w, h, buf[start..p].to_vec())
}

/// mpost.c's `mps_scan_bbox` (mpost.c is not ported; `translate_origin`
/// is 0, and `Xorigin`/`Yorigin` are only read by mpost.c): status and
/// the `%%BoundingBox`.
fn mps_scan_bbox(s: &[u8], pp: &mut usize) -> (i32, PdfRect) {
    let mut bbox = PdfRect::default();
    let mut values = [0.0f64; 4];

    // skip_white() skips lines starting '%'...
    while *pp < s.len() && crate::fmt::is_c_space(s[*pp]) {
        *pp += 1;
    }

    // Scan for bounding box record
    while *pp < s.len() && s[*pp] == b'%' {
        if *pp + 14 < s.len() && &s[*pp..*pp + 14] == b"%%BoundingBox:" {
            *pp += 14;

            let mut i = 0;
            while i < 4 {
                skip_white(s, pp);
                let Some(number) = parse_number(s, pp) else {
                    break;
                };
                values[i] = crate::fmt::atof(&number);
                i += 1;
            }
            if i < 4 {
                return (-1, bbox);
            }
            bbox.llx = values[0];
            bbox.lly = values[1];
            bbox.urx = values[2];
            bbox.ury = values[3];
            return (0, bbox);
        }
        skip_line(s, pp);
        while *pp < s.len() && crate::fmt::is_c_space(s[*pp]) {
            *pp += 1;
        }
    }

    (-1, bbox)
}

/// `spc_handler_postscriptbox`.
fn spc_handler_postscriptbox(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> i32 {
    let options = LoadOptions {
        page_no: 1,
        bbox_type: 0,
        dict: None,
        page_name: None,
    };

    if ap.curptr >= ap.endptr {
        dpx.spc_warn(
            spe,
            format_args!("No width/height/filename given for postscriptbox special."),
        );
        return -1;
    }

    // input is not NULL terminated
    let rest = ap.rest();
    let buf = cstr(&rest[..rest.len().min(511)]).to_vec();

    let mut ti = TransformInfo::default();
    ti.transform_info_clear();

    dpx.spc_warn(spe, format_args!("{:?}", buf));
    let (n, width, height, filename) = scan_postscriptbox(&buf);
    if n != 3 {
        dpx.spc_warn(spe, format_args!("Syntax error in postscriptbox special?"));
        return -1;
    }
    ti.width = width;
    ti.height = height;
    ap.curptr = ap.endptr;

    ti.width *= 72.0 / 72.27;
    ti.height *= 72.0 / 72.27;

    let Some(fullname) = dpx
        .files
        .find(&filename, crate::io::Format::Pict, b"dvipdfmx")
    else {
        dpx.spc_warn(spe, format_args!("Image file {:?} not found.", filename));
        return -1;
    };

    let Some(data) = dpx.files.read(&fullname) else {
        dpx.spc_warn(
            spe,
            format_args!("Could not open image file: {:?}", fullname),
        );
        return -1;
    };
    let mut fp = MemFile::new(data, &fullname);

    ti.flags |= INFO_HAS_WIDTH | INFO_HAS_HEIGHT;

    while let Some(line) = fp.mfgets(512) {
        let line = cstr(&line);
        let mut p = 0;
        let (r, bbox) = mps_scan_bbox(line, &mut p);
        if r >= 0 {
            ti.bbox = bbox;
            ti.flags |= INFO_HAS_USER_BBOX;
            break;
        }
    }

    let form_id = dpx.pdf_ximage_load_image(None, &filename, options);
    if form_id < 0 {
        dpx.spc_warn(
            spe,
            format_args!("Failed to load image file: {:?}", filename),
        );
        return -1;
    }

    let (x, y) = (spe.x_user, spe.y_user);
    dpx.spc_put_image(spe, form_id, &mut ti, x, y);

    0
}

/// `spc_handler_null`: skips the rest.
fn spc_handler_null(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    args.curptr = args.endptr;
    0
}

/// `misc_handlers`.
pub static MISC_HANDLERS: [SpcHandler; 9] = [
    SpcHandler {
        key: b"postscriptbox",
        exec: spc_handler_postscriptbox,
    },
    SpcHandler {
        key: b"pdfcolorstackinit",
        exec: spc_handler_pdfcolorstackinit,
    },
    SpcHandler {
        key: b"pdfcolorstack",
        exec: spc_handler_pdfcolorstack,
    },
    SpcHandler {
        key: b"pdffontattr",
        exec: spc_handler_pdffontattr,
    },
    // handled at bop
    SpcHandler {
        key: b"landscape",
        exec: spc_handler_null,
    },
    // handled at bop
    SpcHandler {
        key: b"papersize",
        exec: spc_handler_null,
    },
    // simply ignored
    SpcHandler {
        key: b"src:",
        exec: spc_handler_null,
    },
    SpcHandler {
        key: b"pos:",
        exec: spc_handler_null,
    },
    SpcHandler {
        key: b"om:",
        exec: spc_handler_null,
    },
];

/// `spc_misc_check_special`.
pub fn spc_misc_check_special(buffer: &[u8]) -> bool {
    let mut p = 0;
    skip_white(buffer, &mut p);
    let rest = &buffer[p..];
    MISC_HANDLERS.iter().any(|h| rest.starts_with(h.key))
}

/// `spc_misc_setup_handler` (sets `handle.key` to `"???:"`).
pub fn spc_misc_setup_handler(
    dpx: &mut Dpx,
    handle: &mut SpcHandler,
    spe: &mut SpcEnv,
    args: &mut SpcArg,
) -> i32 {
    args.skip_white();

    let key = args.curptr;
    while args.curptr < args.endptr && args.cur().is_ascii_alphabetic() {
        args.curptr += 1;
    }

    if args.curptr < args.endptr && args.cur() == b':' {
        args.curptr += 1;
    }

    let keylen = args.curptr - key;
    if keylen < 1 {
        return -1;
    }

    for h in &MISC_HANDLERS {
        if keylen == h.key.len() && &args.buf[key..args.curptr] == h.key {
            args.skip_white();

            args.command = Some(h.key);

            handle.key = b"???:";
            handle.exec = h.exec;

            return 0;
        }
    }

    -1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check() {
        assert!(spc_misc_check_special(b" pdfcolorstack 0 push (0 g)"));
        assert!(spc_misc_check_special(b"src:12foo.tex"));
        assert!(spc_misc_check_special(b"landscape"));
        assert!(!spc_misc_check_special(b"pdf:literal"));
    }

    #[test]
    fn psbox_scan() {
        let (n, w, h, f) = scan_postscriptbox(b"{100.0pt}{50pt}{fig.eps}");
        assert_eq!((n, w, h, f.as_slice()), (3, 100.0, 50.0, &b"fig.eps"[..]));
        let (n, ..) = scan_postscriptbox(b"{100.0pt}{50pt}{}");
        assert_eq!(n, 2);
        let (n, ..) = scan_postscriptbox(b"{ 1pt}x");
        assert_eq!(n, 1);
        let (n, ..) = scan_postscriptbox(b"100pt");
        assert_eq!(n, 0);
    }

    #[test]
    fn bbox_scan() {
        let line = b"%%BoundingBox: 0 1 20.5 30";
        let mut p = 0;
        let (r, b) = mps_scan_bbox(line, &mut p);
        assert_eq!(r, 0);
        assert_eq!((b.llx, b.lly, b.urx, b.ury), (0.0, 1.0, 20.5, 30.0));
        let mut p = 0;
        assert_eq!(mps_scan_bbox(b"%!PS-Adobe-3.0 EPSF-3.0", &mut p).0, -1);
        let mut p = 0;
        assert_eq!(mps_scan_bbox(b"%%BoundingBox: (atend)", &mut p).0, -1);
    }
}
