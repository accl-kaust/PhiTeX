//! A page as it is shipped, before the PDF is written (DESIGN 4.8): its
//! content stream and what its resources name, handed to the host as each
//! stream ends ([`Host::stream_shipped`](crate::Host::stream_shipped)),
//! so a viewer can draw the page while later pages are still typeset.
//!
//! The PDF names a page's fonts, forms and images by objects that are
//! written later (the fonts, subset, at the job's end) and finds them
//! through a cross-reference table written last. A shipped stream is drawn
//! instead through [`page_pdf`]: a PDF of that one page, made of the
//! stream's bytes as the PDF holds them, its forms' streams, its JPEG
//! images, and its fonts with the whole Type 1 programs their map entries
//! name (not the subsets), so the same reader that draws the finished PDF
//! (`phitex-draw`) draws it. Nothing of it is written to the job's PDF:
//! with a host that asks for streams the output is the same bytes.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::displist::{FontRec, Shipped};
use crate::host::FileKind;
use crate::pdf::image::Image;

/// A content stream as shipped: a page's or a form's.
#[derive(Clone, Debug)]
pub struct ShippedStream(
    pub(crate) Arc<Shipped>,
    pub(crate) bool,
    pub(crate) Option<Uni>,
);

/// The `\pdfglyphtounicode` entries a stream's fonts get their
/// `/ToUnicode` maps from (`\pdfgentounicode` on), as the job's fonts
/// will.
#[derive(Clone)]
pub struct Uni(pub(crate) crate::pdfconv::ToUnicodeTable);

impl core::fmt::Debug for Uni {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Uni({} entries)", self.0.len())
    }
}

impl ShippedStream {
    /// The form's object number (as the page's resources name it), or
    /// `None` for a page.
    #[must_use]
    pub fn form(&self) -> Option<i32> {
        (self.0.form != 0).then_some(self.0.form)
    }

    /// The forms it draws, by object number.
    pub fn forms(&self) -> impl Iterator<Item = i32> + '_ {
        self.0.forms.iter().map(|&(_, k)| k)
    }

    /// Whether [`page_pdf`] draws it as the job's PDF will: its images are
    /// JPEGs, and (a page) it has no `\pdfpageresources` and no
    /// annotations or links, (a form) no resources of its own. Its forms
    /// are asked apart ([`Shipments::page_pdf`]).
    #[must_use]
    pub fn whole(&self) -> bool {
        self.1
    }
}

/// The streams a build shipped so far, as a host that draws pages while
/// the build runs keeps them ([`crate::Host::stream_shipped`]): each
/// page's last, each form by object number. A page shipped again (a later
/// trip's, a rebuild's) replaces the one before.
#[derive(Clone, Debug, Default)]
pub struct Shipments {
    forms: BTreeMap<i32, ShippedStream>,
    pages: Vec<Option<ShippedStream>>,
}

impl Shipments {
    /// Keep stream `s`, page `page` (from 0) or a form.
    pub fn add(&mut self, page: Option<usize>, s: ShippedStream) {
        match (page, s.form()) {
            (Some(k), _) => {
                if self.pages.len() <= k {
                    self.pages.resize_with(k + 1, || None);
                }
                self.pages[k] = Some(s);
            }
            (None, Some(n)) => {
                self.forms.insert(n, s);
            }
            (None, None) => {}
        }
    }

    /// `other`'s streams kept too, over those kept for the same page or
    /// form (they were shipped after them).
    pub fn extend(&mut self, other: Shipments) {
        self.forms.extend(other.forms);
        for (k, p) in other.pages.into_iter().enumerate() {
            if let Some(p) = p {
                self.add(Some(k), p);
            }
        }
    }

    /// Forget every stream (a build from the start).
    pub fn clear(&mut self) {
        self.forms.clear();
        self.pages.clear();
    }

    /// The forms kept (a form a page draws may be shipped after the page:
    /// one not `\immediate` is written at its first use, after the page
    /// object).
    #[must_use]
    pub fn forms(&self) -> usize {
        self.forms.len()
    }

