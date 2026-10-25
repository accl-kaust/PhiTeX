//! The file a watch writes while a document is edited (`phitex_pdf::Updater`:
//! an exact base and appended update sections) read back by a small
//! reader of its own: cross-reference tables and streams chained by
//! `/Prev`, object streams, plain objects. Every page resolves through
//! the chain to what the exact file of the same layout has: its content
//! stream's bytes, decoded, and the rest of the page as a value (its
//! resources, its fonts, their files), objects followed by number.
//!
//! - [`sections_resolve_as_the_exact_file`] and its table variant: a
//!   document of pages written as the link writes them (a chunk per
//!   page, the chunks a later layout shares with an earlier one the same
//!   allocation), edited in turn: a word, a longer word, a new font, a
//!   page inserted, one removed; after each edit the section appended,
//!   the file read back.
//! - [`harness_pairs`]: the pairs a watch session left
//!   (`PHITEX_UPDATE_PAIRS=<dir>`: `<k>.update.pdf`, the file after edit
//!   `k`, and `<k>.exact.pdf`, the file after the idle write that
//!   followed it; `scripts/accl/gate-update.py`), each read back the same
//!   way; every update file the one before it with bytes appended.

#![allow(
    clippy::unreadable_literal,
    clippy::many_single_char_names,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::type_complexity,
    clippy::assigning_clones
)]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use phitex_pdf::map::{MapEntry, parse_map};
use phitex_pdf::{FontSource, Layout, PdfBytes, PdfParams, PdfState, Section, Updater};
use phitex_tfm::{ScaledFont, Tfm};
use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::{CharInfo, FontId, Fonts, Left, LigKern};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{BoxNode, KernKind, Node};
use phitex_typeset::pack::{NATURAL, PackParams, Spec, hpack, vpackage};

// ------------------------------------------------------------- reader

/// A PDF value.
#[derive(Clone, Debug, PartialEq)]
enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    Real(String),
    Name(Vec<u8>),
    Str(Vec<u8>),
    Array(Vec<Obj>),
    Dict(BTreeMap<Vec<u8>, Obj>),
    Ref(usize),
    Stream(BTreeMap<Vec<u8>, Obj>, Vec<u8>),
}

impl Obj {
    fn dict(&self) -> Option<&BTreeMap<Vec<u8>, Obj>> {
        match self {
            Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
            _ => None,
        }
    }
    fn get(&self, k: &str) -> Option<&Obj> {
        self.dict()?.get(k.as_bytes())
    }
    fn int(&self) -> Option<i64> {
        match self {
            Obj::Int(i) => Some(*i),
            _ => None,
        }
    }
}

/// A parser over a file's bytes.
struct Lexer<'a> {
    b: &'a [u8],
    i: usize,
}

fn is_white(c: u8) -> bool {
    matches!(c, b' ' | b'\n' | b'\r' | b'\t' | b'\x0c' | 0)
}

fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

