//! E5: fonts in the PDF: pdftex.map, Type 1 subsetting, TrueType/OTF via
//! partex-otf, VF, encodings, ToUnicode.
//!
//! `FontSet` is the back end's font table. The page writer (E1) calls
//! [`FontSet::resource`] for every (font, expansion) a page uses (that
//! allocates the font dictionary's object number) and [`FontSet::use_glyph`]
//! for every character shown; at the end of a build [`FontSet::write`] writes
//! every font dictionary, its `/Widths`, `/FontDescriptor`, embedded file,
//! `/Encoding` and `/ToUnicode`.
//!
//! Incremental: the embedded font file streams (subset, encrypted,
//! compressed) are cached on (font file, glyph set); a font whose glyph set
//! did not change between builds is not subset again.
//!
//! Map directives (`\pdfmapfile`, `\pdfmapline`) and `\pdfglyphtounicode`
//! are global values in env (`Key::Other(tag::PDF, MAP_OPS / GLYPH_UNI)`);
//! E1 hands them to [`WriteOpts`].
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::prim::{tag, Kind, Registry};
use crate::tok::Tok;
use crate::val::{Datum, FontId, Key, Val};

// ---------------------------------------------------------------- primitives

/// `Key::Other(tag::PDF, MAP_OPS)`: the map directives so far (`MapOps`).
pub const MAP_OPS: u32 = 0x4D41_5000;
/// `Key::Other(tag::PDF, GLYPH_UNI)`: `\pdfglyphtounicode` pairs (`GlyphUni`).
pub const GLYPH_UNI: u32 = 0x4D41_5001;

pub fn register(r: &mut Registry) {
    r.add("pdfmapfile", 0, Kind::Command(pdf_map), 2);
    r.add("pdfmapline", 1, Kind::Command(pdf_map), 2);
    r.add("pdfglyphtounicode", 0, Kind::Command(pdf_glyph_to_unicode), 2);
}

/// A map directive: `\pdfmapfile` (`file`) or `\pdfmapline`, with its
/// prefix: `+` add, `=` replace, `-` delete, ` ` none (replace the map).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MapOp {
    pub mode: char,
    pub file: bool,
    pub text: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct MapOps(pub Vec<MapOp>);