    /// One past the last page shipped (0: none yet).
    #[must_use]
    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    /// Page `k`'s stream, if it was shipped.
    #[must_use]
    pub fn page(&self, k: usize) -> Option<&ShippedStream> {
        self.pages.get(k)?.as_ref()
    }

    /// Page `k`'s own PDF ([`page_pdf`]), its fonts' programs read through
    /// `read`, and whether it is whole: drawn as the job's PDF will draw
    /// it, so that its page hash there is this PDF's (the page's stream,
    /// its size and its forms' and images' bytes, which a hash of the
    /// job's PDF covers, are the same), the page and every form it draws
    /// being [`ShippedStream::whole`] and found.
    pub fn page_pdf(&self, k: usize, read: ReadFile<'_>) -> Option<(Vec<u8>, bool)> {
        let page = self.page(k)?;
        let mut whole = page.whole();
        let mut seen = alloc::collections::BTreeSet::new();
        let mut todo: Vec<i32> = page.forms().collect();
        while let Some(n) = todo.pop() {
            if !seen.insert(n) {
                continue;
            }
            match self.forms.get(&n) {
                Some(f) => {
                    whole &= f.whole();
                    todo.extend(f.forms());
                }
                None => whole = false,
            }
        }
        let pdf = page_pdf(page, &|n| self.forms.get(&n).cloned(), read);
        Some((pdf, whole))
    }
}

/// How [`page_pdf`] reads a file its fonts need (a Type 1 program, an
/// encoding): its bytes, or `None`.
pub type ReadFile<'a> = &'a mut dyn FnMut(&[u8], FileKind) -> Option<Arc<[u8]>>;

/// Forms drawn inside forms, at most this deep (as `phitex-draw` draws them).
const DEPTH: u32 = 16;

/// A PDF of page `page` alone: its stream, the forms it draws (`form`
/// finds a form's stream by its object number; one not found is left
/// out), its JPEG images, and its fonts, each with the whole program its
/// map entry names and its encoding, read through `read` (a font whose
/// program cannot be read has its widths only). A form stream given as
/// `page` is drawn as a page of its box.
#[must_use]
pub fn page_pdf(
    page: &ShippedStream,
    form: &dyn Fn(i32) -> Option<ShippedStream>,
    read: ReadFile<'_>,
) -> Vec<u8> {
    let mut w = Writer {
        uni: page.2.clone(),
        out: b"%PDF-1.5\n%\xd0\xd4\xc5\xd8\n".to_vec(),
        at: Vec::new(),
        fonts: BTreeMap::new(),
        forms: BTreeMap::new(),
        images: BTreeMap::new(),
        form,
        read,
    };
    let (catalog, pages, page_obj, contents) = (w.alloc(), w.alloc(), w.alloc(), w.alloc());
    w.obj(
        catalog,
        format!("<< /Type /Catalog /Pages {pages} 0 R >>").as_bytes(),
    );
    w.obj(
        pages,
        format!("<< /Type /Pages /Kids [{page_obj} 0 R] /Count 1 >>").as_bytes(),
    );
    let s = &page.0;
    w.stream(contents, b"", &s.bytes);
    let res = w.resources(s, 0);
    let (media, rotate) = s.media();
    let mut d = format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [{} {} {} {}] /Contents {contents} 0 R /Resources ",
        media[0], media[1], media[2], media[3]
    )
    .into_bytes();
    d.extend_from_slice(&res);
    if rotate != 0 {
        d.extend_from_slice(format!(" /Rotate {rotate}").as_bytes());
    }
    d.extend_from_slice(b" >>");
    w.obj(page_obj, &d);
    w.finish(catalog)
}

struct Writer<'a, 'r> {
    out: Vec<u8>,
    /// Each object's offset, by its number less one.
    at: Vec<usize>,
    /// The objects written, by what they are made from.
    fonts: BTreeMap<usize, u32>,
    forms: BTreeMap<i32, u32>,
    images: BTreeMap<usize, Option<u32>>,
    form: &'a dyn Fn(i32) -> Option<ShippedStream>,
    read: ReadFile<'r>,
    /// `/ToUnicode` maps made from these entries, as the job's fonts get.
    uni: Option<Uni>,
}

