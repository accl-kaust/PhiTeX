//! An included PDF file, read as TeX Live's xpdf (`libs/xpdf`, 4.05)
//! reads it for pdfTeX's `pdftoepdf.cc`. Each part follows its xpdf
//! source: the lexer and the parser (`Lexer.cc`, `Parser.cc`: numbers,
//! strings and names as xpdf makes them, a stream's length from
//! `/Length` or from its `endstream`), the cross-reference sections
//! (`XRef.cc`: tables and streams, `/Prev`, `/XRefStm`, object streams;
//! a damaged file's objects found by scanning it), the page tree
//! (`Catalog.cc`: a page found by the counts of its tree's nodes), what a
//! page inherits (`Page.cc`'s `PageAttrs`: the boxes, the rotation, the
//! resources, merged where a node and its page both have them), the
//! named destinations, and the filters a page's contents are decoded
//! with (`Stream.cc`).
//!
//! What pdfTeX copies is a page's dictionary entries and the objects
//! they refer to, each byte of a string or a stream as it is in the
//! file: the writer (`partex-core`'s `pdf/epdf.rs`) prints these values.
//! An encrypted file is not read (xpdf reads one whose user password is
//! empty), nor are the image filters decoded (no page's contents use
//! them).

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::cmp::Ordering;

use crate::inflate::inflate;

/// An object's number and generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ref {
    pub num: i32,
    pub generation: i32,
}

/// A dictionary: its entries in the order their keys first came; a key
/// given twice keeps its first place and takes its last value (xpdf's
/// `Dict::add`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dict(pub Vec<(Vec<u8>, Obj)>);

impl Dict {
    /// The value of `key`, not followed if it is a reference (xpdf's
    /// `lookupNF`).
    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<&Obj> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn get_mut(&mut self, key: &[u8]) -> Option<&mut Obj> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// `Dict::add`: `key` bound to `v`, in its place if it is there.
    pub fn insert(&mut self, key: Vec<u8>, v: Obj) {
        match self.get_mut(&key) {
            Some(o) => *o = v,
            None => self.0.push((key, v)),
        }
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
    /// A keyword where an object was read (`objCmd`).
    Cmd(Vec<u8>),
    /// The lexer's error, a negative reference, a stream not made
    /// (`objError`).
    Error,
    /// The end of the data where an object was read (`objEOF`).
    Eof,
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

    /// xpdf's `objTypeNames`, for pdfTeX's messages.
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
            Obj::Cmd(_) => "cmd",
            Obj::Error => "error",
            Obj::Eof => "eof",
        }
    }
}

/// xpdf's `specialChars`: 1 whitespace, 2 a delimiter.
fn special(c: u8) -> u8 {
    match c {
        0 | b'\t' | b'\n' | 0x0c | b'\r' | b' ' => 1,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%' => 2,
        _ => 0,
    }
}

/// `Lexer::isSpace` of a byte, or of the end (`None`).
fn is_space(c: Option<u8>) -> bool {
    c.is_some_and(|c| special(c) == 1)
}

/// C's `isspace` (the C locale).
fn c_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// A hex digit's value.
fn hex_value(c: Option<u8>) -> Option<u8> {
    c.and_then(|c| char::from(c).to_digit(16))
        .and_then(|v| u8::try_from(v).ok())
}

/// `tokBufSize`: a keyword is at most one less.
const TOK_BUF: usize = 128;

/// xpdf's `Lexer`, over `d` (a file from its header, or the decoded
/// bytes of an object stream) from `pos`.
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

    /// `Lexer::skipToNextLine`: past the end of the line; false if the
    /// data ended first.
    fn skip_to_next_line(&mut self) -> bool {
        loop {
            match self.get() {
                None => return false,
                Some(b'\n') => return true,
                Some(b'\r') => {
                    if self.look() == Some(b'\n') {
                        self.pos += 1;
                    }
                    return true;
                }
                Some(_) => {}
            }
        }
    }

    /// `Lexer::getObj`: the next token.
    fn token(&mut self) -> Obj {
        // (whitespace and comments)
        let mut comment = false;
        let c = loop {
            let Some(c) = self.get() else {
                return Obj::Eof;
            };
            if comment {
                if c == b'\r' || c == b'\n' {
                    comment = false;
                }
            } else if c == b'%' {
                comment = true;
            } else if special(c) != 1 {
                break c;
            }
        };
        match c {
            b'0'..=b'9' | b'+' | b'-' | b'.' => self.number(c),
            b'(' => self.literal(),
            b'/' => self.name(),
            b'[' | b']' => Obj::Cmd(alloc::vec![c]),
            b'<' if self.look() == Some(b'<') => {
                self.pos += 1;
                Obj::Cmd(b"<<".to_vec())
            }
            b'<' => self.hex(),
            b'>' if self.look() == Some(b'>') => {
                self.pos += 1;
                Obj::Cmd(b">>".to_vec())
            }
            b'>' | b')' | b'{' | b'}' => Obj::Error,
            _ => self.command(c),
        }
    }

    /// A number, with Adobe's "interesting" cases as xpdf reads them:
    /// `--123` is 0, `50-100` is 50, a minus inside a real's fraction is
    /// skipped; an integer's digits wrap; a real is summed digit by digit.
    fn number(&mut self, c: u8) -> Obj {
        let (mut neg, mut double_minus, mut real) = (false, false, false);
        let (mut xi, mut xf) = (0i32, 0.0f64);
        match c {
            b'+' => {}
            b'-' => {
                neg = true;
                if self.look() == Some(b'-') {
                    double_minus = true;
                    while self.look() == Some(b'-') {
                        self.pos += 1;
                    }
                }
            }
            b'.' => real = true,
            _ => {
                xi = i32::from(c - b'0');
                xf = f64::from(c - b'0');
            }
        }
        while !real {
            match self.look() {
                Some(c @ b'0'..=b'9') => {
                    self.pos += 1;
                    xi = xi.wrapping_mul(10).wrapping_add(i32::from(c - b'0'));
                    if xf < 1e20 {
                        xf = xf * 10.0 + f64::from(c - b'0');
                    }
                }
                Some(b'.') => {
                    self.pos += 1;
                    real = true;
                }
                _ => break,
            }
        }
        if real {
            let mut scale = 0.1;
            loop {
                match self.look() {
                    // ("Badly formatted number")
                    Some(b'-') => self.pos += 1,
                    Some(c @ b'0'..=b'9') => {
                        self.pos += 1;
                        xf += scale * f64::from(c - b'0');
                        scale *= 0.1;
                    }
                    _ => break,
                }
            }
        }
        while matches!(self.look(), Some(b'-' | b'0'..=b'9')) {
            self.pos += 1;
        }
        if real {
            return Obj::Real(if neg { -xf } else { xf });
        }
        if neg {
            xi = xi.wrapping_neg();
        }
        if double_minus {
            xi = 0;
        }
        Obj::Int(xi)
    }

    /// A literal string: its escapes, and each end of line inside it read
    /// as a line feed.
    fn literal(&mut self) -> Obj {
        let mut s = Vec::new();
        let mut depth = 1;
        loop {
            let Some(c) = self.get() else {
                // ("Unterminated string")
                return Obj::Str(s);
            };
            match c {
                b'(' => {
                    depth += 1;
                    s.push(c);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Obj::Str(s);
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
                                    v = v << 3 | u32::from(c - b'0');
                                }
                                _ => break,
                            }
                        }
                        s.push(v.to_le_bytes()[0]);
                    }
                    Some(b'\r') => {
                        if self.look() == Some(b'\n') {
                            self.pos += 1;
                        }
                    }
                    Some(b'\n') => {}
                    Some(c) => s.push(c),
                    None => return Obj::Str(s),
                },
                _ => s.push(c),
            }
        }
    }

    /// A name: `#xx` escapes (a `#` not followed by a hex digit kept, one
    /// followed by a single digit its value); `#00` an error; a C string,
    /// so ended by a zero byte made otherwise.
    fn name(&mut self) -> Obj {
        let mut s = Vec::new();
        let mut invalid = false;
        while let Some(c) = self.look() {
            if special(c) != 0 {
                break;
            }
            self.pos += 1;
            let mut b = c;
            if c == b'#'
                && let Some(h) = hex_value(self.look())
            {
                self.pos += 1;
                b = h;
                if let Some(l) = hex_value(self.look()) {
                    self.pos += 1;
                    b = h << 4 | l;
                    invalid |= b == 0;
                }
            }
            s.push(b);
        }
        if invalid {
            // ("Null character in name")
            return Obj::Error;
        }
        if let Some(i) = s.iter().position(|&b| b == 0) {
            s.truncate(i);
        }
        Obj::Name(s)
    }

    /// A hex string: whitespace skipped, any other byte a digit (one not
    /// hex is 0, and after 100 of them the string ends), an odd last digit
    /// padded.
    fn hex(&mut self) -> Obj {
        let mut s = Vec::new();
        let (mut m, mut v) = (0, 0u8);
        let mut errors = 0;
        while errors < 100 {
            let Some(c) = self.get() else {
                break;
            };
            if c == b'>' {
                break;
            }
            if special(c) == 1 {
                continue;
            }
            v <<= 4;
            match hex_value(Some(c)) {
                Some(x) => v |= x,
                None => errors += 1,
            }
            m += 1;
            if m == 2 {
                s.push(v);
                (m, v) = (0, 0);
            }
        }
        if m == 1 {
            s.push(v << 4);
        }
        Obj::Str(s)
    }

    /// A keyword (at most 127 bytes: the 128th is dropped and ends it),
    /// or `true`, `false`, `null`.
    fn command(&mut self, c: u8) -> Obj {
        let mut k = alloc::vec![c];
        while let Some(c) = self.look() {
            if special(c) != 0 {
                break;
            }
            self.pos += 1;
            if k.len() + 1 == TOK_BUF {
                // ("Command token too long")
                break;
            }
            k.push(c);
        }
        match &k[..] {
            b"true" => Obj::Bool(true),
            b"false" => Obj::Bool(false),
            b"null" => Obj::Null,
            _ => Obj::Cmd(k),
        }
    }
}

