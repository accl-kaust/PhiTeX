//! pdfTeX's PDF output (`\pdfoutput>0`): the PDF file as a fold over the
//! shipped pages, as the DVI file is (`dvi.rs`).
//!
//! A page's content stream is a value of the shipped page, made where
//! the page is made ([`content`]), a pure function of the page, what
//! shipping read and the fonts' numbering; the fold ([`fold`]) receives
//! the pages in document order and does what pdfTeX's `pdf_ship_out` and
//! the end of the job do: it numbers the objects, writes the pages, the
//! fonts used (their files subsetted to the glyphs the pages used), the
//! page tree, the catalog, the information dictionary and the
//! cross-reference table. Nothing in a page's value depends on object
//! numbers.
//!
//! The parts, by what another agent could take on its own:
//! - [`num`], [`zlib`], [`md5`]: pdfTeX's number formatting, the
//!   compression of streams, the digests;
//! - [`content`]: a page's content stream (text, rules, literals);
//! - [`font`]: the fonts (the map file, encodings, Type 1 subsetting, the
//!   font dictionaries);
//! - [`fold`]: the objects of the file and its structure.

pub mod md5;
pub mod zlib;
pub mod content;
pub mod fold;
pub mod font;
pub mod num;
pub mod what;
pub mod lib;
pub mod image;
pub mod colorstack;
pub mod rand;
pub mod vf;

use std::sync::Arc;

use crate::val::{DimenPar, IntPar, Key, Tok, TokPar, Val};

/// What shipping a page in PDF mode reads (pdftex.web §750–§752 and the
/// parameters `check_pdfversion` and `init_pdf_output` fix at the first
/// page): part of the page's `ShipParams`, `None` in DVI mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ship {
    /// `\pdfoutput` (positive).
    pub output: i32,
    pub horigin: i32,
    pub vorigin: i32,
    pub page_width: i32,
    pub page_height: i32,
    pub decimal_digits: i32,
    pub compress_level: i32,
    pub obj_compress_level: i32,
    pub major_version: i32,
    pub minor_version: i32,
    pub draftmode: i32,
    /// `\pdfomitprocset`.
    pub omit_procset: i32,
    /// `\pdfpageattr` and `\pdfpageresources` as `tokens_to_string`
    /// shows them (`None`: empty, TeX's null).
    pub page_attr: Option<Arc<[u8]>>,
    pub page_resources: Option<Arc<[u8]>>,
    /// `\pdflinkmargin`, `\pdfdestmargin`, `\pdfthreadmargin` (read by
    /// the walk where the marks are).
    pub margins: content::Margins,
    /// The parameters the objects written with the page read (images,
    /// forms, `\pdfsuppresswarningdupdest`).
    pub params: Arc<what::Params>,
}

/// What the first page fixed for the whole file (`check_pdfversion`,
/// `init_pdf_output`), which every page's stream assumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fixed {
    pub decimal_digits: i32,
}

impl Fixed {
    /// What `ship` fixes when it is the first page.
    pub fn of(ship: &Ship) -> Fixed {
        Fixed { decimal_digits: ship.decimal_digits.clamp(0, 4) }
    }
}

/// The names [`read_ship`] reads.
pub fn ship_keys() -> Vec<Key> {
    let mut v: Vec<Key> = [
        IntPar::Pdfoutput,
        IntPar::Pdfdecimaldigits,
        IntPar::Pdfcompresslevel,
        IntPar::Pdfobjcompresslevel,
        IntPar::Pdfmajorversion,
        IntPar::Pdfminorversion,
        IntPar::Pdfdraftmode,
        IntPar::Pdfomitprocset,
        IntPar::Escapechar,
    ]
    .into_iter()
    .chain(PARAM_INTS)
    .map(Key::IntPar)
    .collect();
    v.extend(
        [
            DimenPar::Pdfhorigin,
            DimenPar::Pdfvorigin,
            DimenPar::Pdfpagewidth,
            DimenPar::Pdfpageheight,
            DimenPar::Pdflinkmargin,
            DimenPar::Pdfdestmargin,
            DimenPar::Pdfthreadmargin,
        ]
            .into_iter()
            .map(Key::DimenPar),
    );
    v.extend([TokPar::Pdfpageattr, TokPar::Pdfpageresources].into_iter().map(Key::TokPar));
    v
}

/// The integer parameters of [`what::Params`] beyond the ship's own.
const PARAM_INTS: [IntPar; 9] = [
    IntPar::Pdfinclusioncopyfonts,
    IntPar::Pdfimagehicolor,
    IntPar::Pdfimageapplygamma,
    IntPar::Pdfgamma,
    IntPar::Pdfimagegamma,
    IntPar::Pdfinclusionerrorlevel,
    IntPar::Pdfsuppresswarningdupdest,
    IntPar::Pdfsuppressptexinfo,
    IntPar::Pdfsuppresswarningpagegroup,
];