impl Lexer<'_> {
    fn skip(&mut self) {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if is_white(c) {
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

    /// A regular token (a number, a keyword).
    fn word(&mut self) -> &[u8] {
        self.skip();
        let a = self.i;
        while self.i < self.b.len() && !is_white(self.b[self.i]) && !is_delim(self.b[self.i]) {
            self.i += 1;
        }
        &self.b[a..self.i]
    }

    fn peek_word(&mut self) -> Vec<u8> {
        let i = self.i;
        let w = self.word().to_vec();
        self.i = i;
        w
    }

    fn expect(&mut self, w: &[u8]) {
        let got = self.word().to_vec();
        assert_eq!(
            String::from_utf8_lossy(&got),
            String::from_utf8_lossy(w),
            "at byte {}",
            self.i
        );
    }

    fn value(&mut self) -> Obj {
        self.skip();
        let c = self.b[self.i];
        match c {
            b'/' => {
                self.i += 1;
                let a = self.i;
                while self.i < self.b.len()
                    && !is_white(self.b[self.i])
                    && !is_delim(self.b[self.i])
                {
                    self.i += 1;
                }
                Obj::Name(self.b[a..self.i].to_vec())
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    self.skip();
                    if self.b[self.i] == b']' {
                        self.i += 1;
                        return Obj::Array(v);
                    }
                    v.push(self.value());
                }
            }
            b'<' if self.b.get(self.i + 1) == Some(&b'<') => {
                self.i += 2;
                let mut d = BTreeMap::new();
                loop {
                    self.skip();
                    if self.b[self.i] == b'>' {
                        self.i += 2;
                        break;
                    }
                    let Obj::Name(k) = self.value() else {
                        panic!("a dictionary key not a name at byte {}", self.i)
                    };
                    let v = self.value();
                    d.insert(k, v);
                }
                // a stream after it
                let save = self.i;
                if self.word() == b"stream" {
                    if self.b[self.i] == b'\r' {
                        self.i += 1;
                    }
                    assert_eq!(self.b[self.i], b'\n', "stream not followed by EOL");
                    self.i += 1;
                    let n = d
                        .get(&b"Length"[..])
                        .and_then(Obj::int)
                        .expect("a stream's /Length, direct") as usize;
                    let data = self.b[self.i..self.i + n].to_vec();
                    self.i += n;
                    self.expect(b"endstream");
                    return Obj::Stream(d, data);
                }
                self.i = save;
                Obj::Dict(d)
            }
            b'<' => {
                self.i += 1;
                let mut hex = Vec::new();
                while self.b[self.i] != b'>' {
                    if !is_white(self.b[self.i]) {
                        hex.push(self.b[self.i]);
                    }
                    self.i += 1;
                }
                self.i += 1;
                if hex.len() % 2 == 1 {
                    hex.push(b'0');
                }
                let v = hex
                    .chunks(2)
                    .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                    .collect();
                Obj::Str(v)
            }
            b'(' => {
                self.i += 1;
                let mut depth = 1;
                let mut s = Vec::new();
                loop {
                    let c = self.b[self.i];
                    self.i += 1;
                    match c {
                        b'\\' => {
                            let e = self.b[self.i];
                            self.i += 1;
                            match e {
                                b'n' => s.push(b'\n'),
                                b'r' => s.push(b'\r'),
                                b't' => s.push(b'\t'),
                                b'b' => s.push(8),
                                b'f' => s.push(12),
                                b'0'..=b'7' => {
                                    let mut v = u32::from(e - b'0');
                                    for _ in 0..2 {
                                        match self.b[self.i] {
                                            d @ b'0'..=b'7' => {
                                                v = v * 8 + u32::from(d - b'0');
                                                self.i += 1;
                                            }
                                            _ => break,
                                        }
                                    }
                                    s.push((v & 0xff) as u8);
                                }
                                b'\n' => {}
                                b'\r' => {
                                    if self.b[self.i] == b'\n' {
                                        self.i += 1;
                                    }
                                }
                                x => s.push(x),
                            }
                        }
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
                        _ => s.push(c),
                    }
                }
            }
            _ => {
                let w = self.word().to_vec();
                match &w[..] {
                    b"null" => Obj::Null,
                    b"true" => Obj::Bool(true),
                    b"false" => Obj::Bool(false),
                    _ if w.contains(&b'.') => Obj::Real(String::from_utf8(w).unwrap()),
                    _ => {
                        let n: i64 = std::str::from_utf8(&w)
                            .ok()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or_else(|| {
                                panic!(
                                    "not a value: {:?} at byte {}",
                                    String::from_utf8_lossy(&w),
                                    self.i
                                )
                            });
                        // `n g R`
                        let save = self.i;
                        let g = self.word().to_vec();
                        if !g.is_empty() && g.iter().all(u8::is_ascii_digit) && self.word() == b"R"
                        {
                            return Obj::Ref(usize::try_from(n).unwrap());
                        }
                        self.i = save;
                        Obj::Int(n)
                    }
                }
            }
        }
    }
}

