//! (observe) The persisted graph's feasibility probe
//! (docs/spec-persisted-graph.md §9). After an evaluation, with the line
//! quiet, it encodes what a persisted graph would hold of the engine: the
//! values the records name (each once, a sequence by its rope's nodes),
//! the nodes (operation, text, parent, claim, state) and their last and
//! prior records (output, definitions, contributions, reads, sources,
//! placements, claim, effect, parked state), the units' order, the
//! environment, the paths and the memo of applications; then decodes the
//! bytes into a fresh value store and records coded as a commit codes
//! them, and checks that every clean node's every read still answers what
//! it saw (the check a loaded graph could make in full).
//!
//! Changes nothing the engine or a later evaluation sees: it reads under
//! the locks a quiet line's readers take and writes only its own buffers.
//! The client's values (`Operation::roots`) are encoded as a section of
//! their own when the caller passes them.

use std::time::Instant;

use crate::Engine;
use crate::api::{Effect, Locality, NodeId, Rel, Step, UnitId};
use crate::node::{Anchor, ReadKind, ReadRec, RunRecord, State};
use crate::values::{FxHashMap, Value, ValueId, Values};

/// What the probe found and how long each part took.
#[derive(Debug, Default, Clone)]
pub struct PersistProbe {
    /// Nodes in the table: live (clean), live (not clean), inert, gone.
    pub nodes: [u64; 4],
    pub units: u64,
    pub segments: u64,
    pub records: u64,
    pub priors: u64,
    pub provisional: u64,
    pub reads: u64,
    /// Reads by kind: field (me, before, after-own, end, final), env,
    /// fold, next, placed, following, hole.
    pub reads_by_kind: [u64; 11],
    pub defs: u64,
    pub contribs: u64,
    pub placed: u64,
    pub sources: u64,
    pub text_values: u64,
    /// The engine's own coding of the records' reads (`Reads::bytes`).
    pub engine_read_bytes: u64,
    /// Distinct values in the table; of them strings, lists, rope nodes.
    pub values: u64,
    pub strs: u64,
    pub str_bytes: u64,
    pub lists: u64,
    pub ropes: u64,
    /// Bytes by section: values, nodes and order, records (without
    /// reads), reads, environment + paths + memo, client values.
    pub section_bytes: [u64; 6],
    pub env: u64,
    pub paths: u64,
    pub memo: u64,
    /// Client roots given, and the values only they reach.
    pub client_roots: u64,
    pub client_values: u64,
    /// Times (ns): encoding; decoding the values into a fresh store;
    /// decoding the records (reads coded as a commit codes them); the
    /// check of every clean node's reads.
    pub encode_ns: u64,
    pub decode_values_ns: u64,
    pub decode_records_ns: u64,
    pub verify_ns: u64,
    pub verify_reads: u64,
    /// Reads whose answer now is not what they saw, by kind (as
    /// `reads_by_kind`), and the nodes with one.
    pub verify_moved: [u64; 11],
    pub verify_moved_nodes: u64,
    /// Decoded records equal to the engine's (definitions, contributions,
    /// reads, output) once mapped back: 0 differing is the round trip.
    pub roundtrip_differ: u64,
    pub bytes: Vec<u8>,
}

