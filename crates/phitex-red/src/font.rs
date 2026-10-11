//! C7: TFM fonts (parse, `\font` with `at`/`scaled`, `\fontdimen`,
//! `\hyphenchar`, `\skewchar`, `\fontname`, `\nullfont`), fonts by id.
//!
//! A font is an immutable value in the session's append-only table
//! (`fonts()`), keyed by (name, file bytes, size, variant) so equal loads get
//! the same id from any thread. Everything a document can change about a font
//! (`\fontdimen`, `\hyphenchar`, `\skewchar`, pdfTeX's char codes) lives in
//! env under global keys, defined when the font is first loaded in that env.
//!
//! Keys used (all global):
//! * `Key::FontDimen(f, n)` (n 1-based), `Key::HyphenChar(f)`, `Key::SkewChar(f)`;
//! * `Key::FontCode(which, f | c << 20)`: pdfTeX per-char codes (`code::*`);
//! * `Key::FontLoaded(area+name, s)`: `s` as `\font` gave it (`at` > 0,
//!   `-scaled` < 0), and the resulting size; `Val::Font(f)`;
//! * `Key::Other(tag::FONT, …)`: see `other`.
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::ctx::Ctx;
use crate::diag::error;
use crate::prim::{ip, tag, Kind, Level, Registry};
use crate::scan;
use crate::tok::Tok;
use crate::val::{FontId, Key, Meaning, Val};

const UNITY: i32 = 65536;

/// `Key::Other(tag::FONT, n)` numbers.
pub mod other {
    /// The last font loaded (TeX's `font_ptr`), `Val::Int`.
    pub const LAST: u32 = 0;
    /// The dummy `\fontdimen` slot after an error (`fmem_ptr`).
    pub const DUMMY: u32 = 1;
    /// `| f`: number of `\fontdimen` parameters of f.
    pub const PARAMS: u32 = 0x1000_0000;
    /// `| f`: `font_id_text` (`Val::Toks` of one token).
    pub const ID_TEXT: u32 = 0x2000_0000;
    /// `| f`: `\pdffontexpand` (`Val::Ints [stretch, shrink, step, autoexpand]`).
    pub const EXPAND: u32 = 0x3000_0000;
    /// `| f`: `\pdfnoligatures` (Int 1).
    pub const NO_LIGS: u32 = 0x4000_0000;
    /// `| f`: number of `\pdfcopyfont`/`\letterspacefont` made from f.
    pub const COPIES: u32 = 0x5000_0000;
}

/// pdfTeX per-char codes (`Key::FontCode` first field).
pub mod code {
    pub const LP: u8 = 0;
    pub const RP: u8 = 1;
    pub const EF: u8 = 2;
    pub const TAG: u8 = 3;
    pub const KNBS: u8 = 4;
    pub const STBS: u8 = 5;
    pub const SHBS: u8 = 6;
    pub const KNBC: u8 = 7;
    pub const KNAC: u8 = 8;
    pub const NAMES: &[&str] =
        &["lpcode", "rpcode", "efcode", "tagcode", "knbscode", "stbscode", "shbscode", "knbccode", "knaccode"];
}

fn okey(n: u32) -> Key {
    Key::Other(tag::FONT, n)
}

/// The key of a pdfTeX per-char code (`code::*`) of char `c` in font `f`.
pub fn code_key(which: u8, f: FontId, c: u16) -> Key {
    Key::FontCode(which, ((c as u32 & 0xFF) << 20) | (f & 0xF_FFFF))
}

// ---------------------------------------------------------------- the font

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Font {
    /// The name as `\font` gave it (no area, no extension).
    name: Arc<str>,
    area: Arc<str>,
    /// The TFM file as found.
    file: Arc<str>,
    size: i32,
    dsize: i32,
    check_sum: u32,
    bc: i32,
    ec: i32,
    /// char_info words for bc..=ec.
    info: Vec<[u8; 4]>,
    widths: Vec<i32>,
    heights: Vec<i32>,
    depths: Vec<i32>,
    italics: Vec<i32>,
    lig: Vec<[u8; 4]>,
    kerns: Vec<i32>,
    exten: Vec<[u8; 4]>,
    /// params[0] is `\fontdimen1` (slant, unscaled).
    params: Vec<i32>,
    bchar_label: Option<usize>,
    /// 256: none.
    bchar: u16,
    false_bchar: u16,
    /// pdfTeX `\letterspacefont` amount (thousandths of an em), 0 none.
    letterspace: i32,
    /// pdfTeX `\pdfcopyfont` / `\letterspacefont` copy number (0: the original).
    copy: u32,
    tfm: Arc<[u8]>,
}

pub enum LigKern {
    None,
    Kern(i32),
    Lig { op: u8, c: u16 },
}