/// Where an object is, as the newest cross-reference section says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Free,
    At(usize),
    In(usize, usize),
}

/// A file read back.
struct Reader<'a> {
    b: &'a [u8],
    xref: HashMap<usize, Entry>,
    trailer: Obj,
    /// The cross-reference sections met, newest first.
    sections: usize,
    /// The objects' digests ([`Reader::digest`]; `None`: being made).
    digests: RefCell<HashMap<usize, Option<u64>>>,
    /// The object streams decoded: their data, `/First` and the
    /// objects' numbers and offsets.
    streams: RefCell<HashMap<usize, Rc<(Vec<u8>, usize, Vec<(usize, usize)>)>>>,
}

fn decoded(d: &BTreeMap<Vec<u8>, Obj>, data: &[u8]) -> Vec<u8> {
    match d.get(&b"Filter"[..]) {
        None => data.to_vec(),
        Some(Obj::Name(n)) if n == b"FlateDecode" => {
            phitex_codecs::inflate::inflate(data).expect("a zlib stream")
        }
        Some(f) => panic!("filter {f:?}"),
    }
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Reader<'a> {
        let tail = &b[b.len().saturating_sub(64)..];
        let k = tail
            .windows(9)
            .rposition(|w| w == b"startxref")
            .expect("startxref");
        let mut lx = Lexer { b: tail, i: k + 9 };
        let mut at: usize = std::str::from_utf8(lx.word()).unwrap().parse().unwrap();
        let mut xref = HashMap::new();
        let mut trailer = None;
        let mut sections = 0;
        loop {
            sections += 1;
            let mut lx = Lexer { b, i: at };
            let t = if lx.peek_word() == b"xref" {
                lx.word();
                loop {
                    let w = lx.peek_word();
                    if w == b"trailer" {
                        lx.word();
                        break;
                    }
                    let a: usize = std::str::from_utf8(lx.word()).unwrap().parse().unwrap();
                    let n: usize = std::str::from_utf8(lx.word()).unwrap().parse().unwrap();
                    for k in a..a + n {
                        let off: usize = std::str::from_utf8(lx.word()).unwrap().parse().unwrap();
                        let _gen = lx.word().to_vec();
                        let kind = lx.word().to_vec();
                        let e = if kind == b"n" {
                            Entry::At(off)
                        } else {
                            Entry::Free
                        };
                        xref.entry(k).or_insert(e);
                    }
                }
                lx.value()
            } else {
                lx.word();
                lx.word();
                lx.expect(b"obj");
                let s = lx.value();
                let Obj::Stream(d, data) = &s else {
                    panic!("no cross-reference at {at}")
                };
                assert_eq!(d.get(&b"Type"[..]), Some(&Obj::Name(b"XRef".to_vec())));
                let rows = decoded(d, data);
                let w: Vec<usize> = match d.get(&b"W"[..]) {
                    Some(Obj::Array(a)) => a.iter().map(|x| x.int().unwrap() as usize).collect(),
                    _ => panic!("/W"),
                };
                let size = d.get(&b"Size"[..]).and_then(Obj::int).unwrap() as usize;
                let index: Vec<usize> = match d.get(&b"Index"[..]) {
                    Some(Obj::Array(a)) => a.iter().map(|x| x.int().unwrap() as usize).collect(),
                    _ => vec![0, size],
                };
                let field = |r: &[u8], a: usize, n: usize, def: usize| {
                    if n == 0 {
                        def
                    } else {
                        r[a..a + n]
                            .iter()
                            .fold(0usize, |v, &c| v << 8 | usize::from(c))
                    }
                };
                let rl: usize = w.iter().sum();
                let mut k = 0;
                for p in index.chunks(2) {
                    for num in p[0]..p[0] + p[1] {
                        let r = &rows[k * rl..(k + 1) * rl];
                        k += 1;
                        let t = field(r, 0, w[0], 1);
                        let f2 = field(r, w[0], w[1], 0);
                        let f3 = field(r, w[0] + w[1], w[2], 0);
                        let e = match t {
                            0 => Entry::Free,
                            1 => Entry::At(f2),
                            2 => Entry::In(f2, f3),
                            _ => panic!("type {t}"),
                        };
                        xref.entry(num).or_insert(e);
                    }
                }
                assert_eq!(k * rl, rows.len(), "the rows /Index names");
                Obj::Dict(d.clone())
            };
            let prev = t.get("Prev").and_then(Obj::int);
            if trailer.is_none() {
                trailer = Some(t);
            }
            match prev {
                Some(p) => at = usize::try_from(p).unwrap(),
                None => break,
            }
        }
        Reader {
            b,
            xref,
            trailer: trailer.unwrap(),
            sections,
            digests: RefCell::default(),
            streams: RefCell::default(),
        }
    }

    fn object(&self, n: usize) -> Obj {
        match self.xref.get(&n).copied().unwrap_or(Entry::Free) {
            Entry::Free => Obj::Null,
            Entry::At(off) => {
                let mut lx = Lexer { b: self.b, i: off };
                let got: usize = std::str::from_utf8(lx.word()).unwrap().parse().unwrap();
                assert_eq!(got, n, "object {n}'s entry at {off} finds object {got}");
                lx.word();
                lx.expect(b"obj");
                lx.value()
            }
            Entry::In(s, idx) => {
                let st = self.stream(s);
                let (data, first, pairs) = &*st;
                assert!(idx < pairs.len(), "object {n}: index {idx} of {} in stream {s}", pairs.len());
                assert_eq!(pairs[idx].0, n, "stream {s}'s object {idx} is not {n}");
                let mut lx = Lexer {
                    b: data,
                    i: first + pairs[idx].1,
                };
                lx.value()
            }
        }
    }

    /// Object stream `s` decoded, its header read.
    fn stream(&self, s: usize) -> Rc<(Vec<u8>, usize, Vec<(usize, usize)>)> {
        if let Some(x) = self.streams.borrow().get(&s) {
            return x.clone();
        }
        let Obj::Stream(d, data) = self.object(s) else {
            panic!("object stream {s}")
        };
        let data = decoded(&d, &data);
        let count = d.get(&b"N"[..]).and_then(Obj::int).unwrap() as usize;
        let first = d.get(&b"First"[..]).and_then(Obj::int).unwrap() as usize;
        let mut lx = Lexer { b: &data, i: 0 };
        let mut pairs = Vec::new();
        for _ in 0..count {
            let a: usize = std::str::from_utf8(lx.word()).unwrap().parse().unwrap();
            let o: usize = std::str::from_utf8(lx.word()).unwrap().parse().unwrap();
            pairs.push((a, o));
        }
        let x = Rc::new((data, first, pairs));
        self.streams.borrow_mut().insert(s, x.clone());
        x
    }

    /// A digest of `x` with every reference followed (but `/Parent`'s
    /// and a page's own, `/P`), streams decoded: what a page shows,
    /// whatever the objects' numbers; each object's made once.
    fn digest(&self, x: &Obj) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        self.feed(x, &mut h);
        h.finish()
    }

    fn feed(&self, x: &Obj, h: &mut std::hash::DefaultHasher) {
        match x {
            Obj::Ref(n) => {
                let known = self.digests.borrow().get(n).copied();
                let d = match known {
                    Some(Some(d)) => d,
                    // (a cycle: the object being digested)
                    Some(None) => 0x6379_636c_65,
                    None => {
                        self.digests.borrow_mut().insert(*n, None);
                        let d = self.digest(&self.object(*n));
                        self.digests.borrow_mut().insert(*n, Some(d));
                        d
                    }
                };
                0u8.hash(h);
                d.hash(h);
            }
            Obj::Array(a) => {
                1u8.hash(h);
                a.len().hash(h);
                for y in a {
                    self.feed(y, h);
                }
            }
            Obj::Dict(d) => {
                2u8.hash(h);
                self.feed_dict(d, h);
            }
            Obj::Stream(d, data) => {
                3u8.hash(h);
                self.feed_dict(d, h);
                decoded(d, data).hash(h);
            }
            other => {
                4u8.hash(h);
                format!("{other:?}").hash(h);
            }
        }
    }

    fn feed_dict(&self, d: &BTreeMap<Vec<u8>, Obj>, h: &mut std::hash::DefaultHasher) {
        for (k, v) in d {
            if matches!(&k[..], b"Parent" | b"P" | b"Length" | b"Filter" | b"DecodeParms") {
                continue;
            }
            k.hash(h);
            self.feed(v, h);
        }
    }

    /// The pages in order.
    fn pages(&self) -> Vec<Obj> {
        let root = self.object(match self.trailer.get("Root") {
            Some(Obj::Ref(n)) => *n,
            r => panic!("/Root {r:?}"),
        });
        let mut out = Vec::new();
        let mut stack = vec![root.get("Pages").cloned().expect("/Pages")];
        while let Some(x) = stack.pop() {
            let node = match &x {
                Obj::Ref(n) => self.object(*n),
                o => o.clone(),
            };
            match node.get("Type") {
                Some(Obj::Name(t)) if t == b"Pages" => {
                    let Some(Obj::Array(kids)) = node.get("Kids") else {
                        panic!("/Kids")
                    };
                    for k in kids.iter().rev() {
                        stack.push(k.clone());
                    }
                }
                Some(Obj::Name(t)) if t == b"Page" => out.push(node),
                t => panic!("a page tree node of type {t:?}"),
            }
        }
        out
    }

    /// A page's content stream's bytes, decoded (its streams', in turn).
    fn contents(&self, page: &Obj) -> Vec<u8> {
        let get = |x: &Obj| match x {
            Obj::Ref(n) => self.object(*n),
            o => o.clone(),
        };
        let one = |x: Obj| match x {
            Obj::Stream(d, data) => decoded(&d, &data),
            c => panic!("/Contents {c:?}"),
        };
        match page.get("Contents").map(get) {
            Some(Obj::Array(a)) => a.iter().flat_map(|s| one(get(s))).collect(),
            Some(x) => one(x),
            None => Vec::new(),
        }
    }
}