/// How deep arrays, dictionaries and fetches nest
/// (`objectRecursionLimit`).
const DEPTH: u32 = 500;

/// xpdf's `Parser`: objects from the lexer, with its two tokens of
/// lookahead (`num generation R` is a reference).
struct Parser<'a, 'd> {
    lex: Lexer<'a>,
    buf1: Obj,
    buf2: Obj,
    /// The file (xpdf's `xref`; none while the sections are read).
    doc: Option<&'d Doc>,
    /// Whether a dictionary followed by `stream` is a stream
    /// (`allowStreams`).
    streams: bool,
}

impl<'a, 'd> Parser<'a, 'd> {
    fn new(d: &'a [u8], pos: usize, doc: Option<&'d Doc>, streams: bool) -> Self {
        let mut lex = Lexer { d, pos };
        let buf1 = lex.token();
        let buf2 = lex.token();
        Parser {
            lex,
            buf1,
            buf2,
            doc,
            streams,
        }
    }

    fn shift(&mut self) {
        self.buf1 = core::mem::replace(&mut self.buf2, Obj::Null);
        self.buf2 = self.lex.token();
    }

    fn is_cmd(o: &Obj, c: &[u8]) -> bool {
        matches!(o, Obj::Cmd(k) if k == c)
    }

    /// `Parser::getObj`; `simple` reads no array or dictionary (a `[` or
    /// `<<` is then the keyword itself).
    fn obj(&mut self, simple: bool, depth: u32) -> Obj {
        if !simple && depth < DEPTH && Self::is_cmd(&self.buf1, b"[") {
            self.shift();
            let mut a = Vec::new();
            while !Self::is_cmd(&self.buf1, b"]") && self.buf1 != Obj::Eof {
                a.push(self.obj(false, depth + 1));
            }
            self.shift();
            return Obj::Array(a);
        }
        if !simple && depth < DEPTH && Self::is_cmd(&self.buf1, b"<<") {
            self.shift();
            let mut d = Dict::default();
            while !Self::is_cmd(&self.buf1, b">>") && self.buf1 != Obj::Eof {
                let Obj::Name(key) = &self.buf1 else {
                    // ("Dictionary key must be a name object")
                    self.shift();
                    continue;
                };
                let key = key.clone();
                self.shift();
                if matches!(self.buf1, Obj::Eof | Obj::Error) {
                    break;
                }
                let v = self.obj(false, depth + 1);
                d.insert(key, v);
            }
            if self.streams && Self::is_cmd(&self.buf2, b"stream") {
                return match self.make_stream(d, depth + 1) {
                    Some(s) => Obj::Stream(Box::new(s)),
                    None => Obj::Error,
                };
            }
            self.shift();
            return Obj::Dict(d);
        }
        if let Obj::Int(num) = self.buf1 {
            self.shift();
            if let Obj::Int(generation) = self.buf1
                && Self::is_cmd(&self.buf2, b"R")
            {
                self.shift();
                self.shift();
                return if num >= 0 && generation >= 0 {
                    Obj::Ref(Ref { num, generation })
                } else {
                    Obj::Error
                };
            }
            return Obj::Int(num);
        }
        let o = core::mem::replace(&mut self.buf1, Obj::Null);
        self.shift();
        o
    }

    /// `Parser::makeStream`: the bytes after the line of `stream`, up to
    /// a repaired file's next `endstream`, or as many as `/Length` says
    /// (and 5,000 more if `endstream` is not after them), or, with no
    /// length, up to the first `endstream`.
    fn make_stream(&mut self, dict: Dict, depth: u32) -> Option<Stream> {
        if !self.lex.skip_to_next_line() {
            return None;
        }
        let pos = self.lex.pos;
        let d = self.lex.d;
        let mut length = None;
        if let Some(end) = self.doc.and_then(|doc| doc.stream_end(pos)) {
            length = Some(end - pos);
        } else {
            let l = match (dict.get(b"Length"), self.doc) {
                (Some(Obj::Ref(r)), Some(doc)) => doc.fetch_at(*r, depth),
                (Some(o), _) => o.clone(),
                (None, _) => Obj::Null,
            };
            if let Obj::Int(n) = l {
                // (`(Guint)`: a negative length is a huge one)
                length = Some(n.cast_unsigned() as usize);
            }
        }
        let len = if let Some(n) = length {
            let mut k = pos.saturating_add(n);
            let mut c = None;
            for _ in 0..100 {
                c = d.get(k).copied();
                k = k.saturating_add(1);
                if !is_space(c) {
                    break;
                }
            }
            if c == Some(b'e') && d.get(k..k.saturating_add(8)) == Some(b"ndstream") {
                n
            } else {
                // ("Missing 'endstream'")
                n.saturating_add(5000)
            }
        } else {
            let mut k = pos;
            loop {
                let c = *d.get(k)?;
                k += 1;
                if c == b'e' {
                    let b = &d[k..(k + 8).min(d.len())];
                    k += b.len();
                    if b == b"ndstream" {
                        break k - 9 - pos;
                    }
                }
            }
        };
        Some(Stream {
            dict,
            start: pos,
            len,
        })
    }
}

/// What an entry of the cross-reference holds (`XRefEntryType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Free,
    Uncompressed,
    Compressed,
}

/// A cross-reference entry (`XRefEntry`): where its object is (a
/// compressed one: in which object stream, and its index there as its
/// generation); unset while its offset is -1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    offset: i64,
    generation: i32,
    kind: Kind,
}

const UNSET: Entry = Entry {
    offset: -1,
    generation: 0,
    kind: Kind::Free,
};

/// An object stream's objects (`ObjectStream`): their numbers and
/// values.
struct ObjStm {
    nums: Vec<i32>,
    objs: Vec<Obj>,
}

/// The page boxes and what a page inherits.
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
    /// The resources (inherited, or merged), followed.
    pub resources: Option<Dict>,
}

/// An open PDF file.
pub struct Doc {
    /// The file's bytes; its offsets count from its `%PDF-` header
    /// (`base`: xpdf moves its start there).
    data: Arc<[u8]>,
    base: usize,
    entries: Vec<Entry>,
    /// The highest object number an entry was set for (`last`).
    last: i64,
    pub trailer: Dict,
    root: Option<Ref>,
    /// Where each `endstream` at a line's start is, in a repaired file
    /// (`streamEnds`).
    stream_ends: Vec<usize>,
    /// No section failed (`ok`).
    ok: bool,
    /// The header's version (`getPDFVersion`).
    pub version: f64,
    /// Object streams read, by number.
    objstms: RefCell<BTreeMap<i32, Option<Arc<ObjStm>>>>,
    /// The page tree's top node and its count (`numPages`).
    top: Ref,
    num_pages: usize,
    /// The catalog's `/Dests`, and its `/Names` `/Dests` (followed).
    dests: Obj,
    name_tree: Obj,
}

/// Why a file is not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No cross-reference, catalog or page tree was found.
    Broken,
    /// It is encrypted (not read yet).
    Encrypted,
}

impl Doc {
    /// The bytes of the file, from its header.
    fn bytes(&self) -> &[u8] {
        &self.data[self.base..]
    }

    /// `getNumObjects`.
    fn num_objects(&self) -> usize {
        usize::try_from(self.last + 1).unwrap_or(0)
    }

    /// Open `data` (`PDFDoc::setup`): its header, its cross-reference
    /// (read, or if that fails or its catalog does, rebuilt by scanning
    /// the file), its catalog and page tree.
    pub fn open(data: &Arc<[u8]>) -> Result<Doc, Error> {
        let (base, version) = check_header(data);
        for repair in [false, true] {
            let mut doc = Doc {
                data: data.clone(),
                base,
                entries: Vec::new(),
                last: -1,
                trailer: Dict::default(),
                root: None,
                stream_ends: Vec::new(),
                ok: true,
                version,
                objstms: RefCell::new(BTreeMap::new()),
                top: Ref {
                    num: 0,
                    generation: 0,
                },
                num_pages: 0,
                dests: Obj::Null,
                name_tree: Obj::Null,
            };
            if doc.read_xref(repair) {
                let t = doc.trailer.clone();
                if matches!(doc.lookup(&t, b"Encrypt"), Obj::Dict(_)) {
                    return Err(Error::Encrypted);
                }
                if doc.read_catalog() {
                    return Ok(doc);
                }
            }
        }
        Err(Error::Broken)
    }

    /// `XRef::XRef`: the sections from `startxref` (the catalog's
    /// reference in the first trailer, else the file scanned), or with
    /// `repair` the file scanned. Whether it was read.
    fn read_xref(&mut self, repair: bool) -> bool {
        if repair {
            return self.construct();
        }
        let mut pos = self.start_xref();
        if pos == 0 {
            return false;
        }
        let mut seen = Vec::new();
        while self.read_section(&mut pos, &mut seen, false) {}
        if !self.ok {
            return false;
        }
        match self.trailer.get(b"Root") {
            Some(Obj::Ref(r)) => {
                self.root = Some(*r);
                true
            }
            _ => self.construct(),
        }
    }