impl Datum for MapOps {
    fn same(&self, o: &dyn Datum) -> bool {
        o.as_any().downcast_ref::<MapOps>() == Some(self)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `\pdfglyphtounicode` pairs: glyph name (or `tfm:NAME/glyph`) → code points.
#[derive(Debug, PartialEq, Eq, Default, Clone)]
pub struct GlyphUni(pub BTreeMap<String, Vec<u32>>);
impl Datum for GlyphUni {
    fn same(&self, o: &dyn Datum) -> bool {
        o.as_any().downcast_ref::<GlyphUni>() == Some(self)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The text of an expanded general text, as pdfTeX's `makecstring`.
fn toks_text(c: &mut dyn Ctx, toks: &[Tok]) -> String {
    let esc = c.int(&Key::IntPar(crate::prim::ip::of("escapechar")));
    let mut s = String::new();
    for t in toks {
        match *t {
            Tok::Char { ch, .. } | Tok::Active(ch) => s.push(ch),
            Tok::Cs(cs) => {
                if let Some(e) = u32::try_from(esc).ok().and_then(char::from_u32) {
                    s.push(e);
                }
                let n = c.names().name(cs);
                s.push_str(&n);
                let single_other = n.chars().count() == 1 && !n.chars().all(char::is_alphabetic);
                if !single_other {
                    s.push(' ');
                }
            }
            Tok::Param(n) => {
                s.push('#');
                if n > 0 {
                    s.push(char::from(b'0' + n));
                }
            }
            Tok::DontExpand => {}
        }
    }
    s
}

fn scan_text(c: &mut dyn Ctx) -> String {
    let (_, body) = crate::scan::scan_toks(c, false, true);
    toks_text(c, &body)
}

fn pdf_map(c: &mut dyn Ctx, code: u16) {
    let text = scan_text(c);
    let t = text.trim_start();
    let (mode, rest) = match t.chars().next() {
        Some(m @ ('+' | '=' | '-')) => (m, t[1..].trim_start()),
        _ => (' ', t),
    };
    let op = MapOp { mode, file: code == 0, text: rest.trim_end().to_string() };
    if op.file && op.text.is_empty() {
        return;
    }
    let key = Key::Other(tag::PDF, MAP_OPS);
    let mut ops = match c.get(&key) {
        Val::Data(d) => d.as_any().downcast_ref::<MapOps>().map(|m| m.0.clone()).unwrap_or_default(),
        _ => Vec::new(),
    };
    ops.push(op);
    c.define(key, Val::Data(Arc::new(MapOps(ops))), true);
}

fn pdf_glyph_to_unicode(c: &mut dyn Ctx, _code: u16) {
    let name = scan_text(c);
    let uni = scan_text(c);
    let name = name.trim().to_string();
    if name.is_empty() {
        return;
    }
    let key = Key::Other(tag::PDF, GLYPH_UNI);
    let mut g = match c.get(&key) {
        Val::Data(d) => d.as_any().downcast_ref::<GlyphUni>().cloned().unwrap_or_default(),
        _ => GlyphUni::default(),
    };
    match parse_uni_hex(&uni) {
        Some(v) => {
            g.0.insert(name, v);
        }
        None => {
            g.0.remove(&name);
        }
    }
    c.define(key, Val::Data(Arc::new(g)), true);
}

/// `0041`, `0066 0069` (code points of 4–6 hex digits), or `D835DC00`
/// (UTF-16 units run together).
fn parse_uni_hex(s: &str) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for w in s.split_whitespace() {
        if !w.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        if (4..=6).contains(&w.len()) {
            out.push(u32::from_str_radix(w, 16).ok()?);
        } else if w.len() % 4 == 0 {
            let mut units = Vec::new();
            for i in (0..w.len()).step_by(4) {
                units.push(u16::from_str_radix(&w[i..i + 4], 16).ok()?);
            }
            out.extend(char::decode_utf16(units).map(|r| r.map_or(0xFFFD, u32::from)));
        } else {
            return None;
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

// ---------------------------------------------------------------- arithmetic

/// pdfTeX's `divide_scaled(s, m, dd)`: `s/m * 10^dd`, rounded.
pub fn divide_scaled(s: i32, m: i32, dd: u32) -> i32 {
    if m == 0 {
        return 0;
    }
    let mut sign = 1i64;
    let (mut s, mut m) = (i64::from(s), i64::from(m));
    if s < 0 {
        sign = -sign;
        s = -s;
    }
    if m < 0 {
        sign = -sign;
        m = -m;
    }
    let mut q = s / m;
    let mut r = s % m;
    for _ in 0..dd {
        q = 10 * q + (10 * r) / m;
        r = (10 * r) % m;
    }
    if 2 * r >= m {
        q += 1;
    }
    (sign * q) as i32
}

/// `round_xn_over_d(x, n, d)`: `x*n/d` rounded half away from zero.
pub fn round_xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    let neg = (x < 0) != (n < 0);
    let p = i64::from(x).abs() * i64::from(n).abs();
    let d = i64::from(d);
    let q = (2 * p + d) / (2 * d);
    (if neg { -q } else { q }) as i32
}

// ---------------------------------------------------------------- map files

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Embed {
    /// No font file: a non-embedded font.
    None,
    /// `<file`: subset.
    Subset,
    /// `<<file` (or the old `<[file`): the whole file.
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Type1,
    TrueType,
    OpenType,
}

/// One pdftex.map line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapEntry {
    pub tfm: String,
    pub ps_name: Option<String>,
    pub flags: Option<u32>,
    /// `SlantFont` / `ExtendFont` in thousandths (0: none).
    pub slant: i32,
    pub extend: i32,
    pub enc: Option<String>,
    pub file: Option<String>,
    pub embed: Embed,
}

impl MapEntry {
    pub fn kind(&self) -> Option<FileKind> {
        let f = self.file.as_deref()?.to_ascii_lowercase();
        Some(if f.ends_with(".ttf") || f.ends_with(".ttc") {
            FileKind::TrueType
        } else if f.ends_with(".otf") {
            FileKind::OpenType
        } else {
            FileKind::Type1
        })
    }
}

fn thousandths(s: &str) -> Option<i32> {
    let v: f64 = s.parse().ok()?;
    Some((v * 1000.0).round() as i32)
}

/// Parses one map line (pdfTeX's `fm_scan_line`). `None`: comment, blank or invalid.
pub fn parse_map_line(line: &str) -> Option<MapEntry> {
    let line = line.trim();
    match line.bytes().next() {
        None | Some(b'%' | b'#' | b';' | b'*') => return None,
        _ => {}
    }
    let b = line.as_bytes();
    let word = |i: &mut usize| -> String {
        let s = *i;
        while *i < b.len() && !b[*i].is_ascii_whitespace() && b[*i] != b'"' && b[*i] != b'<' {
            *i += 1;
        }
        line[s..*i].to_string()
    };
    let skip = |i: &mut usize| {
        while *i < b.len() && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
    };
    let mut i = 0;
    let tfm = word(&mut i);
    if tfm.is_empty() {
        return None;
    }
    let mut e = MapEntry {
        tfm,
        ps_name: None,
        flags: None,
        slant: 0,
        extend: 0,
        enc: None,
        file: None,
        embed: Embed::None,
    };
    loop {
        skip(&mut i);
        if i >= b.len() {
            break;
        }
        match b[i] {
            b'"' => {
                i += 1;
                let s = i;
                while i < b.len() && b[i] != b'"' {
                    i += 1;
                }
                let special = &line[s..i];
                i = (i + 1).min(b.len());
                let ws: Vec<&str> = special.split_whitespace().collect();
                for (k, w) in ws.iter().enumerate() {
                    match *w {
                        "SlantFont" if k > 0 => e.slant = thousandths(ws[k - 1]).unwrap_or(0),
                        "ExtendFont" if k > 0 => e.extend = thousandths(ws[k - 1]).unwrap_or(0),
                        _ => {}
                    }
                }
            }
            b'<' => {
                i += 1;
                let mut mode = Embed::Subset;
                let mut bracket = false;
                if i < b.len() && b[i] == b'<' {
                    mode = Embed::Full;
                    i += 1;
                } else if i < b.len() && b[i] == b'[' {
                    bracket = true;
                    i += 1;
                }
                skip(&mut i);
                let name = word(&mut i);
                if name.is_empty() {
                    continue;
                }
                if name.to_ascii_lowercase().ends_with(".enc") {
                    e.enc = Some(name);
                } else {
                    e.file = Some(name);
                    e.embed = if bracket { Embed::Full } else { mode };
                }
            }
            c if c.is_ascii_digit() && e.flags.is_none() => {
                let w = word(&mut i);
                match w.parse() {
                    Ok(f) => e.flags = Some(f),
                    Err(_) => {
                        if e.ps_name.is_none() {
                            e.ps_name = Some(w);
                        }
                    }
                }
            }
            _ => {
                let w = word(&mut i);
                if w.is_empty() {
                    i += 1;
                } else if e.ps_name.is_none() {
                    e.ps_name = Some(w);
                }
            }
        }
    }
    if e.file.is_none() && e.ps_name.is_none() {
        return None;
    }
    Some(e)
}

/// The map: TFM name → entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Map {
    pub entries: HashMap<String, MapEntry>,
}

impl Map {
    fn apply_line(&mut self, mode: char, line: &str, warn: &mut Vec<String>) {
        let Some(e) = parse_map_line(line) else { return };
        match mode {
            '-' => {
                self.entries.remove(&e.tfm);
            }
            '+' => {
                if self.entries.contains_key(&e.tfm) {
                    warn.push(format!("pdfTeX warning: entry for `{}' already exists, duplicates ignored", e.tfm));
                } else {
                    self.entries.insert(e.tfm.clone(), e);
                }
            }
            _ => {
                self.entries.insert(e.tfm.clone(), e);
            }
        }
    }

    fn apply_file(&mut self, mode: char, data: &[u8], warn: &mut Vec<String>) {
        let text = String::from_utf8_lossy(data);
        for line in text.lines() {
            self.apply_line(mode, line, warn);
        }
    }

    /// The map after `ops` (pdftex.map first, unless a prefix-less
    /// directive replaces it).
    pub fn build(ops: &[MapOp], loader: &Loader, warn: &mut Vec<String>) -> Map {
        let mut m = Map::default();
        if !ops.iter().any(|o| o.mode == ' ') {
            match loader("pdftex.map") {
                Some(d) => m.apply_file('+', &d, warn),
                None => warn.push("pdfTeX warning: cannot open map file pdftex.map".into()),
            }
        }
        for o in ops {
            if o.mode == ' ' {
                m.entries.clear();
            }
            let mode = if o.mode == ' ' { '=' } else { o.mode };
            if o.file {
                match loader(&o.text) {
                    Some(d) => m.apply_file(mode, &d, warn),
                    None => warn.push(format!("pdfTeX warning: cannot open map file {}", o.text)),
                }
            } else {
                m.apply_line(mode, &o.text, warn);
            }
        }
        m
    }
}

// ---------------------------------------------------------------- encodings

pub type Names256 = Arc<[Option<String>; 256]>;

fn empty256() -> [Option<String>; 256] {
    std::array::from_fn(|_| None)
}

/// An `.enc` file: its PostScript name and its 256 glyph names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoding {
    pub name: String,
    pub glyphs: Names256,
}

/// Parses a PostScript encoding vector file (`/Name [ /a /b … ] def`).
pub fn parse_enc(data: &[u8]) -> Option<Encoding> {
    let text = String::from_utf8_lossy(data);
    let mut toks = Vec::new();
    for line in text.lines() {
        let line = line.split('%').next().unwrap_or("");
        let mut cur = String::new();
        for ch in line.chars() {
            match ch {
                '[' | ']' | '{' | '}' => {
                    if !cur.is_empty() {
                        toks.push(std::mem::take(&mut cur));
                    }
                    toks.push(ch.to_string());
                }
                '/' => {
                    if !cur.is_empty() {
                        toks.push(std::mem::take(&mut cur));
                    }
                    cur.push('/');
                }
                c if c.is_whitespace() => {
                    if !cur.is_empty() {
                        toks.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            toks.push(cur);
        }
    }
    let open = toks.iter().position(|t| t == "[")?;
    let name = toks[..open].iter().rev().find_map(|t| t.strip_prefix('/')).unwrap_or("").to_string();
    let mut g = empty256();
    let mut k = 0usize;
    for t in &toks[open + 1..] {
        if t == "]" {
            break;
        }
        if let Some(n) = t.strip_prefix('/') {
            if k < 256 {
                g[k] = Some(n.to_string());
            }
            k += 1;
        }
    }
    Some(Encoding { name, glyphs: Arc::new(g) })
}

/// Adobe StandardEncoding.
pub fn standard_encoding(c: u8) -> Option<&'static str> {
    const LOW: [&str; 95] = [
        "space", "exclam", "quotedbl", "numbersign", "dollar", "percent", "ampersand", "quoteright",
        "parenleft", "parenright", "asterisk", "plus", "comma", "hyphen", "period", "slash", "zero",
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "colon", "semicolon",
        "less", "equal", "greater", "question", "at", "A", "B", "C", "D", "E", "F", "G", "H", "I",
        "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
        "bracketleft", "backslash", "bracketright", "asciicircum", "underscore", "quoteleft", "a",
        "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
        "t", "u", "v", "w", "x", "y", "z", "braceleft", "bar", "braceright", "asciitilde",
    ];
    Some(match c {
        32..=126 => LOW[(c - 32) as usize],
        161 => "exclamdown",
        162 => "cent",
        163 => "sterling",
        164 => "fraction",
        165 => "yen",
        166 => "florin",
        167 => "section",
        168 => "currency",
        169 => "quotesingle",
        170 => "quotedblleft",
        171 => "guillemotleft",
        172 => "guilsinglleft",
        173 => "guilsinglright",
        174 => "fi",
        175 => "fl",
        177 => "endash",
        178 => "dagger",
        179 => "daggerdbl",
        180 => "periodcentered",
        182 => "paragraph",
        183 => "bullet",
        184 => "quotesinglbase",
        185 => "quotedblbase",
        186 => "quotedblright",
        187 => "guillemotright",
        188 => "ellipsis",
        189 => "perthousand",
        191 => "questiondown",
        193 => "grave",
        194 => "acute",
        195 => "circumflex",
        196 => "tilde",
        197 => "macron",
        198 => "breve",
        199 => "dotaccent",
        200 => "dieresis",
        202 => "ring",
        203 => "cedilla",
        205 => "hungarumlaut",
        206 => "ogonek",
        207 => "caron",
        208 => "emdash",
        225 => "AE",
        227 => "ordfeminine",
        232 => "Lslash",
        233 => "Oslash",
        234 => "OE",
        235 => "ordmasculine",
        241 => "ae",
        245 => "dotlessi",
        248 => "lslash",
        249 => "oslash",
        250 => "oe",
        251 => "germandbls",
        _ => return None,
    })
}

// ---------------------------------------------------------------- Type 1

const EEXEC_R: u16 = 55665;
const CS_R: u16 = 4330;
const C1: u16 = 52845;
const C2: u16 = 22719;

fn decrypt(data: &[u8], mut r: u16) -> Vec<u8> {
    data.iter()
        .map(|&c| {
            let p = c ^ (r >> 8) as u8;
            r = u16::from(c).wrapping_add(r).wrapping_mul(C1).wrapping_add(C2);
            p
        })
        .collect()
}

fn encrypt(data: &[u8], mut r: u16) -> Vec<u8> {
    data.iter()
        .map(|&p| {
            let c = p ^ (r >> 8) as u8;
            r = u16::from(c).wrapping_add(r).wrapping_mul(C1).wrapping_add(C2);
            c
        })
        .collect()
}

/// A Type 1 font split into its parts.
#[derive(Clone, Debug)]
pub struct Type1 {
    /// Clear text, line ends normalised to `\n`, through `currentfile eexec\n`.
    pub clear: Vec<u8>,
    /// Decrypted private part (with its 4 leading bytes).
    pub private: Vec<u8>,
    /// Encrypted bytes as in the file (for full embedding).
    pub binary: Vec<u8>,
    pub font_name: String,
    pub bbox: [i32; 4],
    pub italic_angle: i32,
    pub std_vw: Option<i32>,
    /// The built-in encoding; `None`: StandardEncoding.
    pub builtin: Option<Names256>,
}

fn normalize_eol(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\r' {
            out.push(b'\n');
            if i + 1 < b.len() && b[i + 1] == b'\n' {
                i += 1;
            }
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    out
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > hay.len() || needle.is_empty() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn t1_round(x: f64) -> i32 {
    if x >= 0.0 { (x + 0.5).floor() as i32 } else { -((-x + 0.5).floor() as i32) }
}

fn ends_def(l: &str) -> bool {
    l.ends_with("def") || l.contains(" def ")
}

/// Splits a PFB or PFA file.
pub fn parse_type1(data: &[u8]) -> Option<Type1> {
    let (clear_raw, binary) = if data.first() == Some(&0x80) {
        let mut clear = Vec::new();
        let mut bin = Vec::new();
        let mut i = 0;
        while i + 1 < data.len() && data[i] == 0x80 {
            let t = data[i + 1];
            if t == 3 {
                break;
            }
            if i + 6 > data.len() {
                return None;
            }
            let len = u32::from_le_bytes([data[i + 2], data[i + 3], data[i + 4], data[i + 5]]) as usize;
            let s = i + 6;
            let e = (s + len).min(data.len());
            match t {
                1 => {
                    if bin.is_empty() {
                        clear.extend_from_slice(&data[s..e]);
                    }
                }
                2 => bin.extend_from_slice(&data[s..e]),
                _ => return None,
            }
            i = e;
        }
        (clear, bin)
    } else {
        let p = find(data, b"eexec", 0)?;
        let mut e = p + 5;
        while e < data.len() && matches!(data[e], b'\r' | b'\n' | b' ' | b'\t') {
            e += 1;
        }
        let clear = data[..e].to_vec();
        let mut bin = Vec::new();
        let mut hi: Option<u8> = None;
        for &c in &data[e..] {
            let v = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                b' ' | b'\t' | b'\r' | b'\n' => continue,
                _ => break,
            };
            match hi.take() {
                None => hi = Some(v),
                Some(h) => bin.push(h << 4 | v),
            }
        }
        (clear, bin)
    };
    let mut clear = normalize_eol(&clear_raw);
    while clear.last().is_some_and(u8::is_ascii_whitespace) {
        clear.pop();
    }
    clear.push(b'\n');
    let private = decrypt(&binary, EEXEC_R);
    let ctext = String::from_utf8_lossy(&clear).into_owned();

    let mut font_name = String::new();
    let mut bbox = [0; 4];
    let mut italic_angle = 0;
    let mut builtin: Option<[Option<String>; 256]> = None;
    let mut in_enc = false;
    for line in ctext.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("/FontName") {
            if let Some(n) = r.split_whitespace().next().and_then(|w| w.strip_prefix('/')) {
                font_name = n.to_string();
            }
        } else if let Some(r) = l.strip_prefix("/FontBBox") {
            let nums: Vec<i32> = r
                .replace(['{', '}', '[', ']'], " ")
                .split_whitespace()
                .filter_map(|w| w.parse::<f64>().ok())
                .map(t1_round)
                .collect();
            if nums.len() >= 4 {
                bbox = [nums[0], nums[1], nums[2], nums[3]];
            }
        } else if let Some(r) = l.strip_prefix("/ItalicAngle") {
            if let Some(v) = r.split_whitespace().next().and_then(|w| w.parse::<f64>().ok()) {
                italic_angle = t1_round(v);
            }
        } else if l.starts_with("/Encoding") && !l.contains("StandardEncoding") {
            in_enc = true;
            builtin = Some(empty256());
        }
        if in_enc {
            let ws: Vec<&str> = l.split_whitespace().collect();
            let mut k = 0;
            while k + 2 < ws.len() {
                if ws[k] == "dup" {
                    if let (Ok(code), Some(name)) = (ws[k + 1].parse::<usize>(), ws[k + 2].strip_prefix('/')) {
                        if code < 256 {
                            if let Some(b) = builtin.as_mut() {
                                b[code] = Some(name.to_string());
                            }
                        }
                        k += 3;
                        continue;
                    }
                }
                k += 1;
            }
            if ends_def(l) {
                in_enc = false;
            }
        }
    }
    let ptext = String::from_utf8_lossy(&private);
    let std_vw = ptext.find("/StdVW").and_then(|p| {
        ptext[p + 6..]
            .replace(['[', ']', '{', '}'], " ")
            .split_whitespace()
            .next()
            .and_then(|w| w.parse::<f64>().ok())
            .map(t1_round)
    });
    Some(Type1 {
        clear,
        private,
        binary,
        font_name,
        bbox,
        italic_angle,
        std_vw,
        builtin: builtin.map(Arc::new),
    })
}

/// The private part, cut into its pieces.
struct Private {
    /// Text before `/Subrs` (or before `/CharStrings` without Subrs).
    pre: Vec<u8>,
    /// `(index, encrypted charstring)`.
    subrs: Vec<(usize, Vec<u8>)>,
    has_subrs: bool,
    /// Text between the Subrs and `/CharStrings`.
    mid: Vec<u8>,
    /// `(name, encrypted charstring)` in file order.
    chars: Vec<(String, Vec<u8>)>,
    post: Vec<u8>,
    rd: String,
    np: String,
    nd: String,
    len_iv: i32,
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn tok_str(b: &[u8], i: usize) -> (String, usize) {
    let s = skip_ws(b, i);
    let mut e = s;
    while e < b.len() && !b[e].is_ascii_whitespace() {
        e += 1;
    }
    (String::from_utf8_lossy(&b[s..e]).into_owned(), e)
}

/// Finds `needle` as a whole token.
fn find_token(b: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let mut f = from;
    while let Some(p) = find(b, needle, f) {
        let before_ok = p == 0 || b[p - 1].is_ascii_whitespace() || b[p - 1] == b'{' || b[p - 1] == b'}';
        let after = p + needle.len();
        let after_ok = after >= b.len() || b[after].is_ascii_whitespace();
        if before_ok && after_ok {
            return Some(p);
        }
        f = p + 1;
    }
    None
}

/// Reads `n RD <n bytes>` at `i`; returns (bytes, rd token, index after).
fn read_cs(b: &[u8], i: usize) -> Option<(Vec<u8>, String, usize)> {
    let (n, i) = tok_str(b, i);
    let n: usize = n.parse().ok()?;
    let (rd, i) = tok_str(b, i);
    let s = i + 1; // one space
    let e = s + n;
    if e > b.len() {
        return None;
    }
    Some((b[s..e].to_vec(), rd, e))
}

/// The rest of the line after a charstring (`NP`, `|`, `ND`, `|-` …).
fn rest_of_line(b: &[u8], i: usize) -> (String, usize) {
    let mut e = i;
    while e < b.len() && b[e] != b'\n' && b[e] != b'\r' {
        e += 1;
    }
    let s = String::from_utf8_lossy(&b[i..e]).trim().to_string();
    if e < b.len() && b[e] == b'\r' {
        e += 1;
    }
    if e < b.len() && b[e] == b'\n' {
        e += 1;
    }
    (s, e)
}

fn parse_private(p: &[u8]) -> Option<Private> {
    let head = String::from_utf8_lossy(&p[..p.len().min(8192)]).into_owned();
    let len_iv = head
        .find("/lenIV")
        .and_then(|k| head[k + 6..].split_whitespace().next().and_then(|w| w.parse().ok()))
        .unwrap_or(4);
    let first_cs = find_token(p, b"/CharStrings", 0)?;
    let subrs_at = find_token(p, b"/Subrs", 0).filter(|&s| s < first_cs);
    let mut rd = "RD".to_string();
    let mut np = "NP".to_string();
    let mut nd = "ND".to_string();
    let mut subrs = Vec::new();
    let mut after_subrs = 0;
    if let Some(s) = subrs_at {
        let (_, i) = tok_str(p, s + 6); // count
        let (_, mut i) = tok_str(p, i); // array
        loop {
            let (t, j) = tok_str(p, i);
            if t != "dup" {
                break;
            }
            let (idx, j) = tok_str(p, j);
            let Ok(idx) = idx.parse::<usize>() else { break };
            let Some((bytes, r, j)) = read_cs(p, j) else { break };
            rd = r;
            let (tail, j) = rest_of_line(p, j);
            if !tail.is_empty() {
                np = tail;
            }
            subrs.push((idx, bytes));
            i = j;
        }
        after_subrs = skip_ws(p, i);
    }
    // `/CharStrings` after the subrs (their binary may contain the bytes).
    let cs_at = find_token(p, b"/CharStrings", after_subrs)?;
    let (pre, mid) = match subrs_at {
        Some(s) => (p[..s].to_vec(), p[after_subrs.min(cs_at)..cs_at].to_vec()),
        None => (p[..cs_at].to_vec(), Vec::new()),
    };
    // `/CharStrings N dict dup begin`
    let (_, i) = rest_of_line(p, cs_at);
    let mut i = i;
    let mut chars = Vec::new();
    loop {
        let (t, j) = tok_str(p, i);
        let Some(name) = t.strip_prefix('/') else { break };
        let Some((bytes, r, j)) = read_cs(p, j) else { break };
        rd = r;
        let (tail, j) = rest_of_line(p, j);
        if !tail.is_empty() {
            nd = tail;
        }
        chars.push((name.to_string(), bytes));
        i = j;
    }
    let s = skip_ws(p, i);
    let mut post = p[s..].to_vec();
    if let Some(k) = find(&post, b"closefile", 0) {
        post.truncate(k + 9);
        post.push(b'\n');
    }
    Some(Private { pre, subrs, has_subrs: subrs_at.is_some(), mid, chars, post, rd, np, nd, len_iv })
}

fn cs_plain(data: &[u8], len_iv: i32) -> Vec<u8> {
    if len_iv < 0 {
        return data.to_vec();
    }
    let d = decrypt(data, CS_R);
    d.get(len_iv as usize..).map(<[u8]>::to_vec).unwrap_or_default()
}

/// Walks a Type 1 charstring: marks the subrs it calls (recursively) and
/// collects `seac` components (base, accent).
fn cs_walk(code: &[u8], subrs: &HashMap<usize, Vec<u8>>, used: &mut BTreeSet<usize>, seac: &mut Vec<(u8, u8)>, depth: u32) {
    if depth > 20 {
        return;
    }
    let mut stack: Vec<i32> = Vec::new();
    let mut ps: Vec<i32> = Vec::new();
    let mut i = 0;
    while i < code.len() {
        let b = code[i];
        i += 1;
        match b {
            32..=246 => stack.push(i32::from(b) - 139),
            247..=250 => {
                let w = *code.get(i).unwrap_or(&0);
                i += 1;
                stack.push((i32::from(b) - 247) * 256 + i32::from(w) + 108);
            }
            251..=254 => {
                let w = *code.get(i).unwrap_or(&0);
                i += 1;
                stack.push(-(i32::from(b) - 251) * 256 - i32::from(w) - 108);
            }
            255 => {
                if i + 4 > code.len() {
                    return;
                }
                stack.push(i32::from_be_bytes([code[i], code[i + 1], code[i + 2], code[i + 3]]));
                i += 4;
            }
            10 => {
                let n = stack.pop().unwrap_or(-1);
                if let Ok(n) = usize::try_from(n) {
                    if used.insert(n) {
                        if let Some(s) = subrs.get(&n) {
                            cs_walk(s, subrs, used, seac, depth + 1);
                        }
                    }
                }
            }
            11 | 14 => return,
            12 => {
                let op = *code.get(i).unwrap_or(&0);
                i += 1;
                match op {
                    6 => {
                        let n = stack.len();
                        if n >= 2 {
                            seac.push((stack[n - 2] as u8, stack[n - 1] as u8));
                        }
                        return;
                    }
                    12 => {
                        let d = stack.pop().unwrap_or(1);
                        let a = stack.pop().unwrap_or(0);
                        stack.push(if d == 0 { 0 } else { a / d });
                    }
                    16 => {
                        let _other = stack.pop();
                        let n = stack.pop().unwrap_or(0).max(0);
                        for _ in 0..n {
                            ps.push(stack.pop().unwrap_or(0));
                        }
                    }
                    17 => stack.push(ps.pop().unwrap_or(0)),
                    _ => stack.clear(),
                }
            }
            _ => stack.clear(),
        }
    }
}

/// pdfTeX's `make_subset_tag`: md5 of the glyph names (sorted, each followed
/// by a space), the font name and a collision counter.
pub fn subset_tag(glyphs: &BTreeSet<String>, font_name: &str, taken: &mut HashSet<String>) -> String {
    let mut j: i32 = 0;
    loop {
        let mut buf = Vec::new();
        for g in glyphs {
            buf.extend_from_slice(g.as_bytes());
            buf.push(b' ');
        }
        buf.extend_from_slice(font_name.as_bytes());
        buf.extend_from_slice(&j.to_le_bytes());
        let d = phitex_codecs::md5::md5(&buf);
        let mut a = [0i32; 6];
        a[0] = d[..13].iter().map(|&x| i32::from(x)).sum();
        for i in 1..6 {
            a[i] = a[i - 1] - i32::from(d[i - 1]) + i32::from(d[(i + 12) % 16]);
        }
        let tag: String = a.iter().map(|&x| char::from(b'A' + x.rem_euclid(26) as u8)).collect();
        if taken.insert(tag.clone()) {
            return tag;
        }
        j += 1;
    }
}

/// A subset font file (writet1): clear part, encrypted part, glyphs kept.
pub struct T1Subset {
    pub clear: Vec<u8>,
    pub binary: Vec<u8>,
    pub charset: BTreeSet<String>,
}

/// Subsets `t1` to `glyphs` (plus `.notdef`, seac components, subrs 0–3 and
/// every subr called), renames it `TAG+Name`, re-encrypts.
pub fn subset_type1(
    t1: &Type1,
    glyphs: &BTreeSet<String>,
    tag: &str,
    reencoded: bool,
    used_codes: &BTreeMap<u8, String>,
) -> Option<T1Subset> {
    let pv = parse_private(&t1.private)?;
    let cs_map: HashMap<&str, &Vec<u8>> = pv.chars.iter().map(|(n, b)| (n.as_str(), b)).collect();
    let subrs_plain: HashMap<usize, Vec<u8>> = pv.subrs.iter().map(|(i, b)| (*i, cs_plain(b, pv.len_iv))).collect();
    let mut keep: BTreeSet<String> = BTreeSet::new();
    let mut work: Vec<String> = glyphs.iter().cloned().collect();
    work.push(".notdef".into());
    let mut used_subrs: BTreeSet<usize> = (0..4).filter(|i| subrs_plain.contains_key(i)).collect();
    while let Some(g) = work.pop() {
        if keep.contains(&g) {
            continue;
        }
        let Some(cs) = cs_map.get(g.as_str()) else { continue };
        keep.insert(g.clone());
        let mut seac = Vec::new();
        cs_walk(&cs_plain(cs, pv.len_iv), &subrs_plain, &mut used_subrs, &mut seac, 0);
        for (b, a) in seac {
            for c in [b, a] {
                if let Some(n) = standard_encoding(c) {
                    if !keep.contains(n) {
                        work.push(n.to_string());
                    }
                }
            }
        }
    }
    // --- clear part
    let ctext = String::from_utf8_lossy(&t1.clear).into_owned();
    let mut clear = String::new();
    let mut in_enc = false;
    for line in ctext.split_inclusive('\n') {
        let l = line.trim();
        if in_enc {
            if ends_def(l) {
                in_enc = false;
            }
            continue;
        }
        if l.starts_with("/UniqueID") {
            continue;
        }
        if let Some(r) = l.strip_prefix("/FontName") {
            let r = r.trim_start();
            let r = r.strip_prefix('/').unwrap_or(r);
            let rest = r.find(char::is_whitespace).map_or("", |k| &r[k..]);
            clear.push_str(&format!("/FontName /{}+{}{}\n", tag, t1.font_name, rest));
            continue;
        }
        if l.starts_with("/Encoding") {
            if !ends_def(l) {
                in_enc = true;
            }
            if reencoded || t1.builtin.is_none() {
                clear.push_str("/Encoding StandardEncoding def\n");
            } else {
                clear.push_str("/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\n");
                for (c, n) in used_codes {
                    if keep.contains(n) {
                        clear.push_str(&format!("dup {c} /{n} put\n"));
                    }
                }
                clear.push_str("readonly def\n");
            }
            continue;
        }
        clear.push_str(line);
    }
    // --- private part
    let mut out: Vec<u8> = Vec::new();
    let pre_text = String::from_utf8_lossy(&pv.pre).into_owned();
    // The 4 leading random bytes are kept as they are (no line to drop there).
    for line in pre_text.split_inclusive('\n') {
        if line.trim_start().starts_with("/UniqueID") {
            continue;
        }
        out.extend_from_slice(line.as_bytes());
    }
    if pv.has_subrs {
        let nsub = used_subrs.iter().next_back().map_or(0, |m| m + 1);
        out.extend_from_slice(format!("/Subrs {nsub} array\n").as_bytes());
        let ret = if pv.len_iv < 0 {
            vec![11]
        } else {
            let mut p = vec![0u8; pv.len_iv as usize];
            p.push(11);
            encrypt(&p, CS_R)
        };
        let by_idx: HashMap<usize, &Vec<u8>> = pv.subrs.iter().map(|(i, b)| (*i, b)).collect();
        for i in 0..nsub {
            let bytes = if used_subrs.contains(&i) { by_idx.get(&i).map_or(&ret, |b| *b) } else { &ret };
            out.extend_from_slice(format!("dup {} {} {} ", i, bytes.len(), pv.rd).as_bytes());
            out.extend_from_slice(bytes);
            out.extend_from_slice(format!(" {}\n", pv.np).as_bytes());
        }
        out.extend_from_slice(&pv.mid);
    }
    let kept: Vec<&(String, Vec<u8>)> = pv.chars.iter().filter(|(n, _)| keep.contains(n)).collect();
    out.extend_from_slice(format!("/CharStrings {} dict dup begin\n", kept.len()).as_bytes());
    for (n, b) in &kept {
        out.extend_from_slice(format!("/{} {} {} ", n, b.len(), pv.rd).as_bytes());
        out.extend_from_slice(b);
        out.extend_from_slice(format!(" {}\n", pv.nd).as_bytes());
    }
    out.extend_from_slice(&pv.post);
    let charset = keep.into_iter().filter(|n| n != ".notdef").collect();
    Some(T1Subset { clear: clear.into_bytes(), binary: encrypt(&out, EEXEC_R), charset })
}

// ---------------------------------------------------------------- VF

/// A VF font definition (`fnt_def`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VfFontDef {
    pub num: u32,
    pub checksum: u32,
    /// Scale relative to the virtual font's size (fix_word): size = `vf_scale(scale, vf size)`.
    pub scale: i32,
    pub design: i32,
    pub area: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VfChar {
    /// Width (fix_word, relative to the design size).
    pub tfm_width: i32,
    /// DVI commands; `fnt_num`s refer to `VfFontDef::num` (the first def is
    /// the font selected at the packet's start).
    pub dvi: Arc<[u8]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vf {
    pub checksum: u32,
    pub design_size: i32,
    pub fonts: Vec<VfFontDef>,
    pub chars: HashMap<u32, VfChar>,
}

fn be(b: &[u8], i: usize, n: usize) -> Option<u32> {
    let s = b.get(i..i + n)?;
    Some(s.iter().fold(0u32, |a, &x| a << 8 | u32::from(x)))
}

/// Parses a VF file into its font definitions and character packets.
pub fn vf_packets(data: &[u8]) -> Option<Vf> {
    if data.len() < 3 || data[0] != 247 || data[1] != 202 {
        return None;
    }
    let k = data[2] as usize;
    let mut i = 3 + k;
    let checksum = be(data, i, 4)?;
    let design_size = be(data, i + 4, 4)? as i32;
    i += 8;
    let mut fonts = Vec::new();
    let mut chars = HashMap::new();
    while i < data.len() {
        let op = data[i];
        match op {
            243..=246 => {
                let kl = (op - 242) as usize;
                let num = be(data, i + 1, kl)?;
                let j = i + 1 + kl;
                let checksum = be(data, j, 4)?;
                let scale = be(data, j + 4, 4)? as i32;
                let design = be(data, j + 8, 4)? as i32;
                let a = *data.get(j + 12)? as usize;
                let l = *data.get(j + 13)? as usize;
                let s = j + 14;
                let area = String::from_utf8_lossy(data.get(s..s + a)?).into_owned();
                let name = String::from_utf8_lossy(data.get(s + a..s + a + l)?).into_owned();
                fonts.push(VfFontDef { num, checksum, scale, design, area, name });
                i = s + a + l;
            }
            242 => {
                let pl = be(data, i + 1, 4)? as usize;
                let cc = be(data, i + 5, 4)?;
                let w = be(data, i + 9, 4)? as i32;
                let s = i + 13;
                let dvi: Arc<[u8]> = data.get(s..s + pl)?.into();
                chars.insert(cc, VfChar { tfm_width: w, dvi });
                i = s + pl;
            }
            0..=241 => {
                let pl = op as usize;
                let cc = u32::from(*data.get(i + 1)?);
                let w = be(data, i + 2, 3)? as i32;
                let s = i + 5;
                let dvi: Arc<[u8]> = data.get(s..s + pl)?.into();
                chars.insert(cc, VfChar { tfm_width: w, dvi });
                i = s + pl;
            }
            _ => break, // 248 post
        }
    }
    Some(Vf { checksum, design_size, fonts, chars })
}

/// A fix_word scaled by `size` sp exactly as TeX scales TFM dimensions
/// (tex.web 571–572).
pub fn vf_scale(fix: i32, size: i32) -> i32 {
    let mut z = size;
    let mut alpha: i32 = 16;
    while z >= 0o40000000 {
        z /= 2;
        alpha += alpha;
    }
    let beta = 256 / alpha;
    let alpha = alpha * z;
    let b = fix.to_be_bytes();
    let (a, b1, c, d) = (i32::from(b[0]), i32::from(b[1]), i32::from(b[2]), i32::from(b[3]));
    let sw = (((d * z) / 0o400 + c * z) / 0o400 + b1 * z) / beta;
    if a == 255 { sw - alpha } else { sw }
}

// ---------------------------------------------------------------- ToUnicode

/// Unicode for a glyph name as pdfTeX's `set_glyph_unicode`.
pub fn glyph_unicode(name: &str, tfm: &str, table: &GlyphUni) -> Option<Vec<u32>> {
    if let Some(v) = table.0.get(&format!("tfm:{tfm}/{name}")) {
        return Some(v.clone());
    }
    if let Some(v) = table.0.get(name) {
        return Some(v.clone());
    }
    if let Some(k) = name.find('.') {
        return if k > 0 { glyph_unicode(&name[..k], tfm, table) } else { None };
    }
    if name.contains('_') {
        let mut out = Vec::new();
        for part in name.split('_') {
            out.extend(glyph_unicode(part, tfm, table)?);
        }
        return Some(out);
    }
    let hex = |s: &str| s.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b));
    if let Some(r) = name.strip_prefix("uni") {
        if !r.is_empty() && r.len() % 4 == 0 && hex(r) {
            let mut out = Vec::new();
            for i in (0..r.len()).step_by(4) {
                let v = u32::from_str_radix(&r[i..i + 4], 16).ok()?;
                if (0xD800..0xE000).contains(&v) {
                    return None;
                }
                out.push(v);
            }
            return Some(out);
        }
    }
    if let Some(r) = name.strip_prefix('u') {
        if (4..=6).contains(&r.len()) && hex(r) {
            let v = u32::from_str_radix(r, 16).ok()?;
            if (0xD800..0xE000).contains(&v) || v > 0x10FFFF {
                return None;
            }
            return Some(vec![v]);
        }
    }
    None
}

fn utf16_hex(cps: &[u32]) -> String {
    let mut s = String::new();
    for &c in cps {
        let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
        let mut buf = [0u16; 2];
        for u in ch.encode_utf16(&mut buf).iter() {
            s.push_str(&format!("{u:04X}"));
        }
    }
    s
}

/// A ToUnicode CMap for 8-bit codes (ranges where codes and single code
/// points run together, the rest as bfchar; at most 100 per block).
pub fn tounicode_cmap(name: &str, map: &BTreeMap<u8, Vec<u32>>) -> String {
    let entries: Vec<(u8, &Vec<u32>)> = map.iter().map(|(c, v)| (*c, v)).collect();
    let mut ranges: Vec<(u8, u8, u32)> = Vec::new();
    let mut singles: Vec<(u8, &Vec<u32>)> = Vec::new();
    let mut k = 0;
    while k < entries.len() {
        let (c, v) = entries[k];
        if v.len() == 1 && v[0] <= 0xFFFF {
            let mut e = k;
            while e + 1 < entries.len() {
                let (c2, v2) = entries[e + 1];
                let off = u32::from(c2 - c);
                if c2 == entries[e].0 + 1 && v2.len() == 1 && v2[0] == v[0] + off && (v[0] & 0xFF) + off <= 0xFF {
                    e += 1;
                } else {
                    break;
                }
            }
            if e > k {
                ranges.push((c, entries[e].0, v[0]));
                k = e + 1;
                continue;
            }
        }
        singles.push((c, v));
        k += 1;
    }
    let mut s = String::new();
    s.push_str("%!PS-Adobe-3.0 Resource-CMap\n%%DocumentNeededResources: ProcSet (CIDInit)\n%%IncludeResource: ProcSet (CIDInit)\n");
    s.push_str(&format!("%%BeginResource: CMap (TeX-{name}-0)\n%%Title: (TeX-{name}-0 TeX {name} 0)\n%%Version: 1.000\n%%EndComments\n"));
    s.push_str("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo\n<< /Registry (TeX)\n");
    s.push_str(&format!("/Ordering ({name})\n/Supplement 0\n>> def\n/CMapName /TeX-{name}-0 def\n/CMapType 2 def\n"));
    s.push_str("1 begincodespacerange\n<00> <FF>\nendcodespacerange\n");
    for chunk in ranges.chunks(100) {
        s.push_str(&format!("{} beginbfrange\n", chunk.len()));
        for (a, b, u) in chunk {
            s.push_str(&format!("<{a:02X}> <{b:02X}> <{}>\n", utf16_hex(&[*u])));
        }
        s.push_str("endbfrange\n");
    }
    for chunk in singles.chunks(100) {
        s.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (c, v) in chunk {
            s.push_str(&format!("<{c:02X}> <{}>\n", utf16_hex(v)));
        }
        s.push_str("endbfchar\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n%%EndResource\n%%EOF\n");
    s
}

// ---------------------------------------------------------------- the set

/// Reads a support file (map, enc, pfb, ttf, otf, vf) by name (kpse).
pub type Loader = Arc<dyn Fn(&str) -> Option<Arc<[u8]>> + Send + Sync>;

/// What `write` needs from the document's env at the end of the build.
#[derive(Clone, Debug, Default)]
pub struct WriteOpts {
    /// `\pdfcompresslevel` (0: streams uncompressed).
    pub compress_level: i32,
    /// `\pdfgentounicode` > 0.
    pub gen_tounicode: bool,
    /// `\pdfomitcharset` > 0.
    pub omit_charset: bool,
    pub map_ops: Vec<MapOp>,
    pub glyph_uni: GlyphUni,
}

impl WriteOpts {
    /// The map ops and glyph table from the env values at
    /// `Key::Other(tag::PDF, MAP_OPS)` and `Key::Other(tag::PDF, GLYPH_UNI)`.
    pub fn from_vals(map_ops: &Val, glyph_uni: &Val) -> (Vec<MapOp>, GlyphUni) {
        let ops = match map_ops {
            Val::Data(d) => d.as_any().downcast_ref::<MapOps>().map(|m| m.0.clone()).unwrap_or_default(),
            _ => Vec::new(),
        };
        let g = match glyph_uni {
            Val::Data(d) => d.as_any().downcast_ref::<GlyphUni>().cloned().unwrap_or_default(),
            _ => GlyphUni::default(),
        };
        (ops, g)
    }
}

/// One PDF font dictionary: a TFM at one expansion.
#[derive(Debug)]
struct Inst {
    /// A font id with this TFM (for metrics).
    font: FontId,
    expand: i32,
    obj: u32,
    res: String,
    used: BTreeSet<u8>,
}

/// A cached embedded file stream.
#[derive(Clone, Debug)]
struct Built {
    glyphs: BTreeSet<String>,
    tag: String,
    charset: BTreeSet<String>,
    /// Stream dictionary entries (no `/Length`) and (compressed) data.
    dict: String,
    data: Arc<[u8]>,
}

/// The back end's font table (see the module doc).
pub struct FontSet {
    loader: Loader,
    insts: Vec<Inst>,
    by_key: HashMap<(Arc<str>, i32), usize>,
    by_font: HashMap<FontId, Arc<str>>,
    cache: HashMap<String, Built>,
    t1: HashMap<String, Option<Arc<Type1>>>,
    encs: HashMap<String, Option<Encoding>>,
    vfs: HashMap<String, Option<Arc<Vf>>>,
    warnings: Vec<String>,
    map: Option<(Vec<MapOp>, Arc<Map>)>,
}

impl std::fmt::Debug for FontSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontSet").field("insts", &self.insts).finish_non_exhaustive()
    }
}

fn tfm_of(f: FontId) -> Arc<crate::font::Font> {
    crate::font::fonts().font(f)
}

fn pdf_name(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b <= b' ' || b >= 127 || b"()<>[]{}/%#".contains(&b) {
            o.push_str(&format!("#{b:02X}"));
        } else {
            o.push(char::from(b));
        }
    }
    o
}

const STD14: [&str; 14] = [
    "Courier", "Courier-Bold", "Courier-Oblique", "Courier-BoldOblique", "Helvetica",
    "Helvetica-Bold", "Helvetica-Oblique", "Helvetica-BoldOblique", "Times-Roman", "Times-Bold",
    "Times-Italic", "Times-BoldItalic", "Symbol", "ZapfDingbats",
];

fn stream_data(data: &[u8], level: i32) -> (bool, Vec<u8>) {
    if level > 0 { (true, phitex_codecs::deflate::compress(data, level)) } else { (false, data.to_vec()) }
}

fn flate(z: bool) -> &'static str {
    if z { " /Filter /FlateDecode" } else { "" }
}

