//! An included PDF file, read as xpdf reads it for pdfTeX's
//! `pdftoepdf.cc`: its objects as xpdf's lexer and parser make them
//! (integers and reals told apart by the token, strings and names
//! decoded), its cross-reference sections (tables and streams, the
//! `/Prev` chain, `/XRefStm`, object streams; a damaged file's objects
//! found by scanning it), and its pages with the attributes they inherit
//! (`PageAttrs`: the boxes, the rotation, the resources).
//!
//! What pdfTeX copies is a page's dictionary entries and the objects
//! they refer to, each byte of a string or a stream as it is in the
//! file: the writer (`partex-core`'s `pdf/epdf.rs`) prints these values.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::inflate::{Predictor, inflate, unpredict};

/// An object's number and generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ref {
    pub num: i32,
    pub generation: i32,
}

/// A dictionary: its entries in the file's order. A key given twice
/// keeps both entries (copied both), and a lookup finds the last one, as
/// xpdf's hash chains do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dict(pub Vec<(Vec<u8>, Obj)>);

impl Dict {
    /// The value of `key`, not followed if it is a reference (xpdf's
    /// `lookupNF`).
    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<&Obj> {
        self.0.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

/// A stream: its dictionary and where its bytes are in the file, as they
/// are there (encoded).
#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    pub dict: Dict,
    pub start: usize,
    pub len: usize,
}

/// An object, as xpdf's `Object` holds it.
#[derive(Clone, Debug, PartialEq)]
pub enum Obj {
    Null,
    Bool(bool),
    Int(i32),
    Real(f64),
    Str(Vec<u8>),
    Name(Vec<u8>),
    Array(Vec<Obj>),
    Dict(Dict),
    Stream(Box<Stream>),
    Ref(Ref),
}

impl Obj {
    #[must_use]
    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Obj::Dict(d) => Some(d),
            Obj::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_int(&self) -> Option<i32> {
        match self {
            Obj::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// An integer or a real, as a real (xpdf's `isNum`, `getNum`).
    #[must_use]
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Obj::Int(i) => Some(f64::from(*i)),
            Obj::Real(r) => Some(*r),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }

    /// xpdf's type names, for pdfTeX's messages.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Obj::Null => "null",
            Obj::Bool(_) => "boolean",
            Obj::Int(_) => "integer",
            Obj::Real(_) => "real",
            Obj::Str(_) => "string",
            Obj::Name(_) => "name",
            Obj::Array(_) => "array",
            Obj::Dict(_) => "dictionary",
            Obj::Stream(_) => "stream",
            Obj::Ref(_) => "ref",
        }
    }
}

/// A token of xpdf's lexer: an object, a command (a keyword, or one of
/// `[ ] << >> { }`), the end, or an error.
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Obj(Obj),
    Cmd(Vec<u8>),
    Eof,
    Error,
}

/// xpdf's `specialChars`: 1 whitespace, 2 a delimiter.
fn special(c: u8) -> u8 {
    match c {
        0 | b'\t' | b'\n' | 0x0c | b'\r' | b' ' => 1,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%' => 2,
        _ => 0,
    }
}