    /// `getStartXref`: the number after the last `startxref` of the
    /// file's last 1,024 bytes (0: none).
    fn start_xref(&self) -> u64 {
        let d = &self.data[..];
        let tail = &d[d.len().saturating_sub(1024)..];
        let Some(i) = tail.windows(9).rposition(|w| w == b"startxref") else {
            return 0;
        };
        let mut x = 0u64;
        for &c in tail[i + 9..]
            .iter()
            .skip_while(|&&c| c_space(c))
            .take_while(|c| c.is_ascii_digit())
        {
            let Some(y) = x.checked_mul(10).and_then(|x| x.checked_add(u64::from(c - b'0'))) else {
                break;
            };
            x = y;
        }
        x
    }

    /// Entry `i`, the table grown to `size` entries first if it is
    /// shorter.
    fn grow(&mut self, size: usize) {
        if self.entries.len() < size {
            self.entries.resize(size, UNSET);
        }
    }

    /// `readXRef`: the section at `pos` (a table with its trailer, or a
    /// stream); whether one follows (`/Prev`, in `pos`). A position seen
    /// before ends the chain.
    fn read_section(&mut self, pos: &mut u64, seen: &mut Vec<u64>, hybrid: bool) -> bool {
        if seen.contains(pos) {
            // ("Infinite loop in xref table")
            return false;
        }
        seen.push(*pos);
        let data = self.data.clone();
        let d = &data[self.base..];
        let p = usize::try_from(*pos).unwrap_or(usize::MAX);
        let buf = d.get(p..p.saturating_add(100).min(d.len())).unwrap_or(&[]);
        let i = buf.iter().take_while(|&&c| special(c) == 1).count();
        if !hybrid && i + 4 < buf.len() && &buf[i..i + 4] == b"xref" && special(buf[i + 4]) == 1 {
            return self.read_table(d, pos, p.saturating_add(i + 5), seen);
        }
        // (an xref stream: `num generation obj << … >> stream`)
        let mut parser = Parser::new(d, p, None, true);
        let ok = matches!(parser.obj(true, 0), Obj::Int(_))
            && matches!(parser.obj(true, 0), Obj::Int(_))
            && Parser::is_cmd(&parser.obj(true, 0), b"obj");
        if ok && let Obj::Stream(s) = parser.obj(false, 0) {
            return self.read_xref_stream(&s, pos);
        }
        if !hybrid {
            self.ok = false;
        }
        false
    }

    /// `readXRefTable`: the subsections from `k` (each entry set unless a
    /// newer section set it), then the trailer: kept if it is the first,
    /// its `/XRefStm` read, its `/Prev` the next section.
    fn read_table(&mut self, d: &[u8], pos: &mut u64, mut k: usize, seen: &mut Vec<u64>) -> bool {
        // (a byte as C's `getChar` gives it: -1 at the end)
        let get = |k: &mut usize| -> i32 {
            let c = d.get(*k).map_or(-1, |&c| i32::from(c));
            *k = k.saturating_add(1);
            c
        };
        let space = |c: i32| u8::try_from(c).is_ok_and(|c| special(c) == 1);
        let digit = |c: i32| (i32::from(b'0')..=i32::from(b'9')).contains(&c);
        loop {
            let mut c = get(&mut k);
            while space(c) {
                c = get(&mut k);
            }
            if c == i32::from(b't') {
                if d.get(k..k + 6) != Some(b"railer") {
                    self.ok = false;
                    return false;
                }
                k += 6;
                break;
            }
            if !digit(c) {
                self.ok = false;
                return false;
            }
            let mut first: i32 = 0;
            loop {
                let v = c - i32::from(b'0');
                if first > (i32::MAX - v) / 10 {
                    self.ok = false;
                    return false;
                }
                first = first * 10 + v;
                c = get(&mut k);
                if !digit(c) {
                    break;
                }
            }
            if !space(c) {
                self.ok = false;
                return false;
            }
            while space(c) {
                c = get(&mut k);
            }
            // (its first byte not checked, as xpdf's)
            let mut n: i32 = 0;
            loop {
                let v = c - i32::from(b'0');
                if n > (i32::MAX - v) / 10 {
                    self.ok = false;
                    return false;
                }
                n = n.wrapping_mul(10).wrapping_add(v);
                c = get(&mut k);
                if !digit(c) {
                    break;
                }
            }
            if !space(c) || first > i32::MAX - n {
                self.ok = false;
                return false;
            }
            let end = usize::try_from(first + n).unwrap_or(0);
            // (each entry takes more than 6 bytes: more than the file
            // has fails at its end, as xpdf's does after growing)
            if end > d.len() {
                self.ok = false;
                return false;
            }
            if end > self.entries.len() {
                let mut size = if self.entries.is_empty() {
                    512
                } else {
                    self.entries.len()
                };
                while size < end {
                    size *= 2;
                }
                self.grow(size);
            }
            let mut i = usize::try_from(first).unwrap_or(0);
            while i < end {
                let mut c = get(&mut k);
                while space(c) {
                    c = get(&mut k);
                }
                let mut off: i64 = 0;
                loop {
                    off = off.wrapping_mul(10).wrapping_add(i64::from(c - i32::from(b'0')));
                    c = get(&mut k);
                    if !digit(c) {
                        break;
                    }
                }
                if !space(c) {
                    self.ok = false;
                    return false;
                }
                while space(c) {
                    c = get(&mut k);
                }
                let mut generation: i32 = 0;
                loop {
                    generation = generation
                        .wrapping_mul(10)
                        .wrapping_add(c - i32::from(b'0'));
                    c = get(&mut k);
                    if !digit(c) {
                        break;
                    }
                }
                if !space(c) {
                    self.ok = false;
                    return false;
                }
                while space(c) {
                    c = get(&mut k);
                }
                let kind = if c == i32::from(b'n') {
                    Kind::Uncompressed
                } else if c == i32::from(b'f') {
                    Kind::Free
                } else {
                    self.ok = false;
                    return false;
                };
                if !space(get(&mut k)) {
                    self.ok = false;
                    return false;
                }
                if self.entries[i].offset == -1 {
                    self.entries[i] = Entry {
                        offset: off,
                        generation,
                        kind,
                    };
                    // (IBM's patent files: a table said to start at 1
                    // that starts at 0)
                    if i == 1
                        && first == 1
                        && self.entries[1]
                            == (Entry {
                                offset: 0,
                                generation: 65535,
                                kind: Kind::Free,
                            })
                    {
                        i = 0;
                        first = 0;
                        self.entries[0] = self.entries[1];
                        self.entries[1].offset = -1;
                    }
                    self.last = self.last.max(i64::try_from(i).unwrap_or(0));
                }
                i += 1;
            }
        }
        let mut parser = Parser::new(d, k, None, true);
        let Obj::Dict(t) = parser.obj(false, 0) else {
            self.ok = false;
            return false;
        };
        let more = match t.get(b"Prev") {
            Some(Obj::Int(n)) => {
                *pos = u64::from(n.cast_unsigned());
                true
            }
            // (buggy writers' `/Prev NNN 0 R`)
            Some(Obj::Ref(r)) => {
                *pos = u64::from(r.num.cast_unsigned());
                true
            }
            _ => false,
        };
        let stm = t.get(b"XRefStm").and_then(Obj::as_int);
        if self.trailer.0.is_empty() {
            self.trailer = t;
        }
        if let Some(s) = stm {
            let mut p2 = u64::from(s.cast_unsigned());
            self.read_section(&mut p2, seen, true);
            if !self.ok {
                return false;
            }
        }
        more
    }

    /// `readXRefStream`: an xref stream's subsections (`/W`, `/Index`)
    /// read from its decoded bytes; its `/Prev`.
    fn read_xref_stream(&mut self, s: &Stream, pos: &mut u64) -> bool {
        let Some(Obj::Int(size)) = s.dict.get(b"Size") else {
            self.ok = false;
            return false;
        };
        let Ok(size) = usize::try_from(*size) else {
            self.ok = false;
            return false;
        };
        let data = self.decode(s);
        let Some(Obj::Array(wa)) = s.dict.get(b"W") else {
            self.ok = false;
            return false;
        };
        if wa.len() < 3 {
            self.ok = false;
            return false;
        }
        let mut w = [0usize; 3];
        for (i, x) in wa.iter().take(3).enumerate() {
            let Obj::Int(v) = x else {
                self.ok = false;
                return false;
            };
            let Ok(v) = usize::try_from(*v) else {
                self.ok = false;
                return false;
            };
            if v > 8 {
                self.ok = false;
                return false;
            }
            w[i] = v;
        }
        // (a size the data cannot fill fails, as xpdf's does at its end)
        if size > data.len().saturating_add(1) << 3 && w.iter().sum::<usize>() > 0 {
            self.ok = false;
            return false;
        }
        self.grow(size);
        let mut at = 0;
        let sections: Vec<(i32, i32)> = match s.dict.get(b"Index") {
            Some(Obj::Array(a)) => {
                let mut v = Vec::new();
                let mut i = 0;
                while i + 1 < a.len() {
                    let (Obj::Int(f), Obj::Int(n)) = (&a[i], &a[i + 1]) else {
                        self.ok = false;
                        return false;
                    };
                    v.push((*f, *n));
                    i += 2;
                }
                v
            }
            _ => alloc::vec![(0, i32::try_from(size).unwrap_or(i32::MAX))],
        };
        for (first, n) in sections {
            if first < 0 || n < 0 || !self.read_xref_stream_section(&data, &mut at, w, first, n) {
                self.ok = false;
                return false;
            }
        }
        let more = match s.dict.get(b"Prev") {
            Some(Obj::Int(n)) => {
                *pos = u64::from(n.cast_unsigned());
                true
            }
            _ => false,
        };
        if self.trailer.0.is_empty() {
            self.trailer = s.dict.clone();
        }
        more
    }

