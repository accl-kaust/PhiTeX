//! A PDF page read back into the viewer's draw list (v2): what the page
//! draws, for the browser to draw itself (each glyph from the PDF's own
//! embedded Type 1 outlines, at TeX's position, with the text over it for
//! selecting; paths for `TikZ`). No PDF renderer, no raster.
//!
//! Read through `partex_engine::pdfread`, so any PDF pdfTeX writes reads
//! (compressed or not, object streams, cross-reference streams): the page
//! tree; each page's content streams (text, paths, colours, the graphics
//! state; not images, shadings, clips or forms yet); its fonts' `/Widths`,
//! `/ToUnicode` maps and `/FontFile` programs.
//!
//! JSON (`"v":2`), in PDF points from the page's top left:
//! `{"v":2,"w","h","f":[font keys],"F":[outline fonts],"g":{"F:code":d},
//! "t":[[font, size, y, "x x ...", text, outlined?, colour?] | [-1, size,
//! y, "x x ...", "", outline font, [codes], colour?, [a,b,c,d]?]],
//! "p":[[d, fill, stroke, width]],"r":[]}`: a text run's glyphs, each at
//! its own x; a path's SVG `d` with its paint (`null`: none). The viewer's
//! `page2.ts` (`Draws2`) is the reader.

// (geometry in f64 rounded to integers and indices, and the short names of
// the PDF and Type 1 operators' operands, as the specifications write them)
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use partex_engine::pdfread;
use std::fmt::Write as _;

use crate::esc;

#[derive(Clone, Debug)]
enum O {
    Num(f64),
    Name(String),
    Str(Vec<u8>),
    Arr(Vec<O>),
    Dict(Vec<(String, O)>),
    Ref(u32),
    Op(String),
    Null,
}

impl O {
    fn get(&self, k: &str) -> Option<&O> {
        match self {
            O::Dict(d) => d.iter().find(|(n, _)| n == k).map(|(_, v)| v),
            _ => None,
        }
    }
    fn num(&self) -> Option<f64> {
        if let O::Num(n) = self { Some(*n) } else { None }
    }
}

struct Lex<'a> {
    b: &'a [u8],
    i: usize,
}

fn white(c: u8) -> bool {
    matches!(c, b' ' | b'\n' | b'\r' | b'\t' | b'\x0c' | 0)
}
fn delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