/// xpdf's `Lexer`, over the file's bytes from `pos`.
struct Lexer<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Lexer<'_> {
    fn look(&self) -> Option<u8> {
        self.d.get(self.pos).copied()
    }

    fn get(&mut self) -> Option<u8> {
        let c = self.look()?;
        self.pos += 1;
        Some(c)
    }

    /// `Lexer::skipToNextLine`: past the end of the line.
    fn skip_to_next_line(&mut self) {
        while let Some(c) = self.get() {
            if c == b'\n' {
                return;
            }
            if c == b'\r' {
                if self.look() == Some(b'\n') {
                    self.pos += 1;
                }
                return;
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn token(&mut self) -> Tok {
        // (whitespace and comments)
        let c = loop {
            let Some(c) = self.get() else {
                return Tok::Eof;
            };
            if c == b'%' {
                while let Some(c) = self.look() {
                    if c == b'\r' || c == b'\n' {
                        break;
                    }
                    self.pos += 1;
                }
            } else if special(c) != 1 {
                break c;
            }
        };
        match c {
            b'0'..=b'9' | b'+' | b'-' | b'.' => self.number(c),
            b'(' => self.literal(),
            b'/' => {
                let mut name = Vec::new();
                while let Some(c) = self.look() {
                    if special(c) != 0 {
                        break;
                    }
                    self.pos += 1;
                    if c == b'#' {
                        let h = |x: u8| char::from(x).to_digit(16);
                        if let (Some(a), Some(b)) = (
                            self.look().and_then(h),
                            self.d.get(self.pos + 1).copied().and_then(h),
                        ) {
                            self.pos += 2;
                            name.push(u8::try_from(a << 4 | b).unwrap_or(0));
                            continue;
                        }
                    }
                    name.push(c);
                }
                Tok::Obj(Obj::Name(name))
            }
            b'[' | b']' | b'{' | b'}' => Tok::Cmd(alloc::vec![c]),
            b'<' => {
                if self.look() == Some(b'<') {
                    self.pos += 1;
                    return Tok::Cmd(b"<<".to_vec());
                }
                // (a hex string: whitespace skipped, an odd digit padded)
                let mut s = Vec::new();
                let mut m: Option<u32> = None;
                while let Some(c) = self.get() {
                    if c == b'>' {
                        break;
                    }
                    if let Some(x) = char::from(c).to_digit(16) {
                        match m.take() {
                            Some(h) => s.push(u8::try_from(h << 4 | x).unwrap_or(0)),
                            None => m = Some(x),
                        }
                    }
                }
                if let Some(h) = m {
                    s.push(u8::try_from(h << 4).unwrap_or(0));
                }
                Tok::Obj(Obj::Str(s))
            }
            b'>' => {
                if self.look() == Some(b'>') {
                    self.pos += 1;
                    Tok::Cmd(b">>".to_vec())
                } else {
                    Tok::Error
                }
            }
            b')' => Tok::Error,
            _ => {
                let mut k = alloc::vec![c];
                while let Some(c) = self.look() {
                    if special(c) != 0 {
                        break;
                    }
                    self.pos += 1;
                    k.push(c);
                }
                match &k[..] {
                    b"true" => Tok::Obj(Obj::Bool(true)),
                    b"false" => Tok::Obj(Obj::Bool(false)),
                    b"null" => Tok::Obj(Obj::Null),
                    _ => Tok::Cmd(k),
                }
            }
        }
    }

    /// xpdf 4's number lexer, with Adobe's "interesting" cases ("--1",
    /// "50-100").
    #[allow(clippy::cast_possible_truncation)]
    fn number(&mut self, c: u8) -> Tok {
        let mut neg = false;
        let (mut xi, mut xf): (i64, f64) = (0, 0.0);
        let mut real = false;
        match c {
            b'+' => {}
            b'-' => {
                neg = true;
                // (a second minus ignored: "--1.5" is -1.5)
                if self.look() == Some(b'-') {
                    self.pos += 1;
                }
            }
            b'.' => real = true,
            _ => {
                xi = i64::from(c - b'0');
                xf = f64::from(c - b'0');
            }
        }
        if !real {
            while let Some(c) = self.look() {
                if c.is_ascii_digit() {
                    self.pos += 1;
                    xi = xi.saturating_mul(10).saturating_add(i64::from(c - b'0'));
                    if xf < 1e20 {
                        xf = xf * 10.0 + f64::from(c - b'0');
                    }
                } else if c == b'.' {
                    self.pos += 1;
                    real = true;
                    break;
                } else {
                    break;
                }
            }
        }
        if real {
            let mut scale = 0.1;
            while let Some(c) = self.look() {
                if c == b'-' {
                    self.pos += 1;
                    continue;
                }
                if !c.is_ascii_digit() {
                    break;
                }
                self.pos += 1;
                xf += scale * f64::from(c - b'0');
                scale *= 0.1;
            }
        }
        while let Some(c) = self.look() {
            if c == b'-' || c.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        if neg {
            xi = -xi;
            xf = -xf;
        }
        if real || !(-2_147_483_648.0..=2_147_483_647.0).contains(&xf) {
            Tok::Obj(Obj::Real(xf))
        } else {
            Tok::Obj(Obj::Int(xi as i32))
        }
    }

    /// A literal string: its escapes, and each end of line inside it read
    /// as a line feed.
    fn literal(&mut self) -> Tok {
        let mut s = Vec::new();
        let mut depth = 1;
        loop {
            let Some(c) = self.get() else {
                return Tok::Obj(Obj::Str(s));
            };
            match c {
                b'(' => {
                    depth += 1;
                    s.push(c);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Tok::Obj(Obj::Str(s));
                    }
                    s.push(c);
                }
                b'\r' => {
                    if self.look() == Some(b'\n') {
                        self.pos += 1;
                    }
                    s.push(b'\n');
                }
                b'\\' => match self.get() {
                    Some(b'n') => s.push(b'\n'),
                    Some(b'r') => s.push(b'\r'),
                    Some(b't') => s.push(b'\t'),
                    Some(b'b') => s.push(8),
                    Some(b'f') => s.push(12),
                    Some(c @ b'0'..=b'7') => {
                        let mut v = u32::from(c - b'0');
                        for _ in 0..2 {
                            match self.look() {
                                Some(c @ b'0'..=b'7') => {
                                    self.pos += 1;
                                    v = v * 8 + u32::from(c - b'0');
                                }
                                _ => break,
                            }
                        }
                        s.push(u8::try_from(v & 0xff).unwrap_or(0));
                    }
                    Some(b'\r') => {
                        if self.look() == Some(b'\n') {
                            self.pos += 1;
                        }
                    }
                    Some(b'\n') => {}
                    Some(c) => s.push(c),
                    None => return Tok::Obj(Obj::Str(s)),
                },
                _ => s.push(c),
            }
        }
    }
}

/// How deep arrays and dictionaries nest at most (xpdf's
/// `recursionLimit`).
const DEPTH: u32 = 500;

/// xpdf's `Parser`: objects from the lexer, with its two tokens of
/// lookahead (`num generation R` is a reference).
struct Parser<'a, 'd> {
    lex: Lexer<'a>,
    buf1: Tok,
    buf2: Tok,
    /// The document, for an indirect `/Length` (none while the
    /// cross-reference sections are read).
    doc: Option<&'d Doc>,
}

impl<'a, 'd> Parser<'a, 'd> {
    fn new(d: &'a [u8], pos: usize, doc: Option<&'d Doc>) -> Self {
        let mut lex = Lexer { d, pos };
        let buf1 = lex.token();
        let buf2 = lex.token();
        Parser {
            lex,
            buf1,
            buf2,
            doc,
        }
    }

    fn shift(&mut self) {
        let t = self.lex.token();
        self.buf1 = core::mem::replace(&mut self.buf2, t);
    }

    fn is_cmd(t: &Tok, c: &[u8]) -> bool {
        matches!(t, Tok::Cmd(k) if k == c)
    }

    /// The next object; a stream only if `streams` (not in an object
    /// stream).
    fn obj(&mut self, streams: bool, depth: u32) -> Obj {
        if depth > DEPTH {
            self.shift();
            return Obj::Null;
        }
        if Self::is_cmd(&self.buf1, b"[") {
            self.shift();
            let mut a = Vec::new();
            while !Self::is_cmd(&self.buf1, b"]") && self.buf1 != Tok::Eof {
                if matches!(self.buf1, Tok::Error | Tok::Cmd(_))
                    && !Self::is_cmd(&self.buf1, b"[")
                    && !Self::is_cmd(&self.buf1, b"<<")
                {
                    // (xpdf keeps a command as an object of its own in an
                    // array; nothing of pdfTeX's copies such a one)
                    self.shift();
                    continue;
                }
                a.push(self.obj(false, depth + 1));
            }
            self.shift();
            return Obj::Array(a);
        }
        if Self::is_cmd(&self.buf1, b"<<") {
            self.shift();
            let mut d = Dict::default();
            while !Self::is_cmd(&self.buf1, b">>") && self.buf1 != Tok::Eof {
                let Tok::Obj(Obj::Name(key)) = core::mem::replace(&mut self.buf1, Tok::Eof) else {
                    // (a key must be a name: the token is dropped)
                    self.shift();
                    continue;
                };
                self.shift();
                if matches!(self.buf1, Tok::Eof | Tok::Error) {
                    break;
                }
                let v = self.obj(false, depth + 1);
                d.0.push((key, v));
            }
            if streams && Self::is_cmd(&self.buf2, b"stream") {
                return match self.stream(d) {
                    Some(s) => Obj::Stream(Box::new(s)),
                    None => Obj::Null,
                };
            }
            self.shift();
            return Obj::Dict(d);
        }
        if let Tok::Obj(Obj::Int(num)) = self.buf1 {
            self.shift();
            if let Tok::Obj(Obj::Int(generation)) = self.buf1
                && Self::is_cmd(&self.buf2, b"R")
            {
                self.shift();
                self.shift();
                return Obj::Ref(Ref { num, generation });
            }
            return Obj::Int(num);
        }
        let t = core::mem::replace(&mut self.buf1, Tok::Eof);
        self.shift();
        match t {
            Tok::Obj(o) => o,
            _ => Obj::Null,
        }
    }

    /// `Parser::makeStream`: the bytes after the line of `stream`, as long
    /// as `/Length` says (a damaged one: to the next `endstream`).
    fn stream(&mut self, dict: Dict) -> Option<Stream> {
        self.lex.skip_to_next_line();
        let start = self.lex.pos;
        let length = match dict.get(b"Length") {
            Some(Obj::Int(n)) => Some(*n),
            Some(Obj::Ref(r)) => self.doc.and_then(|d| d.fetch(*r).as_int()),
            _ => None,
        }
        .and_then(|n| usize::try_from(n).ok());
        let d = self.lex.d;
        let ends_there = |n: usize| {
            let mut l = Lexer {
                d,
                pos: start.checked_add(n)?,
            };
            (l.pos <= d.len() && Self::is_cmd(&l.token(), b"endstream")).then_some(n)
        };
        let len = length
            .and_then(ends_there)
            .or_else(|| find(&d[start.min(d.len())..], b"endstream").map(|k| trim_eol(d, start, k)))
            .or(length)?;
        let len = len.min(d.len().saturating_sub(start));
        self.lex.pos = start + len;
        // (past `>>` and `stream`, then `endstream`)
        self.shift();
        self.shift();
        if Self::is_cmd(&self.buf1, b"endstream") {
            self.shift();
        }
        Some(Stream { dict, start, len })
    }
}

/// The stream's length when it is found by its `endstream`: up to the end
/// of line before it.
fn trim_eol(d: &[u8], start: usize, mut k: usize) -> usize {
    if k > 0 && d.get(start + k - 1) == Some(&b'\n') {
        k -= 1;
        if k > 0 && d.get(start + k - 1) == Some(&b'\r') {
            k -= 1;
        }
    } else if k > 0 && d.get(start + k - 1) == Some(&b'\r') {
        k -= 1;
    }
    k
}

/// Where `pat` first is in `d`.
fn find(d: &[u8], pat: &[u8]) -> Option<usize> {
    d.windows(pat.len()).position(|w| w == pat)
}

/// An entry of the cross-reference sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Free,
    At { offset: usize, generation: i32 },
    InStream { stream: i32, index: i32 },
}

/// The page boxes and what a page inherits (xpdf's `PageAttrs`).
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    /// The page's dictionary, by reference.
    pub dict_ref: Ref,
    pub media: [f64; 4],
    pub crop: [f64; 4],
    pub bleed: [f64; 4],
    pub trim: [f64; 4],
    pub art: [f64; 4],
    /// `/Rotate`, in 0..360.
    pub rotate: i32,
    /// The resources (inherited), followed.
    pub resources: Option<Dict>,
}