/// The update file `upd` reads as the exact file `exact`: the same pages
/// (their content streams' bytes, then all of each page), the same
/// catalog and `/Info`.
fn same_document(upd: &[u8], exact: &[u8], what: &str) {
    let (u, e) = (Reader::new(upd), Reader::new(exact));
    assert_eq!(
        e.sections, 1,
        "{what}: the exact file has one cross-reference section"
    );
    let (up, ep) = (u.pages(), e.pages());
    assert_eq!(up.len(), ep.len(), "{what}: the pages");
    for (k, (a, b)) in up.iter().zip(&ep).enumerate() {
        assert!(
            u.contents(a) == e.contents(b),
            "{what}: page {}'s content stream differs",
            k + 1
        );
        assert!(u.digest(a) == e.digest(b), "{what}: page {} differs", k + 1);
    }
    for key in ["Root", "Info"] {
        let a = u.trailer.get(key).map(|x| u.digest(x));
        let b = e.trailer.get(key).map(|x| e.digest(x));
        assert_eq!(a, b, "{what}: the trailer's /{key}");
    }
    assert_eq!(u.trailer.get("ID"), e.trailer.get("ID"), "{what}: /ID");
}

// ----------------------------------------------------- the documents

const PT: Scaled = 65536;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn tfm(name: &str) -> Tfm {
    let path = format!(
        "{}/../../fonts/tfm/tests/fixtures/{name}.tfm",
        env!("CARGO_MANIFEST_DIR")
    );
    phitex_tfm::parse(&std::fs::read(path).unwrap()).unwrap()
}