    /// `readXRefStreamSection`: entries `first..first + n`, read from
    /// `data` at `at`.
    fn read_xref_stream_section(
        &mut self,
        data: &[u8],
        at: &mut usize,
        w: [usize; 3],
        first: i32,
        n: i32,
    ) -> bool {
        let Some(end) = first.checked_add(n) else {
            return false;
        };
        let (first, end) = (usize::try_from(first).unwrap_or(0), usize::try_from(end).unwrap_or(0));
        let width: usize = w.iter().sum();
        if (end - first).saturating_mul(width) > data.len().saturating_sub(*at) {
            return false;
        }
        if end > self.entries.len() {
            let mut size = if self.entries.is_empty() {
                1024
            } else {
                2 * self.entries.len()
            };
            while size < end {
                size <<= 1;
            }
            self.grow(size);
        }
        let mut field = |n: usize| -> u64 {
            let v = data[*at..*at + n]
                .iter()
                .fold(0u64, |v, &b| v << 8 | u64::from(b));
            *at += n;
            v
        };
        for i in first..end {
            let kind = if w[0] == 0 { 1 } else { field(w[0]) };
            let offset = field(w[1]);
            let generation = field(w[2]);
            let Ok(offset) = i64::try_from(offset) else {
                return false;
            };
            // (a free entry with generation 0xffffffff is written by some)
            if generation > 0x7fff_ffff && kind != 0 {
                return false;
            }
            if self.entries[i].offset == -1 {
                let kind = match kind {
                    0 => Kind::Free,
                    1 => Kind::Uncompressed,
                    2 => Kind::Compressed,
                    _ => return false,
                };
                self.entries[i] = Entry {
                    offset,
                    generation: u32::try_from(generation & 0xffff_ffff).unwrap_or(0).cast_signed(),
                    kind,
                };
                self.last = self.last.max(i64::try_from(i).unwrap_or(0));
            }
        }
        true
    }

    /// `constructXRef`: a damaged file scanned for `n g obj` headers (the
    /// newest of each number kept), trailers (the last with a `/Root`),
    /// the `endstream`s that end its streams, then its xref streams'
    /// dictionaries and its object streams' objects. Whether a catalog
    /// was found.
    fn construct(&mut self) -> bool {
        self.root = None;
        self.stream_ends.clear();
        let data = self.data.clone();
        let d = &data[self.base..];
        // (a byte, 0 past the end: xpdf's buffer ends with one)
        let at = |p: usize| d.get(p).copied().unwrap_or(0);
        let mut stream_nums = Vec::new();
        let mut last_num: i64 = -1;
        let (mut start_of_line, mut space) = (true, true);
        let mut p = 0;
        while p < d.len() {
            let rest = &d[p..];
            if start_of_line && rest.starts_with(b"trailer") {
                self.construct_trailer(d, p + 7);
                p += 7;
                (start_of_line, space) = (false, false);
            } else if start_of_line && rest.starts_with(b"endstream") {
                self.stream_ends.push(p);
                p += 9;
                (start_of_line, space) = (false, false);
            } else if space && rest[0].is_ascii_digit() {
                p = self.construct_entry(d, p, &mut last_num);
                (start_of_line, space) = (false, false);
            } else if rest.starts_with(b">>") {
                p += 2;
                (start_of_line, space) = (false, false);
                // (any PDF whitespace but a zero byte)
                while matches!(at(p), b'\t' | b'\n' | 0x0c | b'\r' | b' ') {
                    if matches!(at(p), b'\n' | b'\r') {
                        start_of_line = true;
                    }
                    space = true;
                    p += 1;
                }
                if d[p.min(d.len())..].starts_with(b"stream") {
                    if last_num >= 0 {
                        stream_nums.push(last_num);
                    }
                    p += 6;
                    (start_of_line, space) = (false, false);
                }
            } else {
                let c = rest[0];
                if c == b'\n' || c == b'\r' {
                    (start_of_line, space) = (true, true);
                } else if special(c) == 1 {
                    space = true;
                } else {
                    (start_of_line, space) = (false, false);
                }
                p += 1;
            }
        }
        for num in stream_nums {
            let (Ok(i), Ok(n)) = (usize::try_from(num), i32::try_from(num)) else {
                continue;
            };
            let generation = self.entries.get(i).map_or(0, |e| e.generation);
            let o = self.fetch(Ref { num: n, generation });
            let Obj::Stream(s) = &o else {
                continue;
            };
            match self.lookup(&s.dict, b"Type") {
                Obj::Name(t) if t == b"XRef" => {
                    let dict = s.dict.clone();
                    self.save_trailer(&dict, true);
                }
                Obj::Name(t) if t == b"ObjStm" => self.construct_objstm_entries(s, n),
                _ => {}
            }
        }
        // (the objects fetched before the scan's end are not kept)
        self.objstms.borrow_mut().clear();
        self.root.is_some()
    }

    /// `constructTrailerDict`.
    fn construct_trailer(&mut self, d: &[u8], pos: usize) {
        let mut parser = Parser::new(d, pos, None, false);
        if let Obj::Dict(t) = parser.obj(false, 0) {
            self.save_trailer(&t, false);
        }
    }

    /// `saveTrailerDict`: a dictionary with a `/Root` reference kept as
    /// the trailer (an xref stream's only if its root is a number seen).
    fn save_trailer(&mut self, d: &Dict, xref_stream: bool) {
        if let Some(Obj::Ref(r)) = d.get(b"Root")
            && (!xref_stream || i64::from(r.num) <= self.last)
        {
            self.root = Some(*r);
            self.trailer = d.clone();
        }
    }

    /// `constructObjectEntry`: `num gen obj` at `p` (spaces and tabs
    /// between, not line ends), set as an entry; where the scan goes on.
    fn construct_entry(&mut self, d: &[u8], mut p: usize, last_num: &mut i64) -> usize {
        let pos = p;
        let at = |p: usize| d.get(p).copied().unwrap_or(0);
        let gap = |c: u8| matches!(c, b'\t' | 0x0c | b' ');
        let mut num: i32 = 0;
        loop {
            num = num * 10 + i32::from(at(p) - b'0');
            p += 1;
            if !(at(p).is_ascii_digit() && num < 100_000_000) {
                break;
            }
        }
        if !gap(at(p)) {
            return p;
        }
        while gap(at(p)) {
            p += 1;
        }
        if !at(p).is_ascii_digit() {
            return p;
        }
        let mut generation: i32 = 0;
        loop {
            generation = generation * 10 + i32::from(at(p) - b'0');
            p += 1;
            if !(at(p).is_ascii_digit() && generation < 100_000_000) {
                break;
            }
        }
        if !gap(at(p)) {
            return p;
        }
        while gap(at(p)) {
            p += 1;
        }
        if !d[p.min(d.len())..].starts_with(b"obj") {
            return p;
        }
        // (the entry's offset: where its number began)
        if self.construct_set(num, generation, i64::try_from(pos).unwrap_or(0), Kind::Uncompressed) {
            *last_num = i64::from(num);
        }
        p
    }

    /// `constructXRefEntry`: entry `num` set unless it holds an object of
    /// a higher generation.
    fn construct_set(&mut self, num: i32, generation: i32, offset: i64, kind: Kind) -> bool {
        let Ok(n) = usize::try_from(num) else {
            return false;
        };
        if n >= self.entries.len() {
            self.grow((n + 1 + 255) & !255);
        }
        let e = &mut self.entries[n];
        if e.kind == Kind::Free || generation >= e.generation {
            *e = Entry {
                offset,
                generation,
                kind,
            };
            self.last = self.last.max(i64::from(num));
        }
        true
    }

    /// `constructObjectStreamEntries`: the objects object stream `num`
    /// holds, by its header, set as compressed entries.
    fn construct_objstm_entries(&mut self, s: &Stream, num: i32) {
        let Obj::Int(n) = self.lookup(&s.dict, b"N") else {
            return;
        };
        if n <= 0 || n > 1_000_000 {
            return;
        }
        let data = self.decode(s);
        let mut parser = Parser::new(&data, 0, None, false);
        for i in 0..n {
            let (a, b) = (parser.obj(true, 0), parser.obj(true, 0));
            if let (Obj::Int(m), Obj::Int(_)) = (a, b)
                && (0..1_000_000).contains(&m)
            {
                self.construct_set(m, i, i64::from(num), Kind::Compressed);
            }
        }
    }

    /// `getStreamEnd`: in a repaired file, the first `endstream` at or
    /// after `pos`.
    fn stream_end(&self, pos: usize) -> Option<usize> {
        let i = self.stream_ends.partition_point(|&e| e < pos);
        self.stream_ends.get(i).copied()
    }

    /// The object `r` refers to (`XRef::fetch`): null if there is none, or
    /// its header is not `num generation obj`; an object stream's object
    /// whatever its reference's generation (as Adobe's reader).
    #[must_use]
    pub fn fetch(&self, r: Ref) -> Obj {
        self.fetch_at(r, 0)
    }

