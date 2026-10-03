//! The character codes a PDF content stream shows, in order, and where
//! (glyph origins' order, DESIGN 4.4): every code a text-showing operator
//! (`Tj`, `TJ`, `'`, `"`) shows, a form `XObject`'s at each `Do` that
//! draws it, again at each use, recursively. Numbers in a `TJ` array are
//! kerns.
//!
//! A code is one byte in a simple font (Type 1, TrueType, Type 3). In a
//! Type 0 font its bytes are its `CMap`'s: two with `Identity-H` or `-V`
//! (what pdfTeX's inputs carry); an embedded `CMap`'s codespace ranges
//! split a string as PDF 9.7.6.2 says (the shortest range a code's first
//! bytes fall in; one byte if none does); any other predefined `CMap`,
//! whose ranges are not in the file, is counted two bytes a code.
//!
//! A code's place is its glyph's origin in the page's default user space
//! (points, from the media box's origin): the text matrix and the current
//! transformation matrix as PDF 9.4.4 makes them, each glyph advanced by
//! its width (`/Widths`, a Type 3 font's through its `/FontMatrix`; `/W`
//! and `/DW` of a Type 0 font's descendant, its codes taken for CIDs),
//! the character and word spacing and the horizontal scaling. The state
//! is the graphics state's: `q`/`Q` save and restore it, and a form
//! starts with the state in force where it is drawn, its `/Matrix`
//! applied.

use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::pdfread::{Dict, Doc, Obj, Page};

/// How a font's strings split into codes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Codes {
    /// So many bytes a code.
    Fixed(usize),
    /// A `CMap`'s codespace ranges: each range's low and high bytes (the
    /// same length).
    Ranges(Vec<(Vec<u8>, Vec<u8>)>),
}

impl Default for Codes {
    fn default() -> Codes {
        Codes::Fixed(1)
    }
}

impl Codes {
    /// The length of the code `s` starts with (at least 1).
    #[must_use]
    pub fn next_len(&self, s: &[u8]) -> usize {
        match self {
            Codes::Fixed(n) => (*n).max(1),
            Codes::Ranges(r) => (1..=4.min(s.len()))
                .find(|&n| {
                    r.iter().any(|(lo, hi)| {
                        lo.len() == n && (0..n).all(|k| lo[k] <= s[k] && s[k] <= hi[k])
                    })
                })
                .unwrap_or(1),
        }
    }
}

/// A font's glyph widths, in text space units per unit of font size.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Widths {
    /// None known: every glyph 0 wide.
    #[default]
    Zero,
    /// A simple font's: `/FirstChar`, its `/Widths` scaled (by 1/1000,
    /// or a Type 3 font's `/FontMatrix`), and the width of the others.
    Simple {
        first: u32,
        widths: Vec<f64>,
        missing: f64,
    },
    /// A CID font's `/W` ranges (first, last, width) and `/DW`, scaled.
    Cid {
        ranges: Vec<(u32, u32, f64)>,
        default: f64,
    },
}

impl Widths {
    /// The width of code `code`.
    #[must_use]
    pub fn of(&self, code: u32) -> f64 {
        match self {
            Widths::Zero => 0.0,
            Widths::Simple {
                first,
                widths,
                missing,
            } => code
                .checked_sub(*first)
                .and_then(|i| widths.get(i as usize))
                .copied()
                .unwrap_or(*missing),
            Widths::Cid { ranges, default } => ranges
                .iter()
                .find(|(a, b, _)| *a <= code && code <= *b)
                .map_or(*default, |r| r.2),
        }
    }
}

/// The text state's font: its resource name, how it splits strings and
/// its widths.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Font {
    pub name: Vec<u8>,
    pub codes: Codes,
    pub widths: Arc<Widths>,
}

/// A transformation matrix `[a b c d e f]`.
pub type Matrix = [f64; 6];