impl Writer<'_, '_> {
    /// A new object's number.
    fn alloc(&mut self) -> u32 {
        self.at.push(0);
        u32::try_from(self.at.len()).unwrap_or(u32::MAX)
    }

    fn obj(&mut self, n: u32, body: &[u8]) {
        self.at[n as usize - 1] = self.out.len();
        self.out
            .extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        self.out.extend_from_slice(body);
        self.out.extend_from_slice(b"\nendobj\n");
    }

    /// Object `n`, a stream of `data` with the entries `dict` (without
    /// its `<<`, `>>` and `/Length`).
    fn stream(&mut self, n: u32, dict: &[u8], data: &[u8]) {
        let mut b = b"<< ".to_vec();
        b.extend_from_slice(dict);
        b.extend_from_slice(format!(" /Length {} >>\nstream\n", data.len()).as_bytes());
        b.extend_from_slice(data);
        b.extend_from_slice(b"\nendstream");
        self.obj(n, &b);
    }

    /// The cross-reference table and the trailer.
    fn finish(mut self, root: u32) -> Vec<u8> {
        let xref = self.out.len();
        let n = self.at.len() + 1;
        self.out
            .extend_from_slice(format!("xref\n0 {n}\n0000000000 65535 f \n").as_bytes());
        for a in &self.at {
            self.out
                .extend_from_slice(format!("{a:010} 00000 n \n").as_bytes());
        }
        self.out.extend_from_slice(
            format!("trailer\n<< /Size {n} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n")
                .as_bytes(),
        );
        self.out
    }

    /// Stream `s`'s resources dictionary, what it names written.
    fn resources(&mut self, s: &Shipped, depth: u32) -> Vec<u8> {
        let mut fonts = Vec::new();
        for (n, rec) in s.fonts.iter() {
            let o = self.font(rec);
            fonts.extend_from_slice(format!(" /F{n} {o} 0 R").as_bytes());
        }
        let mut xs = Vec::new();
        for &(n, k) in s.forms.iter() {
            if depth < DEPTH
                && let Some(o) = self.form_obj(k, depth + 1)
            {
                xs.extend_from_slice(format!(" /Fm{n} {o} 0 R").as_bytes());
            }
        }
        for (n, _, im) in s.images.iter() {
            if let Some(o) = self.image(im) {
                xs.extend_from_slice(format!(" /Im{n} {o} 0 R").as_bytes());
            }
        }
        let mut d = b"<< /Font <<".to_vec();
        d.extend_from_slice(&fonts);
        d.extend_from_slice(b" >> /XObject <<");
        d.extend_from_slice(&xs);
        d.extend_from_slice(b" >> >>");
        d
    }

    /// Form `k`'s object (written once: one that draws itself names its
    /// own object).
    fn form_obj(&mut self, k: i32, depth: u32) -> Option<u32> {
        if let Some(&o) = self.forms.get(&k) {
            return Some(o);
        }
        let f = (self.form)(k)?;
        let o = self.alloc();
        self.forms.insert(k, o);
        let s = &f.0;
        let res = self.resources(s, depth);
        let mut d = b"/Type /XObject /Subtype /Form /BBox [".to_vec();
        d.extend_from_slice(&s.bbox);
        d.extend_from_slice(b"] /FormType 1 /Matrix [1 0 0 1 0 0] /Resources ");
        d.extend_from_slice(&res);
        self.stream(o, &d, &s.bytes);
        Some(o)
    }

