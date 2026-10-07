//! Memoized macro calls (DESIGN.md §7.7 step 0).
//!
//! A call of a macro that works inside a group is a region: it starts once
//! the macro has its arguments and ends when the group it runs in closes,
//! either one it opened (`\begingroup … \endgroup` in its own expansion) or
//! the one its caller opened. The first calls run normally while the engine
//! records the cells they read before writing them (with their values), the
//! cells they wrote globally, and the tokens still pending above the
//! caller's input level when the group closed. Later calls with the same
//! definition, arguments, mode and group type replay the record if every
//! recorded read still has its value: the global writes, the group's end
//! (for a group the caller opened), and the pending tokens pushed back as
//! input. Local writes vanish when the group ends, so they need no replay.
//!
//! Purity is observed, not declared: a call that prints (terminal, log,
//! `\write`), builds nodes, reads what cells do not cover (fonts, boxes,
//! marks, files, the current list), reads its caller's input past its
//! arguments, leaves a conditional or `\afterassignment` open, or writes
//! globally what a word cannot carry (glue, shapes, boxes) is not
//! recorded. Nothing is keyed by a macro's name, and nothing is tried
//! while any tracing parameter is positive.

use crate::relaxed::{Flag, I32, Log, U32, U64, Word};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::host::Host;
use crate::input::ux;
use crate::mem::NULL;
use crate::tex::{Jump, Tex};
use crate::track::{Cell, Tracker};
use crate::web::*;
use crate::xregs::{EXT_BASE, ext_reg, is_word_kind};

/// Calls of a definition before its calls are recorded.
const HOT: u32 = 8;
/// Recorded variants kept per (definition, arguments).
const PER_KEY: usize = 4;
/// Recordings a definition may fail before it is left alone.
const MAX_IMPURE: u32 = 4;
/// Reads beyond which a recording is abandoned.
/// Recordings of a definition before its savings are judged.
const WARMUP: u32 = 32;
/// Tokens of the input ahead in a call's signature.
const SIGHT: usize = 16;
/// Signatures kept per definition.
const MAX_SEEN: usize = 4096;
/// A definition that keeps failing is still tried every `RETRY` calls.
const RETRY: u32 = 4096;
/// The most consumed-token lengths kept for one key.
const MAX_LENS: usize = 32;
const MAX_READS: usize = 50_000;

/// Counters for reports.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoStats {
    pub hits: u64,
    pub checked: u64,
    pub misses: u64,
    pub recorded: u64,
    pub abandoned: u64,
    /// Tokens read by recorded calls, and saved by replays.
    pub recorded_work: u64,
    pub saved_work: u64,
    pub stale: u64,
}

#[derive(Clone, Debug, Default)]
struct DefStats {
    /// The definition list and its generation this slot is for.
    def: i32,
    generation: u32,
    calls: u32,
    impure: u32,
    hits: u32,
    stale: u32,
    recorded: u32,
    /// Tokens its recorded calls read, and its replays saved.
    rec_work: u64,
    saved: u64,
    /// The macro last called with this definition (for reports).
    cs: i32,
    /// Why its last recording was abandoned (for reports).
    why: String,
    /// Signatures (arguments and the next input) of calls not yet
    /// recorded: a call is recorded when its signature comes again.
    seen: BTreeSet<u64>,
    /// The table holds a reference to the definition's token list, so its
    /// id is not reused for another list while entries name it.
    pinned: bool,
}

/// A recording in progress.
#[derive(Clone, Debug)]
struct Rec {
    def: i32,
    /// Its statistics slot.
    slot: usize,
    /// The definition list's generation when the call began (a macro
    /// redefined during its own call frees its list, and the id may be
    /// reused before the call ends).
    def_generation: u32,
    /// The macro called (for check reports).
    cs: i32,
    /// Check mode: the entry a replay would have used.
    check: Option<Entry>,
    key: u64,
    args: Vec<Vec<i32>>,
    /// `cur_level`, `cur_group`, `input_ptr` and mode when it began.
    level: i32,
    group: i32,
    mode: i32,
    cond: usize,
    align: i32,
    printed: u64,
    /// First reads in order: the cell, its value, and for a list-valued
    /// word the list's generation.
    reads: Vec<(Cell, u64, u32)>,
    global: BTreeSet<i32>,
    /// The tokens read from the caller's input (its levels at or below
    /// `Memo::floor`), in order: part of the key.
    consumed: Vec<i32>,
    /// A local assignment at the level the call began at.
    local_at_start: bool,
    bad: bool,
    /// Where the call was found impure (file, line, command or -1), for
    /// reports.
    why: (&'static str, u32, i32),
}

/// A recorded call.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    args: Vec<Vec<i32>>,
    consumed: Vec<i32>,
    mode: i32,
    group: i32,
    /// (cell, value, list generation as `list_generation` gives it)
    reads: Vec<(Cell, u64, u32)>,
    /// Cells written globally, with their final words and the values they
    /// hold (a macro's list).
    globals: Vec<(i32, u64)>,
    global_objs: Vec<Option<crate::objs::Obj>>,
    /// The call closed the group its caller opened.
    closes_caller_group: bool,
    pending: Vec<i32>,
    align: i32,
    /// Tokens the call read from lists: what a replay saves.
    work: u64,
    /// Token lists the entry holds a reference to: those its reads and
    /// global writes name, so their ids are not reused while it exists.
    pins: Vec<i32>,
}

partex_engine::persist_struct!(Entry {
    args,
    consumed,
    mode,
    group,
    reads,
    globals,
    global_objs,
    closes_caller_group,
    pending,
    align,
    work,
    pins
});

/// The entries for one (definition, arguments) key, by the length and
/// hash of the tokens they consumed from the caller's input.
#[derive(Clone, Debug, Default)]
struct Slot {
    def: i32,
    key: u64,
    /// The consumed lengths present, ascending.
    lens: Vec<u32>,
    buckets: BTreeMap<(u32, u64), Vec<Entry>>,
}