/// The identity.
pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `m` then `n` (PDF's `m × n`).
#[must_use]
pub fn concat(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

/// The graphics state's parts that place text.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    /// The current transformation matrix.
    pub ctm: Matrix,
    pub font: Font,
    /// `Tf`'s size, `Tc`, `Tw`, `Tz` (as a fraction), `TL`, `Ts`.
    pub size: f64,
    pub char_space: f64,
    pub word_space: f64,
    pub scale: f64,
    pub leading: f64,
    pub rise: f64,
}

impl Default for State {
    fn default() -> State {
        State {
            ctm: IDENTITY,
            font: Font::default(),
            size: 0.0,
            char_space: 0.0,
            word_space: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
        }
    }
}

/// What a walk of a content stream asks of its resources.
pub trait Text {
    /// Font resource `name`: how it splits strings into codes, and its
    /// widths.
    fn font(&mut self, name: &[u8]) -> Font;
    /// A code shown, in font resource `font`, its glyph's origin at
    /// `(x, y)`.
    fn code(&mut self, font: &[u8], code: u32, x: f64, y: f64);
    /// `XObject` `name` drawn (`Do`), with state `state` in force.
    fn draw(&mut self, name: &[u8], state: &State);
}

/// An operand.
#[derive(Clone, Debug, PartialEq)]
enum Operand {
    Num(f64),
    Str(Vec<u8>),
    Name(Vec<u8>),
}

/// Walk content stream `s`, telling `t` each code shown and each `Do`.
pub fn walk(s: &[u8], t: &mut dyn Text) {
    walk_from(s, t, State::default());
}

/// [`walk`], with state `state` in force at the start (a form's).
#[allow(clippy::too_many_lines)]
pub fn walk_from(s: &[u8], t: &mut dyn Text, state: State) {
    let mut g = state;
    let mut saved: Vec<State> = Vec::new();
    // (the text matrix and the text line matrix)
    let (mut tm, mut tlm) = (IDENTITY, IDENTITY);
    let mut ops: Vec<Operand> = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        match c {
            b'%' => {
                while i < s.len() && s[i] != b'\n' && s[i] != b'\r' {
                    i += 1;
                }
            }
            b'(' => {
                let (bytes, next) = literal_string(s, i + 1);
                ops.push(Operand::Str(bytes));
                i = next;
                continue;
            }
            b'<' if s.get(i + 1) == Some(&b'<') => {
                i += 2;
                continue;
            }
            b'<' => {
                let (bytes, next) = hex_string(s, i + 1);
                ops.push(Operand::Str(bytes));
                i = next;
                continue;
            }
            b'/' => {
                let from = i + 1;
                i += 1;
                while i < s.len() && !is_delim(s[i]) && !is_white(s[i]) {
                    i += 1;
                }
                ops.push(Operand::Name(s[from..i].to_vec()));
                continue;
            }
            b'[' | b']' | b'>' | b'{' | b'}' | b')' => {}
            _ if is_white(c) => {}
            _ => {
                let from = i;
                while i < s.len() && !is_delim(s[i]) && !is_white(s[i]) {
                    i += 1;
                }
                let tok = &s[from..i];
                if let Some(v) = number(tok) {
                    ops.push(Operand::Num(v));
                    continue;
                }
                let num = |k: usize| -> f64 {
                    // (the k-th of the operator's last numbers)
                    let nums: Vec<f64> = ops
                        .iter()
                        .filter_map(|o| match o {
                            Operand::Num(v) => Some(*v),
                            _ => None,
                        })
                        .collect();
                    nums.len()
                        .checked_sub(k)
                        .and_then(|j| nums.get(j))
                        .copied()
                        .unwrap_or(0.0)
                };
                match tok {
                    b"q" => saved.push(g.clone()),
                    b"Q" => {
                        if let Some(old) = saved.pop() {
                            g = old;
                        }
                    }
                    b"cm" => {
                        let m = [num(6), num(5), num(4), num(3), num(2), num(1)];
                        g.ctm = concat(&m, &g.ctm);
                    }
                    b"BT" => {
                        tm = IDENTITY;
                        tlm = IDENTITY;
                    }
                    b"Tf" => {
                        let name = ops.iter().rev().find_map(|o| match o {
                            Operand::Name(n) => Some(n.clone()),
                            _ => None,
                        });
                        g.font = t.font(&name.unwrap_or_default());
                        g.size = num(1);
                    }
                    b"Tc" => g.char_space = num(1),
                    b"Tw" => g.word_space = num(1),
                    b"Tz" => g.scale = num(1) / 100.0,
                    b"TL" => g.leading = num(1),
                    b"Ts" => g.rise = num(1),
                    b"Td" | b"TD" => {
                        let (x, y) = (num(2), num(1));
                        if tok == b"TD" {
                            g.leading = -y;
                        }
                        tlm = concat(&[1.0, 0.0, 0.0, 1.0, x, y], &tlm);
                        tm = tlm;
                    }
                    b"Tm" => {
                        tlm = [num(6), num(5), num(4), num(3), num(2), num(1)];
                        tm = tlm;
                    }
                    b"T*" => {
                        tlm = concat(&[1.0, 0.0, 0.0, 1.0, 0.0, -g.leading], &tlm);
                        tm = tlm;
                    }
                    b"Tj" | b"TJ" | b"'" | b"\"" => {
                        if tok == b"\"" {
                            g.word_space = num(2);
                            g.char_space = num(1);
                        }
                        if tok == b"'" || tok == b"\"" {
                            tlm = concat(&[1.0, 0.0, 0.0, 1.0, 0.0, -g.leading], &tlm);
                            tm = tlm;
                        }
                        let array = tok == b"TJ";
                        for o in &ops {
                            match o {
                                Operand::Str(st) => show(st, &g, &mut tm, t),
                                Operand::Num(n) if array => {
                                    let tx = -n / 1000.0 * g.size * g.scale;
                                    tm = concat(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], &tm);
                                }
                                _ => {}
                            }
                        }
                    }
                    b"Do" => {
                        if let Some(Operand::Name(n)) = ops.last() {
                            let n = n.clone();
                            t.draw(&n, &g);
                        }
                    }
                    b"BI" => {
                        // (an inline image: its data up to `EI`)
                        i = skip_inline_image(s, i);
                    }
                    _ => {}
                }
                ops.clear();
                continue;
            }
        }
        i += 1;
    }
}

