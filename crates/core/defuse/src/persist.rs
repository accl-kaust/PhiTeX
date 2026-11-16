//! The persisted graph, the engine's half (docs/spec-persisted-graph.md
//! §2.1, §4, §7): [`Engine::export`] writes what a collection of the value
//! store keeps of the engine (`Engine::roots`): the nodes, their last and
//! prior records, the units' order, the claims, the paths, the relations
//! learnt, the loop-back values published, the environment, and the
//! values all of these name; [`Engine::import`] reads it into a fresh
//! engine, which then stands as the one exported stood at its quiet point.
//!
//! Nothing derived is written. The segments' definitions, contributions,
//! sums, brackets and fields read, the field and range indexes, the
//! foreign readers, witnesses, pullers and bounded readers are made again
//! by filing each record as a commit files it (`commit::file_record_products`,
//! `commit::file_record_reads`, the commit's own filing functions). Per
//! evaluation state (wakes, run counts, timings, the memo of applications)
//! starts afresh.
//!
//! The bytes: a header (magic, version, the chunks' table with each
//! chunk's length and hash, the header's own hash), then the chunks:
//! values, meta, line, nodes, records. Every value is written once, its
//! parts (a list's elements, a sequence's halves) before it, and named by
//! its number elsewhere. Operations are named by their names (the k-th of
//! a name, `Operation::name`); node, unit and path ids are kept as they
//! were. The whole of the bytes is decoded and checked before anything is
//! installed: a short file, a bad hash, an unknown version, an index out
//! of range or an operation the engine lacks is an error, and the engine
//! is left as it was but for values interned into its store.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::api::{Brackets, Effect, Locality, NodeId, OpId, PathId, ProjId, Rel, Step, UnitId};
use crate::engine::{Engine, PROJ_PATHS, Unit};
use crate::node::{Anchor, DefRec, NO_VALUE, Node, ReadKind, ReadRec, RunRecord, State};
use crate::order::{Pos, SegId};
use crate::read::Reaching;
use crate::segment::Segment;
use crate::values::{Field, FxHashMap, Value, ValueId, Values, hash_bytes};

const MAGIC: &[u8; 8] = b"PXGRAPH\x01";
/// The encoding's version: any change of the bytes' layout bumps it.
const VERSION: u32 = 4;
const HASH_SEED: u64 = 0x7078_6772_6170_6831;

const C_VALUES: u32 = 1;
const C_META: u32 = 2;
const C_LINE: u32 = 3;
const C_NODES: u32 = 4;
const C_RECORDS: u32 = 5;
const CHUNKS: [u32; 5] = [C_VALUES, C_META, C_LINE, C_NODES, C_RECORDS];

/// A refused import: what was wrong with the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError(String);

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "graph refused: {}", self.0)
    }
}

impl std::error::Error for ImportError {}

fn bad(s: impl Into<String>) -> ImportError {
    ImportError(s.into())
}

type R<T> = Result<T, ImportError>;

/// What an import read and installed, and how long each part took.
#[derive(Debug, Default, Clone)]
pub struct ImportStats {
    pub bytes: u64,
    pub values: u64,
    pub nodes: u64,
    pub records: u64,
    pub reads: u64,
    /// Records filed whose reads did not all hold as filed (a `next`
    /// whose search answers otherwise, a read at a node gone): 0 for a
    /// graph saved quiet.
    pub not_holding: u64,
    /// Nanoseconds: checking the bytes, decoding the values into the
    /// store, decoding the rest, installing the line and the nodes,
    /// filing the products, filing the reads.
    pub check_ns: u64,
    pub values_ns: u64,
    pub decode_ns: u64,
    pub install_ns: u64,
    pub products_ns: u64,
    pub reads_ns: u64,
}

/// What [`Engine::import`] gives back.
#[derive(Debug, Default, Clone)]
pub struct Imported {
    /// The roots passed to [`Engine::export`], as values of this engine.
    pub roots: Vec<ValueId>,
    pub stats: ImportStats,
}

/// What [`Engine::verify_reads`] found.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReadCheck {
    pub nodes: u64,
    pub reads: u64,
    /// Reads whose answer now is not what they saw (the engine's own
    /// check, `RunRecord::read_holds`, no shortcut).
    pub failed: u64,
}

fn put(out: &mut Vec<u8>, mut x: u64) {
    while x >= 0x80 {
        out.push((x as u8) | 0x80);
        x >>= 7;
    }
    out.push(x as u8);
}

fn zig(x: i64) -> u64 {
    ((x << 1) ^ (x >> 63)) as u64
}

fn unzig(x: u64) -> i64 {
    ((x >> 1) as i64) ^ -((x & 1) as i64)
}

/// A reader of a chunk: every read checked against its end.
struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cur<'a> {
    fn new(b: &'a [u8]) -> Cur<'a> {
        Cur { b, i: 0 }
    }

    fn u(&mut self) -> R<u64> {
        let mut x = 0u64;
        let mut s = 0u32;
        loop {
            let b = *self.b.get(self.i).ok_or_else(|| bad("a chunk cut short"))?;
            self.i += 1;
            if s == 63 && b > 1 {
                return Err(bad("a number too long"));
            }
            x |= u64::from(b & 0x7f) << s;
            if b < 0x80 {
                return Ok(x);
            }
            s += 7;
            if s > 63 {
                return Err(bad("a number too long"));
            }
        }
    }

    fn u32(&mut self) -> R<u32> {
        u32::try_from(self.u()?).map_err(|_| bad("a number past 32 bits"))
    }

    fn usize(&mut self) -> R<usize> {
        usize::try_from(self.u()?).map_err(|_| bad("a number past usize"))
    }

    /// A count of items each at least `min` bytes long: no more than the
    /// bytes left can hold (no allocation on a count the file cannot back).
    fn count(&mut self, min: usize) -> R<usize> {
        let n = self.usize()?;
        if n.saturating_mul(min.max(1)) > self.b.len() - self.i {
            return Err(bad("a count past the chunk's end"));
        }
        Ok(n)
    }

    fn byte(&mut self) -> R<u8> {
        let b = *self.b.get(self.i).ok_or_else(|| bad("a chunk cut short"))?;
        self.i += 1;
        Ok(b)
    }

    fn bytes(&mut self, n: usize) -> R<&'a [u8]> {
        let end = self.i.checked_add(n).filter(|&e| e <= self.b.len());
        let end = end.ok_or_else(|| bad("a chunk cut short"))?;
        let r = &self.b[self.i..end];
        self.i = end;
        Ok(r)
    }

    fn done(&self, what: &str) -> R<()> {
        if self.i == self.b.len() {
            Ok(())
        } else {
            Err(bad(format!("{what}: bytes past its end")))
        }
    }
}

const V_UNIT: u8 = 0;
const V_FALSE: u8 = 1;
const V_TRUE: u8 = 2;
const V_INT: u8 = 3;
const V_STR: u8 = 4;
const V_LIST: u8 = 5;
const V_NIL: u8 = 6;
const V_UNIT_SEQ: u8 = 7;
const V_CAT: u8 = 8;
const V_BIG: u8 = 9;

/// The values being written: each distinct value once, numbered in the
/// order written, its parts (a list's elements, a rope node's halves)
/// before it and named by their distance back.
struct ValueTable<'a> {
    v: &'a Values,
    at: FxHashMap<ValueId, u32>,
    out: Vec<u8>,
    n: u32,
}