/// tex.web 571 `store_scaled` for a size z.
fn scaler(size: i32) -> impl Fn([u8; 4]) -> Option<i32> {
    let mut z = size;
    let mut alpha: i32 = 16;
    while z >= 0o40000000 {
        z /= 2;
        alpha += alpha;
    }
    let beta = (256 / alpha) as i64;
    let alpha = alpha * z;
    let z = z as i64;
    move |[a, b, c, d]: [u8; 4]| {
        let (b, c, d) = (b as i64, c as i64, d as i64);
        let sw = (((((d * z) / 256) + (c * z)) / 256 + (b * z)) / beta) as i32;
        match a {
            0 => Some(sw),
            255 => Some(sw - alpha),
            _ => None,
        }
    }
}

impl Font {
    /// The null font (`\nullfont`).
    pub fn null() -> Font {
        Font {
            name: "nullfont".into(),
            area: "".into(),
            file: "".into(),
            size: 0,
            dsize: 0,
            check_sum: 0,
            bc: 1,
            ec: 0,
            info: Vec::new(),
            widths: vec![0],
            heights: vec![0],
            depths: vec![0],
            italics: vec![0],
            lig: Vec::new(),
            kerns: Vec::new(),
            exten: Vec::new(),
            params: vec![0; 7],
            bchar_label: None,
            bchar: 256,
            false_bchar: 256,
            letterspace: 0,
            copy: 0,
            tfm: Arc::from(Vec::<u8>::new()),
        }
    }