/// Show string `st` in state `g`, the text matrix `tm` advanced.
fn show(st: &[u8], g: &State, tm: &mut Matrix, t: &mut dyn Text) {
    let mut k = 0;
    while k < st.len() {
        let n = g.font.codes.next_len(&st[k..]);
        let Some(bytes) = st.get(k..k + n) else {
            break;
        };
        let code = bytes.iter().fold(0u32, |a, &b| a << 8 | u32::from(b));
        // (the glyph's origin: (0, rise) in text space)
        let m = concat(tm, &g.ctm);
        let (x, y) = (g.rise * m[2] + m[4], g.rise * m[3] + m[5]);
        t.code(&g.font.name, code, x, y);
        let space = if n == 1 && code == 32 {
            g.word_space
        } else {
            0.0
        };
        let tx = (g.font.widths.of(code) * g.size + g.char_space + space) * g.scale;
        *tm = concat(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], tm);
        k += n;
    }
}

/// A number token's value (PDF's integers and reals).
fn number(tok: &[u8]) -> Option<f64> {
    let ok = !tok.is_empty()
        && tok
            .iter()
            .all(|&b| b.is_ascii_digit() || b == b'.' || b == b'-' || b == b'+')
        && tok.iter().any(u8::is_ascii_digit);
    if !ok {
        return None;
    }
    let (neg, digits) = match tok[0] {
        b'-' => (true, &tok[1..]),
        b'+' => (false, &tok[1..]),
        _ => (false, tok),
    };
    let mut v = 0.0f64;
    let mut frac = 0.0f64;
    let mut scale = 1.0f64;
    let mut dot = false;
    for &b in digits {
        match b {
            b'.' if !dot => dot = true,
            b'0'..=b'9' if dot => {
                scale /= 10.0;
                frac += f64::from(b - b'0') * scale;
            }
            b'0'..=b'9' => v = v * 10.0 + f64::from(b - b'0'),
            // (`--1`, `1.2.3`: as 0, as readers take them)
            _ => return Some(0.0),
        }
    }
    let v = v + frac;
    Some(if neg { -v } else { v })
}

