//! BibTeX as a program of calls, run again only where a version it read
//! changed (partex's DESIGN 3.16, "Exactly the dirty work").
//!
//! A [`Session`] keeps what its last run did. Each run parses the `.aux`
//! files, the style and the databases again (`READ`'s outputs are the
//! entries: each one's key, type and fields, and the cite order), then
//! runs the style's execution commands as *calls*: one per `EXECUTE`, one
//! per entry for `ITERATE` and `REVERSE`. A call's reads and writes are
//! *slots*:
//! - an entry's field, type and key, and the preamble (`READ`'s
//!   outputs, which a call only reads);
//! - an entry's integer and string variables (`sort.key$`, alpha's
//!   `label`), which only that entry's calls read and write;
//! - the global variables, and the output buffer (the line `write$` has
//!   not ended), which any call reads and writes.
//!
//! Each call has a place: its command, then its entry's key in the order
//! that command iterates (`REVERSE`: the reverse). Each written slot keeps
//! its definitions by place, and each slot its readers; a read resolves
//! to the definition before the call's place. A run goes through the
//! places where something changed, in order: a call whose reads all
//! resolve to what they read before is kept (its writes stay, its bytes
//! are reused); any other runs, and each of its writes that changed
//! wakes the readers up to the slot's next definition. `SORT` reads every
//! entry's `sort.key$` and keeps a sorted map (by the key, then the
//! entry's place in the cite order: bibtex.web's tie-break), so an entry
//! whose key changed moves, and its calls in the commands that follow
//! move with it. The `.bbl`, `.blg` and terminal are the parse's pieces
//! and the calls' pieces in order; the `.blg`'s statistics are sums.
//!
//! The result is bibtex.web's, byte for byte: what a call does depends
//! only on what it reads, and every read of state that another call can
//! write goes through a slot. A run that meets a fatal error is made
//! again from scratch by [`crate::run`].

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::builtins::NUM_BLT_IN_FNS;
use crate::table::{Loc, Str};
use crate::{Bib, EMPTY, END_OF_STRING, Files, Options, Outcome, R, UNDEFINED};

/// An entry, numbered by its lower-case cite key for the session.
type Ent = u32;

/// A call's place: its command, then its entry's key in the order the
/// command iterates (0 for `EXECUTE` and `SORT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pos(u32, u64);

/// A slot a call reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Slot {
    /// `READ`'s outputs.
    Field(Ent, u32),
    Type(Ent),
    Key(Ent),
    Preamble,
    /// An entry's variables, by number.
    EntInt(Ent, u32),
    EntStr(Ent, u32),
    /// The global variables (integers by the session's number of their
    /// name; strings by number).
    GlbInt(u32),
    GlbStr(u32),
    /// The output buffer.
    OutBuf,
}

impl Slot {
    /// Whether the slot is one of `READ`'s outputs (no call writes it).
    fn read_only(self) -> bool {
        matches!(
            self,
            Slot::Field(..) | Slot::Type(_) | Slot::Key(_) | Slot::Preamble
        )
    }
}

/// A slot's value.
#[derive(Clone, Debug, PartialEq, Eq)]
enum V {
    Int(i64),
    Str(Str),
    Field(Option<Str>),
    /// An entry's type: its function's name, or `UNDEFINED`/`EMPTY`.
    Type(Option<Str>, Loc),
    /// A global string: kept as the database or style made it, or a copy.
    Glb(Option<Str>, Str),
}

/// What a call made besides its writes: its bytes of each output, its
/// built-in function calls by number, its warnings and errors.
#[derive(Clone, Default)]
struct Chunk {
    bbl: Vec<u8>,
    log: Vec<u8>,
    term: Vec<u8>,
    counts: Vec<(u16, i64)>,
    warns: i64,
    errs: i64,
}

/// A call's record: its command and entry, its reads from outside it
/// (each slot's first, with the value read), its writes (each slot's
/// last value) and its chunk.
#[derive(Clone)]
struct Rec {
    cmd: u32,
    ent: Option<Ent>,
    reads: Vec<(Slot, V)>,
    writes: Vec<(Slot, V)>,
    out: Chunk,
}

/// What the entries' order is: each entry's key, and the entries by key.
#[derive(Clone, Default)]
struct Order {
    key: BTreeMap<Ent, u64>,
    by: BTreeMap<u64, Ent>,
}

/// The gap between two keys an order makes afresh.
const GAP: u64 = 1 << 32;

impl Order {
    fn remove(&mut self, e: Ent) -> Option<u64> {
        let k = self.key.remove(&e)?;
        self.by.remove(&k);
        Some(k)
    }

    fn insert(&mut self, e: Ent, k: u64) {
        self.key.insert(e, k);
        self.by.insert(k, e);
    }
}

/// A `SORT` command's state: each entry's sort key (its `sort.key$` there
/// and its cite order key), the entries by it, the order it made, and the
/// entries to place again.
#[derive(Clone, Default)]
struct SortSt {
    sk: BTreeMap<Ent, (Str, u64)>,
    map: BTreeMap<(Str, u64), Ent>,
    order: Order,
    dirty: BTreeSet<Ent>,
}

/// An execution command of the style, as parsed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Execute,
    Iterate,
    Reverse,
    Sort,
}

/// A command met while parsing the style: its kind, its function, the
/// style's line (its messages say it), and where the parse's log and
/// terminal output were.
#[derive(Clone)]
struct Cmd {
    kind: Kind,
    loc: Loc,
    line: i64,
    log_at: usize,
    term_at: usize,
}

/// Why a place is to be looked at.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Why {
    /// A definition it read changed: run it if a read differs.
    Woken,
    /// It moved: placed again, run if a read differs.
    Moved,
    /// New: run it.
    New,
}

/// What a session's run did (for the report).
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// The run had nothing to start from (a first run, another style).
    pub fresh: bool,
    /// The calls of the run; those run, those looked at and kept, those
    /// moved by an order; the entries a `SORT` placed again.
    pub calls: usize,
    pub run: usize,
    pub kept: usize,
    pub moved: usize,
    pub sorted: usize,
    /// The entries added, removed and changed by `READ`.
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
}