    /// A JPEG image's object (the others, PNG files and PDF pages, are
    /// left out: the viewer counts them as not drawn until the PDF is in).
    fn image(&mut self, im: &Arc<Image>) -> Option<u32> {
        let at = Arc::as_ptr(im) as usize;
        if let Some(&o) = self.images.get(&at) {
            return o;
        }
        let o = (im.pdf.is_none() && !im.png).then(|| {
            let o = self.alloc();
            let cs = match im.color_space {
                crate::pdf::image::JPG_GRAY => "/DeviceGray",
                crate::pdf::image::JPG_RGB => "/DeviceRGB",
                _ => "/DeviceCMYK /Decode [1 0 1 0 1 0 1 0]",
            };
            let d = format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} /BitsPerComponent {} \
                 /ColorSpace {cs} /Filter /DCTDecode",
                im.width, im.height, im.bits
            );
            self.stream(o, d.as_bytes(), &im.data);
            o
        });
        self.images.insert(at, o);
        o
    }

    /// PDF font `rec`'s object: its widths, encoding and whole program.
    fn font(&mut self, rec: &Arc<FontRec>) -> u32 {
        let at = Arc::as_ptr(rec) as usize;
        if let Some(&o) = self.fonts.get(&at) {
            return o;
        }
        let o = self.alloc();
        self.fonts.insert(at, o);
        let map = rec.map.as_deref();
        let name = map
            .and_then(|m| m.ps_name.clone())
            .unwrap_or_else(|| rec.tfm.clone());
        let mut d = b"<< /Type /Font /Subtype /Type1 /BaseFont /".to_vec();
        d.extend_from_slice(&name);
        d.extend_from_slice(b" /FirstChar 0 /LastChar 255 /Widths [");
        for (i, &w) in rec.widths.iter().enumerate() {
            if i > 0 {
                d.push(b' ');
            }
            // (ten-thousandths of the size, as pdfTeX prints them)
            d.extend_from_slice(format!("{}", f64::from(w) / 10.0).as_bytes());
        }
        d.push(b']');
        // (the map entry's encoding, else the program's own)
        let encname = map.and_then(|m| m.encname.clone());
        let names = encname
            .clone()
            .and_then(|e| (self.read)(&e, FileKind::Enc))
            .and_then(|e| crate::pdf::enc::parse_enc(&e).ok());
        let file = map
            .and_then(|m| m.ff_name.clone())
            .and_then(|f| (self.read)(&f, FileKind::Type1));
        // (the job's `/ToUnicode`: from the encoding's glyph names, else the
        // program's own encoding's, `write_fontdictionary`)
        if let Some(uni) = &self.uni {
            let glyphs = match (&encname, &names) {
                (Some(_), Some(n)) => Some(n.clone()),
                (Some(_), None) => None,
                (None, _) => file
                    .as_deref()
                    .and_then(crate::pdf::writet1::builtin_encoding),
            };
            if let Some(g) = glyphs {
                let text =
                    crate::pdf::tounicode::cmap_for(&g, &rec.tfm, encname.as_deref(), &uni.0);
                let t = self.alloc();
                self.stream(t, b"", &text);
                d.extend_from_slice(format!(" /ToUnicode {t} 0 R").as_bytes());
            }
        }
        if let Some(names) = names {
            d.extend_from_slice(b" /Encoding << /Type /Encoding /Differences [0");
            for n in &names {
                d.extend_from_slice(b" /");
                d.extend_from_slice(n);
            }
            d.extend_from_slice(b"] >>");
        }
        let program = file.and_then(|p| type1_parts(&p));
        if let Some((data, len1)) = program {
            let (fd, ff) = (self.alloc(), self.alloc());
            d.extend_from_slice(format!(" /FontDescriptor {fd} 0 R").as_bytes());
            let mut b = b"<< /Type /FontDescriptor /FontName /".to_vec();
            b.extend_from_slice(&name);
            b.extend_from_slice(format!(" /Flags 4 /FontFile {ff} 0 R >>").as_bytes());
            self.obj(fd, &b);
            let dict = format!("/Length1 {len1} /Length2 {} /Length3 0", data.len() - len1);
            self.stream(ff, dict.as_bytes(), &data);
        }
        d.extend_from_slice(b" >>");
        self.obj(o, &d);
        o
    }
}