/// The parameters an object written now reads (`what::Params`), where
/// `read` reads.
pub fn read_params(read: &mut dyn FnMut(&Key) -> Val) -> what::Params {
    let mut i = |p: IntPar| int(&read(&Key::IntPar(p)));
    what::Params {
        output: i(IntPar::Pdfoutput),
        major: i(IntPar::Pdfmajorversion),
        minor: i(IntPar::Pdfminorversion),
        compress_level: i(IntPar::Pdfcompresslevel),
        obj_compress_level: i(IntPar::Pdfobjcompresslevel),
        draftmode: i(IntPar::Pdfdraftmode),
        decimal_digits: i(IntPar::Pdfdecimaldigits),
        copy_fonts: i(IntPar::Pdfinclusioncopyfonts),
        image_hicolor: i(IntPar::Pdfimagehicolor),
        image_apply_gamma: i(IntPar::Pdfimageapplygamma),
        gamma: i(IntPar::Pdfgamma),
        image_gamma: i(IntPar::Pdfimagegamma),
        inclusion_errorlevel: i(IntPar::Pdfinclusionerrorlevel),
        suppress_dup_dest: i(IntPar::Pdfsuppresswarningdupdest),
        suppress_ptex_info: i(IntPar::Pdfsuppressptexinfo),
        suppress_pagegroup: i(IntPar::Pdfsuppresswarningpagegroup),
        omit_procset: i(IntPar::Pdfomitprocset),
    }
}

fn int(v: &Val) -> i32 {
    match v {
        Val::Int(i) => *i,
        _ => 0,
    }
}

/// Where a page is shipped: the names' values there, and a token list as
/// `tokens_to_string` shows it there.
pub trait ShipEnv {
    fn read(&mut self, key: &Key) -> Val;
    fn text(&mut self, toks: &[Tok]) -> Vec<u8>;
}

/// What shipping a page reads in PDF mode, where `env` is. `None` in DVI
/// mode (`\pdfoutput` not positive).
pub fn read_ship(env: &mut dyn ShipEnv) -> Option<Arc<Ship>> {
    let output = int(&env.read(&Key::IntPar(IntPar::Pdfoutput)));
    if output <= 0 {
        return None;
    }
    let mut i = |p: IntPar| int(&env.read(&Key::IntPar(p)));
    let (decimal_digits, compress_level, obj_compress_level) =
        (i(IntPar::Pdfdecimaldigits), i(IntPar::Pdfcompresslevel), i(IntPar::Pdfobjcompresslevel));
    let (major_version, minor_version, draftmode) =
        (i(IntPar::Pdfmajorversion), i(IntPar::Pdfminorversion), i(IntPar::Pdfdraftmode));
    let omit_procset = i(IntPar::Pdfomitprocset);
    let mut d = |p: DimenPar| int(&env.read(&Key::DimenPar(p)));
    let (horigin, vorigin, page_width, page_height) =
        (d(DimenPar::Pdfhorigin), d(DimenPar::Pdfvorigin), d(DimenPar::Pdfpagewidth), d(DimenPar::Pdfpageheight));
    let mut t = |p: TokPar| match env.read(&Key::TokPar(p)) {
        Val::Toks(ts) if !ts.is_empty() => Some(Arc::from(env.text(&ts))),
        _ => None,
    };
    let (page_attr, page_resources) = (t(TokPar::Pdfpageattr), t(TokPar::Pdfpageresources));
    let mut d = |p: DimenPar| int(&env.read(&Key::DimenPar(p)));
    let margins = content::Margins {
        link: d(DimenPar::Pdflinkmargin),
        dest: d(DimenPar::Pdfdestmargin),
        thread: d(DimenPar::Pdfthreadmargin),
    };
    let params = Arc::new(read_params(&mut |k| env.read(k)));
    Some(Arc::new(Ship {
        output,
        horigin,
        vorigin,
        page_width,
        page_height,
        decimal_digits,
        compress_level,
        obj_compress_level,
        major_version,
        minor_version,
        draftmode,
        omit_procset,
        page_attr,
        page_resources,
        margins,
        params,
    }))
}

/// `tokens_to_string` (tex.web 292–294, §262's `print_cs`): a token list
/// as TeX shows it, with the escape character `esc`, a one-letter name
/// followed by a space when `letter` says its character is a letter.
pub fn toks_text(toks: &[Tok], name: &dyn Fn(u32) -> Arc<str>, esc: i32, letter: &mut dyn FnMut(char) -> bool) -> Vec<u8> {
    let mut s = String::new();
    let put_esc = |s: &mut String, n: &str| {
        if let Some(c) = u8::try_from(esc).ok().map(char::from) {
            s.push(c);
        }
        s.push_str(n);
    };
    for t in toks {
        match t {
            Tok::Cs(n) => {
                let n = name(*n);
                let mut it = n.chars();
                match (it.next(), it.next()) {
                    (None, _) => {
                        put_esc(&mut s, "csname");
                        put_esc(&mut s, "endcsname");
                        s.push(' ');
                    }
                    (Some(c), None) => {
                        put_esc(&mut s, &n);
                        if letter(c) {
                            s.push(' ');
                        }
                    }
                    _ => {
                        put_esc(&mut s, &n);
                        s.push(' ');
                    }
                }
            }
            Tok::Char(6, c) => {
                s.push(*c);
                s.push(*c);
            }
            Tok::Char(_, c) | Tok::Active(c) => s.push(*c),
            Tok::Param(n, _) => {
                s.push('#');
                s.push(char::from(b'0' + n));
            }
            Tok::DontExpand => {}
        }
    }
    s.chars().map(|c| u32::from(c).min(255) as u8).collect()
}

/// pdfTeX's date of the file (`makepdftime` in UTC, as with
/// `SOURCE_DATE_EPOCH`): `D:YYYYMMDDHHMMSSZ` of `epoch` seconds.
pub fn pdf_date(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    // (the civil date of a day count: Howard Hinnant's `civil_from_days`)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}