/// What a session keeps between runs.
#[derive(Default)]
struct State {
    /// What the records are valid for: the style's commands and the
    /// options (another style or capacity starts afresh).
    sig: Vec<u8>,
    ents: BTreeMap<Str, Ent>,
    /// `READ`'s outputs: each entry's key, type and fields; the preamble.
    db: BTreeMap<Ent, (Str, V, Vec<Option<Str>>)>,
    preamble: Option<Str>,
    /// The cite order, and each `SORT`'s state, by command.
    order0: Order,
    sorts: BTreeMap<u32, SortSt>,
    /// The global integers' numbers, by name.
    glbs: BTreeMap<Str, u32>,
    /// Each written slot's definitions by place; each slot's readers.
    defs: BTreeMap<Slot, BTreeMap<Pos, V>>,
    readers: BTreeMap<Slot, BTreeSet<Pos>>,
    /// The calls by place, and each iterated call's place.
    calls: BTreeMap<Pos, Rec>,
    at: BTreeMap<(u32, Ent), Pos>,
}

/// The run of a session in progress: its state, this run's commands and
/// entries, and the call being run.
pub(crate) struct Inc {
    st: State,
    cmds: Vec<Cmd>,
    /// This run's cite numbers' entries, and the reverse.
    ent_of: Vec<Ent>,
    c_of: BTreeMap<Ent, usize>,
    /// Each global integer's number, by its location in this run's table.
    glb_ids: BTreeMap<Loc, u32>,
    call: Option<CallCx>,
    work: BTreeMap<Pos, Why>,
    stats: Stats,
}

/// The call being run: its place, its reads from outside it, the slots
/// it read or wrote, its writes, and whether it used the output buffer.
struct CallCx {
    pos: Pos,
    ent: Option<Ent>,
    reads: Vec<(Slot, V)>,
    seen: BTreeSet<Slot>,
    writes: BTreeMap<Slot, V>,
    out: bool,
}

/// BibTeX run by a session: the same program, run again only where a
/// version it read changed.
#[derive(Default)]
pub struct Session {
    st: Option<State>,
}

impl Session {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Run BibTeX on `aux` (as [`crate::run`]), reusing the last run's
    /// calls where what they read is the same.
    pub fn run(&mut self, aux: &[u8], opts: &Options, files: &mut dyn Files) -> (Outcome, Stats) {
        let st = self.st.take().unwrap_or_default();
        if let Ok((out, st, stats)) = run_inc(aux, opts, files, st) {
            self.st = Some(st);
            return (out, stats);
        }
        // (a fatal error: bibtex.web's run, and nothing kept)
        let out = crate::run(aux, opts, files);
        (
            out,
            Stats {
                fresh: true,
                ..Stats::default()
            },
        )
    }
}

/// A session's run: `Err` if it met a fatal error (the caller runs
/// bibtex.web's).
fn run_inc(
    aux: &[u8],
    opts: &Options,
    files: &mut dyn Files,
    st: State,
) -> Result<(Outcome, State, Stats), ()> {
    let mut b = Bib::new(opts.clone(), files);
    b.inc = Some(Box::new(Inc {
        st,
        cmds: Vec::new(),
        ent_of: Vec::new(),
        c_of: BTreeMap::new(),
        glb_ids: BTreeMap::new(),
        call: None,
        work: BTreeMap::new(),
        stats: Stats::default(),
    }));
    b.pre_def_certain_strings();
    let Some((blg_name, bbl_name)) = b.get_the_top_level_aux_file_name(aux) else {
        return Err(());
    };
    let mut banner = b"This is BibTeX, Version 0.99e".to_vec();
    banner.extend_from_slice(&b.opts.version);
    if b.opts.terse {
        b.log.extend_from_slice(&banner);
        b.log.push(b'\n');
    } else {
        b.print_ln(&[&&banner[..]]);
    }
    let max_strings = b.opts.max_strings;
    let hash_size = max_strings.max(5000);
    let line = [
        &"Capacity: max_strings=" as &dyn crate::Piece,
        &max_strings,
        &", hash_size=",
        &hash_size,
        &", hash_prime=",
        &crate::hash_prime(hash_size),
    ];
    b.log_ln(&line);
    if b.read_aux().and_then(|()| b.read_bst()).is_err() || b.history == crate::FATAL_MESSAGE {
        return Err(());
    }
    b.execute_session()?;
    b.clean_up();
    let mut inc = b.inc.take().ok_or(())?;
    let status = if b.history > crate::WARNING_MESSAGE {
        i32::from(b.history)
    } else {
        0
    };
    inc.stats.calls = inc.st.calls.len();
    let out = Outcome {
        term: core::mem::take(&mut b.term),
        blg: Some(crate::OutFile {
            name: blg_name,
            contents: core::mem::take(&mut b.log),
        }),
        bbl: Some(crate::OutFile {
            name: bbl_name,
            contents: core::mem::take(&mut b.bbl),
        }),
        history: b.history,
        status,
    };
    Ok((out, inc.st, inc.stats))
}

/// The style's signature: its commands' kinds and functions, and the
/// options.
fn signature(b: &Bib<'_>, cmds: &[Cmd]) -> Vec<u8> {
    let mut sig = Vec::new();
    for c in cmds {
        sig.push(c.kind as u8);
        if c.kind != Kind::Sort {
            sig.extend_from_slice(b.t.text(c.loc));
        }
        sig.push(0);
        sig.extend_from_slice(&c.line.to_le_bytes());
    }
    let o = &b.opts;
    sig.extend_from_slice(alloc::format!("{o:?}").as_bytes());
    sig.extend_from_slice(alloc::format!("{:?}", b.bst_str).as_bytes());
    sig
}

/// Keys for `n` entries spread over the gap between `lo` and `hi`
/// (exclusive), or `None` if it is too small.
fn spread(lo: u64, hi: u64, n: usize) -> Option<Vec<u64>> {
    let n64 = n as u64;
    let room = hi.checked_sub(lo)?;
    if room <= n64 {
        return None;
    }
    let step = (room / (n64 + 1)).clamp(1, GAP);
    Some((1..=n64).map(|i| lo + step * i).collect())
}

