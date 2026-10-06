//! Type 1 fonts as PDFs embed them (`/FontFile`: the cleartext part, then
//! the eexec-encrypted binary part): each glyph's outline as SVG path data,
//! in the font's units (y up), so a page is drawn in its own fonts, as
//! pdf.js does, not in look-alikes. The charstring interpreter covers what
//! TeX's fonts use: the path operators, `hsbw`/`sbw`, subroutines, flex and
//! hint replacement (the other subroutines 0–3), `seac` and `div`.

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

use std::collections::HashMap;
use std::fmt::Write as _;

/// An embedded Type 1 font: its built-in encoding and its glyphs' programs.
pub struct Type1 {
    /// Code → glyph name, as the font program's /Encoding says.
    pub encoding: Vec<Option<String>>,
    charstrings: HashMap<String, Vec<u8>>,
    subrs: Vec<Vec<u8>>,
}

fn decrypt(b: &[u8], mut r: u16, skip: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    for &c in b {
        out.push(c ^ (r >> 8) as u8);
        r = (u16::from(c).wrapping_add(r))
            .wrapping_mul(52845)
            .wrapping_add(22719);
    }
    out.split_off(skip.min(out.len()))
}

/// The next whitespace-delimited token at `i` (advancing `i`).
fn token<'a>(b: &'a [u8], i: &mut usize) -> &'a [u8] {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
    let s = *i;
    while *i < b.len() && !b[*i].is_ascii_whitespace() {
        *i += 1;
    }
    &b[s..*i]
}

fn num(t: &[u8]) -> Option<usize> {
    std::str::from_utf8(t).ok()?.parse().ok()
}

fn find(b: &[u8], pat: &[u8], from: usize) -> Option<usize> {
    b.get(from..)?
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|p| p + from)
}

impl Type1 {
    /// Parse a `/FontFile` stream whose cleartext part is `len1` bytes.
    #[must_use]
    pub fn parse(file: &[u8], len1: usize) -> Option<Type1> {
        let (clear, enc) = file.split_at(len1.min(file.len()));
        let mut encoding = vec![None; 256];
        if find(clear, b"/Encoding StandardEncoding", 0).is_some() {
            for (c, n) in STANDARD.iter().enumerate() {
                if !n.is_empty() {
                    encoding[c] = Some((*n).to_string());
                }
            }
        } else {
            // (`dup <code> /<name> put`)
            let mut i = 0;
            while let Some(p) = find(clear, b"dup ", i) {
                i = p + 4;
                let mut j = i;
                let code = token(clear, &mut j);
                let name = token(clear, &mut j);
                if let (Some(c), Some(n)) = (num(code), name.strip_prefix(b"/"))
                    && c < 256
                {
                    encoding[c] = Some(String::from_utf8_lossy(n).into_owned());
                }
            }
        }
        // (the binary part; a hex one, rare in PDFs, is not read)
        let p = decrypt(enc, 55665, 4);
        let len_iv = find(&p, b"/lenIV", 0).and_then(|i| {
            let mut j = i + 6;
            num(token(&p, &mut j))
        });
        let skip = len_iv.unwrap_or(4);
        // (`<len> RD <bytes>`, RD spelled `RD` or `-|`)
        let read = |i: &mut usize| -> Option<Vec<u8>> {
            let n = num(token(&p, i))?;
            let rd = token(&p, i);
            if rd != b"RD" && rd != b"-|" {
                return None;
            }
            *i += 1;
            let b = p.get(*i..*i + n)?;
            *i += n;
            Some(decrypt(b, 4330, skip))
        };
        let mut subrs = Vec::new();
        if let Some(s) = find(&p, b"/Subrs", 0) {
            let mut i = s + 6;
            let n = num(token(&p, &mut i)).unwrap_or(0);
            token(&p, &mut i); // (array)
            subrs = vec![Vec::new(); n];
            for _ in 0..n {
                let mut j = i;
                if token(&p, &mut j) != b"dup" {
                    break;
                }
                let Some(k) = num(token(&p, &mut j)) else {
                    break;
                };
                let Some(cs) = read(&mut j) else { break };
                if k < subrs.len() {
                    subrs[k] = cs;
                }
                token(&p, &mut j); // (NP, or `noaccess put`)
                let mut k2 = j;
                if token(&p, &mut k2) == b"put" {
                    j = k2;
                }
                i = j;
            }
        }
        let mut charstrings = HashMap::new();
        let c = find(&p, b"/CharStrings", 0)?;
        let mut i = find(&p, b"begin", c)? + 5;
        loop {
            let mut j = i;
            let t = token(&p, &mut j);
            let Some(name) = t.strip_prefix(b"/") else {
                break;
            };
            let name = String::from_utf8_lossy(name).into_owned();
            let Some(cs) = read(&mut j) else { break };
            charstrings.insert(name, cs);
            token(&p, &mut j); // (ND)
            i = j;
        }
        Some(Type1 {
            encoding,
            charstrings,
            subrs,
        })
    }