/// An object stream's objects, with their numbers.
type ObjStm = Arc<Vec<(i32, Obj)>>;

/// What reading a cross-reference section found: the section before it
/// (`/Prev`), or none.
enum Section {
    Prev(usize),
    First,
}

/// An open PDF file.
pub struct Doc {
    /// The file's bytes from its `%PDF-` header (xpdf moves its start
    /// there).
    data: Arc<[u8]>,
    base: usize,
    xref: Vec<Option<Entry>>,
    pub trailer: Dict,
    /// The header's version (`getPDFVersion`).
    pub version: f64,
    /// Object streams read, by number: their objects and numbers.
    objstms: RefCell<BTreeMap<i32, ObjStm>>,
    pages: Vec<Page>,
}

/// Why a file is not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No catalog or page tree was found.
    Broken,
    /// It is encrypted (not read yet).
    Encrypted,
}

impl Doc {
    /// The bytes of the file, from its header.
    fn bytes(&self) -> &[u8] {
        &self.data[self.base..]
    }

    /// Open `data` (xpdf's `PDFDoc`): its header, its cross-reference
    /// sections (or, damaged, the objects found by scanning it), its
    /// catalog and its pages.
    pub fn open(data: Arc<[u8]>) -> Result<Doc, Error> {
        let head = &data[..data.len().min(1024)];
        let base = find(head, b"%PDF-").unwrap_or(0);
        let version = header_version(&data[base..]);
        let mut doc = Doc {
            data,
            base,
            xref: Vec::new(),
            trailer: Dict::default(),
            version,
            objstms: RefCell::new(BTreeMap::new()),
            pages: Vec::new(),
        };
        if !doc.read_xref() || doc.root().is_none() {
            doc.reconstruct();
        }
        if doc.trailer.get(b"Encrypt").is_some_and(|e| *e != Obj::Null) {
            return Err(Error::Encrypted);
        }
        doc.pages = doc.read_pages().ok_or(Error::Broken)?;
        Ok(doc)
    }