impl ValueTable<'_> {
    fn parts(&self, y: ValueId) -> Vec<ValueId> {
        if let Some(sp) = self.v.seq_split(y) {
            return match sp {
                Ok(x) => vec![x],
                Err((l, r)) => vec![l, r],
            };
        }
        self.v.with(y, |val| match val {
            Value::List(xs) => xs.to_vec(),
            _ => Vec::new(),
        })
    }

    /// The number of `x`, written if new.
    fn put(&mut self, x: ValueId) -> u64 {
        if let Some(&i) = self.at.get(&x) {
            return u64::from(i);
        }
        let mut stack: Vec<(ValueId, bool)> = vec![(x, false)];
        while let Some((y, done)) = stack.pop() {
            if self.at.contains_key(&y) {
                continue;
            }
            if !done {
                stack.push((y, true));
                for e in self.parts(y).into_iter().rev() {
                    if !self.at.contains_key(&e) {
                        stack.push((e, false));
                    }
                }
                continue;
            }
            let k = u64::from(self.n);
            let rel = |at: &FxHashMap<ValueId, u32>, e: &ValueId| k - u64::from(at[e]);
            // (a sequence's id, checked where it is made again; a plain
            // value's place, where it is put: the importing store holds
            // every value under the id it had here, `Values::stable_raw`)
            match self.v.seq_split(y) {
                Some(Ok(e)) => {
                    self.out.push(V_UNIT_SEQ);
                    let d = rel(&self.at, &e);
                    put(&mut self.out, d);
                    put(&mut self.out, self.v.stable_raw(y));
                }
                Some(Err((l, r))) => {
                    self.out.push(V_CAT);
                    let (dl, dr) = (rel(&self.at, &l), rel(&self.at, &r));
                    put(&mut self.out, dl);
                    put(&mut self.out, dr);
                    put(&mut self.out, self.v.stable_raw(y));
                }
                None => {
                    let v = self.v;
                    let place = v.plain_place(y);
                    v.with(y, |val| match val {
                        Value::Unit => self.out.push(V_UNIT),
                        Value::Bool(b) => self.out.push(if *b { V_TRUE } else { V_FALSE }),
                        Value::Int(i) => match place {
                            Some(at) => {
                                self.out.push(V_BIG);
                                put(&mut self.out, at);
                                put(&mut self.out, zig(*i));
                            }
                            None => {
                                self.out.push(V_INT);
                                put(&mut self.out, zig(*i));
                            }
                        },
                        Value::Str(s) => {
                            self.out.push(V_STR);
                            put(&mut self.out, place.unwrap_or(u64::MAX));
                            put(&mut self.out, s.len() as u64);
                            self.out.extend_from_slice(s.as_bytes());
                        }
                        Value::Nil | Value::Seq(_) => self.out.push(V_NIL),
                        Value::List(xs) => {
                            self.out.push(V_LIST);
                            put(&mut self.out, place.unwrap_or(u64::MAX));
                            put(&mut self.out, xs.len() as u64);
                            for e in xs.iter() {
                                let d = rel(&self.at, e);
                                put(&mut self.out, d);
                            }
                        }
                    });
                }
            }
            self.at.insert(y, self.n);
            self.n += 1;
        }
        u64::from(self.at[&x])
    }

    fn opt(&mut self, x: Option<ValueId>) -> u64 {
        x.map_or(0, |x| self.put(x) + 1)
    }
}

/// Decodes the values chunk into `store`: each value's id there, by its
/// number.
fn decode_values(store: &Values, b: &[u8]) -> R<Vec<ValueId>> {
    let mut c = Cur::new(b);
    let mut vals: Vec<ValueId> = Vec::new();
    let back = |vals: &Vec<ValueId>, d: u64| -> R<ValueId> {
        let k = vals.len() as u64;
        if d == 0 || d > k {
            return Err(bad("a value's part out of range"));
        }
        Ok(vals[(k - d) as usize])
    };
    while c.i < b.len() {
        let x = match c.byte()? {
            V_UNIT => ValueId::UNIT,
            V_FALSE => store.bool(false),
            V_TRUE => store.bool(true),
            V_INT => {
                let x = store.int(unzig(c.u()?));
                if store.plain_place(x).is_some() {
                    return Err(bad("a big integer without its place"));
                }
                x
            }
            V_BIG => {
                let at = c.u()?;
                let i = unzig(c.u()?);
                store.place_plain(at, Value::Int(i)).map_err(bad)?
            }
            V_STR => {
                let at = c.u()?;
                let n = c.usize()?;
                let s = std::str::from_utf8(c.bytes(n)?).map_err(|_| bad("a string not UTF-8"))?;
                store.place_plain(at, Value::Str(s.into())).map_err(bad)?
            }
            V_LIST => {
                let at = c.u()?;
                let n = c.count(1)?;
                let mut xs = Vec::with_capacity(n);
                for _ in 0..n {
                    xs.push(back(&vals, c.u()?)?);
                }
                store.place_plain(at, Value::List(xs.into())).map_err(bad)?
            }
            V_NIL => ValueId::NIL,
            V_UNIT_SEQ => {
                let e = back(&vals, c.u()?)?;
                let x = store.seq_unit(e);
                same_seq(store, x, c.u()?)?
            }
            V_CAT => {
                let l = back(&vals, c.u()?)?;
                let r = back(&vals, c.u()?)?;
                let x = store
                    .seq_node_exact(l, r)
                    .ok_or_else(|| bad("a sequence's half not a sequence"))?;
                same_seq(store, x, c.u()?)?
            }
            k => return Err(bad(format!("a value of unknown kind {k}"))),
        };
        vals.push(x);
    }
    Ok(vals)
}

/// Sequence `x` as made again here, checked to have the id it had in the
/// exporting store (`want`, [`Values::stable_raw`]): ids made from
/// elements at the same places are the same, but for a true collision
/// of content hashes that the two stores probed past in another order.
fn same_seq(store: &Values, x: ValueId, want: u64) -> R<ValueId> {
    if store.stable_raw(x) == want {
        Ok(x)
    } else {
        Err(bad("a sequence's id not the one it had"))
    }
}

/// The decoded values, by number.
struct Vals(Vec<ValueId>);

impl Vals {
    fn get(&self, c: &mut Cur<'_>) -> R<ValueId> {
        let k = c.usize()?;
        self.0.get(k).copied().ok_or_else(|| bad("a value number out of range"))
    }

    fn opt(&self, c: &mut Cur<'_>) -> R<Option<ValueId>> {
        match c.usize()? {
            0 => Ok(None),
            k => self
                .0
                .get(k - 1)
                .copied()
                .map(Some)
                .ok_or_else(|| bad("a value number out of range")),
        }
    }

    fn field(&self, c: &mut Cur<'_>) -> R<Field> {
        Ok(Field(self.get(c)?))
    }
}

fn loc_code(l: Locality) -> u8 {
    match l {
        Locality::Local => 0,
        Locality::Global => 1,
        Locality::Scope => 2,
        Locality::ScopeAt(k) => 3u8.saturating_add(k.min(250)),
    }
}

fn loc_of(b: u8) -> Locality {
    match b {
        0 => Locality::Local,
        1 => Locality::Global,
        2 => Locality::Scope,
        k => Locality::ScopeAt(k - 3),
    }
}

fn put_effect(out: &mut Vec<u8>, e: Effect) {
    match e {
        Effect::None => out.push(0),
        Effect::Open => out.push(1),
        Effect::Close => out.push(2),
        Effect::Pending => out.push(3),
        Effect::Scope(b) => {
            out.push(4);
            out.extend_from_slice(&[b.closes, b.opens, u8::from(b.before)]);
        }
    }
}

fn get_effect(c: &mut Cur<'_>) -> R<Effect> {
    Ok(match c.byte()? {
        0 => Effect::None,
        1 => Effect::Open,
        2 => Effect::Close,
        3 => Effect::Pending,
        4 => {
            let b = c.bytes(3)?;
            Effect::Scope(Brackets {
                closes: b[0],
                opens: b[1],
                before: b[2] != 0,
            })
        }
        k => return Err(bad(format!("an effect of unknown kind {k}"))),
    })
}

/// A step's tag and operand (a value's number, an index, a path or a
/// projection).
fn put_step(out: &mut Vec<u8>, t: &mut ValueTable<'_>, s: &Step) {
    let (tag, x): (u8, u64) = match *s {
        Step::Index(k) => (0, u64::from(k)),
        Step::Last => (1, 0),
        Step::Len => (2, 0),
        Step::Key(x) => (3, t.put(x)),
        Step::Has(x) => (4, t.put(x)),
        Step::Proj(j) => (5, u64::from(j.0)),
        Step::Present => (6, 0),
        Step::Eq(x) => (7, t.put(x)),
        Step::Cmp(x) => (8, t.put(x)),
        Step::Within(x) => (9, t.put(x)),
        Step::Derived(x) => (10, t.put(x)),
        Step::Member(x) => (11, t.put(x)),
        Step::Each(q) => (12, u64::from(q.0)),
        Step::Indices(x) => (13, t.put(x)),
        Step::Keys(x) => (14, t.put(x)),
        Step::Rank(x) => (15, t.put(x)),
        Step::Ranks(x) => (16, t.put(x)),
        Step::Distinct => (17, 0),
    };
    out.push(tag);
    put(out, x);
}

fn get_step(c: &mut Cur<'_>, vals: &Vals, n_paths: usize, n_projs: usize) -> R<Step> {
    let tag = c.byte()?;
    let val = |c: &mut Cur<'_>| vals.get(c);
    Ok(match tag {
        0 => Step::Index(c.u32()?),
        1 => {
            c.u()?;
            Step::Last
        }
        2 => {
            c.u()?;
            Step::Len
        }
        3 => Step::Key(val(c)?),
        4 => Step::Has(val(c)?),
        5 => {
            let j = c.usize()?;
            if j >= n_projs {
                return Err(bad("a path's projection the engine lacks"));
            }
            Step::Proj(ProjId(u16::try_from(j).map_err(|_| bad("a projection id"))?))
        }
        6 => {
            c.u()?;
            Step::Present
        }
        7 => Step::Eq(val(c)?),
        8 => Step::Cmp(val(c)?),
        9 => Step::Within(val(c)?),
        10 => Step::Derived(val(c)?),
        11 => Step::Member(val(c)?),
        12 => {
            let q = c.usize()?;
            if q >= n_paths {
                return Err(bad("a path's inner path out of range"));
            }
            Step::Each(PathId(u32::try_from(q).map_err(|_| bad("a path id"))?))
        }
        13 => Step::Indices(val(c)?),
        14 => Step::Keys(val(c)?),
        15 => Step::Rank(val(c)?),
        16 => Step::Ranks(val(c)?),
        17 => {
            c.u()?;
            Step::Distinct
        }
        k => return Err(bad(format!("a step of unknown kind {k}"))),
    })
}