    /// Glyph `name`'s outline as SVG path data (font units, y up), if the font has it.
    #[must_use]
    pub fn path(&self, name: &str) -> Option<String> {
        let cs = self.charstrings.get(name)?;
        let mut r = Run {
            t1: self,
            d: String::new(),
            x: 0.0,
            y: 0.0,
            flex: None,
            ps: Vec::new(),
            open: false,
            depth: 0,
            w: 0.0,
        };
        r.run(cs, &mut Vec::new(), 0.0, 0.0);
        if r.open {
            r.d.push('Z');
        }
        Some(r.d)
    }

    /// Glyph `name`'s advance width (font units, `hsbw`'s), if the font has it.
    #[must_use]
    pub fn width(&self, name: &str) -> Option<f64> {
        let cs = self.charstrings.get(name)?;
        let mut r = Run {
            t1: self,
            d: String::new(),
            x: 0.0,
            y: 0.0,
            flex: None,
            ps: Vec::new(),
            open: false,
            depth: 0,
            w: 0.0,
        };
        r.run(cs, &mut Vec::new(), 0.0, 0.0);
        Some(r.w)
    }

    /// A `.pfb` file (segments `0x80 1` cleartext, `0x80 2` binary, …), or a
    /// font program as the PDF embeds it (cleartext up to `eexec`, then binary).
    #[must_use]
    pub fn from_file(b: &[u8]) -> Option<Type1> {
        if b.first() != Some(&0x80) {
            let e = find(b, b"eexec", 0)? + 5;
            let e = e + b[e..]
                .iter()
                .take_while(|c| matches!(c, b'\r' | b'\n' | b' '))
                .count();
            return Type1::parse(b, e);
        }
        let (mut clear, mut bin, mut i) = (Vec::new(), Vec::new(), 0);
        while i + 6 <= b.len() && b[i] == 0x80 && b[i + 1] != 3 {
            let n = u32::from_le_bytes([b[i + 2], b[i + 3], b[i + 4], b[i + 5]]) as usize;
            let seg = b.get(i + 6..i + 6 + n)?;
            if b[i + 1] == 1 && bin.is_empty() {
                clear.extend_from_slice(seg);
            } else if b[i + 1] == 2 {
                bin.extend_from_slice(seg);
            }
            i += 6 + n;
        }
        let len1 = clear.len();
        clear.extend_from_slice(&bin);
        Type1::parse(&clear, len1)
    }
}

struct Run<'a> {
    t1: &'a Type1,
    d: String,
    x: f64,
    y: f64,
    /// Points of a flex being collected (othersubr 1 … 0).
    flex: Option<Vec<(f64, f64)>>,
    /// What callothersubr leaves for `pop`.
    ps: Vec<f64>,
    open: bool,
    depth: u32,
    /// The advance width `hsbw` gave.
    w: f64,
}