/// Per cell, the last recording that read it and the last that wrote it
/// (and has not had it restored since), by recording number: an O(1)
/// "first read?" test on the hot path.
#[derive(Clone, Debug, Default)]
///
/// A stamp is one word, updated through `&self` (relaxed atomics keep the
/// engine `Sync`): the last reading recording in the low half, the last
/// writing one in the high half. The tables are sized when a recording
/// starts; the e-TeX registers above `EXT_BASE` go into a small table
/// keyed by (recording, register), so a new recording clears it by
/// counting up.
struct Stamps {
    epoch: u32,
    eqtb: Vec<U64>,
    hash: Vec<U64>,
    /// (key: epoch << 32 | register, stamp)
    ext: Vec<(U64, U64)>,
}

/// Slots of `Stamps::ext` (a power of two), and how far a lookup probes.
const EXT_SLOTS: usize = 4096;
const EXT_PROBES: usize = 16;

impl Stamps {
    /// The stamp of `c`; `None` if it has none (the register table full).
    fn at(&self, c: Cell) -> Option<&U64> {
        match c {
            Cell::Eqtb(p) if p >= EXT_BASE => {
                let epoch = u64::from(self.epoch);
                let want = epoch << 32 | u64::from(p.cast_unsigned());
                let h = (ux(p).wrapping_mul(0x9E37_79B9)) >> 7;
                for k in 0..EXT_PROBES {
                    let (key, stamp) = self.ext.get((h + k) & (EXT_SLOTS - 1))?;
                    let at = key.get();
                    if at == want {
                        return Some(stamp);
                    }
                    if at >> 32 != epoch {
                        key.set(want);
                        stamp.set(0);
                        return Some(stamp);
                    }
                }
                None
            }
            Cell::Eqtb(p) => self.eqtb.get(ux(p)),
            Cell::Hash(p) => self.hash.get(ux(p)),
            other => unreachable!("memoized calls see eqtb and the hash only: {other:?}"),
        }
    }

    /// Start a new recording, over `eqtb` and `hash` locations.
    fn next(&mut self, eqtb: usize, hash: usize) {
        if self.epoch == u32::MAX {
            *self = Self::default();
        }
        self.epoch += 1;
        if self.eqtb.len() < eqtb {
            self.eqtb.resize_with(eqtb, U64::default);
        }
        if self.hash.len() < hash {
            self.hash.resize_with(hash, U64::default);
        }
        if self.ext.len() < EXT_SLOTS {
            self.ext.resize_with(EXT_SLOTS, Default::default);
        }
    }
}

/// What the `&self` hooks add to the recording in progress (the rest of
/// [`Rec`] is set when it starts): relaxed atomics, so the engine stays
/// `Sync`. Moved into the `Rec` when the recording ends.
#[derive(Clone, Debug, Default)]
struct RecLog {
    /// First reads: cell (`cell_code`), value, list generation.
    reads: Log<(u64, u64, u32)>,
    global: Log<i32>,
    consumed: Log<i32>,
    local_at_start: Flag,
    bad: Flag,
    /// Where the call was found impure: index into `WHY_FILES`, line,
    /// command.
    why_file: Word,
    why_line: U32,
    why_cmd: I32,
}

/// The source files `impure` is reported from (by name).
const WHY_FILES: &[&str] = &[
    "memo.rs",
    "expand.rs",
    "maincontrol.rs",
    "scanner.rs",
    "scan.rs",
    "equiv.rs",
    "save.rs",
    "conds.rs",
    "print.rs",
    "input.rs",
    "prefixed.rs",
];

/// A cell as a word in [`RecLog`] (memoized calls see eqtb and the hash).
fn cell_code(c: Cell) -> u64 {
    match c {
        Cell::Eqtb(p) => u64::from(p.cast_unsigned()),
        Cell::Hash(p) => 1 << 32 | u64::from(p.cast_unsigned()),
        other => unreachable!("memoized calls see eqtb and the hash only: {other:?}"),
    }
}

fn cell_of(code: u64) -> Cell {
    let p = u32::try_from(code & 0xFFFF_FFFF).unwrap_or(0).cast_signed();
    if code >> 32 == 0 {
        Cell::Eqtb(p)
    } else {
        Cell::Hash(p)
    }
}

impl RecLog {
    /// Empty, for a new recording, with room for what one may collect.
    fn start(&mut self) {
        self.reserve();
        self.reads.clear();
        self.global.clear();
        self.consumed.clear();
        self.local_at_start.set(false);
        self.bad.set(false);
        self.why_file.set(usize::MAX);
        self.why_line.set(0);
        self.why_cmd.set(-1);
    }

    /// Room for what a recording may collect (a clone copies only what
    /// was appended).
    fn reserve(&mut self) {
        self.reads.reserve(MAX_READS + 2);
        self.global.reserve(MAX_READS + 2);
        self.consumed.reserve(MAX_READS + 2);
    }

    /// Move what was collected into `r`.
    fn take_into(&self, r: &mut Rec) {
        r.reads = self
            .reads
            .to_vec()
            .into_iter()
            .map(|(c, v, g)| (cell_of(c), v, g))
            .collect();
        r.global = self.global.to_vec().into_iter().collect();
        r.consumed = self.consumed.to_vec();
        r.local_at_start = self.local_at_start.get();
        r.bad = self.bad.get();
        let file = WHY_FILES.get(self.why_file.get()).copied().unwrap_or("?");
        r.why = (file, self.why_line.get(), self.why_cmd.get());
    }
}

/// Check mode: stored and fresh reads, and the fresh consumed tokens.
type ReadDiff = (Vec<(Cell, u64)>, Vec<(Cell, u64)>, Vec<i32>);