/// A read's kind as its code (low 4 bits of its head), and whether its
/// entry codes a field.
fn read_code(k: &ReadKind) -> u8 {
    match k {
        ReadKind::Field { at, .. } => match at {
            Anchor::Me => 0,
            Anchor::Before(_) => 1,
            Anchor::AfterOwn(_) => 2,
            Anchor::End(_) => 3,
            Anchor::Final => 4,
        },
        ReadKind::Env { .. } => 5,
        ReadKind::Fold { incl, .. } => {
            if *incl {
                7
            } else {
                6
            }
        }
        ReadKind::Next { .. } => 8,
        ReadKind::Placed { guess, .. } => {
            if *guess {
                10
            } else {
                9
            }
        }
        ReadKind::Following { .. } => 11,
        ReadKind::Hole => 12,
    }
}

// The nodes' flags.
const NF_COMMITTED: u32 = 1;
const NF_CLAIM_ALL: u32 = 1 << 1;
const NF_FOLLOWS: u32 = 1 << 2;
const NF_SETTLE_NOW: u32 = 1 << 3;
const NF_REFUTED: u32 = 1 << 4;
const NF_STATE_STALE: u32 = 1 << 5;
const NF_MOVES: u32 = 1 << 6;
const NF_FRONT: u32 = 1 << 7;
const NF_RUN_CHAINED: u32 = 1 << 8;
const NF_LAST: u32 = 1 << 9;
const NF_PRIOR: u32 = 1 << 10;
const NF_PARKED: u32 = 1 << 11;
const NF_EDITED: u32 = 1 << 12;
const NF_MOVED: u32 = 1 << 13;
/// A run of it stopped at the settling layer's frontier
/// (`Node::held_frontier`): learnt, never cleared, it decides whether its
/// next run begins before the frontier passes it. Dropped, an edit after
/// a load ran such a node once more than the build that saved it, the
/// run thrown away at the frontier (acm's resume node at -j1).
const NF_HELD_FRONTIER: u32 = 1 << 14;
/// A slot of the nodes' table reserved and never set (a placement a
/// commit made ready and did not place).
const EMPTY_SLOT: u8 = 0xff;

// The records' flags.
const RF_PROVISIONAL: u8 = 1;
const RF_CLAIM_ALL: u8 = 1 << 1;
const RF_TOLD: u8 = 1 << 2;
const RF_STATE: u8 = 1 << 3;
/// Its text: none (0), its node's (1), its own (2), in bits 4-5.
const RF_TEXT_SHIFT: u32 = 4;

/// A record decoded, its values in the importing store.
struct DRec {
    output: ValueId,
    defs: Vec<DefRec>,
    /// Per definition, the value its segment holds now where that is
    /// another (a relation put right in place since the commit).
    filed: Vec<Option<ValueId>>,
    contribs: Vec<(Field, ValueId)>,
    reads: Vec<ReadRec>,
    sources: Vec<NodeId>,
    placed: Vec<NodeId>,
    claim: usize,
    claim_all: bool,
    effect: Effect,
    provisional: bool,
    state: Option<ValueId>,
    waits: Vec<usize>,
    text: Option<(OpId, Arc<[ValueId]>)>,
    told: bool,
}

/// A node decoded.
struct DNode {
    state: State,
    op: OpId,
    unit: UnitId,
    parent: Option<NodeId>,
    depth: u32,
    pos: u64,
    claimer: u64,
    claim_end: u64,
    anchor: u64,
    slot: u8,
    rank: u64,
    effect: u32,
    flags: u32,
    parked_on: Option<Field>,
    full_ns: u64,
    run_start: u64,
    text: Arc<[ValueId]>,
    last: Option<DRec>,
    prior: Option<DRec>,
}

/// Everything decoded, checked, before any of it is installed.
struct Decoded {
    paths: Vec<Option<Vec<Step>>>,
    member_paths: Vec<((Field, PathId, ValueId, PathId), PathId)>,
    shapes: Vec<((OpId, Field), i64)>,
    settle_exact: Vec<Field>,
    marked: Vec<NodeId>,
    movers: Vec<NodeId>,
    settled: Vec<crate::api::Settled>,
    finals: Vec<(Field, Option<Reaching>, bool, bool)>,
    env: Vec<(Field, Option<ValueId>)>,
    env_provided: Vec<Field>,
    env_changes: u64,
    scalars: [u64; 6],
    has_finals: bool,
    roots: Vec<ValueId>,
    units: Vec<(u64, u64, u64)>,
    segs: Vec<(u64, u64, UnitId)>,
    nodes: Vec<Option<DNode>>,
}

/// How many nodes [`Engine::export_until`] encodes between two asks of
/// its `stop` (a node's records: some microseconds to encode).
const STOP_EVERY: usize = 32;

impl Engine {
    /// The engine as a quiet point leaves it, as bytes
    /// ([`Engine::import`] reads them into a fresh engine), with `roots`,
    /// values the caller holds, written too (given back by the import as
    /// the importing engine's). Call between evaluations, with the line
    /// quiet: every live node clean or taken by a claim.
    ///
    /// # Panics
    ///
    /// Never: nothing stops it ([`Engine::export_until`]).
    #[must_use]
    pub fn export(&self, roots: &[ValueId]) -> Vec<u8> {
        self.export_until(roots, &|| false).expect("an export nothing stops")
    }