    /// `read_font_info` (tex.web 560–575). `s`: > 0 `at s`, < 0 `scaled -s`.
    /// `Err(())`: a bad metric file.
    pub fn parse(name: Arc<str>, area: Arc<str>, file: Arc<str>, tfm: Arc<[u8]>, s: i32) -> Result<Font, ()> {
        let b: &[u8] = &tfm;
        let half = |i: usize| -> Result<i32, ()> {
            let hi = *b.get(2 * i).ok_or(())?;
            let lo = *b.get(2 * i + 1).ok_or(())?;
            if hi > 127 {
                return Err(());
            }
            Ok(hi as i32 * 256 + lo as i32)
        };
        // 565: the subfile sizes
        let lf = half(0)?;
        let lh = half(1)?;
        let mut bc = half(2)?;
        let mut ec = half(3)?;
        if bc > ec + 1 || ec > 255 {
            return Err(());
        }
        if bc > 255 {
            bc = 1;
            ec = 0;
        }
        let nw = half(4)?;
        let nh = half(5)?;
        let nd = half(6)?;
        let ni = half(7)?;
        let nl = half(8)?;
        let nk = half(9)?;
        let ne = half(10)?;
        let np = half(11)?;
        if lf != 6 + lh + (ec - bc + 1) + nw + nh + nd + ni + nl + nk + ne + np {
            return Err(());
        }
        if nw == 0 || nh == 0 || nd == 0 || ni == 0 {
            return Err(());
        }
        if b.len() < 4 * lf as usize {
            return Err(());
        }
        let word = |i: usize| -> [u8; 4] { [b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]] };
        // 568: the header
        if lh < 2 {
            return Err(());
        }
        let mut w = 6usize;
        let check_sum = u32::from_be_bytes(word(w));
        let [a, bb, c, d] = word(w + 1);
        if a > 127 {
            return Err(());
        }
        let mut z = ((a as i32 * 256 + bb as i32) * 256 + c as i32) * 16 + (d as i32 / 16);
        if z < UNITY {
            return Err(());
        }
        w += lh as usize;
        let dsize = z;
        if s != -1000 {
            z = if s >= 0 { s } else { xn_over_d(z, -s, 1000) };
        }
        let size = z;
        // 569–570: char info
        let info: Vec<[u8; 4]> = (0..(ec - bc + 1) as usize).map(|k| word(w + k)).collect();
        w += info.len();
        let exists = |c: i32| c >= bc && c <= ec && info[(c - bc) as usize][0] > 0;
        for (k, q) in info.iter().enumerate() {
            let [a, b2, c2, d2] = *q;
            if a as i32 >= nw || (b2 / 16) as i32 >= nh || (b2 % 16) as i32 >= nd || (c2 / 4) as i32 >= ni {
                return Err(());
            }
            match c2 % 4 {
                1 => {
                    if d2 as i32 >= nl {
                        return Err(());
                    }
                }
                3 => {
                    if d2 as i32 >= ne {
                        return Err(());
                    }
                }
                2 => {
                    let cur = bc + k as i32;
                    let mut dd = d2 as i32;
                    if !exists(dd) {
                        return Err(());
                    }
                    while dd < cur {
                        let qw = info[(dd - bc) as usize];
                        if qw[2] % 4 != 2 {
                            break;
                        }
                        dd = qw[3] as i32;
                    }
                    if dd == cur {
                        return Err(());
                    }
                }
                _ => {}
            }
        }
        // 571: widths, heights, depths, italics
        let sc = scaler(size);
        let read = |n: i32, w: &mut usize| -> Result<Vec<i32>, ()> {
            let mut v = Vec::with_capacity(n as usize);
            for k in 0..n as usize {
                v.push(sc(word(*w + k)).ok_or(())?);
            }
            *w += n as usize;
            Ok(v)
        };
        let widths = read(nw, &mut w)?;
        let heights = read(nh, &mut w)?;
        let depths = read(nd, &mut w)?;
        let italics = read(ni, &mut w)?;
        if widths[0] != 0 || heights[0] != 0 || depths[0] != 0 || italics[0] != 0 {
            return Err(());
        }
        // 573: ligature/kern program
        let lig: Vec<[u8; 4]> = (0..nl as usize).map(|k| word(w + k)).collect();
        w += nl as usize;
        let mut bch_label: i32 = 0o77777;
        let mut bchar: i32 = 256;
        if nl > 0 {
            for (k, q) in lig.iter().enumerate() {
                let [a, b2, c2, d2] = *q;
                if a > 128 {
                    if 256 * c2 as i32 + d2 as i32 >= nl {
                        return Err(());
                    }
                    if a == 255 && k == 0 {
                        bchar = b2 as i32;
                    }
                } else {
                    if b2 as i32 != bchar && !exists(b2 as i32) {
                        return Err(());
                    }
                    if c2 < 128 {
                        if !exists(d2 as i32) {
                            return Err(());
                        }
                    } else if 256 * (c2 as i32 - 128) + d2 as i32 >= nk {
                        return Err(());
                    }
                    if a < 128 && k as i32 + a as i32 + 1 >= nl {
                        return Err(());
                    }
                }
            }
            let last = lig[nl as usize - 1];
            if last[0] == 255 {
                bch_label = 256 * last[2] as i32 + last[3] as i32;
            }
        }
        let kerns = read(nk, &mut w)?;
        // 574: extensible recipes
        let exten: Vec<[u8; 4]> = (0..ne as usize).map(|k| word(w + k)).collect();
        w += ne as usize;
        for q in &exten {
            for (i, &x) in q.iter().enumerate() {
                if (i == 3 || x != 0) && !exists(x as i32) {
                    return Err(());
                }
            }
        }
        // 575: parameters
        let mut params = Vec::with_capacity(np.max(7) as usize);
        for k in 1..=np as usize {
            let q = word(w + k - 1);
            if k == 1 {
                let mut sw = q[0] as i32;
                if sw > 127 {
                    sw -= 256;
                }
                sw = sw * 256 + q[1] as i32;
                sw = sw * 256 + q[2] as i32;
                params.push(sw * 16 + q[3] as i32 / 16);
            } else {
                params.push(sc(q).ok_or(())?);
            }
        }
        while params.len() < 7 {
            params.push(0);
        }
        let bchar_label = if bch_label < nl { Some(bch_label as usize) } else { None };
        let bchar_u = if bchar <= 255 { bchar as u16 } else { 256 };
        let false_bchar = if bchar <= 255 && exists(bchar) { 256 } else { bchar_u };
        Ok(Font {
            name,
            area,
            file,
            size,
            dsize,
            check_sum,
            bc,
            ec,
            info,
            widths,
            heights,
            depths,
            italics,
            lig,
            kerns,
            exten,
            params,
            bchar_label,
            bchar: bchar_u,
            false_bchar,
            letterspace: 0,
            copy: 0,
            tfm,
        })
    }

    fn ci(&self, c: u16) -> Option<[u8; 4]> {
        let c = c as i32;
        if c < self.bc || c > self.ec {
            return None;
        }
        let q = self.info[(c - self.bc) as usize];
        (q[0] > 0).then_some(q)
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn area(&self) -> &str {
        &self.area
    }
    /// The TFM file as found (for the map file / font embedding).
    pub fn file(&self) -> &str {
        &self.file
    }
    pub fn size(&self) -> i32 {
        self.size
    }
    pub fn design_size(&self) -> i32 {
        self.dsize
    }
    pub fn check_sum(&self) -> u32 {
        self.check_sum
    }
    pub fn letterspace(&self) -> i32 {
        self.letterspace
    }
    pub fn copy_number(&self) -> u32 {
        self.copy
    }
    pub fn bc(&self) -> u16 {
        self.bc.max(0) as u16
    }
    pub fn ec(&self) -> u16 {
        self.ec.max(0) as u16
    }
    /// `font_bchar` (None: `non_char`).
    pub fn bchar(&self) -> Option<u16> {
        (self.bchar < 256).then_some(self.bchar)
    }
    /// `font_false_bchar` (None: `non_char`).
    pub fn false_bchar(&self) -> Option<u16> {
        (self.false_bchar < 256).then_some(self.false_bchar)
    }
    /// Whether a left boundary program exists (`bchar_label <> non_address`).
    pub fn has_left_boundary(&self) -> bool {
        self.bchar_label.is_some()
    }
    pub fn has(&self, c: u16) -> bool {
        self.ci(c).is_some()
    }
    pub fn width(&self, c: u16) -> i32 {
        self.ci(c).map_or(0, |q| self.widths[q[0] as usize] + self.ls_extra())
    }
    pub fn height(&self, c: u16) -> i32 {
        self.ci(c).map_or(0, |q| self.heights[(q[1] / 16) as usize])
    }
    pub fn depth(&self, c: u16) -> i32 {
        self.ci(c).map_or(0, |q| self.depths[(q[1] % 16) as usize])
    }
    pub fn italic(&self, c: u16) -> i32 {
        self.ci(c).map_or(0, |q| self.italics[(q[2] / 4) as usize])
    }
    /// The char's tag: 0 none, 1 lig/kern, 2 list, 3 extensible.
    pub fn tag(&self, c: u16) -> u8 {
        self.ci(c).map_or(0, |q| q[2] % 4)
    }
    /// Width a `\letterspacefont` copy adds to each char.
    pub fn ls_extra(&self) -> i32 {
        if self.letterspace == 0 { 0 } else { round_xn_over_d(self.params[5], self.letterspace, 1000) }
    }

    /// Runs a lig/kern program from instruction `start` for next char `next`.
    fn run(&self, start: usize, next: u16) -> LigKern {
        let Some(&first) = self.lig.get(start) else { return LigKern::None };
        let mut k = start;
        if first[0] > 128 {
            k = 256 * first[2] as usize + first[3] as usize;
        }
        loop {
            let Some(&q) = self.lig.get(k) else { return LigKern::None };
            if q[1] as u16 == next && q[0] <= 128 {
                return if q[2] >= 128 {
                    let i = 256 * (q[2] as usize - 128) + q[3] as usize;
                    LigKern::Kern(self.kerns.get(i).copied().unwrap_or(0))
                } else {
                    LigKern::Lig { op: q[2], c: q[3] as u16 }
                };
            }
            if q[0] >= 128 {
                return LigKern::None;
            }
            k += q[0] as usize + 1;
        }
    }
    fn start(&self, a: u16) -> Option<usize> {
        let q = self.ci(a)?;
        (q[2] % 4 == 1).then_some(q[3] as usize)
    }
    pub fn lig_kern(&self, a: u16, b: u16) -> LigKern {
        match self.start(a) {
            Some(k) => self.run(k, b),
            None => LigKern::None,
        }
    }
    /// Boundary char programs: `a` None = left boundary, `b` None = right.
    pub fn lig_kern_boundary(&self, a: Option<u16>, b: Option<u16>) -> LigKern {
        let next = match b.or(self.bchar()) {
            Some(c) => c,
            None => return LigKern::None,
        };
        let start = match a {
            Some(a) => self.start(a),
            None => self.bchar_label,
        };
        match start {
            Some(k) => self.run(k, next),
            None => LigKern::None,
        }
    }
    /// The TFM's parameter n (1-based); 0 past the end. Documents change
    /// parameters through env: nodes read `font_dimen`.
    pub fn param(&self, n: u16) -> i32 {
        if n == 0 {
            return 0;
        }
        self.params.get(n as usize - 1).copied().unwrap_or(0)
    }
    pub fn param_count(&self) -> u16 {
        self.params.len() as u16
    }
    /// Extensible recipe / next larger (math delimiters).
    pub fn next_larger(&self, c: u16) -> Option<u16> {
        let q = self.ci(c)?;
        (q[2] % 4 == 2).then_some(q[3] as u16)
    }
    /// `[top, mid, bot, rep]`; 0 in top/mid/bot means absent.
    pub fn extensible(&self, c: u16) -> Option<[u16; 4]> {
        let q = self.ci(c)?;
        if q[2] % 4 != 3 {
            return None;
        }
        let e = self.exten.get(q[3] as usize)?;
        Some([e[0] as u16, e[1] as u16, e[2] as u16, e[3] as u16])
    }
    pub fn tfm(&self) -> &Arc<[u8]> {
        &self.tfm
    }
}