fn is_white(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0')
}

fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Past an inline image's data (after `BI`): after its `EI`.
fn skip_inline_image(s: &[u8], mut i: usize) -> usize {
    while i + 2 < s.len() {
        if is_white(s[i]) && s[i + 1] == b'E' && s[i + 2] == b'I' {
            let after = s.get(i + 3).copied();
            if after.is_none_or(|c| is_white(c) || is_delim(c)) {
                return i + 3;
            }
        }
        i += 1;
    }
    s.len()
}

/// A hex string from `s[i..]` (after its `<`): its bytes and where it
/// ends (an odd last digit as if followed by 0).
fn hex_string(s: &[u8], mut i: usize) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut high: Option<u8> = None;
    while i < s.len() && s[i] != b'>' {
        let d = match s[i] {
            d @ b'0'..=b'9' => Some(d - b'0'),
            d @ b'a'..=b'f' => Some(d - b'a' + 10),
            d @ b'A'..=b'F' => Some(d - b'A' + 10),
            _ => None,
        };
        if let Some(d) = d {
            match high.take() {
                Some(h) => out.push(h << 4 | d),
                None => high = Some(d),
            }
        }
        i += 1;
    }
    if let Some(h) = high {
        out.push(h << 4);
    }
    (out, (i + 1).min(s.len()))
}

/// A literal string from `s[i..]` (after its `(`): its bytes and where
/// it ends.
fn literal_string(s: &[u8], mut i: usize) -> (Vec<u8>, usize) {
    let mut depth = 1;
    let mut out = Vec::new();
    while i < s.len() {
        match s[i] {
            b'\\' => {
                i += 1;
                match s.get(i) {
                    Some(b'0'..=b'7') => {
                        let mut v = 0u32;
                        let mut k = 0;
                        while k < 3 && matches!(s.get(i), Some(b'0'..=b'7')) {
                            v = v * 8 + u32::from(s[i] - b'0');
                            i += 1;
                            k += 1;
                        }
                        out.push(u8::try_from(v & 0xff).unwrap_or(0));
                        continue;
                    }
                    Some(b'\r') => {
                        if s.get(i + 1) == Some(&b'\n') {
                            i += 1;
                        }
                    }
                    Some(b'\n') | None => {}
                    Some(&e) => out.push(match e {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'b' => 8,
                        b'f' => 12,
                        e => e,
                    }),
                }
            }
            b'(' => {
                depth += 1;
                out.push(b'(');
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return (out, i + 1);
                }
                out.push(b')');
            }
            c => out.push(c),
        }
        i += 1;
    }
    (out, i)
}

/// A stream's codes counted with simple fonts only (a literal's text: the
/// fonts it can name are pdfTeX's, simple).
#[must_use]
pub fn simple_codes(s: &[u8]) -> usize {
    struct Count(usize);
    impl Text for Count {
        fn font(&mut self, _: &[u8]) -> Font {
            Font::default()
        }
        fn code(&mut self, _: &[u8], _: u32, _: f64, _: f64) {
            self.0 += 1;
        }
        fn draw(&mut self, _: &[u8], _: &State) {}
    }
    let mut c = Count(0);
    walk(s, &mut c);
    c.0
}