impl FontSet {
    pub fn new(loader: Loader) -> FontSet {
        FontSet {
            loader,
            insts: Vec::new(),
            by_key: HashMap::new(),
            by_font: HashMap::new(),
            cache: HashMap::new(),
            t1: HashMap::new(),
            encs: HashMap::new(),
            vfs: HashMap::new(),
            warnings: Vec::new(),
            map: None,
        }
    }

    /// Forgets glyph usage and object numbers (a new build); keeps parsed
    /// files and built subsets, so an unchanged glyph set is not subset again.
    pub fn clear_usage(&mut self) {
        self.insts.clear();
        self.by_key.clear();
        self.by_font.clear();
    }

    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    fn tfm_name(&mut self, font: FontId) -> Arc<str> {
        if let Some(n) = self.by_font.get(&font) {
            return n.clone();
        }
        let n: Arc<str> = tfm_of(font).name().into();
        self.by_font.insert(font, n.clone());
        n
    }

    fn inst(&mut self, w: &mut crate::pdf::Writer, font: FontId, expand: i32) -> usize {
        let name = self.tfm_name(font);
        if let Some(&i) = self.by_key.get(&(name.clone(), expand)) {
            return i;
        }
        let obj = w.alloc();
        let i = self.insts.len();
        let res = match expand {
            0 => format!("F{font}"),
            e if e < 0 => format!("F{font}n{}", -e),
            e => format!("F{font}p{e}"),
        };
        self.insts.push(Inst { font, expand, obj, res, used: BTreeSet::new() });
        self.by_key.insert((name, expand), i);
        i
    }