/// A Type 1 program as a PDF embeds it (`/FontFile`): the cleartext part,
/// then the binary one, and the cleartext's length; from a `.pfb` file's
/// segments, or a `.pfa` file's text (its encrypted part from hex).
fn type1_parts(b: &[u8]) -> Option<(Vec<u8>, usize)> {
    let (mut clear, mut bin) = (Vec::new(), Vec::new());
    if b.first() == Some(&0x80) {
        let mut i = 0;
        while i + 6 <= b.len() && b[i] == 0x80 && b[i + 1] != 3 {
            let n = u32::from_le_bytes([b[i + 2], b[i + 3], b[i + 4], b[i + 5]]) as usize;
            let seg = b.get(i + 6..i + 6 + n)?;
            match b[i + 1] {
                1 if bin.is_empty() => clear.extend_from_slice(seg),
                2 => bin.extend_from_slice(seg),
                _ => {}
            }
            i += 6 + n;
        }
    } else {
        let e = b.windows(5).position(|w| w == b"eexec")? + 5;
        let e = e + b[e..]
            .iter()
            .take_while(|c| matches!(c, b'\r' | b'\n' | b' ' | b'\t'))
            .count();
        clear.extend_from_slice(&b[..e]);
        let mut half = None;
        for &c in &b[e..] {
            let v = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                c if c.is_ascii_whitespace() => continue,
                _ => break,
            };
            match half.take() {
                Some(h) => bin.push((h << 4) | v),
                None => half = Some(v),
            }
        }
    }
    if clear.is_empty() || bin.is_empty() {
        return None;
    }
    let len1 = clear.len();
    clear.extend_from_slice(&bin);
    Some((clear, len1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page's own PDF reads as one page of its box, its stream as it
    /// was shipped, a font without a program (no map entry) its widths
    /// only, a form not found left out; a form given draws inside it.
    #[test]
    fn a_page_as_a_pdf() {
        use alloc::string::String;
        use partex_engine::pdfread::{Doc, Obj};
        let page = ShippedStream(Arc::new(Shipped::example()), true, None);
        let mut asked = Vec::new();
        let pdf: Arc<[u8]> = page_pdf(&page, &|_| None, &mut |n, _| {
            asked.push(n.to_vec());
            None
        })
        .into();
        assert_eq!(asked, Vec::<Vec<u8>>::new());
        let doc = Doc::open(&pdf).unwrap();
        assert_eq!(doc.num_pages(), 1);
        let p = doc.page(1).unwrap();
        assert_eq!(p.media, [0.0, 0.0, 612.0, 792.0]);
        let text = String::from_utf8_lossy(&pdf).into_owned();
        assert!(
            text.contains("stream\nBT /F1 10 Tf (a) Tj ET\nendstream"),
            "{text}"
        );
        assert!(text.contains("/BaseFont /cmr10 /FirstChar 0 /LastChar 255 /Widths [0 500]"));
        assert!(text.contains("/XObject << >>"), "{text}");
        // (with its form: drawn through the page's `/Fm1`)
        let mut form = Shipped::example();
        form.form = 12;
        form.forms = alloc::vec![].into();
        form.bbox = b"0 0 10 10".to_vec();
        let form = ShippedStream(Arc::new(form), true, None);
        let pdf: Arc<[u8]> = page_pdf(&page, &|k| (k == 12).then(|| form.clone()), &mut |_, _| {
            None
        })
        .into();
        let doc = Doc::open(&pdf).unwrap();
        let res = doc.page(1).unwrap().resources.unwrap();
        let Some(Obj::Dict(x)) = res.get(b"XObject") else {
            panic!("no XObject in {res:?}");
        };
        let Some(Obj::Ref(r)) = x.get(b"Fm1") else {
            panic!("no /Fm1 in {x:?}");
        };
        assert!(matches!(doc.fetch(*r), Obj::Stream(_)));
    }

    #[test]
    fn type1_programs_from_files() {
        let mut pfb = alloc::vec![0x80, 1, 5, 0, 0, 0];
        pfb.extend_from_slice(b"clear");
        pfb.extend_from_slice(&[0x80, 2, 3, 0, 0, 0, 1, 2, 3, 0x80, 3]);
        assert_eq!(type1_parts(&pfb), Some((b"clear\x01\x02\x03".to_vec(), 5)));
        let pfa = b"%!PS currentfile eexec\n0aFf 10\n0000\ncleartomark";
        assert_eq!(
            type1_parts(pfa),
            Some((b"%!PS currentfile eexec\n\x0a\xff\x10\x00\x00".to_vec(), 23))
        );
    }
}