struct Font {
    name: &'static str,
    tfm: Tfm,
    scaled: ScaledFont,
}

/// cmr10 (font 1) and cmbx10 (font 2).
struct Src {
    fonts: Vec<Font>,
    map: BTreeMap<Vec<u8>, MapEntry>,
}

impl Src {
    fn new() -> Src {
        Src {
            fonts: ["cmr10", "cmbx10"]
                .iter()
                .map(|&name| {
                    let tfm = tfm(name);
                    let scaled = tfm.at(tfm.design_size).unwrap();
                    Font { name, tfm, scaled }
                })
                .collect(),
            map: parse_map(&fixture("cm.map")),
        }
    }

    fn get(&self, f: FontId) -> &Font {
        &self.fonts[f as usize - 1]
    }

    /// A line of words in font `f` (kerns, no ligatures: the words
    /// avoid them).
    fn line(&self, f: FontId, text: &str) -> Node {
        let font = self.get(f);
        let mut list = Vec::new();
        for (i, w) in text.split(' ').enumerate() {
            if i > 0 {
                let s = &font.scaled;
                list.push(Node::glue(Glue::new(
                    s.space(),
                    s.space_stretch(),
                    s.space_shrink(),
                )));
            }
            let w = w.as_bytes();
            for (k, &c) in w.iter().enumerate() {
                list.push(Node::Char {
                    font: f,
                    ch: u16::from(c),
                });
                if let Some(&n) = w.get(k + 1)
                    && let Some(step) = font.tfm.lookup(Some(c), Some(n))
                    && step.is_kern()
                {
                    list.push(Node::Kern(font.scaled.char_kern(step), KernKind::Normal));
                }
            }
        }
        Node::Box(Box::new(
            hpack(list, NATURAL, self, &PackParams::default(), false).node,
        ))
    }