// ---------------------------------------------------------------- arithmetic

fn xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    let r = (x.unsigned_abs() as i64 * n as i64) / d as i64;
    let r = r.min(i32::MAX as i64) as i32;
    if x < 0 { -r } else { r }
}

/// pdfTeX's `round_xn_over_d`.
fn round_xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    let neg = (x < 0) != (n < 0);
    let v = (x as i64).abs() * (n as i64).abs();
    let r = ((v + d as i64 / 2) / d as i64).min(i32::MAX as i64) as i32;
    if neg { -r } else { r }
}

/// tex.web 103 `print_scaled`.
pub fn scaled_text(s: i32) -> String {
    let mut o = String::new();
    let mut s = s as i64;
    if s < 0 {
        o.push('-');
        s = -s;
    }
    let unity = UNITY as i64;
    o.push_str(&(s / unity).to_string());
    o.push('.');
    let mut s = 10 * (s % unity) + 5;
    let mut delta: i64 = 10;
    loop {
        if delta > unity {
            s = s + 0o100000 - 50000;
        }
        o.push(char::from(b'0' + (s / unity) as u8));
        s = 10 * (s % unity);
        delta *= 10;
        if s <= delta {
            break;
        }
    }
    o
}

// ---------------------------------------------------------------- the table

#[derive(Clone, PartialEq, Eq, Hash)]
enum TableKey {
    Load { area: Arc<str>, name: Arc<str>, file: Arc<str>, bytes: Arc<[u8]>, size: i32 },
    Copy { base: FontId, n: u32, ls: i32 },
}

