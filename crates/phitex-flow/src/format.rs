//! The format: the definitions a format's job reaches at its `\dump`,
//! saved, and given to another build as definitions at the place before
//! its first piece (AGENTS_BRIEF §6.1: "known at t=0: the format, a set
//! of definitions with known versions").
//!
//! A format is not a reader's state: no piece starts with anything copied
//! in. A build given one ([`crate::Build::set_format`]) answers a read of a
//! name no piece defined before it with the format's definition, as it
//! answers IniTeX's initial values; a value carries no place, so a
//! definition from the format is equal to the same definition made by
//! running the format's source.
//!
//! What goes in ([`saved`]): every name TeX's `store_fmt_file` keeps —
//! control sequences and active characters with their meanings, the
//! codes, parameters and registers (boxes included), the fonts with their
//! parameters, the hyphenation patterns and exceptions, the interaction
//! mode, the e-TeX mode and the format's identification. What stays out:
//! what belongs to a run, not to a format (the files and streams, the
//! job's name, the page builder's state and marks, `\mag`'s fixing,
//! whether the job ended, the main vertical list).
//!
//! The file is our own: a magic line, the names its control sequences
//! are (by number in the file, mapped to the build's own numbers when
//! read), its fonts (the TFM file's bytes and the size: loaded again as
//! `\font` loads them), the fonts' numbering, then each definition. The order is the names'
//! and keys' own, so the same definitions give the same bytes.

use std::sync::Arc;

use crate::build::CsTable;
use crate::hash::FxMap;
use crate::hyph::{Exceptions, Patterns};
use crate::item::{BoxNode, Disc, GlueSet, Ins, Item, Leaders, Lig, Whatsit};
use crate::prim::{DimenPar, GluePar, IntPar, Prim, TokPar, UNIMPL};
use crate::tfm::{Font, null_font};
use crate::val::{Glue, Key, Macro, Meaning, Tok, Val};

/// The file's first bytes (its version in the last digit).
const MAGIC: &[u8] = b"phitex-flow format 1\n";

/// A format: its definitions (each name's, by the build's numbers), and
/// the fonts its job loaded, in the order it loaded them (TeX's internal
/// font numbers, which the DVI file's are: a build's fonts are numbered
/// after them).
#[derive(Clone, Debug, Default)]
pub struct Format {
    pub defs: Vec<(Key, Val)>,
    pub fonts: Vec<Arc<str>>,
}

/// Whether a format keeps the definition of `key` (the module's doc).
pub fn saved(key: &Key) -> bool {
    !matches!(
        key,
        Key::File(_)
            | Key::ReadStream(_)
            | Key::Job
            | Key::LineBase
            | Key::SplitMarks
            | Key::Page(_)
            | Key::Mark(..)
            | Key::MagSet
            | Key::Ended
            | Key::MainTail
            | Key::MainDepth
    )
}

// ------------------------------------------------------------------ writing

struct Writer<'a> {
    out: Vec<u8>,
    cs: &'a CsTable,
    /// A control sequence's number in the build -> its number in the file.
    names: FxMap<u32, u32>,
    name_list: Vec<Arc<str>>,
    /// A font's identity -> its number in the file.
    fonts: FxMap<Arc<str>, u32>,
    font_list: Vec<Arc<Font>>,
}