    /// A page: its lines (`*` before a line: bold).
    fn page(&self, lines: &[String]) -> BoxNode {
        let mut list = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                list.push(Node::Kern(12 * PT, KernKind::Explicit));
            }
            match l.strip_prefix('*') {
                Some(b) => list.push(self.line(2, b)),
                None => list.push(self.line(1, l)),
            }
        }
        vpackage(list, Spec::Additional(0), 0, &PackParams::default()).node
    }
}

impl Fonts for Src {
    fn char_info(&self, font: FontId, ch: u16) -> Option<CharInfo> {
        let d = self.get(font).scaled.char(u8::try_from(ch).ok()?)?;
        Some(CharInfo {
            width: d.width,
            height: d.height,
            depth: d.depth,
            italic: d.italic,
            ..CharInfo::default()
        })
    }
    fn lig_kern(&self, _: FontId, _: Left, _: u16) -> Option<LigKern> {
        None
    }
    fn has_left_boundary(&self, _: FontId) -> bool {
        false
    }
    fn bchar(&self, _: FontId) -> Option<u16> {
        None
    }
    fn param(&self, font: FontId, n: u16) -> Scaled {
        self.get(font).scaled.param(usize::from(n))
    }
    fn param_count(&self, font: FontId) -> u16 {
        u16::try_from(self.get(font).scaled.params.len()).unwrap()
    }
}