    /// The catalog (`/Root`), followed.
    fn root(&self) -> Option<Dict> {
        match self.follow(self.trailer.get(b"Root")?) {
            Obj::Dict(d) => Some(d),
            _ => None,
        }
    }

    /// The object `r` refers to (`XRef::fetch`): null if there is none,
    /// or its header is not `num generation obj`.
    #[must_use]
    pub fn fetch(&self, r: Ref) -> Obj {
        let Some(Some(e)) = usize::try_from(r.num).ok().and_then(|i| self.xref.get(i)) else {
            return Obj::Null;
        };
        match *e {
            Entry::Free => Obj::Null,
            Entry::At { offset, generation } => {
                if generation != r.generation {
                    return Obj::Null;
                }
                let mut p = Parser::new(self.bytes(), offset, Some(self));
                let (n, g) = (p.obj(false, 0), p.obj(false, 0));
                if n != Obj::Int(r.num)
                    || g != Obj::Int(r.generation)
                    || !Parser::is_cmd(&p.buf1, b"obj")
                {
                    return Obj::Null;
                }
                p.shift();
                p.obj(true, 0)
            }
            Entry::InStream { stream, index } => {
                if r.generation != 0 {
                    return Obj::Null;
                }
                let objs = self.objstm(stream);
                objs.and_then(|o| {
                    let (n, v) = o.get(usize::try_from(index).ok()?)?;
                    (*n == r.num).then(|| v.clone())
                })
                .unwrap_or(Obj::Null)
            }
        }
    }

    /// `o`, followed if it is a reference (xpdf's `fetch`).
    #[must_use]
    pub fn follow(&self, o: &Obj) -> Obj {
        match o {
            Obj::Ref(r) => self.fetch(*r),
            o => o.clone(),
        }
    }

    /// The value of `key` in `d`, followed (xpdf's `lookup`).
    #[must_use]
    pub fn lookup(&self, d: &Dict, key: &[u8]) -> Obj {
        d.get(key).map_or(Obj::Null, |o| self.follow(o))
    }

    /// A stream's bytes as they are in the file.
    #[must_use]
    pub fn raw(&self, s: &Stream) -> &[u8] {
        let d = self.bytes();
        let end = s.start.saturating_add(s.len).min(d.len());
        &d[s.start.min(end)..end]
    }

    /// A stream's bytes decoded: its filters in order (`FlateDecode` with
    /// its predictor, `ASCIIHexDecode`, `ASCII85Decode`, `RunLengthDecode`,
    /// `LZWDecode`); `None` for another filter.
    #[must_use]
    pub fn decode(&self, s: &Stream) -> Option<Vec<u8>> {
        let mut data = self.raw(s).to_vec();
        let filters = match self.lookup(&s.dict, b"Filter") {
            Obj::Name(n) => alloc::vec![n],
            Obj::Array(a) => a
                .iter()
                .filter_map(|f| self.follow(f).as_name().map(<[u8]>::to_vec))
                .collect(),
            _ => Vec::new(),
        };
        let parms = match self.lookup(&s.dict, b"DecodeParms") {
            Obj::Array(a) => a.iter().map(|p| self.follow(p)).collect(),
            Obj::Null => Vec::new(),
            p => alloc::vec![p],
        };
        for (i, f) in filters.iter().enumerate() {
            let parm = parms.get(i).and_then(Obj::as_dict);
            data = match &f[..] {
                b"FlateDecode" | b"Fl" => {
                    let out = inflate(&data)?;
                    unpredict(&out, self.predictor(parm))?
                }
                b"LZWDecode" | b"LZW" => {
                    let early = parm
                        .and_then(|p| self.lookup(p, b"EarlyChange").as_int())
                        .unwrap_or(1);
                    unpredict(&lzw(&data, early != 0), self.predictor(parm))?
                }
                b"ASCIIHexDecode" | b"AHx" => ascii_hex(&data),
                b"ASCII85Decode" | b"A85" => ascii85(&data),
                b"RunLengthDecode" | b"RL" => run_length(&data),
                _ => return None,
            };
        }
        Some(data)
    }

    fn predictor(&self, parm: Option<&Dict>) -> Predictor {
        let mut p = Predictor::default();
        if let Some(d) = parm {
            let get = |k: &[u8], v: &mut i32| {
                if let Some(x) = self.lookup(d, k).as_int() {
                    *v = x;
                }
            };
            get(b"Predictor", &mut p.predictor);
            get(b"Colors", &mut p.colors);
            get(b"BitsPerComponent", &mut p.bits);
            get(b"Columns", &mut p.columns);
        }
        p
    }

    /// Object stream `n`'s objects (`ObjectStream`), read once.
    fn objstm(&self, n: i32) -> Option<ObjStm> {
        if let Some(o) = self.objstms.borrow().get(&n) {
            return Some(o.clone());
        }
        let Obj::Stream(s) = self.fetch(Ref {
            num: n,
            generation: 0,
        }) else {
            return None;
        };
        let count = self.lookup(&s.dict, b"N").as_int()?;
        let first = usize::try_from(self.lookup(&s.dict, b"First").as_int()?).ok()?;
        let data = self.decode(&s)?;
        let mut p = Parser::new(&data, 0, None);
        let mut heads = Vec::new();
        for _ in 0..count {
            let (Obj::Int(num), Obj::Int(off)) = (p.obj(false, 0), p.obj(false, 0)) else {
                return None;
            };
            heads.push((num, usize::try_from(off).ok()?));
        }
        let objs: Vec<(i32, Obj)> = heads
            .iter()
            .map(|&(num, off)| {
                let mut p = Parser::new(&data, first.saturating_add(off), None);
                (num, p.obj(false, 0))
            })
            .collect();
        let objs = Arc::new(objs);
        self.objstms.borrow_mut().insert(n, objs.clone());
        Some(objs)
    }

    fn set(&mut self, num: usize, e: Entry) {
        if self.xref.len() <= num {
            self.xref.resize(num + 1, None);
        }
        if self.xref[num].is_none() {
            self.xref[num] = Some(e);
        }
    }