#[derive(Default)]
struct Table {
    inner: RwLock<(Vec<Arc<Font>>, HashMap<TableKey, FontId>)>,
}

/// Fonts by id: the session's table (append-only, shared).
pub trait Fonts {
    fn font(&self, id: crate::val::FontId) -> Arc<Font>;
}

impl Fonts for Table {
    fn font(&self, id: FontId) -> Arc<Font> {
        let r = self.inner.read().unwrap();
        r.0.get(id as usize).cloned().unwrap_or_else(|| r.0[0].clone())
    }
}

impl Table {
    fn intern(&self, key: TableKey, make: impl FnOnce() -> Result<Font, ()>) -> Result<FontId, ()> {
        if let Some(&f) = self.inner.read().unwrap().1.get(&key) {
            return Ok(f);
        }
        let font = make()?;
        let mut w = self.inner.write().unwrap();
        if let Some(&f) = w.1.get(&key) {
            return Ok(f);
        }
        let f = w.0.len() as FontId;
        w.0.push(Arc::new(font));
        w.1.insert(key, f);
        Ok(f)
    }
}

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        let t = Table::default();
        t.inner.write().unwrap().0.push(Arc::new(Font::null()));
        t
    })
}

/// The session's font table.
pub fn fonts() -> &'static dyn Fonts {
    table()
}

/// Loads (or finds) a TFM in the table, as `\font` does (no env keys).
/// `s`: > 0 `at`, < 0 `-scaled`.
pub fn load_tfm(area: &str, name: &str, file: Arc<str>, bytes: Arc<[u8]>, s: i32) -> Result<FontId, ()> {
    let (n, a): (Arc<str>, Arc<str>) = (name.into(), area.into());
    let key = TableKey::Load { area: a.clone(), name: n.clone(), file: file.clone(), bytes: bytes.clone(), size: s };
    table().intern(key, || Font::parse(n, a, file, bytes, s))
}

// ---------------------------------------------------------------- env reads

/// `\fontdimen n f` as the document has it.
pub fn font_dimen(c: &mut dyn Ctx, f: FontId, n: u16) -> i32 {
    match c.get(&Key::FontDimen(f, n)) {
        Val::Int(v) => v,
        _ => fonts().font(f).param(n),
    }
}

/// The number of `\fontdimen` parameters f has.
pub fn param_count(c: &mut dyn Ctx, f: FontId) -> u16 {
    match c.get(&okey(other::PARAMS | f)) {
        Val::Int(v) => v as u16,
        _ => fonts().font(f).param_count().max(7),
    }
}

pub fn hyphen_char(c: &mut dyn Ctx, f: FontId) -> i32 {
    match c.get(&Key::HyphenChar(f)) {
        Val::Int(v) => v,
        _ if f == 0 => '-' as i32,
        _ => c.int(&Key::IntPar(ip::of("defaulthyphenchar"))),
    }
}

pub fn skew_char(c: &mut dyn Ctx, f: FontId) -> i32 {
    match c.get(&Key::SkewChar(f)) {
        Val::Int(v) => v,
        _ if f == 0 => -1,
        _ => c.int(&Key::IntPar(ip::of("defaultskewchar"))),
    }
}

/// A pdfTeX per-char code, with pdfTeX's ranges and defaults (efcode 1000).
pub fn font_code(c: &mut dyn Ctx, which: u8, f: FontId, ch: u16) -> i32 {
    let v = match c.get(&code_key(which, f, ch)) {
        Val::Int(v) => v,
        _ if which == code::EF => 1000,
        _ => 0,
    };
    match which {
        code::LP | code::RP => v.clamp(-1000, 1000),
        code::EF => v.clamp(0, 1000),
        _ => v,
    }
}

/// `\pdffontexpand` of f: (stretch, shrink, step, autoexpand), if set.
pub fn font_expand(c: &mut dyn Ctx, f: FontId) -> Option<[i32; 4]> {
    match c.get(&okey(other::EXPAND | f)) {
        Val::Ints(v) if v.len() == 4 => Some([v[0], v[1], v[2], v[3]]),
        _ => None,
    }
}

/// `\pdfnoligatures` set on f.
pub fn no_ligatures(c: &mut dyn Ctx, f: FontId) -> bool {
    c.int(&okey(other::NO_LIGS | f)) != 0
}

/// `font_id_text(f)`: the control sequence `\font` last defined f with.
pub fn font_id_text(c: &mut dyn Ctx, f: FontId) -> Option<Tok> {
    match c.get(&okey(other::ID_TEXT | f)) {
        Val::Toks(t) => t.first().copied(),
        _ => None,
    }
}

