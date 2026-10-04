//! pdfTeX's PDF inclusion (`pdftoepdf.cc`): a page of a PDF file read
//! for `\pdfximage` (`read_pdf_info`), and written as a form `XObject`
//! when the image is (`write_epdf`), with the objects it refers to copied
//! after it under new numbers. A document's copied objects are kept, so
//! that an image of it written later refers to them again, while images
//! of it are read and not yet written (`PdfDocument`'s `inObjList` and
//! `occurences`, the `EPDF` field).
//!
//! The file is read as xpdf reads it (`partex_engine::pdfread`); the
//! values are printed as `copyObject` prints them, each `pdf_puts` and
//! `pdf_printf` setting `pdf_last_byte` (which `pdf_newline` reads) and
//! each byte of a name or a string not, as in utils.c.
//!
//! A page's Type 1 font that the map has (by its PostScript name, and
//! whose file is there) is replaced by pdfTeX's own embedding of it
//! (`copyFont`, unless `\pdfinclusioncopyfonts`): its descriptor is the
//! map entry's font file's, shared with TeX's fonts (its glyphs the
//! union), its `/BaseFont` an object holding the name, its `/Encoding`
//! the one xpdf reads it with (`partex_engine::gfxfont`) as
//! `/Differences`. Other fonts are copied as they are.

use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::pdfread::{Dict, Doc, Obj, Ref, Stream};

use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// A PDF image's part of its entry (`pdf_image_struct`, and the entry's
/// fields `read_pdf_info` fills in).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PdfImage {
    /// The document it is of (`EpdfDoc::id`).
    pub doc: u32,
    /// The page included, and the box (`pdf_box_spec_*`).
    pub page: i32,
    pub page_box: i32,
    /// `epdf_orig_x`, `epdf_orig_y`.
    pub orig_x: i32,
    pub orig_y: i32,
    /// `img_rotate`.
    pub rotate: i32,
    /// `img_pages`: the document's pages.
    pub pages: i32,
    /// The page has a `/Group`.
    pub group: bool,
}

partex_engine::persist_struct!(PdfImage {
    doc,
    page,
    page_box,
    orig_x,
    orig_y,
    rotate,
    pages,
    group
});

/// What an object copied is (`InObjType`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum InKind {
    /// `objOther`: copied as it is.
    Other,
    /// `objFont`: a font replaced, its descriptor's key and its encoding's
    /// object (`fd`, `enc_objnum`).
    Font { fd: (Vec<u8>, i32, i32), enc: i32 },
    /// `objFontDesc`: a replaced font's descriptor, which the job's end
    /// writes (its number the descriptor's).
    FontDesc,
}

partex_engine::persist_enum!(InKind {
    Other,
    Font { fd, enc },
    FontDesc
});

/// An object of the file copied (`InObj`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct InObj {
    pub num: i32,
    pub generation: i32,
    /// Its number in the output.
    pub objnum: i32,
    pub written: bool,
    pub kind: InKind,
}

partex_engine::persist_struct!(InObj {
    num,
    generation,
    objnum,
    written,
    kind
});

/// An open document (`PdfDocument`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EpdfDoc {
    pub id: u32,
    /// The file's name as found (`file_name`).
    pub name: Vec<u8>,
    /// `occurences`: images of it read, less those written, less one.
    pub occurrences: i32,
    pub objs: Vec<InObj>,
}

partex_engine::persist_struct!(EpdfDoc {
    id,
    name,
    occurrences,
    objs
});

/// The open documents (`pdfDocuments`), and what the images' writers
/// keep across images: the `EPDF` field.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct EpdfDocs {
    pub docs: Vec<EpdfDoc>,
    /// The next document's id.
    pub next: u32,
    /// `\pdfforcepagebox`'s warning was given (`warn_pdfpagebox` false).
    pub pagebox_warned: bool,
    /// writepng.c's `transparent_page_group` (0: none yet) and
    /// `transparent_page_group_was_written`.
    pub png_group: i32,
    pub png_group_written: bool,
}

partex_engine::persist_struct!(EpdfDocs {
    docs,
    next,
    pagebox_warned,
    png_group,
    png_group_written
});

super::val::record_by_hash!(EpdfDocs);

/// `pdf_box_spec_*`.
pub(crate) const BOX_MEDIA: i32 = 1;
pub(crate) const BOX_CROP: i32 = 2;
pub(crate) const BOX_BLEED: i32 = 3;
pub(crate) const BOX_TRIM: i32 = 4;
pub(crate) const BOX_ART: i32 = 5;

/// writeimg.c's `bp2int`: a `float` of big points, in scaled points,
/// rounded (C's `round`: halves away from zero).
pub(crate) fn bp2int(p: f32) -> i32 {
    let x = f64::from(p) * (f64::from(super::ONE_HUNDRED_BP) / 100.0);
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a page box, far below 2^31 sp"
    )]
    let t = x as i64;
    #[allow(clippy::cast_precision_loss, reason = "t is below 2^52")]
    let f = x - t as f64;
    let r = if f >= 0.5 {
        t + 1
    } else if f <= -0.5 {
        t - 1
    } else {
        t
    };
    i32::try_from(r).unwrap_or(if r < 0 { i32::MIN } else { i32::MAX })
}