    /// The cross-reference sections from `startxref`, newest first: an
    /// entry is kept from the newest section that has it. Whether they
    /// were read.
    fn read_xref(&mut self) -> bool {
        let Some(mut pos) = start_xref(self.bytes()) else {
            return false;
        };
        let mut seen = Vec::new();
        loop {
            if seen.contains(&pos) || seen.len() > 1000 {
                return true;
            }
            seen.push(pos);
            match self.read_section(pos) {
                Some(Section::Prev(prev)) => pos = prev,
                Some(Section::First) => return true,
                None => return false,
            }
        }
    }

    /// One section at `pos` (a table or a stream); its `/Prev`.
    fn read_section(&mut self, pos: usize) -> Option<Section> {
        let data = self.data.clone();
        let d = &data[self.base..];
        let mut p = Parser::new(d, pos, None);
        if Parser::is_cmd(&p.buf1, b"xref") {
            p.shift();
            loop {
                if Parser::is_cmd(&p.buf1, b"trailer") {
                    p.shift();
                    break;
                }
                let (Obj::Int(first), Obj::Int(n)) = (p.obj(false, 0), p.obj(false, 0)) else {
                    return None;
                };
                let (first, n) = (usize::try_from(first).ok()?, usize::try_from(n).ok()?);
                let mut i = first;
                let end = first.checked_add(n)?;
                let mut first = first;
                while i < end {
                    let (Obj::Int(off), Obj::Int(generation)) = (p.obj(false, 0), p.obj(false, 0))
                    else {
                        return None;
                    };
                    let e = if Parser::is_cmd(&p.buf1, b"n") {
                        Entry::At {
                            offset: usize::try_from(off).ok()?,
                            generation,
                        }
                    } else if Parser::is_cmd(&p.buf1, b"f") {
                        Entry::Free
                    } else {
                        return None;
                    };
                    p.shift();
                    // (IBM's patent files: a table from 1 that is from 0)
                    if i == 1 && first == 1 && off == 0 && generation == 65535 && e == Entry::Free {
                        first = 0;
                        i = 0;
                    }
                    self.set(i, e);
                    i += 1;
                }
                let _ = first;
            }
            let Obj::Dict(t) = p.obj(false, 0) else {
                return None;
            };
            let prev = t.get(b"Prev").and_then(Obj::as_int);
            let stm = t.get(b"XRefStm").and_then(Obj::as_int);
            if self.trailer.0.is_empty() {
                self.trailer = t;
            }
            if let Some(s) = stm.and_then(|s| usize::try_from(s).ok()) {
                self.read_section(s);
            }
            return Some(
                prev.and_then(|p| usize::try_from(p).ok())
                    .map_or(Section::First, Section::Prev),
            );
        }
        // (a stream: `num generation obj << /Type /XRef … >> stream`)
        p.obj(false, 0);
        p.obj(false, 0);
        if !Parser::is_cmd(&p.buf1, b"obj") {
            return None;
        }
        p.shift();
        let Obj::Stream(s) = p.obj(true, 0) else {
            return None;
        };
        let size = usize::try_from(s.dict.get(b"Size")?.as_int()?).ok()?;
        let Some(Obj::Array(w)) = s.dict.get(b"W") else {
            return None;
        };
        let w: Vec<usize> = w
            .iter()
            .take(3)
            .map(|x| x.as_int().and_then(|x| usize::try_from(x).ok()))
            .collect::<Option<_>>()?;
        if w.len() < 3 || w.iter().any(|&x| x > 8) {
            return None;
        }
        let rows = self.decode(&s)?;
        let index: Vec<(usize, usize)> = match s.dict.get(b"Index") {
            Some(Obj::Array(a)) => a
                .chunks(2)
                .filter_map(|c| {
                    Some((
                        usize::try_from(c.first()?.as_int()?).ok()?,
                        usize::try_from(c.get(1)?.as_int()?).ok()?,
                    ))
                })
                .collect(),
            _ => alloc::vec![(0, size)],
        };
        let width = w[0] + w[1] + w[2];
        let mut at = 0;
        let field = |row: &[u8], from: usize, n: usize| -> u64 {
            row[from..from + n]
                .iter()
                .fold(0u64, |v, &b| v << 8 | u64::from(b))
        };
        for (first, n) in index {
            for i in first..first.saturating_add(n) {
                let Some(row) = rows.get(at..at + width) else {
                    break;
                };
                at += width;
                let ty = if w[0] == 0 { 1 } else { field(row, 0, w[0]) };
                let (a, b) = (field(row, w[0], w[1]), field(row, w[0] + w[1], w[2]));
                let e = match ty {
                    0 => Entry::Free,
                    1 => Entry::At {
                        offset: usize::try_from(a).ok()?,
                        generation: i32::try_from(b).ok()?,
                    },
                    2 => Entry::InStream {
                        stream: i32::try_from(a).ok()?,
                        index: i32::try_from(b).ok()?,
                    },
                    _ => continue,
                };
                self.set(i, e);
            }
        }
        if self.trailer.0.is_empty() {
            self.trailer = s.dict.clone();
        }
        Some(
            s.dict
                .get(b"Prev")
                .and_then(Obj::as_int)
                .and_then(|p| usize::try_from(p).ok())
                .map_or(Section::First, Section::Prev),
        )
    }

