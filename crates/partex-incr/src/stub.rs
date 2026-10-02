//! A stub language that drives the runtime's tests (`DESIGN.md` §7.0).
//!
//! A program is a list of text lines with stable identities (an editor's
//! view: inserting a line does not rename the others). Each line is a
//! cell whose value is its text and the next line's id, so an edit is a
//! change of a few line cells. One statement per line:
//!
//! | Line | Meaning |
//! |---|---|
//! | `set x3 7` | `x3 := 7` |
//! | `inc x3` | `x3 := x3 + 1` (stays symbolic on a hole: `h + k`) |
//! | `add x3 x4` | `x3 := x3 + x4` (forces one hole if both are holes) |
//! | `mul x3 5` | `x3 := x3 * 5` (a forcing point) |
//! | `copy x3 x4` | `x3 := x4` |
//! | `emit x3` | output `x3` (a hole prints when resolved) |
//! | `say words…` | output the words |
//! | `label L2 x3` | label `L2` takes `x3`'s value |
//! | `ref L2` | output label `L2`'s final value: a forward reference, resolved at the link step (`??` if never set) |
//! | `alloc` | output `obj N`, `N` numbered in allocation order at the link step (never state) |
//! | `if x3 > 5 then <stmt>` | a forcing point |
//! | `work 500` | burn 500 cost units (a paragraph's worth of work) |
//! | `mark 3` | add glyph 3 to the glyphs used (an accumulator, as TeX's glyphs used by pages shipped) |
//! | `fonts` | output the glyphs used so far (as TeX's fonts written at the end read them) |
//! | `has 3` | output whether glyph 3 was used so far: a question about the accumulator (a derived cell, as TeX's final number of one object is a question about the numbering) |
//! | `raw` / `endraw` | between them lines are output verbatim, not run: a mode that changes how later text is read, like a catcode change |
//! | blank / `section` | coarser candidate boundaries (levels 1 and 2) |
//!
//! Unset variables read as 0. [`oracle`] is an independent interpreter the
//! tests compare every path against.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hash::{Hash, Hasher};

use crate::hash::{Version, version_of};
use crate::link::LinkCtx;
use crate::machine::{Affine, Forced, Hole, Machine, Recorder, Split, Step};

pub type LineId = u32;
pub const VARS: usize = 64;
pub const LABELS: usize = 16;

/// A source line: its text and its successor. Hashed once, when made
/// (§7.1: an object stored in a cell acquires its hash when stored).
#[derive(Debug)]
pub struct LineData {
    pub text: Arc<str>,
    pub next: Option<LineId>,
    digest: Version,
}

impl LineData {
    fn new(text: Arc<str>, next: Option<LineId>) -> Arc<Self> {
        let digest = version_of(&(&*text, next));
        Arc::new(Self { text, next, digest })
    }
}