impl PersistProbe {
    /// One line of the numbers.
    #[must_use]
    pub fn line(&self) -> String {
        let mb = |b: u64| b as f64 / 1_048_576.0;
        let ms = |n: u64| n as f64 / 1e6;
        let s = &self.section_bytes;
        format!(
            "persist-probe: nodes live {} (+{} not clean) inert {} gone {}; units {} segs {}; records {} (prior {}, provisional {}); \
             reads {} [me {} before {} after {} end {} final {} env {} fold {} next {} placed {} following {} hole {}]; \
             defs {} contribs {} placed {} sources {} text-values {}; engine-read-bytes {:.2} MB; \
             values {} (strs {} {:.2} MB, lists {}, ropes {}); env {} paths {} memo {}; client roots {} new values {}; \
             bytes {:.2} MB [values {:.2} nodes {:.2} records {:.2} reads {:.2} env+paths+memo {:.2} client {:.2}]; \
             encode {:.1} ms, decode values {:.1} ms, decode records {:.1} ms, verify {:.1} ms over {} reads, moved {} in {} nodes {:?}; roundtrip differ {}",
            self.nodes[0],
            self.nodes[1],
            self.nodes[2],
            self.nodes[3],
            self.units,
            self.segments,
            self.records,
            self.priors,
            self.provisional,
            self.reads,
            self.reads_by_kind[0],
            self.reads_by_kind[1],
            self.reads_by_kind[2],
            self.reads_by_kind[3],
            self.reads_by_kind[4],
            self.reads_by_kind[5],
            self.reads_by_kind[6],
            self.reads_by_kind[7],
            self.reads_by_kind[8],
            self.reads_by_kind[9],
            self.reads_by_kind[10],
            self.defs,
            self.contribs,
            self.placed,
            self.sources,
            self.text_values,
            mb(self.engine_read_bytes),
            self.values,
            self.strs,
            mb(self.str_bytes),
            self.lists,
            self.ropes,
            self.env,
            self.paths,
            self.memo,
            self.client_roots,
            self.client_values,
            mb(self.bytes.len() as u64),
            mb(s[0]),
            mb(s[1]),
            mb(s[2]),
            mb(s[3]),
            mb(s[4]),
            mb(s[5]),
            ms(self.encode_ns),
            ms(self.decode_values_ns),
            ms(self.decode_records_ns),
            ms(self.verify_ns),
            self.verify_reads,
            self.verify_moved.iter().sum::<u64>(),
            self.verify_moved_nodes,
            self.verify_moved,
            self.roundtrip_differ,
        )
    }
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

struct Cur<'a>(&'a [u8], usize);

impl Cur<'_> {
    fn u(&mut self) -> u64 {
        let mut x = 0u64;
        let mut s = 0;
        loop {
            let b = self.0[self.1];
            self.1 += 1;
            x |= u64::from(b & 0x7f) << s;
            if b < 0x80 {
                return x;
            }
            s += 7;
        }
    }
    fn b(&mut self) -> u8 {
        let b = self.0[self.1];
        self.1 += 1;
        b
    }
    fn bytes(&mut self, n: usize) -> &[u8] {
        let r = &self.0[self.1..self.1 + n];
        self.1 += n;
        r
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

/// The values' table being written: each distinct value once, its parts
/// (a list's elements, a rope node's halves) before it.
struct Table<'a> {
    v: &'a Values,
    at: FxHashMap<ValueId, u32>,
    out: Vec<u8>,
    n: u32,
    strs: u64,
    str_bytes: u64,
    lists: u64,
    ropes: u64,
}