    /// A damaged file's objects, found by their headers (`n g obj` at a
    /// line's start), the last of each number kept, its trailer the last
    /// with a `/Root`, and the objects of the object streams found
    /// (xpdf's `constructXRef`).
    fn reconstruct(&mut self) {
        self.xref.clear();
        self.trailer = Dict::default();
        let data = self.data.clone();
        let d = &data[self.base..];
        let mut found: BTreeMap<usize, (usize, i32)> = BTreeMap::new();
        let mut line = 0;
        while line < d.len() {
            let mut p = Lexer { d, pos: line };
            if d[line..].starts_with(b"trailer") {
                let mut pr = Parser::new(d, line + 7, None);
                if let Obj::Dict(t) = pr.obj(false, 0)
                    && t.get(b"Root").is_some()
                {
                    self.trailer = t;
                }
            } else if let (Tok::Obj(Obj::Int(n)), Tok::Obj(Obj::Int(g))) = (p.token(), p.token())
                && p.token() == Tok::Cmd(b"obj".to_vec())
                && let Ok(n) = usize::try_from(n)
            {
                found.insert(n, (line, g));
            }
            line = match d[line..].iter().position(|&c| c == b'\n' || c == b'\r') {
                Some(k) => line + k + 1,
                None => d.len(),
            };
        }
        for (&n, &(offset, generation)) in &found {
            if self.xref.len() <= n {
                self.xref.resize(n + 1, None);
            }
            self.xref[n] = Some(Entry::At { offset, generation });
        }
        // (the objects of the object streams, unless found whole)
        let nums: Vec<usize> = found.keys().copied().collect();
        for n in nums {
            let Ok(num) = i32::try_from(n) else { continue };
            let o = self.fetch(Ref {
                num,
                generation: found[&n].1,
            });
            if let Obj::Stream(s) = &o
                && s.dict.get(b"Type").and_then(Obj::as_name) == Some(b"ObjStm")
                && let Some(objs) = self.objstm(num)
            {
                for (k, (m, _)) in objs.iter().enumerate() {
                    if let (Ok(m), Ok(k)) = (usize::try_from(*m), i32::try_from(k))
                        && !found.contains_key(&m)
                    {
                        if self.xref.len() <= m {
                            self.xref.resize(m + 1, None);
                        }
                        self.xref[m] = Some(Entry::InStream {
                            stream: num,
                            index: k,
                        });
                    }
                }
            }
            if self.trailer.0.is_empty()
                && let Some(d) = o.as_dict()
                && d.get(b"Type").and_then(Obj::as_name) == Some(b"XRef")
            {
                self.trailer = d.clone();
            }
        }
        if self.root().is_none() {
            // (a catalog found by its type)
            for (&n, &(_, generation)) in &found {
                let Ok(num) = i32::try_from(n) else { continue };
                if self
                    .fetch(Ref { num, generation })
                    .as_dict()
                    .and_then(|d| d.get(b"Type"))
                    .and_then(Obj::as_name)
                    == Some(b"Catalog")
                {
                    self.trailer
                        .0
                        .push((b"Root".to_vec(), Obj::Ref(Ref { num, generation })));
                    break;
                }
            }
        }
    }

    /// The pages in order, each with what it inherits (the page tree
    /// walked from `/Pages`; a node that is not a dictionary skipped, a
    /// loop cut).
    fn read_pages(&self) -> Option<Vec<Page>> {
        let root = self.root()?;
        let top = root.get(b"Pages")?.clone();
        let mut pages = Vec::new();
        let mut seen = Vec::new();
        let attrs = Attrs {
            media: [0.0, 0.0, 612.0, 792.0],
            crop: None,
            rotate: 0,
            resources: None,
        };
        self.walk(&top, &attrs, &mut pages, &mut seen, 0);
        Some(pages)
    }

    fn walk(&self, node: &Obj, up: &Attrs, pages: &mut Vec<Page>, seen: &mut Vec<Ref>, depth: u32) {
        if depth > 64 {
            return;
        }
        let r = match node {
            Obj::Ref(r) => {
                if seen.contains(r) {
                    return;
                }
                seen.push(*r);
                Some(*r)
            }
            _ => None,
        };
        let Obj::Dict(d) = self.follow(node) else {
            return;
        };
        let attrs = up.with(self, &d);
        match self.lookup(&d, b"Kids") {
            Obj::Array(kids) if d.get(b"Type").and_then(Obj::as_name) != Some(b"Page") => {
                for k in &kids {
                    self.walk(k, &attrs, pages, seen, depth + 1);
                }
            }
            _ => {
                if let Some(r) = r {
                    pages.push(attrs.page(self, &d, r));
                }
            }
        }
    }

    /// The number of pages.
    #[must_use]
    pub fn num_pages(&self) -> usize {
        self.pages.len()
    }

    /// Page `n` (from 1).
    #[must_use]
    pub fn page(&self, n: usize) -> Option<&Page> {
        self.pages.get(n.checked_sub(1)?)
    }

    /// The number of the page `r` refers to (`Catalog::findPage`), 0 if
    /// it is none.
    #[must_use]
    pub fn find_page(&self, r: Ref) -> usize {
        self.pages
            .iter()
            .position(|p| p.dict_ref == r)
            .map_or(0, |i| i + 1)
    }

    /// The page a named destination is on (`PDFDoc::findDest`): in the
    /// catalog's `/Dests`, or its `/Names` tree's; its first element a
    /// page reference or a page number.
    #[must_use]
    pub fn find_dest(&self, name: &[u8]) -> Option<usize> {
        let root = self.root()?;
        let mut dest = match self.lookup(&root, b"Dests") {
            Obj::Dict(d) => self.lookup(&d, name),
            _ => Obj::Null,
        };
        if dest == Obj::Null
            && let Obj::Dict(names) = self.lookup(&root, b"Names")
        {
            dest = self.name_tree(&self.lookup(&names, b"Dests"), name, 0);
        }
        if let Obj::Dict(d) = &dest {
            dest = self.lookup(d, b"D");
        }
        let Obj::Array(a) = dest else {
            return None;
        };
        match a.first()? {
            Obj::Ref(r) => Some(self.find_page(*r)),
            Obj::Int(n) => usize::try_from(*n).ok().map(|n| n + 1),
            _ => None,
        }
    }

    fn name_tree(&self, node: &Obj, name: &[u8], depth: u32) -> Obj {
        let Obj::Dict(d) = self.follow(node) else {
            return Obj::Null;
        };
        if depth > 32 {
            return Obj::Null;
        }
        if let Obj::Array(a) = self.lookup(&d, b"Names") {
            for c in a.chunks(2) {
                if let [k, v] = c
                    && let Obj::Str(k) = self.follow(k)
                    && k == name
                {
                    return self.follow(v);
                }
            }
        }
        if let Obj::Array(kids) = self.lookup(&d, b"Kids") {
            for k in &kids {
                let o = self.name_tree(k, name, depth + 1);
                if o != Obj::Null {
                    return o;
                }
            }
        }
        Obj::Null
    }
}