/// The memo table and the recording state.
#[derive(Clone, Debug, Default)]
pub(crate) struct Memo {
    pub(crate) enabled: bool,
    /// Check mode: never replay; record the call again and compare.
    pub(crate) check: bool,
    /// Check mode: the first calls whose fresh record differed, as
    /// (macro, what differed).
    pub(crate) mismatches: Vec<(i32, String)>,
    /// Debugging: replay only the first `limit` hits (0 = all), and
    /// remember the macro of the last one replayed.
    limit: u64,
    last_hit: i32,
    /// Check mode: the first differing pending tokens, as (stored, fresh).
    pending_diffs: Vec<(Vec<i32>, Vec<i32>)>,
    /// Check mode: for the first calls that were not replayable again,
    /// the stored and fresh reads (in order) and consumed tokens.
    read_diffs: Vec<ReadDiff>,
    /// Per cell, the lookups whose validation it failed first.
    stale_cells: BTreeMap<Cell, u64>,
    /// The token being fetched is for `main_control` itself, not for a
    /// scanner or another expansion.
    pub(crate) top: bool,
    /// A recording is active (read by the state accessors) unless
    /// `stopped`: set and cleared with `&mut self`, so the accessors'
    /// test is a plain load; a `&self` hook that finds the call impure
    /// sets `stopped` (a relaxed atomic, which keeps the engine `Sync`).
    live: bool,
    stopped: Flag,
    /// While recording: the top input level that belongs to the caller
    /// (tokens read from it or below are consumed).
    floor: Word,
    /// Tokens read from lists by the call being recorded.
    work: U64,
    /// The recording in progress (what `&self` hooks add is in `log`).
    rec: Option<Rec>,
    log: RecLog,
    stamps: Stamps,
    /// Per definition (by `by_def`; slot 0 unused).
    defs: Vec<DefStats>,
    /// The statistics slot of each definition, by its key (`def_key`).
    by_def: crate::u64map::U64Map<u32>,
    /// The entries by (definition, arguments): `index` maps a hash of the
    /// pair to a slot in `table`.
    index: crate::u64map::U64Map<u32>,
    table: Vec<Slot>,
    /// Characters printed so far (any selector).
    pub(crate) printed: u64,
    pub(crate) stats: MemoStats,
}

/// A cell's value as validation compares it: `eq_level` is left out,
/// since only assignments look at it and a recorded call's local
/// assignments are undone by its group's end.
#[inline]
fn masked(c: Cell, bits: u64) -> u64 {
    match c {
        Cell::Eqtb(p) if p < INT_BASE || (p >= EXT_BASE && !is_word_kind(ext_reg(p).0)) => {
            bits & !0xFFFF
        }
        _ => bits,
    }
}

/// A macro argument's tokens.
fn arg(a: Option<&crate::tok::Tokens>) -> &[i32] {
    a.map_or(&[][..], |l| l.tokens())
}

/// A definition's key and generation, by its content (`\protected`
/// flag and tokens): equal definitions are one.
fn def_key(body: &crate::tok::TokenList) -> (i32, u32) {
    let h = partex_engine::stablehash::StableHasher::of(&(body.protected(), body.tokens()));
    #[allow(clippy::cast_possible_truncation, reason = "a key's bits")]
    let key = (h as u32 & 0x7fff_ffff) | 1;
    #[allow(clippy::cast_possible_truncation, reason = "a key's bits")]
    let generation = (h >> 32) as u32;
    (key.cast_signed(), generation)
}

fn tok_hash(toks: &[i32]) -> u64 {
    toks.iter().fold(0xcbf2_9ce4_8422_2325, |h, &t| {
        (h ^ u64::from(t.cast_unsigned())).wrapping_mul(0x100_0000_01b3)
    })
}