/// `\fontname f` (tex.web 1261): the name, and ` at <size>pt` if not the design size.
pub fn font_name_text(_c: &mut dyn Ctx, f: FontId) -> String {
    let font = fonts().font(f);
    let mut s = font.name().to_string();
    if font.size() != font.design_size() {
        s.push_str(" at ");
        s.push_str(&scaled_text(font.size()));
        s.push_str("pt");
    }
    s
}

fn esc(c: &mut dyn Ctx, name: &str) -> String {
    let e = c.int(&Key::IntPar(ip::of("escapechar")));
    let mut s = String::new();
    if (0..256).contains(&e) {
        if let Some(ch) = char::from_u32(e as u32) {
            s.push(ch);
        }
    }
    s.push_str(name);
    s
}

fn tok_text(c: &mut dyn Ctx, t: Tok) -> String {
    match t {
        Tok::Cs(cs) => {
            let n = c.names().name(cs);
            esc(c, &n)
        }
        Tok::Active(ch) => ch.to_string(),
        _ => esc(c, "FONT"),
    }
}

/// How a font identifier is shown (`print_esc(font_id_text(f))`).
pub fn font_ident_text(c: &mut dyn Ctx, f: FontId) -> String {
    if f == 0 {
        return esc(c, "nullfont");
    }
    match font_id_text(c, f) {
        Some(t) => tok_text(c, t),
        None => esc(c, "FONT"),
    }
}

fn tok_key(t: Tok) -> Option<Key> {
    match t {
        Tok::Cs(cs) => Some(Key::Cs(cs)),
        Tok::Active(ch) => Some(Key::Active(ch)),
        _ => None,
    }
}

fn define_global(c: &mut dyn Ctx, key: Key, val: Val) {
    c.define(key, val, true);
}

/// `\globaldefs` applied to a definition made before the final one (that
/// one goes through `assign::assign`, which also fires `\afterassignment`).
fn eff_global(c: &mut dyn Ctx, global: bool) -> bool {
    let g = c.int(&Key::IntPar(ip::of("globaldefs")));
    if g > 0 {
        true
    } else if g < 0 {
        false
    } else {
        global
    }
}

fn loaded_in_env(c: &mut dyn Ctx, f: FontId) -> bool {
    f == 0 || matches!(c.get(&okey(other::PARAMS | f)), Val::Int(_))
}

/// Defines what env keeps about a font loaded for the first time in this env
/// (tex.web 576: params, `\hyphenchar`, `\skewchar`; `font_ptr`).
fn init_font_keys(c: &mut dyn Ctx, f: FontId) {
    let font = fonts().font(f);
    let np = font.param_count().max(7);
    for n in 1..=np {
        define_global(c, Key::FontDimen(f, n), Val::Int(font.param(n)));
    }
    define_global(c, okey(other::PARAMS | f), Val::Int(np as i32));
    let h = c.int(&Key::IntPar(ip::of("defaulthyphenchar")));
    let s = c.int(&Key::IntPar(ip::of("defaultskewchar")));
    define_global(c, Key::HyphenChar(f), Val::Int(h));
    define_global(c, Key::SkewChar(f), Val::Int(s));
    if c.pdf() {
        for ch in 0..256u16 {
            define_global(c, code_key(code::EF, f, ch), Val::Int(1000));
        }
    }
    define_global(c, okey(other::LAST), Val::Int(f as i32));
}

// ---------------------------------------------------------------- \font

/// Splits a file name into (area, name) dropping the extension (TeX's
/// `cur_area`, `cur_name`; the metric file is always `name.tfm`).
fn split_name(s: &str) -> (String, String) {
    let (area, rest) = match s.rfind('/') {
        Some(i) => (&s[..=i], &s[i + 1..]),
        None => ("", s),
    };
    let name = match rest.rfind('.') {
        Some(i) if i > 0 => &rest[..i],
        _ => rest,
    };
    (area.to_string(), name.to_string())
}