/// An embedded `CMap`'s codespace ranges (`begincodespacerange` …
/// `endcodespacerange`).
#[must_use]
pub fn codespace_ranges(cmap: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0;
    let key: &[u8] = b"begincodespacerange";
    while let Some(at) = cmap[i..].windows(key.len()).position(|w| w == key) {
        i += at + key.len();
        let mut pair: Vec<Vec<u8>> = Vec::new();
        while i < cmap.len() {
            match cmap[i] {
                b'<' => {
                    let (bytes, next) = hex_string(cmap, i + 1);
                    pair.push(bytes);
                    i = next;
                    if pair.len() == 2 {
                        let hi = pair.pop().unwrap_or_default();
                        let lo = pair.pop().unwrap_or_default();
                        if lo.len() == hi.len() && !lo.is_empty() {
                            out.push((lo, hi));
                        }
                    }
                    continue;
                }
                b'e' => break,
                _ => {}
            }
            i += 1;
        }
    }
    out
}

/// A shown code: its font's resource name, the code, and its glyph's
/// origin.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub font: Vec<u8>,
    pub code: u32,
    pub x: f64,
    pub y: f64,
}

/// The walk of a PDF file's content stream, with its resources: each
/// code shown goes to `out`, forms are walked where they are drawn.
pub struct DocText<'a> {
    pub doc: &'a Doc,
    pub resources: Option<Dict>,
    pub depth: usize,
    pub out: &'a mut Vec<Shown>,
}