    /// [`Engine::export`], given up once `stop` says so (asked between
    /// nodes, every [`STOP_EVERY`]): none then. A host that saves the
    /// graph while nothing is asked (the wasm core, at idle) stops it for
    /// the next edit, which then waits for no save.
    #[must_use]
    pub fn export_until(&self, roots: &[ValueId], stop: &dyn Fn() -> bool) -> Option<Vec<u8>> {
        if stop() {
            return None;
        }
        let mut t = ValueTable {
            v: &self.values,
            at: FxHashMap::default(),
            out: Vec::new(),
            n: 0,
        };
        // (meta)
        let mut meta: Vec<u8> = Vec::new();
        put(&mut meta, self.ops.len() as u64);
        for i in 0..self.ops.len() {
            let name = self.ops.get(i).map_or("", |o| o.name());
            put(&mut meta, name.len() as u64);
            meta.extend_from_slice(name.as_bytes());
        }
        put(&mut meta, self.projections.len() as u64);
        let n_paths = self.paths.steps.len();
        put(&mut meta, n_paths as u64);
        for i in PROJ_PATHS.min(n_paths)..n_paths {
            match self.paths.steps.get(i) {
                None => meta.push(0),
                Some(ss) => {
                    meta.push(1);
                    put(&mut meta, ss.len() as u64);
                    for s in ss.iter() {
                        put_step(&mut meta, &mut t, s);
                    }
                }
            }
        }
        {
            let mp = self.member_paths.read();
            put(&mut meta, mp.len() as u64);
            for (&(f, p, x, q), &r) in mp.iter() {
                put(&mut meta, t.put(f.0));
                put(&mut meta, u64::from(p.0));
                put(&mut meta, t.put(x));
                put(&mut meta, u64::from(q.0));
                put(&mut meta, u64::from(r.0));
            }
        }
        {
            let sh = self.shapes.read();
            put(&mut meta, sh.len() as u64);
            for (&(op, f), &c) in sh.iter() {
                put(&mut meta, u64::from(op.0));
                put(&mut meta, t.put(f.0));
                put(&mut meta, zig(c));
            }
        }
        {
            let se = self.settle_exact.read();
            put(&mut meta, se.len() as u64);
            for f in se.iter() {
                put(&mut meta, t.put(f.0));
            }
        }
        let ids = |meta: &mut Vec<u8>, xs: Vec<NodeId>| {
            put(meta, xs.len() as u64);
            for x in xs {
                put(meta, x.raw());
            }
        };
        let mut marked: Vec<NodeId> = self.marked.read().iter().copied().collect();
        marked.sort_unstable();
        ids(&mut meta, marked);
        let mut movers: Vec<NodeId> = self.movers.lock().iter().copied().collect();
        movers.sort_unstable();
        ids(&mut meta, movers);
        {
            let st = self.settled.lock();
            put(&mut meta, st.len() as u64);
            for x in st.iter() {
                put(&mut meta, x.node.raw());
                put(&mut meta, x.version);
                put(&mut meta, t.put(x.output));
            }
        }
        let finals = self.ranges.finals_state();
        put(&mut meta, finals.len() as u64);
        for (f, p, dirty, pending) in finals {
            put(&mut meta, t.put(f.0));
            meta.push(u8::from(p.is_some()) | (u8::from(dirty) << 1) | (u8::from(pending) << 2));
            if let Some(p) = p {
                put(&mut meta, t.opt(p.value));
                put(&mut meta, p.by.map_or(0, |b| b.raw() + 1));
                meta.push(u8::from(p.provisional));
            }
        }
        {
            let env = self.env.values.read();
            let provided = self.env.provided.read();
            put(&mut meta, env.len() as u64);
            for (f, x) in env.iter() {
                put(&mut meta, t.put(f.0));
                put(&mut meta, t.opt(*x));
                // (the provider's answer, or the client's setting)
                meta.push(u8::from(provided.contains(f)));
            }
            put(&mut meta, self.env.changes.load(Ordering::Acquire));
        }
        for x in [
            self.epoch.load(Ordering::Acquire),
            self.final_gen.load(Ordering::Acquire),
            self.quiets.load(Ordering::Acquire),
            self.version.load(Ordering::Acquire),
            self.window.load(Ordering::Acquire),
            self.window_released.load(Ordering::Acquire),
        ] {
            put(&mut meta, x);
        }
        meta.push(u8::from(self.has_finals.load(Ordering::Acquire)));
        put(&mut meta, roots.len() as u64);
        for &x in roots {
            put(&mut meta, t.put(x));
        }
        // (the line: units, then segments in order)
        let mut line: Vec<u8> = Vec::new();
        put(&mut line, self.units.len() as u64);
        for i in 0..self.units.len() {
            let u = self.units.get(i).expect("a unit made");
            put(&mut line, u.start.0);
            put(&mut line, u.end.0);
            put(&mut line, u.last.load(Ordering::Acquire));
        }
        let segs = self.order.all();
        put(&mut line, segs.len() as u64);
        for &sid in &segs {
            let s = self.seg(sid);
            let hi = s.inner.read().hi;
            put(&mut line, s.lo.0);
            put(&mut line, hi.0);
            put(&mut line, u64::from(s.unit.0));
        }
        // (the nodes, and their records)
        let mut nodes: Vec<u8> = Vec::new();
        let mut recs: Vec<u8> = Vec::new();
        put(&mut nodes, self.nodes.len() as u64);
        for i in 0..self.nodes.len() {
            if i % STOP_EVERY == 0 && stop() {
                return None;
            }
            let Some(n) = self.nodes.get(i) else {
                nodes.push(EMPTY_SLOT);
                continue;
            };
            let state = n.state();
            nodes.push(state as u8);
            let ld = |a: &std::sync::atomic::AtomicBool| a.load(Ordering::Acquire);
            let last = n.last.lock().clone();
            let prior = n.prior.lock().clone();
            let parked = *n.parked_on.lock();
            let mut flags = 0u32;
            for (on, bit) in [
                (n.serial.load(Ordering::Acquire) > 0, NF_COMMITTED),
                (ld(&n.claim_all), NF_CLAIM_ALL),
                (ld(&n.follows), NF_FOLLOWS),
                (ld(&n.settle_now), NF_SETTLE_NOW),
                (ld(&n.refuted), NF_REFUTED),
                (ld(&n.state_stale), NF_STATE_STALE),
                (ld(&n.moves), NF_MOVES),
                (ld(&n.front), NF_FRONT),
                (ld(&n.run_chained), NF_RUN_CHAINED),
                (last.is_some(), NF_LAST),
                (prior.is_some(), NF_PRIOR),
                (parked.is_some(), NF_PARKED),
                (ld(&n.edited), NF_EDITED),
                (ld(&n.moved), NF_MOVED),
                (ld(&n.held_frontier), NF_HELD_FRONTIER),
            ] {
                if on {
                    flags |= bit;
                }
            }
            put(&mut nodes, u64::from(flags));
            put(&mut nodes, u64::from(n.op().0));
            put(&mut nodes, u64::from(n.unit.0));
            put(&mut nodes, n.parent.map_or(0, |p| p.raw() + 1));
            put(&mut nodes, u64::from(n.depth));
            put(&mut nodes, n.pos_hint().0);
            put(&mut nodes, n.claimer.load(Ordering::Acquire));
            put(&mut nodes, n.claim_end.load(Ordering::Acquire));
            put(&mut nodes, n.anchor.load(Ordering::Acquire));
            nodes.push(n.slot());
            put(&mut nodes, n.rank());
            put(&mut nodes, u64::from(n.effect.load(Ordering::Acquire)));
            if let Some(f) = parked {
                put(&mut nodes, t.put(f.0));
            }
            put(&mut nodes, n.full_ns.load(Ordering::Acquire));
            put(&mut nodes, n.run_start.load(Ordering::Acquire));
            let text = n.args();
            if state == State::Gone {
                put(&mut nodes, 0);
            } else {
                put(&mut nodes, text.len() as u64);
                for &x in text.iter() {
                    put(&mut nodes, t.put(x));
                }
            }
            // (a relation's value as its segment holds it now, where the
            // node's definitions are filed there)
            let filed_now = |f: Field| -> Option<ValueId> {
                if state != State::Clean {
                    return None;
                }
                let s = self.seg(n.seg_hint());
                let inner = s.inner.read();
                let p = n.pos_hint();
                if inner.nodes.get(&p) != Some(&n.id) {
                    return None;
                }
                inner
                    .defs
                    .get(&f)
                    .and_then(|m| m.get(&p))
                    .filter(|d| d.by == n.id && !d.multi)
                    .map(|d| d.value)
            };
            if let Some(r) = &last {
                encode_record(&mut recs, &mut t, r, n, Some(&filed_now));
            }
            if let Some(r) = &prior {
                encode_record(&mut recs, &mut t, r, n, None);
            }
        }
        // (the bytes: header, table, chunks)
        let chunks: [(u32, &[u8]); 5] = [
            (C_VALUES, &t.out),
            (C_META, &meta),
            (C_LINE, &line),
            (C_NODES, &nodes),
            (C_RECORDS, &recs),
        ];
        let head_len = 8 + 4 + 4 + 8 + chunks.len() * (4 + 8 + 8) + 8;
        let total = head_len + chunks.iter().map(|c| c.1.len()).sum::<usize>();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        out.extend_from_slice(&(total as u64).to_le_bytes());
        for (tag, b) in &chunks {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&(b.len() as u64).to_le_bytes());
            out.extend_from_slice(&hash_bytes(HASH_SEED, b).to_le_bytes());
        }
        let h = hash_bytes(HASH_SEED, &out);
        out.extend_from_slice(&h.to_le_bytes());
        for (_, b) in &chunks {
            out.extend_from_slice(b);
        }
        debug_assert_eq!(out.len(), total);
        Some(out)
    }

    /// Reads a graph written by [`Engine::export`] into this engine, which
    /// must be fresh (no unit, node or path made) and hold the operations,
    /// projections, accumulators, append and environment provider the
    /// exporting engine held (operations are found by name). Everything
    /// is decoded and checked first; any fault is an error and the engine
    /// is left without nodes. Then the line, the nodes and their records
    /// are put in place and every record filed as its commit filed it:
    /// the engine stands as the exporting one stood at its quiet point, an
    /// evaluation with nothing changed runs nothing, and an edit runs what
    /// it runs there.
    ///
    /// # Errors
    ///
    /// The bytes refused: cut short, a hash or the version wrong, an index
    /// out of range, an operation the engine lacks; or the engine not
    /// fresh.
    pub fn import(&self, bytes: &[u8]) -> Result<Imported, ImportError> {
        let t0 = Instant::now();
        if self.nodes.len() != 0 || self.units.len() != 0 || self.segs.len() != 0 {
            return Err(bad("the engine is not fresh"));
        }
        let chunks = check(bytes)?;
        let mut stats = ImportStats {
            bytes: bytes.len() as u64,
            check_ns: ns(t0),
            ..ImportStats::default()
        };
        let t1 = Instant::now();
        let vals = Vals(decode_values(&self.values, chunks[0])?);
        stats.values = vals.0.len() as u64;
        stats.values_ns = ns(t1);
        let t2 = Instant::now();
        let d = self.decode(&vals, &chunks)?;
        stats.decode_ns = ns(t2);
        let roots = d.roots.clone();
        self.install(d, &mut stats);
        Ok(Imported { roots, stats })
    }

    /// The meta, line, nodes and records chunks decoded and checked.
    fn decode(&self, vals: &Vals, chunks: &[&[u8]; 5]) -> R<Decoded> {
        // (meta)
        let mut c = Cur::new(chunks[1]);
        let n_ops = c.count(1)?;
        let mut names: FxHashMap<&str, Vec<OpId>> = FxHashMap::default();
        for i in 0..self.ops.len() {
            if let Some(o) = self.ops.get(i) {
                names
                    .entry(o.name())
                    .or_default()
                    .push(OpId(u32::try_from(i).map_err(|_| bad("an op id"))?));
            }
        }
        let mut seen_of: FxHashMap<String, usize> = FxHashMap::default();
        let mut ops: Vec<OpId> = Vec::with_capacity(n_ops);
        for _ in 0..n_ops {
            let k = c.usize()?;
            let name = std::str::from_utf8(c.bytes(k)?).map_err(|_| bad("an operation's name not UTF-8"))?;
            let j = seen_of.entry(name.to_owned()).or_default();
            let op = names.get(name).and_then(|v| v.get(*j)).copied().ok_or_else(|| {
                bad(format!(
                    "operation {name:?} not registered (the {}th of its name)",
                    *j + 1
                ))
            })?;
            *j += 1;
            ops.push(op);
        }
        let op_of = |c: &mut Cur<'_>| -> R<OpId> {
            let k = c.usize()?;
            ops.get(k)
                .copied()
                .ok_or_else(|| bad("an operation number out of range"))
        };
        let n_projs = c.usize()?;
        if n_projs != self.projections.len() {
            return Err(bad(format!(
                "{n_projs} projections saved, {} registered",
                self.projections.len()
            )));
        }
        let n_paths = c.usize()?;
        if n_paths < PROJ_PATHS {
            return Err(bad("fewer paths than the built-in ones"));
        }
        let mut paths: Vec<Option<Vec<Step>>> = Vec::with_capacity(n_paths.saturating_sub(PROJ_PATHS));
        for _ in PROJ_PATHS..n_paths {
            match c.byte()? {
                0 => paths.push(None),
                1 => {
                    let k = c.count(2)?;
                    let mut ss = Vec::with_capacity(k);
                    for _ in 0..k {
                        ss.push(get_step(&mut c, vals, n_paths, n_projs)?);
                    }
                    paths.push(Some(ss));
                }
                _ => return Err(bad("a path's mark")),
            }
        }
        // (paths this engine made already: the same ones, the same ids)
        for (i, ss) in paths.iter().enumerate() {
            let id = PROJ_PATHS + i;
            if id >= self.paths.steps.len() {
                break;
            }
            if self.paths.steps.get(id).map(|x| x.to_vec()) != *ss {
                return Err(bad(format!("path {id} made already, unlike the saved one")));
            }
        }
        let path = |c: &mut Cur<'_>| -> R<PathId> {
            let k = c.usize()?;
            if k >= n_paths {
                return Err(bad("a path number out of range"));
            }
            Ok(PathId(u32::try_from(k).map_err(|_| bad("a path id"))?))
        };
        let k = c.count(5)?;
        let mut member_paths = Vec::with_capacity(k);
        for _ in 0..k {
            let f = vals.field(&mut c)?;
            let p = path(&mut c)?;
            let x = vals.get(&mut c)?;
            let q = path(&mut c)?;
            let r = path(&mut c)?;
            member_paths.push(((f, p, x, q), r));
        }
        let k = c.count(3)?;
        let mut shapes = Vec::with_capacity(k);
        for _ in 0..k {
            let op = op_of(&mut c)?;
            let f = vals.field(&mut c)?;
            shapes.push(((op, f), unzig(c.u()?)));
        }
        let k = c.count(1)?;
        let mut settle_exact = Vec::with_capacity(k);
        for _ in 0..k {
            settle_exact.push(vals.field(&mut c)?);
        }
        let raw_ids = |c: &mut Cur<'_>| -> R<Vec<u64>> {
            let k = c.count(1)?;
            (0..k).map(|_| c.u()).collect()
        };
        let marked_raw = raw_ids(&mut c)?;
        let movers_raw = raw_ids(&mut c)?;
        let k = c.count(3)?;
        let mut settled_raw = Vec::with_capacity(k);
        for _ in 0..k {
            settled_raw.push((c.u()?, c.u()?, vals.get(&mut c)?));
        }
        let k = c.count(2)?;
        let mut finals_raw = Vec::with_capacity(k);
        for _ in 0..k {
            let f = vals.field(&mut c)?;
            let fl = c.byte()?;
            let p = if fl & 1 != 0 {
                let value = vals.opt(&mut c)?;
                let by = c.u()?;
                let provisional = c.byte()? != 0;
                Some((value, by, provisional))
            } else {
                None
            };
            finals_raw.push((f, p, fl & 2 != 0, fl & 4 != 0));
        }
        let k = c.count(2)?;
        let mut env = Vec::with_capacity(k);
        let mut env_provided = Vec::new();
        for _ in 0..k {
            let f = vals.field(&mut c)?;
            env.push((f, vals.opt(&mut c)?));
            if c.byte()? != 0 {
                env_provided.push(f);
            }
        }
        let env_changes = c.u()?;
        let mut scalars = [0u64; 6];
        for x in &mut scalars {
            *x = c.u()?;
        }
        let has_finals = c.byte()? != 0;
        let k = c.count(1)?;
        let mut roots = Vec::with_capacity(k);
        for _ in 0..k {
            roots.push(vals.get(&mut c)?);
        }
        c.done("the meta chunk")?;
        // (the line)
        let mut c = Cur::new(chunks[2]);
        let k = c.count(3)?;
        let mut units = Vec::with_capacity(k);
        for _ in 0..k {
            let (start, end, last) = (c.u()?, c.u()?, c.u()?);
            if start >= end {
                return Err(bad("a unit's range empty"));
            }
            units.push((start, end, last));
        }
        let n_units = units.len();
        let unit = |c: &mut Cur<'_>| -> R<UnitId> {
            let u = c.usize()?;
            if u >= n_units {
                return Err(bad("a unit number out of range"));
            }
            Ok(UnitId(u32::try_from(u).map_err(|_| bad("a unit id"))?))
        };
        let k = c.count(3)?;
        let mut segs = Vec::with_capacity(k);
        let mut prev_hi = 0u64;
        for _ in 0..k {
            let (lo, hi) = (c.u()?, c.u()?);
            let u = unit(&mut c)?;
            if lo >= hi || lo < prev_hi {
                return Err(bad("segments out of order"));
            }
            prev_hi = hi;
            segs.push((lo, hi, u));
        }
        c.done("the line chunk")?;
        // (the nodes)
        let mut c = Cur::new(chunks[3]);
        let mut rc = Cur::new(chunks[4]);
        let n_nodes = c.count(1)?;
        let node_id = |x: u64| -> R<NodeId> {
            if (x as usize) < n_nodes {
                Ok(NodeId::from_raw(x))
            } else {
                Err(bad("a node number out of range"))
            }
        };
        let opt_node = |x: u64| -> R<Option<NodeId>> {
            match x {
                0 => Ok(None),
                x => node_id(x - 1).map(Some),
            }
        };
        let mut nodes: Vec<Option<DNode>> = Vec::with_capacity(n_nodes);
        for _ in 0..n_nodes {
            let st = c.byte()?;
            if st == EMPTY_SLOT {
                nodes.push(None);
                continue;
            }
            let state = match st {
                0 => State::Clean,
                4 => State::Inert,
                5 => State::Gone,
                // (a graph is saved quiet: no node runs or waits to)
                s => return Err(bad(format!("a node in state {s}, not quiet"))),
            };
            let flags = c.u32()?;
            let op = op_of(&mut c)?;
            let u = unit(&mut c)?;
            let parent = opt_node(c.u()?)?;
            let depth = c.u32()?;
            let pos = c.u()?;
            let claimer = c.u()?;
            let claim_end = c.u()?;
            let anchor = c.u()?;
            opt_node(claimer)?;
            opt_node(claim_end)?;
            opt_node(anchor)?;
            let slot = c.byte()?;
            let rank = c.u()?;
            let effect = c.u32()?;
            let parked_on = if flags & NF_PARKED != 0 {
                Some(vals.field(&mut c)?)
            } else {
                None
            };
            let full_ns = c.u()?;
            let run_start = c.u()?;
            let k = c.count(1)?;
            let mut text = Vec::with_capacity(k);
            for _ in 0..k {
                text.push(vals.get(&mut c)?);
            }
            let text: Arc<[ValueId]> = text.into();
            let rec = |rc: &mut Cur<'_>| -> R<DRec> {
                decode_record(rc, vals, &op_of, &path, &node_id, &opt_node, n_units, op, &text)
            };
            let last = if flags & NF_LAST != 0 {
                Some(rec(&mut rc)?)
            } else {
                None
            };
            let prior = if flags & NF_PRIOR != 0 {
                Some(rec(&mut rc)?)
            } else {
                None
            };
            if state == State::Clean && last.as_ref().is_none_or(|r| r.provisional) {
                return Err(bad("a clean node with no record of a run"));
            }
            nodes.push(Some(DNode {
                state,
                op,
                unit: u,
                parent,
                depth,
                pos,
                claimer,
                claim_end,
                anchor,
                slot,
                rank,
                effect,
                flags,
                parked_on,
                full_ns,
                run_start,
                text,
                last,
                prior,
            }));
        }
        c.done("the nodes chunk")?;
        rc.done("the records chunk")?;
        // (every live node on a segment of its unit, one to a label)
        let mut at: FxHashMap<u64, usize> = FxHashMap::default();
        for (i, n) in nodes.iter().enumerate() {
            let Some(n) = n else { continue };
            if n.state == State::Gone {
                continue;
            }
            let s = seg_of(&segs, n.pos).ok_or_else(|| bad(format!("node {i} at a label no segment holds")))?;
            if segs[s].2 != n.unit {
                return Err(bad(format!("node {i} on a segment of another unit")));
            }
            if at.insert(n.pos, i).is_some() {
                return Err(bad(format!("node {i} at a label another holds")));
            }
            if n.parent
                .is_some_and(|p| nodes.get(p.raw() as usize).is_none_or(Option::is_none))
            {
                return Err(bad(format!("node {i}'s parent never made")));
            }
        }
        let ids = |xs: Vec<u64>| -> R<Vec<NodeId>> { xs.into_iter().map(node_id).collect() };
        let finals = finals_raw
            .into_iter()
            .map(|(f, p, dirty, pending)| {
                let p = match p {
                    None => None,
                    Some((value, by, provisional)) => Some(Reaching {
                        value,
                        by: opt_node(by)?,
                        provisional,
                    }),
                };
                Ok((f, p, dirty, pending))
            })
            .collect::<R<Vec<_>>>()?;
        Ok(Decoded {
            paths,
            member_paths,
            shapes,
            settle_exact,
            marked: ids(marked_raw)?,
            movers: ids(movers_raw)?,
            settled: settled_raw
                .into_iter()
                .map(|(n, version, output)| {
                    Ok(crate::api::Settled {
                        node: node_id(n)?,
                        version,
                        output,
                    })
                })
                .collect::<R<Vec<_>>>()?,
            finals,
            env,
            env_provided,
            env_changes,
            scalars,
            has_finals,
            roots,
            units,
            segs,
            nodes,
        })
    }

    /// Puts what was decoded in place, then files every clean node's
    /// record: its products first, every one, then its reads (a read's
    /// filing looks at what others define and contribute).
    fn install(&self, d: Decoded, stats: &mut ImportStats) {
        let t0 = Instant::now();
        // (the paths, with the ids they had)
        {
            let mut by = self.paths.by_steps.write();
            for (i, ss) in d.paths.into_iter().enumerate() {
                let id = PROJ_PATHS + i;
                if id < self.paths.steps.len() {
                    continue;
                }
                let k = self.paths.steps.reserve();
                debug_assert_eq!(k, id);
                if let Some(ss) = ss {
                    let b: Box<[Step]> = ss.into();
                    self.paths.steps.set(k, b.clone());
                    by.insert(b, PathId(u32::try_from(k).expect("path ids")));
                }
            }
        }
        self.member_paths.write().extend(d.member_paths);
        {
            let mut sh = self.shapes.write();
            sh.extend(d.shapes);
            self.shapes_n.store(sh.len(), Ordering::Release);
        }
        for f in d.settle_exact {
            self.mark_settled(f);
        }
        {
            let mut env = self.env.values.write();
            env.extend(d.env);
            self.env.provided.write().extend(d.env_provided);
            self.env.changes.store(d.env_changes, Ordering::Release);
        }
        // (the units and segments)
        for (start, end, last) in d.units {
            let i = self.units.reserve();
            self.units.set(
                i,
                Unit {
                    start: Pos(start),
                    end: Pos(end),
                    last: std::sync::atomic::AtomicU64::new(last),
                },
            );
        }
        let first_seg = self.segs.reserve_n(d.segs.len());
        let mut order = Vec::with_capacity(d.segs.len());
        for (k, &(lo, hi, u)) in d.segs.iter().enumerate() {
            let sid = SegId(u32::try_from(first_seg + k).expect("segment ids"));
            self.segs
                .set(sid.0 as usize, Arc::new(Segment::new(sid, u, Pos(lo), Pos(hi))));
            order.push((Pos(lo), sid));
        }
        self.order.insert_many(order);
        // (the nodes, each at its id; the live ones on their segments)
        let n = d.nodes.len();
        let first = self.nodes.reserve_n(n);
        debug_assert_eq!(first, 0);
        let mut on_seg: Vec<Vec<(Pos, NodeId, OpId, bool)>> = vec![Vec::new(); d.segs.len()];
        let mut recs: Vec<(NodeId, Option<DRec>, Option<DRec>)> = Vec::new();
        let mut edited: Vec<NodeId> = Vec::new();
        for (i, dn) in d.nodes.into_iter().enumerate() {
            let Some(dn) = dn else { continue };
            let id = NodeId::from_raw(i as u64);
            let k = seg_of(&d.segs, dn.pos);
            let sid = k.map_or(SegId(0), |k| SegId(u32::try_from(first_seg + k).expect("segment ids")));
            let node = Node::new(
                id,
                dn.unit,
                dn.op,
                dn.text.to_vec(),
                dn.parent,
                dn.depth,
                sid,
                Pos(dn.pos),
            );
            let fl = |b: u32| dn.flags & b != 0;
            let committed = fl(NF_COMMITTED);
            let serial = u32::from(committed);
            node.serial.store(serial, Ordering::Release);
            node.filed_floor.store(serial, Ordering::Release);
            node.claimer.store(dn.claimer, Ordering::Release);
            node.claim_end.store(dn.claim_end, Ordering::Release);
            node.claim_all.store(fl(NF_CLAIM_ALL), Ordering::Release);
            node.follows.store(fl(NF_FOLLOWS), Ordering::Release);
            node.settle_now.store(fl(NF_SETTLE_NOW), Ordering::Release);
            node.refuted.store(fl(NF_REFUTED), Ordering::Release);
            node.state_stale.store(fl(NF_STATE_STALE), Ordering::Release);
            node.moves.store(fl(NF_MOVES), Ordering::Release);
            node.front.store(fl(NF_FRONT), Ordering::Release);
            node.run_chained.store(fl(NF_RUN_CHAINED), Ordering::Release);
            node.edited.store(fl(NF_EDITED), Ordering::Release);
            node.moved.store(fl(NF_MOVED), Ordering::Release);
            node.held_frontier.store(fl(NF_HELD_FRONTIER), Ordering::Release);
            // (a node gone has left the line: it claims nothing, `commit::release_leaving`)
            *node.claim_mx.lock() = dn.state == State::Gone;
            if fl(NF_EDITED) {
                edited.push(id);
            }
            node.anchor.store(dn.anchor, Ordering::Release);
            node.slot.store(dn.slot, Ordering::Release);
            node.rank.store(dn.rank, Ordering::Release);
            node.effect.store(dn.effect, Ordering::Release);
            *node.parked_on.lock() = dn.parked_on;
            node.full_ns.store(dn.full_ns, Ordering::Release);
            node.run_start.store(dn.run_start, Ordering::Release);
            node.run_began.store(dn.run_start, Ordering::Release);
            node.set_state(dn.state);
            if dn.state != State::Gone
                && let Some(k) = k
            {
                on_seg[k].push((Pos(dn.pos), id, dn.op, committed));
            }
            self.nodes.set(i, Arc::new(node));
            if dn.last.is_some() || dn.prior.is_some() {
                recs.push((id, dn.last, dn.prior));
            }
        }
        for (k, mut xs) in on_seg.into_iter().enumerate() {
            xs.sort_unstable_by_key(|x| x.0);
            let seg = self.seg(SegId(u32::try_from(first_seg + k).expect("segment ids")));
            let mut inner = seg.inner.write();
            for (p, id, op, committed) in xs {
                inner.nodes.insert(p, id);
                inner.by_op.entry(op).or_default().push(id);
                if !committed {
                    inner.holes += 1;
                }
            }
        }
        {
            let mut m = self.marked.write();
            m.extend(d.marked);
            self.marked_n.store(m.len(), Ordering::Release);
        }
        {
            let mut m = self.movers.lock();
            m.extend(d.movers);
            self.movers_n.store(m.len() as u64, Ordering::Release);
        }
        {
            let mut m = self.edited.lock();
            m.extend(edited);
            self.edited_n.store(m.len() as u64, Ordering::Release);
        }
        self.settled.lock().extend(d.settled);
        stats.nodes = n as u64;
        stats.install_ns = ns(t0);
        // (the products: each clean node's brackets, definitions and
        // contributions, a segment's under one hold of its lock)
        let t1 = Instant::now();
        let clean = |id: NodeId| self.node(id).state() == State::Clean;
        let mut changed: FxHashMap<NodeId, Vec<Field>> = FxHashMap::default();
        {
            let mut by_seg: FxHashMap<SegId, Vec<usize>> = FxHashMap::default();
            for (j, (id, last, _)) in recs.iter().enumerate() {
                if last.is_some() && clean(*id) {
                    by_seg.entry(self.node(*id).seg_hint()).or_default().push(j);
                }
            }
            let mut segs: Vec<(SegId, Vec<usize>)> = by_seg.into_iter().collect();
            segs.sort_unstable_by_key(|x| x.0);
            for (sid, js) in segs {
                let seg = self.seg(sid);
                let mut inner = seg.inner.write();
                for j in js {
                    let (id, last, _) = &recs[j];
                    let r = last.as_ref().expect("a record");
                    let node = self.node(*id);
                    let defs: Vec<DefRec> = r
                        .defs
                        .iter()
                        .zip(&r.filed)
                        .map(|(d, f)| DefRec {
                            value: f.unwrap_or(d.value),
                            ..*d
                        })
                        .collect();
                    let ch =
                        crate::commit::file_record_products(self, seg, &mut inner, node, &defs, &r.contribs, r.effect);
                    changed.insert(*id, ch);
                }
            }
        }
        stats.products_ns = ns(t1);
        // (the records, made with the counts filing left)
        let scope_gen = self.scope_gen.load(Ordering::Acquire);
        let moves_gen = self.moves_gen.load(Ordering::Acquire);
        let make = |r: DRec, serial: u32| -> Arc<RunRecord> {
            Arc::new(RunRecord {
                serial,
                output: r.output,
                defs: r.defs,
                contribs: r.contribs,
                reads: crate::reads::Reads::new(&r.reads),
                sources: r.sources.into_boxed_slice(),
                placed: r.placed,
                claim: r.claim,
                claim_all: r.claim_all,
                effect: r.effect,
                provisional: r.provisional,
                state: r.state,
                waits: r.waits.into_boxed_slice(),
                scope_gen,
                moves_gen,
                text: r.text,
                told: r.told,
            })
        };
        let mut filed: Vec<(NodeId, Arc<RunRecord>)> = Vec::new();
        for (id, last, prior) in recs {
            let node = self.node(id);
            let serial = node.serial.load(Ordering::Acquire);
            stats.records += u64::from(last.is_some()) + u64::from(prior.is_some());
            if let Some(r) = last {
                stats.reads += r.reads.len() as u64;
                let rec = make(r, serial);
                if node.state() == State::Clean {
                    filed.push((id, rec.clone()));
                }
                *node.last.lock() = Some(rec);
            }
            if let Some(r) = prior {
                *node.prior.lock() = Some(make(r, 0));
            }
        }
        // (the reads, every record's products in place)
        let t2 = Instant::now();
        for (id, rec) in &filed {
            let ch = changed.remove(id).unwrap_or_default();
            if !crate::commit::file_record_reads(self, self.node(*id), rec, &ch) {
                stats.not_holding += 1;
            }
        }
        // (the loop-back values as they were published, not as the
        // filing marked them)
        self.ranges.clear_finals_marks();
        for (f, p, dirty, pending) in d.finals {
            self.ranges.set_finals_state(f, p, dirty, pending);
        }
        self.has_finals.store(
            d.has_finals || self.has_finals.load(Ordering::Acquire),
            Ordering::Release,
        );
        let [epoch, final_gen, quiets, version, window, released] = d.scalars;
        self.epoch.store(epoch, Ordering::Release);
        self.final_gen.store(final_gen, Ordering::Release);
        self.quiets.store(quiets, Ordering::Release);
        self.version.store(version, Ordering::Release);
        self.window.store(window, Ordering::Release);
        self.window_released.store(released, Ordering::Release);
        self.holds_on.store(epoch > 0, Ordering::Release);
        stats.reads_ns = ns(t2);
    }

    /// Every clean node's every read asked as the engine asks a record's
    /// read (`RunRecord::read_holds`, no shortcut): a quiet line, or one
    /// just imported, has none failing (`PHITEX_GRAPH_VERIFY`).
    #[must_use]
    pub fn verify_reads(&self) -> ReadCheck {
        let mut out = ReadCheck::default();
        for i in 0..self.nodes.len() {
            let Some(n) = self.nodes.get(i) else { continue };
            if n.state() != State::Clean {
                continue;
            }
            let Some(r) = n.run_record() else { continue };
            out.nodes += 1;
            let mut memo = crate::node::HoldMemo::default();
            for rd in r.reads.iter() {
                out.reads += 1;
                let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    r.read_holds(self, n, &rd, true, &mut memo)
                }))
                .unwrap_or(false);
                if !ok {
                    out.failed += 1;
                }
            }
        }
        out
    }

    /// (tests) The line as its segments hold it, a line each: segments,
    /// nodes, brackets, definitions and contributions, with values by
    /// their content ([`Values::digest`]): two engines that hold the same
    /// line give the same lines, whatever their stores' ids.
    #[doc(hidden)]
    #[must_use]
    pub fn filing_lines(&self) -> Vec<String> {
        let v = &self.values;
        let dg = |x: ValueId| v.digest(x);
        let rel = |r: Option<Rel>| match r {
            None => "-".to_owned(),
            Some(Rel::Add(c)) => format!("+{c}"),
            Some(Rel::Append(x)) => format!("append {:x}", dg(x)),
        };
        let mut out = Vec::new();
        for sid in self.order.all() {
            let s = self.seg(sid);
            let inner = s.inner.read();
            out.push(format!(
                "seg {} {} unit {} depth {} {}",
                s.lo.0, inner.hi.0, s.unit.0, inner.depth_net, inner.depth_min
            ));
            for (p, id) in inner.nodes.iter() {
                let n = self.node(*id);
                out.push(format!(
                    "  node {} at {} state {:?} serial>0 {} claimer {} end {}",
                    id.raw(),
                    p.0,
                    n.state(),
                    n.committed(),
                    n.claimer.load(Ordering::Acquire),
                    n.claim_end.load(Ordering::Acquire)
                ));
            }
            for (p, (id, b)) in inner.brackets.iter() {
                out.push(format!(
                    "  brackets {} by {} {} {} {}",
                    p.0,
                    id.raw(),
                    b.closes,
                    b.opens,
                    b.before
                ));
            }
            let mut defs: Vec<String> = inner
                .defs
                .iter()
                .flat_map(|(f, m)| {
                    m.iter()
                        .map(|(p, d)| {
                            format!(
                                "  def {:x} at {} by {} value {:x} depth {} rel {} prov {} multi {}",
                                dg(f.0),
                                p.0,
                                d.by.raw(),
                                dg(d.value),
                                d.depth,
                                rel(d.rel),
                                d.provisional,
                                d.multi
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            defs.sort();
            out.extend(defs);
            let mut cs: Vec<String> = inner
                .contribs
                .iter()
                .flat_map(|(f, m)| {
                    m.iter()
                        .map(|(p, x)| format!("  contrib {:x} at {} value {:x}", dg(f.0), p.0, dg(*x)))
                        .collect::<Vec<_>>()
                })
                .collect();
            cs.sort();
            out.extend(cs);
        }
        let mut fs: Vec<String> = self
            .ranges
            .finals_state()
            .into_iter()
            .map(|(f, p, dirty, pending)| {
                format!(
                    "final {:x} {:?} {dirty} {pending}",
                    dg(f.0),
                    p.map(|p| (p.value.map(dg), p.by.map(NodeId::raw), p.provisional))
                )
            })
            .collect();
        fs.sort();
        out.extend(fs);
        out
    }
}

/// The segment (by its place in `segs`) whose range holds `pos`.
fn seg_of(segs: &[(u64, u64, UnitId)], pos: u64) -> Option<usize> {
    let k = segs.partition_point(|s| s.0 <= pos);
    let k = k.checked_sub(1)?;
    (pos < segs[k].1).then_some(k)
}

fn ns(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// The header checked, every chunk's hash with it: the chunks in their
/// order.
fn check(bytes: &[u8]) -> R<[&[u8]; 5]> {
    let n = CHUNKS.len();
    let head_len = 8 + 4 + 4 + 8 + n * (4 + 8 + 8) + 8;
    if bytes.len() < 8 || &bytes[..8] != MAGIC {
        return Err(bad("not a graph (its magic)"));
    }
    if bytes.len() < head_len {
        return Err(bad("the header cut short"));
    }
    let le32 = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap_or([0; 4]));
    let le64 = |i: usize| u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap_or([0; 8]));
    let version = le32(8);
    if version != VERSION {
        return Err(bad(format!("version {version}, not {VERSION}")));
    }
    if le32(12) as usize != n {
        return Err(bad("the chunks' count"));
    }
    if le64(16) != bytes.len() as u64 {
        return Err(bad(format!("{} bytes, the header says {}", bytes.len(), le64(16))));
    }
    if hash_bytes(HASH_SEED, &bytes[..head_len - 8]) != le64(head_len - 8) {
        return Err(bad("the header's hash"));
    }
    let mut out: [&[u8]; 5] = [&[]; 5];
    let mut at = head_len;
    for (k, &tag) in CHUNKS.iter().enumerate() {
        let e = 24 + k * 20;
        if le32(e) != tag {
            return Err(bad("the chunks' table"));
        }
        let len = usize::try_from(le64(e + 4)).map_err(|_| bad("a chunk's length"))?;
        let end = at
            .checked_add(len)
            .filter(|&x| x <= bytes.len())
            .ok_or_else(|| bad("a chunk past the end"))?;
        let b = &bytes[at..end];
        if hash_bytes(HASH_SEED, b) != le64(e + 12) {
            return Err(bad(format!("chunk {tag}'s hash")));
        }
        out[k] = b;
        at = end;
    }
    if at != bytes.len() {
        return Err(bad("bytes past the last chunk"));
    }
    Ok(out)
}

/// A record written: flags, output, state, definitions (a relation's
/// value as its segment holds it now, `filed_now`, where that is
/// another), contributions, sources, placements, claim, effect, waits,
/// text, reads.
fn encode_record(
    out: &mut Vec<u8>,
    t: &mut ValueTable<'_>,
    r: &RunRecord,
    n: &Node,
    filed_now: Option<&dyn Fn(Field) -> Option<ValueId>>,
) {
    let text_kind: u8 = match &r.text {
        None => 0,
        Some((op, args)) if *op == n.op() && **args == *n.args() => 1,
        Some(_) => 2,
    };
    out.push(
        u8::from(r.provisional)
            | (u8::from(r.claim_all) << 1)
            | (u8::from(r.told) << 2)
            | (u8::from(r.state.is_some()) << 3)
            | (text_kind << RF_TEXT_SHIFT),
    );
    put(out, t.put(r.output));
    if let Some(s) = r.state {
        put(out, t.put(s));
    }
    put(out, r.defs.len() as u64);
    for d in &r.defs {
        put(out, t.put(d.f.0));
        out.push(loc_code(d.loc));
        put(out, t.put(d.value));
        match d.rel {
            None => out.push(0),
            Some(Rel::Add(c)) => {
                out.push(1);
                put(out, zig(c));
            }
            Some(Rel::Append(a)) => {
                out.push(2);
                put(out, t.put(a));
            }
        }
        if d.rel.is_some() {
            let now = filed_now.and_then(|g| g(d.f)).filter(|&x| x != d.value);
            put(out, t.opt(now));
        }
    }
    put(out, r.contribs.len() as u64);
    for &(f, x) in &r.contribs {
        put(out, t.put(f.0));
        put(out, t.put(x));
    }
    put(out, r.sources.len() as u64);
    for s in r.sources.iter() {
        put(out, s.raw());
    }
    put(out, r.placed.len() as u64);
    for s in &r.placed {
        put(out, s.raw());
    }
    put(out, r.claim as u64);
    put_effect(out, r.effect);
    put(out, r.waits.len() as u64);
    for &w in r.waits.iter() {
        put(out, w as u64);
    }
    if text_kind == 2
        && let Some((op, args)) = &r.text
    {
        put(out, u64::from(op.0));
        put(out, args.len() as u64);
        for &x in args.iter() {
            put(out, t.put(x));
        }
    }
    put(out, r.reads.len() as u64);
    for rd in r.reads.iter() {
        let code = read_code(rd.kind());
        out.push(code | (u8::from(rd.filed()) << 4) | (u8::from(rd.seen_raw() == NO_VALUE) << 5));
        if let Some(f) = rd.kind().field() {
            put(out, t.put(f.0));
        }
        match *rd.kind() {
            ReadKind::Field {
                at: Anchor::Before(a) | Anchor::AfterOwn(a),
                ..
            } => put(out, a.raw()),
            ReadKind::Field { at: Anchor::End(u), .. } => put(out, u64::from(u.0)),
            ReadKind::Fold { from, to, .. } => {
                put(out, from.map_or(0, |x| x.raw() + 1));
                put(out, to.map_or(0, |x| x.raw() + 1));
            }
            ReadKind::Next { after, .. } => put(out, after.map_or(0, |x| x.raw() + 1)),
            ReadKind::Placed { i, .. } | ReadKind::Following { i } => put(out, i as u64),
            _ => {}
        }
        put(out, u64::from(rd.path().0));
        if let Some(s) = rd.seen() {
            put(out, t.put(s));
        }
    }
}

/// A record read back ([`encode_record`]).
#[allow(clippy::too_many_arguments)]
fn decode_record(
    c: &mut Cur<'_>,
    vals: &Vals,
    op_of: &dyn Fn(&mut Cur<'_>) -> R<OpId>,
    path: &dyn Fn(&mut Cur<'_>) -> R<PathId>,
    node_id: &dyn Fn(u64) -> R<NodeId>,
    opt_node: &dyn Fn(u64) -> R<Option<NodeId>>,
    n_units: usize,
    op: OpId,
    text: &Arc<[ValueId]>,
) -> R<DRec> {
    let flags = c.byte()?;
    let output = vals.get(c)?;
    let state = if flags & RF_STATE != 0 {
        Some(vals.get(c)?)
    } else {
        None
    };
    let k = c.count(4)?;
    let mut defs = Vec::with_capacity(k);
    let mut filed = Vec::with_capacity(k);
    for _ in 0..k {
        let f = vals.field(c)?;
        let loc = loc_of(c.byte()?);
        let value = vals.get(c)?;
        let rel = match c.byte()? {
            0 => None,
            1 => Some(Rel::Add(unzig(c.u()?))),
            2 => Some(Rel::Append(vals.get(c)?)),
            k => return Err(bad(format!("a relation of unknown kind {k}"))),
        };
        filed.push(if rel.is_some() { vals.opt(c)? } else { None });
        defs.push(DefRec { f, loc, value, rel });
    }
    let k = c.count(2)?;
    let mut contribs = Vec::with_capacity(k);
    for _ in 0..k {
        let f = vals.field(c)?;
        contribs.push((f, vals.get(c)?));
    }
    let k = c.count(1)?;
    let sources = (0..k).map(|_| node_id(c.u()?)).collect::<R<Vec<_>>>()?;
    let k = c.count(1)?;
    let placed = (0..k).map(|_| node_id(c.u()?)).collect::<R<Vec<_>>>()?;
    let claim = c.usize()?;
    let effect = get_effect(c)?;
    let k = c.count(1)?;
    let waits = (0..k).map(|_| c.usize()).collect::<R<Vec<_>>>()?;
    let text = match (flags >> RF_TEXT_SHIFT) & 3 {
        0 => None,
        1 => Some((op, text.clone())),
        2 => {
            let o = op_of(c)?;
            let k = c.count(1)?;
            let args = (0..k).map(|_| vals.get(c)).collect::<R<Vec<_>>>()?;
            Some((o, args.into()))
        }
        _ => return Err(bad("a record's text mark")),
    };
    let k = c.count(3)?;
    let mut reads = Vec::with_capacity(k);
    for _ in 0..k {
        let h = c.byte()?;
        let code = h & 15;
        let f = if code <= 8 {
            vals.field(c)?
        } else {
            Field(ValueId::UNIT)
        };
        let kind = match code {
            0 => ReadKind::Field { f, at: Anchor::Me },
            1 => ReadKind::Field {
                f,
                at: Anchor::Before(node_id(c.u()?)?),
            },
            2 => ReadKind::Field {
                f,
                at: Anchor::AfterOwn(node_id(c.u()?)?),
            },
            3 => {
                let u = c.u32()?;
                if u as usize >= n_units {
                    return Err(bad("a read at the end of a unit never made"));
                }
                ReadKind::Field {
                    f,
                    at: Anchor::End(UnitId(u)),
                }
            }
            4 => ReadKind::Field { f, at: Anchor::Final },
            5 => ReadKind::Env { f },
            6 | 7 => ReadKind::Fold {
                f,
                from: opt_node(c.u()?)?,
                to: opt_node(c.u()?)?,
                incl: code == 7,
            },
            8 => ReadKind::Next {
                f,
                after: opt_node(c.u()?)?,
            },
            9 | 10 => ReadKind::Placed {
                i: c.usize()?,
                guess: code == 10,
            },
            11 => ReadKind::Following { i: c.usize()? },
            12 => ReadKind::Hole,
            k => return Err(bad(format!("a read of unknown kind {k}"))),
        };
        let p = path(c)?;
        let seen = if h & 32 != 0 { NO_VALUE } else { vals.get(c)?.raw() };
        reads.push(ReadRec::from_parts(kind, p, h & 16 != 0, u64::MAX, seen));
    }
    Ok(DRec {
        output,
        defs,
        filed,
        contribs,
        reads,
        sources,
        placed,
        claim,
        claim_all: flags & RF_CLAIM_ALL != 0,
        effect,
        provisional: flags & RF_PROVISIONAL != 0,
        state,
        waits,
        text,
        told: flags & RF_TOLD != 0,
    })
}