fn arg_hash<'a>(def: i32, args: impl Iterator<Item = &'a [i32]>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ u64::from(def.cast_unsigned());
    for a in args {
        for &t in a.iter().chain(core::iter::once(&-1)) {
            h = (h ^ u64::from(t.cast_unsigned())).wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

fn slot_hash(def: i32, key: u64) -> u64 {
    key ^ u64::from(def.cast_unsigned())
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left(29)
}

impl Memo {
    /// Where the entries for (`def`, `key`) are in `table`.
    #[inline]
    fn slot_index(&self, def: i32, key: u64) -> Option<usize> {
        let i = self.index.get(slot_hash(def, key))? as usize;
        let s = &self.table[i];
        (s.def == def && s.key == key).then_some(i)
    }

    /// The entries for (`def`, `key`), made if new; `None` if another
    /// pair has its hash.
    fn slot_mut(&mut self, def: i32, key: u64) -> Option<&mut Slot> {
        let h = slot_hash(def, key);
        let i = if let Some(i) = self.index.get(h) {
            i as usize
        } else {
            self.table.push(Slot {
                def,
                key,
                ..Slot::default()
            });
            let i = self.table.len() - 1;
            self.index.insert(h, u32::try_from(i).ok()?);
            i
        };
        let s = &mut self.table[i];
        (s.def == def && s.key == key).then_some(s)
    }

    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            defs: alloc::vec![DefStats::default()],
            ..Self::default()
        }
    }

    /// Whether a recording is active (see `live`).
    #[inline]
    fn on(&self) -> bool {
        self.live && !self.stopped.get()
    }

    /// Whether a recording is active: the accessors' fast test.
    #[inline]
    pub(crate) fn recording(&self) -> bool {
        self.on()
    }

    /// A read of `c`, whose value is `bits`.
    #[inline]
    pub(crate) fn read(&self, c: Cell, bits: u64) {
        if self.on() {
            self.read_word(c, bits, 0);
        }
    }

    /// A read of `c`, whose value is `bits`; `generation` is the
    /// generation of the token list the word names, if any.
    pub(crate) fn read_word(&self, c: Cell, bits: u64, generation: u32) {
        if self.first_read(c) {
            self.push_read(c, bits, generation);
        }
    }

    /// Whether `c` is read for the first time by the call (not read or
    /// written by it before); marks it read.
    #[inline]
    pub(crate) fn first_read(&self, c: Cell) -> bool {
        let epoch = u64::from(self.stamps.epoch);
        let Some(s) = self.stamps.at(c) else {
            self.impure();
            return false;
        };
        let v = s.get();
        if v & 0xFFFF_FFFF == epoch || v >> 32 == epoch {
            return false;
        }
        s.set(v & !0xFFFF_FFFF | epoch);
        true
    }

    /// Room for a recording again (after a clone, which copies only what
    /// the recording in progress collected).
    pub(crate) fn reserve(&mut self) {
        if self.rec.is_some() {
            self.log.reserve();
        }
    }

    /// Record a first read of `c` (see [`Self::first_read`]).
    #[cold]
    pub(crate) fn push_read(&self, c: Cell, bits: u64, generation: u32) {
        if self.rec.is_some() {
            let log = &self.log;
            let room = log.reads.push((cell_code(c), masked(c, bits), generation));
            if !room || log.reads.len() > MAX_READS {
                log.bad.set(true);
                self.stopped.set(true);
            }
        }
    }

    /// A write of `c`.
    #[inline]
    pub(crate) fn wrote(&self, c: Cell) {
        if self.on() {
            let epoch = u64::from(self.stamps.epoch);
            match self.stamps.at(c) {
                Some(s) => s.set(s.get() & 0xFFFF_FFFF | epoch << 32),
                None => self.impure(),
            }
        }
    }

    /// A group's end restored `c` to a value from before its group: later
    /// reads may see a value from before the call again.
    pub(crate) fn restored(&self, c: Cell) {
        if self.on() {
            match self.stamps.at(c) {
                Some(s) => s.set(s.get() & 0xFFFF_FFFF),
                None => self.impure(),
            }
        }
    }

    /// A global assignment to `p`.
    pub(crate) fn wrote_global(&self, p: i32) {
        if self.on() && self.rec.is_some() && !self.log.global.push(p) {
            self.log.bad.set(true);
            self.stopped.set(true);
        }
    }

    /// A local assignment at level `level`.
    pub(crate) fn wrote_local(&self, level: i32) {
        if self.on()
            && let Some(r) = &self.rec
            && level == r.level
        {
            self.log.local_at_start.set(true);
        }
    }

    /// Something a record cannot replay happened: stop recording.
    #[cold]
    #[track_caller]
    pub(crate) fn impure(&self) {
        self.impure_cmd(-1);
    }

    /// Command `cmd` makes the call impure.
    #[track_caller]
    pub(crate) fn impure_cmd(&self, cmd: i32) {
        if self.on() {
            self.stopped.set(true);
            if self.rec.is_some() {
                let log = &self.log;
                log.bad.set(true);
                let at = core::panic::Location::caller();
                let file = at.file().rsplit('/').next().unwrap_or("");
                let i = WHY_FILES.iter().position(|&f| f == file);
                log.why_file.set(i.unwrap_or(usize::MAX));
                log.why_line.set(at.line());
                log.why_cmd.set(cmd);
            }
        }
    }

    /// Account for a recording abandoned since the last call.
    fn reap(&mut self) {
        if self.on() || self.rec.is_none() {
            return;
        }
        if let Some(mut r) = self.rec.take() {
            self.log.take_into(&mut r);
            self.stats.abandoned += 1;
            let d = &mut self.defs[r.slot];
            d.impure += 1;
            if r.why.1 != 0 {
                d.why = format!("impure at {}:{} cmd {}", r.why.0, r.why.1, r.why.2);
            } else {
                d.why = "too many reads".into();
            }
        }
    }

    /// An `\aftergroup` at level `level`: a token the call would leave
    /// for its caller's group.
    #[track_caller]
    pub(crate) fn after_group(&self, level: i32) {
        if self.on() && self.rec.as_ref().is_some_and(|r| level <= r.level) {
            self.impure();
        }
    }

    /// A conditional was closed, leaving `len` open.
    #[track_caller]
    pub(crate) fn cond_closed(&self, len: usize) {
        if self.on() && self.rec.as_ref().is_some_and(|r| len < r.cond) {
            self.impure();
        }
    }

    /// A token list at input level `ptr` (of type `token_type`) ended:
    /// if it was the caller's, the call goes on to read the level below.
    #[track_caller]
    #[inline]
    pub(crate) fn list_ended(&self, ptr: usize, token_type: i32) {
        if self.on() && ptr <= self.floor.get() {
            if token_type == U_TEMPLATE || token_type == V_TEMPLATE {
                self.impure();
            } else {
                self.floor.set(ptr - 1);
            }
        }
    }

    /// A token `t` read from input level `ptr`.
    #[inline]
    pub(crate) fn fetched(&self, ptr: usize, t: i32, cur_level: i32) {
        if self.on() {
            self.work.set(self.work.get() + 1);
            if ptr == self.floor.get() {
                self.consumed(t, cur_level);
            }
        }
    }

    /// A token read from the caller's input. A call with no group of its
    /// own open would run on into its caller's code: not worth recording.
    #[cold]
    #[track_caller]
    fn consumed(&self, t: i32, cur_level: i32) {
        if self.rec.as_ref().is_some_and(|r| cur_level <= r.level) {
            self.impure();
            return;
        }
        if self.rec.is_some() {
            let log = &self.log;
            if !log.consumed.push(t) || log.consumed.len() > MAX_READS {
                log.bad.set(true);
                self.stopped.set(true);
            }
        }
    }

    /// A parameter of the macro at input level `ptr` was pushed above it:
    /// if the macro is the caller's, so are the parameter's tokens.
    #[inline]
    pub(crate) fn param_pushed(&self, ptr: usize) {
        if self.on() && ptr == self.floor.get() {
            self.floor.set(ptr + 1);
        }
    }

    /// Input is read from a file at level `ptr`.
    #[track_caller]
    #[inline]
    pub(crate) fn file_read(&self, ptr: usize) {
        if self.on() && ptr <= self.floor.get() {
            self.impure();
        }
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Whether no tracing parameter could make a replay visible.
    fn memo_quiet(&self) -> bool {
        [
            TRACING_ONLINE_CODE,
            TRACING_MACROS_CODE,
            TRACING_COMMANDS_CODE,
            TRACING_RESTORES_CODE,
            TRACING_ASSIGNS_CODE,
            TRACING_GROUPS_CODE,
            TRACING_IFS_CODE,
            TRACING_SCAN_TOKENS_CODE,
            TRACING_NESTING_CODE,
        ]
        .iter()
        .all(|&c| self.peek_eqtb(INT_BASE + c).int() <= 0)
    }

    /// §389, before the body of macro `def` (its token list) is fed to the
    /// scanner with arguments `args`: replay a recorded call and return
    /// `true`, or start recording this one.
    pub(crate) fn memo_call(
        &mut self,
        body: &crate::tok::Tokens,
        args: &[Option<crate::tok::Tokens>],
    ) -> Result<bool, Jump> {
        if self.memo.on() {
            return Ok(false); // (inside a recording: run it as part of it)
        }
        self.memo.reap();
        // (a definition by its content: a list is a value)
        let (def, generation) = def_key(body);
        let slot = self.memo_slot(def, generation);
        let stats = &mut self.memo.defs[slot];
        stats.calls = stats.calls.saturating_add(1);
        stats.cs = self.warning_index;
        let (calls, impure) = (stats.calls, stats.impure);
        // Recording costs about half the work it records again; a
        // definition whose replays save less than that is not worth it.
        let worth = stats.recorded < WARMUP || stats.saved * 2 >= stats.rec_work;
        let retry = calls.is_multiple_of(RETRY);
        let record = calls >= HOT && ((worth && impure < MAX_IMPURE + 2 * stats.recorded) || retry);
        if !(record || stats.pinned && worth) {
            self.memo.stats.misses += 1;
            return Ok(false); // (no entries, and none to record: skip the key)
        }
        if self.after_token() != 0 || !self.memo_quiet() {
            return Ok(false);
        }
        let key = arg_hash(def, args.iter().map(|a| arg(a.as_ref())));
        let same_args = |e: &Entry| {
            e.args.len() == args.len()
                && e.args
                    .iter()
                    .zip(args)
                    .all(|(x, a)| x[..] == *arg(a.as_ref()))
        };
        let mode = self.mode();
        // The caller's next tokens, as far as entries went on to read.
        let max = self
            .memo
            .slot_index(def, key)
            .map_or(0, |i| self.memo.table[i].lens.last().copied().unwrap_or(0));
        let ahead = if max > 0 {
            self.memo_peek(max as usize)
        } else {
            Vec::new()
        };
        if let Some(i) = self.memo.slot_index(def, key) {
            let tab = &self.memo.table[i];
            let group = self.cur_group();
            let fits = |e: &Entry| e.mode == mode && e.group == group && same_args(e);
            let buckets = || {
                tab.lens
                    .iter()
                    .rev()
                    .filter(|&&l| l as usize <= ahead.len())
                    .filter_map(|&l| {
                        let c = &ahead[..l as usize];
                        let h = tok_hash(c);
                        tab.buckets.get(&(l, h)).map(|v| (l, h, c, v))
                    })
            };
            let hit = buckets().find_map(|(l, h, c, v)| {
                v.iter()
                    .position(|e| {
                        fits(e)
                            && e.consumed[..] == *c
                            && e.reads
                                .iter()
                                .all(|&(cell, x, g)| self.memo_read_ok(cell, x, g))
                    })
                    .map(|i| v[i].clone())
                    .map(|e| (l, h, e))
            });
            if let Some((_, _, e)) = hit {
                self.memo.stats.hits += 1;
                self.memo.stats.saved_work += e.work;
                let d = &mut self.memo.defs[slot];
                d.hits += 1;
                d.saved += e.work;
                if self.memo.check {
                    let argv = args.iter().map(|a| arg(a.as_ref()).to_vec()).collect();
                    self.memo_start(def, generation, slot, key, argv, Some(e));
                    return Ok(false);
                }
                if self.memo.limit != 0 {
                    if self.memo.stats.hits > self.memo.limit {
                        return Ok(false);
                    }
                    self.memo.last_hit = self.warning_index;
                }
                self.memo_replay(&e)?;
                return Ok(true);
            }
            self.memo.stats.stale += 1;
            self.memo.defs[slot].stale += 1;
            let first = buckets().find_map(|(_, _, c, v)| {
                v.iter().find_map(|e| {
                    (fits(e) && e.consumed[..] == *c)
                        .then(|| {
                            e.reads
                                .iter()
                                .find(|&&(cell, x, g)| !self.memo_read_ok(cell, x, g))
                                .map(|r| r.0)
                        })
                        .flatten()
                })
            });
            if let Some(c) = first {
                *self.memo.stale_cells.entry(c).or_default() += 1;
            }
        }
        self.memo.stats.misses += 1;
        if record {
            let argv = args.iter().map(|a| arg(a.as_ref()).to_vec()).collect();
            if self.memo_seen_before(slot, key) {
                self.memo_start(def, generation, slot, key, argv, None);
            }
        }
        Ok(false)
    }

    /// Whether a call like this one (same arguments, same next tokens)
    /// came before; remembers it.
    fn memo_seen_before(&mut self, slot: usize, key: u64) -> bool {
        let sig = tok_hash(&self.memo_peek(SIGHT)) ^ key.rotate_left(17);
        let d = &mut self.memo.defs[slot];
        if d.seen.remove(&sig) {
            return true;
        }
        if d.seen.len() >= MAX_SEEN {
            d.seen.clear();
        }
        d.seen.insert(sig);
        false
    }

    /// The statistics slot of definition list `def` (a reused id starts
    /// afresh).
    fn memo_slot(&mut self, def: i32, generation: u32) -> usize {
        let s = self
            .memo
            .by_def
            .get(u64::from(def.cast_unsigned()))
            .map_or(0, |s| s as usize);
        if s != 0 && self.memo.defs[s].def == def {
            if self.memo.defs[s].generation != generation {
                self.memo.defs[s] = DefStats {
                    def,
                    generation,
                    ..DefStats::default()
                };
            }
            return s;
        }
        let s = self.memo.defs.len();
        self.memo.defs.push(DefStats {
            def,
            generation,
            ..DefStats::default()
        });
        self.memo.by_def.insert(
            u64::from(def.cast_unsigned()),
            u32::try_from(s).unwrap_or(0),
        );
        s
    }

    fn memo_start(
        &mut self,
        def: i32,
        def_generation: u32,
        slot: usize,
        key: u64,
        argv: Vec<Vec<i32>>,
        check: Option<Entry>,
    ) {
        let mode = self.mode();
        {
            self.memo.rec = Some(Rec {
                def,
                slot,
                def_generation,
                cs: self.warning_index,
                check,
                key,
                args: argv,
                level: self.cur_level(),
                group: self.cur_group(),
                mode,
                cond: self.cond_stack.len(),
                align: self.align_state(),
                printed: self.memo.printed,
                reads: Vec::new(),
                global: BTreeSet::new(),
                consumed: Vec::new(),
                local_at_start: false,
                bad: false,
                why: ("", 0, -1),
            });
            self.memo.floor.set(self.input_ptr);
            self.memo.work.set(0);
            let (eqtb, hash) = (self.eqtb.len(), self.hash.len());
            self.memo.stamps.next(eqtb, hash);
            self.memo.log.start();
            self.memo.live = true;
            self.memo.stopped.set(false);
        }
    }

    /// The next `n` tokens of the current input, or fewer where it stops
    /// being token lists: what `get_next` would read, parameters
    /// substituted, without reading it.
    fn memo_peek(&self, n: usize) -> Vec<i32> {
        let mut out = Vec::with_capacity(n);
        for i in (0..=self.input_ptr).rev() {
            let s = if i == self.input_ptr {
                &self.cur_input
            } else {
                &self.input_stack[i]
            };
            if s.state != TOKEN_LIST || s.index == U_TEMPLATE || s.index == V_TEMPLATE {
                break;
            }
            if s.loc == NULL {
                continue;
            }
            let list = s.tokens_in(&self.param_stack);
            for &t in &list[ux(s.loc)..] {
                if s.index == MACRO && t < CS_TOKEN_FLAG && tok_cmd(t) == OUT_PARAM {
                    if let Some(p) = &self.param_stack[ux(s.limit + tok_chr(t) - 1)] {
                        out.extend_from_slice(p);
                    }
                } else {
                    out.push(t);
                }
                if out.len() >= n {
                    out.truncate(n);
                    return out;
                }
            }
        }
        out
    }

    /// Whether a recorded read still holds: the cell has value `x`, and a
    /// list it names is the one read (generation `g`, as
    /// `list_generation` gives it; 0 for no list).
    #[inline]
    fn memo_read_ok(&self, c: Cell, x: u64, g: u32) -> bool {
        self.memo_value(c) == x
            && (g == 0 || {
                let Cell::Eqtb(p) = c else { return false };
                self.list_generation(p, self.peek_eqtb(p)) == g
            })
    }

    /// A cell's current value, without recording the read.
    fn memo_value(&self, c: Cell) -> u64 {
        match c {
            Cell::Eqtb(p) => masked(c, self.peek_eqtb(p).bits()),
            Cell::Hash(p) => self.hash[ux(p - HASH_BASE)].bits(),
            other => unreachable!("memoized calls see eqtb and the hash only: {other:?}"),
        }
    }

    fn memo_replay(&mut self, e: &Entry) -> Result<(), Jump> {
        if !e.consumed.is_empty() {
            // Read the consumed tokens as the call did (ending the lists
            // it ended); their braces are in `e.align`, and no alignment
            // entry can end among them.
            let (align, status) = (self.align_state(), self.scanner_status);
            self.set_align_state(1_000_000);
            self.scanner_status = NORMAL;
            for _ in 0..e.consumed.len() {
                self.get_next()?;
            }
            self.set_align_state(align);
            self.scanner_status = status;
        }
        for (&(p, bits), o) in e.globals.iter().zip(&e.global_objs) {
            let w = crate::mem::MemoryWord::from_bits(bits);
            if (INT_BASE..=EQTB_SIZE).contains(&p) || (p >= EXT_BASE && is_word_kind(ext_reg(p).0))
            {
                self.geq_word_define(p, w.int());
            } else {
                let (t, v) = (w.b0(), w.rh());
                self.geq_define_obj(p, t, v, o.clone());
            }
        }
        if !e.pending.is_empty() {
            let p = self.tok_from(&e.pending);
            self.back_list(p)?;
        }
        if e.closes_caller_group {
            self.unsave()?; // (its `\aftergroup` tokens go above `pending`)
        }
        self.set_align_state(self.align_state() + e.align);
        Ok(())
    }

    /// §281, when a group has just closed while recording (`group` is the
    /// type it had, `input_ptr` the input level below its `\aftergroup`
    /// tokens): finish the recording if this ends the call.
    pub(crate) fn memo_group_closed(&mut self, group: i32, input_ptr: usize) {
        let Some(level) = self.memo.rec.as_ref().map(|r| r.level) else {
            return;
        };
        if self.cur_level() > level {
            return; // a group inside the call
        }
        let Some(mut r) = self.memo.rec.take() else {
            return;
        };
        self.memo.log.take_into(&mut r);
        self.memo.live = false;
        let closes_caller_group = self.cur_level() < level;
        let entry = if group == SIMPLE_GROUP || group == SEMI_SIMPLE_GROUP {
            self.memo_finish(&r, closes_caller_group, input_ptr)
        } else {
            Err("ended by a non-simple group".into())
        };
        if let Some(old) = &r.check {
            self.memo.stats.checked += 1;
            let what = match &entry {
                Err(why) => Some(format!(
                    "replayability: {why} (consumed {} stored, {} fresh)",
                    old.consumed.len(),
                    r.consumed.len()
                )),
                Ok(e) if e.consumed != old.consumed => Some("consumed tokens".into()),
                Ok(e) if e.globals != old.globals => Some("global writes".into()),
                Ok(e) if e.pending != old.pending => Some("pending tokens".into()),
                Ok(e) if e.closes_caller_group != old.closes_caller_group => {
                    Some("group end".into())
                }
                Ok(e) if e.align != old.align => Some("align_state".into()),
                Ok(_) => None,
            };
            if let Ok(e) = &entry
                && e.pending != old.pending
                && self.memo.pending_diffs.len() < 4
            {
                self.memo
                    .pending_diffs
                    .push((old.pending.clone(), e.pending.clone()));
            }
            if entry.is_err() && self.memo.read_diffs.len() < 4 {
                let fresh = r.reads.iter().map(|&(c, v, _)| (c, v)).collect();
                let stored = old.reads.iter().map(|&(c, v, _)| (c, v)).collect();
                self.memo
                    .read_diffs
                    .push((stored, fresh, r.consumed.clone()));
            }
            if let Some(w) = what
                && self.memo.mismatches.len() < 50
            {
                self.memo.mismatches.push((r.cs, w));
            }
            return;
        }
        let e = match entry {
            Ok(e) => e,
            Err(why) => {
                self.memo.stats.abandoned += 1;
                let d = &mut self.memo.defs[r.slot];
                d.impure += 1;
                d.why = why;
                return;
            }
        };
        let (len, h) = (
            u32::try_from(e.consumed.len()).unwrap_or(u32::MAX),
            tok_hash(&e.consumed),
        );
        let Some(slot) = self.memo.slot_mut(r.def, r.key) else {
            return;
        };
        if let Err(i) = slot.lens.binary_search(&len) {
            if slot.lens.len() >= MAX_LENS {
                return; // (too many shapes of input for one key)
            }
            slot.lens.insert(i, len);
        }
        self.memo.stats.recorded += 1;
        self.memo.stats.recorded_work += e.work;
        let d = &mut self.memo.defs[r.slot];
        d.recorded += 1;
        d.rec_work += e.work;
        d.pinned = true;
        let Some(slot) = self.memo.slot_mut(r.def, r.key) else {
            return;
        };
        let v = slot.buckets.entry((len, h)).or_default();
        let old = if v.len() >= PER_KEY {
            Some(v.remove(0))
        } else {
            None
        };
        v.push(e);
        drop(old);
    }

    /// The entry for the recording that just ended, if it can be replayed.
    fn memo_finish(
        &mut self,
        r: &Rec,
        closes_caller_group: bool,
        top: usize,
    ) -> Result<Entry, String> {
        if r.bad {
            return Err(format!("impure at {}:{} cmd {}", r.why.0, r.why.1, r.why.2));
        }
        let fail = [
            (self.memo.printed != r.printed, "printed"),
            (self.cond_stack.len() != r.cond, "conditionals"),
            (self.after_token() != 0, "\\afterassignment"),
            (self.mode() != r.mode, "mode"),
            (
                !closes_caller_group && r.local_at_start,
                "local write at start level",
            ),
        ];
        if let Some(&(_, why)) = fail.iter().find(|f| f.0) {
            return Err(why.into());
        }
        // The tokens still to be read above the caller's level, top first.
        // When the caller's group closed, its `\aftergroup` tokens (pushed
        // above `top`) are left out: replay's own `unsave` pushes them.
        let top = if closes_caller_group {
            top
        } else {
            self.input_ptr
        };
        let mut pending = Vec::new();
        for i in (self.memo.floor.get() + 1..=top).rev() {
            let s = if i == self.input_ptr {
                &self.cur_input
            } else {
                &self.input_stack[i]
            };
            if s.state != TOKEN_LIST || s.index == U_TEMPLATE || s.index == V_TEMPLATE {
                return Err("pending file or template".into());
            }
            if s.loc == NULL {
                continue;
            }
            let list = s.tokens_in(&self.param_stack);
            for &t in &list[ux(s.loc)..] {
                if s.index == MACRO && tok_cmd(t) == OUT_PARAM {
                    if let Some(p) = &self.param_stack[ux(s.limit + tok_chr(t) - 1)] {
                        pending.extend_from_slice(p);
                    }
                } else {
                    pending.push(t);
                }
            }
        }
        let pins: Vec<i32> = Vec::new();
        // A read value that names an object in a store whose ids are
        // reused keeps the call from being recorded (glue, boxes, shapes);
        // token lists are told apart by generation (`memo_read_ok`).
        for &(c, v, _) in &r.reads {
            let Cell::Eqtb(p) = c else { continue };
            if (INT_BASE..=EQTB_SIZE).contains(&p) || (p >= EXT_BASE && is_word_kind(ext_reg(p).0))
            {
                continue;
            }
            let w = crate::mem::MemoryWord::from_bits(v);
            let t = w.b0();
            if matches!(t, GLUE_REF | SHAPE_REF | BOX_REF)
                || (GLUE_BASE..LOCAL_BASE).contains(&p)
                || (BOX_BASE..CUR_FONT_LOC).contains(&p)
            {
                return Err("read a glue, box or shape".into());
            }
        }
        let mut globals = Vec::new();
        let mut global_objs = Vec::new();
        for &p in &r.global {
            let w = self.peek_eqtb(p);
            let word = (INT_BASE..=EQTB_SIZE).contains(&p)
                || (p >= EXT_BASE && is_word_kind(ext_reg(p).0));
            if !word {
                let t = w.b0();
                if matches!(t, GLUE_REF | SHAPE_REF | BOX_REF)
                    || (GLUE_BASE..LOCAL_BASE).contains(&p)
                    || p >= BOX_BASE && p < CUR_FONT_LOC
                {
                    return Err("wrote a glue, box or shape".into());
                }
            }
            globals.push((p, w.bits()));
            global_objs.push(self.peek_obj(p).cloned());
        }
        Ok(Entry {
            work: self.memo.work.get(),
            args: r.args.clone(),
            consumed: r.consumed.clone(),
            mode: r.mode,
            group: r.group,
            reads: r.reads.clone(),
            globals,
            global_objs,
            closes_caller_group,
            pending,
            align: self.align_state() - r.align,
            pins,
        })
    }

    /// Whether command `cmd`, `chr` may run inside a recorded call: only
    /// assignments to cells, grouping and commands that do nothing.
    pub(crate) fn memo_command_ok(&self, cmd: i32) -> bool {
        match cmd {
            RELAX | IGNORE_SPACES | AFTER_ASSIGNMENT | AFTER_GROUP | BEGIN_GROUP | END_GROUP
            | LEFT_BRACE | RIGHT_BRACE | TOKS_REGISTER | ASSIGN_TOKS | ASSIGN_INT
            | ASSIGN_DIMEN | ASSIGN_GLUE | ASSIGN_MU_GLUE | DEF_CODE | XETEX_DEF_CODE
            | DEF_FAMILY | SET_FONT | REGISTER | ADVANCE | MULTIPLY | DIVIDE | PREFIX | LET
            | SHORTHAND_DEF | DEF => true,
            SPACER => self.mode().abs() != HMODE,
            _ => false,
        }
    }

    /// Whether expandable command `cmd`, `chr` may run inside a recorded
    /// call: what depends only on tokens and cells.
    pub(crate) fn memo_expand_ok(cmd: i32, chr: i32) -> bool {
        match cmd {
            EXPAND_AFTER | NO_EXPAND | CS_NAME | FI_OR_ELSE | THE => true,
            IF_TEST => !matches!(
                chr % UNLESS_CODE,
                IF_EOF_CODE | IF_FONT_CHAR_CODE | IF_IN_CSNAME_CODE
            ),
            CONVERT => matches!(
                chr,
                NUMBER_CODE
                    | ROMAN_NUMERAL_CODE
                    | STRING_CODE
                    | MEANING_CODE
                    | ETEX_REVISION_CODE
                    | EXPANDED_CODE
                    | PDF_STRCMP_CODE
                    | JOB_NAME_CODE
            ),
            _ => false,
        }
    }

    /// Whether `\the`-like access to internal quantity `cmd`, `chr` stays
    /// within cells (not fonts, boxes, the current list or the page).
    pub(crate) fn memo_internal_ok(cmd: i32, chr: i32) -> bool {
        match cmd {
            ASSIGN_FONT_DIMEN | ASSIGN_FONT_INT | SET_AUX | SET_PREV_GRAF | SET_PAGE_DIMEN
            | SET_PAGE_INT | SET_BOX_DIMEN => false,
            LAST_ITEM => {
                chr == ETEX_VERSION_CODE
                    || (GLUE_STRETCH_ORDER_CODE..=GLUE_SHRINK_ORDER_CODE).contains(&chr)
                    || (PAR_SHAPE_LENGTH_CODE..=GLUE_SHRINK_CODE).contains(&chr)
                    || chr == MU_TO_GLUE_CODE
                    || chr == GLUE_TO_MU_CODE
                    || chr >= ETEX_EXPR
            }
            _ => true,
        }
    }

    /// Memo counters, for reports.
    pub fn memo_stats(&self) -> MemoStats {
        self.memo.stats
    }

    /// The cells that most often made a recorded call stale, by name.
    pub fn memo_stale_cells(&self) -> Vec<(alloc::vec::Vec<u8>, u64)> {
        let mut v: Vec<(Cell, u64)> = self
            .memo
            .stale_cells
            .iter()
            .map(|(&c, &n)| (c, n))
            .collect();
        v.sort_by_key(|x| core::cmp::Reverse(x.1));
        v.truncate(30);
        let cells: Vec<Cell> = v.iter().map(|x| x.0).collect();
        self.cell_names(&cells)
            .into_iter()
            .zip(v.iter().map(|x| x.1))
            .collect()
    }

    /// Check mode (`PARTEX_MEMO=check`): record every call a replay would
    /// answer and compare instead of replaying.
    pub fn set_memo_check(&mut self) {
        self.memo.check = true;
    }

    /// Check mode: the first differing pending tokens, shown as
    /// (stored, fresh).
    pub fn memo_pending_diffs(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let show = |toks: &[i32]| {
            let mut v = Vec::new();
            for &t in toks {
                if t >= CS_TOKEN_FLAG {
                    let cs = t - CS_TOKEN_FLAG;
                    v.extend(self.cell_names(&[Cell::Eqtb(cs)]).concat());
                    v.push(b' ');
                } else {
                    v.push(u8::try_from(tok_chr(t)).unwrap_or(b'?'));
                }
            }
            v
        };
        self.memo
            .pending_diffs
            .iter()
            .map(|(a, b)| (show(a), show(b)))
            .collect()
    }

    /// Check mode: for calls not replayable again, where the fresh reads
    /// first departed from the stored ones: lines of cell, stored value,
    /// fresh value.
    pub fn memo_read_diffs(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for (old, new, consumed) in &self.memo.read_diffs {
            let i = old.iter().zip(new).take_while(|(a, b)| a == b).count();
            let mut v = format!(
                "reads {} stored, {} fresh, same for the first {i}; consumed {}:",
                old.len(),
                new.len(),
                consumed.len()
            )
            .into_bytes();
            for j in i.saturating_sub(2)..(i + 3) {
                for (tag, list) in [(b"stored", old), (b"fresh ", new)] {
                    if let Some(&(c, x)) = list.get(j) {
                        v.extend_from_slice(b"\n    ");
                        v.extend_from_slice(tag);
                        v.extend(format!(" #{j} ").bytes());
                        v.extend(self.cell_names(&[c]).concat());
                        v.extend(format!(" = {x:#x}").bytes());
                    }
                }
            }
            out.push(v);
        }
        out
    }

    /// Per definition, the most called: (macro, calls, hits, stale,
    /// recorded, abandoned).
    pub fn memo_defs(&self, n: usize) -> Vec<(Vec<u8>, [u32; 5], String)> {
        let mut v: Vec<&DefStats> = self.memo.defs.iter().skip(1).collect();
        v.sort_by_key(|d| core::cmp::Reverse(d.calls));
        v.truncate(n);
        let names = self.cell_names(&v.iter().map(|d| Cell::Eqtb(d.cs)).collect::<Vec<_>>());
        names
            .into_iter()
            .zip(v)
            .map(|(name, d)| {
                (
                    name,
                    [d.calls, d.hits, d.stale, d.recorded, d.impure],
                    d.why.clone(),
                )
            })
            .collect()
    }

    /// Debugging (bisecting a replay bug): replay only the first `n` hits.
    pub fn set_memo_limit(&mut self, n: u64) {
        self.memo.limit = n;
    }

    /// With a limit: the macro of the last hit replayed.
    pub fn memo_last_hit(&self) -> Vec<u8> {
        self.cell_names(&[Cell::Eqtb(self.memo.last_hit)])
            .pop()
            .unwrap_or_default()
    }

    /// Check mode: the calls whose fresh record differed from the stored
    /// one, by macro name.
    pub fn memo_mismatches(&self) -> Vec<(alloc::vec::Vec<u8>, String)> {
        let cells: Vec<Cell> = self
            .memo
            .mismatches
            .iter()
            .map(|m| Cell::Eqtb(m.0))
            .collect();
        self.cell_names(&cells)
            .into_iter()
            .zip(self.memo.mismatches.iter().map(|m| m.1.clone()))
            .collect()
    }
}