impl Writer<'_> {
    fn u(&mut self, mut x: u64) {
        loop {
            let b = (x & 0x7f) as u8;
            x >>= 7;
            if x == 0 {
                self.out.push(b);
                return;
            }
            self.out.push(b | 0x80);
        }
    }

    fn i(&mut self, x: i32) {
        self.u(u64::from(((x << 1) ^ (x >> 31)) as u32));
    }

    fn byte(&mut self, b: u8) {
        self.out.push(b);
    }

    fn ch(&mut self, c: char) {
        self.u(u64::from(c));
    }

    fn str(&mut self, s: &str) {
        self.u(s.len() as u64);
        self.out.extend_from_slice(s.as_bytes());
    }

    fn name(&mut self, n: u32) {
        let next = self.name_list.len() as u32;
        let i = *self.names.entry(n).or_insert(next);
        if i == next {
            self.name_list.push(self.cs.name(n));
        }
        self.u(u64::from(i));
    }

    fn font(&mut self, f: &Arc<Font>) {
        let next = self.font_list.len() as u32;
        let i = *self.fonts.entry(f.key.clone()).or_insert(next);
        if i == next {
            self.font_list.push(f.clone());
        }
        self.u(u64::from(i));
    }

    fn tok(&mut self, t: &Tok) {
        match t {
            Tok::Char(cat, c) => {
                self.byte(0);
                self.byte(*cat);
                self.ch(*c);
            }
            Tok::Cs(n) => {
                self.byte(1);
                self.name(*n);
            }
            Tok::Active(c) => {
                self.byte(2);
                self.ch(*c);
            }
            Tok::Param(n, c) => {
                self.byte(3);
                self.byte(*n);
                self.ch(*c);
            }
            Tok::DontExpand => self.byte(4),
        }
    }

    fn toks(&mut self, ts: &[Tok]) {
        self.u(ts.len() as u64);
        for t in ts {
            self.tok(t);
        }
    }

    fn prim(&mut self, p: Prim) -> Result<(), String> {
        let name = p.name();
        if prim_named(name) != Some(p) {
            return Err(format!("the primitive {p:?} has no name it is found by again"));
        }
        self.str(name);
        Ok(())
    }

    fn meaning(&mut self, m: &Meaning) -> Result<(), String> {
        match m {
            Meaning::Undefined => self.byte(0),
            Meaning::Prim(p) => {
                self.byte(1);
                self.prim(*p)?;
            }
            Meaning::Macro(m) => {
                self.byte(2);
                self.toks(&m.params);
                self.toks(&m.body);
                self.byte(u8::from(m.long) | u8::from(m.outer) << 1 | u8::from(m.protected) << 2);
            }
            Meaning::Char(cat, c) => {
                self.byte(3);
                self.byte(*cat);
                self.ch(*c);
            }
            Meaning::CharGiven(x) => {
                self.byte(4);
                self.u(u64::from(*x));
            }
            Meaning::MathCharGiven(x) => {
                self.byte(5);
                self.u(u64::from(*x));
            }
            Meaning::CountRef(n) => {
                self.byte(6);
                self.u(u64::from(*n));
            }
            Meaning::DimenRef(n) => {
                self.byte(7);
                self.u(u64::from(*n));
            }
            Meaning::SkipRef(n) => {
                self.byte(8);
                self.u(u64::from(*n));
            }
            Meaning::MuskipRef(n) => {
                self.byte(9);
                self.u(u64::from(*n));
            }
            Meaning::ToksRef(n) => {
                self.byte(10);
                self.u(u64::from(*n));
            }
            Meaning::Font(f) => {
                self.byte(11);
                self.font(f);
            }
        }
        Ok(())
    }

    fn glue(&mut self, g: &Glue) {
        self.i(g.width);
        self.i(g.stretch);
        self.i(g.shrink);
        self.byte(g.stretch_order);
        self.byte(g.shrink_order);
    }

    fn box_node(&mut self, b: &BoxNode) -> Result<(), String> {
        self.byte(u8::from(b.vertical));
        self.i(b.w);
        self.i(b.h);
        self.i(b.d);
        self.i(b.shift);
        self.byte(b.glue.sign as u8);
        self.byte(b.glue.order);
        self.u(b.glue.ratio.to_bits());
        self.items(&b.list)
    }

    fn items(&mut self, list: &[Item]) -> Result<(), String> {
        self.u(list.len() as u64);
        for it in list {
            self.item(it)?;
        }
        Ok(())
    }

    fn item(&mut self, it: &Item) -> Result<(), String> {
        match it {
            Item::Char(f, c) => {
                self.byte(0);
                self.font(f);
                self.byte(*c);
            }
            Item::Lig(l) => {
                self.byte(1);
                self.font(&l.font);
                self.byte(l.c);
                self.u(l.orig.len() as u64);
                self.out.extend_from_slice(&l.orig);
                self.byte(u8::from(l.left) | u8::from(l.right) << 1);
            }
            Item::Box(b) => {
                self.byte(2);
                self.box_node(b)?;
            }
            Item::Rule { w, h, d } => {
                self.byte(3);
                self.i(*w);
                self.i(*h);
                self.i(*d);
            }
            Item::Glue { g, sub, zero } => {
                self.byte(4);
                self.glue(g);
                self.byte(*sub);
                self.byte(u8::from(*zero));
            }
            Item::Kern { w, sub } => {
                self.byte(5);
                self.i(*w);
                self.byte(*sub);
            }
            Item::Penalty(p) => {
                self.byte(6);
                self.i(*p);
            }
            Item::Disc(d) => {
                self.byte(7);
                self.items(&d.pre)?;
                self.items(&d.post)?;
                self.byte(d.replace);
            }
            Item::Mark(t) => {
                self.byte(8);
                self.toks(t);
            }
            Item::Leaders(l) => {
                self.byte(9);
                self.glue(&l.g);
                self.byte(l.sub);
                self.byte(u8::from(l.zero));
                self.item(&l.b)?;
            }
            Item::Whatsit(w) => {
                self.byte(10);
                match &**w {
                    Whatsit::Open { stream, name } => {
                        self.byte(0);
                        self.byte(*stream);
                        self.str(name);
                    }
                    Whatsit::Write { stream, toks } => {
                        self.byte(1);
                        self.byte(*stream);
                        self.toks(toks);
                    }
                    Whatsit::Close { stream } => {
                        self.byte(2);
                        self.byte(*stream);
                    }
                    Whatsit::Special(t) => {
                        self.byte(3);
                        self.str(t);
                    }
                }
            }
            Item::Ins(x) => {
                self.byte(11);
                self.byte(x.n);
                self.i(x.height);
                self.i(x.depth);
                self.glue(&x.split_top);
                self.i(x.float_cost);
                self.items(&x.list)?;
            }
            Item::Adjust(l) => {
                self.byte(12);
                self.items(l)?;
            }
            Item::Marks(c, t) => {
                self.byte(13);
                self.u(u64::from(*c));
                self.toks(t);
            }
            Item::Math { w, after, lr } => {
                self.byte(14);
                self.i(*w);
                self.byte(u8::from(*after));
                self.byte(*lr);
            }
            Item::Lang { lang, lhm, rhm } => {
                self.byte(15);
                self.byte(*lang);
                self.byte(*lhm);
                self.byte(*rhm);
            }
            _ => return Err(format!("an item a format does not hold: {it:?}")),
        }
        Ok(())
    }

    fn val(&mut self, v: &Val) -> Result<(), String> {
        match v {
            Val::Int(x) => {
                self.byte(0);
                self.i(*x);
            }
            Val::Cat(c) => {
                self.byte(1);
                self.byte(*c);
            }
            Val::Meaning(m) => {
                self.byte(2);
                self.meaning(m)?;
            }
            Val::Font(f) => {
                self.byte(3);
                self.font(f);
            }
            Val::Glue(g) => {
                self.byte(4);
                self.glue(g);
            }
            Val::Toks(t) => {
                self.byte(5);
                self.toks(t);
            }
            Val::Parshape(p) => {
                self.byte(6);
                self.u(p.len() as u64);
                for &(a, b) in p.iter() {
                    self.i(a);
                    self.i(b);
                }
            }
            Val::Penalties(p) => {
                self.byte(7);
                self.u(p.len() as u64);
                for &x in p.iter() {
                    self.i(x);
                }
            }
            Val::Void => self.byte(8),
            Val::Name(n) => {
                self.byte(9);
                self.str(n);
            }
            Val::Patterns(p) => {
                self.byte(10);
                self.u(p.len() as u64);
                for (letters, digits) in p.entries() {
                    self.u(letters.len() as u64);
                    for &c in letters {
                        self.ch(c);
                    }
                    self.u(digits.len() as u64);
                    self.out.extend_from_slice(digits);
                }
            }
            Val::Exceptions(x) => {
                self.byte(11);
                self.u(x.len() as u64);
                for (word, hyphens) in x.entries() {
                    self.u(word.len() as u64);
                    for &c in word {
                        self.ch(c);
                    }
                    self.u(hyphens.len() as u64);
                    for &h in hyphens {
                        self.u(h as u64);
                    }
                }
            }
            Val::Box(b) => {
                self.byte(12);
                self.box_node(b)?;
            }
            Val::Bytes(_) | Val::Stream(_) | Val::SplitMarks(_) | Val::Tail(_) | Val::Para(_) | Val::Outer(_) => {
                return Err(format!("a value a format does not hold: {v:?}"));
            }
        }
        Ok(())
    }

    fn key(&mut self, k: &Key) -> Result<(), String> {
        let char_key = |w: &mut Self, tag: u8, c: char| {
            w.byte(tag);
            w.ch(c);
        };
        let num = |w: &mut Self, tag: u8, n: u64| {
            w.byte(tag);
            w.u(n);
        };
        let font_key = |w: &mut Self, tag: u8, f: &str| {
            w.byte(tag);
            w.str(f);
        };
        match k {
            Key::Cs(n) => {
                self.byte(0);
                self.name(*n);
            }
            Key::Active(c) => char_key(self, 1, *c),
            Key::Cat(c) => char_key(self, 2, *c),
            Key::Lccode(c) => char_key(self, 3, *c),
            Key::Uccode(c) => char_key(self, 4, *c),
            Key::Sfcode(c) => char_key(self, 5, *c),
            Key::Mathcode(c) => char_key(self, 6, *c),
            Key::Delcode(c) => char_key(self, 7, *c),
            Key::IntPar(p) => num(self, 8, *p as u64),
            Key::DimenPar(p) => num(self, 9, *p as u64),
            Key::GluePar(p) => num(self, 10, *p as u64),
            Key::TokPar(p) => num(self, 11, *p as u64),
            Key::Count(n) => num(self, 12, u64::from(*n)),
            Key::Dimen(n) => num(self, 13, u64::from(*n)),
            Key::Skip(n) => num(self, 14, u64::from(*n)),
            Key::Muskip(n) => num(self, 15, u64::from(*n)),
            Key::Toks(n) => num(self, 16, u64::from(*n)),
            Key::Box(n) => num(self, 17, u64::from(*n)),
            Key::Parshape => self.byte(18),
            Key::Font => self.byte(19),
            Key::FontDimen(f, n) => {
                font_key(self, 20, f);
                self.u(u64::from(*n));
            }
            Key::HyphenChar(f) => font_key(self, 21, f),
            Key::SkewChar(f) => font_key(self, 22, f),
            Key::FontIdent(f) => font_key(self, 23, f),
            Key::FontLoaded(f) => font_key(self, 24, f),
            Key::FamFont(a, b) => {
                self.byte(25);
                self.byte(*a);
                self.byte(*b);
            }
            Key::PenaltiesArray(n) => num(self, 26, u64::from(*n)),
            Key::ETeXMode => self.byte(27),
            Key::Patterns(l) => num(self, 28, u64::from(*l)),
            Key::Exceptions(l) => num(self, 29, u64::from(*l)),
            Key::Format => self.byte(30),
            Key::FontParams(f) => font_key(self, 31, f),
            Key::LastFont => self.byte(32),
            Key::Interaction => self.byte(33),
            Key::GlyphUnicode(g) => {
                self.byte(34);
                self.str(g);
            }
            Key::FontCode(k, f) => {
                font_key(self, 35, f);
                self.byte(*k);
            }
            Key::FontExpand(f) => font_key(self, 36, f),
            Key::FontAttr(f) => font_key(self, 37, f),
            Key::FontCopies(f) => font_key(self, 38, f),
            _ => return Err(format!("a name a format does not hold: {k:?}")),
        }
        Ok(())
    }
}