    fn fetch_at(&self, r: Ref, depth: u32) -> Obj {
        let Some(e) = usize::try_from(r.num)
            .ok()
            .and_then(|i| self.entries.get(i))
            .copied()
        else {
            return Obj::Null;
        };
        match e.kind {
            Kind::Uncompressed => {
                if e.generation != r.generation {
                    return Obj::Null;
                }
                let Ok(off) = usize::try_from(e.offset) else {
                    return Obj::Null;
                };
                let mut p = Parser::new(self.bytes(), off, Some(self), true);
                let (n, g, o) = (p.obj(true, depth), p.obj(true, depth), p.obj(true, depth));
                if n != Obj::Int(r.num) || g != Obj::Int(r.generation) || !Parser::is_cmd(&o, b"obj") {
                    return Obj::Null;
                }
                p.obj(false, depth)
            }
            Kind::Compressed => {
                let stm_ok = usize::try_from(e.offset)
                    .ok()
                    .and_then(|i| self.entries.get(i))
                    .is_some_and(|s| s.kind == Kind::Uncompressed);
                let (true, Ok(stm)) = (stm_ok && depth < DEPTH, i32::try_from(e.offset)) else {
                    return Obj::Null;
                };
                let Some(objs) = self.objstm(stm, depth + 1) else {
                    return Obj::Null;
                };
                match usize::try_from(e.generation) {
                    Ok(i) if objs.nums.get(i) == Some(&r.num) => objs.objs[i].clone(),
                    _ => Obj::Null,
                }
            }
            Kind::Free => Obj::Null,
        }
    }

    /// Object stream `n` (`ObjectStream`), read once: its header's
    /// numbers and offsets (from `/First` on, in order), each object
    /// parsed from its offset to the next one's.
    fn objstm(&self, n: i32, depth: u32) -> Option<Arc<ObjStm>> {
        if let Some(o) = self.objstms.borrow().get(&n) {
            return o.clone();
        }
        let read = || -> Option<ObjStm> {
            let Obj::Stream(s) = self.fetch_at(
                Ref {
                    num: n,
                    generation: 0,
                },
                depth,
            ) else {
                return None;
            };
            let follow = |o: Option<&Obj>| match o {
                Some(Obj::Ref(r)) => self.fetch_at(*r, depth),
                Some(o) => o.clone(),
                None => Obj::Null,
            };
            let Obj::Int(count) = follow(s.dict.get(b"N")) else {
                return None;
            };
            if count <= 0 || count > 1_000_000 {
                return None;
            }
            let Obj::Int(first) = follow(s.dict.get(b"First")) else {
                return None;
            };
            let first = usize::try_from(first).ok()?;
            let data = self.decode(&s);
            let head = &data[..first.min(data.len())];
            let mut p = Parser::new(head, 0, None, false);
            let mut nums = Vec::new();
            let mut offs: Vec<usize> = Vec::new();
            for _ in 0..count {
                let (Obj::Int(num), Obj::Int(off)) = (p.obj(true, 0), p.obj(true, 0)) else {
                    return None;
                };
                let off = usize::try_from(off).ok()?;
                if num < 0 || offs.last().is_some_and(|&l| off < l) {
                    return None;
                }
                nums.push(num);
                offs.push(off);
            }
            let objs = (0..offs.len())
                .map(|i| {
                    let a = first.saturating_add(offs[i]).min(data.len());
                    let b = offs
                        .get(i + 1)
                        .map_or(data.len(), |&o| first.saturating_add(o).min(data.len()));
                    Parser::new(&data[a..b.max(a)], 0, Some(self), false).obj(false, 0)
                })
                .collect();
            Some(ObjStm { nums, objs })
        };
        let o = read().map(Arc::new);
        self.objstms.borrow_mut().insert(n, o.clone());
        o
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

    /// A stream's bytes decoded (`addFilters`): its `/Filter` (else `/F`)
    /// with its `/DecodeParms` (else `/DP`): `FlateDecode` and
    /// `LZWDecode` with their predictors, `ASCIIHexDecode`,
    /// `ASCII85Decode`, `RunLengthDecode`; another filter (an image's)
    /// gives nothing, as a bad one does.
    #[must_use]
    pub fn decode(&self, s: &Stream) -> Vec<u8> {
        let mut data = self.raw(s).to_vec();
        let mut f = self.lookup(&s.dict, b"Filter");
        if f == Obj::Null {
            f = self.lookup(&s.dict, b"F");
        }
        let mut parms = self.lookup(&s.dict, b"DecodeParms");
        if parms == Obj::Null {
            parms = self.lookup(&s.dict, b"DP");
        }
        match f {
            Obj::Name(n) => data = self.filter(&n, &data, &parms).unwrap_or_default(),
            Obj::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    let p = match &parms {
                        Obj::Array(pa) if i < pa.len() => self.follow(&pa[i]),
                        _ => Obj::Null,
                    };
                    let Obj::Name(n) = self.follow(x) else {
                        // ("Bad filter name")
                        return Vec::new();
                    };
                    match self.filter(&n, &data, &p) {
                        Some(d) => data = d,
                        None => return Vec::new(),
                    }
                }
            }
            _ => {}
        }
        data
    }

    /// One filter (`makeFilter`); `None` for one not decoded here.
    fn filter(&self, name: &[u8], d: &[u8], parms: &Obj) -> Option<Vec<u8>> {
        let int = |k: &[u8], v: i32| match parms {
            Obj::Dict(p) => self.lookup(p, k).as_int().unwrap_or(v),
            _ => v,
        };
        let pred = || Pred {
            predictor: int(b"Predictor", 1),
            colors: int(b"Colors", 1),
            bits: int(b"BitsPerComponent", 8),
            columns: int(b"Columns", 1),
        };
        Some(match name {
            b"FlateDecode" | b"Fl" => pred().apply(&inflate(d).unwrap_or_default()),
            b"LZWDecode" | b"LZW" => pred().apply(&lzw(d, int(b"EarlyChange", 1))),
            b"ASCIIHexDecode" | b"AHx" => ascii_hex(d),
            b"ASCII85Decode" | b"A85" => ascii85(d),
            b"RunLengthDecode" | b"RL" => run_length(d),
            _ => return None,
        })
    }

    /// `Catalog::Catalog`: the catalog, its page tree's top node and
    /// count, its named destinations. Whether they were found.
    fn read_catalog(&mut self) -> bool {
        let Some(root) = self.root else {
            return false;
        };
        let Obj::Dict(cat) = self.fetch(root) else {
            return false;
        };
        let Some(Obj::Ref(top)) = cat.get(b"Pages").cloned() else {
            return false;
        };
        let Obj::Dict(t) = self.fetch(top) else {
            return false;
        };
        let n = match self.lookup(&t, b"Count") {
            // (Acrobat counts a zero count's tree; an absurd count is
            // counted too)
            Obj::Int(n) if n == 0 || n > 50_000 => {
                let mut touched = alloc::vec![false; self.num_objects()];
                self.count_tree(&Obj::Ref(top), &mut touched)
            }
            Obj::Int(n) => n,
            // (a page, not a page tree)
            _ => 1,
        };
        let Ok(n) = usize::try_from(n) else {
            return false;
        };
        self.top = top;
        self.num_pages = n;
        self.dests = self.lookup(&cat, b"Dests");
        self.name_tree = match self.lookup(&cat, b"Names") {
            Obj::Dict(names) => self.lookup(&names, b"Dests"),
            _ => Obj::Null,
        };
        true
    }

    /// `countPageTree`: the leaves under `node` (a node without `/Kids`
    /// is one), each object visited once.
    fn count_tree(&self, node: &Obj, touched: &mut [bool]) -> i32 {
        let o = match node {
            Obj::Ref(r) => {
                let Some(t) = usize::try_from(r.num).ok().and_then(|i| touched.get_mut(i)) else {
                    return 0;
                };
                if *t {
                    // ("Loop in Pages tree")
                    return 0;
                }
                *t = true;
                self.fetch(*r)
            }
            o => o.clone(),
        };
        let Obj::Dict(d) = o else {
            return 0;
        };
        let kids = match d.get(b"Kids") {
            Some(Obj::Ref(r)) if usize::try_from(r.num).is_ok_and(|i| i < touched.len()) => {
                let i = usize::try_from(r.num).unwrap_or(0);
                if touched[i] {
                    return 0;
                }
                touched[i] = true;
                self.fetch(*r)
            }
            Some(o) => o.clone(),
            None => Obj::Null,
        };
        let Obj::Array(kids) = kids else {
            return 1;
        };
        let mut n: i32 = 0;
        for k in &kids {
            let m = self.count_tree(k, touched);
            n = if m < i32::MAX - n { n + m } else { i32::MAX };
        }
        n
    }

    /// The number of pages (`getNumPages`).
    #[must_use]
    pub fn num_pages(&self) -> usize {
        self.num_pages
    }

    /// Page `n` (from 1), found as `Catalog::loadPage2` finds it: down
    /// the tree by its nodes' counts (a kid without one counts 1), each
    /// node's attributes passed down; none if the tree does not lead to
    /// it.
    #[must_use]
    pub fn page(&self, n: usize) -> Option<Page> {
        let mut node = self.top;
        let mut count = i64::try_from(self.num_pages).ok()?;
        let mut rel = i64::try_from(n).ok()? - 1;
        let mut parents: Vec<Ref> = Vec::new();
        let mut up: Option<Attrs> = None;
        loop {
            if rel < 0 || rel >= count || parents.contains(&node) {
                return None;
            }
            let Obj::Dict(d) = self.fetch(node) else {
                return None;
            };
            let attrs = Attrs::new(up.as_ref(), self, &d);
            let Obj::Array(kids) = self.lookup(&d, b"Kids") else {
                return Some(attrs.page(node));
            };
            let mut next = None;
            for k in &kids {
                let Obj::Ref(kr) = k else {
                    continue;
                };
                let Obj::Dict(kd) = self.fetch(*kr) else {
                    continue;
                };
                let c = match self.lookup(&kd, b"Count") {
                    Obj::Int(c) => i64::from(c),
                    _ => 1,
                };
                if rel < c {
                    next = Some((*kr, c));
                    break;
                }
                rel -= c;
            }
            let (kr, c) = next?;
            parents.push(node);
            up = Some(attrs);
            node = kr;
            count = c;
        }
    }

    /// The number of the page `r` refers to (`Catalog::findPage`), 0 if
    /// it is none.
    #[must_use]
    pub fn find_page(&self, r: Ref) -> usize {
        (1..=self.num_pages)
            .find(|&i| self.page(i).is_some_and(|p| p.dict_ref == r))
            .unwrap_or(0)
    }

    /// The page a named destination is on (`PDFDoc::findDest`, then
    /// `findPage` of its page reference): `None` if there is no such
    /// destination or it is not a valid one (`LinkDest::isOk`); 0 if it
    /// names no page (one given by number names none: pdfTeX asks for
    /// its reference).
    #[must_use]
    pub fn find_dest(&self, name: &[u8]) -> Option<usize> {
        // (the catalog's dictionary is looked up with a C string)
        let cname = &name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())];
        let mut dest = match &self.dests {
            Obj::Dict(d) => self.lookup(d, cname),
            _ => Obj::Null,
        };
        if dest == Obj::Null
            && let Obj::Dict(tree) = &self.name_tree
        {
            let mut touched = alloc::vec![false; self.num_objects()];
            dest = self.dest_in_tree(&Obj::Null, tree, name, &mut touched);
        }
        let a = match dest {
            Obj::Array(a) => a,
            Obj::Dict(d) => match self.lookup(&d, b"D") {
                Obj::Array(a) => a,
                _ => return None,
            },
            _ => return None,
        };
        Some(match link_dest(self, &a)? {
            DestPage::Number => 0,
            DestPage::Page(r) => self.find_page(r),
        })
    }

    /// `findDestInTree`: a leaf's names in order (stopping past `name`),
    /// or the kid whose `/Limits` hold it.
    fn dest_in_tree(&self, tree_ref: &Obj, tree: &Dict, name: &[u8], touched: &mut [bool]) -> Obj {
        if let Obj::Ref(r) = tree_ref {
            let Some(t) = usize::try_from(r.num).ok().and_then(|i| touched.get_mut(i)) else {
                return Obj::Null;
            };
            if *t {
                // ("Loop in destination name tree")
                return Obj::Null;
            }
            *t = true;
        }
        if let Obj::Array(names) = self.lookup(tree, b"Names") {
            let mut i = 0;
            while i < names.len() {
                if let Obj::Str(k) = self.follow(&names[i]) {
                    match name.cmp(&k[..]) {
                        Ordering::Equal => {
                            return names.get(i + 1).map_or(Obj::Null, |v| self.follow(v));
                        }
                        Ordering::Less => return Obj::Null,
                        Ordering::Greater => {}
                    }
                }
                i += 2;
            }
            return Obj::Null;
        }
        if let Obj::Array(kids) = self.lookup(tree, b"Kids") {
            for k in &kids {
                let Obj::Dict(kd) = self.follow(k) else {
                    continue;
                };
                let Obj::Array(lim) = self.lookup(&kd, b"Limits") else {
                    continue;
                };
                let s = |i: usize| match lim.get(i).map(|o| self.follow(o)) {
                    Some(Obj::Str(s)) => Some(s),
                    _ => None,
                };
                if let Some(low) = s(0)
                    && name >= &low[..]
                    && let Some(high) = s(1)
                    && name <= &high[..]
                {
                    return self.dest_in_tree(k, &kd, name, touched);
                }
            }
        }
        Obj::Null
    }
}