impl Run<'_> {
    fn moveto(&mut self, x: f64, y: f64) {
        if let Some(f) = &mut self.flex {
            f.push((x, y));
            self.x = x;
            self.y = y;
            return;
        }
        if self.open {
            self.d.push('Z');
        }
        let _ = write!(self.d, "M{} {}", r1(x), r1(y));
        self.x = x;
        self.y = y;
        self.open = true;
    }
    fn lineto(&mut self, x: f64, y: f64) {
        let _ = write!(self.d, "L{} {}", r1(x), r1(y));
        self.x = x;
        self.y = y;
    }
    fn curveto(&mut self, a: (f64, f64), b: (f64, f64), c: (f64, f64)) {
        let _ = write!(
            self.d,
            "C{} {} {} {} {} {}",
            r1(a.0),
            r1(a.1),
            r1(b.0),
            r1(b.1),
            r1(c.0),
            r1(c.1)
        );
        self.x = c.0;
        self.y = c.1;
    }

    /// Interpret `cs` with the operand stack `st`; `(ox, oy)`: where the
    /// glyph's origin is (an accent's, in seac). Returns true at endchar.
    fn run(&mut self, cs: &[u8], st: &mut Vec<f64>, ox: f64, oy: f64) -> bool {
        self.depth += 1;
        if self.depth > 20 {
            return true;
        }
        let mut i = 0;
        while i < cs.len() {
            let v = cs[i];
            i += 1;
            match v {
                32..=246 => st.push(f64::from(v) - 139.0),
                247..=250 => {
                    let w = f64::from(*cs.get(i).unwrap_or(&0));
                    i += 1;
                    st.push((f64::from(v) - 247.0) * 256.0 + w + 108.0);
                }
                251..=254 => {
                    let w = f64::from(*cs.get(i).unwrap_or(&0));
                    i += 1;
                    st.push(-(f64::from(v) - 251.0) * 256.0 - w - 108.0);
                }
                255 => {
                    let b = cs.get(i..i + 4).unwrap_or(&[0, 0, 0, 0]);
                    i += 4;
                    st.push(f64::from(i32::from_be_bytes([b[0], b[1], b[2], b[3]])));
                }
                13 => {
                    // hsbw: sbx wx
                    let sbx = st.first().copied().unwrap_or(0.0);
                    self.w = st.get(1).copied().unwrap_or(0.0);
                    self.x = ox + sbx;
                    self.y = oy;
                    st.clear();
                }
                21 => {
                    let (dx, dy) = (arg(st, 0), arg(st, 1));
                    self.moveto(self.x + dx, self.y + dy);
                    st.clear();
                }
                22 => {
                    let dx = arg(st, 0);
                    self.moveto(self.x + dx, self.y);
                    st.clear();
                }
                4 => {
                    let dy = arg(st, 0);
                    self.moveto(self.x, self.y + dy);
                    st.clear();
                }
                5 => {
                    let (dx, dy) = (arg(st, 0), arg(st, 1));
                    self.lineto(self.x + dx, self.y + dy);
                    st.clear();
                }
                6 => {
                    let dx = arg(st, 0);
                    self.lineto(self.x + dx, self.y);
                    st.clear();
                }
                7 => {
                    let dy = arg(st, 0);
                    self.lineto(self.x, self.y + dy);
                    st.clear();
                }
                8 => {
                    let a = (self.x + arg(st, 0), self.y + arg(st, 1));
                    let b = (a.0 + arg(st, 2), a.1 + arg(st, 3));
                    let c = (b.0 + arg(st, 4), b.1 + arg(st, 5));
                    self.curveto(a, b, c);
                    st.clear();
                }
                30 => {
                    // vhcurveto: dy1 dx2 dy2 dx3
                    let a = (self.x, self.y + arg(st, 0));
                    let b = (a.0 + arg(st, 1), a.1 + arg(st, 2));
                    let c = (b.0 + arg(st, 3), b.1);
                    self.curveto(a, b, c);
                    st.clear();
                }
                31 => {
                    // hvcurveto: dx1 dx2 dy2 dy3
                    let a = (self.x + arg(st, 0), self.y);
                    let b = (a.0 + arg(st, 1), a.1 + arg(st, 2));
                    let c = (b.0, b.1 + arg(st, 3));
                    self.curveto(a, b, c);
                    st.clear();
                }
                9 => {
                    if self.open {
                        self.d.push('Z');
                        self.open = false;
                    }
                    st.clear();
                }
                10 => {
                    let n = st.pop().unwrap_or(-1.0);
                    if n >= 0.0
                        && let Some(s) = self.t1.subrs.get(n as usize)
                        && self.run(s, st, ox, oy)
                    {
                        self.depth -= 1;
                        return true;
                    }
                }
                11 => {
                    self.depth -= 1;
                    return false;
                }
                14 => {
                    self.depth -= 1;
                    return true;
                }
                12 => {
                    let e = *cs.get(i).unwrap_or(&0);
                    i += 1;
                    match e {
                        6 => {
                            // seac: asb adx ady bchar achar
                            let (asb, adx, ady) = (arg(st, 0), arg(st, 1), arg(st, 2));
                            let (b, a) = (arg(st, 3) as usize, arg(st, 4) as usize);
                            st.clear();
                            let names = (
                                STANDARD.get(b).copied().unwrap_or(""),
                                STANDARD.get(a).copied().unwrap_or(""),
                            );
                            if let Some(bc) = self.t1.charstrings.get(names.0) {
                                self.run(bc, &mut Vec::new(), ox, oy);
                                if self.open {
                                    self.d.push('Z');
                                    self.open = false;
                                }
                            }
                            if let Some(ac) = self.t1.charstrings.get(names.1) {
                                self.run(ac, &mut Vec::new(), ox + adx - asb, oy + ady);
                            }
                            self.depth -= 1;
                            return true;
                        }
                        7 => {
                            // sbw: sbx sby wx wy
                            self.x = ox + arg(st, 0);
                            self.y = oy + arg(st, 1);
                            st.clear();
                        }
                        12 => {
                            let b = st.pop().unwrap_or(1.0);
                            let a = st.pop().unwrap_or(0.0);
                            st.push(if b == 0.0 { 0.0 } else { a / b });
                        }
                        16 => {
                            // callothersubr: args… n othersubr#
                            let o = st.pop().unwrap_or(0.0) as i32;
                            let n = st.pop().unwrap_or(0.0).max(0.0) as usize;
                            let args: Vec<f64> = st.split_off(st.len().saturating_sub(n));
                            match o {
                                1 => self.flex = Some(Vec::new()),
                                0 => {
                                    let pts = self.flex.take().unwrap_or_default();
                                    if pts.len() >= 7 {
                                        self.curveto(pts[1], pts[2], pts[3]);
                                        self.curveto(pts[4], pts[5], pts[6]);
                                    }
                                    // (pop pop setcurrentpoint: the end point)
                                    self.ps = vec![self.y, self.x];
                                }
                                2 => {}
                                // (3, hint replacement, and the rest: `pop`
                                // gives the arguments back, first first)
                                _ => self.ps = args.into_iter().rev().collect(),
                            }
                        }
                        17 => st.push(self.ps.pop().unwrap_or(0.0)),
                        33 => {
                            self.x = arg(st, 0);
                            self.y = arg(st, 1);
                            st.clear();
                        }
                        _ => st.clear(), // (dotsection, vstem3, hstem3)
                    }
                }
                _ => st.clear(),
            }
        }
        self.depth -= 1;
        false
    }
}