/// What a page tree node passes down (`PageAttrs`).
struct Attrs {
    media: [f64; 4],
    crop: Option<[f64; 4]>,
    rotate: i32,
    resources: Option<Dict>,
}

impl Attrs {
    fn with(&self, doc: &Doc, d: &Dict) -> Attrs {
        let mut a = Attrs {
            media: self.media,
            crop: self.crop,
            rotate: self.rotate,
            resources: self.resources.clone(),
        };
        if let Some(b) = read_box(doc, d, b"MediaBox") {
            a.media = b;
        }
        if let Some(b) = read_box(doc, d, b"CropBox") {
            a.crop = Some(b);
        }
        if let Obj::Int(r) = doc.lookup(d, b"Rotate") {
            a.rotate = r.rem_euclid(360);
        }
        if let Obj::Dict(r) = doc.lookup(d, b"Resources") {
            a.resources = Some(r);
        }
        a
    }

    /// The page's boxes: the crop box the media box if none, the others
    /// the crop box if none, each clipped to the media box
    /// (`PageAttrs::clipBoxes`).
    fn page(&self, doc: &Doc, d: &Dict, r: Ref) -> Page {
        let crop = self.crop.unwrap_or(self.media);
        let other = |k: &[u8]| read_box(doc, d, k).unwrap_or(crop);
        let (bleed, trim, art) = (other(b"BleedBox"), other(b"TrimBox"), other(b"ArtBox"));
        let m = self.media;
        Page {
            dict_ref: r,
            media: m,
            crop: clip(crop, m),
            bleed: clip(bleed, m),
            trim: clip(trim, m),
            art: clip(art, m),
            rotate: self.rotate,
            resources: self.resources.clone(),
        }
    }
}

/// `PDFRectangle::clipTo`.
fn clip(mut b: [f64; 4], r: [f64; 4]) -> [f64; 4] {
    for (i, (lo, hi)) in [(r[0], r[2]), (r[1], r[3]), (r[0], r[2]), (r[1], r[3])]
        .into_iter()
        .enumerate()
    {
        if b[i] < lo {
            b[i] = lo;
        } else if b[i] > hi {
            b[i] = hi;
        }
    }
    b
}

/// `PageAttrs::readBox`: four numbers, each pair in order.
fn read_box(doc: &Doc, d: &Dict, key: &[u8]) -> Option<[f64; 4]> {
    let Obj::Array(a) = doc.lookup(d, key) else {
        return None;
    };
    if a.len() != 4 {
        return None;
    }
    let mut b = [0.0; 4];
    for (i, o) in a.iter().enumerate() {
        b[i] = doc.follow(o).as_num()?;
    }
    if b[0] > b[2] {
        b.swap(0, 2);
    }
    if b[1] > b[3] {
        b.swap(1, 3);
    }
    Some(b)
}

/// `startxref`'s offset, in the file's last 1,024 bytes.
fn start_xref(d: &[u8]) -> Option<usize> {
    let tail = &d[d.len().saturating_sub(1024)..];
    let i = tail.windows(9).rposition(|w| w == b"startxref")?;
    let rest = &tail[i + 9..];
    let s = rest.iter().position(|c| !c.is_ascii_whitespace())?;
    let digits: Vec<u8> = rest[s..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .copied()
        .collect();
    core::str::from_utf8(&digits).ok()?.parse().ok()
}

/// The header's version (`atof` of what follows `%PDF-`).
fn header_version(d: &[u8]) -> f64 {
    let Some(rest) = d.get(5..) else {
        return 0.0;
    };
    let mut v = 0.0;
    let mut scale = 0.0;
    for &c in rest.iter().take(16) {
        match c {
            b'0'..=b'9' if scale == 0.0 => v = v * 10.0 + f64::from(c - b'0'),
            b'0'..=b'9' => {
                v += scale * f64::from(c - b'0');
                scale /= 10.0;
            }
            b'.' if scale == 0.0 => scale = 0.1,
            _ => break,
        }
    }
    v
}

fn ascii_hex(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut m: Option<u32> = None;
    for &c in d {
        if c == b'>' {
            break;
        }
        if let Some(x) = char::from(c).to_digit(16) {
            match m.take() {
                Some(h) => out.push(u8::try_from(h << 4 | x).unwrap_or(0)),
                None => m = Some(x),
            }
        }
    }
    if let Some(h) = m {
        out.push(u8::try_from(h << 4).unwrap_or(0));
    }
    out
}

fn ascii85(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut group = [0u32; 5];
    let mut n = 0;
    for c in d.iter().copied() {
        match c {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group[n] = u32::from(c - b'!');
                n += 1;
                if n == 5 {
                    let v = group.iter().fold(0u64, |v, &g| v * 85 + u64::from(g));
                    out.extend_from_slice(
                        &u32::try_from(v & 0xffff_ffff).unwrap_or(0).to_be_bytes(),
                    );
                    n = 0;
                }
            }
            _ => {}
        }
    }
    if n > 1 {
        for g in group.iter_mut().skip(n) {
            *g = 84;
        }
        let v = group.iter().fold(0u64, |v, &g| v * 85 + u64::from(g));
        let b = u32::try_from(v & 0xffff_ffff).unwrap_or(0).to_be_bytes();
        out.extend_from_slice(&b[..n - 1]);
    }
    out
}

fn run_length(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < d.len() {
        let l = d[i];
        i += 1;
        match l {
            128 => break,
            0..=127 => {
                let n = usize::from(l) + 1;
                out.extend_from_slice(&d[i..(i + n).min(d.len())]);
                i += n;
            }
            _ => {
                if let Some(&b) = d.get(i) {
                    out.extend(core::iter::repeat_n(b, 257 - usize::from(l)));
                }
                i += 1;
            }
        }
    }
    out
}