/// `convertNumToPDF`: a real with at most six decimals, no exponent, its
/// zeros after the point dropped (and a tiny one 0).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "C's (int) floor of a value in a PDF's range"
)]
fn num_to_pdf(n: f64) -> Vec<u8> {
    const EPSILON: f64 = 0.5e-6;
    if n.abs() < EPSILON {
        return b"0".to_vec();
    }
    let mut out = Vec::new();
    let mut n = n;
    if n < 0.0 {
        out.push(b'-');
        n = -n;
    }
    n += EPSILON;
    let ival = n as i32;
    n -= f64::from(ival);
    out.extend_from_slice(alloc::format!("{ival}").as_bytes());
    let mut fval = (n * 1e6) as i32;
    if fval != 0 {
        let mut digits = [0u8; 6];
        for d in digits.iter_mut().rev() {
            *d = b'0' + (fval % 10) as u8;
            fval /= 10;
        }
        let keep = 6 - digits.iter().rev().take_while(|&&d| d == b'0').count();
        out.push(b'.');
        out.extend_from_slice(&digits[..keep]);
    }
    out
}

/// utils.c's `stripzeros`: the zeros after a point dropped from each
/// number of `a`, and the point with them if none is left (a leading one
/// kept as `0`).
fn stripzeros(a: &[u8]) -> Vec<u8> {
    #[derive(Clone, Copy, PartialEq)]
    enum S {
        NoNum,
        DotNoNum,
        Int,
        Dot,
        LeadDot,
        Frac,
    }
    let mut buf = a.to_vec();
    let (mut s, mut t) = (S::NoNum, S::NoNum);
    let (mut p, mut q, mut r) = (0usize, 0usize, 0usize);
    let digit = |c: u8| c.is_ascii_digit();
    while p < buf.len() {
        let c = buf[p];
        s = match s {
            S::NoNum if digit(c) => S::Int,
            S::NoNum if c == b'.' => S::LeadDot,
            S::Int if c == b'.' => S::Dot,
            S::Int if digit(c) => S::Int,
            S::Dot | S::LeadDot | S::Frac if digit(c) => S::Frac,
            S::Dot | S::LeadDot | S::Frac | S::DotNoNum if c == b'.' => S::DotNoNum,
            S::DotNoNum if digit(c) => S::DotNoNum,
            _ => S::NoNum,
        };
        match s {
            S::Dot => r = q,
            S::LeadDot => r = q + 1,
            S::Frac if c > b'0' => r = q + 1,
            S::NoNum if (t == S::Frac || t == S::Dot) && r != 0 => {
                q = r;
                r -= 1;
                // (a leading point and only zeros: 0)
                if buf[r] == b'.' {
                    buf[r] = b'0';
                }
                r = 0;
            }
            _ => {}
        }
        buf[q] = buf[p];
        q += 1;
        p += 1;
        t = s;
    }
    buf.truncate(q);
    buf
}

/// `sprintf("%.8f")` of each of `xs`, separated by spaces.
fn eight(xs: &[f64]) -> Vec<u8> {
    let mut v = Vec::new();
    for (i, x) in xs.iter().enumerate() {
        if i > 0 {
            v.push(b' ');
        }
        v.extend_from_slice(alloc::format!("{x:.8}").as_bytes());
    }
    v
}

/// utils.c's `convertStringToPDFString`: the bytes outside `!`..`~` in
/// octal, parentheses and backslashes escaped.
fn pdf_string(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &c in s {
        if !(b'!'..=b'~').contains(&c) {
            out.extend_from_slice(alloc::format!("\\{c:03o}").as_bytes());
        } else if c == b'(' || c == b')' || c == b'\\' {
            out.push(b'\\');
            out.push(c);
        } else {
            out.push(c);
        }
    }
    out
}

/// What `read_pdf_info` read of a page, for the image's entry.
pub(crate) struct PdfInfo {
    pub image: PdfImage,
    /// `img_width`, `img_height`.
    pub width: i32,
    pub height: i32,
}

/// The rectangle of page box `spec`.
fn page_box(p: &partex_engine::pdfread::Page, spec: i32) -> Option<[f64; 4]> {
    Some(match spec {
        BOX_MEDIA => p.media,
        BOX_CROP => p.crop,
        BOX_BLEED => p.bleed,
        BOX_TRIM => p.trim,
        BOX_ART => p.art,
        _ => return None,
    })
}

/// The `/BBox` and `/Matrix` (identity if none) `write_epdf` gives page
/// `page` of PDF file `data` (box `spec`), as a reader of the PDF reads
/// them (display lists, DESIGN 4.6).
pub(crate) fn included_form_box(
    data: &Arc<[u8]>,
    page: i32,
    spec: i32,
) -> Option<([f64; 4], [f64; 6])> {
    let doc = Doc::open(data).ok()?;
    let p = doc.page(usize::try_from(page).ok()?)?;
    let b = page_box(&p, spec)?;
    // (the numbers as `write_epdf` prints them, read back)
    let read = |xs: &[f64]| -> Vec<f64> {
        stripzeros(&eight(xs))
            .split(|&c| c == b' ')
            .filter_map(partex_engine::pdfread::number_value)
            .collect()
    };
    let mut bbox = [0.0; 4];
    for (x, v) in bbox.iter_mut().zip(read(&b)) {
        *x = v;
    }
    let mut matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let m = match p.rotate {
        90 => Some([0.0, -1.0, 1.0, 0.0, b[0] - b[1], b[1] + b[2]]),
        180 => Some([-1.0, 0.0, 0.0, -1.0, b[0] + b[2], b[1] + b[3]]),
        270 => Some([0.0, 1.0, -1.0, 0.0, b[0] + b[3], b[1] - b[0]]),
        _ => None,
    };
    if let Some(m) = m {
        for (x, v) in matrix.iter_mut().zip(read(&m)) {
            *x = v;
        }
    }
    Some((bbox, matrix))
}