/// `new_font` (tex.web 1257): `\font\x=name [at d | scaled n]`.
pub fn new_font(c: &mut dyn Ctx, global: bool) {
    let u = crate::assign::get_r_token(c);
    let Some(ukey) = tok_key(u) else { return };
    let g = eff_global(c, global);
    c.define(ukey.clone(), Val::Meaning(Meaning::Font(0)), g);
    scan::scan_optional_equals(c);
    let fname = scan::scan_file_name(c);
    let (area, name) = split_name(&fname);
    // 1258: scan_font_size
    let mut s: i32 = -1000;
    if scan::scan_keyword(c, "at") {
        s = scan::scan_normal_dimen(c);
        if s <= 0 || s >= 0o1000000000 {
            let msg = format!("Improper `at' size ({}pt), replaced by 10pt", scaled_text(s));
            error(c, &msg, &["I can only handle fonts at positive sizes that are", "less than 2048pt, so I've changed what you said to 10pt."]);
            s = 10 * UNITY;
        }
    } else if scan::scan_keyword(c, "scaled") {
        let n = scan::scan_int(c);
        s = -n;
        if n <= 0 || n > 32768 {
            error(c, "Illegal magnification has been changed to 1000", &["The magnification ratio must be between 1 and 32768."]);
            s = -1000;
        }
    }
    let full: Arc<str> = format!("{area}{name}").into();
    // 1260: a font already loaded under this name and size is shared
    let f = match c.get(&Key::FontLoaded(full.clone(), s)) {
        Val::Font(f) => f,
        _ => match c.file(&format!("{area}{name}.tfm")) {
            None => font_error(c, u, &full, s, "Metric (TFM) file not found"),
            Some((found, bytes)) => match load_tfm(&area, &name, found, bytes, s) {
                Err(()) => font_error(c, u, &full, s, "Bad metric (TFM) file"),
                Ok(f) => {
                    if !loaded_in_env(c, f) {
                        init_font_keys(c, f);
                    }
                    define_global(c, Key::FontLoaded(full.clone(), s), Val::Font(f));
                    if s < 0 {
                        let z = fonts().font(f).size();
                        define_global(c, Key::FontLoaded(full.clone(), z), Val::Font(f));
                    }
                    f
                }
            },
        },
    };
    if f != 0 {
        define_global(c, okey(other::ID_TEXT | f), Val::Toks(Arc::from(vec![u])));
    }
    crate::assign::assign(c, ukey, Val::Meaning(Meaning::Font(f)), global);
}

/// tex.web 561: "Font \x=name not loadable: …"; the result is the null font.
fn font_error(c: &mut dyn Ctx, u: Tok, full: &str, s: i32, why: &str) -> FontId {
    let mut msg = format!("Font {}={}", tok_text(c, u), full);
    if s >= 0 {
        msg.push_str(&format!(" at {}pt", scaled_text(s)));
    } else if s != -1000 {
        msg.push_str(&format!(" scaled {}", -s));
    }
    msg.push_str(" not loadable: ");
    msg.push_str(why);
    error(
        c,
        &msg,
        &[
            "I wasn't able to read the size data for this font,",
            "so I will ignore the font specification.",
            "[Wizards can fix TFM files using TFtoPL/PLtoTF.]",
            "You might try inserting a different font spec;",
            "e.g., type `I\\font<same font id>=<substitute font name>'.",
        ],
    );
    0
}

/// Selecting a font identifier `\x` (tex.web 1217, `set_font`).
pub fn select_font(c: &mut dyn Ctx, f: FontId, global: bool) {
    crate::assign::assign(c, Key::CurFont, Val::Font(f), global);
}

// ---------------------------------------------------------------- \fontdimen & co

/// `find_font_dimen` (tex.web 578): the key of `\fontdimen n f`, growing the
/// parameters of the last font loaded.
fn fontdimen_key(c: &mut dyn Ctx, _code: u16) -> Key {
    let n = scan::scan_int(c);
    let f = scan::scan_font_ident(c);
    if n > 0 {
        let np = param_count(c, f) as i32;
        if n <= np {
            return Key::FontDimen(f, n as u16);
        }
        let last = match c.get(&okey(other::LAST)) {
            Val::Int(v) => v as FontId,
            _ => 0,
        };
        if f == last && n <= u16::MAX as i32 {
            for k in np + 1..=n {
                define_global(c, Key::FontDimen(f, k as u16), Val::Int(0));
            }
            define_global(c, okey(other::PARAMS | f), Val::Int(n));
            return Key::FontDimen(f, n as u16);
        }
    }
    let np = param_count(c, f);
    let msg = format!("Font {} has only {} fontdimen parameters", font_ident_text(c, f), np);
    error(c, &msg, &["To increase the number of font parameters, you must", "use \\fontdimen immediately after the \\font is loaded."]);
    define_global(c, okey(other::DUMMY), Val::Int(0));
    okey(other::DUMMY)
}

fn hyphenchar_key(c: &mut dyn Ctx, _code: u16) -> Key {
    let f = scan::scan_font_ident(c);
    if f == 0 && matches!(c.get(&Key::HyphenChar(0)), Val::Void) {
        define_global(c, Key::HyphenChar(0), Val::Int('-' as i32));
    }
    Key::HyphenChar(f)
}

fn skewchar_key(c: &mut dyn Ctx, _code: u16) -> Key {
    let f = scan::scan_font_ident(c);
    if f == 0 && matches!(c.get(&Key::SkewChar(0)), Val::Void) {
        define_global(c, Key::SkewChar(0), Val::Int(-1));
    }
    Key::SkewChar(f)
}

/// `\lpcode f c` etc. (code = `code::*`).
fn fontcode_key(c: &mut dyn Ctx, which: u16) -> Key {
    let f = scan::scan_font_ident(c);
    let ch = scan::scan_char_num(c);
    let key = code_key(which as u8, f, ch);
    if which as u8 == code::EF && matches!(c.get(&key), Val::Void) {
        define_global(c, key.clone(), Val::Int(1000));
    }
    key
}

// ---------------------------------------------------------------- pdfTeX