/// LZW (TIFF's variant, PDF's `LZWDecode`).
fn lzw(d: &[u8], early: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut table: Vec<Vec<u8>> = Vec::new();
    let reset = |t: &mut Vec<Vec<u8>>| {
        t.clear();
        for b in 0..=255u8 {
            t.push(alloc::vec![b]);
        }
        t.push(Vec::new());
        t.push(Vec::new());
    };
    reset(&mut table);
    let (mut bits, mut buf, mut cnt, mut pos) = (9u32, 0u32, 0u32, 0usize);
    let mut prev: Option<Vec<u8>> = None;
    loop {
        while cnt < bits {
            let Some(&b) = d.get(pos) else {
                return out;
            };
            pos += 1;
            buf = buf << 8 | u32::from(b);
            cnt += 8;
        }
        let code = usize::try_from((buf >> (cnt - bits)) & ((1 << bits) - 1)).unwrap_or(0);
        cnt -= bits;
        match code {
            256 => {
                reset(&mut table);
                bits = 9;
                prev = None;
                continue;
            }
            257 => return out,
            _ => {}
        }
        let entry = if let Some(e) = table.get(code).filter(|e| !e.is_empty() || code < 256) {
            e.clone()
        } else if let Some(p) = &prev {
            let mut e = p.clone();
            e.push(p[0]);
            e
        } else {
            return out;
        };
        out.extend_from_slice(&entry);
        if let Some(mut p) = prev.take() {
            p.push(entry[0]);
            table.push(p);
        }
        prev = Some(entry);
        let n = table.len() + usize::from(early);
        bits = if n >= 2048 {
            12
        } else if n >= 1024 {
            11
        } else if n >= 512 {
            10
        } else {
            9
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small file with a classic table: two pages, the second rotated
    /// and with a crop box, resources inherited from the tree.
    fn classic() -> Vec<u8> {
        let mut f = b"%PDF-1.4\n".to_vec();
        let mut offs = Vec::new();
        let mut obj = |f: &mut Vec<u8>, body: &[u8]| {
            offs.push(f.len());
            let n = offs.len();
            f.extend_from_slice(alloc::format!("{n} 0 obj\n").as_bytes());
            f.extend_from_slice(body);
            f.extend_from_slice(b"\nendobj\n");
        };
        obj(&mut f, b"<< /Type /Catalog /Pages 2 0 R >>");
        obj(
            &mut f,
            b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 100] /Resources << /ProcSet [/PDF] >> >>",
        );
        obj(
            &mut f,
            b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Group << /S /Transparency >> >>",
        );
        obj(
            &mut f,
            b"<< /Type /Page /Parent 2 0 R /Rotate -90 /CropBox [10 300 -5 20] /Contents [5 0 R 5 0 R] >>",
        );
        obj(
            &mut f,
            b"<< /Length 6 0 R >>\nstream\n0 0 m 10 10 l S\nendstream",
        );
        obj(&mut f, b"15");
        let x = f.len();
        f.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
        for o in &offs {
            f.extend_from_slice(alloc::format!("{o:010} 00000 n \n").as_bytes());
        }
        f.extend_from_slice(
            alloc::format!("trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n")
                .as_bytes(),
        );
        f
    }

    #[test]
    fn reads_a_classic_file() {
        let doc = Doc::open(Arc::from(classic())).unwrap();
        assert!((doc.version - 1.4).abs() < 1e-9);
        assert_eq!(doc.num_pages(), 2);
        let p1 = doc.page(1).unwrap();
        assert_eq!(p1.media, [0.0, 0.0, 200.0, 100.0]);
        assert_eq!(p1.crop, p1.media);
        assert!(p1.resources.as_ref().unwrap().get(b"ProcSet").is_some());
        let p2 = doc.page(2).unwrap();
        assert_eq!(p2.rotate, 270);
        // (the crop box ordered and clipped to the media box)
        assert_eq!(p2.crop, [0.0, 20.0, 10.0, 100.0]);
        let Obj::Dict(d) = doc.fetch(p1.dict_ref) else {
            panic!()
        };
        let Obj::Stream(s) = doc.lookup(&d, b"Contents") else {
            panic!()
        };
        assert_eq!(doc.raw(&s), b"0 0 m 10 10 l S");
        assert_eq!(doc.find_page(p2.dict_ref), 2);
    }

    #[test]
    fn a_damaged_table_is_rebuilt_by_scanning() {
        let mut f = classic();
        // (`startxref` pointing nowhere)
        let at = f.windows(9).rposition(|w| w == b"startxref").unwrap();
        f.truncate(at);
        f.extend_from_slice(b"startxref\n3\n%%EOF\n");
        let doc = Doc::open(Arc::from(f)).unwrap();
        assert_eq!(doc.num_pages(), 2);
    }

    #[test]
    fn lexes_numbers_strings_and_names_as_xpdf() {
        let d = b"[-.5 3 -0 50-100 1.25 (a\\(b\\)\\101\r\nc) <4A4> /A#42c --1.5]";
        let mut p = Parser::new(d, 0, None);
        let Obj::Array(a) = p.obj(false, 0) else {
            panic!()
        };
        assert_eq!(a[0], Obj::Real(-0.5));
        assert_eq!(a[1], Obj::Int(3));
        assert_eq!(a[2], Obj::Int(0));
        assert_eq!(a[3], Obj::Int(50));
        assert_eq!(a[4], Obj::Real(1.25));
        assert_eq!(a[5], Obj::Str(b"a(b)A\nc".to_vec()));
        assert_eq!(a[6], Obj::Str(alloc::vec![0x4a, 0x40]));
        assert_eq!(a[7], Obj::Name(b"ABc".to_vec()));
        assert_eq!(a[8], Obj::Real(-1.5));
    }

    #[test]
    fn decodes_ascii_filters() {
        assert_eq!(ascii_hex(b"48 65 6C6c6F>"), b"Hello");
        assert_eq!(ascii85(b"87cURD]i,\"Ebo7~>"), b"Hello World");
        assert_eq!(
            run_length(&[2, b'a', b'b', b'c', 254, b'x', 128]),
            b"abcxxx"
        );
    }
}