/// A document's copy in progress (`write_epdf`'s locals and the
/// document's `inObjList`).
struct Copy<'d> {
    doc: &'d Doc,
    objs: Vec<InObj>,
    /// `encodingList`: the replaced fonts' encodings (`None` for a CID
    /// font) and their objects, the last added first.
    encodings: Vec<(Option<partex_engine::fofi::Encoding>, i32)>,
}

/// What `copyFont` reads of a font it replaces.
struct Replaced {
    fm: Arc<crate::fontmap::MapEntry>,
    desc: Ref,
    stem_v: f64,
    charset: Option<Vec<u8>>,
    dict: Dict,
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `pdf_puts`: bytes, which set `pdf_last_byte`.
    fn ep_puts(&mut self, s: &[u8]) {
        self.pdf.out.print(s);
        if let Some(&c) = s.last() {
            self.pdf.out.last_byte = c;
        }
    }

    /// `pdf_newline`.
    fn ep_newline(&mut self) {
        if self.pdf.out.last_byte != b'\n' {
            self.ep_puts(b"\n");
        }
    }

    /// `pdf_printf("%d 0 R", n)`.
    fn ep_ref(&mut self, n: i32) {
        self.pdf.out.objnum(n);
        self.ep_puts(b" 0 R");
    }

    /// `find_add_document`: the open document named `name`, one more
    /// image of it read; or a new one. Its id.
    fn epdf_find_add(&mut self, name: &[u8]) -> u32 {
        let docs = &mut *self.pdf.epdf;
        if let Some(d) = docs.docs.iter_mut().find(|d| d.name == name) {
            d.occurrences += 1;
            return d.id;
        }
        let id = docs.next;
        docs.next += 1;
        docs.docs.push(EpdfDoc {
            id,
            name: name.to_vec(),
            occurrences: 0,
            objs: Vec::new(),
        });
        id
    }