fn arg(st: &[f64], k: usize) -> f64 {
    st.get(k).copied().unwrap_or(0.0)
}

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Adobe's `StandardEncoding` (seac's codes, and fonts that use it).
const STANDARD: [&str; 256] = [
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "space",
    "exclam",
    "quotedbl",
    "numbersign",
    "dollar",
    "percent",
    "ampersand",
    "quoteright",
    "parenleft",
    "parenright",
    "asterisk",
    "plus",
    "comma",
    "hyphen",
    "period",
    "slash",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "colon",
    "semicolon",
    "less",
    "equal",
    "greater",
    "question",
    "at",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "bracketleft",
    "backslash",
    "bracketright",
    "asciicircum",
    "underscore",
    "quoteleft",
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "braceleft",
    "bar",
    "braceright",
    "asciitilde",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "exclamdown",
    "cent",
    "sterling",
    "fraction",
    "yen",
    "florin",
    "section",
    "currency",
    "quotesingle",
    "quotedblleft",
    "guillemotleft",
    "guilsinglleft",
    "guilsinglright",
    "fi",
    "fl",
    "",
    "endash",
    "dagger",
    "daggerdbl",
    "periodcentered",
    "",
    "paragraph",
    "bullet",
    "quotesinglbase",
    "quotedblbase",
    "quotedblright",
    "guillemotright",
    "ellipsis",
    "perthousand",
    "",
    "questiondown",
    "",
    "grave",
    "acute",
    "circumflex",
    "tilde",
    "macron",
    "breve",
    "dotaccent",
    "dieresis",
    "",
    "ring",
    "cedilla",
    "",
    "hungarumlaut",
    "ogonek",
    "caron",
    "emdash",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "AE",
    "",
    "ordfeminine",
    "",
    "",
    "",
    "",
    "Lslash",
    "Oslash",
    "OE",
    "ordmasculine",
    "",
    "",
    "",
    "",
    "",
    "ae",
    "",
    "",
    "",
    "dotlessi",
    "",
    "",
    "lslash",
    "oslash",
    "oe",
    "germandbls",
    "",
    "",
    "",
    "",
];