impl Table<'_> {
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

    fn put(&mut self, x: ValueId) -> u32 {
        if let Some(&i) = self.at.get(&x) {
            return i;
        }
        let mut stack: Vec<(ValueId, bool)> = vec![(x, false)];
        while let Some((y, done)) = stack.pop() {
            if self.at.contains_key(&y) {
                continue;
            }
            let parts = self.parts(y);
            if !done {
                stack.push((y, true));
                for &e in parts.iter().rev() {
                    if !self.at.contains_key(&e) {
                        stack.push((e, false));
                    }
                }
                continue;
            }
            let k = u64::from(self.n);
            let rel = |at: &FxHashMap<ValueId, u32>, e: &ValueId| k - u64::from(at[e]);
            match self.v.seq_split(y) {
                Some(Ok(e)) => {
                    self.out.push(V_UNIT_SEQ);
                    let d = rel(&self.at, &e);
                    put(&mut self.out, d);
                    self.ropes += 1;
                }
                Some(Err((l, r))) => {
                    self.out.push(V_CAT);
                    let (dl, dr) = (rel(&self.at, &l), rel(&self.at, &r));
                    put(&mut self.out, dl);
                    put(&mut self.out, dr);
                    self.ropes += 1;
                }
                None => {
                    let v = self.v;
                    v.with(y, |val| match val {
                        Value::Unit => self.out.push(V_UNIT),
                        Value::Bool(b) => self.out.push(if *b { V_TRUE } else { V_FALSE }),
                        Value::Int(i) => {
                            self.out.push(V_INT);
                            put(&mut self.out, zig(*i));
                        }
                        Value::Str(s) => {
                            self.out.push(V_STR);
                            put(&mut self.out, s.len() as u64);
                            self.out.extend_from_slice(s.as_bytes());
                            self.strs += 1;
                            self.str_bytes += s.len() as u64;
                        }
                        Value::Nil | Value::Seq(_) => self.out.push(V_NIL),
                        Value::List(xs) => {
                            self.out.push(V_LIST);
                            put(&mut self.out, xs.len() as u64);
                            for e in xs.iter() {
                                let d = rel(&self.at, e);
                                put(&mut self.out, d);
                            }
                            self.lists += 1;
                        }
                    });
                }
            }
            self.at.insert(y, self.n);
            self.n += 1;
        }
        self.at[&x]
    }
}

/// A read's kind's code and its slot in `reads_by_kind`.
fn kind_code(k: &ReadKind) -> (u8, usize) {
    match k {
        ReadKind::Field { at, .. } => match at {
            Anchor::Me => (0, 0),
            Anchor::Before(_) => (1, 1),
            Anchor::AfterOwn(_) => (2, 2),
            Anchor::End(_) => (3, 3),
            Anchor::Final => (4, 4),
        },
        ReadKind::Env { .. } => (5, 5),
        ReadKind::Fold { incl, .. } => (if *incl { 7 } else { 6 }, 6),
        ReadKind::Next { .. } => (8, 7),
        ReadKind::Placed { guess, .. } => (if *guess { 10 } else { 9 }, 8),
        ReadKind::Following { .. } => (11, 9),
        ReadKind::Hole => (12, 10),
    }
}

/// A decoded record, as a load would hold it before filing.
struct Lite {
    output: ValueId,
    defs: Vec<(ValueId, u8, ValueId, Option<Rel>)>,
    contribs: Vec<(ValueId, ValueId)>,
    reads: crate::reads::Reads,
    n_reads: usize,
    sources: Vec<u64>,
    placed: Vec<u64>,
}