/// A valid destination's page.
enum DestPage {
    /// A page number (`findDest` gives 0 for it).
    Number,
    /// A page object.
    Page(Ref),
}

/// `LinkDest::LinkDest(Array)`: whether a destination is valid (its page
/// a number or a reference, its kind known, an `/XYZ`'s positions numbers
/// or null, enough of them for the kinds that need some); its page.
fn link_dest(doc: &Doc, a: &[Obj]) -> Option<DestPage> {
    if a.len() < 2 {
        return None;
    }
    let page = match &a[0] {
        Obj::Int(_) => DestPage::Number,
        Obj::Ref(r) => DestPage::Page(*r),
        _ => return None,
    };
    let Obj::Name(kind) = doc.follow(&a[1]) else {
        return None;
    };
    match &kind[..] {
        b"XYZ" => {
            for i in 2..5 {
                if let Some(o) = a.get(i)
                    && !matches!(doc.follow(o), Obj::Null | Obj::Int(_) | Obj::Real(_))
                {
                    return None;
                }
            }
        }
        b"Fit" | b"FitB" => {}
        b"FitH" | b"FitV" | b"FitBH" | b"FitBV" if a.len() >= 3 => {}
        b"FitR" if a.len() >= 6 => {}
        _ => return None,
    }
    Some(page)
}

/// What a page tree node passes down, and what a page has
/// (`PageAttrs`).
#[derive(Clone)]
struct Attrs {
    media: [f64; 4],
    crop: [f64; 4],
    have_crop: bool,
    bleed: [f64; 4],
    trim: [f64; 4],
    art: [f64; 4],
    rotate: i32,
    resources: Option<Dict>,
}

impl Attrs {
    /// `PageAttrs::PageAttrs(attrs, dict)`: the media box, crop box and
    /// rotation inherited unless `d` has its own (the crop box the media
    /// box if no node had one), the other boxes `d`'s or the crop box;
    /// the resources inherited, `d`'s own, or both merged.
    fn new(up: Option<&Attrs>, doc: &Doc, d: &Dict) -> Attrs {
        let (mut media, mut crop, mut have_crop, mut rotate) = match up {
            Some(a) => (a.media, a.crop, a.have_crop, a.rotate),
            None => ([0.0, 0.0, 612.0, 792.0], [0.0; 4], false, 0),
        };
        if let Some(b) = read_box(doc, d, b"MediaBox") {
            media = b;
        }
        if let Some(b) = read_box(doc, d, b"CropBox") {
            crop = b;
            have_crop = true;
        }
        if !have_crop {
            crop = media;
        }
        let other = |k: &[u8]| read_box(doc, d, k).unwrap_or(crop);
        let (bleed, trim, art) = (other(b"BleedBox"), other(b"TrimBox"), other(b"ArtBox"));
        if let Obj::Int(r) = doc.lookup(d, b"Rotate") {
            rotate = r;
        }
        rotate = rotate.rem_euclid(360);
        let resources = match (up.and_then(|a| a.resources.as_ref()), doc.lookup(d, b"Resources")) {
            (Some(p), Obj::Dict(c)) => Some(merge(doc, p, &c)),
            (Some(p), _) => Some(p.clone()),
            (None, Obj::Dict(c)) => Some(c),
            (None, _) => None,
        };
        Attrs {
            media,
            crop,
            have_crop,
            bleed,
            trim,
            art,
            rotate,
            resources,
        }
    }

    /// The page (`Page::Page`): its boxes clipped to the media box
    /// (`clipBoxes`).
    fn page(self, r: Ref) -> Page {
        let m = self.media;
        Page {
            dict_ref: r,
            media: m,
            crop: clip(self.crop, m),
            bleed: clip(self.bleed, m),
            trim: clip(self.trim, m),
            art: clip(self.art, m),
            rotate: self.rotate,
            resources: self.resources,
        }
    }
}