/// The primitive whose name is `name` (each table, then the reader's own
/// and the ones without a graph form).
fn prim_named(name: &str) -> Option<Prim> {
    Prim::by_name(name)
        .or_else(|| Prim::by_name_etex(name))
        .or_else(|| Prim::by_name_pdftex(name))
        .or_else(|| Prim::internal(name))
        .or_else(|| UNIMPL.iter().position(|n| *n == name).map(|i| Prim::Unimpl(i as u16)))
}

/// The file of definitions `defs` (keys and tokens by the numbers of
/// `cs`) and font numbering `numbering`; the order is the names' own,
/// whatever the numbers.
pub fn encode(defs: &[(Key, Val)], numbering: &[Arc<str>], cs: &CsTable) -> Result<Vec<u8>, String> {
    // (the order: by what each key says, a control sequence by its name)
    let order = |k: &Key| match k {
        Key::Cs(n) => format!("0{}", cs.name(*n)),
        k => format!("1{k:?}"),
    };
    let mut sorted: Vec<(String, &Key, &Val)> = defs.iter().map(|(k, v)| (order(k), k, v)).collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut w = Writer {
        out: Vec::new(),
        cs,
        names: FxMap::default(),
        name_list: Vec::new(),
        fonts: FxMap::default(),
        font_list: Vec::new(),
    };
    w.u(sorted.len() as u64);
    for (_, k, v) in &sorted {
        w.key(k)?;
        w.val(v)?;
    }
    let body = std::mem::take(&mut w.out);
    w.out.extend_from_slice(MAGIC);
    let names = std::mem::take(&mut w.name_list);
    w.u(names.len() as u64);
    for n in &names {
        w.str(n);
    }
    let fonts = std::mem::take(&mut w.font_list);
    w.u(fonts.len() as u64);
    for f in &fonts {
        w.str(&f.name);
        w.str(&f.key);
        w.i(f.size);
        w.u(f.data.len() as u64);
        w.out.extend_from_slice(&f.data);
    }
    w.u(numbering.len() as u64);
    for f in numbering {
        w.str(f);
    }
    w.out.extend_from_slice(&body);
    Ok(w.out)
}