impl Engine {
    /// (observe) The persisted graph's probe ([`PersistProbe`]); `client`
    /// gives the client's own roots (`Operation::roots`), none for the
    /// engine's alone. Call with the line quiet, between evaluations.
    pub fn persist_probe(&self, client: Option<&mut dyn FnMut(&mut dyn FnMut(ValueId))>) -> PersistProbe {
        let mut p = PersistProbe::default();
        let t0 = Instant::now();
        let v = &self.values;
        let mut t = Table {
            v,
            at: FxHashMap::default(),
            out: Vec::new(),
            n: 0,
            strs: 0,
            str_bytes: 0,
            lists: 0,
            ropes: 0,
        };
        // (the nodes kept: every one not gone, numbered in table order)
        let mut idx: Vec<u64> = vec![0; self.nodes.len()];
        let mut kept = 0u64;
        for i in 0..self.nodes.len() {
            let Some(n) = self.nodes.get(i) else { continue };
            match n.state() {
                State::Gone => p.nodes[3] += 1,
                State::Inert | State::RunningInert => {
                    p.nodes[2] += 1;
                    kept += 1;
                    idx[i] = kept;
                }
                State::Clean => {
                    p.nodes[0] += 1;
                    kept += 1;
                    idx[i] = kept;
                }
                _ => {
                    p.nodes[1] += 1;
                    kept += 1;
                    idx[i] = kept;
                }
            }
        }
        let nref = |n: NodeId| {
            usize::try_from(n.raw())
                .ok()
                .and_then(|i| idx.get(i).copied())
                .unwrap_or(0)
        };
        let onref = |n: Option<NodeId>| n.map_or(0, |n| nref(n) + 1);
        let mut nodes_b: Vec<u8> = Vec::new();
        let mut recs_b: Vec<u8> = Vec::new();
        let mut reads_b: Vec<u8> = Vec::new();
        let mut rec_order: Vec<(usize, bool)> = Vec::new();
        let mut enc_rec =
            |t: &mut Table<'_>, p: &mut PersistProbe, r: &RunRecord, recs_b: &mut Vec<u8>, reads_b: &mut Vec<u8>| {
                let flags = u8::from(r.provisional)
                    | (u8::from(r.claim_all) << 1)
                    | (u8::from(r.told) << 2)
                    | (u8::from(r.state.is_some()) << 3)
                    | (u8::from(r.text.is_some()) << 4);
                recs_b.push(flags);
                let o = t.put(r.output);
                put(recs_b, u64::from(o));
                if let Some(s) = r.state {
                    let s = t.put(s);
                    put(recs_b, u64::from(s));
                }
                put(recs_b, r.defs.len() as u64);
                for d in &r.defs {
                    let f = t.put(d.f.0);
                    let x = t.put(d.value);
                    put(recs_b, u64::from(f));
                    recs_b.push(match d.loc {
                        Locality::Local => 0,
                        Locality::Global => 1,
                        Locality::Scope => 2,
                        Locality::ScopeAt(k) => 3 + k.min(250),
                    });
                    put(recs_b, u64::from(x));
                    match d.rel {
                        None => recs_b.push(0),
                        Some(Rel::Add(c)) => {
                            recs_b.push(1);
                            put(recs_b, zig(c));
                        }
                        Some(Rel::Append(a)) => {
                            recs_b.push(2);
                            let a = t.put(a);
                            put(recs_b, u64::from(a));
                        }
                    }
                }
                put(recs_b, r.contribs.len() as u64);
                for &(f, x) in &r.contribs {
                    let (f, x) = (t.put(f.0), t.put(x));
                    put(recs_b, u64::from(f));
                    put(recs_b, u64::from(x));
                }
                put(recs_b, r.sources.len() as u64);
                for &s in r.sources.iter() {
                    put(recs_b, nref(s) as u64);
                }
                put(recs_b, r.placed.len() as u64);
                for &s in &r.placed {
                    put(recs_b, nref(s) as u64);
                }
                put(recs_b, r.claim as u64);
                match r.effect {
                    Effect::None => recs_b.push(0),
                    Effect::Open => recs_b.push(1),
                    Effect::Close => recs_b.push(2),
                    Effect::Pending => recs_b.push(3),
                    Effect::Scope(b) => {
                        recs_b.push(4);
                        recs_b.extend_from_slice(&[b.closes, b.opens, u8::from(b.before)]);
                    }
                }
                put(recs_b, r.waits.len() as u64);
                for &w in r.waits.iter() {
                    put(recs_b, w as u64);
                }
                p.defs += r.defs.len() as u64;
                p.contribs += r.contribs.len() as u64;
                p.sources += r.sources.len() as u64;
                p.placed += r.placed.len() as u64;
                p.engine_read_bytes += r.reads.bytes() as u64;
                put(reads_b, r.reads.len() as u64);
                for rd in r.reads.iter() {
                    let (code, slot) = kind_code(rd.kind());
                    p.reads_by_kind[slot] += 1;
                    p.reads += 1;
                    reads_b.push(code | (u8::from(rd.filed()) << 4) | (u8::from(rd.seen().is_none()) << 5));
                    if let Some(f) = rd.kind().field() {
                        let f = t.put(f.0);
                        put(reads_b, u64::from(f));
                    }
                    match *rd.kind() {
                        ReadKind::Field {
                            at: Anchor::Before(n) | Anchor::AfterOwn(n),
                            ..
                        } => put(reads_b, nref(n) as u64),
                        ReadKind::Field { at: Anchor::End(u), .. } => put(reads_b, u64::from(u.0)),
                        ReadKind::Fold { from, to, .. } => {
                            put(reads_b, onref(from) as u64);
                            put(reads_b, onref(to) as u64);
                        }
                        ReadKind::Next { after, .. } => put(reads_b, onref(after) as u64),
                        ReadKind::Placed { i, .. } | ReadKind::Following { i } => put(reads_b, i as u64),
                        _ => {}
                    }
                    put(reads_b, u64::from(rd.path().0));
                    if let Some(s) = rd.seen() {
                        let s = t.put(s);
                        put(reads_b, u64::from(s));
                    }
                }
            };
        for i in 0..self.nodes.len() {
            if idx[i] == 0 {
                continue;
            }
            let Some(n) = self.nodes.get(i) else { continue };
            put(&mut nodes_b, u64::from(n.op.load(std::sync::atomic::Ordering::Acquire)));
            put(&mut nodes_b, u64::from(n.unit.0));
            put(&mut nodes_b, onref(n.parent) as u64);
            put(&mut nodes_b, u64::from(n.depth));
            nodes_b.push(n.state() as u8);
            let cl = n.claimer.load(std::sync::atomic::Ordering::Acquire);
            put(&mut nodes_b, if cl == 0 { 0 } else { nref(NodeId(cl - 1)) as u64 + 1 });
            let ce = n.claim_end.load(std::sync::atomic::Ordering::Acquire);
            put(&mut nodes_b, if ce == 0 { 0 } else { nref(NodeId(ce - 1)) as u64 + 1 });
            let text = n.text.read().clone();
            put(&mut nodes_b, text.len() as u64);
            for &x in text.iter() {
                let k = t.put(x);
                put(&mut nodes_b, u64::from(k));
            }
            p.text_values += text.len() as u64;
            let last = n.last.lock().clone();
            let prior = n.prior.lock().clone();
            nodes_b.push(u8::from(last.is_some()) | (u8::from(prior.is_some()) << 1));
            if let Some(r) = last {
                p.records += 1;
                p.provisional += u64::from(r.provisional);
                enc_rec(&mut t, &mut p, &r, &mut recs_b, &mut reads_b);
                rec_order.push((i, false));
            }
            if let Some(r) = prior {
                p.priors += 1;
                enc_rec(&mut t, &mut p, &r, &mut recs_b, &mut reads_b);
                rec_order.push((i, true));
            }
        }
        // (the units' order: each unit's nodes, by number)
        p.units = self.units.len() as u64;
        p.segments = self.order.all().len() as u64;
        for u in 0..self.units.len() {
            let order = self.order(UnitId(u as u32));
            put(&mut nodes_b, order.len() as u64);
            for n in order {
                put(&mut nodes_b, nref(n) as u64);
            }
        }
        // (the environment, the paths, the memo)
        let mut misc: Vec<u8> = Vec::new();
        {
            let env = self.env.values.read();
            p.env = env.len() as u64;
            put(&mut misc, env.len() as u64);
            for (f, x) in env.iter() {
                let f = t.put(f.0);
                put(&mut misc, u64::from(f));
                put(&mut misc, x.map_or(0, |x| u64::from(t.put(x)) + 1));
            }
        }
        p.paths = self.paths.steps.len() as u64;
        put(&mut misc, p.paths);
        for i in 0..self.paths.steps.len() {
            let Some(ss) = self.paths.steps.get(i) else {
                put(&mut misc, 0);
                continue;
            };
            put(&mut misc, ss.len() as u64);
            for s in ss.iter() {
                let (tag, x): (u8, u64) = match *s {
                    Step::Index(k) => (0, u64::from(k)),
                    Step::Last => (1, 0),
                    Step::Len => (2, 0),
                    Step::Key(x) => (3, u64::from(t.put(x))),
                    Step::Has(x) => (4, u64::from(t.put(x))),
                    Step::Proj(j) => (5, u64::from(j.0)),
                    Step::Present => (6, 0),
                    Step::Eq(x) => (7, u64::from(t.put(x))),
                    Step::Cmp(x) => (8, u64::from(t.put(x))),
                    Step::Within(x) => (9, u64::from(t.put(x))),
                    Step::Derived(x) => (10, u64::from(t.put(x))),
                    Step::Member(x) => (11, u64::from(t.put(x))),
                    Step::Each(q) => (12, u64::from(q.0)),
                    Step::Indices(x) => (13, u64::from(t.put(x))),
                    Step::Keys(x) => (14, u64::from(t.put(x))),
                    Step::Rank(x) => (15, u64::from(t.put(x))),
                    Step::Ranks(x) => (16, u64::from(t.put(x))),
                    Step::Distinct => (17, 0),
                };
                misc.push(tag);
                put(&mut misc, x);
            }
        }
        {
            let memo = self.memo.read();
            p.memo = memo.len() as u64;
            put(&mut misc, memo.len() as u64);
            for ((op, args), &x) in memo.iter() {
                put(&mut misc, u64::from(op.0));
                put(&mut misc, args.len() as u64);
                for &a in args.iter() {
                    let a = t.put(a);
                    put(&mut misc, u64::from(a));
                }
                let x = t.put(x);
                put(&mut misc, u64::from(x));
            }
        }
        let engine_values_bytes = t.out.len() as u64;
        // (the client's values: those only its roots reach)
        let mut client_b: Vec<u8> = Vec::new();
        let before = t.n;
        if let Some(c) = client {
            let mut roots = Vec::new();
            c(&mut |x| roots.push(x));
            p.client_roots = roots.len() as u64;
            put(&mut client_b, roots.len() as u64);
            for x in roots {
                let k = t.put(x);
                put(&mut client_b, u64::from(k));
            }
        }
        p.client_values = u64::from(t.n - before);
        p.values = u64::from(t.n);
        p.strs = t.strs;
        p.str_bytes = t.str_bytes;
        p.lists = t.lists;
        p.ropes = t.ropes;
        p.section_bytes = [
            engine_values_bytes,
            nodes_b.len() as u64,
            recs_b.len() as u64,
            reads_b.len() as u64,
            misc.len() as u64,
            client_b.len() as u64 + (t.out.len() as u64 - engine_values_bytes),
        ];
        let mut bytes =
            Vec::with_capacity(t.out.len() + nodes_b.len() + recs_b.len() + reads_b.len() + misc.len() + 64);
        bytes.extend_from_slice(b"PXGRAPH0");
        for sec in [&t.out, &nodes_b, &recs_b, &reads_b, &misc, &client_b] {
            bytes.extend_from_slice(&(sec.len() as u64).to_le_bytes());
            bytes.extend_from_slice(sec);
        }
        p.encode_ns = t0.elapsed().as_nanos() as u64;
        let n_values = t.n as usize;
        let table_at = std::mem::take(&mut t.at);
        drop(t);