    /// The resource name and font dictionary object for `font` at expansion
    /// `expand` (thousandths; 0 normal). Fonts with the same TFM share one
    /// dictionary, as in pdfTeX. Allocates the object on first use.
    pub fn resource(&mut self, w: &mut crate::pdf::Writer, font: FontId, expand: i32) -> (String, u32) {
        let i = self.inst(w, font, expand);
        (self.insts[i].res.clone(), self.insts[i].obj)
    }

    /// Marks character `c` of `font` (at `expand`) as shown.
    pub fn use_glyph(&mut self, w: &mut crate::pdf::Writer, font: FontId, expand: i32, c: u16) {
        let i = self.inst(w, font, expand);
        if let Ok(c) = u8::try_from(c) {
            self.insts[i].used.insert(c);
        }
    }

    /// The map entry for a TFM (`SlantFont`/`ExtendFont` for the text matrix).
    pub fn map_entry(&mut self, ops: &[MapOp], tfm: &str) -> Option<MapEntry> {
        self.map_for(ops).entries.get(tfm).cloned()
    }

    fn map_for(&mut self, ops: &[MapOp]) -> Arc<Map> {
        if let Some((o, m)) = &self.map {
            if o.as_slice() == ops {
                return m.clone();
            }
        }
        let m = Arc::new(Map::build(ops, &self.loader, &mut self.warnings));
        self.map = Some((ops.to_vec(), m.clone()));
        m
    }