// ------------------------------------------------------------------ reading

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
    /// A control sequence's number in the file -> the build's.
    names: Vec<u32>,
    fonts: Vec<Arc<Font>>,
}

/// A file that is not a format, or not this version's.
fn bad(what: &str) -> String {
    format!("not a format file ({what})")
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.b.get(self.at).ok_or_else(|| bad("it ends early"))?;
        self.at += 1;
        Ok(b)
    }

    fn u(&mut self) -> Result<u64, String> {
        let mut x = 0u64;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            if shift > 63 {
                return Err(bad("a number too long"));
            }
            x |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(x);
            }
            shift += 7;
        }
    }

    fn n<T: TryFrom<u64>>(&mut self) -> Result<T, String> {
        T::try_from(self.u()?).map_err(|_| bad("a number out of range"))
    }

    fn i(&mut self) -> Result<i32, String> {
        let z: u32 = self.n()?;
        Ok(((z >> 1) as i32) ^ -((z & 1) as i32))
    }

    fn ch(&mut self) -> Result<char, String> {
        char::from_u32(self.n()?).ok_or_else(|| bad("a character out of range"))
    }

    fn bytes(&mut self, len: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(len).filter(|&e| e <= self.b.len()).ok_or_else(|| bad("it ends early"))?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn str(&mut self) -> Result<String, String> {
        let len = self.n()?;
        String::from_utf8(self.bytes(len)?.to_vec()).map_err(|_| bad("a name not UTF-8"))
    }

    fn name(&mut self) -> Result<u32, String> {
        let i: usize = self.n()?;
        self.names.get(i).copied().ok_or_else(|| bad("a control sequence out of range"))
    }

    fn font(&mut self) -> Result<Arc<Font>, String> {
        let i: usize = self.n()?;
        self.fonts.get(i).cloned().ok_or_else(|| bad("a font out of range"))
    }

    fn tok(&mut self) -> Result<Tok, String> {
        Ok(match self.byte()? {
            0 => Tok::Char(self.byte()?, self.ch()?),
            1 => Tok::Cs(self.name()?),
            2 => Tok::Active(self.ch()?),
            3 => Tok::Param(self.byte()?, self.ch()?),
            4 => Tok::DontExpand,
            _ => return Err(bad("a token")),
        })
    }

    fn toks(&mut self) -> Result<Vec<Tok>, String> {
        let len: usize = self.n()?;
        let mut v = Vec::with_capacity(len.min(1 << 20));
        for _ in 0..len {
            v.push(self.tok()?);
        }
        Ok(v)
    }

    fn meaning(&mut self) -> Result<Meaning, String> {
        Ok(match self.byte()? {
            0 => Meaning::Undefined,
            1 => {
                let name = self.str()?;
                Meaning::Prim(prim_named(&name).ok_or_else(|| bad("an unknown primitive"))?)
            }
            2 => {
                let params = self.toks()?;
                let body = Arc::from(self.toks()?);
                let f = self.byte()?;
                Meaning::Macro(Arc::new(Macro {
                    params,
                    body,
                    long: f & 1 != 0,
                    outer: f & 2 != 0,
                    protected: f & 4 != 0,
                }))
            }
            3 => Meaning::Char(self.byte()?, self.ch()?),
            4 => Meaning::CharGiven(self.n()?),
            5 => Meaning::MathCharGiven(self.n()?),
            6 => Meaning::CountRef(self.n()?),
            7 => Meaning::DimenRef(self.n()?),
            8 => Meaning::SkipRef(self.n()?),
            9 => Meaning::MuskipRef(self.n()?),
            10 => Meaning::ToksRef(self.n()?),
            11 => Meaning::Font(self.font()?),
            _ => return Err(bad("a meaning")),
        })
    }

    fn glue(&mut self) -> Result<Arc<Glue>, String> {
        let (width, stretch, shrink) = (self.i()?, self.i()?, self.i()?);
        let (stretch_order, shrink_order) = (self.byte()?, self.byte()?);
        Ok(Arc::new(Glue { width, stretch, shrink, stretch_order, shrink_order }))
    }

    fn box_node(&mut self) -> Result<BoxNode, String> {
        let vertical = self.byte()? != 0;
        let (w, h, d, shift) = (self.i()?, self.i()?, self.i()?, self.i()?);
        let sign = self.byte()? as i8;
        let order = self.byte()?;
        let ratio = f64::from_bits(self.u()?);
        let list = self.items()?;
        Ok(BoxNode { origins: crate::item::Origins::default(), vertical, w, h, d, shift, list, glue: GlueSet { sign, order, ratio } })
    }

    fn items(&mut self) -> Result<Vec<Item>, String> {
        let len: usize = self.n()?;
        let mut v = Vec::with_capacity(len.min(1 << 16));
        for _ in 0..len {
            v.push(self.item()?);
        }
        Ok(v)
    }

    fn item(&mut self) -> Result<Item, String> {
        Ok(match self.byte()? {
            0 => Item::Char(self.font()?, self.byte()?),
            1 => {
                let font = self.font()?;
                let c = self.byte()?;
                let n: usize = self.n()?;
                let orig = self.bytes(n)?.to_vec();
                let f = self.byte()?;
                Item::Lig(Arc::new(Lig { font, c, orig, left: f & 1 != 0, right: f & 2 != 0 }))
            }
            2 => Item::Box(Arc::new(self.box_node()?)),
            3 => Item::Rule { w: self.i()?, h: self.i()?, d: self.i()? },
            4 => {
                let g = self.glue()?;
                Item::Glue { g, sub: self.byte()?, zero: self.byte()? != 0 }
            }
            5 => Item::Kern { w: self.i()?, sub: self.byte()? },
            6 => Item::Penalty(self.i()?),
            7 => {
                let pre = self.items()?;
                let post = self.items()?;
                Item::Disc(Arc::new(Disc { pre, post, replace: self.byte()? }))
            }
            8 => Item::Mark(Arc::new(self.toks()?)),
            9 => {
                let g = self.glue()?;
                let sub = self.byte()?;
                let zero = self.byte()? != 0;
                let b = self.item()?;
                Item::Leaders(Arc::new(Leaders { g, sub, b, zero }))
            }
            10 => Item::Whatsit(Arc::new(match self.byte()? {
                0 => Whatsit::Open { stream: self.byte()?, name: Arc::from(self.str()?) },
                1 => Whatsit::Write { stream: self.byte()?, toks: Arc::new(self.toks()?) },
                2 => Whatsit::Close { stream: self.byte()? },
                3 => Whatsit::Special(Arc::from(self.str()?)),
                _ => return Err(bad("a whatsit")),
            })),
            11 => {
                let n = self.byte()?;
                let (height, depth) = (self.i()?, self.i()?);
                let split_top = self.glue()?;
                let float_cost = self.i()?;
                let list = self.items()?;
                Item::Ins(Arc::new(Ins { n, height, depth, split_top, float_cost, list }))
            }
            12 => Item::Adjust(Arc::new(self.items()?)),
            13 => {
                let c = self.n()?;
                Item::Marks(c, Arc::new(self.toks()?))
            }
            14 => Item::Math { w: self.i()?, after: self.byte()? != 0, lr: self.byte()? },
            15 => Item::Lang { lang: self.byte()?, lhm: self.byte()?, rhm: self.byte()? },
            _ => return Err(bad("an item")),
        })
    }

    fn val(&mut self) -> Result<Val, String> {
        Ok(match self.byte()? {
            0 => Val::Int(self.i()?),
            1 => Val::Cat(self.byte()?),
            2 => Val::Meaning(self.meaning()?),
            3 => Val::Font(self.font()?),
            4 => Val::Glue(self.glue()?),
            5 => Val::Toks(Arc::new(self.toks()?)),
            6 => {
                let len: usize = self.n()?;
                let mut v = Vec::with_capacity(len.min(1 << 16));
                for _ in 0..len {
                    v.push((self.i()?, self.i()?));
                }
                Val::Parshape(Arc::new(v))
            }
            7 => {
                let len: usize = self.n()?;
                let mut v = Vec::with_capacity(len.min(1 << 16));
                for _ in 0..len {
                    v.push(self.i()?);
                }
                Val::Penalties(Arc::from(v))
            }
            8 => Val::Void,
            9 => Val::Name(Arc::from(self.str()?)),
            10 => {
                let len: usize = self.n()?;
                let mut v = Vec::with_capacity(len.min(1 << 20));
                for _ in 0..len {
                    let n: usize = self.n()?;
                    let letters = (0..n).map(|_| self.ch()).collect::<Result<Box<[char]>, _>>()?;
                    let n: usize = self.n()?;
                    let digits: Box<[u8]> = self.bytes(n)?.into();
                    v.push((letters, digits));
                }
                Val::Patterns(Patterns::from_entries(v))
            }
            11 => {
                let len: usize = self.n()?;
                let mut v = Vec::with_capacity(len.min(1 << 20));
                for _ in 0..len {
                    let n: usize = self.n()?;
                    let word = (0..n).map(|_| self.ch()).collect::<Result<Box<[char]>, _>>()?;
                    let n: usize = self.n()?;
                    let hyphens = (0..n).map(|_| self.n::<usize>()).collect::<Result<Box<[usize]>, _>>()?;
                    v.push((word, hyphens));
                }
                Val::Exceptions(Exceptions::from_entries(v))
            }
            12 => Val::Box(Arc::new(self.box_node()?)),
            _ => return Err(bad("a value")),
        })
    }

    fn key(&mut self) -> Result<Key, String> {
        fn par<T: Copy>(all: &[T], i: usize) -> Result<T, String> {
            all.get(i).copied().ok_or_else(|| bad("a parameter out of range"))
        }
        Ok(match self.byte()? {
            0 => Key::Cs(self.name()?),
            1 => Key::Active(self.ch()?),
            2 => Key::Cat(self.ch()?),
            3 => Key::Lccode(self.ch()?),
            4 => Key::Uccode(self.ch()?),
            5 => Key::Sfcode(self.ch()?),
            6 => Key::Mathcode(self.ch()?),
            7 => Key::Delcode(self.ch()?),
            8 => Key::IntPar(par(&IntPar::ALL, self.n()?)?),
            9 => Key::DimenPar(par(&DimenPar::ALL, self.n()?)?),
            10 => Key::GluePar(par(&GluePar::ALL, self.n()?)?),
            11 => Key::TokPar(par(&TokPar::ALL, self.n()?)?),
            12 => Key::Count(self.n()?),
            13 => Key::Dimen(self.n()?),
            14 => Key::Skip(self.n()?),
            15 => Key::Muskip(self.n()?),
            16 => Key::Toks(self.n()?),
            17 => Key::Box(self.n()?),
            18 => Key::Parshape,
            19 => Key::Font,
            20 => {
                let f = Arc::from(self.str()?);
                Key::FontDimen(f, self.n()?)
            }
            21 => Key::HyphenChar(Arc::from(self.str()?)),
            22 => Key::SkewChar(Arc::from(self.str()?)),
            23 => Key::FontIdent(Arc::from(self.str()?)),
            24 => Key::FontLoaded(Arc::from(self.str()?)),
            25 => Key::FamFont(self.byte()?, self.byte()?),
            26 => Key::PenaltiesArray(self.n()?),
            27 => Key::ETeXMode,
            28 => Key::Patterns(self.n()?),
            29 => Key::Exceptions(self.n()?),
            30 => Key::Format,
            31 => Key::FontParams(Arc::from(self.str()?)),
            32 => Key::LastFont,
            33 => Key::Interaction,
            34 => Key::GlyphUnicode(Arc::from(self.str()?)),
            35 => {
                let f = Arc::from(self.str()?);
                Key::FontCode(self.byte()?, f)
            }
            36 => Key::FontExpand(Arc::from(self.str()?)),
            37 => Key::FontAttr(Arc::from(self.str()?)),
            38 => Key::FontCopies(Arc::from(self.str()?)),
            _ => return Err(bad("a name")),
        })
    }
}