impl Lex<'_> {
    fn skip(&mut self) {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if white(c) {
                self.i += 1;
            } else if c == b'%' {
                while self.i < self.b.len() && self.b[self.i] != b'\n' && self.b[self.i] != b'\r' {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    fn word(&mut self) -> &[u8] {
        let s = self.i;
        while self.i < self.b.len() && !white(self.b[self.i]) && !delim(self.b[self.i]) {
            self.i += 1;
        }
        &self.b[s..self.i]
    }

    /// The next object (an operator is an `O::Op`); `None` at the end.
    fn next(&mut self) -> Option<O> {
        self.skip();
        let c = *self.b.get(self.i)?;
        match c {
            b'/' => {
                self.i += 1;
                Some(O::Name(String::from_utf8_lossy(self.word()).into_owned()))
            }
            b'(' => {
                self.i += 1;
                let (mut depth, mut out) = (1, Vec::new());
                while self.i < self.b.len() {
                    let c = self.b[self.i];
                    self.i += 1;
                    match c {
                        b'\\' => {
                            let e = *self.b.get(self.i).unwrap_or(&b'\\');
                            self.i += 1;
                            match e {
                                b'n' => out.push(b'\n'),
                                b'r' => out.push(b'\r'),
                                b't' => out.push(b'\t'),
                                b'b' => out.push(8),
                                b'f' => out.push(12),
                                b'0'..=b'7' => {
                                    let mut v = u32::from(e - b'0');
                                    for _ in 0..2 {
                                        match self.b.get(self.i) {
                                            Some(d @ b'0'..=b'7') => {
                                                v = v * 8 + u32::from(d - b'0');
                                                self.i += 1;
                                            }
                                            _ => break,
                                        }
                                    }
                                    out.push(v as u8);
                                }
                                b'\r' | b'\n' => {}
                                e => out.push(e),
                            }
                        }
                        b'(' => {
                            depth += 1;
                            out.push(c);
                        }
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            out.push(c);
                        }
                        c => out.push(c),
                    }
                }
                Some(O::Str(out))
            }
            b'<' if self.b.get(self.i + 1) == Some(&b'<') => {
                self.i += 2;
                let mut d = Vec::new();
                loop {
                    self.skip();
                    if self.b.get(self.i..self.i + 2) == Some(b">>") {
                        self.i += 2;
                        break;
                    }
                    if let O::Name(k) = self.next()? {
                        let v = self.value()?;
                        d.push((k, v));
                    }
                }
                Some(O::Dict(d))
            }
            b'<' => {
                self.i += 1;
                let mut hex = Vec::new();
                while self.i < self.b.len() && self.b[self.i] != b'>' {
                    let c = self.b[self.i];
                    if c.is_ascii_hexdigit() {
                        hex.push(c);
                    }
                    self.i += 1;
                }
                self.i += 1;
                if hex.len() % 2 == 1 {
                    hex.push(b'0');
                }
                let v = hex
                    .chunks(2)
                    .map(|p| {
                        u8::from_str_radix(std::str::from_utf8(p).unwrap_or("0"), 16).unwrap_or(0)
                    })
                    .collect();
                Some(O::Str(v))
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                loop {
                    self.skip();
                    if self.b.get(self.i) == Some(&b']') {
                        self.i += 1;
                        break;
                    }
                    a.push(self.value()?);
                }
                Some(O::Arr(a))
            }
            b']' | b'>' | b')' | b'{' | b'}' => {
                self.i += 1;
                Some(O::Op(String::from(c as char)))
            }
            _ => {
                let w = self.word();
                if w.is_empty() {
                    self.i += 1;
                    return Some(O::Null);
                }
                let s = String::from_utf8_lossy(w).into_owned();
                Some(match s.parse::<f64>() {
                    Ok(n) => O::Num(n),
                    Err(_) if s == "null" => O::Null,
                    Err(_) => O::Op(s),
                })
            }
        }
    }

    /// An object as a value: `n g R` is a reference.
    fn value(&mut self) -> Option<O> {
        let o = self.next()?;
        if let O::Num(n) = o {
            let save = self.i;
            if let Some(O::Num(_)) = self.next()
                && let Some(O::Op(r)) = self.next()
                && r == "R"
            {
                return Some(O::Ref(n as u32));
            }
            self.i = save;
        }
        Some(o)
    }
}

/// An open PDF: [`pdfread`](partex_engine::pdfread)'s, which reads any
/// PDF pdfTeX or xdvipdfmx writes (compressed streams, object streams,
/// cross-reference streams), its objects given to the walk below as
/// [`O`]s. Opened once, drawn from page by page.
pub struct Pdf {
    doc: pdfread::Doc,
}

/// An object number as a reference (pdfTeX's and xdvipdfmx's objects are
/// all of generation 0).
fn rf(n: u32) -> pdfread::Ref {
    pdfread::Ref {
        num: i32::try_from(n).unwrap_or(i32::MAX),
        generation: 0,
    }
}

/// `o` as the walk reads it (a stream as its dictionary).
fn conv(o: &pdfread::Obj) -> O {
    use pdfread::Obj;
    let name = |n: &[u8]| String::from_utf8_lossy(n).into_owned();
    let dict = |d: &pdfread::Dict| O::Dict(d.0.iter().map(|(k, v)| (name(k), conv(v))).collect());
    match o {
        Obj::Int(i) => O::Num(f64::from(*i)),
        Obj::Real(r) => O::Num(*r),
        Obj::Str(s) => O::Str(s.clone()),
        Obj::Name(n) => O::Name(name(n)),
        Obj::Array(a) => O::Arr(a.iter().map(conv).collect()),
        Obj::Dict(d) => dict(d),
        Obj::Stream(s) => dict(&s.dict),
        Obj::Ref(r) => O::Ref(u32::try_from(r.num).unwrap_or(0)),
        Obj::Cmd(c) => O::Op(name(c)),
        Obj::Null | Obj::Bool(_) | Obj::Error | Obj::Eof => O::Null,
    }
}

impl Pdf {
    /// Open `data` (`None`: not a PDF that can be read).
    #[must_use]
    pub fn open(data: &Arc<[u8]>) -> Option<Self> {
        pdfread::Doc::open(data).ok().map(|doc| Pdf { doc })
    }

    /// How many pages it has.
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.doc.num_pages()
    }

    /// Object `n`, and where its stream's bytes are in the file, as they
    /// are there (encoded), if it is a stream.
    fn obj(&self, n: u32) -> Option<(O, Option<std::ops::Range<usize>>)> {
        match self.doc.fetch(rf(n)) {
            pdfread::Obj::Null => None,
            pdfread::Obj::Stream(s) => Some((
                conv(&pdfread::Obj::Dict(s.dict.clone())),
                Some(s.start..s.start + s.len),
            )),
            o => Some((conv(&o), None)),
        }
    }

    fn resolve(&self, o: &O) -> O {
        match o {
            O::Ref(r) => self.obj(*r).map_or(O::Null, |(o, _)| o),
            o => o.clone(),
        }
    }

    /// The decoded bytes of the stream `o` refers to.
    fn stream(&self, o: &O) -> Option<Vec<u8>> {
        let O::Ref(r) = o else { return None };
        match self.doc.fetch(rf(*r)) {
            pdfread::Obj::Stream(s) => Some(self.doc.decode(&s)),
            _ => None,
        }
    }

    /// Page `k` (from 0): its dictionary, with the resources and media box
    /// it inherits.
    fn page(&self, k: usize) -> Option<O> {
        let pg = self.doc.page(k + 1)?;
        let O::Dict(mut d) = conv(&self.doc.fetch(pg.dict_ref)) else {
            return None;
        };
        d.retain(|(k, _)| k != "Resources" && k != "MediaBox");
        if let Some(r) = &pg.resources {
            d.push(("Resources".into(), conv(&pdfread::Obj::Dict(r.clone()))));
        }
        d.push((
            "MediaBox".into(),
            O::Arr(pg.media.iter().map(|&v| O::Num(v)).collect()),
        ));
        Some(O::Dict(d))
    }

    /// The pages, in order: each page's dictionary.
    fn pages(&self) -> Vec<O> {
        (0..self.page_count())
            .filter_map(|k| self.page(k))
            .collect()
    }

    /// Image `XObject` `r` (with resources `res`, for a named colour
    /// space), and its soft mask.
    fn image_ref(
        &self,
        r: u32,
        res: &O,
    ) -> Option<(crate::image::Image, Option<crate::image::Image>)> {
        let pdfread::Obj::Stream(st) = self.doc.fetch(rf(r)) else {
            return None;
        };
        let dict = conv(&pdfread::Obj::Dict(st.dict.clone()));
        let raw = self.doc.raw(&st).to_vec();
        let samples = Some(self.doc.decode(&st)).filter(|s| !s.is_empty());
        let img = self.image(&dict, raw, samples, res)?;
        let smask = match dict.get("SMask") {
            Some(O::Ref(m)) => self.image_ref(*m, res).map(|(i, _)| i),
            _ => None,
        };
        Some((img, smask))
    }

    /// An inline image: dictionary `d` (abbreviations allowed), bytes `raw`.
    fn image_inline(
        &self,
        d: &O,
        raw: Vec<u8>,
        res: &O,
    ) -> Option<(crate::image::Image, Option<crate::image::Image>)> {
        fn expand(o: &O) -> O {
            let name = |n: &str| -> String {
                match n {
                    "AHx" => "ASCIIHexDecode",
                    "A85" => "ASCII85Decode",
                    "LZW" => "LZWDecode",
                    "Fl" => "FlateDecode",
                    "RL" => "RunLengthDecode",
                    "DCT" => "DCTDecode",
                    "CCF" => "CCITTFaxDecode",
                    "G" => "DeviceGray",
                    "RGB" => "DeviceRGB",
                    "CMYK" => "DeviceCMYK",
                    "I" => "Indexed",
                    n => n,
                }
                .to_owned()
            };
            match o {
                O::Name(n) => O::Name(name(n)),
                O::Arr(a) => O::Arr(a.iter().map(expand).collect()),
                o => o.clone(),
            }
        }
        fn to_obj(o: &O) -> pdfread::Obj {
            match o {
                O::Num(n) if n.fract() == 0.0 => pdfread::Obj::Int(*n as i32),
                O::Num(n) => pdfread::Obj::Real(*n),
                O::Name(n) => pdfread::Obj::Name(n.as_bytes().to_vec()),
                O::Str(s) => pdfread::Obj::Str(s.clone()),
                O::Arr(a) => pdfread::Obj::Array(a.iter().map(to_obj).collect()),
                O::Dict(d) => pdfread::Obj::Dict(pdfread::Dict(
                    d.iter()
                        .map(|(k, v)| (k.as_bytes().to_vec(), to_obj(v)))
                        .collect(),
                )),
                _ => pdfread::Obj::Null,
            }
        }
        let O::Dict(entries) = d else { return None };
        let long = |k: &str| -> String {
            match k {
                "W" => "Width",
                "H" => "Height",
                "BPC" => "BitsPerComponent",
                "CS" => "ColorSpace",
                "F" => "Filter",
                "DP" => "DecodeParms",
                "IM" => "ImageMask",
                "D" => "Decode",
                "I" => "Interpolate",
                k => k,
            }
            .to_owned()
        };
        let dict = O::Dict(entries.iter().map(|(k, v)| (long(k), expand(v))).collect());
        let f = dict.get("Filter").map_or(pdfread::Obj::Null, to_obj);
        let parms = dict.get("DecodeParms").map_or(pdfread::Obj::Null, to_obj);
        let samples = Some(self.doc.decode_with(&raw, &f, &parms)).filter(|s| !s.is_empty());
        Some((self.image(&dict, raw, samples, res)?, None))
    }

    /// An image from its dictionary, bytes and samples.
    fn image(
        &self,
        dict: &O,
        raw: Vec<u8>,
        samples: Option<Vec<u8>>,
        res: &O,
    ) -> Option<crate::image::Image> {
        let num = |k: &str| dict.get(k).map(|o| self.resolve(o)).and_then(|o| o.num());
        let mask = matches!(dict.get("ImageMask"), Some(O::Op(t)) if t == "true");
        let space = if mask {
            crate::image::Space::Gray
        } else {
            self.space(&self.resolve(dict.get("ColorSpace")?), res, 0)?
        };
        let filters = match dict.get("Filter").map(|f| self.resolve(f)) {
            Some(O::Name(n)) => vec![n],
            Some(O::Arr(a)) => a
                .iter()
                .filter_map(|f| {
                    if let O::Name(n) = self.resolve(f) {
                        Some(n)
                    } else {
                        None
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        let parms = match dict.get("DecodeParms").map(|d| self.resolve(d)) {
            Some(O::Arr(a)) => a.first().map_or(O::Null, |d| self.resolve(d)),
            Some(d) => d,
            None => O::Null,
        };
        let pn = |k: &str, v: f64| parms.get(k).and_then(O::num).unwrap_or(v) as i64;
        let bpc = if mask {
            1
        } else {
            num("BitsPerComponent").unwrap_or(8.0) as usize
        };
        Some(crate::image::Image {
            width: num("Width")? as usize,
            height: num("Height")? as usize,
            bpc,
            space,
            decode: match dict.get("Decode").map(|d| self.resolve(d)) {
                Some(O::Arr(a)) => Some(a.iter().map(|x| x.num().unwrap_or(0.0)).collect()),
                _ => None,
            },
            mask,
            raw,
            filters,
            predictor: (
                pn("Predictor", 1.0),
                pn("Colors", 1.0),
                pn("Columns", 1.0),
                pn("BitsPerComponent", 8.0),
            ),
            samples,
        })
    }

    /// Colour space `cs` (a name in `res`'s `/ColorSpace` too), as an
    /// image's samples read; none for one not drawn (Lab, `DeviceN`,
    /// `Separation`, a pattern).
    fn space(&self, cs: &O, res: &O, depth: u32) -> Option<crate::image::Space> {
        use crate::image::Space;
        if depth > 8 {
            return None;
        }
        match cs {
            O::Name(n) => match n.as_str() {
                "DeviceGray" | "CalGray" | "G" => Some(Space::Gray),
                "DeviceRGB" | "CalRGB" | "RGB" => Some(Space::Rgb),
                "DeviceCMYK" | "CMYK" => Some(Space::Cmyk),
                n => {
                    let named = res
                        .get("ColorSpace")
                        .map(|c| self.resolve(c))?
                        .get(n)
                        .map(|c| self.resolve(c))?;
                    self.space(&named, res, depth + 1)
                }
            },
            O::Arr(a) => {
                let kind = match a.first() {
                    Some(O::Name(k)) => k.as_str(),
                    _ => return None,
                };
                match kind {
                    "CalGray" => Some(Space::Gray),
                    "CalRGB" => Some(Space::Rgb),
                    "ICCBased" => {
                        let d = self.resolve(a.get(1)?);
                        match d.get("N").and_then(O::num) {
                            Some(n) if (n - 1.0).abs() < 1e-9 => Some(Space::Gray),
                            Some(n) if (n - 3.0).abs() < 1e-9 => Some(Space::Rgb),
                            Some(n) if (n - 4.0).abs() < 1e-9 => Some(Space::Cmyk),
                            _ => self.space(&self.resolve(d.get("Alternate")?), res, depth + 1),
                        }
                    }
                    "Indexed" | "I" => {
                        let base = self.space(&self.resolve(a.get(1)?), res, depth + 1)?;
                        let hival = self.resolve(a.get(2)?).num()? as usize;
                        let table = match a.get(3)? {
                            O::Str(s) => s.clone(),
                            r @ O::Ref(_) => match self.resolve(r) {
                                O::Str(s) => s,
                                _ => self.stream(r)?,
                            },
                            _ => return None,
                        };
                        let n = base.n();
                        let pal = (0..=hival)
                            .map(|i| {
                                let e = table.get(i * n..(i + 1) * n).unwrap_or(&[]);
                                let at = |k: usize| e.get(k).copied().unwrap_or(0);
                                match base {
                                    Space::Gray => [at(0), at(0), at(0)],
                                    Space::Cmyk => {
                                        crate::image::cmyk(&[at(0), at(1), at(2), at(3)])
                                    }
                                    _ => [at(0), at(1), at(2)],
                                }
                            })
                            .collect();
                        Some(Space::Indexed(pal))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// A font as the page uses it: its TeX name, widths, and codes as text.
struct Font {
    key: usize,
    first: i64,
    widths: Vec<f64>,
    uni: HashMap<u32, String>,
    /// Computer Modern's math extension font: its big glyphs are drawn at their size.
    ex: bool,
    /// The embedded Type 1 program and the code → glyph name the PDF uses
    /// (the font's encoding with the font dictionary's /Differences): the
    /// glyphs are drawn from their outlines.
    outlines: Option<(Arc<crate::type1::Type1>, Vec<Option<String>>)>,
    /// The font's id on the page (`F`): its outlines' ids.
    fref: usize,
    /// A Type 0 font (`XeTeX`'s native fonts, through xdvipdfmx): its glyphs
    /// come from the glyph runs (`xetex::extra`), not from here.
    cid: bool,
}

/// The CSS face a TeX font is drawn in (`panel.ts` maps these to Latin Modern).
fn face(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    let n = n.split('+').next_back().unwrap_or(&n);
    if n.contains("tt") || n.starts_with("lmmono") {
        return "mono";
    }
    if n.starts_with("cmsy")
        || n.starts_with("cmex")
        || n.starts_with("msa")
        || n.starts_with("msb")
        || n.starts_with("lmsy")
        || n.starts_with("lmex")
        || n.contains("math")
    {
        return "math";
    }
    let bold = n.contains("bx") || n.starts_with("cmb") || n.contains("bold") || n.contains("-b");
    let italic = n.contains("ti")
        || n.contains("it")
        || n.contains("mi")
        || n.contains("sl")
        || n.contains("italic")
        || n.contains("oblique");
    match (bold, italic) {
        (true, true) => "bolditalic",
        (true, false) => "bold",
        (false, true) => "italic",
        _ => "roman",
    }
}

/// A `/ToUnicode` `CMap`'s `bfchar` and `bfrange` entries.
fn cmap(b: &[u8]) -> HashMap<u32, String> {
    let mut m = HashMap::new();
    let mut l = Lex { b, i: 0 };
    let hex = |o: &O| -> Option<Vec<u8>> {
        if let O::Str(s) = o {
            Some(s.clone())
        } else {
            None
        }
    };
    let code = |s: &[u8]| s.iter().fold(0u32, |a, &c| (a << 8) | u32::from(c));
    let text = |s: &[u8]| {
        let u: Vec<u16> = s
            .chunks(2)
            .map(|p| u16::from_be_bytes([p[0], *p.get(1).unwrap_or(&0)]))
            .collect();
        String::from_utf16_lossy(&u)
    };
    let mut toks = Vec::new();
    while let Some(o) = l.value() {
        toks.push(o);
    }
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            O::Op(s) if s == "beginbfchar" => {
                i += 1;
                while i + 1 < toks.len() && !matches!(&toks[i], O::Op(s) if s == "endbfchar") {
                    if let (Some(a), Some(b)) = (hex(&toks[i]), hex(&toks[i + 1])) {
                        m.insert(code(&a), text(&b));
                    }
                    i += 2;
                }
            }
            O::Op(s) if s == "beginbfrange" => {
                i += 1;
                while i + 2 < toks.len() && !matches!(&toks[i], O::Op(s) if s == "endbfrange") {
                    if let (Some(a), Some(b)) = (hex(&toks[i]), hex(&toks[i + 1])) {
                        let (a, b) = (code(&a), code(&b));
                        match &toks[i + 2] {
                            O::Str(d) => {
                                let base = text(d);
                                let mut u: Vec<u16> = base.encode_utf16().collect();
                                for c in a..=b.min(a + 0xffff) {
                                    m.insert(c, String::from_utf16_lossy(&u));
                                    if let Some(last) = u.last_mut() {
                                        *last = last.wrapping_add(1);
                                    }
                                }
                            }
                            O::Arr(v) => {
                                for (k, d) in v.iter().enumerate() {
                                    if let Some(d) = hex(d) {
                                        m.insert(a + k as u32, text(&d));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    i += 3;
                }
            }
            _ => i += 1,
        }
    }
    m
}

type M = [f64; 6];
fn mul(a: &M, b: &M) -> M {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}
fn apply(m: &M, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

#[derive(Clone)]
struct G {
    ctm: M,
    fill: String,
    stroke: String,
    lw: f64,
    /// (the clip in force: its index in `Ctx::clips`)
    clip: Option<usize>,
    /// (the font ref)
    font: Option<usize>,
    size: f64,
    tc: f64,
    tw: f64,
    tz: f64,
    tl: f64,
    rise: f64,
}

impl G {
    /// The state a page starts in.
    fn new() -> Self {
        G {
            ctm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            fill: "#000000".into(),
            stroke: "#000000".into(),
            lw: 1.0,
            clip: None,
            font: None,
            size: 10.0,
            tc: 0.0,
            tw: 0.0,
            tz: 100.0,
            tl: 0.0,
            rise: 0.0,
        }
    }
}

fn rgb(r: f64, g: f64, b: f64) -> String {
    let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", c(r), c(g), c(b))
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// What drawing pages keeps across pages and builds (a new PDF's fonts
/// and images are mostly the last one's): parsed font programs, by a hash
/// of the font file's bytes, so drawing a page parses only fonts it has
/// not seen; images' data URIs, by a hash of their bytes.
#[derive(Default)]
pub struct Fonts {
    programs: HashMap<u64, Option<Arc<crate::type1::Type1>>>,
    images: HashMap<u64, Option<Arc<str>>>,
}

impl Fonts {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// A page's hash, with where its content lies in the PDF (the streams'
/// byte ranges) and its size: what [`Pdf::hashes_since`] needs to keep it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageSum {
    ranges: Vec<std::ops::Range<usize>>,
    wh: (i64, i64),
    pub hash: u64,
}

/// What a page adds to the PDF's drawing (`XeTeX`'s glyph runs): fonts,
/// outlines and runs, as the draw list's JSON parts.
#[derive(Clone, Debug, Default)]
pub struct Extra {
    /// The outline fonts' names (`F`), after the PDF's own.
    pub fonts: Vec<String>,
    /// Outlines (`g`'s entries, without the braces).
    pub g: String,
    /// Runs (`t`'s entries, without the brackets).
    pub t: String,
}

impl Pdf {
    /// Each page's hash, the PDF's bytes before `first` unchanged since
    /// `prev` was made of it: a page whose size is the same and whose
    /// content streams lie where they did, all before `first`, keeps its
    /// hash (its bytes are the same); the others are hashed again. `first`
    /// None: every page.
    #[must_use]
    pub fn hashes_since(&self, prev: &[PageSum], first: Option<usize>) -> Vec<PageSum> {
        self.pages()
            .iter()
            .enumerate()
            .map(|(k, page)| {
                let refs: Vec<O> = match page.get("Contents") {
                    Some(O::Arr(a)) => a.clone(),
                    Some(c) => vec![c.clone()],
                    None => Vec::new(),
                };
                // (the XObjects' streams too: an image or form changed
                // under the same name changes the page)
                let xs = self.xobjects(page);
                let ranges: Vec<std::ops::Range<usize>> = refs
                    .iter()
                    .filter_map(|c| if let O::Ref(r) = c { Some(*r) } else { None })
                    .chain(xs.iter().copied())
                    .filter_map(|r| self.obj(r).and_then(|(_, s)| s))
                    .collect();
                let wh = size(page);
                let wh = (wh.0 as i64, wh.1 as i64);
                if let (Some(f), Some(old)) = (first, prev.get(k))
                    && old.wh == wh
                    && old.ranges == ranges
                    && ranges.iter().all(|r| r.end <= f)
                {
                    return old.clone();
                }
                PageSum {
                    hash: page_hash(self, page, &xs),
                    ranges,
                    wh,
                }
            })
            .collect()
    }

    /// The `XObject`s page `page` can paint (those its resources name, and
    /// theirs, and soft masks): their objects, each once.
    fn xobjects(&self, page: &O) -> Vec<u32> {
        fn walk(p: &Pdf, res: &O, seen: &mut Vec<u32>, depth: u32) {
            let O::Dict(xd) = res.get("XObject").map_or(O::Null, |x| p.resolve(x)) else {
                return;
            };
            for (_, r) in xd {
                let O::Ref(n) = r else { continue };
                if seen.contains(&n) {
                    continue;
                }
                seen.push(n);
                let x = p.resolve(&O::Ref(n));
                if let Some(O::Ref(m)) = x.get("SMask")
                    && !seen.contains(m)
                {
                    seen.push(*m);
                }
                if depth < 16
                    && let Some(r) = x.get("Resources")
                {
                    walk(p, &p.resolve(r), seen, depth + 1);
                }
            }
        }
        let mut seen = Vec::new();
        if let Some(r) = page.get("Resources") {
            walk(self, &self.resolve(r), &mut seen, 0);
        }
        seen
    }

    /// Where page `k` (from 0) shows each glyph, in the order glyph origins
    /// count them (`partex_core::srcmap`): x and y in PDF points from the
    /// page's top left, as the draw list's.
    #[must_use]
    pub fn glyph_places(&self, k: usize) -> Vec<(f64, f64)> {
        let Some(pg) = self.doc.page(k + 1) else {
            return Vec::new();
        };
        let [x0, _, _, y1] = pg.media;
        partex_engine::pdftext::page_codes(&self.doc, &pg)
            .iter()
            .map(|s| (s.x - x0, y1 - s.y))
            .collect()
    }

    /// Each page's hash (its content and size): cheap, no drawing.
    #[must_use]
    pub fn hashes(&self) -> Vec<u64> {
        self.pages()
            .iter()
            .map(|page| page_hash(self, page, &self.xobjects(page)))
            .collect()
    }

    /// Page `k`'s draw list (v2), its fonts parsed through `fonts`.
    pub fn draw(&self, k: usize, fonts: &mut Fonts) -> Option<String> {
        self.draw_with(k, fonts, None)
    }

    /// Page `k`'s draw list with what `extra` adds (`XeTeX`'s glyph runs:
    /// given the first font ref free and the page's height).
    pub fn draw_with(
        &self,
        k: usize,
        fonts: &mut Fonts,
        extra: Option<&mut dyn FnMut(usize, f64) -> Extra>,
    ) -> Option<String> {
        let page = self.page(k)?;
        Some(draw_with(self, &page, fonts, extra))
    }
}

/// Each page's hash (its content and size) of PDF `pdf`.
#[must_use]
pub fn hashes(pdf: &Arc<[u8]>) -> Vec<u64> {
    Pdf::open(pdf).map(|p| p.hashes()).unwrap_or_default()
}

/// Page `k`'s draw list (v2) of PDF `pdf`, its fonts parsed through `fonts`.
pub fn page(pdf: &Arc<[u8]>, k: usize, fonts: &mut Fonts) -> Option<String> {
    Pdf::open(pdf)?.draw(k, fonts)
}

/// A page's width and height (its media box).
fn size(page: &O) -> (f64, f64) {
    let media = match page.get("MediaBox") {
        Some(O::Arr(a)) if a.len() == 4 => {
            a.iter().map(|o| o.num().unwrap_or(0.0)).collect::<Vec<_>>()
        }
        _ => vec![0.0, 0.0, 612.0, 792.0],
    };
    (media[2] - media[0], media[3] - media[1])
}

/// A page's hash: its content, its size, and the bytes of the `XObject`s
/// `xs` it can paint, as they are in the file.
fn page_hash(p: &Pdf, page: &O, xs: &[u32]) -> u64 {
    use std::hash::{Hash, Hasher};
    let (w, h) = size(page);
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    page_content(p, page).hash(&mut hs);
    (w as i64, h as i64).hash(&mut hs);
    for &n in xs {
        if let pdfread::Obj::Stream(s) = p.doc.fetch(rf(n)) {
            p.doc.raw(&s).hash(&mut hs);
        }
    }
    hs.finish()
}

/// A page's content: its streams joined.
fn page_content(p: &Pdf, page: &O) -> Vec<u8> {
    let mut content = Vec::new();
    match page.get("Contents") {
        Some(O::Arr(a)) => {
            for c in a {
                if let Some(s) = p.stream(c) {
                    content.extend_from_slice(&s);
                    content.push(b'\n');
                }
            }
        }
        Some(c) => {
            if let Some(s) = p.stream(c) {
                content.extend_from_slice(&s);
            }
        }
        None => {}
    }
    content
}

/// A page being drawn: the fonts of its resources and of the forms it
/// draws (`F`, by font ref), and what it draws so far.
struct Ctx<'a> {
    p: &'a Pdf,
    cache: &'a mut Fonts,
    /// (`XeTeX`: the glyphs come from the glyph runs)
    runs_only: bool,
    page_h: f64,
    keys: Vec<&'static str>,
    frefs: Vec<String>,
    fonts: Vec<Font>,
    /// (fonts by their dictionary's object: one font ref for a font the
    /// page and its forms share)
    by_obj: HashMap<u32, usize>,
    /// (the glyphs drawn from outlines: font ref and code)
    used: BTreeSet<(usize, u8)>,
    text: String,
    paths: String,
    out: Out,
    /// (clips, `C`: the path, even-odd, the clip it is inside; each once)
    clips: Vec<(String, bool, Option<usize>)>,
    clip_ids: HashMap<(String, bool, Option<usize>), usize>,
}

impl Ctx<'_> {
    /// Clip path `d` (page coordinates; even-odd or not) inside clip
    /// `parent`: its index.
    fn clip(&mut self, d: String, evenodd: bool, parent: Option<usize>) -> usize {
        let key = (d, evenodd, parent);
        if let Some(&c) = self.clip_ids.get(&key) {
            return c;
        }
        self.clips.push(key.clone());
        self.clip_ids.insert(key, self.clips.len() - 1);
        self.clips.len() - 1
    }

    /// The fonts resources `res` name: name to font ref.
    fn fonts_of(&mut self, res: &O) -> HashMap<String, usize> {
        let p = self.p;
        let mut named = HashMap::new();
        if let O::Dict(fd) = res.get("Font").map_or(O::Null, |f| p.resolve(f)) {
            for (name, r) in fd {
                if let O::Ref(n) = r
                    && let Some(&fr) = self.by_obj.get(&n)
                {
                    named.insert(name, fr);
                    continue;
                }
                let f = p.resolve(&r);
                let base = match f.get("BaseFont") {
                    Some(O::Name(n)) => n.clone(),
                    _ => String::new(),
                };
                let k = face(&base);
                let key = self.keys.iter().position(|x| *x == k).unwrap_or_else(|| {
                    self.keys.push(k);
                    self.keys.len() - 1
                });
                let first = f.get("FirstChar").and_then(O::num).unwrap_or(0.0) as i64;
                let widths = match f.get("Widths").map(|w| p.resolve(w)) {
                    Some(O::Arr(a)) => a.iter().map(|o| o.num().unwrap_or(0.0)).collect(),
                    _ => Vec::new(),
                };
                let uni = f
                    .get("ToUnicode")
                    .and_then(|t| p.stream(t))
                    .map(|b| cmap(&b))
                    .unwrap_or_default();
                let ex = base.to_ascii_uppercase().contains("CMEX");
                let outlines = (!self.runs_only)
                    .then(|| font_program(p, &f, self.cache))
                    .flatten()
                    .map(|t1| {
                        let mut names = t1.encoding.clone();
                        if let Some(e @ O::Dict(_)) = f.get("Encoding").map(|e| p.resolve(e))
                            && let Some(O::Arr(d)) = e.get("Differences").map(|d| p.resolve(d))
                        {
                            let mut code = 0usize;
                            for o in d {
                                match o {
                                    O::Num(n) => code = n as usize,
                                    O::Name(n) => {
                                        if code < names.len() {
                                            names[code] = Some(n.clone());
                                        }
                                        code += 1;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        (t1, names)
                    });
                let fref = self.fonts.len();
                self.frefs.push(base.clone());
                let cid =
                    self.runs_only || matches!(f.get("Subtype"), Some(O::Name(s)) if s == "Type0");
                if let O::Ref(n) = r {
                    self.by_obj.insert(n, fref);
                }
                named.insert(name, fref);
                self.fonts.push(Font {
                    key,
                    first,
                    widths,
                    uni,
                    ex,
                    outlines,
                    fref,
                    cid,
                });
            }
        }
        named
    }
}

fn draw_with(
    p: &Pdf,
    page: &O,
    programs: &mut Fonts,
    extra: Option<&mut dyn FnMut(usize, f64) -> Extra>,
) -> String {
    // (XeTeX: every glyph, TFM fonts' too, comes from the glyph runs; the
    // PDF gives the paths and rules only)
    let runs_only = extra.is_some();
    {
        let media = match page.get("MediaBox") {
            Some(O::Arr(a)) if a.len() == 4 => {
                a.iter().map(|o| o.num().unwrap_or(0.0)).collect::<Vec<_>>()
            }
            _ => vec![0.0, 0.0, 612.0, 792.0],
        };
        let (w, h) = (media[2] - media[0], media[3] - media[1]);
        let res = page.get("Resources").map_or(O::Null, |r| p.resolve(r));
        let mut ctx = Ctx {
            p,
            cache: programs,
            runs_only,
            page_h: h,
            keys: Vec::new(),
            frefs: Vec::new(),
            fonts: Vec::new(),
            by_obj: HashMap::new(),
            used: BTreeSet::new(),
            text: String::new(),
            paths: String::new(),
            out: Out::default(),
            clips: Vec::new(),
            clip_ids: HashMap::new(),
        };
        // fonts of the page
        let fonts = ctx.fonts_of(&res);
        // the content
        let content = page_content(p, page);
        interpret(&mut ctx, &content, &fonts, &res, G::new(), 0);
        let Ctx {
            keys,
            frefs,
            fonts,
            used,
            text: t,
            paths,
            mut out,
            clips,
            ..
        } = ctx;
        let f: Vec<String> = keys.iter().map(|k| esc(k)).collect();
        // (the outlines of the glyphs the page uses, by font and code)
        let mut g = String::new();
        for (fr, c) in used {
            let Some((t1, names)) = fonts.get(fr).and_then(|f| f.outlines.as_ref()) else {
                continue;
            };
            let Some(d) = names
                .get(usize::from(c))
                .cloned()
                .flatten()
                .and_then(|n| t1.path(&n))
            else {
                continue;
            };
            let _ = write!(
                g,
                "{}\"{fr}:{c}\":{}",
                if g.is_empty() { "" } else { "," },
                esc(&d)
            );
        }
        let mut fr: Vec<String> = frefs.iter().map(|b| esc(&ident(b))).collect();
        let mut t = t;
        if let Some(extra) = extra {
            let e = extra(fr.len(), h);
            fr.extend(e.fonts.iter().map(|f| esc(f)));
            if !e.g.is_empty() {
                if !g.is_empty() {
                    g.push(',');
                }
                g.push_str(&e.g);
            }
            if !e.t.is_empty() {
                if !t.is_empty() {
                    t.push(',');
                }
                t.push_str(&e.t);
                push_order(&mut out.order, 2, count_entries(&e.t));
            }
        }
        let mut tail = String::new();
        if !out.images.is_empty() {
            tail.push_str(",\"I\":{");
            for (i, (id, uri)) in out.images.iter().enumerate() {
                let _ = write!(
                    tail,
                    "{}{}:{}",
                    if i == 0 { "" } else { "," },
                    esc(id),
                    esc(uri)
                );
            }
            tail.push('}');
        }
        if !clips.is_empty() {
            tail.push_str(",\"C\":{");
            for (i, (d, evenodd, parent)) in clips.iter().enumerate() {
                let parent = parent.map_or(String::new(), |c| format!(",\"c{c}\""));
                let _ = write!(
                    tail,
                    "{}\"c{i}\":[{},{}{parent}]",
                    if i == 0 { "" } else { "," },
                    esc(d),
                    u8::from(*evenodd)
                );
            }
            tail.push('}');
        }
        if let Some(o) = order_json(&out.order) {
            let _ = write!(tail, ",\"o\":{o}");
        }
        if out.unsupported > 0 {
            let _ = write!(tail, ",\"x\":{}", out.unsupported);
        }
        format!(
            "{{\"v\":2,\"w\":{},\"h\":{},\"f\":[{}],\"F\":[{}],\"g\":{{{g}}},\"t\":[{t}],\"p\":[{paths}],\"r\":[{}]{tail}}}",
            r2(w),
            r2(h),
            f.join(","),
            fr.join(","),
            out.r
        )
    }
}

/// Content stream `b` (resources `res`, their fonts `fonts`), drawn from
/// state `g` into `ctx`; a form it paints drawn in turn, `depth` deep.
fn interpret(
    ctx: &mut Ctx,
    b: &[u8],
    fonts: &HashMap<String, usize>,
    res: &O,
    mut g: G,
    depth: u32,
) {
    let p = ctx.p;
    let page_h = ctx.page_h;
    let mut stack: Vec<G> = Vec::new();
    let (mut tm, mut tlm): (M, M) = (
        [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    );
    let mut d = String::new();
    // (`W`, `W*`: the path being made clips, from its painting operator on)
    let mut clip_next: Option<bool> = None;
    let mut ops: Vec<O> = Vec::new();
    let mut l = Lex { b, i: 0 };
    // (device space: y down from the page's top)
    let dev = |m: &M, x: f64, y: f64| {
        let (x, y) = apply(m, x, y);
        (r2(x), r2(page_h - y))
    };
    while let Some(o) = l.value() {
        let O::Op(op) = o else {
            ops.push(o);
            continue;
        };
        let n = |i: usize| ops.get(i).and_then(O::num).unwrap_or(0.0);
        match op.as_str() {
            "q" => stack.push(g.clone()),
            "Q" => {
                if let Some(s) = stack.pop() {
                    g = s;
                }
            }
            "cm" if ops.len() >= 6 => g.ctm = mul(&[n(0), n(1), n(2), n(3), n(4), n(5)], &g.ctm),
            "w" => g.lw = n(0),
            "g" => g.fill = rgb(n(0), n(0), n(0)),
            "G" => g.stroke = rgb(n(0), n(0), n(0)),
            "rg" => g.fill = rgb(n(0), n(1), n(2)),
            "RG" => g.stroke = rgb(n(0), n(1), n(2)),
            "k" => {
                g.fill = rgb(
                    (1.0 - n(0)) * (1.0 - n(3)),
                    (1.0 - n(1)) * (1.0 - n(3)),
                    (1.0 - n(2)) * (1.0 - n(3)),
                );
            }
            "K" => {
                g.stroke = rgb(
                    (1.0 - n(0)) * (1.0 - n(3)),
                    (1.0 - n(1)) * (1.0 - n(3)),
                    (1.0 - n(2)) * (1.0 - n(3)),
                );
            }
            "sc" | "scn" => match ops.iter().filter(|o| o.num().is_some()).count() {
                1 => g.fill = rgb(n(0), n(0), n(0)),
                3 => g.fill = rgb(n(0), n(1), n(2)),
                4 => {
                    g.fill = rgb(
                        (1.0 - n(0)) * (1.0 - n(3)),
                        (1.0 - n(1)) * (1.0 - n(3)),
                        (1.0 - n(2)) * (1.0 - n(3)),
                    );
                }
                _ => {}
            },
            "SC" | "SCN" => match ops.iter().filter(|o| o.num().is_some()).count() {
                1 => g.stroke = rgb(n(0), n(0), n(0)),
                3 => g.stroke = rgb(n(0), n(1), n(2)),
                4 => {
                    g.stroke = rgb(
                        (1.0 - n(0)) * (1.0 - n(3)),
                        (1.0 - n(1)) * (1.0 - n(3)),
                        (1.0 - n(2)) * (1.0 - n(3)),
                    );
                }
                _ => {}
            },
            "m" => {
                let (x, y) = dev(&g.ctm, n(0), n(1));
                let _ = write!(d, "M{x} {y}");
            }
            "l" => {
                let (x, y) = dev(&g.ctm, n(0), n(1));
                let _ = write!(d, "L{x} {y}");
            }
            "c" => {
                let (a, b) = dev(&g.ctm, n(0), n(1));
                let (c, e) = dev(&g.ctm, n(2), n(3));
                let (x, y) = dev(&g.ctm, n(4), n(5));
                let _ = write!(d, "C{a} {b} {c} {e} {x} {y}");
            }
            "v" | "y" => {
                // (one control point given: drawn as a straight cubic's ends)
                let (c, e) = dev(&g.ctm, n(0), n(1));
                let (x, y) = dev(&g.ctm, n(2), n(3));
                let _ = write!(d, "Q{c} {e} {x} {y}");
            }
            "h" => d.push('Z'),
            "re" => {
                let (x0, y0) = (n(0), n(1));
                let (w, hh) = (n(2), n(3));
                let p = [(x0, y0), (x0 + w, y0), (x0 + w, y0 + hh), (x0, y0 + hh)];
                for (i, (x, y)) in p.iter().enumerate() {
                    let (x, y) = dev(&g.ctm, *x, *y);
                    let _ = write!(d, "{}{x} {y}", if i == 0 { "M" } else { "L" });
                }
                d.push('Z');
            }
            "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" => {
                if op == "s" || op == "b" || op == "b*" {
                    d.push('Z');
                }
                let fill = matches!(op.as_str(), "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                let stroke = matches!(op.as_str(), "S" | "s" | "B" | "B*" | "b" | "b*");
                if (fill || stroke) && !d.is_empty() {
                    push_order(&mut ctx.out.order, 0, 1);
                    let lw = g.lw * ((g.ctm[0] * g.ctm[3] - g.ctm[1] * g.ctm[2]).abs().sqrt());
                    let _ = write!(
                        ctx.paths,
                        "{}[{},{},{},{}{}]",
                        if ctx.paths.is_empty() { "" } else { "," },
                        esc(&d),
                        if fill { esc(&g.fill) } else { "null".into() },
                        if stroke {
                            esc(&g.stroke)
                        } else {
                            "null".into()
                        },
                        r2(lw.max(0.1)),
                        clip_field(g.clip)
                    );
                }
                if let Some(evenodd) = clip_next.take()
                    && !d.is_empty()
                {
                    g.clip = Some(ctx.clip(d.clone(), evenodd, g.clip));
                }
                d.clear();
            }
            "W" => clip_next = Some(false),
            "W*" => clip_next = Some(true),
            "BT" => {
                tm = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
                tlm = tm;
            }
            "Tf" => {
                if let Some(O::Name(f)) = ops.first() {
                    g.font = fonts.get(f.as_str()).copied();
                }
                g.size = n(1);
            }
            "Tc" => g.tc = n(0),
            "Tw" => g.tw = n(0),
            "Tz" => g.tz = n(0),
            "TL" => g.tl = n(0),
            "Ts" => g.rise = n(0),
            "Td" => {
                tlm = mul(&[1.0, 0.0, 0.0, 1.0, n(0), n(1)], &tlm);
                tm = tlm;
            }
            "TD" => {
                g.tl = -n(1);
                tlm = mul(&[1.0, 0.0, 0.0, 1.0, n(0), n(1)], &tlm);
                tm = tlm;
            }
            "Tm" => {
                tlm = [n(0), n(1), n(2), n(3), n(4), n(5)];
                tm = tlm;
            }
            "T*" => {
                tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -g.tl], &tlm);
                tm = tlm;
            }
            "Tj" | "'" | "\"" => {
                if op != "Tj" {
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -g.tl], &tlm);
                    tm = tlm;
                }
                if let Some(O::Str(s)) = ops.last()
                    && let Some(font) = g.font.and_then(|f| ctx.fonts.get(f))
                {
                    let k = show(
                        font,
                        s,
                        &g,
                        &mut tm,
                        &mut ctx.text,
                        &mut ctx.used,
                        page_h,
                        &[],
                    );
                    push_order(&mut ctx.out.order, 2, k);
                }
            }
            "TJ" => {
                if let Some(O::Arr(a)) = ops.first() {
                    // (one run: the strings joined, each kern after the glyph before it)
                    let (mut s, mut kerns) = (Vec::new(), Vec::new());
                    let mut lead = 0.0;
                    for e in a {
                        match e {
                            O::Str(x) => s.extend_from_slice(x),
                            O::Num(k) if s.is_empty() => lead += k,
                            O::Num(k) => kerns.push((s.len() - 1, *k)),
                            _ => {}
                        }
                    }
                    if lead != 0.0 {
                        let th = g.tz / 100.0;
                        tm = mul(
                            &[1.0, 0.0, 0.0, 1.0, -lead / 1000.0 * g.size * th, 0.0],
                            &tm,
                        );
                    }
                    if let Some(font) = g.font.and_then(|f| ctx.fonts.get(f)) {
                        let k = show(
                            font,
                            &s,
                            &g,
                            &mut tm,
                            &mut ctx.text,
                            &mut ctx.used,
                            page_h,
                            &kerns,
                        );
                        push_order(&mut ctx.out.order, 2, k);
                    }
                }
            }
            "Do" => {
                let xo = ops.first().and_then(|n| match n {
                    O::Name(n) => res
                        .get("XObject")
                        .map(|x| p.resolve(x))
                        .and_then(|x| x.get(n).cloned()),
                    _ => None,
                });
                let Some(O::Ref(r)) = xo else {
                    ctx.out.unsupported += 1;
                    ops.clear();
                    continue;
                };
                let x = p.resolve(&O::Ref(r));
                match x.get("Subtype") {
                    Some(O::Name(t)) if t == "Image" => {
                        let img = p.image_ref(r, res);
                        place_image(img, &g, page_h, ctx.cache, &mut ctx.out);
                    }
                    // (a form: its content drawn through its matrix, with
                    // its resources, or the page's if it has none; a form
                    // that paints itself stops)
                    Some(O::Name(t)) if t == "Form" && depth < 16 => {
                        let body = p.stream(&O::Ref(r)).unwrap_or_default();
                        let m = match x.get("Matrix").map(|m| p.resolve(m)) {
                            Some(O::Arr(a)) if a.len() == 6 => {
                                let v = |i: usize| a[i].num().unwrap_or(0.0);
                                [v(0), v(1), v(2), v(3), v(4), v(5)]
                            }
                            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                        };
                        let mut fg = g.clone();
                        fg.ctm = mul(&m, &g.ctm);
                        // (clipped to its /BBox)
                        if let Some(O::Arr(bb)) = x.get("BBox").map(|b| p.resolve(b))
                            && bb.len() == 4
                        {
                            let v = |i: usize| bb[i].num().unwrap_or(0.0);
                            let mut c = String::new();
                            let corners = [(v(0), v(1)), (v(2), v(1)), (v(2), v(3)), (v(0), v(3))];
                            for (i, (x, y)) in corners.into_iter().enumerate() {
                                let (x, y) = dev(&fg.ctm, x, y);
                                let _ = write!(c, "{}{x} {y}", if i == 0 { "M" } else { "L" });
                            }
                            c.push('Z');
                            fg.clip = Some(ctx.clip(c, false, fg.clip));
                        }
                        if let Some(r) = x.get("Resources") {
                            let fres = p.resolve(r);
                            let ffonts = ctx.fonts_of(&fres);
                            interpret(ctx, &body, &ffonts, &fres, fg, depth + 1);
                        } else {
                            interpret(ctx, &body, fonts, res, fg, depth + 1);
                        }
                    }
                    _ => ctx.out.unsupported += 1,
                }
            }
            "BI" => {
                // (an inline image: its dictionary to `ID`, its bytes to `EI`)
                let mut d = Vec::new();
                while let Some(o) = l.value() {
                    match o {
                        O::Op(x) if x == "ID" => break,
                        O::Name(k) => {
                            if let Some(v) = l.value() {
                                d.push((k, v));
                            }
                        }
                        _ => {}
                    }
                }
                let start = (l.i + 1).min(l.b.len());
                let mut end = start;
                while end + 2 <= l.b.len() {
                    if &l.b[end..end + 2] == b"EI"
                        && end > start
                        && white(l.b[end - 1])
                        && l.b.get(end + 2).is_none_or(|&c| white(c) || delim(c))
                    {
                        break;
                    }
                    end += 1;
                }
                let data = l.b[start..end.min(l.b.len())].to_vec();
                l.i = (end + 2).min(l.b.len());
                let img = p.image_inline(&O::Dict(d), data, res);
                place_image(img, &g, page_h, ctx.cache, &mut ctx.out);
            }
            // (shadings: not drawn yet)
            "sh" => ctx.out.unsupported += 1,
            _ => {}
        }
        ops.clear();
    }
}

/// String `s` shown in `font` from state `g` and text matrix `tm`
/// (`kern_after`: `TJ`'s kerns, each after the glyph it follows): its
/// runs into `text`, the outlines it uses into `used`; how many entries.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn show(
    font: &Font,
    s: &[u8],
    g: &G,
    tm: &mut M,
    text: &mut String,
    used: &mut BTreeSet<(usize, u8)>,
    page_h: f64,
    kern_after: &[(usize, f64)],
) -> usize {
    let mut written = 0;
    if font.cid {
        return 0;
    }
    let (mut xs, mut txt) = (String::new(), String::new());
    // (runs: size, y as in the PDF, the x list, the text)
    let mut runs: Vec<(f64, f64, String, String)> = Vec::new();
    // (the glyphs drawn from outlines: x and code each)
    let (mut gxs, mut codes) = (String::new(), String::new());
    let outl = font.outlines.is_some();
    let th = g.tz / 100.0;
    let trm0 = mul(
        &[g.size * th, 0.0, 0.0, g.size, 0.0, g.rise],
        &mul(tm, &g.ctm),
    );
    let scale = (trm0[2] * trm0[2] + trm0[3] * trm0[3]).sqrt();
    let (_, y0) = apply(&trm0, 0.0, 0.0);
    let mut k = 0;
    // (where the glyph before ended: a gap wider than a fifth of the
    // size is a word space, which TeX sets as a kern, not a glyph)
    let mut end: Option<f64> = None;
    for (i, &c) in s.iter().enumerate() {
        let trm = mul(
            &[g.size * th, 0.0, 0.0, g.size, 0.0, g.rise],
            &mul(tm, &g.ctm),
        );
        let (x, _) = apply(&trm, 0.0, 0.0);
        let u = font.uni.get(&u32::from(c)).cloned().unwrap_or_else(|| {
            if (32..127).contains(&c) {
                char::from(c).to_string()
            } else {
                String::new()
            }
        });
        if let Some(e) = end
            && x - e > 0.2 * scale
            && !u.starts_with(' ')
        {
            let _ = write!(xs, " {}", r2(e));
            txt.push(' ');
        }
        let w0 = font
            .widths
            .get((i64::from(c) - font.first).max(0) as usize)
            .copied()
            .unwrap_or(0.0)
            / 1000.0;
        // (one glyph, several characters: a ligature is drawn as the
        // font's ligature, else the characters share the glyph's width;
        // variation selectors, which pdfTeX's maps add to math, dropped)
        if outl {
            let _ = write!(gxs, "{}{}", if gxs.is_empty() { "" } else { " " }, r2(x));
            let _ = write!(codes, "{}{c}", if codes.is_empty() { "" } else { "," });
            used.insert((font.fref, c));
        }
        let u = ligature(&u);
        // (a big operator or delimiter from cmex: a run of its own, the
        // character scaled to the glyph's height and depth, centred on
        // the box TeX set; the text font's ∫ is a text-size glyph)
        if font.ex
            && !outl
            && let Some(&(h, d)) = CMEX10.get(usize::from(c))
            && h + d > 1.3
            && !u.is_empty()
        {
            if !txt.is_empty() {
                runs.push((scale, y0, std::mem::take(&mut xs), std::mem::take(&mut txt)));
            }
            let big = scale * (h + d) / 1.1;
            let (_, yb) = apply(&trm, 0.0, 0.0);
            // (page y grows down: the box's centre, and the character's
            // baseline about a quarter of its size below its centre)
            let centre = page_h - yb + (d - h) / 2.0 * scale;
            runs.push((big, page_h - (centre + 0.25 * big), r2(x).to_string(), u));
            end = None;
            let w0 = font
                .widths
                .get((i64::from(c) - font.first).max(0) as usize)
                .copied()
                .unwrap_or(0.0)
                / 1000.0;
            let mut tx = (w0 * g.size + g.tc) * th;
            while k < kern_after.len() && kern_after[k].0 == i {
                tx -= kern_after[k].1 / 1000.0 * g.size * th;
                k += 1;
            }
            *tm = mul(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], tm);
            continue;
        }
        let n = u.chars().count().max(1);
        for (j, ch) in u.chars().enumerate() {
            let xj = x + w0 * g.size * th * scale / g.size.max(1e-9) * j as f64 / n as f64;
            let _ = write!(xs, "{}{}", if xs.is_empty() { "" } else { " " }, r2(xj));
            txt.push(ch);
        }
        let mut tx = (w0 * g.size + g.tc + if c == b' ' { g.tw } else { 0.0 }) * th;
        while k < kern_after.len() && kern_after[k].0 == i {
            tx -= kern_after[k].1 / 1000.0 * g.size * th;
            k += 1;
        }
        let (ex, _) = apply(
            &mul(
                &[g.size * th, 0.0, 0.0, g.size, 0.0, g.rise],
                &mul(
                    &mul(&[1.0, 0.0, 0.0, 1.0, w0 * g.size * th, 0.0], tm),
                    &g.ctm,
                ),
            ),
            0.0,
            0.0,
        );
        end = Some(ex);
        *tm = mul(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], tm);
    }
    if !txt.is_empty() {
        runs.push((scale, y0, xs, txt));
    }
    // (a run whose glyphs are drawn from outlines is text for selection
    // only: a sixth field, 1)
    // (a colour other than black: one more field, after the sixth, 0 or 1)
    let black = g.fill == "#000000";
    for (size, y, xs, txt) in runs {
        let tail = if let Some(c) = g.clip {
            let colour = if black { "null".into() } else { esc(&g.fill) };
            format!(",{},{colour},\"c{c}\"", u8::from(outl))
        } else if !black {
            format!(",{},{}", u8::from(outl), esc(&g.fill))
        } else if outl {
            ",1".into()
        } else {
            String::new()
        };
        let _ = write!(
            text,
            "{}[{},{},{},{},{}{tail}]",
            if text.is_empty() { "" } else { "," },
            font.key,
            r2(size),
            r2(page_h - y),
            esc(&xs),
            esc(&txt)
        );
        written += 1;
    }
    // (the glyphs from outlines: [-1, size, y, xs, "", font ref, codes,
    // colour?, matrix?, clip?])
    if !codes.is_empty() {
        let colour = if let Some(c) = g.clip {
            let colour = if black { "null".into() } else { esc(&g.fill) };
            format!(",{colour},null,\"c{c}\"")
        } else if black {
            String::new()
        } else {
            format!(",{}", esc(&g.fill))
        };
        let _ = write!(
            text,
            "{}[-1,{},{},{},\"\",{},[{codes}]{colour}]",
            if text.is_empty() { "" } else { "," },
            r2(scale),
            r2(page_h - y0),
            esc(&gxs),
            font.fref
        );
        written += 1;
    }
    written
}

/// What a content stream adds besides its text and paths.
#[derive(Default)]
struct Out {
    /// Images' entries (`r`), as a JSON array's body.
    r: String,
    /// Images' data URIs, by id.
    images: BTreeMap<String, String>,
    /// The paint order: runs of entries of one list (0 `p`, 1 `r`, 2 `t`).
    order: Vec<(u8, usize)>,
    /// What was not drawn (shadings, forms, images in a form not read).
    unsupported: usize,
}

/// `n` more entries of list `list` painted next.
fn push_order(order: &mut Vec<(u8, usize)>, list: u8, n: usize) {
    if n == 0 {
        return;
    }
    match order.last_mut() {
        Some((l, k)) if *l == list => *k += n,
        _ => order.push((list, n)),
    }
}

/// The paint order as `[[list, first, count], …]`, if it is not each list
/// whole in the order `p`, `r`, `t` (what no `"o"` means).
fn order_json(order: &[(u8, usize)]) -> Option<String> {
    if order.windows(2).all(|w| w[0].0 < w[1].0) {
        return None;
    }
    let mut first = [0usize; 3];
    let mut out = String::from("[");
    for (i, &(l, n)) in order.iter().enumerate() {
        let f = &mut first[usize::from(l)];
        let _ = write!(out, "{}[{l},{},{n}]", if i == 0 { "" } else { "," }, *f);
        *f += n;
    }
    out.push(']');
    Some(out)
}

/// How many entries the JSON array body `s` holds.
fn count_entries(s: &str) -> usize {
    let (mut depth, mut n, mut quoted, mut escaped) = (0i32, 0, false, false);
    for c in s.chars() {
        if quoted {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '[' | '{' => {
                if depth == 0 {
                    n += 1;
                }
                depth += 1;
            }
            ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    n
}

/// Image `img`, painted through the current matrix: its entry, `["id",
/// a, b, c, d, e, f]`, the matrix that takes an SVG unit square (its row
/// 0 at the top) to the page, from its top left; its data URI kept by id.
fn place_image(
    img: Option<(crate::image::Image, Option<crate::image::Image>)>,
    g: &G,
    page_h: f64,
    cache: &mut Fonts,
    out: &mut Out,
) {
    let Some((img, smask)) = img else {
        out.unsupported += 1;
        return;
    };
    let fill = hex_rgb(&g.fill);
    let key = {
        use std::hash::{Hash, Hasher};
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        img.raw.hash(&mut hs);
        smask.as_ref().map(|m| &m.raw).hash(&mut hs);
        if img.mask {
            fill.hash(&mut hs);
        }
        (img.width, img.height).hash(&mut hs);
        hs.finish()
    };
    let uri = if let Some(u) = cache.images.get(&key) {
        u.clone()
    } else {
        let u: Option<Arc<str>> =
            crate::image::data_uri(&img, smask.as_ref(), fill).map(Into::into);
        cache.images.insert(key, u.clone());
        u
    };
    let Some(uri) = uri else {
        out.unsupported += 1;
        return;
    };
    let id = format!("i{key:016x}");
    out.images
        .entry(id.clone())
        .or_insert_with(|| uri.to_string());
    let [a, b, c, d, e, f] = g.ctm;
    let m = [a, -b, -c, d, c + e, page_h - d - f];
    let _ = write!(
        out.r,
        "{}[{},{},{},{},{},{},{}{}]",
        if out.r.is_empty() { "" } else { "," },
        esc(&id),
        r4(m[0]),
        r4(m[1]),
        r4(m[2]),
        r4(m[3]),
        r4(m[4]),
        r4(m[5]),
        clip_field(g.clip)
    );
    push_order(&mut out.order, 1, 1);
}

/// `#rrggbb` as bytes.
/// An entry's clip field: `,"cN"`, or nothing.
fn clip_field(c: Option<usize>) -> String {
    c.map_or(String::new(), |c| format!(",\"c{c}\""))
}

fn hex_rgb(s: &str) -> [u8; 3] {
    let v = |i: usize| {
        s.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .unwrap_or(0)
    };
    [v(1), v(3), v(5)]
}

fn r4(x: f64) -> f64 {
    // (and -0 as 0)
    (x * 10_000.0).round() / 10_000.0 + 0.0
}

/// `u` (a glyph's `ToUnicode` text) as one character where a font has it: the
/// f-ligatures; and without variation selectors (U+FE00–FE0F).
fn ligature(u: &str) -> String {
    match u {
        "ff" => "\u{FB00}".into(),
        "fi" => "\u{FB01}".into(),
        "fl" => "\u{FB02}".into(),
        "ffi" => "\u{FB03}".into(),
        "ffl" => "\u{FB04}".into(),
        _ => u
            .chars()
            .filter(|c| !('\u{FE00}'..='\u{FE0F}').contains(c))
            .collect(),
    }
}

/// cmex10's characters' height and depth, in em (from its TFM).
const CMEX10: [(f64, f64); 128] = [
    (0.04, 1.16),
    (0.04, 1.16),
    (0.04, 1.16),
    (0.04, 1.16),
    (0.04, 1.16),
    (0.04, 1.16),
    (0.04, 1.16),
    (0.04, 1.16),
    (0.04, 1.16),
    (0.04, 1.16),
    (0.04, 1.16),
    (0.04, 1.16),
    (0.0, 0.6),
    (0.0, 0.6),
    (0.04, 1.16),
    (0.04, 1.16),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.36),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 2.96),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.0, 0.6),
    (0.0, 0.6),
    (0.0, 0.9),
    (0.0, 0.9),
    (0.0, 0.9),
    (0.0, 0.9),
    (0.0, 1.8),
    (0.0, 1.8),
    (0.0, 0.3),
    (0.0, 0.6),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.0, 0.6),
    (0.0, 0.6),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.0, 1.0),
    (0.1, 1.5),
    (0.0, 1.111),
    (0.0, 2.222),
    (0.0, 1.0),
    (0.1, 1.5),
    (0.0, 1.0),
    (0.1, 1.5),
    (0.0, 1.0),
    (0.1, 1.5),
    (0.0, 1.0),
    (0.0, 1.0),
    (0.0, 1.111),
    (0.0, 1.0),
    (0.0, 1.0),
    (0.0, 1.0),
    (0.0, 1.0),
    (0.0, 1.0),
    (0.1, 1.5),
    (0.1, 1.5),
    (0.0, 2.222),
    (0.1, 1.5),
    (0.1, 1.5),
    (0.1, 1.5),
    (0.1, 1.5),
    (0.1, 1.5),
    (0.0, 1.0),
    (0.1, 1.5),
    (0.722, 0.0),
    (0.75, 0.0),
    (0.75, 0.0),
    (0.722, 0.0),
    (0.75, 0.0),
    (0.75, 0.0),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.76),
    (0.04, 1.16),
    (0.04, 1.76),
    (0.04, 2.36),
    (0.04, 2.96),
    (0.0, 1.8),
    (0.0, 0.6),
    (0.04, 0.56),
    (0.0, 0.6),
    (0.0, 0.6),
    (0.0, 0.6),
    (0.12, 0.0),
    (0.12, 0.0),
    (0.12, 0.0),
    (0.12, 0.0),
    (0.0, 0.6),
    (0.0, 0.6),
];

/// The embedded Type 1 program of font dictionary `f` (`/FontDescriptor`'s
/// `/FontFile`), parsed once per object.
fn font_program(p: &Pdf, f: &O, cache: &mut Fonts) -> Option<Arc<crate::type1::Type1>> {
    let fd = p.resolve(f.get("FontDescriptor")?);
    let O::Ref(r) = fd.get("FontFile")? else {
        return None;
    };
    let b = p.stream(&O::Ref(*r))?;
    // (by the program's bytes: the same font in the next build's PDF, another object number)
    let key = {
        use std::hash::{Hash, Hasher};
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        b.hash(&mut hs);
        hs.finish()
    };
    if let Some(t) = cache.programs.get(&key) {
        return t.clone();
    }
    let t = (|| {
        let (d, _) = p.obj(*r)?;
        let len1 = match d.get("Length1") {
            Some(O::Ref(x)) => p.obj(*x)?.0.num()?,
            o => o?.num()?,
        } as usize;
        crate::type1::Type1::parse(&b, len1).map(Arc::new)
    })();
    cache.programs.insert(key, t.clone());
    t
}

/// A font name as an id (letters and digits).
fn ident(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