/// The resources of a node and of its parent, merged as xpdf merges them
/// ("some PDF files violate the PDF spec and expect this merging"): each
/// of the parent's categories that is a dictionary, copied; then each of
/// the node's that is one, its entries added to the parent's category of
/// its name (replacing those of the same name) or the category added. A
/// category that is not a dictionary (`/ProcSet`) is dropped.
fn merge(doc: &Doc, parent: &Dict, child: &Dict) -> Dict {
    let mut res = Dict::default();
    for (k, v) in &parent.0 {
        if let Obj::Dict(sub) = doc.follow(v) {
            let mut copy = Dict::default();
            for (k2, v2) in sub.0 {
                copy.insert(k2, v2);
            }
            res.insert(k.clone(), Obj::Dict(copy));
        }
    }
    for (k, v) in &child.0 {
        if let Obj::Dict(sub) = doc.follow(v) {
            match res.get_mut(k) {
                Some(Obj::Dict(into)) => {
                    for (k2, v2) in sub.0 {
                        into.insert(k2, v2);
                    }
                }
                _ => res.insert(k.clone(), Obj::Dict(sub)),
            }
        }
    }
    res
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

/// `PageAttrs::readBox`: four numbers, each kept within ±10⁹, each pair
/// in order.
fn read_box(doc: &Doc, d: &Dict, key: &[u8]) -> Option<[f64; 4]> {
    let Obj::Array(a) = doc.lookup(d, key) else {
        return None;
    };
    if a.len() != 4 {
        return None;
    }
    let mut b = [0.0; 4];
    for (i, o) in a.iter().enumerate() {
        b[i] = doc.follow(o).as_num()?.clamp(-1e9, 1e9);
    }
    if b[0] > b[2] {
        b.swap(0, 2);
    }
    if b[1] > b[3] {
        b.swap(1, 3);
    }
    Some(b)
}

/// `PDFDoc::checkHeader`: where `%PDF-` is in the first 1,024 bytes (the
/// file's start for its offsets), and the version after it (`atof` of the
/// first word: 0 if there is no header).
fn check_header(d: &[u8]) -> (usize, f64) {
    let head = &d[..d.len().min(1024)];
    let Some(i) = (0..1019).find(|&i| head.get(i..i + 5) == Some(b"%PDF-")) else {
        return (0, 0.0);
    };
    let word: Vec<u8> = head[i + 5..]
        .iter()
        .copied()
        .skip_while(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
        .take_while(|c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0))
        .collect();
    (i, atof(&word))
}

/// C's `atof` of a word: the longest number at its start (sign, digits,
/// a point, an exponent), 0 if none.
fn atof(w: &[u8]) -> f64 {
    let mut end = 0;
    let mut best = 0;
    if matches!(w.first(), Some(b'+' | b'-')) {
        end = 1;
    }
    let digits = |from: usize| w[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let int = digits(end);
    end += int;
    let mut frac = 0;
    if w.get(end) == Some(&b'.') {
        frac = digits(end + 1);
        if int > 0 || frac > 0 {
            end += 1 + frac;
        }
    }
    if int + frac > 0 {
        best = end;
        if matches!(w.get(end), Some(b'e' | b'E')) {
            let mut e = end + 1;
            if matches!(w.get(e), Some(b'+' | b'-')) {
                e += 1;
            }
            let n = digits(e);
            if n > 0 {
                best = e + n;
            }
        }
    }
    core::str::from_utf8(&w[..best])
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

/// A predictor of `/DecodeParms` (`StreamPredictor`): rows of
/// `columns × colors × bits`, each with PNG's filter byte before it
/// (`predictor` 10 or more), or TIFF's (2), or none (any other; a last
/// partial row is then the previous row's ending, as xpdf's buffer is).
#[derive(Clone, Copy)]
struct Pred {
    predictor: i32,
    colors: i32,
    bits: i32,
    columns: i32,
}

impl Pred {
    /// The data predicted; as it is if the parameters are not valid (as
    /// xpdf drops the predictor) or there is none (1).
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "byte arithmetic as C's"
    )]
    fn apply(self, d: &[u8]) -> Vec<u8> {
        let (width, comps, bits) = (self.columns, self.colors, self.bits);
        if self.predictor == 1
            || width <= 0
            || comps <= 0
            || bits <= 0
            || comps > 32
            || bits > 16
            || width >= i32::MAX / comps
            || width * comps >= (i32::MAX - 7) / bits
        {
            return d.to_vec();
        }
        let n_vals = (width * comps) as usize;
        let (comps, bits) = (comps as usize, bits as usize);
        let pix = (comps * bits).div_ceil(8);
        let row = (n_vals * bits).div_ceil(8) + pix;
        let mut line = alloc::vec![0u8; row];
        let mut out = Vec::with_capacity(d.len());
        let mut k = 0;
        loop {
            let cur = if self.predictor >= 10 {
                let Some(&b) = d.get(k) else {
                    break;
                };
                k += 1;
                i32::from(b) + 10
            } else {
                self.predictor
            };
            let mut up_left = [0u8; 65];
            let mut ended = false;
            for i in pix..row {
                for j in (1..=pix).rev() {
                    up_left[j] = up_left[j - 1];
                }
                up_left[0] = line[i];
                let Some(&c) = d.get(k) else {
                    if i > pix {
                        // (a truncated last row, read as Adobe does)
                        ended = true;
                        break;
                    }
                    return out;
                };
                k += 1;
                line[i] = match cur {
                    11 => line[i - pix].wrapping_add(c),
                    12 => line[i].wrapping_add(c),
                    13 => line[i - pix].midpoint(line[i]).wrapping_add(c),
                    14 => {
                        let (left, up, ul) = (
                            i32::from(line[i - pix]),
                            i32::from(line[i]),
                            i32::from(up_left[pix]),
                        );
                        let p = left + up - ul;
                        let (pa, pb, pc) = ((p - left).abs(), (p - up).abs(), (p - ul).abs());
                        let base = if pa <= pb && pa <= pc {
                            left
                        } else if pb <= pc {
                            up
                        } else {
                            ul
                        };
                        (base as u8).wrapping_add(c)
                    }
                    _ => c,
                };
            }
            if self.predictor == 2 {
                tiff(&mut line, pix, row, comps, bits, n_vals / comps);
            }
            out.extend_from_slice(&line[pix..]);
            if ended {
                break;
            }
        }
        out
    }
}

/// TIFF's predictor on a row (`StreamPredictor::getNextLine`'s second
/// half): each component the sum of it and the one before.
#[allow(clippy::cast_possible_truncation, reason = "byte arithmetic as C's")]
fn tiff(line: &mut [u8], pix: usize, row: usize, comps: usize, bits: usize, width: usize) {
    if bits == 8 {
        for i in pix..row {
            line[i] = line[i].wrapping_add(line[i - comps]);
        }
    } else if bits == 16 {
        let mut i = pix;
        while i + 1 < row {
            let c = ((usize::from(line[i]) + usize::from(line[i - 2 * comps])) << 8)
                + usize::from(line[i + 1])
                + usize::from(line[i + 1 - 2 * comps]);
            line[i] = (c >> 8) as u8;
            line[i + 1] = (c & 0xff) as u8;
            i += 2;
        }
    } else {
        let mut acc = [0u8; 32];
        let mask = (1u64 << bits) - 1;
        let (mut in_buf, mut out_buf) = (0u64, 0u64);
        let (mut in_bits, mut out_bits) = (0usize, 0usize);
        let (mut j, mut k) = (pix, pix);
        for _ in 0..width {
            for a in acc.iter_mut().take(comps) {
                if in_bits < bits {
                    in_buf = in_buf << 8 | u64::from(line[j]);
                    j += 1;
                    in_bits += 8;
                }
                *a = ((u64::from(*a) + (in_buf >> (in_bits - bits))) & mask) as u8;
                in_bits -= bits;
                out_buf = out_buf << bits | u64::from(*a);
                out_bits += bits;
                if out_bits >= 8 {
                    line[k] = (out_buf >> (out_bits - 8)) as u8;
                    k += 1;
                    out_bits -= 8;
                }
            }
        }
        if out_bits > 0 {
            line[k] = ((out_buf << (8 - out_bits)) + (in_buf & ((1 << (8 - out_bits)) - 1))) as u8;
        }
    }
}

/// `ASCIIHexStream`: pairs of hex digits, C's whitespace skipped, `>` the
/// end (a lone digit's pair 0); a byte not hex is 0; at the data's end a
/// last 0 byte.
fn ascii_hex(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut k = 0;
    let mut get = || -> Option<u8> {
        let c = d.get(k).copied();
        k += 1;
        c
    };
    loop {
        let mut c1 = get();
        while c1.is_some_and(c_space) {
            c1 = get();
        }
        if c1 == Some(b'>') {
            return out;
        }
        let mut c2 = get();
        while c2.is_some_and(c_space) {
            c2 = get();
        }
        let mut eof = false;
        if c2 == Some(b'>') {
            eof = true;
            c2 = Some(b'0');
        }
        let mut x = match c1 {
            None => {
                eof = true;
                0
            }
            c => hex_value(c).unwrap_or(0) << 4,
        };
        match c2 {
            None => {
                eof = true;
                x = 0;
            }
            c => x = x.wrapping_add(hex_value(c).unwrap_or(0)),
        }
        out.push(x);
        if eof {
            return out;
        }
    }
}

/// `ASCII85Stream`: groups of five (PDF whitespace skipped, `z` four
/// zeros at a group's start), any byte its value less 33, `~` or the end
/// ending it, a partial group padded with `u`.
fn ascii85(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut k = 0;
    let mut get = || -> Option<u8> {
        loop {
            let c = d.get(k).copied();
            k += 1;
            if !is_space(c) {
                return c;
            }
        }
    };
    loop {
        let c0 = get();
        match c0 {
            None | Some(b'~') => return out,
            Some(b'z') => out.extend_from_slice(&[0; 4]),
            Some(c0) => {
                let mut c = [i64::from(c0); 5];
                let mut n = 4;
                let mut eof = false;
                for (i, slot) in c.iter_mut().enumerate().skip(1) {
                    match get() {
                        None | Some(b'~') => {
                            n = i - 1;
                            eof = true;
                            break;
                        }
                        Some(b) => *slot = i64::from(b),
                    }
                }
                if eof {
                    for slot in c.iter_mut().skip(n + 2) {
                        *slot = 0x21 + 84;
                    }
                    c[n + 1] = 0x21 + 84;
                }
                let t = c
                    .iter()
                    .fold(0u64, |t, &v| t.wrapping_mul(85).wrapping_add((v - 0x21).cast_unsigned()));
                out.extend_from_slice(&t.to_be_bytes()[4..][..n]);
                if eof {
                    return out;
                }
            }
        }
    }
}

/// `RunLengthStream`: a length byte (128 the end), then that many bytes
/// plus one, or one byte repeated 257 less it times; a byte past the
/// data's end is 255 (C's `(char)EOF`).
fn run_length(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    let byte = |i: usize| d.get(i).copied().unwrap_or(0xff);
    while let Some(&l) = d.get(i) {
        i += 1;
        match l {
            128 => break,
            0..=127 => {
                for _ in 0..=l {
                    out.push(byte(i));
                    i += 1;
                }
            }
            _ => {
                let b = byte(i);
                i += 1;
                out.extend(core::iter::repeat_n(b, 257 - usize::from(l)));
            }
        }
    }
    out
}

/// `LZWStream`: codes of 9 to 12 bits (`early` making them longer one
/// code sooner), 256 clearing the table, 257 the end; the table cleared
/// past 4,096 entries.
fn lzw(d: &[u8], early: i32) -> Vec<u8> {
    let mut out = Vec::new();
    // (each entry: its length, the code before it, its last byte)
    let mut table = alloc::vec![(0usize, 0usize, 0u8); 4097];
    let (mut next_code, mut next_bits) = (258usize, 9u32);
    let (mut buf, mut cnt, mut pos) = (0u32, 0u32, 0usize);
    let mut first = true;
    let mut seq: Vec<u8> = Vec::new();
    let (mut prev_code, mut new_char) = (0usize, 0u8);
    loop {
        while cnt < next_bits {
            let Some(&b) = d.get(pos) else {
                return out;
            };
            pos += 1;
            buf = buf << 8 | u32::from(b);
            cnt += 8;
        }
        let code = ((buf >> (cnt - next_bits)) & ((1 << next_bits) - 1)) as usize;
        cnt -= next_bits;
        match code {
            257 => return out,
            256 => {
                (next_code, next_bits, first) = (258, 9, true);
                seq.clear();
                continue;
            }
            _ => {}
        }
        if next_code >= 4097 {
            // ("Bad LZW stream - expected clear-table code")
            (next_code, next_bits, first) = (258, 9, true);
            seq.clear();
        }
        let next_len = seq.len() + 1;
        if code < 256 {
            seq.clear();
            seq.push(u8::try_from(code).unwrap_or(0));
        } else if code < next_code {
            let len = table[code].0;
            seq.clear();
            seq.resize(len, 0);
            let mut j = code;
            for i in (1..len).rev() {
                seq[i] = table[j].2;
                j = table[j].1;
            }
            seq[0] = u8::try_from(j).unwrap_or(0);
        } else if code == next_code {
            seq.push(new_char);
        } else {
            // ("Bad LZW stream - unexpected code")
            return out;
        }
        new_char = seq[0];
        if first {
            first = false;
        } else {
            table[next_code] = (next_len, prev_code, new_char);
            next_code += 1;
            let n = i64::try_from(next_code).unwrap_or(0) + i64::from(early);
            match n {
                512 => next_bits = 10,
                1024 => next_bits = 11,
                2048 => next_bits = 12,
                _ => {}
            }
        }
        prev_code = code;
        out.extend_from_slice(&seq);
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
            b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 100] /Resources << /ProcSet [/PDF] /Font << /F1 7 0 R /F2 7 0 R >> >> >>",
        );
        obj(
            &mut f,
            b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Group << /S /Transparency >> >>",
        );
        obj(
            &mut f,
            b"<< /Type /Page /Parent 2 0 R /Rotate -90 /CropBox [10 300 -5 20] /Contents [5 0 R 5 0 R] /Resources << /Font << /F2 8 0 R /F3 8 0 R >> /XObject << >> >> >>",
        );
        obj(
            &mut f,
            b"<< /Length 6 0 R >>\nstream\n0 0 m 10 10 l S\nendstream",
        );
        obj(&mut f, b"15");
        obj(&mut f, b"<< /Type /Font /A 1 /B 2 /A 3 >>");
        obj(&mut f, b"<< /Type /Font >>");
        let x = f.len();
        f.extend_from_slice(b"xref\n0 9\n0000000000 65535 f \n");
        for o in &offs {
            f.extend_from_slice(alloc::format!("{o:010} 00000 n \n").as_bytes());
        }
        f.extend_from_slice(
            alloc::format!("trailer\n<< /Size 9 /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n")
                .as_bytes(),
        );
        f
    }

    #[test]
    fn reads_a_classic_file() {
        let doc = Doc::open(&Arc::from(classic())).unwrap();
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
        assert_eq!(doc.page(3), None);
    }

    #[test]
    fn merges_resources_and_keeps_a_key_once() {
        let doc = Doc::open(&Arc::from(classic())).unwrap();
        let r = doc.page(2).unwrap().resources.unwrap();
        // (the parent's /ProcSet dropped; /Font merged, the page's /F2
        // replacing the parent's in its place; the page's /XObject added)
        let keys: Vec<&[u8]> = r.0.iter().map(|(k, _)| &k[..]).collect();
        assert_eq!(keys, [&b"Font"[..], b"XObject"]);
        let Some(Obj::Dict(f)) = r.get(b"Font") else {
            panic!()
        };
        let fonts: Vec<(&[u8], &Obj)> = f.0.iter().map(|(k, v)| (&k[..], v)).collect();
        let r7 = Obj::Ref(Ref {
            num: 7,
            generation: 0,
        });
        let r8 = Obj::Ref(Ref {
            num: 8,
            generation: 0,
        });
        assert_eq!(fonts, [(&b"F1"[..], &r7), (b"F2", &r8), (b"F3", &r8)]);
        let Obj::Dict(d) = doc.fetch(Ref {
            num: 7,
            generation: 0,
        }) else {
            panic!()
        };
        let keys: Vec<&[u8]> = d.0.iter().map(|(k, _)| &k[..]).collect();
        assert_eq!(keys, [&b"Type"[..], b"A", b"B"]);
        assert_eq!(d.get(b"A"), Some(&Obj::Int(3)));
    }

    #[test]
    fn a_damaged_table_is_rebuilt_by_scanning() {
        let mut f = classic();
        // (`startxref` pointing nowhere)
        let at = f.windows(9).rposition(|w| w == b"startxref").unwrap();
        f.truncate(at);
        f.extend_from_slice(b"startxref\n3\n%%EOF\n");
        let doc = Doc::open(&Arc::from(f)).unwrap();
        assert_eq!(doc.num_pages(), 2);
        // (a repaired file's stream ends at its `endstream`'s line)
        let p1 = doc.page(1).unwrap();
        let Obj::Dict(d) = doc.fetch(p1.dict_ref) else {
            panic!()
        };
        let Obj::Stream(s) = doc.lookup(&d, b"Contents") else {
            panic!()
        };
        assert_eq!(doc.raw(&s), b"0 0 m 10 10 l S\n");
    }

    #[test]
    fn lexes_as_xpdf() {
        let d = b"[-.5 3 -0 50-100 1.25 (a\\(b\\)\\101\r\nc) <4A4> /A#42c --1.5 --7 \
                   <4G 1> /x#4 { /a#00 1.5-2 2147483648]";
        let mut p = Parser::new(d, 0, None, false);
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
        // (a double minus makes an integer 0)
        assert_eq!(a[9], Obj::Int(0));
        // (a byte not hex is a 0 digit)
        assert_eq!(a[10], Obj::Str(alloc::vec![0x40, 0x10]));
        // (one digit after `#`: its value)
        assert_eq!(a[11], Obj::Name(alloc::vec![b'x', 4]));
        assert_eq!(a[12], Obj::Error);
        assert_eq!(a[13], Obj::Error);
        // (a minus in a fraction skipped: 1.52)
        assert!(matches!(a[14], Obj::Real(r) if (r - 1.52).abs() < 1e-12));
        // (an integer's digits wrap)
        assert_eq!(a[15], Obj::Int(i32::MIN));
    }

    #[test]
    fn decodes_as_xpdf() {
        assert_eq!(ascii_hex(b"48 65 6C6c6F>"), b"Hello");
        // (no `>`: a last 0 byte)
        assert_eq!(ascii_hex(b"4865"), b"He\0");
        assert_eq!(ascii_hex(b"486>"), b"H`");
        assert_eq!(ascii85(b"87cURD]i,\"Ebo7~>"), b"Hello World");
        assert_eq!(ascii85(b"z!!~>"), [0, 0, 0, 0, 0]);
        assert_eq!(run_length(&[2, b'a', b'b', b'c', 254, b'x', 128]), b"abcxxx");
        assert_eq!(run_length(&[3, b'a']), [b'a', 255, 255, 255]);
        // (PNG's Up on rows of 2: [1,2] then [1+3, 2+4])
        let p = Pred {
            predictor: 12,
            colors: 1,
            bits: 8,
            columns: 2,
        };
        assert_eq!(p.apply(&[2, 1, 2, 2, 3, 4]), [1, 2, 4, 6]);
        // (a cross-reference stream's rows: /Columns 3 /Predictor 12)
        let x = Pred {
            columns: 3,
            ..p
        };
        assert_eq!(x.apply(&[2, 1, 0, 16, 2, 0, 1, 5]), [1, 0, 16, 1, 1, 21]);
        // (TIFF's on 8 bits, 2 colors)
        let t = Pred {
            predictor: 2,
            colors: 2,
            bits: 8,
            columns: 2,
        };
        assert_eq!(t.apply(&[1, 2, 3, 4]), [1, 2, 4, 6]);
        // (the PDF Reference's example of `LZWDecode`)
        let z = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(lzw(&z, 1), b"-----A---B");
    }

    #[test]
    fn reads_the_header_version_as_atof() {
        assert_eq!(check_header(b"%PDF-1.5\n%\xe2"), (0, 1.5));
        assert_eq!(check_header(b"junk%PDF- 1.7 x"), (4, 1.7));
        assert_eq!(check_header(b"nothing"), (0, 0.0));
    }
}