/// The format in file `b`, its control sequences numbered by `cs` (each
/// name is given its number there), its fonts loaded again from the bytes
/// the file holds.
pub fn decode(b: &[u8], cs: &CsTable) -> Result<Format, String> {
    if !b.starts_with(MAGIC) {
        return Err(bad("its first line"));
    }
    let mut r = Reader { b, at: MAGIC.len(), names: Vec::new(), fonts: Vec::new() };
    let names: usize = r.n()?;
    for _ in 0..names {
        let n = r.str()?;
        r.names.push(cs.id(&n));
    }
    let fonts: usize = r.n()?;
    for _ in 0..fonts {
        let name = r.str()?;
        let key = r.str()?;
        let size = r.i()?;
        let len: usize = r.n()?;
        let data: Arc<[u8]> = Arc::from(r.bytes(len)?);
        let f = if data.is_empty() {
            null_font()
        } else {
            // (as `\font` loads it: at its design size, or at `size`)
            let at = (Font::design_size_of(&data).ok() != Some(size)).then_some(size);
            Arc::new(Font::load(&name, data, at).map_err(|_| format!("the format's font {name}: bad TFM data"))?)
        };
        if *f.key != *key {
            return Err(format!("the format's font {key} loads as {}", f.key));
        }
        r.fonts.push(f);
    }
    let numbered: usize = r.n()?;
    let mut fonts = Vec::with_capacity(numbered.min(1 << 16));
    for _ in 0..numbered {
        fonts.push(Arc::from(r.str()?));
    }
    let len: usize = r.n()?;
    let mut defs = Vec::with_capacity(len.min(1 << 24));
    for _ in 0..len {
        let k = r.key()?;
        let v = r.val()?;
        defs.push((k, v));
    }
    if r.at != b.len() {
        return Err(bad("bytes after its definitions"));
    }
    Ok(Format { defs, fonts })
}