    /// The virtual font for a TFM name, if it has one (for E1: a character of
    /// such a font is drawn by interpreting its packet).
    pub fn vf(&mut self, tfm: &str) -> Option<Arc<Vf>> {
        if let Some(v) = self.vfs.get(tfm) {
            return v.clone();
        }
        let v = (self.loader)(&format!("{tfm}.vf")).and_then(|d| vf_packets(&d)).map(Arc::new);
        self.vfs.insert(tfm.to_string(), v.clone());
        v
    }

    fn enc(&mut self, name: &str) -> Option<Encoding> {
        if let Some(e) = self.encs.get(name) {
            return e.clone();
        }
        let e = (self.loader)(name).and_then(|d| parse_enc(&d));
        if e.is_none() {
            self.warnings.push(format!("pdfTeX warning: cannot open encoding file {name}"));
        }
        self.encs.insert(name.to_string(), e.clone());
        e
    }

    fn type1(&mut self, file: &str) -> Option<Arc<Type1>> {
        if let Some(t) = self.t1.get(file) {
            return t.clone();
        }
        let t = (self.loader)(file).and_then(|d| parse_type1(&d)).map(Arc::new);
        if t.is_none() {
            self.warnings.push(format!("pdfTeX warning: cannot open Type 1 font file {file}"));
        }
        self.t1.insert(file.to_string(), t.clone());
        t
    }