        // (decoding: the values into a fresh store, then the records)
        let t1 = Instant::now();
        let fresh = Values::new();
        let mut secs: Vec<&[u8]> = Vec::new();
        {
            let mut o = 8usize;
            for _ in 0..6 {
                let n = u64::from_le_bytes(bytes[o..o + 8].try_into().unwrap_or([0; 8])) as usize;
                secs.push(&bytes[o + 8..o + 8 + n]);
                o += 8 + n;
            }
        }
        let mut vals: Vec<ValueId> = Vec::with_capacity(n_values);
        {
            let mut c = Cur(secs[0], 0);
            while c.1 < c.0.len() {
                let k = vals.len();
                let x = match c.b() {
                    V_UNIT => ValueId::UNIT,
                    V_FALSE => fresh.bool(false),
                    V_TRUE => fresh.bool(true),
                    V_INT => fresh.int(unzig(c.u())),
                    V_STR => {
                        let n = c.u() as usize;
                        let s = std::str::from_utf8(c.bytes(n)).unwrap_or("");
                        fresh.str(s)
                    }
                    V_LIST => {
                        let n = c.u() as usize;
                        let xs: Vec<ValueId> = (0..n).map(|_| vals[k - c.u() as usize]).collect();
                        fresh.list(xs)
                    }
                    V_UNIT_SEQ => {
                        let e = vals[k - c.u() as usize];
                        fresh.seq_unit(e)
                    }
                    V_CAT => {
                        let l = vals[k - c.u() as usize];
                        let r = vals[k - c.u() as usize];
                        fresh.seq_concat(l, r)
                    }
                    _ => ValueId::NIL,
                };
                vals.push(x);
            }
        }
        p.decode_values_ns = t1.elapsed().as_nanos() as u64;
        let t2 = Instant::now();
        let mut lites: Vec<Lite> = Vec::with_capacity(rec_order.len());
        {
            let mut rc = Cur(secs[2], 0);
            let mut dc = Cur(secs[3], 0);
            let val = |k: u64| vals[k as usize];
            for _ in 0..rec_order.len() {
                let flags = rc.b();
                let output = val(rc.u());
                if flags & 8 != 0 {
                    let _ = rc.u();
                }
                let nd = rc.u() as usize;
                let mut defs = Vec::with_capacity(nd);
                for _ in 0..nd {
                    let f = val(rc.u());
                    let loc = rc.b();
                    let x = val(rc.u());
                    let rel = match rc.b() {
                        1 => Some(Rel::Add(unzig(rc.u()))),
                        2 => Some(Rel::Append(val(rc.u()))),
                        _ => None,
                    };
                    defs.push((f, loc, x, rel));
                }
                let nc = rc.u() as usize;
                let contribs: Vec<(ValueId, ValueId)> = (0..nc).map(|_| (val(rc.u()), val(rc.u()))).collect();
                let ns = rc.u() as usize;
                let sources: Vec<u64> = (0..ns).map(|_| rc.u()).collect();
                let np = rc.u() as usize;
                let placed: Vec<u64> = (0..np).map(|_| rc.u()).collect();
                let _claim = rc.u();
                if rc.b() == 4 {
                    let _ = rc.bytes(3);
                }
                let nw = rc.u() as usize;
                for _ in 0..nw {
                    let _ = rc.u();
                }
                // (the reads, coded as a commit codes them; nodes by
                // their load numbers)
                let nr = dc.u() as usize;
                let mut rs: Vec<ReadRec> = Vec::with_capacity(nr);
                for _ in 0..nr {
                    let h = dc.b();
                    let code = h & 15;
                    let f = if code <= 8 {
                        crate::values::Field(val(dc.u()))
                    } else {
                        crate::values::Field(ValueId::UNIT)
                    };
                    let node = |x: u64| NodeId(x);
                    let onode = |x: u64| (x != 0).then(|| NodeId(x - 1));
                    let kind = match code {
                        0 => ReadKind::Field { f, at: Anchor::Me },
                        1 => ReadKind::Field {
                            f,
                            at: Anchor::Before(node(dc.u())),
                        },
                        2 => ReadKind::Field {
                            f,
                            at: Anchor::AfterOwn(node(dc.u())),
                        },
                        3 => ReadKind::Field {
                            f,
                            at: Anchor::End(UnitId(dc.u() as u32)),
                        },
                        4 => ReadKind::Field { f, at: Anchor::Final },
                        5 => ReadKind::Env { f },
                        6 | 7 => {
                            let from = onode(dc.u());
                            let to = onode(dc.u());
                            ReadKind::Fold {
                                f,
                                from,
                                to,
                                incl: code == 7,
                            }
                        }
                        8 => ReadKind::Next {
                            f,
                            after: onode(dc.u()),
                        },
                        9 | 10 => ReadKind::Placed {
                            i: dc.u() as usize,
                            guess: code == 10,
                        },
                        11 => ReadKind::Following { i: dc.u() as usize },
                        _ => ReadKind::Hole,
                    };
                    let path = crate::api::PathId(dc.u() as u32);
                    let seen = if h & 32 != 0 {
                        crate::node::NO_VALUE
                    } else {
                        val(dc.u()).raw()
                    };
                    rs.push(ReadRec::from_parts(kind, path, h & 16 != 0, u64::MAX, seen));
                }
                lites.push(Lite {
                    output,
                    defs,
                    contribs,
                    reads: crate::reads::Reads::new(&rs),
                    n_reads: nr,
                    sources,
                    placed,
                });
            }
        }
        p.decode_records_ns = t2.elapsed().as_nanos() as u64;