impl PartialEq for LineData {
    fn eq(&self, o: &Self) -> bool {
        self.digest == o.digest
    }
}
impl Eq for LineData {}
impl Hash for LineData {
    fn hash<H: Hasher>(&self, h: &mut H) {
        h.write_u128(self.digest.0);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Val {
    Int(i64),
    Sym(Affine),
    Raw(bool),
    Line(Arc<LineData>),
    Used(Used),
}

/// The accumulator's value (as TeX's `Glyphs`): its version is the whole
/// set, but what a region writes, and replay adds, is what it added.
#[derive(Clone, Copy, Debug)]
pub struct Used {
    pub all: u32,
    pub added: u32,
}

impl PartialEq for Used {
    fn eq(&self, o: &Self) -> bool {
        self.all == o.all
    }
}
impl Eq for Used {}
impl Hash for Used {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.all.hash(h);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cell {
    Line(LineId),
    Mode,
    Var(u8),
    Label(u8),
    /// The glyphs used: an accumulator.
    Used,
    /// Whether glyph `g` was used: a question about `Used`
    /// ([`Machine::derived`]).
    UsedHas(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Int(i64),
    Sym(Affine),
    /// `text[from..]` and a newline.
    Text {
        text: Arc<str>,
        from: u32,
    },
    Ref(u8),
    Alloc,
    Used(u32),
    Has(bool),
}

/// A parsed line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stmt {
    Blank,
    Section,
    Set(u8, i64),
    Inc(u8),
    Add(u8, u8),
    Mul(u8, i64),
    Copy(u8, u8),
    Emit(u8),
    /// Output the line from this byte offset.
    Say(u32),
    Label(u8, u8),
    Ref(u8),
    Alloc,
    Raw,
    EndRaw,
    Verbatim,
    If(u8, i64, alloc::boxed::Box<Stmt>),
    Work(u32),
    Mark(u8),
    Fonts,
    Has(u8),
    Bad,
}

fn var(w: &str) -> Option<u8> {
    let n: usize = w.strip_prefix('x')?.parse().ok()?;
    u8::try_from(n).ok().filter(|_| n < VARS)
}

fn label(w: &str) -> Option<u8> {
    let n: usize = w.strip_prefix('L')?.parse().ok()?;
    u8::try_from(n).ok().filter(|_| n < LABELS)
}

/// Parse a line read in the given mode.
#[must_use]
pub fn parse(text: &str, raw: bool) -> Stmt {
    if raw {
        return if text.trim() == "endraw" {
            Stmt::EndRaw
        } else {
            Stmt::Verbatim
        };
    }
    parse_code(text, 0).unwrap_or(Stmt::Bad)
}

fn parse_code(text: &str, start: usize) -> Option<Stmt> {
    let body = &text[start..];
    let mut w = body.split_whitespace();
    let Some(op) = w.next() else {
        return Some(Stmt::Blank);
    };
    let int = |s: Option<&str>| s.and_then(|s| s.parse::<i64>().ok());
    let s = match op {
        "section" => Stmt::Section,
        "set" => Stmt::Set(var(w.next()?)?, int(w.next())?),
        "inc" => Stmt::Inc(var(w.next()?)?),
        "add" => Stmt::Add(var(w.next()?)?, var(w.next()?)?),
        "mul" => Stmt::Mul(var(w.next()?)?, int(w.next())?),
        "copy" => Stmt::Copy(var(w.next()?)?, var(w.next()?)?),
        "emit" => Stmt::Emit(var(w.next()?)?),
        "say" => {
            let at = body.find("say")? + 3;
            let rest = body[at..].trim_start();
            let off = text.len() - rest.len();
            return Some(Stmt::Say(u32::try_from(off).ok()?));
        }
        "label" => Stmt::Label(label(w.next()?)?, var(w.next()?)?),
        "ref" => Stmt::Ref(label(w.next()?)?),
        "alloc" => Stmt::Alloc,
        "raw" => Stmt::Raw,
        "endraw" => Stmt::EndRaw,
        "work" => Stmt::Work(w.next()?.parse().ok()?),
        "mark" => Stmt::Mark(w.next()?.parse().ok().filter(|&g: &u8| g < 32)?),
        "fonts" => Stmt::Fonts,
        "has" => Stmt::Has(w.next()?.parse().ok().filter(|&g: &u8| g < 32)?),
        "if" => {
            let x = var(w.next()?)?;
            if w.next()? != ">" {
                return None;
            }
            let n = int(w.next())?;
            if w.next()? != "then" {
                return None;
            }
            let at = start + body.find(" then ")? + 6;
            let inner = parse_code(text, at)?;
            if matches!(
                inner,
                Stmt::If(..) | Stmt::Raw | Stmt::EndRaw | Stmt::Blank | Stmt::Section
            ) {
                return None;
            }
            return Some(Stmt::If(x, n, alloc::boxed::Box::new(inner)));
        }
        _ => return None,
    };
    if w.next().is_some() {
        return None;
    }
    Some(s)
}

/// Deterministic busy work standing in for an expensive statement.
fn burn(n: u32) -> u64 {
    let mut x: u64 = 0x2545_f491_4f6c_dd1d ^ u64::from(n);
    for i in 0..n {
        // A serial dependency chain the optimizer cannot fold.
        x = core::hint::black_box((x ^ u64::from(i)).wrapping_mul(6_364_136_223_846_793_005))
            .rotate_left(17);
    }
    x
}

const CHUNK: usize = 64;
type LineChunk = Arc<Vec<Option<Arc<LineData>>>>;

/// Line storage by id: a two-level persistent vector, so a machine clone
/// shares it and replacing one line copies one chunk and the spine
/// (§5.3's persistent state, in miniature).
#[derive(Clone, Debug, Default)]
struct Lines {
    chunks: Arc<Vec<LineChunk>>,
    len: usize,
}

impl Lines {
    fn get(&self, id: LineId) -> Option<&Arc<LineData>> {
        let i = id as usize;
        self.chunks.get(i / CHUNK)?[i % CHUNK].as_ref()
    }

    fn put(&mut self, id: LineId, v: Option<Arc<LineData>>) -> Option<Arc<LineData>> {
        let i = id as usize;
        let chunks = Arc::make_mut(&mut self.chunks);
        while chunks.len() <= i / CHUNK {
            chunks.push(Arc::new(alloc::vec![None; CHUNK]));
        }
        let slot = &mut Arc::make_mut(&mut chunks[i / CHUNK])[i % CHUNK];
        let old = core::mem::replace(slot, v);
        self.len = self.len + usize::from(slot.is_some()) - usize::from(old.is_some());
        old
    }

    fn insert(&mut self, id: LineId, v: Arc<LineData>) {
        self.put(id, Some(v));
    }

    fn remove(&mut self, id: LineId) -> Option<Arc<LineData>> {
        self.put(id, None)
    }

    fn len(&self) -> usize {
        self.len
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn iter(&self) -> impl Iterator<Item = (LineId, &Arc<LineData>)> {
        (0u32..)
            .zip(self.chunks.iter().flat_map(|c| c.iter()))
            .filter_map(|(i, l)| l.as_ref().map(|l| (i, l)))
    }
}

impl core::ops::Index<&LineId> for Lines {
    type Output = Arc<LineData>;
    fn index(&self, id: &LineId) -> &Arc<LineData> {
        self.get(*id).expect("a line of the program")
    }
}

/// A program under edit.
#[derive(Clone, Debug)]
pub struct Program {
    lines: Lines,
    head: Option<LineId>,
    next_id: LineId,
}

impl Program {
    /// One line per line of `src`.
    ///
    /// # Panics
    ///
    /// Beyond 2^32 lines.
    #[must_use]
    pub fn from_text(src: &str) -> Self {
        let texts: Vec<&str> = src.lines().collect();
        let n = u32::try_from(texts.len()).expect("fewer than 2^32 lines");
        let mut lines = Lines::default();
        for (i, t) in (0u32..).zip(&texts) {
            let next = (i + 1 < n).then_some(i + 1);
            lines.insert(i, LineData::new(Arc::from(*t), next));
        }
        Self {
            lines,
            head: (n > 0).then_some(0),
            next_id: n,
        }
    }

    /// The same program with its lines numbered by their places, as a
    /// file's lines are (an insertion renumbers the lines after it), and
    /// each old id's new one.
    #[must_use]
    pub fn renumbered(&self) -> (Self, alloc::collections::BTreeMap<LineId, LineId>) {
        let ids = self.ids();
        let map: alloc::collections::BTreeMap<LineId, LineId> = ids
            .iter()
            .zip(0u32..)
            .map(|(&old, new)| (old, new))
            .collect();
        let texts: Vec<&str> = ids.iter().map(|&id| self.line(id)).collect();
        let mut src = String::new();
        for t in texts {
            src.push_str(t);
            src.push('\n');
        }
        (Self::from_text(&src), map)
    }

    /// Line ids in program order.
    #[must_use]
    pub fn ids(&self) -> Vec<LineId> {
        let mut out = Vec::with_capacity(self.lines.len());
        let mut at = self.head;
        while let Some(id) = at {
            out.push(id);
            at = self.lines[&id].next;
        }
        out
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The text of line `id`.
    #[must_use]
    pub fn line(&self, id: LineId) -> &str {
        &self.lines[&id].text
    }

    /// The whole text.
    #[must_use]
    pub fn text(&self) -> String {
        let mut s = String::new();
        for id in self.ids() {
            s.push_str(&self.lines[&id].text);
            s.push('\n');
        }
        s
    }

    /// Replace line `id`'s text. Returns the cells changed.
    pub fn replace(&mut self, id: LineId, text: &str) -> Vec<Cell> {
        let lines = &mut self.lines;
        let next = lines[&id].next;
        lines.insert(id, LineData::new(Arc::from(text), next));
        alloc::vec![Cell::Line(id)]
    }

    /// Insert a line after `after` (`None`: first). Returns the cells
    /// changed.
    ///
    /// # Panics
    ///
    /// If `after` is not a line of the program.
    pub fn insert_after(&mut self, after: Option<LineId>, text: &str) -> Vec<Cell> {
        let id = self.next_id;
        self.next_id += 1;
        let lines = &mut self.lines;
        match after {
            None => {
                lines.insert(id, LineData::new(Arc::from(text), self.head));
                self.head = Some(id);
                alloc::vec![Cell::Line(id)]
            }
            Some(a) => {
                let prev = lines[&a].clone();
                lines.insert(id, LineData::new(Arc::from(text), prev.next));
                lines.insert(a, LineData::new(prev.text.clone(), Some(id)));
                alloc::vec![Cell::Line(a), Cell::Line(id)]
            }
        }
    }

    /// Delete line `id`. Returns the cells changed.
    ///
    /// # Panics
    ///
    /// If `id` is not a line of the program.
    pub fn delete(&mut self, id: LineId) -> Vec<Cell> {
        let pred = self.ids().into_iter().take_while(|&i| i != id).last();
        let lines = &mut self.lines;
        let gone = lines.remove(id).expect("a line of the program");
        match pred {
            None => {
                self.head = gone.next;
                alloc::vec![Cell::Line(id)]
            }
            Some(p) => {
                let prev = lines[&p].clone();
                lines.insert(p, LineData::new(prev.text.clone(), gone.next));
                alloc::vec![Cell::Line(p), Cell::Line(id)]
            }
        }
    }

    /// The machine at the program's start.
    #[must_use]
    pub fn machine(&self) -> Stub {
        Stub {
            lines: self.lines.clone(),
            pc: self.head,
            raw: false,
            vars: [const { None }; VARS],
            labels: [const { None }; LABELS],
            used: 0,
            region: RegionUsed {
                fresh: true,
                ..RegionUsed::default()
            },
        }
    }
}

/// The stub machine.
#[derive(Clone, Debug)]
pub struct Stub {
    lines: Lines,
    pc: Option<LineId>,
    raw: bool,
    vars: [Option<Val>; VARS],
    labels: [Option<Val>; LABELS],
    /// The glyphs used.
    used: u32,
    region: RegionUsed,
}

/// The accumulator's reads and writes, reported when the region is cut
/// (as TeX reports its glyphs): read at the region's entry, whatever the
/// region added before reading, and written as what it added. Not state.
#[derive(Clone, Debug, Default)]
struct RegionUsed {
    /// A region begins with the next step (cut, or moved to by replay).
    fresh: bool,
    at_entry: u32,
    added: u32,
    read: bool,
    /// The glyphs asked about (`has`), their answers at the entry.
    asked: u32,
}

struct Suspend;

impl Stub {
    /// This state with its program renumbered: `p`'s lines, and the next
    /// line to run by `map` (a build's final state, renamed:
    /// `Build::rename`).
    pub fn renamed(&mut self, p: &Program, map: &alloc::collections::BTreeMap<LineId, LineId>) {
        self.lines = p.lines.clone();
        self.pc = self.pc.and_then(|id| map.get(&id).copied());
    }

    fn var<R: Recorder<Self>>(&self, r: &mut R, x: u8) -> Option<Val> {
        let v = &self.vars[usize::from(x)];
        r.read(&Cell::Var(x), v.as_ref());
        v.clone()
    }

    /// A concrete integer, forcing a hole.
    fn int<R: Recorder<Self>>(r: &mut R, v: Option<&Val>) -> Result<i64, Suspend> {
        match v {
            Some(Val::Int(i)) => Ok(*i),
            Some(Val::Sym(a)) => match r.force(a.hole) {
                Forced::Value(Val::Int(g)) => Ok(g.wrapping_add(a.k)),
                Forced::Value(_) => Ok(a.k),
                Forced::Suspend => Err(Suspend),
            },
            _ => Ok(0),
        }
    }

    fn put<R: Recorder<Self>>(&mut self, r: &mut R, c: &Cell, v: Val) {
        r.write(c, &v);
        match *c {
            Cell::Var(x) => self.vars[usize::from(x)] = Some(v),
            Cell::Label(l) => self.labels[usize::from(l)] = Some(v),
            Cell::Mode => self.raw = v == Val::Raw(true),
            Cell::Line(_) => unreachable!("programs do not write their source"),
            Cell::Used => unreachable!("the accumulator is written when a region is cut"),
            Cell::UsedHas(_) => unreachable!("a question is not written"),
        }
    }

    /// Run one statement. Every force comes before any write or effect,
    /// so a suspended step has done nothing.
    fn exec<R: Recorder<Self>>(
        &mut self,
        r: &mut R,
        s: &Stmt,
        line: &LineData,
    ) -> Result<(), Suspend> {
        match *s {
            Stmt::Blank => {}
            Stmt::Section | Stmt::Verbatim | Stmt::Bad => r.effect(Effect::Text {
                text: line.text.clone(),
                from: 0,
            }),
            Stmt::Set(x, n) => self.put(r, &Cell::Var(x), Val::Int(n)),
            Stmt::Inc(x) => {
                let v = match self.var(r, x) {
                    Some(Val::Int(a)) => Val::Int(a.wrapping_add(1)),
                    Some(Val::Sym(a)) => Val::Sym(a.plus(1)),
                    _ => Val::Int(1),
                };
                self.put(r, &Cell::Var(x), v);
            }
            Stmt::Add(x, y) => {
                let a = self.var(r, x);
                let b = self.var(r, y);
                let v = match (&a, &b) {
                    (Some(Val::Sym(a)), b) => Val::Sym(a.plus(Self::int(r, b.as_ref())?)),
                    (a, Some(Val::Sym(b))) => Val::Sym(b.plus(Self::int(r, a.as_ref())?)),
                    (a, b) => {
                        Val::Int(Self::int(r, a.as_ref())?.wrapping_add(Self::int(r, b.as_ref())?))
                    }
                };
                self.put(r, &Cell::Var(x), v);
            }
            Stmt::Mul(x, n) => {
                let a = self.var(r, x);
                let a = Self::int(r, a.as_ref())?;
                self.put(r, &Cell::Var(x), Val::Int(a.wrapping_mul(n)));
            }
            Stmt::Copy(x, y) => {
                let v = self.var(r, y).unwrap_or(Val::Int(0));
                self.put(r, &Cell::Var(x), v);
            }
            Stmt::Emit(x) => {
                let e = match self.var(r, x) {
                    Some(Val::Sym(a)) => Effect::Sym(a),
                    v => Effect::Int(Self::int(r, v.as_ref())?),
                };
                r.effect(e);
            }
            Stmt::Say(from) => r.effect(Effect::Text {
                text: line.text.clone(),
                from,
            }),
            Stmt::Label(l, x) => {
                let v = self.var(r, x).unwrap_or(Val::Int(0));
                self.put(r, &Cell::Label(l), v);
            }
            Stmt::Ref(l) => r.effect(Effect::Ref(l)),
            Stmt::Alloc => r.effect(Effect::Alloc),
            Stmt::Raw => self.put(r, &Cell::Mode, Val::Raw(true)),
            Stmt::EndRaw => self.put(r, &Cell::Mode, Val::Raw(false)),
            Stmt::If(x, n, ref inner) => {
                let v = self.var(r, x);
                if Self::int(r, v.as_ref())? > n {
                    self.exec(r, inner, line)?;
                }
            }
            Stmt::Work(n) => {
                burn(n);
                r.cost(u64::from(n));
            }
            Stmt::Mark(g) => {
                self.used |= 1 << g;
                self.region.added |= 1 << g;
            }
            Stmt::Fonts => {
                self.region.read = true;
                r.effect(Effect::Used(self.used));
            }
            Stmt::Has(g) => {
                self.region.asked |= 1 << g;
                r.effect(Effect::Has(self.used & (1 << g) != 0));
            }
        }
        Ok(())
    }
}

fn push_int(out: &mut Vec<u8>, v: i64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    let mut u = v.unsigned_abs();
    loop {
        i -= 1;
        buf[i] = b'0' + u8::try_from(u % 10).expect("a digit");
        u /= 10;
        if u == 0 {
            break;
        }
    }
    if v < 0 {
        out.push(b'-');
    }
    out.extend_from_slice(&buf[i..]);
}

fn subst(v: &Val, env: &dyn Fn(Hole) -> Option<Val>) -> Val {
    match v {
        Val::Sym(a) => match env(a.hole) {
            Some(Val::Int(i)) => Val::Int(i.wrapping_add(a.k)),
            _ => v.clone(),
        },
        _ => v.clone(),
    }
}

impl Machine for Stub {
    type Cell = Cell;
    type Value = Val;
    type Effect = Effect;
    type Boundary = Option<LineId>;

    fn step<R: Recorder<Self>>(&mut self, r: &mut R) -> Step {
        if core::mem::take(&mut self.region.fresh) {
            self.region = RegionUsed {
                at_entry: self.used,
                ..RegionUsed::default()
            };
        }
        let Some(id) = self.pc else {
            return Step::Halt;
        };
        let line = self.lines[&id].clone();
        r.read(&Cell::Line(id), Some(&Val::Line(line.clone())));
        r.read(&Cell::Mode, Some(&Val::Raw(self.raw)));
        let s = parse(&line.text, self.raw);
        if self.exec(r, &s, &line).is_err() {
            return Step::Suspended;
        }
        self.pc = line.next;
        Step::Candidate(match s {
            Stmt::Blank => 1,
            Stmt::Section => 2,
            _ => 0,
        })
    }

    fn get(&self, c: &Cell) -> Option<Val> {
        match *c {
            Cell::Line(id) => self.lines.get(id).map(|l| Val::Line(l.clone())),
            Cell::Mode => Some(Val::Raw(self.raw)),
            Cell::Var(x) => self.vars[usize::from(x)].clone(),
            Cell::Label(l) => self.labels[usize::from(l)].clone(),
            Cell::Used => Some(Val::Used(Used {
                all: self.used,
                added: self.region.added,
            })),
            Cell::UsedHas(g) => Some(Val::Int(i64::from(self.used >> g & 1))),
        }
    }

    fn set(&mut self, c: &Cell, v: Option<Val>) {
        match *c {
            Cell::Line(id) => {
                let lines = &mut self.lines;
                match v {
                    Some(Val::Line(l)) => {
                        lines.insert(id, l);
                    }
                    _ => {
                        lines.remove(id);
                    }
                }
            }
            Cell::Mode => self.raw = v == Some(Val::Raw(true)),
            Cell::Var(x) => self.vars[usize::from(x)] = v,
            Cell::Label(l) => self.labels[usize::from(l)] = v,
            // (setting the accumulator adds to it)
            Cell::Used => {
                if let Some(Val::Used(u)) = v {
                    self.used |= u.added;
                }
            }
            // (a question: setting it does nothing)
            Cell::UsedHas(_) => {}
        }
    }

    fn at(&self) -> Option<LineId> {
        self.pc
    }

    fn seek(&mut self, b: &Option<LineId>) {
        self.pc = *b;
        self.region.fresh = true;
    }

    fn prepare_cut<R: Recorder<Self>>(&mut self, r: &mut R) {
        let u = |all, added| Val::Used(Used { all, added });
        let e = self.region.at_entry;
        if self.region.read || self.region.asked != 0 {
            r.read(&Cell::Used, Some(&u(e, 0)));
        }
        if !self.region.read {
            // (only questions: their answers at the entry, what the
            // region added before asking being its own)
            for g in 0..32u8 {
                if self.region.asked >> g & 1 != 0 {
                    r.read(&Cell::UsedHas(g), Some(&Val::Int(i64::from(e >> g & 1))));
                }
            }
        }
        if self.region.added != 0 {
            r.write(&Cell::Used, &u(self.used, self.region.added));
        }
        self.region.fresh = true;
    }

    fn accumulates(c: &Cell) -> bool {
        *c == Cell::Used
    }

    fn derived(c: &Cell) -> Option<Cell> {
        matches!(c, Cell::UsedHas(_)).then_some(Cell::Used)
    }

    fn combine(_c: &Cell, a: &Val, b: &Val) -> Val {
        match (a, b) {
            (Val::Used(a), Val::Used(b)) => Val::Used(Used {
                all: b.all,
                added: a.added | b.added,
            }),
            _ => b.clone(),
        }
    }

    fn digest(&self) -> Version {
        let mut h = crate::hash::StableHasher::new();
        (self.pc, self.raw, &self.vars, &self.labels, self.used).hash(&mut h);
        for (id, l) in self.lines.iter() {
            (id, l).hash(&mut h);
        }
        Version(h.finish128())
    }

    fn render(e: &Effect, cx: &mut LinkCtx<'_, Self>, out: &mut Vec<u8>) {
        match e {
            Effect::Int(v) => push_int(out, *v),
            Effect::Sym(a) => match cx.hole(a.hole) {
                Some(Val::Int(v)) => push_int(out, v.wrapping_add(a.k)),
                _ => out.push(b'?'),
            },
            Effect::Text { text, from } => {
                out.extend_from_slice(&text.as_bytes()[*from as usize..]);
            }
            Effect::Ref(l) => match &cx.final_state.labels[usize::from(*l)] {
                Some(Val::Int(v)) => push_int(out, *v),
                None => out.extend_from_slice(b"??"),
                Some(_) => out.push(b'?'),
            },
            Effect::Alloc => {
                out.extend_from_slice(b"obj ");
                let n = cx.alloc() + 1;
                push_int(out, i64::try_from(n).unwrap_or(i64::MAX));
            }
            Effect::Used(u) => {
                out.extend_from_slice(b"used ");
                push_int(out, i64::from(*u));
            }
            Effect::Has(h) => out.extend_from_slice(if *h { b"has 1" } else { b"has 0" }),
        }
        out.push(b'\n');
    }

    fn allocs(e: &Effect) -> u64 {
        u64::from(matches!(e, Effect::Alloc))
    }

    fn index(c: &Cell) -> Option<u32> {
        Some(match *c {
            Cell::Var(x) => u32::from(x),
            Cell::Label(l) => 256 + u32::from(l),
            Cell::Mode => 512,
            Cell::Used => 513,
            Cell::UsedHas(g) => 514 + u32::from(g),
            Cell::Line(id) => 1024 + id,
        })
    }

    fn hole_value(c: &Cell, h: Hole) -> Option<Val> {
        matches!(c, Cell::Var(_)).then_some(Val::Sym(Affine { hole: h, k: 0 }))
    }

    fn subst_value(v: &Val, env: &dyn Fn(Hole) -> Option<Val>) -> Val {
        subst(v, env)
    }

    fn subst_effect(e: &Effect, env: &dyn Fn(Hole) -> Option<Val>) -> Effect {
        match e {
            Effect::Sym(a) => match env(a.hole) {
                Some(Val::Int(i)) => Effect::Int(i.wrapping_add(a.k)),
                _ => e.clone(),
            },
            _ => e.clone(),
        }
    }

    fn subst_state(&mut self, env: &dyn Fn(Hole) -> Option<Val>) {
        for v in self.vars.iter_mut().chain(self.labels.iter_mut()).flatten() {
            *v = subst(v, env);
        }
    }
}

impl Split for Stub {
    /// Every `n / parts`-th line, moved to a blank line just after it when
    /// there is one close by.
    fn split(&self, parts: usize) -> Vec<Option<LineId>> {
        let mut ids = Vec::new();
        let mut at = self.pc;
        while let Some(id) = at {
            ids.push(id);
            at = self.lines[&id].next;
        }
        let mut out = alloc::vec![self.pc];
        if ids.is_empty() {
            return out;
        }
        let size = ids.len().div_ceil(parts.max(1)).max(1);
        let mut i = size;
        while i < ids.len() {
            let window = (i..(i + size / 4).min(ids.len()))
                .find(|&j| j > 0 && self.lines[&ids[j - 1]].text.trim().is_empty());
            let j = window.unwrap_or(i);
            out.push(Some(ids[j]));
            i = j + size;
        }
        out
    }

    /// A line that still exists (line ids are stable under edits, which
    /// keep the order of the lines they do not touch).
    fn still_at(&self, b: &Option<LineId>) -> bool {
        b.is_some_and(|id| self.lines.get(id).is_some())
    }
}

/// An independent interpreter: the reference output.
#[must_use]
pub fn oracle(p: &Program) -> Vec<u8> {
    enum Piece {
        Bytes(Vec<u8>),
        Ref(u8),
    }
    /// The interpreter's state.
    struct St {
        vars: [i64; VARS],
        labels: [Option<i64>; LABELS],
        raw: bool,
        allocs: i64,
        used: u32,
    }
    fn run(s: &Stmt, text: &str, st: &mut St, out: &mut Vec<Piece>) {
        let St {
            vars,
            labels,
            raw,
            allocs,
            used,
        } = st;
        let mut line = |b: &[u8]| {
            let mut v = b.to_vec();
            v.push(b'\n');
            out.push(Piece::Bytes(v));
        };
        match *s {
            Stmt::Blank | Stmt::Work(_) => {}
            Stmt::Section | Stmt::Verbatim | Stmt::Bad => line(text.as_bytes()),
            Stmt::Set(x, n) => vars[usize::from(x)] = n,
            Stmt::Inc(x) => vars[usize::from(x)] = vars[usize::from(x)].wrapping_add(1),
            Stmt::Add(x, y) => {
                vars[usize::from(x)] = vars[usize::from(x)].wrapping_add(vars[usize::from(y)]);
            }
            Stmt::Mul(x, n) => vars[usize::from(x)] = vars[usize::from(x)].wrapping_mul(n),
            Stmt::Copy(x, y) => vars[usize::from(x)] = vars[usize::from(y)],
            Stmt::Emit(x) => {
                let mut b = Vec::new();
                push_int(&mut b, vars[usize::from(x)]);
                line(&b);
            }
            Stmt::Say(from) => line(&text.as_bytes()[from as usize..]),
            Stmt::Label(l, x) => labels[usize::from(l)] = Some(vars[usize::from(x)]),
            Stmt::Ref(l) => out.push(Piece::Ref(l)),
            Stmt::Alloc => {
                *allocs += 1;
                let mut b = b"obj ".to_vec();
                push_int(&mut b, *allocs);
                line(&b);
            }
            Stmt::Raw => *raw = true,
            Stmt::EndRaw => *raw = false,
            Stmt::If(x, n, ref inner) => {
                if vars[usize::from(x)] > n {
                    run(inner, text, st, out);
                }
            }
            Stmt::Mark(g) => *used |= 1 << g,
            Stmt::Fonts => {
                let mut b = b"used ".to_vec();
                push_int(&mut b, i64::from(*used));
                line(&b);
            }
            Stmt::Has(g) => line(if *used >> g & 1 != 0 {
                b"has 1"
            } else {
                b"has 0"
            }),
        }
    }
    let mut st = St {
        vars: [0; VARS],
        labels: [None; LABELS],
        raw: false,
        allocs: 0,
        used: 0,
    };
    let mut pieces = Vec::new();
    for id in p.ids() {
        let text = p.line(id);
        let s = parse(text, st.raw);
        run(&s, text, &mut st, &mut pieces);
    }
    let mut out = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Bytes(b) => out.extend_from_slice(&b),
            Piece::Ref(l) => {
                match st.labels[usize::from(l)] {
                    Some(v) => push_int(&mut out, v),
                    None => out.extend_from_slice(b"??"),
                }
                out.push(b'\n');
            }
        }
    }
    out
}

/// Seeded generators of programs and edits, for the property tests and
/// the measurements.
pub mod generate {
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    use super::{Cell, Program};

    /// `SplitMix64`.
    #[derive(Clone, Debug)]
    pub struct Rng(pub u64);

    impl Rng {
        pub fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        /// Uniform in `0..n`.
        ///
        /// # Panics
        ///
        /// If `n` is 0.
        pub fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next_u64() % n as u64).expect("below n")
        }
    }

    /// What generated programs look like.
    #[derive(Clone, Debug)]
    pub struct Shape {
        pub vars: usize,
        pub labels: usize,
        /// Largest `work` statement (0: none).
        pub work: u32,
        /// Whether `raw` blocks appear.
        pub raw: bool,
        /// Whether `mark` and `fonts` (the accumulator) appear.
        pub acc: bool,
    }

    impl Default for Shape {
        fn default() -> Self {
            Self {
                vars: 8,
                labels: 4,
                work: 0,
                raw: true,
                acc: false,
            }
        }
    }

    /// One random line.
    ///
    /// # Panics
    ///
    /// If the shape has no variables or labels.
    pub fn statement(r: &mut Rng, s: &Shape) -> String {
        let x = |r: &mut Rng| format!("x{}", r.below(s.vars));
        let l = |r: &mut Rng| format!("L{}", r.below(s.labels));
        let n = |r: &mut Rng| i64::try_from(r.below(21)).expect("small") - 5;
        if s.acc && r.below(6) == 0 {
            // (few glyphs, so edits add and take back the same ones)
            return match r.below(4) {
                0 => String::from("fonts"),
                1 => format!("has {}", r.below(6)),
                _ => format!("mark {}", r.below(6)),
            };
        }
        match r.below(100) {
            0..12 => format!("set {} {}", x(r), n(r)),
            12..28 => format!("inc {}", x(r)),
            28..36 => format!("add {} {}", x(r), x(r)),
            36..41 => format!("copy {} {}", x(r), x(r)),
            92..98 if s.work > 0 => format!("work {}", 1 + r.below(s.work as usize)),
            41..55 | 92..98 => format!("emit {}", x(r)),
            55..60 => format!("say word{} and more", r.below(100)),
            60..65 => format!("label {} {}", l(r), x(r)),
            65..70 => format!("ref {}", l(r)),
            70..75 => String::from("alloc"),
            75..82 => {
                let inner = match r.below(4) {
                    0 => format!("emit {}", x(r)),
                    1 => format!("inc {}", x(r)),
                    2 => format!("mul {} 3", x(r)),
                    _ => format!("say taken {}", r.below(10)),
                };
                format!("if {} > {} then {inner}", x(r), n(r))
            }
            82..85 => format!("mul {} {}", x(r), n(r)),
            85..91 => String::new(),
            91 => String::from("section"),
            98 if s.raw => String::from("raw"),
            99 if s.raw => String::from("endraw"),
            _ => String::from("say plain"),
        }
    }

    /// A random program of `len` lines.
    pub fn program(r: &mut Rng, len: usize, s: &Shape) -> String {
        let mut out = String::new();
        for _ in 0..len {
            out.push_str(&statement(r, s));
            out.push('\n');
        }
        out
    }

    /// A random edit: replace, insert or delete one line. Returns the
    /// cells changed.
    pub fn edit(r: &mut Rng, p: &mut Program, s: &Shape) -> Vec<Cell> {
        let ids = p.ids();
        if ids.is_empty() {
            return p.insert_after(None, &statement(r, s));
        }
        let id = ids[r.below(ids.len())];
        match r.below(4) {
            0 | 1 => p.replace(id, &statement(r, s)),
            2 => {
                let after = if r.below(20) == 0 { None } else { Some(id) };
                p.insert_after(after, &statement(r, s))
            }
            _ => p.delete(id),
        }
    }
}