impl DocText<'_> {
    fn resource(&self, kind: &[u8], name: &[u8]) -> Obj {
        let Some(r) = &self.resources else {
            return Obj::Null;
        };
        let d = self.doc.lookup(r, kind);
        match d.as_dict() {
            Some(d) => self.doc.lookup(d, name),
            None => Obj::Null,
        }
    }

    /// Walk stream `s` (decoded) with these resources.
    pub fn walk(&mut self, s: &[u8]) {
        walk(s, self);
    }

    /// Numbers of array `o` (followed).
    fn nums(&self, o: &Obj) -> Vec<f64> {
        match self.doc.follow(o) {
            Obj::Array(a) => a
                .iter()
                .map(|x| self.doc.follow(x).as_num().unwrap_or(0.0))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A simple font's widths (`d` its dictionary; `scale` 1/1000, or a
    /// Type 3 font's `/FontMatrix`'s).
    fn simple_widths(&self, d: &Dict, scale: f64) -> Widths {
        let first = self.doc.lookup(d, b"FirstChar").as_int().unwrap_or(0);
        let widths: Vec<f64> = self
            .nums(&self.doc.lookup(d, b"Widths"))
            .into_iter()
            .map(|w| w * scale)
            .collect();
        let missing = match self.doc.lookup(d, b"FontDescriptor") {
            Obj::Dict(fd) => self
                .doc
                .lookup(&fd, b"MissingWidth")
                .as_num()
                .unwrap_or(0.0),
            _ => 0.0,
        } * scale;
        Widths::Simple {
            first: u32::try_from(first).unwrap_or(0),
            widths,
            missing,
        }
    }

    /// A Type 0 font's widths: its descendant's `/W` and `/DW`.
    fn cid_widths(&self, d: &Dict) -> Widths {
        let desc = match self.doc.lookup(d, b"DescendantFonts") {
            Obj::Array(a) => a.first().map(|o| self.doc.follow(o)),
            _ => None,
        };
        let Some(Obj::Dict(cd)) = desc else {
            return Widths::Zero;
        };
        let default = self.doc.lookup(&cd, b"DW").as_num().unwrap_or(1000.0) / 1000.0;
        let mut ranges = Vec::new();
        if let Obj::Array(w) = self.doc.lookup(&cd, b"W") {
            let mut k = 0;
            while k < w.len() {
                let first = self.doc.follow(&w[k]).as_int().unwrap_or(0);
                let first = u32::try_from(first).unwrap_or(0);
                match w.get(k + 1).map(|o| self.doc.follow(o)) {
                    Some(Obj::Array(list)) => {
                        for (j, x) in list.iter().enumerate() {
                            let c = first + u32::try_from(j).unwrap_or(0);
                            let v = self.doc.follow(x).as_num().unwrap_or(0.0) / 1000.0;
                            ranges.push((c, c, v));
                        }
                        k += 2;
                    }
                    Some(last) => {
                        let last = u32::try_from(last.as_int().unwrap_or(0)).unwrap_or(0);
                        let v = w
                            .get(k + 2)
                            .map_or(0.0, |x| self.doc.follow(x).as_num().unwrap_or(0.0))
                            / 1000.0;
                        ranges.push((first, last, v));
                        k += 3;
                    }
                    None => break,
                }
            }
        }
        Widths::Cid { ranges, default }
    }
}

impl Text for DocText<'_> {
    fn font(&mut self, name: &[u8]) -> Font {
        let f = self.resource(b"Font", name);
        let mut font = Font {
            name: name.to_vec(),
            ..Font::default()
        };
        let Some(d) = f.as_dict() else {
            return font;
        };
        match self.doc.lookup(d, b"Subtype").as_name() {
            Some(b"Type0") => {
                font.codes = match self.doc.lookup(d, b"Encoding") {
                    Obj::Stream(s) => {
                        let r = codespace_ranges(&self.doc.decode(&s));
                        if r.is_empty() {
                            Codes::Fixed(2)
                        } else {
                            Codes::Ranges(r)
                        }
                    }
                    _ => Codes::Fixed(2),
                };
                font.widths = Arc::new(self.cid_widths(d));
            }
            Some(b"Type3") => {
                let m = self.nums(&self.doc.lookup(d, b"FontMatrix"));
                let scale = m.first().copied().unwrap_or(0.001);
                font.widths = Arc::new(self.simple_widths(d, scale));
            }
            _ => font.widths = Arc::new(self.simple_widths(d, 0.001)),
        }
        font
    }

    fn code(&mut self, font: &[u8], code: u32, x: f64, y: f64) {
        self.out.push(Shown {
            font: font.to_vec(),
            code,
            x,
            y,
        });
    }

    fn draw(&mut self, name: &[u8], state: &State) {
        if self.depth > 32 {
            return;
        }
        let Obj::Stream(s) = self.resource(b"XObject", name) else {
            return;
        };
        if self.doc.lookup(&s.dict, b"Subtype").as_name() != Some(b"Form") {
            return;
        }
        let data = self.doc.decode(&s);
        let resources = match self.doc.lookup(&s.dict, b"Resources") {
            Obj::Dict(d) => Some(d),
            _ => self.resources.clone(),
        };
        let mut st = state.clone();
        let m = self.nums(&self.doc.lookup(&s.dict, b"Matrix"));
        if let [a, b, c, d, e, f] = m[..] {
            st.ctm = concat(&[a, b, c, d, e, f], &st.ctm);
        }
        let mut inner = DocText {
            doc: self.doc,
            resources,
            depth: self.depth + 1,
            out: &mut *self.out,
        };
        walk_from(&data, &mut inner, st);
    }
}

/// A page's content (its `/Contents`, a stream or an array of them,
/// decoded and joined as one stream).
#[must_use]
pub fn page_content(doc: &Doc, page: &Page) -> Vec<u8> {
    let Obj::Dict(d) = doc.fetch(page.dict_ref) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut add = |o: &Obj| {
        if let Obj::Stream(s) = doc.follow(o) {
            out.extend_from_slice(&doc.decode(&s));
            out.push(b'\n');
        }
    };
    match doc.lookup(&d, b"Contents") {
        Obj::Array(a) => a.iter().for_each(&mut add),
        o @ Obj::Stream(_) => add(&o),
        _ => {}
    }
    out
}

/// The codes page `page` shows, in order, each with its font's resource
/// name and its glyph's origin.
#[must_use]
pub fn page_codes(doc: &Doc, page: &Page) -> Vec<Shown> {
    let content = page_content(doc, page);
    let mut codes = Vec::new();
    let mut t = DocText {
        doc,
        resources: page.resources.clone(),
        depth: 0,
        out: &mut codes,
    };
    t.walk(&content);
    codes
}