        // (the round trip: each decoded record against the engine's, by
        // the values' contents, which the fresh store gives back as ids
        // of its own: compared through the table's numbering)
        let back: FxHashMap<ValueId, ValueId> = table_at.iter().map(|(&old, &k)| (vals[k as usize], old)).collect();
        for (j, &(i, prior)) in rec_order.iter().enumerate() {
            let Some(n) = self.nodes.get(i) else { continue };
            let r = if prior {
                n.prior.lock().clone()
            } else {
                n.last.lock().clone()
            };
            let Some(r) = r else { continue };
            let l = &lites[j];
            let ok = back.get(&l.output) == Some(&r.output)
                && l.defs.len() == r.defs.len()
                && l.defs
                    .iter()
                    .zip(&r.defs)
                    .all(|(a, d)| back.get(&a.0) == Some(&d.f.0) && back.get(&a.2) == Some(&d.value))
                && l.contribs.len() == r.contribs.len()
                && l.contribs
                    .iter()
                    .zip(&r.contribs)
                    .all(|(a, c)| back.get(&a.0) == Some(&c.0.0) && back.get(&a.1) == Some(&c.1))
                && l.n_reads == r.reads.len()
                && l.reads.len() == r.reads.len()
                && l.sources.len() == r.sources.len()
                && l.placed.len() == r.placed.len();
            if !ok {
                p.roundtrip_differ += 1;
            }
        }
        drop(lites);

        // (the check a loaded graph could make in full: every clean
        // node's every read answered now, against what it saw)
        let t3 = Instant::now();
        for i in 0..self.nodes.len() {
            let Some(n) = self.nodes.get(i) else { continue };
            if n.state() != State::Clean {
                continue;
            }
            let Some(r) = n.run_record() else { continue };
            let mut memo = crate::read::FoldMemo::new();
            let mut moved = false;
            for rd in r.reads.iter() {
                p.verify_reads += 1;
                let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::read::answer_in(self, n, rd.kind(), rd.path(), &mut memo)
                }));
                if a.map_or(true, |a| a != rd.seen()) {
                    p.verify_moved[kind_code(rd.kind()).1] += 1;
                    moved = true;
                }
            }
            p.verify_moved_nodes += u64::from(moved);
        }
        p.verify_ns = t3.elapsed().as_nanos() as u64;
        p.bytes = bytes;
        p
    }

    /// (observe) The client's roots, as a collection takes them
    /// (`Operation::roots`), handed to `keep`.
    pub fn client_roots(&self, keep: &mut dyn FnMut(ValueId)) {
        for i in 0..self.ops.len() {
            if let Some(op) = self.ops.get(i) {
                op.roots(self, keep);
            }
        }
    }
}