/// The tag numbers (CORE.md "Tag numbers"): the format's tags of keys,
/// meanings and whatsits are each distinct. The keys are
/// `val::every_key`; the meanings and whatsits are listed here, each list
/// kept whole by a `match` without wildcard (a new kind does not compile
/// until it has an index, and the `*_once` test fails until it is listed).
#[cfg(test)]
mod tag_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn writer(cs: &CsTable) -> Writer<'_> {
        Writer { out: Vec::new(), cs, names: FxMap::default(), name_list: Vec::new(), fonts: FxMap::default(), font_list: Vec::new() }
    }

    /// Panics naming both when two things share a tag.
    fn distinct<T: std::fmt::Debug>(what: &str, tags: Vec<(u8, T)>) {
        let mut seen: BTreeMap<u8, String> = BTreeMap::new();
        for (t, x) in tags {
            if let Some(o) = seen.insert(t, format!("{x:?}")) {
                panic!("format {what} tag {t}: {o} and {x:?}");
            }
        }
    }

    fn once<T>(all: &[T], index: impl Fn(&T) -> usize, what: &str) {
        let mut ix: Vec<usize> = all.iter().map(index).collect();
        ix.sort_unstable();
        let want: Vec<usize> = (0..ix.len()).collect();
        assert_eq!(ix, want, "{what}: one of each kind, each kind's index");
    }

    fn every_meaning() -> Vec<Meaning> {
        let m = Macro { params: Vec::new(), body: Arc::from(Vec::new()), long: false, outer: false, protected: false };
        vec![
            Meaning::Undefined,
            Meaning::Prim(Prim::by_name("relax").expect("relax")),
            Meaning::Macro(Arc::new(m)),
            Meaning::Char(11, 'a'),
            Meaning::CharGiven(1),
            Meaning::MathCharGiven(1),
            Meaning::CountRef(1),
            Meaning::DimenRef(1),
            Meaning::SkipRef(1),
            Meaning::MuskipRef(1),
            Meaning::ToksRef(1),
            Meaning::Font(null_font()),
        ]
    }

    fn meaning_index(m: &Meaning) -> usize {
        match m {
            Meaning::Undefined => 0,
            Meaning::Prim(_) => 1,
            Meaning::Macro(_) => 2,
            Meaning::Char(..) => 3,
            Meaning::CharGiven(_) => 4,
            Meaning::MathCharGiven(_) => 5,
            Meaning::CountRef(_) => 6,
            Meaning::DimenRef(_) => 7,
            Meaning::SkipRef(_) => 8,
            Meaning::MuskipRef(_) => 9,
            Meaning::ToksRef(_) => 10,
            Meaning::Font(_) => 11,
        }
    }

    fn every_whatsit() -> Vec<Whatsit> {
        vec![
            Whatsit::Open { stream: 1, name: Arc::from("a") },
            Whatsit::Write { stream: 1, toks: Arc::new(Vec::new()) },
            Whatsit::Close { stream: 1 },
            Whatsit::Special(Arc::from("a")),
        ]
    }

    fn whatsit_index(w: &Whatsit) -> usize {
        match w {
            Whatsit::Open { .. } => 0,
            Whatsit::Write { .. } => 1,
            Whatsit::Close { .. } => 2,
            Whatsit::Special(_) => 3,
        }
    }

    #[test]
    fn meanings_and_whatsits_once() {
        once(&every_meaning(), meaning_index, "every_meaning");
        once(&every_whatsit(), whatsit_index, "every_whatsit");
    }

    #[test]
    fn format_key_tags_are_distinct() {
        let cs = CsTable::new();
        let mut tags = Vec::new();
        for k in crate::val::every_key() {
            let k = if let Key::Cs(_) = k { Key::Cs(cs.id("relax")) } else { k };
            let mut w = writer(&cs);
            // (a key a format does not hold has no tag)
            if w.key(&k).is_ok() {
                tags.push((w.out[0], k));
            }
        }
        distinct("key", tags);
    }

    #[test]
    fn format_meaning_tags_are_distinct() {
        let cs = CsTable::new();
        let mut tags = Vec::new();
        for m in every_meaning() {
            let mut w = writer(&cs);
            w.meaning(&m).expect("a meaning a format holds");
            tags.push((w.out[0], m));
        }
        distinct("meaning", tags);
    }

    #[test]
    fn format_whatsit_tags_are_distinct() {
        let cs = CsTable::new();
        let mut tags = Vec::new();
        for x in every_whatsit() {
            let mut w = writer(&cs);
            w.item(&Item::Whatsit(Arc::new(x.clone()))).expect("a whatsit a format holds");
            // (after the item's own tag)
            tags.push((w.out[1], x));
        }
        distinct("whatsit", tags);
    }
}