    /// pdftoepdf.cc's `read_pdf_info` for file `name` (as found) with
    /// contents `data`: the page (by `page_name` if given, else number
    /// `page_num`) and its box `spec`; the version allowed
    /// (`major.minor`, and what a newer one is: `errorlevel`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn read_pdf_info(
        &mut self,
        name: &[u8],
        data: &Arc<[u8]>,
        page_name: Option<&[u8]>,
        page_num: i32,
        spec: i32,
        major: i32,
        minor: i32,
        errorlevel: i32,
    ) -> Result<PdfInfo, Jump> {
        let id = self.epdf_find_add(name);
        let Ok(doc) = Doc::open(data) else {
            return self.pdftex_fail(Some(name), b"xpdf: reading PDF image failed");
        };
        #[allow(clippy::cast_possible_truncation, reason = "C's float")]
        let found = doc.version as f32;
        #[allow(clippy::cast_possible_truncation, reason = "C's float")]
        let wanted = (f64::from(major) + f64::from(minor) * 0.1) as f32;
        if f64::from(found) > f64::from(wanted) + 0.01 {
            let msg = alloc::format!(
                "PDF inclusion: found PDF version <{:.1}>, but at most version <{:.1}> allowed",
                f64::from(found),
                f64::from(wanted)
            );
            if errorlevel > 0 {
                return self.pdftex_fail(Some(name), msg.as_bytes());
            } else if errorlevel == 0 {
                self.pdftex_warn_in(Some(name), msg.as_bytes());
            }
        }
        let pages = i32::try_from(doc.num_pages()).unwrap_or(i32::MAX);
        let page_num = if let Some(dest) = page_name {
            let Some(n) = doc.find_dest(dest) else {
                let mut m = b"PDF inclusion: invalid destination <".to_vec();
                m.extend_from_slice(dest);
                m.push(b'>');
                return self.pdftex_fail(Some(name), &m);
            };
            if n == 0 {
                let mut m = b"PDF inclusion: destination is not a page <".to_vec();
                m.extend_from_slice(dest);
                m.push(b'>');
                return self.pdftex_fail(Some(name), &m);
            }
            i32::try_from(n).unwrap_or(i32::MAX)
        } else {
            if page_num <= 0 || page_num > pages {
                let m = alloc::format!("PDF inclusion: required page does not exist <{pages}>");
                return self.pdftex_fail(Some(name), m.as_bytes());
            }
            page_num
        };
        let Some(page) = doc.page(usize::try_from(page_num).unwrap_or(0)) else {
            return self.pdftex_fail(Some(name), b"xpdf: reading PDF image failed");
        };
        let Some(b) = page_box(&page, spec) else {
            let m = alloc::format!("PDF inclusion: unknown value of pagebox spec ({spec})");
            return self.pdftex_fail(Some(name), m.as_bytes());
        };
        // (the `float` globals `epdf_orig_x`, `epdf_width`, …)
        #[allow(clippy::cast_possible_truncation, reason = "C's float")]
        let f = |x: f64| x as f32;
        let (orig_x, width) = if b[2] > b[0] {
            (f(b[0]), f(b[2] - b[0]))
        } else {
            (f(b[2]), f(b[0] - b[2]))
        };
        let (orig_y, height) = if b[3] > b[1] {
            (f(b[1]), f(b[3] - b[1]))
        } else {
            (f(b[3]), f(b[1] - b[3]))
        };
        let group = matches!(
            doc.fetch(page.dict_ref)
                .as_dict()
                .map(|d| doc.lookup(d, b"Group")),
            Some(Obj::Dict(_))
        );
        Ok(PdfInfo {
            image: PdfImage {
                doc: id,
                page: page_num,
                page_box: spec,
                orig_x: bp2int(orig_x),
                orig_y: bp2int(orig_y),
                rotate: page.rotate,
                pages,
                group,
            },
            width: bp2int(width),
            height: bp2int(height),
        })
    }

    /// `copyName`: `/` (a `pdf_puts`) and the name's bytes, each outside
    /// `[0-9A-Za-z_.+-]` as `#XX` (a `pdf_printf`).
    fn ep_name(&mut self, s: &[u8]) {
        self.ep_puts(b"/");
        for &c in s {
            if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-' | b'+') {
                self.pdf.out.out(c);
            } else {
                self.ep_puts(alloc::format!("#{c:02X}").as_bytes());
            }
        }
    }

    /// `addOther`: the number of the copy of object `r`, new if it was
    /// not copied yet (and then copied by `writeRefs`).
    fn ep_add(&mut self, c: &mut Copy<'_>, r: Ref, file: &[u8]) -> Result<i32, Jump> {
        self.ep_add_kind(c, r, InKind::Other, None, file)
    }

    /// `addInObj`: object `r` as `kind`, numbered `num` (a descriptor's,
    /// `get_fd_objnum`) or anew; or its number if it is there already.
    fn ep_add_kind(
        &mut self,
        c: &mut Copy<'_>,
        r: Ref,
        kind: InKind,
        num: Option<i32>,
        file: &[u8],
    ) -> Result<i32, Jump> {
        if r.num == 0 {
            return self.pdftex_fail(Some(file), b"PDF inclusion: invalid reference");
        }
        if let Some(o) = c
            .objs
            .iter()
            .find(|o| o.num == r.num && o.generation == r.generation)
        {
            return Ok(o.objnum);
        }
        let objnum = match num {
            Some(n) => n,
            None => self.pdf_new_objnum()?,
        };
        c.objs.push(InObj {
            num: r.num,
            generation: r.generation,
            objnum,
            written: false,
            kind,
        });
        Ok(objnum)
    }

    /// `copyDict`'s entries.
    fn ep_dict(&mut self, c: &mut Copy<'_>, d: &Dict, file: &[u8]) -> Result<(), Jump> {
        for (k, v) in &d.0 {
            self.ep_name(k);
            self.ep_puts(b" ");
            self.ep_object(c, v, file)?;
            self.ep_puts(b"\n");
        }
        Ok(())
    }

    /// `copyStream`: bytes as they are, the last one `pdf_last_byte`.
    fn ep_stream_bytes(&mut self, data: &[u8]) {
        for &b in data {
            self.pdf.out.out(b);
        }
        self.pdf.out.last_byte = data.last().copied().unwrap_or(0);
    }

    /// `copyObject`.
    fn ep_object(&mut self, c: &mut Copy<'_>, o: &Obj, file: &[u8]) -> Result<(), Jump> {
        match o {
            Obj::Bool(b) => self.ep_puts(if *b { b"true" } else { b"false" }),
            Obj::Int(i) => self.ep_puts(alloc::format!("{i}").as_bytes()),
            Obj::Real(r) => self.ep_puts(&num_to_pdf(*r)),
            Obj::Str(s) => {
                if s.contains(&0) {
                    self.ep_puts(b"<");
                    for &b in s {
                        self.ep_puts(alloc::format!("{b:02x}").as_bytes());
                    }
                    self.ep_puts(b">");
                } else {
                    self.ep_puts(b"(");
                    for &b in s {
                        if b == b'(' || b == b')' || b == b'\\' {
                            self.ep_puts(&[b'\\', b]);
                        } else if !(0x20..=0x7f).contains(&b) {
                            self.ep_puts(alloc::format!("\\{b:03o}").as_bytes());
                        } else {
                            self.pdf.out.out(b);
                        }
                    }
                    self.ep_puts(b")");
                }
            }
            Obj::Name(n) => self.ep_name(n),
            Obj::Null => self.ep_puts(b"null"),
            Obj::Array(a) => {
                self.ep_puts(b"[");
                for x in a {
                    if !matches!(x, Obj::Name(_)) {
                        self.ep_puts(b" ");
                    }
                    self.ep_object(c, x, file)?;
                }
                self.ep_puts(b"]");
            }
            Obj::Dict(d) => {
                self.ep_puts(b"<<\n");
                self.ep_dict(c, d, file)?;
                self.ep_puts(b">>");
            }
            Obj::Stream(s) => {
                self.ep_puts(b"<<\n");
                self.ep_dict(c, &s.dict, file)?;
                self.ep_puts(b">>\n");
                self.ep_puts(b"stream\n");
                let raw = c.doc.raw(s).to_vec();
                self.ep_stream_bytes(&raw);
                self.ep_puts(b"\nendstream");
            }
            Obj::Ref(r) => {
                if r.num == 0 {
                    return self.pdftex_fail(
                        Some(file),
                        b"PDF inclusion: reference to invalid object (is the included pdf broken?)",
                    );
                }
                let n = self.ep_add(c, *r, file)?;
                self.ep_ref(n);
            }
            Obj::Cmd(_) | Obj::Error | Obj::Eof => {
                let m = alloc::format!("PDF inclusion: type <{}> cannot be copied", o.type_name());
                return self.pdftex_fail(Some(file), m.as_bytes());
            }
        }
        Ok(())
    }

    /// `copyFontResources`: each font copied (a reference as its object,
    /// a dictionary in place).
    fn ep_fonts(&mut self, c: &mut Copy<'_>, o: &Obj, file: &[u8]) -> Result<(), Jump> {
        let Obj::Dict(d) = o else {
            let m = alloc::format!(
                "PDF inclusion: invalid font resources dict type <{}>",
                o.type_name()
            );
            return self.pdftex_fail(Some(file), m.as_bytes());
        };
        self.ep_puts(b"/Font << ");
        for (k, v) in &d.0 {
            match v {
                Obj::Ref(r) => {
                    // (`copyFont`: one copied already, by its number)
                    if let Some(o) = c
                        .objs
                        .iter()
                        .find(|o| o.num == r.num && o.generation == r.generation)
                    {
                        let n = o.objnum;
                        self.ep_name(k);
                        self.ep_puts(b" ");
                        self.ep_ref(n);
                        self.ep_puts(b" ");
                    } else if let Some(rep) = self.ep_replaceable(c, *r) {
                        self.ep_replace_font(c, k, *r, &rep, file)?;
                    } else {
                        self.ep_name(k);
                        self.ep_puts(b" ");
                        self.ep_object(c, v, file)?;
                    }
                }
                Obj::Dict(_) => {
                    self.ep_name(k);
                    self.ep_puts(b" ");
                    self.ep_object(c, v, file)?;
                }
                _ => {
                    let m = alloc::format!(
                        "PDF inclusion: invalid font in reference type <{}>",
                        v.type_name()
                    );
                    return self.pdftex_fail(Some(file), m.as_bytes());
                }
            }
        }
        self.ep_puts(b">>\n");
        Ok(())
    }

    /// `copyFont`'s test: font `r` is a Type 1 font with its file (or a
    /// Type 1C one) whose PostScript name the map has, and replacement is
    /// on; what is read of it then.
    fn ep_replaceable(&mut self, c: &Copy<'_>, r: Ref) -> Option<Replaced> {
        if self.pdf.out.fixed_inclusion_copy_font != 0 {
            return None;
        }
        let doc = c.doc;
        let Obj::Dict(dict) = doc.fetch(r) else {
            return None;
        };
        if doc.lookup(&dict, b"Subtype").as_name() != Some(b"Type1") {
            return None;
        }
        let Obj::Name(base) = doc.lookup(&dict, b"BaseFont") else {
            return None;
        };
        let Some(Obj::Ref(desc)) = dict.get(b"FontDescriptor") else {
            return None;
        };
        let desc = *desc;
        let Obj::Dict(fd) = doc.fetch(desc) else {
            return None;
        };
        let file_ok = matches!(doc.lookup(&fd, b"FontFile"), Obj::Stream(_))
            || matches!(doc.lookup(&fd, b"FontFile3"),
                Obj::Stream(s) if doc.lookup(&s.dict, b"Subtype").as_name() == Some(b"Type1C"));
        if !file_ok {
            return None;
        }
        let fm = self.lookup_fontmap(&base)?;
        // (a /StemV that is no number: pdfTeX reads what xpdf's union
        // holds; 0 here)
        let stem_v = doc.lookup(&fd, b"StemV").as_num().unwrap_or(0.0);
        let charset = match doc.lookup(&fd, b"CharSet") {
            Obj::Str(s) => Some(s),
            _ => None,
        };
        Some(Replaced {
            fm,
            desc,
            stem_v,
            charset,
            dict,
        })
    }

    /// `copyFont`'s replacement: the font's descriptor made or shared,
    /// its glyphs marked (or the whole font), its encoding read, and the
    /// font numbered as an `objFont`, which `writeRefs` writes.
    fn ep_replace_font(
        &mut self,
        c: &mut Copy<'_>,
        tag: &[u8],
        r: Ref,
        rep: &Replaced,
        file: &[u8],
    ) -> Result<(), Jump> {
        let key = self.epdf_create_fontdescriptor(&rep.fm, crate::arith::zround(rep.stem_v))?;
        match &rep.charset {
            Some(cs) if rep.fm.is(crate::fontmap::F_SUBSETTED) => self.epdf_mark_glyphs(&key, cs),
            _ => self.embed_whole_font(&key),
        }
        let fd_objnum = self.fd_objnum(&key);
        self.ep_add_kind(c, rep.desc, InKind::FontDesc, Some(fd_objnum), file)?;
        self.ep_name(tag);
        // (`GfxFont::makeFont`, then `addEncoding`: its object first)
        let enc = partex_engine::gfxfont::encoding(c.doc, &rep.dict);
        let enc_objnum = self.pdf_new_objnum()?;
        c.encodings.insert(0, (enc, enc_objnum));
        let n = self.ep_add_kind(
            c,
            r,
            InKind::Font {
                fd: key,
                enc: enc_objnum,
            },
            None,
            file,
        )?;
        self.ep_puts(b" ");
        self.ep_ref(n);
        self.ep_puts(b" ");
        Ok(())
    }

    /// `copyFontDict`: a replaced font's dictionary, its descriptor, name
    /// and encoding pdfTeX's.
    fn ep_font_dict(
        &mut self,
        c: &mut Copy<'_>,
        o: &Obj,
        fd: &(Vec<u8>, i32, i32),
        enc: i32,
        file: &[u8],
    ) -> Result<(), Jump> {
        let Obj::Dict(d) = o else {
            let m = alloc::format!("PDF inclusion: invalid dict type <{}>", o.type_name());
            return self.pdftex_fail(Some(file), m.as_bytes());
        };
        self.ep_puts(b"<<\n");
        for (k, v) in &d.0 {
            if k.starts_with(b"FontDescriptor")
                || k.starts_with(b"BaseFont")
                || k.starts_with(b"Encoding")
            {
                continue;
            }
            self.ep_name(k);
            self.ep_puts(b" ");
            self.ep_object(c, v, file)?;
            self.ep_puts(b"\n");
        }
        let fd_objnum = self.fd_objnum(fd);
        let fn_objnum = self.fn_objnum(fd)?;
        for (key, n) in [
            (&b"FontDescriptor"[..], fd_objnum),
            (b"BaseFont", fn_objnum),
            (b"Encoding", enc),
        ] {
            self.ep_puts(b"/");
            self.ep_puts(key);
            self.ep_puts(b" ");
            self.pdf.out.objnum(n);
            self.ep_puts(b" 0 R\n");
        }
        self.ep_puts(b">>");
        Ok(())
    }

    /// `writeEncodings`: each replaced font's encoding (`epdf_write_enc`),
    /// the last added first; a CID font fails.
    fn ep_write_encodings(&mut self, c: &mut Copy<'_>, file: &[u8]) -> Result<(), Jump> {
        for (enc, objnum) in core::mem::take(&mut c.encodings) {
            let Some(names) = enc else {
                return self.pdftex_fail(
                    Some(file),
                    b"PDF inclusion: CID fonts are not supported (try to disable font replacement to fix this)",
                );
            };
            self.pdf_begin_dict(objnum, 1)?;
            self.ep_puts(b"/Type /Encoding\n");
            self.ep_puts(b"/Differences [");
            let mut old: i32 = -2;
            for (i, name) in (0i32..).zip(&names) {
                let Some(name) = name else {
                    continue;
                };
                let mut l = Vec::new();
                if i != old + 1 {
                    if old != -2 {
                        l.push(b' ');
                    }
                    l.extend_from_slice(alloc::format!("{i}").as_bytes());
                }
                l.push(b'/');
                l.extend_from_slice(name);
                self.ep_puts(&l);
                old = i;
            }
            self.ep_puts(b"]\n");
            self.pdf_end_dict();
        }
        Ok(())
    }

    /// `copyProcSet`.
    fn ep_procset(&mut self, o: &Obj, file: &[u8]) -> Result<(), Jump> {
        let Obj::Array(a) = o else {
            let m = alloc::format!(
                "PDF inclusion: invalid ProcSet array type <{}>",
                o.type_name()
            );
            return self.pdftex_fail(Some(file), m.as_bytes());
        };
        self.ep_puts(b"/ProcSet [ ");
        for x in a {
            let Obj::Name(n) = x else {
                let m = alloc::format!(
                    "PDF inclusion: invalid ProcSet entry type <{}>",
                    x.type_name()
                );
                return self.pdftex_fail(Some(file), m.as_bytes());
            };
            self.ep_name(n);
            self.ep_puts(b" ");
        }
        self.ep_puts(b"]\n");
        Ok(())
    }

    /// `copyOtherResources`: a dictionary (or a `/Subtype` name) copied;
    /// anything else warned of and left out.
    fn ep_other(&mut self, c: &mut Copy<'_>, o: &Obj, key: &[u8], file: &[u8]) -> Result<(), Jump> {
        let ok = if key == b"Subtype" {
            matches!(o, Obj::Name(_))
        } else {
            matches!(o, Obj::Dict(_))
        };
        if !ok {
            let what = if key == b"Subtype" {
                "PDF inclusion: Subtype in Resources dict is not a name"
            } else {
                "PDF inclusion: invalid other resource which is no dict"
            };
            let m = alloc::format!(
                "{what} (key '{}', type <{}>); ignored.",
                alloc::string::String::from_utf8_lossy(key),
                o.type_name()
            );
            self.pdftex_warn_in(Some(file), m.as_bytes());
            return Ok(());
        }
        self.ep_name(key);
        self.ep_puts(b" ");
        self.ep_object(c, o, file)
    }

    /// `writeRefs`: each object referred to and not written yet, in the
    /// order first referred to (the list grows as they are written).
    fn ep_write_refs(&mut self, c: &mut Copy<'_>, file: &[u8]) -> Result<(), Jump> {
        let mut i = 0;
        while i < c.objs.len() {
            if !c.objs[i].written {
                c.objs[i].written = true;
                let o = c.doc.fetch(Ref {
                    num: c.objs[i].num,
                    generation: c.objs[i].generation,
                });
                match c.objs[i].kind.clone() {
                    InKind::Font { fd, enc } => {
                        self.pdf_begin_obj(c.objs[i].objnum, 2)?;
                        self.ep_font_dict(c, &o, &fd, enc, file)?;
                        self.ep_puts(b"\n");
                        self.pdf_end_obj();
                    }
                    // (the job's end writes it, `write_fontdescriptor`)
                    InKind::FontDesc => {}
                    InKind::Other => {
                        let level = if matches!(o, Obj::Stream(_)) { 0 } else { 2 };
                        self.pdf_begin_obj(c.objs[i].objnum, level)?;
                        self.ep_object(c, &o, file)?;
                        self.ep_puts(b"\n");
                        self.pdf_end_obj();
                    }
                }
            }
            i += 1;
        }
        Ok(())
    }

    /// pdftoepdf.cc's `write_epdf`: image `im` of file `file`, its
    /// dictionary begun (`pdf_begin_dict` and its `attr`).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn write_epdf(
        &mut self,
        im: &PdfImage,
        file: &[u8],
        data: &Arc<[u8]>,
    ) -> Result<(), Jump> {
        let suppress = self.int_par(PDF_SUPPRESS_PTEX_INFO_CODE);
        let sep: &[u8] = if self.int_par(PDF_PTEX_USE_UNDERSCORE_CODE) > 0 {
            b"_"
        } else {
            b"."
        };
        let objs = {
            let docs = &mut *self.pdf.epdf;
            let Some(d) = docs.docs.iter_mut().find(|d| d.id == im.doc) else {
                return self.pdftex_fail(Some(file), b"xpdf: reading PDF image failed");
            };
            d.occurrences -= 1;
            core::mem::take(&mut d.objs)
        };
        let Ok(doc) = Doc::open(data) else {
            return self.pdftex_fail(Some(file), b"xpdf: reading PDF image failed");
        };
        let Some(page) = doc.page(usize::try_from(im.page).unwrap_or(0)) else {
            return self.pdftex_fail(Some(file), b"xpdf: reading PDF image failed");
        };
        let mut c = Copy {
            doc: &doc,
            objs,
            encodings: Vec::new(),
        };
        let page_obj = doc.fetch(page.dict_ref);
        let empty = Dict::default();
        let page_dict = page_obj.as_dict().unwrap_or(&empty);
        let rotate = page.rotate;
        self.ep_puts(b"/Type /XObject\n");
        self.ep_puts(b"/Subtype /Form\n");
        self.ep_puts(b"/FormType 1\n");
        if suppress & 0x02 == 0 {
            let mut l = b"/PTEX".to_vec();
            l.extend_from_slice(sep);
            l.extend_from_slice(b"FileName (");
            l.extend_from_slice(&pdf_string(file));
            l.extend_from_slice(b")\n");
            self.ep_puts(&l);
        }
        if suppress & 0x04 == 0 {
            let mut l = b"/PTEX".to_vec();
            l.extend_from_slice(sep);
            l.extend_from_slice(alloc::format!("PageNumber {}\n", im.page).as_bytes());
            self.ep_puts(&l);
        }
        if suppress & 0x08 == 0
            && let Some(Obj::Ref(info)) = doc.trailer.get(b"Info")
        {
            let mut l = b"/PTEX".to_vec();
            l.extend_from_slice(sep);
            l.extend_from_slice(b"InfoDict ");
            self.ep_puts(&l);
            let n = self.ep_add(&mut c, *info, file)?;
            self.ep_ref(n);
            self.ep_puts(b"\n");
        }
        let Some(b) = page_box(&page, im.page_box) else {
            let m = alloc::format!(
                "PDF inclusion: unknown value of pagebox spec ({})",
                im.page_box
            );
            return self.pdftex_fail(Some(file), m.as_bytes());
        };
        if rotate != 0 && rotate % 90 == 0 {
            self.print_str(alloc::format!(", page is rotated {rotate} degrees").as_bytes());
            let m = match rotate {
                90 => Some([0.0, -1.0, 1.0, 0.0, b[0] - b[1], b[1] + b[2]]),
                180 => Some([-1.0, 0.0, 0.0, -1.0, b[0] + b[2], b[1] + b[3]]),
                270 => Some([0.0, 1.0, -1.0, 0.0, b[0] + b[3], b[1] - b[0]]),
                _ => None,
            };
            if let Some(m) = m {
                let mut l = b"/Matrix [".to_vec();
                l.extend_from_slice(&eight(&m));
                l.extend_from_slice(b"]\n");
                self.ep_puts(&stripzeros(&l));
            }
        }
        let mut l = b"/BBox [".to_vec();
        l.extend_from_slice(&eight(&b));
        l.extend_from_slice(b"]\n");
        self.ep_puts(&stripzeros(&l));
        if let Some(m) = page_dict.get(b"Metadata")
            && !matches!(m, Obj::Null | Obj::Ref(_))
        {
            self.pdftex_warn_in(
                Some(file),
                b"PDF inclusion: /Metadata must be indirect object",
            );
        }
        for key in [
            &b"LastModified"[..],
            b"Metadata",
            b"PieceInfo",
            b"SeparationInfo",
        ] {
            if let Some(v) = page_dict.get(key)
                && *v != Obj::Null
            {
                self.ep_newline();
                self.ep_puts(b"/");
                self.ep_puts(key);
                self.ep_puts(b" ");
                self.ep_object(&mut c, v, file)?;
            }
        }
        let mut sep_group = None;
        if let Some(g) = page_dict.get(b"Group")
            && *g != Obj::Null
        {
            let val = self.pdf.ship.page_group_val;
            if val == 0 {
                if self.int_par(PDF_SUPPRESS_WARNING_PAGE_GROUP_CODE) == 0 {
                    self.pdftex_warn_in(
                        Some(file),
                        b"PDF inclusion: multiple pdfs with page group included in a single page",
                    );
                }
                self.ep_newline();
                self.ep_puts(b"/Group ");
                self.ep_object(&mut c, g, file)?;
            } else {
                let Obj::Dict(gd) = doc.follow(g) else {
                    return self.pdftex_fail(Some(file), b"PDF inclusion: /Group dict missing");
                };
                sep_group = Some((val, gd));
                self.ep_puts(b"/Group ");
                self.ep_ref(val);
                self.ep_puts(b"\n");
            }
        }
        match &page.resources {
            None => self.pdftex_warn_in(
                Some(file),
                b"PDF inclusion: /Resources missing. 'This practice is not recommended' (PDF Ref)",
            ),
            Some(res) => {
                self.ep_newline();
                self.ep_puts(b"/Resources <<\n");
                for (k, v) in &res.0 {
                    let v = doc.follow(v);
                    match &k[..] {
                        b"Font" => self.ep_fonts(&mut c, &v, file)?,
                        b"ProcSet" => self.ep_procset(&v, file)?,
                        _ => self.ep_other(&mut c, &v, k, file)?,
                    }
                }
                self.ep_puts(b">>\n");
            }
        }
        match doc.lookup(page_dict, b"Contents") {
            Obj::Stream(s) => self.ep_contents(&doc, &mut c, &s, file)?,
            Obj::Array(a) => {
                self.pdf_begin_stream();
                for (i, x) in a.iter().enumerate() {
                    if let Obj::Stream(s) = doc.follow(x) {
                        let decoded = doc.decode(&s);
                        self.ep_stream_bytes(&decoded);
                    }
                    if i + 1 < a.len() {
                        self.ep_newline();
                    }
                }
                self.pdf_end_stream();
            }
            _ => {
                self.pdf_begin_stream();
                self.pdf_end_stream();
            }
        }
        self.ep_write_encodings(&mut c, file)?;
        if let Some((val, gd)) = sep_group {
            self.pdf_begin_obj(val, 2)?;
            self.ep_object(&mut c, &Obj::Dict(gd), file)?;
            self.ep_puts(b"\n");
            self.pdf_end_obj();
            self.pdf.ship.page_group_val = 0;
        }
        self.ep_write_refs(&mut c, file)?;
        let objs = c.objs;
        if let Some(d) = self.pdf.epdf.docs.iter_mut().find(|d| d.id == im.doc) {
            d.objs = objs;
        }
        Ok(())
    }

    /// A page's single content stream: its length, filter and parameters,
    /// then its bytes as they are in the file.
    fn ep_contents(
        &mut self,
        doc: &Doc,
        c: &mut Copy<'_>,
        s: &Stream,
        file: &[u8],
    ) -> Result<(), Jump> {
        if doc.lookup(&s.dict, b"F") != Obj::Null {
            return self.pdftex_fail(Some(file), b"PDF inclusion: Unsupported external stream");
        }
        let len = doc.lookup(&s.dict, b"Length");
        self.ep_puts(b"/Length ");
        self.ep_object(c, &len, file)?;
        self.ep_puts(b"\n");
        let filter = doc.lookup(&s.dict, b"Filter");
        if filter != Obj::Null {
            self.ep_puts(b"/Filter ");
            self.ep_object(c, &filter, file)?;
            self.ep_puts(b"\n");
            let parms = doc.lookup(&s.dict, b"DecodeParms");
            if parms != Obj::Null {
                self.ep_puts(b"/DecodeParms ");
                self.ep_object(c, &parms, file)?;
                self.ep_puts(b"\n");
            }
        }
        self.ep_puts(b">>\nstream\n");
        let raw = doc.raw(s).to_vec();
        self.ep_stream_bytes(&raw);
        self.pdf_end_stream();
        Ok(())
    }

    /// `epdf_delete`, after image `im` was written: its document closed
    /// if no image of it is still to be written.
    pub(crate) fn epdf_delete(&mut self, im: &PdfImage) {
        let docs = &mut *self.pdf.epdf;
        if let Some(i) = docs.docs.iter().position(|d| d.id == im.doc)
            && docs.docs[i].occurrences < 0
        {
            docs.docs.remove(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_numbers_as_pdftex() {
        assert_eq!(num_to_pdf(0.0), b"0");
        assert_eq!(num_to_pdf(1e-7), b"0");
        assert_eq!(num_to_pdf(612.0), b"612");
        assert_eq!(num_to_pdf(-0.5), b"-0.5");
        assert_eq!(num_to_pdf(1.25), b"1.25");
        assert_eq!(num_to_pdf(0.333_333_33), b"0.333333");
        assert_eq!(num_to_pdf(2.999_999_9), b"3");
    }

    #[test]
    fn strips_zeros_as_utils_c() {
        assert_eq!(
            stripzeros(b"/BBox [0.00000000 0.00000000 612.00000000 792.50000000]\n"),
            b"/BBox [0 0 612 792.5]\n"
        );
        assert_eq!(stripzeros(b"[-1.00000000 0.25000000]"), b"[-1 0.25]");
        assert_eq!(stripzeros(b"[.50000000 .000]"), b"[.5 0]");
        assert_eq!(stripzeros(b"1.2.300 7.00"), b"1.2.300 7.00");
    }

    #[test]
    fn formats_eight_decimals_as_printf() {
        // (the double's exact value rounded, ties to even, as glibc's
        // printf: 791.999999995 is 791.99999999500005…, 1.000000015 is
        // 1.00000001499999…)
        assert_eq!(eight(&[0.001_953_125]), b"0.00195312");
        assert_eq!(eight(&[791.999_999_995]), b"792.00000000");
        assert_eq!(eight(&[1.000_000_015]), b"1.00000001");
    }

    #[test]
    fn rounds_big_points_as_writeimg() {
        assert_eq!(bp2int(612.0), 40_258_437);
        assert_eq!(bp2int(0.0), 0);
        assert_eq!(bp2int(-10.0), -657_818);
    }

    #[test]
    fn escapes_the_file_name() {
        assert_eq!(pdf_string(b"./a (b)\\c.pdf"), b"./a\\040\\(b\\)\\\\c.pdf");
    }
}