    /// Writes every font used: dictionaries, widths, descriptors, files,
    /// encodings, ToUnicode, in first-use order.
    pub fn write(&mut self, w: &mut crate::pdf::Writer, opts: &WriteOpts) {
        let map = self.map_for(&opts.map_ops);
        let mut taken: HashSet<String> = HashSet::new();
        // One descriptor and file per TFM; its glyph set is the union over
        // the expansions.
        let mut groups: Vec<(Arc<str>, Vec<usize>)> = Vec::new();
        for i in 0..self.insts.len() {
            let name = self.tfm_name(self.insts[i].font);
            match groups.iter_mut().find(|g| g.0 == name) {
                Some(g) => g.1.push(i),
                None => groups.push((name, vec![i])),
            }
        }
        for (tfm, idxs) in groups {
            let used: BTreeSet<u8> = idxs.iter().flat_map(|&i| self.insts[i].used.iter().copied()).collect();
            let entry = map.entries.get(&*tfm).cloned();
            self.write_group(w, opts, &tfm, &idxs, &used, entry.as_ref(), &mut taken);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_group(
        &mut self,
        w: &mut crate::pdf::Writer,
        opts: &WriteOpts,
        tfm: &str,
        idxs: &[usize],
        used: &BTreeSet<u8>,
        entry: Option<&MapEntry>,
        taken: &mut HashSet<String>,
    ) {
        let font = tfm_of(self.insts[idxs[0]].font);
        let size = font.size();
        let Some(entry) = entry else {
            // pdfTeX would make a Type 3 font from PK bitmaps.
            self.warnings.push(format!("pdfTeX warning: no mapping for font {tfm} (PK fonts not supported)"));
            for &i in idxs {
                let (first, last, widths) = self.widths(i, used, size);
                let wobj = w.alloc();
                w.object(wobj, widths.as_bytes());
                let body = format!(
                    "<< /Type /Font\n/Subtype /Type1\n/BaseFont /{}\n/FirstChar {first}\n/LastChar {last}\n/Widths {wobj} 0 R\n>>",
                    pdf_name(tfm)
                );
                w.object(self.insts[i].obj, body.as_bytes());
            }
            return;
        };
        let enc = match &entry.enc {
            Some(e) => self.enc(e),
            None => None,
        };
        let kind = entry.kind();
        let t1 = match (kind, entry.file.as_deref()) {
            (Some(FileKind::Type1), Some(f)) if entry.embed != Embed::None => self.type1(f),
            _ => None,
        };
        let mut ps_name = entry.ps_name.clone().unwrap_or_else(|| tfm.to_string());
        if let Some(t) = &t1 {
            if entry.ps_name.is_none() && !t.font_name.is_empty() {
                ps_name = t.font_name.clone();
            }
        }
        // Glyph names per code: the map's encoding, else the font's own.
        let names: [Option<String>; 256] = std::array::from_fn(|c| {
            if let Some(e) = &enc {
                return e.glyphs[c].clone();
            }
            match &t1 {
                Some(t) => match &t.builtin {
                    Some(b) => b[c].clone(),
                    None => standard_encoding(c as u8).map(str::to_string),
                },
                None => None,
            }
        });
        let used_codes: BTreeMap<u8, String> = used
            .iter()
            .filter_map(|&c| names[c as usize].clone().filter(|n| n != ".notdef").map(|n| (c, n)))
            .collect();
        let glyphs: BTreeSet<String> = used_codes.values().cloned().collect();

        // --- the descriptor's metrics (pdfTeX's preset_fontmetrics)
        let thou = |x: i32| divide_scaled(x, size, 3);
        let ch = |c: u8| u16::from(c);
        let mut cap_height = if font.has(ch(b'H')) { thou(font.height(ch(b'H'))) } else { 0 };
        let mut ascent = if font.has(ch(b'h')) { thou(font.height(ch(b'h'))) } else { 0 };
        let mut descent = if font.has(ch(b'y')) { -thou(font.depth(ch(b'y'))) } else { 0 };
        let mut stem_v = if font.has(ch(b'.')) { thou(font.width(ch(b'.'))) / 3 } else { 0 };
        let x_height = thou(font.param(5));
        let mut italic = 0;
        let mut bbox = [0i32; 4];
        let flags = entry.flags.unwrap_or(4);

        // --- the font file
        let mut file_ref: Option<(&'static str, u32)> = None;
        let mut base_name = ps_name.clone();
        let mut charset: Option<BTreeSet<String>> = None;
        let mut subtype = "Type1";
        match kind {
            Some(FileKind::Type1) => {
                if let Some(t) = &t1 {
                    bbox = t.bbox;
                    italic = t.italic_angle;
                    if let Some(v) = t.std_vw {
                        stem_v = v;
                    }
                    let file = entry.file.clone().unwrap_or_default();
                    let reenc = enc.is_some();
                    let subset = entry.embed == Embed::Subset;
                    let tag = if subset { subset_tag(&glyphs, &ps_name, taken) } else { String::new() };
                    let ckey = format!("t1\u{0}{file}\u{0}{ps_name}\u{0}{reenc}\u{0}{subset}\u{0}{}", opts.compress_level);
                    let want = if subset { glyphs.clone() } else { BTreeSet::new() };
                    let reuse = self.cache.get(&ckey).filter(|b| b.glyphs == want && b.tag == tag).cloned();
                    let built = match reuse {
                        Some(b) => Some(b),
                        None => {
                            let parts = if subset {
                                subset_type1(t, &glyphs, &tag, reenc, &used_codes).map(|s| (s.clear, s.binary, s.charset))
                            } else {
                                Some((t.clear.clone(), t.binary.clone(), BTreeSet::new()))
                            };
                            match parts {
                                Some((clear, binary, cs)) => {
                                    let mut raw = clear.clone();
                                    raw.extend_from_slice(&binary);
                                    let (z, data) = stream_data(&raw, opts.compress_level);
                                    let dict = format!("/Length1 {} /Length2 {} /Length3 0{}", clear.len(), binary.len(), flate(z));
                                    let b = Built { glyphs: want, tag: tag.clone(), charset: cs, dict, data: data.into() };
                                    self.cache.insert(ckey, b.clone());
                                    Some(b)
                                }
                                None => {
                                    self.warnings.push(format!("pdfTeX warning: invalid Type 1 font file {file}"));
                                    None
                                }
                            }
                        }
                    };
                    if let Some(b) = built {
                        let o = w.alloc();
                        w.stream(o, &b.dict, &b.data);
                        file_ref = Some(("FontFile", o));
                        if !b.tag.is_empty() {
                            base_name = format!("{}+{}", b.tag, ps_name);
                            charset = Some(b.charset);
                        }
                    }
                }
            }
            Some(k @ (FileKind::TrueType | FileKind::OpenType)) => {
                let file = entry.file.clone().unwrap_or_default();
                let face = (self.loader)(&file).and_then(|d| partex_otf::Face::new(d, 0));
                match face {
                    None => self.warnings.push(format!("pdfTeX warning: cannot open font file {file}")),
                    Some(face) => {
                        let upem = i32::from(face.units_per_em()).max(1);
                        let sc = |v: i32| t1_round(f64::from(v) * 1000.0 / f64::from(upem));
                        if let Some(h) = face.table(partex_otf::tag(b"head")) {
                            if h.len() >= 44 {
                                let g = |o: usize| i32::from(i16::from_be_bytes([h[o], h[o + 1]]));
                                bbox = [sc(g(36)), sc(g(38)), sc(g(40)), sc(g(42))];
                            }
                        }
                        ascent = sc(i32::from(face.ascender()));
                        descent = sc(i32::from(face.descender()));
                        if let Some(o) = face.os2() {
                            if o.cap_height != 0 {
                                cap_height = sc(i32::from(o.cap_height));
                            }
                        }
                        if let Some(a) = face.italic_angle() {
                            italic = t1_round(f64::from(a) / 65536.0);
                        }
                        let truetype = k == FileKind::TrueType || face.outlines() == partex_otf::face::Outlines::TrueType;
                        if entry.embed != Embed::None {
                            let ckey = format!("ot\u{0}{file}\u{0}{}", opts.compress_level);
                            let b = match self.cache.get(&ckey).cloned() {
                                Some(b) => b,
                                None => {
                                    let dict;
                                    let (z, d) = if truetype {
                                        let raw = face.data().to_vec();
                                        let (z, d) = stream_data(&raw, opts.compress_level);
                                        dict = format!("/Length1 {}{}", raw.len(), flate(z));
                                        (z, d)
                                    } else {
                                        let raw = face.table(partex_otf::tag(b"CFF ")).map_or_else(|| face.data().to_vec(), <[u8]>::to_vec);
                                        let (z, d) = stream_data(&raw, opts.compress_level);
                                        dict = format!("/Subtype /Type1C{}", flate(z));
                                        (z, d)
                                    };
                                    let _ = z;
                                    let b = Built { glyphs: BTreeSet::new(), tag: String::new(), charset: BTreeSet::new(), dict, data: d.into() };
                                    self.cache.insert(ckey, b.clone());
                                    b
                                }
                            };
                            let o = w.alloc();
                            w.stream(o, &b.dict, &b.data);
                            file_ref = Some((if truetype { "FontFile2" } else { "FontFile3" }, o));
                        }
                        if truetype {
                            subtype = "TrueType";
                        }
                    }
                }
            }
            None => {}
        }
        if bbox == [0; 4] {
            bbox = [0, descent, 0, ascent];
        } else if t1.is_some() {
            // pdfTeX's fix_fontmetrics: ascent/descent from the font's bbox
            // when the font file gives one.
            ascent = bbox[3];
            descent = bbox[1];
        }
        let std14 = file_ref.is_none() && STD14.contains(&ps_name.as_str());
        let desc = if std14 {
            None
        } else {
            let o = w.alloc();
            let mut d = format!(
                "<< /Type /FontDescriptor\n/FontName /{}\n/Flags {flags}\n/FontBBox [{} {} {} {}]\n/Ascent {ascent}\n/CapHeight {cap_height}\n/Descent {descent}\n/ItalicAngle {italic}\n/StemV {stem_v}\n/XHeight {x_height}\n",
                pdf_name(&base_name),
                bbox[0],
                bbox[1],
                bbox[2],
                bbox[3]
            );
            if let Some(cs) = &charset {
                if !opts.omit_charset {
                    d.push_str("/CharSet (");
                    for g in cs {
                        d.push('/');
                        d.push_str(g);
                    }
                    d.push_str(")\n");
                }
            }
            if let Some((k, f)) = file_ref {
                d.push_str(&format!("/{k} {f} 0 R\n"));
            }
            d.push_str(">>");
            w.object(o, d.as_bytes());
            Some(o)
        };
        // --- /Encoding: /Differences for the used codes
        let enc_obj = match &enc {
            Some(e) if !used_codes.is_empty() => {
                let o = w.alloc();
                let mut s = String::from("<< /Type /Encoding\n/Differences [");
                let mut prev: Option<u8> = None;
                for &c in used_codes.keys() {
                    if prev.is_none_or(|p| u16::from(p) + 1 != u16::from(c)) {
                        s.push_str(&format!(" {c}"));
                    }
                    s.push('/');
                    s.push_str(&pdf_name(e.glyphs[c as usize].as_deref().unwrap_or(".notdef")));
                    prev = Some(c);
                }
                s.push_str("]\n>>");
                w.object(o, s.as_bytes());
                Some(o)
            }
            _ => None,
        };
        // --- /ToUnicode
        let mut tu_obj = None;
        if opts.gen_tounicode && !used_codes.is_empty() {
            let mut m = BTreeMap::new();
            for (&c, n) in &used_codes {
                if let Some(u) = glyph_unicode(n, tfm, &opts.glyph_uni) {
                    m.insert(c, u);
                }
            }
            if !m.is_empty() {
                let cname = match &enc {
                    Some(e) if !e.name.is_empty() => e.name.clone(),
                    _ => format!("{ps_name}-builtin"),
                };
                let cmap = tounicode_cmap(&cname, &m);
                let (z, d) = stream_data(cmap.as_bytes(), opts.compress_level);
                let o = w.alloc();
                w.stream(o, flate(z).trim_start(), &d);
                tu_obj = Some(o);
            }
        }
        // --- one dictionary per expansion instance
        for &i in idxs {
            let (first, last, widths) = self.widths(i, used, size);
            let wobj = w.alloc();
            w.object(wobj, widths.as_bytes());
            let mut d = format!(
                "<< /Type /Font\n/Subtype /{subtype}\n/BaseFont /{}\n/FirstChar {first}\n/LastChar {last}\n/Widths {wobj} 0 R\n",
                pdf_name(&base_name)
            );
            if let Some(o) = desc {
                d.push_str(&format!("/FontDescriptor {o} 0 R\n"));
            }
            if let Some(o) = enc_obj {
                d.push_str(&format!("/Encoding {o} 0 R\n"));
            }
            if let Some(o) = tu_obj {
                d.push_str(&format!("/ToUnicode {o} 0 R\n"));
            }
            d.push_str(">>");
            w.object(self.insts[i].obj, d.as_bytes());
        }
    }

    /// `/Widths` (pdfTeX's `write_charwidth_array`): in 1/1000 em with one
    /// decimal, 8 per line; codes not shown are 0.
    fn widths(&self, i: usize, used: &BTreeSet<u8>, size: i32) -> (u8, u8, String) {
        let inst = &self.insts[i];
        let font = tfm_of(inst.font);
        let first = used.iter().next().copied().unwrap_or(0);
        let last = used.iter().next_back().copied().unwrap_or(0);
        let mut s = String::from("[");
        for c in first..=last {
            let v = if used.contains(&c) && font.has(u16::from(c)) {
                let mut wd = font.width(u16::from(c));
                if inst.expand != 0 {
                    wd = round_xn_over_d(wd, 1000 + inst.expand, 1000);
                }
                divide_scaled(wd, size, 4)
            } else {
                0
            };
            let a = v.abs();
            if v < 0 {
                s.push('-');
            }
            s.push_str(&(a / 10).to_string());
            if a % 10 != 0 {
                s.push_str(&format!(".{}", a % 10));
            }
            if c < last {
                s.push(if (c - first) % 8 == 7 { '\n' } else { ' ' });
            }
        }
        s.push(']');
        (first, last, s)
    }
}