impl FontSource for Src {
    fn metrics(&self, f: FontId) -> &ScaledFont {
        &self.get(f).scaled
    }
    fn name(&self, f: FontId) -> &[u8] {
        self.get(f).name.as_bytes()
    }
    fn map_entry(&self, f: FontId) -> Option<MapEntry> {
        self.map.get(self.name(f)).cloned()
    }
    fn font_file(&self, f: FontId) -> Option<Vec<u8>> {
        let m = self.map_entry(f)?;
        Some(fixture(std::str::from_utf8(m.ff_name.as_deref()?).ok()?))
    }
    fn encoding_file(&self, f: FontId) -> Option<Vec<u8>> {
        let m = self.map_entry(f)?;
        Some(fixture(std::str::from_utf8(m.encname.as_deref()?).ok()?))
    }
}

/// The layout of `pages` as a link makes it: a chunk (or more) per
/// page, then the tail; the chunks `last` has with the same bytes from
/// the start and from the end taken from it, the same allocations (a
/// link copies an unchanged record's chunks).
fn layout(
    src: &Src,
    streams: bool,
    pages: &[Vec<String>],
    last: Option<&[phitex_pdf::Chunk]>,
) -> (Layout, Vec<phitex_pdf::Chunk>) {
    let mut st = PdfState::new(PdfParams {
        compress_level: 9,
        obj_compress_level: if streams { 2 } else { 0 },
        minor_version: if streams { 5 } else { 4 },
        date: phitex_pdf::pdf_date(1),
        banner: "This is pdfTeX".to_owned(),
        output_file_name: "doc.pdf".to_owned(),
        ..PdfParams::default()
    });
    let mut chunks = Vec::new();
    for p in pages {
        st.ship_out(&src.page(p), src, &mut |_, _| None).unwrap();
        if let Some(c) = st.flush_chunk() {
            chunks.push(c);
        }
    }
    if let Some(last) = last {
        let pre = chunks
            .iter()
            .zip(last)
            .take_while(|(a, b)| a.bytes == b.bytes)
            .count();
        for k in 0..pre {
            chunks[k].bytes = last[k].bytes.clone();
        }
        let n = chunks.len();
        let suf = chunks
            .iter()
            .rev()
            .zip(last.iter().rev())
            .take(n - pre)
            .take_while(|(a, b)| a.bytes == b.bytes)
            .count();
        for k in 0..suf {
            chunks[n - 1 - k].bytes = last[last.len() - 1 - k].bytes.clone();
        }
    }
    let mut bytes = PdfBytes::new();
    for c in &chunks {
        bytes.push(c.clone());
    }
    let tail = st.tail().unwrap().unwrap();
    (Layout::new(Arc::new(tail), bytes), chunks)
}

fn text(k: usize) -> Vec<String> {
    vec![
        format!("page {} begins here", k + 1),
        "the quick brown fox jumps over the lazy dog".to_owned(),
        "and then some more words".to_owned(),
    ]
}