/// How many codes page `page` (1-based) of PDF file `data` shows (0 if
/// it cannot be read).
#[must_use]
pub fn page_code_count(data: &Arc<[u8]>, page: usize) -> usize {
    let Ok(doc) = Doc::open(data) else {
        return 0;
    };
    doc.page(page).map_or(0, |p| page_codes(&doc, &p).len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_text_codes() {
        assert_eq!(simple_codes(b"BT /F1 10 Tf (abc) Tj ET"), 3);
        assert_eq!(simple_codes(b"[(a) -100 (b\\(c)] TJ"), 4);
        assert_eq!(simple_codes(b"<41 42> Tj (x) ' 1 2 (yz) \""), 5);
        assert_eq!(simple_codes(b"q 1 0 0 1 0 0 cm Q % (no) Tj\n"), 0);
        assert_eq!(simple_codes(b"(\\101\\102) Tj"), 2);
        assert_eq!(simple_codes(b"BI /W 1 /H 1 ID x( EI (ab) Tj"), 2);
        assert_eq!(simple_codes(b"/P <</ActualText (xyz)>> BDC (a) Tj EMC"), 1);
    }

    /// Fonts by name: `C0` two bytes a code, `C1` a codespace of one and
    /// two bytes, others one byte, each glyph 0.5 wide.
    struct Codes2(Vec<(u32, f64, f64)>);
    impl Text for Codes2 {
        fn font(&mut self, n: &[u8]) -> Font {
            Font {
                name: n.to_vec(),
                codes: match n {
                    b"C0" => Codes::Fixed(2),
                    b"C1" => Codes::Ranges(codespace_ranges(
                        b"begincodespacerange <00> <80> <8140> <9ffc> endcodespacerange",
                    )),
                    _ => Codes::Fixed(1),
                },
                widths: Arc::new(Widths::Simple {
                    first: 0,
                    widths: Vec::new(),
                    missing: 0.5,
                }),
            }
        }
        fn code(&mut self, _: &[u8], c: u32, x: f64, y: f64) {
            self.0.push((c, x, y));
        }
        fn draw(&mut self, _: &[u8], _: &State) {}
    }

    fn codes(s: &[u8]) -> Vec<u32> {
        let mut t = Codes2(Vec::new());
        walk(s, &mut t);
        t.0.into_iter().map(|c| c.0).collect()
    }

    #[test]
    fn multi_byte_codes() {
        assert_eq!(
            codes(b"/C0 9 Tf <00410042> Tj /F1 9 Tf (ab) Tj"),
            [0x41, 0x42, 0x61, 0x62]
        );
        // (the font restored by Q)
        assert_eq!(
            codes(b"/C0 9 Tf q /F1 9 Tf (ab) Tj Q <0041> Tj"),
            [0x61, 0x62, 0x41]
        );
        // (a CMap's codespace: one byte or two)
        assert_eq!(
            codes(b"/C1 9 Tf <41 8145 42 ff> Tj"),
            [0x41, 0x8145, 0x42, 0xff]
        );
    }

    #[test]
    fn places_glyphs() {
        let mut t = Codes2(Vec::new());
        // (10pt: glyphs 5 wide; a kern of -200: 2 more; a line down;
        // the CTM moved and scaled)
        walk(
            b"1 0 0 1 100 200 cm BT /F1 10 Tf 3 4 Td [(ab)-200(c)]TJ 0 -12 Td (d) Tj ET \
              q 2 0 0 2 0 0 cm BT /F1 10 Tf (e) Tj ET Q",
            &mut t,
        );
        let at: Vec<(f64, f64)> = t.0.iter().map(|c| (c.1, c.2)).collect();
        assert_eq!(
            at,
            [
                (103.0, 204.0),
                (108.0, 204.0),
                (115.0, 204.0),
                (103.0, 192.0),
                (100.0, 200.0)
            ]
        );
    }
}