/// `\pdffontexpand f stretch shrink step [autoexpand]`.
fn pdffontexpand(c: &mut dyn Ctx, _code: u16) {
    let f = scan::scan_font_ident(c);
    scan::scan_optional_equals(c);
    let stretch = scan::scan_int(c);
    let shrink = scan::scan_int(c);
    let step = scan::scan_int(c);
    let auto = scan::scan_keyword(c, "autoexpand") as i32;
    if f == 0 {
        return;
    }
    let mut stretch = stretch.clamp(0, 1000);
    let mut shrink = shrink.clamp(0, 500);
    let step = step.clamp(0, 100);
    if step == 0 {
        stretch = 0;
        shrink = 0;
    } else {
        stretch -= stretch % step;
        shrink -= shrink % step;
    }
    if stretch == 0 && shrink == 0 {
        error(c, "invalid expansion values: stretch and shrink are both zero", &[]);
        return;
    }
    define_global(c, okey(other::EXPAND | f), Val::Ints(Arc::from(vec![stretch, shrink, step, auto])));
}

/// A copy of f as a new font (`\pdfcopyfont`, `\letterspacefont`).
fn copy_font(c: &mut dyn Ctx, f: FontId, ls: i32) -> FontId {
    let n = c.int(&okey(other::COPIES | f)) as u32 + 1;
    define_global(c, okey(other::COPIES | f), Val::Int(n as i32));
    let base = fonts().font(f);
    let nf = table()
        .intern(TableKey::Copy { base: f, n, ls }, || {
            let mut x = (*base).clone();
            x.copy = n;
            x.letterspace = ls;
            Ok(x)
        })
        .unwrap_or(0);
    if !loaded_in_env(c, nf) {
        // a copy takes the original's current parameters
        let np = param_count(c, f);
        for k in 1..=np {
            let v = font_dimen(c, f, k);
            define_global(c, Key::FontDimen(nf, k), Val::Int(v));
        }
        define_global(c, okey(other::PARAMS | nf), Val::Int(np as i32));
        let h = hyphen_char(c, f);
        let s = skew_char(c, f);
        define_global(c, Key::HyphenChar(nf), Val::Int(h));
        define_global(c, Key::SkewChar(nf), Val::Int(s));
        if let Some(e) = font_expand(c, f) {
            define_global(c, okey(other::EXPAND | nf), Val::Ints(Arc::from(e.to_vec())));
        }
        define_global(c, okey(other::LAST), Val::Int(nf as i32));
    }
    nf
}

fn define_copy(c: &mut dyn Ctx, global: bool, letterspace: bool) {
    let u = crate::assign::get_r_token(c);
    let Some(ukey) = tok_key(u) else { return };
    let g = eff_global(c, global);
    c.define(ukey.clone(), Val::Meaning(Meaning::Font(0)), g);
    let f = scan::scan_font_ident(c);
    let ls = if letterspace { scan::scan_int(c).clamp(-1000, 1000) } else { fonts().font(f).letterspace };
    let nf = if f == 0 { 0 } else { copy_font(c, f, ls) };
    if nf != 0 {
        define_global(c, okey(other::ID_TEXT | nf), Val::Toks(Arc::from(vec![u])));
    }
    crate::assign::assign(c, ukey, Val::Meaning(Meaning::Font(nf)), global);
}

/// `\pdfcopyfont\x f`.
pub fn pdfcopyfont(c: &mut dyn Ctx, global: bool) {
    define_copy(c, global, false);
}

/// `\letterspacefont\x f n`.
pub fn letterspacefont(c: &mut dyn Ctx, global: bool) {
    define_copy(c, global, true);
}

fn pdfnoligatures(c: &mut dyn Ctx, _code: u16) {
    let f = scan::scan_font_ident(c);
    if f != 0 {
        define_global(c, okey(other::NO_LIGS | f), Val::Int(1));
    }
}

// ---------------------------------------------------------------- registration

pub fn register(r: &mut Registry) {
    r.add("font", 0, Kind::Command(|c, _| new_font(c, false)), 0);
    r.add("nullfont", 0, Kind::Command(|c, _| select_font(c, 0, false)), 0);
    r.add("fontdimen", 0, Kind::Quantity { level: Level::Dimen, key: fontdimen_key }, 0);
    r.add("hyphenchar", 0, Kind::Quantity { level: Level::Int, key: hyphenchar_key }, 0);
    r.add("skewchar", 0, Kind::Quantity { level: Level::Int, key: skewchar_key }, 0);
    r.add("pdffontexpand", 0, Kind::Command(pdffontexpand), 2);
    r.add("pdfcopyfont", 0, Kind::Command(|c, _| pdfcopyfont(c, false)), 2);
    r.add("letterspacefont", 0, Kind::Command(|c, _| letterspacefont(c, false)), 2);
    r.add("pdfnoligatures", 0, Kind::Command(pdfnoligatures), 2);
    for (i, n) in code::NAMES.iter().enumerate() {
        r.add(n, i as u16, Kind::Quantity { level: Level::Int, key: fontcode_key }, 2);
    }
}