/// A document edited in turn, each layout's section appended to the
/// file and the file read back against the layout's exact file.
fn edits(streams: bool) {
    let src = Src::new();
    let mut doc: Vec<Vec<String>> = (0..130).map(text).collect();
    let (l, mut last) = layout(&src, streams, &doc, None);
    let exact = l.exact().unwrap();
    let mut file = exact.to_vec();
    let mut up = Updater::after_exact(l, &exact);
    let steps: Vec<(&str, Box<dyn Fn(&mut Vec<Vec<String>>)>)> = vec![
        (
            "a word",
            Box::new(|d| d[40][1] = "the quick green fox jumps over the lazy dog".to_owned()),
        ),
        (
            "a longer word",
            Box::new(|d| d[41][2] = "and then some more wordiness".to_owned()),
        ),
        ("the same text", Box::new(|_| {})),
        (
            "a bold line (a new font)",
            Box::new(|d| d[70][0] = "*bold here".to_owned()),
        ),
        ("a page inserted", Box::new(|d| d.insert(10, text(999)))),
        (
            "a page removed",
            Box::new(|d| {
                d.remove(100);
            }),
        ),
        (
            "a word on the last page",
            Box::new(|d| {
                let n = d.len() - 1;
                d[n][1] = "the quick brown fox jumps over the lazy cat".to_owned();
            }),
        ),
        (
            "the bold line gone",
            Box::new(|d| d[71][0] = "plain again".to_owned()),
        ),
    ];
    for (what, f) in steps {
        f(&mut doc);
        let (l, chunks) = layout(&src, streams, &doc, Some(&last));
        last = chunks;
        let before = file.len();
        match up.section(&l) {
            Section::Same => assert_eq!(what, "the same text", "{what}: no section"),
            Section::Append(b) => {
                assert_ne!(what, "the same text", "a section for no change");
                file.extend_from_slice(&b);
                if what == "a word" {
                    // (its page's content stream and the cross-reference
                    // section's few hundred bytes)
                    assert!(b.len() < 3000, "{what}: a section of {} bytes", b.len());
                }
            }
        }
        assert_eq!(up.len() as usize, file.len());
        assert!(file.len() >= before);
        same_document(&file, &l.exact().unwrap(), what);
    }
    // the exact file written over it: the state starts again from there
    let (l, _) = layout(&src, streams, &doc, Some(&last));
    let exact = l.exact().unwrap();
    let mut up = Updater::after_exact(l.clone(), &exact);
    let mut file = exact.to_vec();
    doc[5][1] = "the quick brown cat jumps over the lazy dog".to_owned();
    let (l2, _) = layout(&src, streams, &doc, Some(&last));
    if let Section::Append(b) = up.section(&l2) {
        file.extend_from_slice(&b);
    }
    same_document(&file, &l2.exact().unwrap(), "after the exact file");
}

#[test]
fn sections_resolve_as_the_exact_file() {
    edits(true);
}

#[test]
fn sections_resolve_as_the_exact_file_with_tables() {
    edits(false);
}

/// The files a watch session left (`PHITEX_UPDATE_PAIRS`): each update
/// file reads as the exact file written after it, and is the update file
/// before it (or the exact file before that) with bytes appended.
#[test]
fn harness_pairs() {
    let Some(dir) = std::env::var_os("PHITEX_UPDATE_PAIRS") else {
        eprintln!("harness_pairs: PHITEX_UPDATE_PAIRS not set, nothing to read");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let mut ks: Vec<usize> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let n = e.ok()?.file_name().into_string().ok()?;
            n.strip_suffix(".update.pdf")?.parse().ok()
        })
        .collect();
    ks.sort_unstable();
    assert!(!ks.is_empty(), "no pairs in {}", dir.display());
    let mut checked = 0;
    for k in ks {
        let upd = std::fs::read(dir.join(format!("{k}.update.pdf"))).unwrap();
        let exact = std::fs::read(dir.join(format!("{k}.exact.pdf"))).unwrap();
        // (what it was before: the exact file the last idle write left,
        // or the update before it)
        let before = format!("{k}.before.pdf");
        if let Ok(b) = std::fs::read(dir.join(&before)) {
            assert!(
                upd.len() >= b.len() && upd[..b.len()] == b[..],
                "edit {k}: the file is not {before} with bytes appended"
            );
            // (the sections' bytes, and of them the pages' content
            // streams': what an edit must write)
            let r = Reader::new(&upd);
            let mut contents = 0;
            for p in r.pages() {
                if let Some(Obj::Ref(n)) = p.get("Contents")
                    && let Some(&Entry::At(off)) = r.xref.get(n)
                    && off >= b.len()
                {
                    let end = upd[off..].windows(6).position(|w| w == b"endobj").map_or(0, |e| e + 7);
                    contents += end;
                }
            }
            eprintln!(
                "harness_pairs: edit {k}: {} bytes appended, {contents} of them pages' content streams, {} sections",
                upd.len() - b.len(),
                r.sections
            );
        }
        same_document(&upd, &exact, &format!("edit {k}"));
        checked += 1;
    }
    eprintln!("harness_pairs: {checked} edits' files read back as their exact files");
}