/// The longest increasing subsequence of `xs` by value: which indices
/// keep their place.
fn lis(xs: &[u64]) -> Vec<bool> {
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = alloc::vec![None; xs.len()];
    for (i, &x) in xs.iter().enumerate() {
        let k = tails.partition_point(|&t| xs[t] < x);
        prev[i] = k.checked_sub(1).map(|k| tails[k]);
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut keep = alloc::vec![false; xs.len()];
    let mut at = tails.last().copied();
    while let Some(i) = at {
        keep[i] = true;
        at = prev[i];
    }
    keep
}

impl Inc {
    fn ent(&mut self, lc: &[u8]) -> Ent {
        if let Some(&e) = self.st.ents.get(lc) {
            return e;
        }
        let e = Ent::try_from(self.st.ents.len()).unwrap_or(Ent::MAX);
        self.st.ents.insert(Str::from(lc), e);
        e
    }

    /// Look at place `p` for `why` (the strongest reason wins).
    fn mark(&mut self, p: Pos, why: Why) {
        let w = self.work.entry(p).or_insert(why);
        if *w < why {
            *w = why;
        }
    }

    /// The command governing order at command `j`: the last `SORT` before
    /// it, if any.
    fn sort_before(&self, j: u32) -> Option<u32> {
        self.cmds[..j as usize]
            .iter()
            .rposition(|c| c.kind == Kind::Sort)
            .map(|s| s as u32)
    }

    /// The order command `j` iterates.
    fn order_at(&self, j: u32) -> &Order {
        match self.sort_before(j) {
            Some(s) => &self.st.sorts[&s].order,
            None => &self.st.order0,
        }
    }

    /// Entry `e`'s place in command `j` (an `ITERATE` or `REVERSE`).
    fn place(&self, j: u32, e: Ent) -> Option<Pos> {
        let k = *self.order_at(j).key.get(&e)?;
        Some(match self.cmds[j as usize].kind {
            Kind::Reverse => Pos(j, u64::MAX - k),
            _ => Pos(j, k),
        })
    }

    /// The iterating commands an order governs: those after `from` (a
    /// `SORT`, or `None` for the cite order) up to the next `SORT`.
    fn governed(&self, from: Option<u32>) -> Vec<u32> {
        let start = from.map_or(0, |s| s as usize + 1);
        self.cmds[start..]
            .iter()
            .enumerate()
            .take_while(|(_, c)| c.kind != Kind::Sort)
            .filter(|(_, c)| matches!(c.kind, Kind::Iterate | Kind::Reverse))
            .map(|(i, _)| (start + i) as u32)
            .collect()
    }

    /// Wake the readers of `slot` after place `p`, up to its next
    /// definition, that one's maker included (a `SORT` reading an
    /// entry's sort key: that entry).
    fn wake(&mut self, slot: Slot, p: Pos) {
        let next = self
            .st
            .defs
            .get(&slot)
            .and_then(|d| {
                d.range((core::ops::Bound::Excluded(p), core::ops::Bound::Unbounded))
                    .next()
            })
            .map(|(&q, _)| q);
        let Some(rs) = self.st.readers.get(&slot) else {
            return;
        };
        // (the call that makes the next definition read the slot before
        // it, if it read it: it is a reader of this one)
        let hi = next.map_or(core::ops::Bound::Unbounded, core::ops::Bound::Included);
        let woken: Vec<Pos> = rs
            .range((core::ops::Bound::Excluded(p), hi))
            .copied()
            .collect();
        for q in woken {
            if self
                .cmds
                .get(q.0 as usize)
                .is_some_and(|c| c.kind == Kind::Sort)
            {
                if let Slot::EntStr(e, _) = slot
                    && let Some(s) = self.st.sorts.get_mut(&q.0)
                {
                    s.dirty.insert(e);
                }
                self.mark(q, Why::Woken);
            } else {
                self.mark(q, Why::Woken);
            }
        }
    }

    /// Wake the readers of every place of a read-only slot.
    fn wake_all(&mut self, slot: Slot) {
        let rs: Vec<Pos> = self
            .st
            .readers
            .get(&slot)
            .map(|r| r.iter().copied().collect())
            .unwrap_or_default();
        for q in rs {
            self.mark(q, Why::Woken);
        }
    }

    /// Set slot `slot`'s definition at `p` to `v` (none: taken away),
    /// waking its readers after it if that changed what they find.
    fn define(&mut self, slot: Slot, p: Pos, v: Option<&V>) {
        let d = self.st.defs.entry(slot).or_default();
        let was = match v {
            Some(v) => d.insert(p, v.clone()),
            None => d.remove(&p),
        };
        if d.is_empty() {
            self.st.defs.remove(&slot);
        }
        if was.as_ref() != v {
            self.wake(slot, p);
        }
    }

    /// Take the call at `p` out of the slots: its definitions (its
    /// readers woken) and its reads. Its record.
    fn unplace(&mut self, p: Pos) -> Option<Rec> {
        let rec = self.st.calls.remove(&p)?;
        for (s, _) in &rec.writes {
            self.define(*s, p, None);
        }
        for (s, _) in &rec.reads {
            if let Some(r) = self.st.readers.get_mut(s) {
                r.remove(&p);
                if r.is_empty() {
                    self.st.readers.remove(s);
                }
            }
        }
        if let Some(e) = rec.ent {
            self.st.at.remove(&(rec.cmd, e));
        }
        Some(rec)
    }

    /// Put record `rec` at `p`: its definitions (readers after it woken
    /// where they differ) and its reads.
    fn place_rec(&mut self, p: Pos, rec: Rec) {
        for (s, v) in &rec.writes {
            self.define(*s, p, Some(v));
        }
        for (s, _) in &rec.reads {
            self.st.readers.entry(*s).or_default().insert(p);
        }
        if let Some(e) = rec.ent {
            self.st.at.insert((rec.cmd, e), p);
        }
        self.st.calls.insert(p, rec);
    }

    /// The calls `olds` (a command, an entry, its old place) placed where
    /// the order puts them now: all taken out first (an entry's new key
    /// can be another's old one), then each put at its place to be
    /// looked at again (moved), or made (new).
    fn replace_calls(&mut self, olds: Vec<(u32, Ent, Option<Pos>)>) {
        let mut taken: Vec<(u32, Ent, Option<Rec>)> = Vec::new();
        for (j, e, was) in olds {
            if was.is_some() && was == self.place(j, e) {
                continue;
            }
            let rec = was.and_then(|p| {
                self.work.remove(&p);
                self.unplace(p)
            });
            taken.push((j, e, rec));
        }
        for (j, e, rec) in taken {
            let Some(now) = self.place(j, e) else {
                continue;
            };
            match rec {
                Some(rec) => {
                    self.stats.moved += 1;
                    self.st.calls.insert(now, rec);
                    self.mark(now, Why::Moved);
                }
                None => self.mark(now, Why::New),
            }
        }
    }

    /// The initial value of a written slot (no definition before a read).
    fn initial(b_info: impl Fn(u32) -> i64, slot: Slot) -> V {
        match slot {
            Slot::EntInt(..) => V::Int(0),
            Slot::GlbInt(g) => V::Int(b_info(g)),
            Slot::GlbStr(_) => V::Glb(None, Str::from(&b""[..])),
            _ => V::Str(Str::from(&b""[..])),
        }
    }
}

impl Bib<'_> {
    // --- the slots' accessors: the arrays, or the session's slots ---

    /// The entry of cite number `c` in the session's run.
    fn inc_ent(&self) -> Option<Ent> {
        let inc = self.inc.as_ref()?;
        inc.call.as_ref()?;
        inc.ent_of.get(self.cite_ptr).copied()
    }

    /// Note a read of read-only slot `slot`, valued `v`.
    fn note_const(&mut self, slot: Slot, v: V) {
        if let Some(inc) = self.inc.as_mut()
            && let Some(cx) = inc.call.as_mut()
            && cx.seen.insert(slot)
        {
            cx.reads.push((slot, v));
        }
    }

    /// Global integer `g`'s value before any call sets it (the style's:
    /// 0, or `entry.max$`'s and `global.max$`'s capacities).
    fn glb_initial(&self, g: u32) -> i64 {
        let inc = self.inc.as_ref().expect("a session");
        inc.glb_ids
            .iter()
            .find(|&(_, &id)| id == g)
            .map_or(0, |(&loc, _)| self.t.info(loc))
    }

    /// The value written slot `slot` has where the call is (its own
    /// write, or the definition before its place), its read noted.
    fn slot_read(&mut self, slot: Slot) -> V {
        let inc = self.inc.as_ref().expect("a session");
        let cx = inc.call.as_ref().expect("a call");
        if let Some(v) = cx.writes.get(&slot) {
            return v.clone();
        }
        let found = inc
            .st
            .defs
            .get(&slot)
            .and_then(|d| d.range(..cx.pos).next_back())
            .map(|(_, v)| v.clone());
        let v = match found {
            Some(v) => v,
            None => Inc::initial(|g| self.glb_initial(g), slot),
        };
        let inc = self.inc.as_mut().expect("a session");
        let cx = inc.call.as_mut().expect("a call");
        if cx.seen.insert(slot) {
            cx.reads.push((slot, v.clone()));
        }
        v
    }

    fn slot_write(&mut self, slot: Slot, v: V) {
        let inc = self.inc.as_mut().expect("a session");
        let cx = inc.call.as_mut().expect("a call");
        cx.seen.insert(slot);
        cx.writes.insert(slot, v);
    }

    /// `cite_list[cite_ptr]`.
    pub(crate) fn cur_cite(&mut self) -> Str {
        let s = self.cite_list[self.cite_ptr].clone();
        if let Some(e) = self.inc_ent() {
            self.note_const(Slot::Key(e), V::Str(s.clone()));
        }
        s
    }

    /// Field `f` of the current entry.
    pub(crate) fn field(&mut self, f: usize) -> Option<Str> {
        let v = self
            .field_info
            .get(self.cite_ptr * self.num_fields + f)
            .cloned()
            .flatten();
        if let Some(e) = self.inc_ent() {
            self.note_const(Slot::Field(e, f as u32), V::Field(v.clone()));
        }
        v
    }

    /// The current entry's type (`type_list[cite_ptr]`).
    pub(crate) fn entry_type(&mut self) -> Loc {
        let t = self.type_list[self.cite_ptr];
        if let Some(e) = self.inc_ent() {
            let v = self.type_value(t);
            self.note_const(Slot::Type(e), v);
        }
        t
    }

    fn type_value(&self, t: Loc) -> V {
        match t {
            UNDEFINED | EMPTY => V::Type(None, t),
            t => V::Type(Some(self.t.text(t).clone()), 0),
        }
    }

    /// `preamble$`'s string.
    pub(crate) fn preamble(&mut self) -> Vec<u8> {
        let v: Vec<u8> = self.s_preamble[..self.num_preamble_strings].concat();
        if self.inc.as_ref().is_some_and(|i| i.call.is_some()) {
            self.note_const(Slot::Preamble, V::Str(Str::from(&v[..])));
        }
        v
    }

    pub(crate) fn ent_int(&mut self, i: usize) -> i64 {
        match self.inc_ent() {
            Some(e) => match self.slot_read(Slot::EntInt(e, i as u32)) {
                V::Int(n) => n,
                _ => 0,
            },
            None => self.entry_ints[self.cite_ptr * self.num_ent_ints + i],
        }
    }

    pub(crate) fn set_ent_int(&mut self, i: usize, n: i64) {
        match self.inc_ent() {
            Some(e) => self.slot_write(Slot::EntInt(e, i as u32), V::Int(n)),
            None => self.entry_ints[self.cite_ptr * self.num_ent_ints + i] = n,
        }
    }

    pub(crate) fn ent_str(&mut self, i: usize) -> Vec<u8> {
        if let Some(e) = self.inc_ent() {
            return match self.slot_read(Slot::EntStr(e, i as u32)) {
                V::Str(s) => s.to_vec(),
                _ => Vec::new(),
            };
        }
        let s = &self.entry_strs[self.cite_ptr * self.num_ent_strs + i];
        let end = s
            .iter()
            .position(|&c| c == END_OF_STRING)
            .unwrap_or(s.len());
        s[..end].to_vec()
    }

    pub(crate) fn set_ent_str(&mut self, i: usize, v: Vec<u8>) {
        match self.inc_ent() {
            Some(e) => {
                let end = v
                    .iter()
                    .position(|&c| c == END_OF_STRING)
                    .unwrap_or(v.len());
                self.slot_write(Slot::EntStr(e, i as u32), V::Str(Str::from(&v[..end])));
            }
            None => self.entry_strs[self.cite_ptr * self.num_ent_strs + i] = v,
        }
    }

    /// The global integer's number in the session (by its name).
    fn glb_id(&mut self, loc: Loc) -> Option<u32> {
        let name = self.t.text(loc).clone();
        let inc = self.inc.as_mut()?;
        inc.call.as_ref()?;
        if let Some(&g) = inc.glb_ids.get(&loc) {
            return Some(g);
        }
        let n = u32::try_from(inc.st.glbs.len()).unwrap_or(u32::MAX);
        let g = *inc.st.glbs.entry(name).or_insert(n);
        inc.glb_ids.insert(loc, g);
        Some(g)
    }

    pub(crate) fn glb_int(&mut self, loc: Loc) -> i64 {
        match self.glb_id(loc) {
            Some(g) => match self.slot_read(Slot::GlbInt(g)) {
                V::Int(n) => n,
                _ => 0,
            },
            None => self.t.info(loc),
        }
    }

    pub(crate) fn set_glb_int(&mut self, loc: Loc, n: i64) {
        match self.glb_id(loc) {
            Some(g) => self.slot_write(Slot::GlbInt(g), V::Int(n)),
            None => self.t.set_info(loc, n),
        }
    }

    pub(crate) fn glb_str(&mut self, i: usize) -> (Option<Str>, Str) {
        if self.inc.as_ref().is_some_and(|i| i.call.is_some()) {
            return match self.slot_read(Slot::GlbStr(i as u32)) {
                V::Glb(k, c) => (k, c),
                _ => (None, Str::from(&b""[..])),
            };
        }
        let (kept, copy) = &self.glb_strs[i];
        (kept.clone(), Str::from(&copy[..]))
    }

    pub(crate) fn set_glb_str(&mut self, i: usize, kept: Option<Str>, copy: Vec<u8>) {
        if self.inc.as_ref().is_some_and(|i| i.call.is_some()) {
            self.slot_write(Slot::GlbStr(i as u32), V::Glb(kept, Str::from(&copy[..])));
            return;
        }
        self.glb_strs[i] = (kept, copy);
    }

    /// Before the output buffer is used: in a session's call, its value
    /// where the call is, loaded once.
    pub(crate) fn out_touch(&mut self) {
        let Some(inc) = self.inc.as_ref() else {
            return;
        };
        let Some(cx) = inc.call.as_ref() else {
            return;
        };
        if cx.out {
            return;
        }
        let v = self.slot_read(Slot::OutBuf);
        if let Some(cx) = self.inc.as_mut().and_then(|i| i.call.as_mut()) {
            cx.out = true;
        }
        let s = match v {
            V::Str(s) => s,
            _ => Str::from(&b""[..]),
        };
        self.out_buf.clear();
        self.out_buf.extend_from_slice(&s);
        self.out_buf_length = s.len();
    }

    // --- deferring the style's execution commands ---

    /// In a session, an execution command met while parsing the style is
    /// kept, to run after the parse; whether it was.
    pub(crate) fn defer(&mut self, kind: u8, loc: Loc) -> bool {
        let (log_at, term_at, line) = (self.log.len(), self.term.len(), self.bst_line_num);
        let Some(inc) = self.inc.as_mut() else {
            return false;
        };
        let kind = match kind {
            0 => Kind::Execute,
            1 => Kind::Iterate,
            2 => Kind::Reverse,
            _ => Kind::Sort,
        };
        inc.cmds.push(Cmd {
            kind,
            loc,
            line,
            log_at,
            term_at,
        });
        true
    }

    // --- the run ---

    /// After the parse: the commands run as calls, each run again only
    /// where a version it read changed; the outputs put together.
    fn execute_session(&mut self) -> Result<(), ()> {
        let mut inc = self.inc.take().ok_or(())?;
        let sig = signature(self, &inc.cmds);
        if inc.st.sig != sig {
            // (another style or other options: nothing to reuse)
            inc.st = State {
                sig,
                ..State::default()
            };
            inc.stats.fresh = true;
        }
        if !self.read_completed {
            // (no READ: no command runs entries; nothing else to run)
            self.inc = Some(inc);
            return self.assemble();
        }
        // this run's entries, by cite number
        let n = self.num_cites;
        inc.ent_of = (0..n)
            .map(|c| {
                let lc = self.cite_list[c].to_ascii_lowercase();
                inc.ent(&lc)
            })
            .collect();
        inc.c_of = inc
            .ent_of
            .iter()
            .enumerate()
            .map(|(c, &e)| (e, c))
            .collect();
        self.inc = Some(inc);
        self.diff_db();
        self.diff_order0();
        // the EXECUTE calls, whose places no order moves
        {
            let inc = self.inc.as_mut().expect("a session");
            for j in 0..inc.cmds.len() {
                if inc.cmds[j].kind == Kind::Execute
                    && !inc.st.calls.contains_key(&Pos(j as u32, 0))
                {
                    inc.mark(Pos(j as u32, 0), Why::New);
                }
            }
            // (a style with fewer commands than the records': the records
            // of commands not run now go; the signature makes that rare)
        }
        while let Some((p, why)) = {
            let inc = self.inc.as_mut().expect("a session");
            inc.work.pop_first()
        } {
            let kind = self.inc.as_ref().expect("a session").cmds[p.0 as usize].kind;
            if kind == Kind::Sort {
                self.run_sort(p.0);
                continue;
            }
            self.visit(p, why)?;
        }
        self.assemble()
    }

    /// `READ`'s outputs against the last run's: entries added and removed,
    /// and the readers of each field, type or key that changed woken.
    fn diff_db(&mut self) {
        let nf = self.num_fields;
        let mut now: BTreeMap<Ent, (Str, V, Vec<Option<Str>>)> = BTreeMap::new();
        let ents: Vec<Ent> = self.inc.as_ref().expect("a session").ent_of.clone();
        for (c, &e) in ents.iter().enumerate() {
            let fields = (0..nf)
                .map(|f| self.field_info.get(c * nf + f).cloned().flatten())
                .collect();
            let t = self.type_value(self.type_list[c]);
            now.insert(e, (self.cite_list[c].clone(), t, fields));
        }
        let pre = Str::from(&self.s_preamble[..self.num_preamble_strings].concat()[..]);
        let inc = self.inc.as_mut().expect("a session");
        let old = core::mem::take(&mut inc.st.db);
        for (e, (k, t, fs)) in &old {
            let Some((k2, t2, fs2)) = now.get(e) else {
                // (removed: its calls go, and it leaves every order)
                inc.stats.removed += 1;
                let places: Vec<Pos> = inc
                    .st
                    .at
                    .range((0, *e)..)
                    .filter(|((_, x), _)| x == e)
                    .map(|(_, &p)| p)
                    .collect();
                let mut places = places;
                places.extend(
                    inc.st
                        .at
                        .iter()
                        .filter(|((_, x), _)| x == e)
                        .map(|(_, &p)| p),
                );
                places.sort_unstable();
                places.dedup();
                for p in places {
                    inc.unplace(p);
                    inc.work.remove(&p);
                }
                inc.st.order0.remove(*e);
                for s in inc.st.sorts.values_mut() {
                    if let Some(k) = s.sk.remove(e) {
                        s.map.remove(&k);
                    }
                    s.order.remove(*e);
                    s.dirty.remove(e);
                }
                continue;
            };
            let mut changed = false;
            if k != k2 {
                inc.wake_all(Slot::Key(*e));
                changed = true;
            }
            if t != t2 {
                inc.wake_all(Slot::Type(*e));
                changed = true;
            }
            for (f, (a, b)) in fs.iter().zip(fs2.iter()).enumerate() {
                if a != b {
                    inc.wake_all(Slot::Field(*e, f as u32));
                    changed = true;
                }
            }
            if changed {
                inc.stats.changed += 1;
            }
        }
        inc.stats.added += now.keys().filter(|e| !old.contains_key(e)).count();
        if inc.st.preamble.as_ref() != Some(&pre) {
            inc.wake_all(Slot::Preamble);
            inc.st.preamble = Some(pre);
        }
        inc.st.db = now;
    }

    /// The cite order against the last run's: an entry that kept its
    /// place among the others keeps its key; one added or moved gets a key
    /// between its neighbours', and its calls in the commands the cite
    /// order governs are placed (new) or moved, and every `SORT` places
    /// it again (the cite order is its tie-break).
    fn diff_order0(&mut self) {
        let inc = self.inc.as_mut().expect("a session");
        let ents = inc.ent_of.clone();
        let old: Vec<u64> = ents
            .iter()
            .map(|e| inc.st.order0.key.get(e).copied().unwrap_or(u64::MAX))
            .collect();
        // (the entries that keep their keys: in order, and an increasing
        // run of the old keys)
        let present: Vec<usize> = (0..ents.len()).filter(|&i| old[i] != u64::MAX).collect();
        let keep_sub = lis(&present.iter().map(|&i| old[i]).collect::<Vec<_>>());
        let mut keep = alloc::vec![false; ents.len()];
        for (k, &i) in present.iter().enumerate() {
            keep[i] = keep_sub[k];
        }
        let mut keys: Vec<Option<u64>> = (0..ents.len()).map(|i| keep[i].then(|| old[i])).collect();
        // (the others: spread between the kept keys around them)
        let mut i = 0;
        let mut fits = true;
        while i < ents.len() {
            if keys[i].is_some() {
                i += 1;
                continue;
            }
            let j = (i..ents.len())
                .find(|&j| keys[j].is_some())
                .unwrap_or(ents.len());
            let lo = if i == 0 { 0 } else { keys[i - 1].unwrap_or(0) };
            let hi = keys
                .get(j)
                .copied()
                .flatten()
                .unwrap_or_else(|| lo.saturating_add(GAP * (j - i + 1) as u64));
            let Some(ks) = spread(lo, hi, j - i) else {
                fits = false;
                break;
            };
            for (x, k) in ks.into_iter().enumerate() {
                keys[i + x] = Some(k);
            }
            i = j;
        }
        let keys: Vec<u64> = if fits {
            keys.into_iter().map(|k| k.unwrap_or(0)).collect()
        } else {
            (1..=ents.len() as u64).map(|i| i * GAP).collect()
        };
        let moved: Vec<Ent> = ents
            .iter()
            .zip(&keys)
            .filter(|&(e, k)| inc.st.order0.key.get(e) != Some(k))
            .map(|(&e, _)| e)
            .collect();
        let governed = inc.governed(None);
        // (the old places of the moved entries' calls, before the order
        // changes)
        let olds: Vec<(u32, Ent, Option<Pos>)> = moved
            .iter()
            .flat_map(|&e| governed.iter().map(move |&j| (j, e)))
            .map(|(j, e)| (j, e, inc.st.at.get(&(j, e)).copied()))
            .collect();
        for &e in &moved {
            inc.st.order0.remove(e);
        }
        for (&e, &k) in ents.iter().zip(&keys) {
            if moved.contains(&e) {
                inc.st.order0.insert(e, k);
            }
        }
        inc.replace_calls(olds);
        for j in 0..inc.cmds.len() as u32 {
            if inc.cmds[j as usize].kind == Kind::Sort {
                let s = inc.st.sorts.entry(j).or_default();
                for &e in &moved {
                    s.dirty.insert(e);
                }
                if !s.dirty.is_empty() {
                    inc.mark(Pos(j, 0), Why::Woken);
                }
            }
        }
    }

    /// `SORT` command `s`: its dirty entries placed again by their sort
    /// keys; an entry that moved has its calls in the commands it governs
    /// moved (or made).
    fn run_sort(&mut self, s: u32) {
        let skn = self.sort_key_num as u32;
        let inc = self.inc.as_mut().expect("a session");
        let mut st = inc.st.sorts.remove(&s).unwrap_or_default();
        let dirty = core::mem::take(&mut st.dirty);
        let p = Pos(s, 0);
        // (each dirty entry's sort key where the SORT is, and its place in
        // the cite order; those whose key changed leave the map first, then
        // come back with their new keys)
        let mut placing: Vec<((Str, u64), Ent)> = Vec::new();
        for e in dirty {
            let Some(&ck) = inc.st.order0.key.get(&e) else {
                continue;
            };
            let slot = Slot::EntStr(e, skn);
            let key = match inc
                .st
                .defs
                .get(&slot)
                .and_then(|d| d.range(..p).next_back())
                .map(|(_, v)| v.clone())
            {
                Some(V::Str(k)) => k,
                _ => Str::from(&b""[..]),
            };
            inc.st.readers.entry(slot).or_default().insert(p);
            let sk = (key, ck);
            if st.sk.get(&e) == Some(&sk) && st.order.key.contains_key(&e) {
                continue;
            }
            if let Some(old) = st.sk.insert(e, sk.clone()) {
                st.map.remove(&old);
            }
            placing.push((sk, e));
        }
        for (sk, e) in &placing {
            st.map.insert(sk.clone(), *e);
        }
        placing.sort();
        let pending: BTreeSet<(Str, u64)> = placing.iter().map(|(k, _)| k.clone()).collect();
        let mut done: BTreeSet<(Str, u64)> = BTreeSet::new();
        let mut moved: Vec<Ent> = Vec::new();
        let mut renumber = false;
        for (sk, e) in placing {
            inc.stats.sorted += 1;
            // (between the entry before it, placed, and the first after it
            // that is not still to be placed; it keeps its key if that is
            // still between theirs)
            let lo = st
                .map
                .range(..sk.clone())
                .next_back()
                .and_then(|(_, x)| st.order.key.get(x).copied())
                .unwrap_or(0);
            let hi = st
                .map
                .range((
                    core::ops::Bound::Excluded(sk.clone()),
                    core::ops::Bound::Unbounded,
                ))
                .find(|(k, _)| !pending.contains(*k) || done.contains(*k))
                .and_then(|(_, x)| st.order.key.get(x).copied());
            done.insert(sk.clone());
            if let Some(&k) = st.order.key.get(&e)
                && k > lo
                && hi.is_none_or(|h| k < h)
            {
                continue;
            }
            st.order.remove(e);
            let hi = hi.unwrap_or_else(|| lo.saturating_add(2 * GAP));
            let Some(k) = spread(lo, hi, 1) else {
                renumber = true;
                break;
            };
            st.order.insert(e, k[0]);
            moved.push(e);
        }
        if renumber {
            // (no room: every key made again, every entry moved)
            let all: Vec<Ent> = st.map.values().copied().collect();
            st.order = Order::default();
            for (i, &x) in all.iter().enumerate() {
                st.order.insert(x, (i as u64 + 1) * GAP);
            }
            moved = all;
        }
        moved.sort_unstable();
        moved.dedup();
        let governed = inc.governed(Some(s));
        let olds: Vec<(u32, Ent, Option<Pos>)> = moved
            .iter()
            .flat_map(|&e| governed.iter().map(move |&j| (j, e)))
            .map(|(j, e)| (j, e, inc.st.at.get(&(j, e)).copied()))
            .collect();
        inc.st.sorts.insert(s, st);
        inc.replace_calls(olds);
    }

    /// Look at the call at `p`: run it if new, or if a read resolves to
    /// other than it read; keep it otherwise.
    fn visit(&mut self, p: Pos, why: Why) -> Result<(), ()> {
        let inc = self.inc.as_mut().expect("a session");
        let j = p.0;
        let cmd = inc.cmds[j as usize].clone();
        let ent = if cmd.kind == Kind::Execute {
            None
        } else {
            let k = if cmd.kind == Kind::Reverse {
                u64::MAX - p.1
            } else {
                p.1
            };
            Some(*inc.order_at(j).by.get(&k).ok_or(())?)
        };
        // (a moved call is in `calls` at its new place, not in the slots)
        let placed = why != Why::Moved;
        if why != Why::New
            && let Some(rec) = inc.st.calls.get(&p)
        {
            let reads = rec.reads.clone();
            if self.reads_hold(p, &reads) {
                let inc = self.inc.as_mut().expect("a session");
                inc.stats.kept += 1;
                if !placed {
                    let rec = inc.st.calls.remove(&p).ok_or(())?;
                    inc.place_rec(p, rec);
                }
                return Ok(());
            }
        }
        let inc = self.inc.as_mut().expect("a session");
        let old = if placed {
            inc.st.calls.get(&p).cloned()
        } else {
            inc.st.calls.remove(&p)
        };
        let rec = self.run_call(p, &cmd, ent)?;
        let inc = self.inc.as_mut().expect("a session");
        inc.stats.run += 1;
        // the new record in place of the old: each write that changed
        // (or that only one run made) wakes its readers
        let olds: BTreeMap<Slot, V> = old
            .as_ref()
            .filter(|_| placed)
            .map(|r| r.writes.iter().cloned().collect())
            .unwrap_or_default();
        let news: BTreeMap<Slot, V> = rec.writes.iter().cloned().collect();
        if placed && let Some(o) = &old {
            for (s, _) in &o.reads {
                if let Some(r) = inc.st.readers.get_mut(s) {
                    r.remove(&p);
                }
            }
        }
        for s in olds
            .keys()
            .chain(news.keys())
            .copied()
            .collect::<BTreeSet<_>>()
        {
            let (a, b) = (olds.get(&s), news.get(&s));
            if a != b || !placed {
                inc.define(s, p, b);
            }
        }
        for (s, _) in &rec.reads {
            inc.st.readers.entry(*s).or_default().insert(p);
        }
        if let Some(e) = ent {
            inc.st.at.insert((j, e), p);
        }
        inc.st.calls.insert(p, rec);
        Ok(())
    }

    /// Whether each of `reads` resolves at `p` to the value it read.
    fn reads_hold(&self, p: Pos, reads: &[(Slot, V)]) -> bool {
        let inc = self.inc.as_ref().expect("a session");
        reads.iter().all(|(s, v)| {
            let now = if s.read_only() {
                self.const_value(*s)
            } else {
                Some(
                    inc.st
                        .defs
                        .get(s)
                        .and_then(|d| d.range(..p).next_back())
                        .map_or_else(
                            || Inc::initial(|g| self.glb_initial(g), *s),
                            |(_, v)| v.clone(),
                        ),
                )
            };
            now.as_ref() == Some(v)
        })
    }

    /// A read-only slot's value in this run.
    fn const_value(&self, s: Slot) -> Option<V> {
        let inc = self.inc.as_ref()?;
        let c_of = |e: Ent| inc.c_of.get(&e).copied();
        Some(match s {
            Slot::Key(e) => V::Str(self.cite_list[c_of(e)?].clone()),
            Slot::Type(e) => self.type_value(self.type_list[c_of(e)?]),
            Slot::Field(e, f) => V::Field(
                self.field_info
                    .get(c_of(e)? * self.num_fields + f as usize)
                    .cloned()
                    .flatten(),
            ),
            Slot::Preamble => V::Str(Str::from(
                &self.s_preamble[..self.num_preamble_strings].concat()[..],
            )),
            _ => return None,
        })
    }

    /// Run the call of command `cmd` at `p` (on entry `ent`): its record.
    fn run_call(&mut self, p: Pos, cmd: &Cmd, ent: Option<Ent>) -> Result<Rec, ()> {
        let inc = self.inc.as_mut().expect("a session");
        if let Some(e) = ent {
            self.cite_ptr = *inc.c_of.get(&e).ok_or(())?;
        }
        inc.call = Some(CallCx {
            pos: p,
            ent,
            reads: Vec::new(),
            seen: BTreeSet::new(),
            writes: BTreeMap::new(),
            out: false,
        });
        self.mess_with_entries = cmd.kind != Kind::Execute;
        self.bst_line_num = cmd.line;
        let (log0, term0, bbl0) = (self.log.len(), self.term.len(), self.bbl.len());
        let (w0, e0) = (self.n_warn, self.n_err);
        let counts0 = self.execution_count;
        self.init_command_execution();
        let r: R = self
            .execute_fn(cmd.loc)
            .and_then(|()| self.check_command_execution());
        if r.is_err() || self.history == crate::FATAL_MESSAGE {
            return Err(());
        }
        // (the output buffer, if the call used it, is one of its writes)
        let out = self
            .inc
            .as_ref()
            .and_then(|i| i.call.as_ref())
            .is_some_and(|c| c.out);
        if out {
            let v = V::Str(Str::from(&self.out_buf[..self.out_buf_length]));
            self.slot_write(Slot::OutBuf, v);
        }
        let cx = self.inc.as_mut().and_then(|i| i.call.take()).ok_or(())?;
        let chunk = Chunk {
            bbl: self.bbl.split_off(bbl0),
            log: self.log.split_off(log0),
            term: self.term.split_off(term0),
            counts: (0..NUM_BLT_IN_FNS)
                .filter(|&i| self.execution_count[i] != counts0[i])
                .map(|i| (i as u16, self.execution_count[i] - counts0[i]))
                .collect(),
            warns: self.n_warn - w0,
            errs: self.n_err - e0,
        };
        self.execution_count = counts0;
        self.n_warn = w0;
        self.n_err = e0;
        Ok(Rec {
            cmd: p.0,
            ent: cx.ent,
            reads: cx.reads,
            writes: cx.writes.into_iter().collect(),
            out: chunk,
        })
    }

    /// The outputs: the parse's pieces and the calls' chunks in order; the
    /// counts and the history from their sums.
    fn assemble(&mut self) -> Result<(), ()> {
        let inc = self.inc.as_ref().ok_or(())?;
        let (log, term) = (
            core::mem::take(&mut self.log),
            core::mem::take(&mut self.term),
        );
        let (mut l0, mut t0) = (0, 0);
        let mut bbl = core::mem::take(&mut self.bbl);
        let (mut warns, mut errs) = (self.n_warn, self.n_err);
        let mut counts = self.execution_count;
        for (j, c) in inc.cmds.iter().enumerate() {
            self.log.extend_from_slice(&log[l0..c.log_at]);
            self.term.extend_from_slice(&term[t0..c.term_at]);
            (l0, t0) = (c.log_at, c.term_at);
            let j = j as u32;
            for (_, rec) in inc.st.calls.range(Pos(j, 0)..Pos(j + 1, 0)) {
                self.log.extend_from_slice(&rec.out.log);
                self.term.extend_from_slice(&rec.out.term);
                bbl.extend_from_slice(&rec.out.bbl);
                for &(i, n) in &rec.out.counts {
                    counts[i as usize] += n;
                }
                warns += rec.out.warns;
                errs += rec.out.errs;
            }
        }
        self.log.extend_from_slice(&log[l0..]);
        self.term.extend_from_slice(&term[t0..]);
        self.bbl = bbl;
        self.execution_count = counts;
        // (bibtex.web's history from the events' counts: an error makes
        // it an error and counts the errors only, else the warnings)
        if errs > 0 {
            self.history = crate::ERROR_MESSAGE;
            self.err_count = errs;
        } else if warns > 0 {
            self.history = crate::WARNING_MESSAGE;
            self.err_count = warns;
        } else {
            self.history = crate::SPOTLESS;
            self.err_count = 0;
        }
        Ok(())
    }
}
