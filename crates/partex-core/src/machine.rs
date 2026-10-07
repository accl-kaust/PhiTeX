//! The engine as a `partex_incr::Machine` (DESIGN.md §7.0, "TeX's
//! adapter"): the runtime's builds, rebuilds and link run TeX.
//!
//! The cells:
//!
//! - `Eqtb(p)`, one cell per eqtb word below the registers above 255
//!   (dense: flat recording keeps them in bit sets). A value is the word
//!   with its level and the token list, glue, shape or box it names, by
//!   content (`CellValue`); its version is [`Tex::cell_content`].
//! - `Line(path, k)`: line `k` of an input file read by lines (`\input`,
//!   `\openin`), as the host serves it (absent past the end). A region
//!   depends on the lines it read, not on the whole file, so an edit
//!   dirties the regions that read the edited lines.
//! - `File(path)`: a file as the host serves it. Files read by lines are
//!   read through their `Line` cells; the `File` cell carries the
//!   contents a rebuild's starting state serves.
//! - `Rest`: everything else (the input stack and scanner, conditionals,
//!   the nest and page builder, the hash table, fonts, the writers'
//!   state, what the host keeps of the `\write` files). Its value is a
//!   snapshot of the whole engine (copy-on-write, so a snapshot costs
//!   what changed since the last one); its version hashes everything but
//!   the eqtb words and the input files served by lines, which count by
//!   the lines read ([`Tex::rest_hash_memo`]: a tree of sub-hashes, so
//!   hashing costs what changed). Setting it takes the snapshot's state
//!   but keeps this state's eqtb words and input files (an open file
//!   whose contents changed is read on from the same line), so a
//!   region's writes apply to a state that differs from the one it ran
//!   in exactly as the runtime expects.
//!
//! A step runs main control up to the next command read straight from a
//! file (a boundary: a candidate for a region's end). What a region read
//! and wrote is reported when it ends (`prepare_cut`): first reads of
//! eqtb words by their versions at the region's entry (a snapshot the
//! machine keeps), so reading costs a bit test, not a hash.
//!
//! Every region reads `Rest` at its start, so a rebuild re-executes from
//! the first changed region until the whole rest of the state agrees
//! with the previous run's at a region boundary (early cutoff); eqtb and
//! line cells then decide which later regions still differ (read-set
//! cutoff).
//!
//! Effects are the engine's output as values (`effects.rs`); the link is
//! [`crate::effects::link`] over the regions' effects in program order.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hash::{Hash, Hasher};

use partex_engine::node::Node;
use partex_engine::stablehash::StableHasher;
use partex_incr::link::LinkCtx;
use partex_incr::{Machine, Recorder, Step, Version};

use crate::cmds::{BOX_REF, CALL, GLUE_REF, LONG_OUTER_CALL, SHAPE_REF};
use crate::effects::Effect;
use crate::host::Host;
use crate::mem::MemoryWord;
use core::sync::atomic::AtomicU8;
use core::sync::atomic::Ordering::Relaxed;

use crate::pdf::objtab::{Id, NameCell};
use crate::relaxed::{Flag, Log};
use crate::run;
use crate::tex::Tex;
use crate::track::{Cell, Tracker};
use crate::web::{ACTIVE_BASE, EQTB_SIZE, INT_BASE, TOKEN_LIST};

// (a build kept outside the process: `machine_store.rs`)
#[path = "machine_store.rs"]
mod store;
pub use store::{
    Cx, MakeLazy, Part, Parts, Regions, SavedChunk, StoreHost, assemble, assemble_later, census,
    check_digest, check_snapshots, chunk_of, load_build, load_chunk, load_final, load_parts,
    load_snapshot_body, save_build,
};

/// A file's contents with where its lines begin ([`line_starts`]).
pub type FileLines = (Arc<[u8]>, Arc<[usize]>);

/// What the machine needs from its host beyond [`Host`]: input files as
/// cells, and a hash of what the host keeps that belongs to `Rest`.
pub trait CellHost: Host + Clone + Send + Sync {
    /// The input files served since the last call, by resolved path.
    fn take_reads(&mut self) -> Vec<Vec<u8>>;
    /// What the host serves for `path` (`None`: not served yet or absent).
    fn file(&self, path: &[u8]) -> Option<Arc<[u8]>>;
    /// Serve `contents` for `path` from now on.
    fn set_file(&mut self, path: &[u8], contents: Option<Arc<[u8]>>);
    /// What the host serves for `path` with where its lines begin
    /// ([`line_starts`]; a host keeps them, as it keeps the files).
    fn lines(&self, path: &[u8]) -> Option<FileLines>;
    /// A hash of the host's own state (which `\write` files are open),
    /// not of the input files or of what the `\write` files hold.
    fn digest(&self) -> u128;
    /// What `\write` file `id` holds so far (a cell of its own,
    /// [`MCell::Written`]).
    fn written(&self, id: u32) -> Option<Arc<Vec<u8>>>;
    /// Every `\write` file's id.
    fn written_ids(&self) -> Vec<u32>;
    /// Append `bytes` to `\write` file `id` (replaying a region).
    fn append_written(&mut self, id: u32, bytes: &[u8]);
    /// Whether reading the file `name` gives what the job wrote to it (a
    /// `\write` file read back, as it shadows an input file of the same
    /// name: its lines are not the input file's).
    fn reads_back(&self, name: &[u8]) -> bool;
    /// What the job appended to `\write` files since the last call, by
    /// file, and the `\write` files it read back.
    fn take_written(&mut self) -> (alloc::collections::BTreeMap<u32, Vec<u8>>, Vec<u32>);
    /// A host with `snapshot`'s state and this host's input files
    /// (setting `Rest`).
    #[must_use]
    fn clone_state_from(&self, snapshot: &Self) -> Self;
    /// A clock in nanoseconds, for `PARTEX_CUT_TIMING` ([`CUT_TIMING`]);
    /// read only when that is on.
    fn nanos(&self) -> u64 {
        0
    }
}

/// Whether a cut's pieces are timed (`PARTEX_CUT_TIMING=1`), with the
/// host's clock: [`cut_timing_take`] returns the sums.
pub static CUT_TIMING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// With [`CUT_TIMING`], the measurements that cost work of their own (a
/// second clone of the token store per cut, the engine's field groups
/// cloned again per snapshot): `PARTEX_CUT_TIMING=2`. Without it, the
/// timing only reads the clock, and a rebuild's wall time is its own.
pub static CUT_TIMING_DETAIL: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
#[allow(clippy::declare_interior_mutable_const)]
const TZ: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The sums of [`CutTiming`], in its order.
static CUT_T: [core::sync::atomic::AtomicU64; 92] = [TZ; 92];

/// What the cuts since the last [`cut_timing_take`] cost, by piece
/// (nanoseconds, summed), and the token store's sizes at the last one.
/// When timing is on, a cut runs its commits first, one piece at a time
/// (the `Rest` hash's own commit then finds nothing to do), and clones
/// the token store once more to time that alone.
#[derive(Clone, Copy, Debug, Default)]
pub struct CutTiming {
    pub cuts: u64,
    /// The token store's commit (`frozen_now` and the flag reset).
    pub tok_commit: u64,
    /// A clone of the token store, just committed.
    pub tok_clone: u64,
    /// eqtb's, the hash's and the save stack's commits.
    pub jvec_commits: u64,
    /// The five `Flat`s' commits.
    pub flat_commits: u64,
    /// The `Rest` hash, its commits done.
    pub rest_hash: u64,
    /// `Snapshot::of`: the lists, the engine's clone.
    pub snapshot_of: u64,
    pub lists: u64,
    pub free: u64,
    /// The pooled lists, in use or not (`tok.rs`, `pooled_new`).
    pub pooled: u64,
    /// Each `Flat`'s commit (`str_pool`, `str_start`, `buffer`,
    /// `input_stack`, `param_stack`), and at the last cut its length and
    /// its live prefix, in bytes.
    pub flat: [u64; 5],
    pub flat_bytes: [u64; 5],
    pub flat_live: [u64; 5],
    /// Each `JVec`'s commit (eqtb, the hash, the save stack): the
    /// nanoseconds walking its dirty bits, comparing and copying, and its
    /// chunks at the last cut, the dirty chunks summed and at most, and
    /// the chunks copied summed.
    pub jvec: [[u64; 7]; 3],
    /// `save_ptr` at the last cut (the save stack's live prefix, words).
    pub save_ptr: u64,
    /// `Snapshot::of`'s clone of the engine, whole, then by field group
    /// ([`CLONE_GROUPS`]), each cloned again on its own to time it.
    pub clone_whole: u64,
    pub clone_groups: [u64; CLONE_GROUPS.len()],
    /// The restores (`restore_rest`) since the last call, and their
    /// pieces summed ([`RESTORE_PIECES`]).
    pub restores: u64,
    pub restore: [u64; RESTORE_PIECES.len()],
    /// `replay_exit`s, and their pieces summed ([`REPLAY_PIECES`]); the
    /// snapshot each takes is also counted as a cut.
    pub replays: u64,
    pub replay: [u64; REPLAY_PIECES.len()],
}

/// The pieces of a `replay_exit` [`CutTiming::replay`] times.
pub const REPLAY_PIECES: [&str; 5] = [
    "restore_rest",
    "positions and the span's accumulating cells set",
    "the patched cells set",
    "the tracker reset and the snapshot taken (a cut)",
    "all",
];

/// The pieces of a restore [`CutTiming::restore`] times.
pub const RESTORE_PIECES: [&str; 9] = [
    "the snapshot's engine cloned (Snapshot::engine)",
    "eqtb's differences and the cells exported",
    "thaw: the token store",
    "thaw: the JVecs",
    "thaw: the Flats",
    "thaw: the memo's tables",
    "the cells imported and the rest taken over",
    "the old engine dropped",
    "all",
];

/// The field groups [`CutTiming::clone_groups`] times (DESIGN.md
/// §7.16.3's table).
pub const CLONE_GROUPS: [&str; 21] = [
    "objects (objs, xregs, xeq_level)",
    "lists' records (nest, cur_list, page, align, adjust, cur_box, split_discards)",
    "JVecs (eqtb, hash, save stack)",
    "token store and Flats",
    "fonts",
    "hyph",
    "prims",
    "fontmap, fonts_mapped",
    "cur_mark",
    "seals",
    "cs_cache",
    "str_index",
    "skip",
    "memo",
    "pdf: objs",
    "pdf: out",
    "pdf: ship",
    "pdf: fontw",
    "pdf: all",
    "output buffers (log, write files, dvi, effects, the logs)",
    "host",
];

/// Read and reset the sums of the cuts' timing.
#[must_use]
pub fn cut_timing_take() -> CutTiming {
    let t: [u64; 92] = core::array::from_fn(|i| CUT_T[i].swap(0, Relaxed));
    CutTiming {
        cuts: t[0],
        tok_commit: t[1],
        tok_clone: t[2],
        jvec_commits: t[3],
        flat_commits: t[4],
        rest_hash: t[5],
        snapshot_of: t[6],
        lists: t[7],
        free: t[8],
        pooled: t[9],
        flat: core::array::from_fn(|i| t[10 + i]),
        flat_bytes: core::array::from_fn(|i| t[15 + i]),
        flat_live: core::array::from_fn(|i| t[20 + i]),
        jvec: core::array::from_fn(|j| core::array::from_fn(|k| t[25 + 7 * j + k])),
        save_ptr: t[46],
        clone_whole: t[47],
        clone_groups: core::array::from_fn(|i| t[48 + i]),
        restores: t[74],
        restore: core::array::from_fn(|i| t[75 + i]),
        replays: t[84],
        replay: core::array::from_fn(|i| t[85 + i]),
    }
}

/// The tracker of a machine region: first reads and first writes of eqtb
/// words (the registers above 255 included) since the region began.
/// Updated through `&self` with relaxed atomics (the engine stays
/// `Sync`): a byte per location, so a read seen before costs one load; a
/// clone is empty (it is scratch between two regions).
#[derive(Default)]
pub struct CellTracker {
    /// Per slot ([`CellTracker::slot`]): `READ` if first read, `WRITTEN`
    /// once written, and `SOFT` after a local assignment read it softly
    /// (`Tracker::soft_read`, at group level `soft_level`), `SAVED` if the
    /// save holds its value at the region's entry.
    state: Vec<AtomicU8>,
    soft_level: Vec<AtomicU8>,
    /// Slots read and written first.
    reads: Log<i32>,
    writes: Log<i32>,
    /// The soft reads, as a stack by group level: slot and level.
    softs: Log<u64>,
    overflow: Flag,
    /// The first slot of the registers above 255 (eqtb's length).
    xbase: usize,
    /// With names as cells (`Tex::name_cells`): a hash slot's name shares
    /// its eqtb word's slot (`Cell::Hash`), and its link has a slot of its
    /// own from `lbase` (`Cell::HashNext`); 0 without.
    lbase: usize,
    /// With fonts as cells (`Tex::font_cells`): font `f`'s slot is
    /// `fbase + f` (`Cell::Font`), up to `fend`; 0 without.
    fbase: usize,
    fend: usize,
    /// Sized since it was made or reset ([`CellTracker::reserve`]): until
    /// then its tables are sized exactly, as a fresh tracker's are.
    sized: bool,
    /// The page builder's state (`MCell::Page`): `READ` if read first,
    /// `WRITTEN` once written.
    page: AtomicU8,
    /// The current marks (`MCell::Marks`): `READ` if read first,
    /// `WRITTEN` once written.
    marks: AtomicU8,
    /// The `\pdflast…` values (`MCell::PdfLast`): bit `k` if value `k`
    /// was read first, bit `16 + k` once written.
    pdf_last: core::sync::atomic::AtomicU32,
    /// The PDF writer's words (`MCell::PdfWord`) the tracker is told of
    /// (`pdf::word`; the lists' heads are the object table's to log):
    /// bit `k` if word `k` was read first, bit `16 + k` once written.
    pdf_words: core::sync::atomic::AtomicU32,
    /// Relocatable numbers (`reloc.rs`).
    reloc: RelocLog,
}

/// What a tracker's slot stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Tracked {
    /// An eqtb word (a register above 255 included).
    Eqtb(i32),
    /// A hash slot's link.
    Link(i32),
    /// A font.
    Font(i32),
}

/// Slots for the registers above 255: six kinds of 32768.
const XREG_SLOTS: usize = 6 * 0x8000;

/// The dense number of the first register above 255 (`Machine::index`):
/// past eqtb at its largest (`hash_extra` at most 2097151).
#[allow(clippy::cast_sign_loss)] // (a positive constant)
const XREG_INDEX: u32 = 3 + EQTB_SIZE as u32 + 2_097_152;

/// The dense number of the first hash slot's link (`MCell::Link`), past
/// the registers above 255.
#[allow(clippy::cast_possible_truncation)] // (a small constant)
const LINK_INDEX: u32 = XREG_INDEX + XREG_SLOTS as u32;

/// The dense number of font slot 0 (`MCell::Font`), past the links.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // (a small constant)
const FONT_INDEX: u32 = LINK_INDEX + EQTB_SIZE as u32 + 2_097_152;

/// A hash slot's link as a version.
fn link_version(k: i32) -> u128 {
    StableHasher::of(&k.to_le_bytes())
}

/// Where a name is (0: nowhere) as a version.
fn name_version(p: i32) -> u128 {
    let mut h = StableHasher::new();
    (b"name", p).hash(&mut h);
    h.finish128()
}

const READ: u8 = 1;
const WRITTEN: u8 = 2;
const SOFT: u8 = 4;
const SAVED: u8 = 8;

/// Soft reads (`Tracker::soft_read`; on by default, process-wide:
/// `PARTEX_MACHINE_SOFTREADS=0`, `set_soft_reads`).
static SOFT_READS_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Whether a state restored from a snapshot and patched only in cells
/// takes `Rest`'s version from the trace instead of hashing it (on by
/// default), and whether that is checked by hashing it anyway (the
/// sanitizer's); the process's.
static KNOWN_REST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);
static VERIFY_REST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether the line numbers of the input are a cell of their own
/// ([`MCell::Positions`]) and not `Rest`'s (`PARTEX_MACHINE_RENAME=1`,
/// DESIGN.md §7.15; off by default).
pub fn set_position_cells(on: bool) {
    crate::statehash::POSITION_CELLS.store(on, Relaxed);
}

/// Whether the input's line numbers are a cell of their own and a rebuild
/// renames them through the files' diffs (`PARTEX_MACHINE_RENAME=1`).
#[must_use]
pub fn renaming() -> bool {
    position_cells()
}

fn position_cells() -> bool {
    crate::statehash::POSITION_CELLS.load(Relaxed)
}

/// Whether the page builder's state is a cell of its own
/// ([`MCell::Page`]) and not `Rest`'s (DESIGN.md §7.16.2; on by default,
/// `PARTEX_MACHINE_PAGE_CELL=0` turns it off).
pub fn set_page_cells(on: bool) {
    crate::statehash::PAGE_CELLS.store(on, Relaxed);
}

fn page_cells() -> bool {
    crate::statehash::PAGE_CELLS.load(Relaxed)
}

/// Whether the current marks are a cell of their own ([`MCell::Marks`])
/// and not `Rest`'s (on by default, `PARTEX_MACHINE_MARKS=0` turns it
/// off).
pub fn set_mark_cells(on: bool) {
    crate::statehash::MARK_CELLS.store(on, Relaxed);
}

fn mark_cells() -> bool {
    crate::statehash::MARK_CELLS.load(Relaxed)
}

#[path = "machine_reloc.rs"]
mod machine_reloc;

/// Whether machines number with relocatable numbers (`reloc.rs`: tagged
/// digits, `MCell::Origin` and `MCell::IntCmp` guards; on by default,
/// `PARTEX_MACHINE_RELOCATE=0` turns it off).
static RELOCATE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Turn [`RELOCATE`] on or off for the machines made after.
pub fn set_relocate(on: bool) {
    RELOCATE.store(on, Relaxed);
}

/// An origin that stands for all of them (`MCell::Origin`): a region
/// whose answers overflowed the tracker's log.
pub const ANY_ORIGIN: i32 = -1;

/// The version of every `MCell::Origin` guard (the guard always holds).
const ORIGIN_VERSION: u128 = 0x6f72_6967_696e;

/// The version of an `MCell::IntCmp` guard: its answer, made from the
/// cell itself.
fn int_cmp_version(x: i32, rel: u8, y: i32) -> u128 {
    let answer = match rel {
        crate::reloc::SHIFT => true,
        b'<' => x < y,
        b'>' => x > y,
        _ => x == y,
    };
    0x696e_7463_6d70_0000 | u128::from(answer)
}

/// What a region did with relocatable numbers (`reloc.rs`): the origins
/// it observed, and the answers it depends on.
#[derive(Default)]
struct RelocLog {
    /// Origins observed, by bit.
    seen: Vec<crate::relaxed::U64>,
    observed: Log<i32>,
    /// (origin and `x`, `rel` and `y`, the answer)
    answers: Log<(u64, u64, u32)>,
    /// An answer did not fit: the region observed every origin.
    overflow: Flag,
}

/// An answer about a number (`MCell::IntCmp`): origin, `x`, `rel`, `y`.
type IntAnswer = (i32, i32, u8, i32);

/// Room for the answers of one step.
const ANSWERS_PER_STEP: usize = 1 << 14;

impl RelocLog {
    fn reserve(&mut self) {
        let n = usize::try_from(crate::web::TAG_ORIGINS).unwrap_or(0) + 1;
        if self.seen.len() < n.div_ceil(64) {
            self.seen.resize_with(n.div_ceil(64), Default::default);
            self.observed.reserve(n);
        }
        self.answers.reserve(self.answers.len() + ANSWERS_PER_STEP);
    }

    #[inline]
    fn observe(&self, o: i32) {
        let Ok(i) = usize::try_from(o) else {
            return;
        };
        match self.seen.get(i / 64) {
            Some(w) => {
                let b = 1u64 << (i % 64);
                let v = w.get();
                if v & b == 0 {
                    w.set(v | b);
                    if !self.observed.push(o) {
                        self.overflow.set(true);
                    }
                }
            }
            None => self.overflow.set(true),
        }
    }

    #[inline]
    fn answer(&self, o: i32, x: i32, rel: u8, y: i32) {
        let a = (u64::from(o.cast_unsigned()) << 32) | u64::from(x.cast_unsigned());
        let b = (u64::from(rel) << 32) | u64::from(y.cast_unsigned());
        if !self.answers.last_is(|l| l.0 == a && l.1 == b) && !self.answers.push((a, b, 0)) {
            self.overflow.set(true);
        }
    }

    /// The origins observed and the answers (origin, `x`, `rel`, `y`),
    /// sorted, and whether the log overflowed; then forget them.
    #[allow(clippy::cast_possible_truncation)] // (the halves)
    fn take(&self) -> (Vec<i32>, Vec<IntAnswer>, bool) {
        let mut obs = self.observed.to_vec();
        for &o in &obs {
            if let Some(w) = usize::try_from(o).ok().and_then(|i| self.seen.get(i / 64)) {
                w.set(0);
            }
        }
        obs.sort_unstable();
        let mut ans: Vec<IntAnswer> = self
            .answers
            .to_vec()
            .into_iter()
            .map(|(a, b, _)| {
                (
                    ((a >> 32) as u32).cast_signed(),
                    (a as u32).cast_signed(),
                    (b >> 32) as u8,
                    (b as u32).cast_signed(),
                )
            })
            .collect();
        ans.sort_unstable();
        ans.dedup();
        self.observed.clear();
        self.answers.clear();
        let overflow = self.overflow.get();
        self.overflow.set(false);
        (obs, ans, overflow)
    }
}

/// The current marks, by class (`Tex::cur_mark`).
pub type MarkMap = alloc::collections::BTreeMap<i32, [Option<crate::tok::Tokens>; 5]>;

/// `MCell::Marks`'s version: the marks' by content.
fn marks_version(m: &MarkMap) -> u128 {
    StableHasher::of(&(b"marks", m))
}

/// Whether a region that asks the numbering for single numbers guards
/// the answers (`MCell::FinalNum`, `OfFinal`, `NumState`) instead of the
/// whole numbering (DESIGN.md §7.16.5; on by default,
/// `PARTEX_MACHINE_NUM_ANSWERS=0` turns it off).
pub fn set_num_answers(on: bool) {
    crate::pdf::objtab::NUM_ANSWERS.store(on, Relaxed);
}

/// Whether a virtual object that TeX identifies (a page by its number, a
/// destination by its name, a leaf of the page tree by its place…) is
/// named by that identity, not by the position of the step that made it
/// (DESIGN.md §7.16.5; on by default, `PARTEX_MACHINE_TREE_NAMES=0`
/// turns it off).
pub fn set_tree_names(on: bool) {
    crate::pdf::objtab::TREE_NAMES.store(on, Relaxed);
}

/// Whether the values `\pdflast…` and `\pdfretval` read are cells of
/// their own ([`MCell::PdfLast`]) and not `Rest`'s (DESIGN.md §7.16.2; on
/// by default, `PARTEX_MACHINE_PDF_LAST=0` turns it off).
pub fn set_pdf_last_cells(on: bool) {
    crate::statehash::PDF_LAST_CELLS.store(on, Relaxed);
}

fn pdf_last_cells() -> bool {
    crate::statehash::PDF_LAST_CELLS.load(Relaxed)
}

/// Whether the PDF writer's words that few routines touch (`pdf::word`)
/// are cells of their own ([`MCell::PdfWord`]) and not `Rest`'s (on by
/// default, `PARTEX_MACHINE_PDF_WORDS=0` turns it off).
pub fn set_pdf_word_cells(on: bool) {
    crate::statehash::PDF_WORD_CELLS.store(on, Relaxed);
}

fn pdf_word_cells() -> bool {
    crate::statehash::PDF_WORD_CELLS.load(Relaxed)
}

/// Whether word `k` of `t`'s PDF writer is a cell of its own: a list's
/// head only while the object table's entries are cells.
fn pdf_word_is_cell<H: Host, T: Tracker>(t: &Tex<H, T>, k: u8) -> bool {
    pdf_word_cells()
        && k < crate::pdf::word::COUNT
        && (k >= crate::pdf::word::HEADS || t.pdf.objs.log.on)
}

/// The version of an answer of the numbering (`FinalNum`, `OfFinal`,
/// `NumState`).
fn num_answer_version<T: Hash>(v: T) -> u128 {
    StableHasher::of(&(b"num", v))
}

/// Whether a machine sets the state that is dead at a clean point to its
/// initial value at every cut (`PARTEX_MACHINE_CANON=0` turns it off),
/// or to junk at every candidate (`PARTEX_MACHINE_POISON=1`: the test that
/// it is dead; DESIGN §7.16.1).
static CANON: AtomicU8 = AtomicU8::new(1);

/// Turn [`CANON`] off (0), on (1), or to poison (2).
pub fn set_canon(mode: u8) {
    CANON.store(mode, Relaxed);
}

/// Set the state that is dead at a clean point to its initial value, or
/// to junk if `poison`: what differs after an edit and is never observed
/// again (Task 14c's class (a)), so that `Rest` compares constants:
/// - the ship's per-page lists and `pdf_v` (`Ship::canonicalize`);
/// - `scaled_out`, which `pdf_print_bp` and `divide_scaled` write and
///   their callers read right after (pdfTeX §690);
/// - `cur_val` and the lineage of `cur_glue` (`glue_origin`), the
///   scanner's results: a command that uses one runs a `scan_…`
///   routine first (§409–§413, §440–§463), so none is read between
///   commands.
fn canonicalize_dead<H: CellHost>(t: &mut Tex<H, CellTracker>, poison: bool) {
    t.pdf.ship.canonicalize(poison);
    t.pdf.out.scaled_out = if poison { 0x0765_4321 } else { 0 };
    t.cur_val = if poison { 0x5ead_beef } else { 0 };
    t.glue_origin = poison.then_some(0xdead_beef);
}

/// `MCell::PdfLast`'s version: the value's.
fn pdf_last_version(v: i32) -> u128 {
    StableHasher::of(&v)
}

/// `MCell::PdfLast(k)`'s field.
fn pdf_last_of(k: u8) -> crate::pdf::PdfLast {
    crate::pdf::PdfLast::ALL[usize::from(k).min(crate::pdf::PdfLast::ALL.len() - 1)]
}

/// The version of the page builder's state `b` whose list is `list`
/// (a snapshot keeps it apart, in chunks): every field, the boxes on the
/// page by content.
fn page_version<'a>(
    b: &partex_engine::builder::Builder,
    list: impl Iterator<Item = &'a [Node]>,
) -> u128 {
    let mut h = StableHasher::new();
    b.contents.hash(&mut h);
    let mut n = 0usize;
    for ch in list {
        n += ch.len();
        for x in ch {
            x.hash(&mut h);
        }
    }
    n.hash(&mut h);
    (
        b.so_far,
        b.max_depth,
        b.least_cost,
        b.best_break,
        b.best_size,
    )
        .hash(&mut h);
    (&b.ins, b.insert_penalties, &b.last, &b.discards).hash(&mut h);
    h.finish128()
}

/// The page builder's state as `MCell::Page`'s value: the builder
/// without its list, and the list in chunks (shared with the snapshot it
/// was taken from, as [`Lists`]).
#[derive(Clone)]
pub struct PageValue {
    builder: partex_engine::builder::Builder,
    list: Vec<Arc<[Node]>>,
}

impl PageValue {
    /// The builder whole.
    fn builder(&self) -> partex_engine::builder::Builder {
        let mut b = self.builder.clone();
        b.list = partex_engine::nodelist::NodeList::from_vec(self.list.concat());
        b
    }
}

/// Turn [`KNOWN_REST`] and its check on or off.
pub fn set_known_rest(on: bool, verify: bool) {
    KNOWN_REST.store(on, Relaxed);
    VERIFY_REST.store(verify, Relaxed);
}

/// Whether a snapshot restore takes the token lists the running engine
/// has unchanged by content (on by default; the process's).
pub fn set_thaw_by_content(_on: bool) {
    // (token lists are shared values: a restore shares them, and there is
    // nothing to thaw)
}

/// Whether a machine keeps its tracker's tables across `set` and restores,
/// cleared by the slots they logged, instead of making new ones (on by
/// default, process-wide: `PARTEX_MACHINE_KEEP_TRACKER=0`,
/// `set_keep_tracker`).
static KEEP_TRACKER: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Turn [`KEEP_TRACKER`] on or off for every machine of the process.
pub fn set_keep_tracker(on: bool) {
    KEEP_TRACKER.store(on, Relaxed);
}

/// Whether clean points are candidates of level 3 (DESIGN §7.16.1), with
/// exhausted token lists popped at `big_switch` so that they are seen (on
/// by default, process-wide: `PARTEX_MACHINE_CLEAN_CUTS=0` gives back the
/// boundaries from before, levels 1 and 2 only).
static CLEAN_CUTS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Turn [`CLEAN_CUTS`] on or off for every machine of the process.
pub fn set_clean_cuts(on: bool) {
    CLEAN_CUTS.store(on, Relaxed);
}

/// Whether [`CLEAN_CUTS`] is on.
pub(crate) fn clean_cuts() -> bool {
    CLEAN_CUTS.load(Relaxed)
}

/// Turn soft reads on or off for every machine of the process.
pub fn set_soft_reads(on: bool) {
    SOFT_READS_ON.store(on, Relaxed);
}

/// A group level as the tracker keeps it.
fn level_byte(level: i32) -> u8 {
    u8::try_from(level.clamp(0, 255)).unwrap_or(u8::MAX)
}

impl Clone for CellTracker {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl CellTracker {
    /// Room for eqtb locations below `n` and the registers above 255, and
    /// with names as cells, `links` hash slots' links.
    fn reserve(&mut self, n: usize, links: usize, fonts: usize) {
        self.reloc.reserve();
        self.xbase = n;
        self.lbase = if links > 0 { n + XREG_SLOTS } else { 0 };
        let n = n + XREG_SLOTS + links;
        (self.fbase, self.fend) = if fonts > 0 { (n, n + fonts) } else { (0, 0) };
        let n = n + fonts;
        if core::mem::replace(&mut self.sized, true) {
            if self.state.len() < n {
                self.state.resize_with(n, AtomicU8::default);
                self.soft_level.resize_with(n, AtomicU8::default);
            }
            self.reads.reserve(n + 1);
            self.writes.reserve(n + 1);
            self.softs.reserve(n + 1);
        } else {
            // (made or reset: the sizes a fresh tracker takes, the slots
            // kept all 0 and the room past the logs' lengths unread)
            self.state.resize_with(n, AtomicU8::default);
            self.soft_level.resize_with(n, AtomicU8::default);
            self.reads.resize(n + 1);
            self.writes.resize(n + 1);
            self.softs.resize(n + 1);
        }
    }

    /// Forget everything, as a fresh tracker would, but keep the tables:
    /// the slots the logs name set back to 0 ([`CellTracker::clear`]),
    /// which are all the slots that are not 0 (a slot's state is set
    /// only with an append to a log, and `group_end` pops a soft read
    /// only as it clears its bits). `soft_level` is left as it is: it is
    /// read only for a slot whose state is `SOFT` or `SAVED`, which
    /// `soft_read` sets together with it. After an overflow a log may
    /// miss a slot: the tables are made again.
    fn reset(&mut self) {
        if self.overflow.get() {
            *self = Self::default();
            return;
        }
        self.clear();
        *self = Self {
            state: core::mem::take(&mut self.state),
            soft_level: core::mem::take(&mut self.soft_level),
            reads: core::mem::take(&mut self.reads),
            writes: core::mem::take(&mut self.writes),
            softs: core::mem::take(&mut self.softs),
            ..Self::default()
        };
    }

    /// Take `old`'s tables, reset, for this tracker, a fresh one (a
    /// restore replaces the engine, whose clone's tracker is fresh); its
    /// own flag and layout stay. `old` is left fresh.
    fn adopt(&mut self, old: &mut Self) {
        if !self.state.is_empty() || old.overflow.get() {
            return;
        }
        old.reset();
        self.state = core::mem::take(&mut old.state);
        self.soft_level = core::mem::take(&mut old.soft_level);
        self.reads = core::mem::take(&mut old.reads);
        self.writes = core::mem::take(&mut old.writes);
        self.softs = core::mem::take(&mut old.softs);
        self.sized = false;
    }

    /// Where `cell`'s byte is: an eqtb word below the registers above 255
    /// at its location, a register above 255 past eqtb.
    #[inline]
    fn slot(&self, cell: Cell) -> Option<usize> {
        match cell {
            Cell::Eqtb(p) if p >= ACTIVE_BASE && p < crate::xregs::EXT_BASE => {
                usize::try_from(p).ok()
            }
            Cell::Eqtb(p) if p >= crate::xregs::EXT_BASE => {
                let i = usize::try_from(p - crate::xregs::EXT_BASE).ok()?;
                (i < XREG_SLOTS).then_some(self.xbase + i)
            }
            // (a name is part of its control sequence's value)
            Cell::Hash(p) if self.lbase > 0 && p < crate::xregs::EXT_BASE => {
                usize::try_from(p).ok()
            }
            Cell::HashNext(p) if self.lbase > 0 => {
                Some(self.lbase + usize::try_from(p - crate::web::HASH_BASE).ok()?)
            }
            Cell::Font(f) if self.fbase > 0 => {
                let s = self.fbase + usize::try_from(f).ok()?;
                (s < self.fend).then_some(s)
            }
            _ => None,
        }
    }

    /// What slot `s` stands for.
    fn loc(&self, s: i32) -> Tracked {
        let at = |base: usize| i32::try_from(usize::try_from(s).unwrap_or(0) - base).unwrap_or(0);
        match usize::try_from(s) {
            Ok(u) if self.fbase > 0 && u >= self.fbase => Tracked::Font(at(self.fbase)),
            Ok(u) if self.lbase > 0 && u >= self.lbase => {
                Tracked::Link(crate::web::HASH_BASE + at(self.lbase))
            }
            Ok(u) if u >= self.xbase => Tracked::Eqtb(crate::xregs::EXT_BASE + at(self.xbase)),
            _ => Tracked::Eqtb(s),
        }
    }

    /// A region ends: forget what was seen.
    fn clear(&self) {
        let softs = self
            .softs
            .to_vec()
            .into_iter()
            .map(|e| i32::try_from(e >> 8).unwrap_or(i32::MAX));
        for p in self
            .reads
            .to_vec()
            .into_iter()
            .chain(self.writes.to_vec())
            .chain(softs)
        {
            if let Some(s) = self.state.get(usize::try_from(p).unwrap_or(usize::MAX)) {
                s.store(0, Relaxed);
            }
        }
        self.reads.clear();
        self.writes.clear();
        self.softs.clear();
        self.page.store(0, Relaxed);
        self.marks.store(0, Relaxed);
        self.pdf_last.store(0, Relaxed);
        self.pdf_words.store(0, Relaxed);
        let _ = self.reloc.take();
    }

    /// The PDF writer's words the region read first, and those it wrote
    /// (by bit: word `k` is bit `k`; the lists' heads are not here).
    fn pdf_words_touched(&self) -> (u16, u16) {
        let v = self.pdf_words.load(Relaxed);
        #[allow(clippy::cast_possible_truncation)] // (the halves)
        (v as u16, (v >> 16) as u16)
    }

    /// The `\pdflast…` values the region read first, and those it wrote
    /// (by bit: value `k` is bit `k`).
    fn pdf_last_touched(&self) -> (u16, u16) {
        let v = self.pdf_last.load(Relaxed);
        #[allow(clippy::cast_possible_truncation)] // (the halves)
        (v as u16, (v >> 16) as u16)
    }

    /// Whether the region read the page builder's state before writing
    /// it, and whether it wrote it.
    fn page_touched(&self) -> (bool, bool) {
        let v = self.page.load(Relaxed);
        (v & READ != 0, v & WRITTEN != 0)
    }

    /// Whether the region read the current marks before writing them,
    /// and whether it wrote them.
    fn marks_touched(&self) -> (bool, bool) {
        let v = self.marks.load(Relaxed);
        (v & READ != 0, v & WRITTEN != 0)
    }

    fn state_of(&self, s: i32) -> u8 {
        self.state
            .get(usize::try_from(s).unwrap_or(usize::MAX))
            .map_or(0, |s| s.load(Relaxed))
    }

    /// The locations the region read first.
    fn read_locs(&self) -> Vec<Tracked> {
        self.reads
            .to_vec()
            .into_iter()
            .map(|s| self.loc(s))
            .collect()
    }

    /// The locations the region wrote (a value restored to the one at
    /// its entry is not a write).
    fn written(&self) -> Vec<Tracked> {
        let mut w: Vec<Tracked> = self
            .writes
            .to_vec()
            .into_iter()
            .filter(|&s| self.state_of(s) & WRITTEN != 0)
            .map(|s| self.loc(s))
            .collect();
        w.sort_unstable();
        w.dedup();
        w
    }

    /// The locations read softly in groups still open (the save stack
    /// holds their values at the entry): reads after all.
    fn soft_pending(&self) -> Vec<Tracked> {
        let mut v: Vec<Tracked> = self
            .softs
            .to_vec()
            .into_iter()
            .map(|e| i32::try_from(e >> 8).unwrap_or(i32::MAX))
            .filter(|&s| self.state_of(s) & (SOFT | READ) == SOFT)
            .map(|s| self.loc(s))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

impl Tracker for CellTracker {
    const LINES: bool = true;
    const CHANGED_IDS: bool = true;

    #[inline]
    fn observe(&self, origin: i32) {
        self.reloc.observe(origin);
    }

    #[inline]
    fn int_answer(&self, origin: i32, x: i32, rel: u8, y: i32, _answer: bool) {
        self.reloc.answer(origin, x, rel, y);
    }

    #[inline]
    fn pdf_last_access(&self, k: u8, write: bool) {
        let v = self.pdf_last.load(Relaxed);
        let (r, w) = (1u32 << k, 1u32 << (16 + k));
        if write {
            self.pdf_last.store(v | w, Relaxed);
        } else if v & w == 0 {
            self.pdf_last.store(v | r, Relaxed);
        }
    }

    #[inline]
    fn pdf_word_access(&self, k: u8, write: bool) {
        let v = self.pdf_words.load(Relaxed);
        let (r, w) = (1u32 << k, 1u32 << (16 + k));
        if write {
            self.pdf_words.store(v | w, Relaxed);
        } else if v & w == 0 {
            self.pdf_words.store(v | r, Relaxed);
        }
    }

    #[inline]
    fn mark_access(&self, write: bool) {
        let v = self.marks.load(Relaxed);
        if v & WRITTEN == 0 {
            // (a write reads first: the marks are changed, not replaced)
            let w = if write { WRITTEN } else { 0 };
            self.marks.store(v | READ | w, Relaxed);
        }
    }

    #[inline]
    fn page_access(&self, write: bool) {
        let v = self.page.load(Relaxed);
        if v & WRITTEN == 0 {
            // (a write reads first: the page is changed, not replaced)
            let w = if write { WRITTEN } else { 0 };
            self.page.store(v | READ | w, Relaxed);
        }
    }

    #[inline]
    fn read(&self, cell: Cell) {
        let Some(p) = self.slot(cell) else {
            return;
        };
        let Some(s) = self.state.get(p) else {
            self.overflow.set(true);
            return;
        };
        let v = s.load(Relaxed);
        if v & (READ | WRITTEN) == 0 {
            s.store(v | READ, Relaxed);
            if !self.reads.push(i32::try_from(p).unwrap_or(0)) {
                self.overflow.set(true);
            }
        }
    }

    const SOFT_READS: bool = true;

    fn soft_read(&self, cell: Cell, level: i32) {
        if !SOFT_READS_ON.load(Relaxed) {
            self.read(cell);
            return;
        }
        let Some(p) = self.slot(cell) else {
            return;
        };
        let (Some(s), Some(l)) = (self.state.get(p), self.soft_level.get(p)) else {
            self.overflow.set(true);
            return;
        };
        if s.load(Relaxed) == 0 {
            s.store(SOFT, Relaxed);
            let lv = level_byte(level);
            l.store(lv, Relaxed);
            if !self.softs.push(((p as u64) << 8) | u64::from(lv)) {
                self.overflow.set(true);
            }
        }
    }

    fn saved(&self, cell: Cell, level: i32) {
        let Some(p) = self.slot(cell) else {
            return;
        };
        let (Some(s), Some(l)) = (self.state.get(p), self.soft_level.get(p)) else {
            return;
        };
        let v = s.load(Relaxed);
        if v == SOFT {
            // (the value saved is the one at the region's entry)
            s.store(SOFT | SAVED, Relaxed);
            let lv = level_byte(level);
            if l.load(Relaxed) != lv {
                l.store(lv, Relaxed);
                if !self.softs.push(((p as u64) << 8) | u64::from(lv)) {
                    self.overflow.set(true);
                }
            }
        } else if v & SAVED != 0 && l.load(Relaxed) == level_byte(level) {
            // (saved again at the same level: a global assignment came
            // between, §279, and this save holds its value, which the
            // group's end restores first; the entry's value never comes
            // back, so that restore is a change after all)
            s.store(v & !SAVED, Relaxed);
        }
    }

    fn restored(&self, cell: Cell, level: i32) {
        let Some(p) = self.slot(cell) else {
            return;
        };
        let (Some(s), Some(l)) = (self.state.get(p), self.soft_level.get(p)) else {
            return;
        };
        let v = s.load(Relaxed);
        if v & (SAVED | READ) == SAVED && l.load(Relaxed) == level_byte(level) {
            // (back to the value at the entry, never observed: untouched)
            s.store(0, Relaxed);
        }
    }

    fn group_end(&self, level: i32) {
        let lv = level_byte(level);
        while let Some(e) = self.softs.last() {
            #[allow(clippy::cast_possible_truncation)]
            if (e & 0xff) as u8 >= lv {
                self.softs.pop();
                let p = usize::try_from(e >> 8).unwrap_or(usize::MAX);
                if let (Some(s), Some(l)) = (self.state.get(p), self.soft_level.get(p)) {
                    let v = s.load(Relaxed);
                    if v & SOFT != 0 && l.load(Relaxed) >= lv {
                        // (the group read softly in ended: the value is
                        // the entry's again, or one written since)
                        s.store(v & !(SOFT | SAVED), Relaxed);
                    }
                }
            } else {
                break;
            }
        }
    }

    #[inline]
    fn write(&self, cell: Cell) {
        let Some(p) = self.slot(cell) else {
            return;
        };
        let Some(s) = self.state.get(p) else {
            self.overflow.set(true);
            return;
        };
        let v = s.load(Relaxed);
        if v & WRITTEN == 0 {
            s.store(v | WRITTEN, Relaxed);
            if !self.writes.push(i32::try_from(p).unwrap_or(0)) {
                self.overflow.set(true);
            }
        }
    }
}

/// Whether eqtb location `p` holds a reference with its level in the
/// word (a meaning, a glue, box or token register, a control sequence of
/// `hash_extra` too); else an integer or dimension with its level apart
/// (`xeq_level`: regions 5 and 6, the count and dimen registers above
/// 255).
pub(crate) fn refers(p: i32) -> bool {
    use crate::xregs::{EXT_BASE, ext_reg, is_word_kind};
    if p >= EXT_BASE {
        !is_word_kind(ext_reg(p).0)
    } else {
        p < INT_BASE || p > EQTB_SIZE
    }
}

/// A font's slot as a value (`MCell::Font`, with fonts as cells): what the
/// engine keeps of the font, but the glyphs used (`Glyphs`'s) and the
/// interword glue (a cache of its parameters); whether it is loaded is
/// the order of loading's (`FontOrder`).
#[derive(Clone, Debug)]
pub struct FontSlot {
    ident: u128,
    retagged: bool,
    metrics: Arc<partex_engine::font::Font>,
    tfm: Arc<[u8]>,
    name: Arc<[u8]>,
    area: Arc<[u8]>,
    hyphen_char: i32,
    skew_char: i32,
    codes: crate::fonts::Codes,
    expand: crate::fonts::Expand,
    pdf: crate::pdf::ship::PdfFont,
    /// `XeTeX`: the native font in the slot, and its direction state.
    native: Option<Arc<crate::native::NativeFont>>,
    native_dir: u32,
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// String `s`'s characters, not tracked (a snapshot's too).
    pub(crate) fn string_bytes(&self, s: i32) -> alloc::borrow::Cow<'_, [u8]> {
        match usize::try_from(s) {
            Ok(t) if t < self.str_ptr => self
                .str_pool
                .range_all(self.str_start.get_all(t), self.str_start.get_all(t + 1)),
            _ => alloc::borrow::Cow::Borrowed(&[]),
        }
    }

    /// Font slot `f`'s version as a machine's cell: whether it is loaded,
    /// what it is (its identity, which gives its metrics), its parameters
    /// and the rest of what can change about it, and what the PDF writer
    /// keeps of it (but the glyphs used).
    pub(crate) fn font_version(&self, f: i32) -> u128 {
        let mut h = StableHasher::new();
        let fonts = &self.fonts;
        let loaded = |i: usize| f == 0 || fonts.rank.get(i).is_some_and(|&r| r > 0);
        // (a slot not loaded is the same whatever it holds, and whether or
        // not the arrays reach it: nothing refers to it)
        match usize::try_from(f)
            .ok()
            .filter(|&i| i < fonts.metrics.len() && loaded(i))
        {
            None => (0u8, f).hash(&mut h),
            Some(i) => {
                (1u8, fonts.ident[i], fonts.retagged[i]).hash(&mut h);
                let m = &fonts.metrics[i];
                if fonts.retagged[i] || fonts.ident[i] == 0 {
                    // (its metrics are not what its identity says, or it
                    // has none: a format's font)
                    m.hash(&mut h);
                } else {
                    m.params.hash(&mut h);
                }
                (
                    fonts.hyphen_char[i],
                    fonts.skew_char[i],
                    &fonts.codes[i],
                    fonts.expand[i],
                    // (a native font is its identity's; its direction
                    // state is not)
                    fonts.native_dir[i],
                )
                    .hash(&mut h);
                self.string_bytes(fonts.name[i]).hash(&mut h);
                self.string_bytes(fonts.area[i]).hash(&mut h);
                // (what the PDF writer has not kept yet is its default)
                match self.pdf.ship.fonts.get(i) {
                    Some(p) => p.hash(&mut h),
                    None => crate::pdf::ship::PdfFont::default().hash(&mut h),
                }
            }
        }
        h.finish128()
    }

    /// Font slot `f` as a value.
    pub(crate) fn export_font(&self, f: i32) -> Option<FontSlot> {
        let i = usize::try_from(f)
            .ok()
            .filter(|&i| i < self.fonts.metrics.len())?;
        let fonts = &self.fonts;
        let mut pdf = self.pdf.ship.fonts.get(i).cloned().unwrap_or_default();
        pdf.chars = [0; 4];
        Some(FontSlot {
            ident: fonts.ident[i],
            retagged: fonts.retagged[i],
            metrics: fonts.metrics[i].clone(),
            tfm: fonts.tfm[i].clone(),
            name: Arc::from(&*self.string_bytes(fonts.name[i])),
            area: Arc::from(&*self.string_bytes(fonts.area[i])),
            hyphen_char: fonts.hyphen_char[i],
            skew_char: fonts.skew_char[i],
            codes: fonts.codes[i].clone(),
            expand: fonts.expand[i],
            pdf,
            native: fonts.native[i].clone(),
            native_dir: fonts.native_dir[i],
        })
    }

    /// Give font slot `f` the value `v` (its names as strings of this
    /// state's; the glyphs used stay).
    pub(crate) fn import_font(&mut self, f: i32, v: &FontSlot) {
        self.thaw();
        let i = crate::fonts::fx(f);
        let name = if *self.slot_font_name(i, false) == *v.name {
            self.fonts.name.get(i).copied().unwrap_or(0)
        } else {
            self.intern_str(&v.name)
        };
        let area = if *self.slot_font_name(i, true) == *v.area {
            self.fonts.area.get(i).copied().unwrap_or(0)
        } else {
            self.intern_str(&v.area)
        };
        let fonts = &mut self.fonts;
        fonts.ensure(i);
        fonts.metrics[i] = v.metrics.clone();
        fonts.tfm[i] = v.tfm.clone();
        fonts.name[i] = name;
        fonts.area[i] = area;
        fonts.glue[i] = None;
        fonts.hyphen_char[i] = v.hyphen_char;
        fonts.skew_char[i] = v.skew_char;
        fonts.codes[i].clone_from(&v.codes);
        fonts.expand[i] = v.expand;
        fonts.ident[i] = v.ident;
        fonts.retagged[i] = v.retagged;
        fonts.native[i].clone_from(&v.native);
        fonts.native_dir[i] = v.native_dir;
        let pf = &mut self.pdf.ship.fonts;
        if pf.len() <= i {
            pf.resize(i + 1, crate::pdf::ship::PdfFont::default());
        }
        let chars = pf[i].chars;
        pf[i] = v.pdf.clone();
        pf[i].chars = chars;
    }

    /// The characters of font slot `i`'s name (or area).
    fn slot_font_name(&self, i: usize, area: bool) -> alloc::borrow::Cow<'_, [u8]> {
        let v = if area {
            &self.fonts.area
        } else {
            &self.fonts.name
        };
        self.string_bytes(v.get(i).copied().unwrap_or(0))
    }

    /// The fonts named `name` as a version: each loaded one's slot, area
    /// and size, in the order they were loaded (§1260's search finds the
    /// first that fits).
    pub(crate) fn font_name_version(&self, name: &[u8]) -> u128 {
        let mut h = StableHasher::new();
        for f in self.fonts.loaded_fonts() {
            let i = crate::fonts::fx(f);
            if *self.slot_font_name(i, false) == *name {
                (
                    f,
                    &*self.slot_font_name(i, true),
                    self.fonts.metrics[i].size,
                )
                    .hash(&mut h);
            }
        }
        h.finish128()
    }

    /// Take `other`'s fonts, which are cells (a machine's `Rest` set over
    /// a state), their names as strings of this state's.
    pub(crate) fn take_fonts_from(&mut self, other: &Self) {
        let names = other.font_names();
        self.take_fonts_named(other, &names);
    }

    /// Each font slot's name and area, by their characters (what
    /// [`Tex::take_fonts_named`] takes).
    pub(crate) fn font_names(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        (0..self.fonts.metrics.len())
            .map(|i| {
                (
                    self.slot_font_name(i, false).into_owned(),
                    self.slot_font_name(i, true).into_owned(),
                )
            })
            .collect()
    }

    /// [`Tex::take_fonts_from`], with `other`'s font names given
    /// ([`Tex::font_names`]): read before `other`'s strings are moved.
    pub(crate) fn take_fonts_named(&mut self, other: &Self, other_names: &[(Vec<u8>, Vec<u8>)]) {
        let n = other.fonts.metrics.len();
        let (names, areas): (Vec<i32>, Vec<i32>) = (0..n)
            .map(|i| {
                let pick = |me: &mut Self, area: bool| {
                    let (nm, ar) = &other_names[i];
                    let b: &[u8] = if area { ar } else { nm };
                    if *me.slot_font_name(i, area) == *b {
                        let v = if area { &me.fonts.area } else { &me.fonts.name };
                        v.get(i).copied().unwrap_or(0)
                    } else if b.is_empty() {
                        0
                    } else {
                        me.intern_str(b)
                    }
                };
                (pick(self, false), pick(self, true))
            })
            .collect();
        self.fonts = other.fonts.clone();
        self.fonts.name = names;
        self.fonts.area = areas;
        self.pdf.ship.fonts.clone_from(&other.pdf.ship.fonts);
        self.font_ptr = other.font_ptr;
        self.fmem_ptr = other.fmem_ptr;
    }
}

/// Whether eqtb location `p` is a control sequence's, with a hash slot
/// (a name) beside it: a multiletter control sequence or a frozen one
/// (§222), or one of `hash_extra`.
fn is_cs_slot(p: i32) -> bool {
    (crate::web::HASH_BASE..crate::web::UNDEFINED_CONTROL_SEQUENCE).contains(&p)
        || (p > EQTB_SIZE && p < crate::xregs::EXT_BASE)
}

/// An eqtb word as a value: the word, its level, and what it names, by
/// content.
#[derive(Clone, Debug)]
pub struct CellValue {
    word: MemoryWord,
    level: Option<i32>,
    named: Named,
    /// With names as cells (`Tex::name_cells`), a control sequence's
    /// slot's name, by its characters (empty: no name).
    name: Option<Arc<[u8]>>,
}

impl Named {
    /// The object an entry of this value holds.
    fn obj(&self) -> Option<crate::objs::Obj> {
        match self {
            Named::Nothing => None,
            Named::List {
                toks,
                protected,
                interned: _,
            } => Some(crate::objs::Obj::Toks(Arc::new(
                crate::tok::TokenList::new(toks.to_vec(), *protected),
            ))),
            Named::Glue(g, lineage) => {
                (!g.shared_zero).then_some(crate::objs::Obj::Glue(crate::objs::Glue {
                    spec: *g,
                    lineage: *lineage,
                }))
            }
            Named::Shape(s) => s.clone().map(|s| crate::objs::Obj::Shape(Arc::new(s))),
            Named::Box(b) => b.clone().map(crate::objs::Obj::Box),
        }
    }
}

#[derive(Clone, Debug)]
enum Named {
    Nothing,
    List {
        toks: Arc<[i32]>,
        protected: bool,
        interned: bool,
    },
    Glue(partex_engine::node::GlueSpec, u64),
    Shape(Option<crate::objs::Shaped>),
    Box(Option<Arc<partex_engine::node::BoxNode>>),
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Eqtb location `p` as a value.
    pub(crate) fn export_cell(&self, p: i32) -> CellValue {
        let w = self.peek_eqtb(p);
        let e = w.rh();
        // (regions 5 and 6 hold integers and dimensions, not references,
        // as do the count and dimen registers above 255)
        let kind = if refers(p) { w.b0() } else { -1 };
        let _ = e;
        let o = if kind >= 0 { self.peek_obj(p) } else { None };
        let named = match (kind, o) {
            (CALL..=LONG_OUTER_CALL, Some(crate::objs::Obj::Toks(t))) => Named::List {
                toks: t.tokens().into(),
                protected: t.protected(),
                interned: false,
            },
            (GLUE_REF, Some(crate::objs::Obj::Glue(g))) => Named::Glue(g.spec, g.lineage),
            (GLUE_REF, _) => Named::Glue(partex_engine::node::GlueSpec::ZERO_GLUE, 0),
            (SHAPE_REF, Some(crate::objs::Obj::Shape(sh))) => Named::Shape(Some((**sh).clone())),
            (SHAPE_REF, _) => Named::Shape(None),
            (BOX_REF, Some(crate::objs::Obj::Box(b))) => Named::Box(Some(b.clone())),
            (BOX_REF, _) => Named::Box(None),
            _ => Named::Nothing,
        };
        CellValue {
            word: w,
            level: (!refers(p)).then(|| self.peek_xeq_level(p)),
            named,
            name: (self.name_cells && is_cs_slot(p)).then(|| Arc::from(&*self.slot_name(p))),
        }
    }

    /// The characters of hash slot `p`'s name (none if it has none), not
    /// tracked.
    pub(crate) fn slot_name(&self, p: i32) -> alloc::borrow::Cow<'_, [u8]> {
        let t = usize::try_from(p - crate::web::HASH_BASE)
            .ok()
            .filter(|&i| i < self.hash.len())
            .map_or(0, |i| self.hash[i].rh());
        match usize::try_from(t) {
            Ok(t) if t > 0 && t < self.str_ptr => self
                .str_pool
                .range_all(self.str_start.get_all(t), self.str_start.get_all(t + 1)),
            _ => alloc::borrow::Cow::Borrowed(&[]),
        }
    }

    /// Where the control sequence named `name` is (0 if nowhere), by
    /// tex.web's hash chains (§259), not tracked.
    pub(crate) fn name_location(&self, name: &[u8]) -> i32 {
        use crate::web::{HASH_BASE, HASH_PRIME};
        let Some((&first, rest)) = name.split_first() else {
            return 0;
        };
        let mut h = i32::from(first);
        for &c in rest {
            h = h + h + i32::from(c);
            while h >= HASH_PRIME {
                h -= HASH_PRIME;
            }
        }
        let mut p = h + HASH_BASE;
        loop {
            if *self.slot_name(p) == *name {
                return p;
            }
            let next = usize::try_from(p - HASH_BASE)
                .ok()
                .filter(|&i| i < self.hash.len())
                .map_or(0, |i| self.hash[i].lh());
            if next == 0 {
                return 0;
            }
            p = next;
        }
    }

    /// A new string with the characters `b` (a name imported by its
    /// characters: which number it has, and whether another string has
    /// the same characters, TeX never observes).
    fn intern_str(&mut self, b: &[u8]) -> i32 {
        let l = b.len();
        self.grow_pool(l);
        // (the current string, if one is being made, moves up, as §260
        // moves it)
        let d = self.cur_length();
        while self.pool_ptr > self.str_start[self.str_ptr] {
            self.pool_ptr -= 1;
            self.str_pool[self.pool_ptr + l] = self.str_pool[self.pool_ptr];
        }
        for &c in b {
            self.append_char(c);
        }
        self.str_ptr += 1;
        self.grow_starts(self.str_ptr + 1);
        self.str_start[self.str_ptr] = self.pool_ptr;
        self.pool_ptr += d;
        i32::try_from(self.str_ptr - 1).unwrap_or(0)
    }

    /// Give hash slot `p` the name `b` (none if empty), by its characters.
    fn set_slot_name(&mut self, p: i32, b: &[u8]) {
        if *self.slot_name(p) == *b {
            return;
        }
        let s = if b.is_empty() { 0 } else { self.intern_str(b) };
        let i = usize::try_from(p - crate::web::HASH_BASE).unwrap_or(0);
        let mut w = self.hash[i];
        w.set_rh(s);
        self.hash[i] = w;
    }

    /// Give hash slot `p` the link `next`.
    pub(crate) fn set_slot_link(&mut self, p: i32, next: i32) {
        let i = usize::try_from(p - crate::web::HASH_BASE).unwrap_or(0);
        let mut w = self.hash[i];
        w.set_lh(next);
        self.hash[i] = w;
    }

    /// Hash slot `p`'s link, not tracked.
    #[must_use]
    pub(crate) fn slot_link(&self, p: i32) -> i32 {
        usize::try_from(p - crate::web::HASH_BASE)
            .ok()
            .filter(|&i| i < self.hash.len())
            .map_or(0, |i| self.hash[i].lh())
    }

    /// Eqtb location `p`'s version as a machine's cell: its content
    /// ([`Tex::cell_content`]) and, with names as cells, a control
    /// sequence's name.
    #[must_use]
    pub(crate) fn mcell_content(&self, p: i32) -> u128 {
        let c = self.cell_content(Cell::Eqtb(p));
        if self.name_cells && is_cs_slot(p) {
            let mut h = StableHasher::new();
            h.write_u128(c);
            self.slot_name(p).hash(&mut h);
            h.finish128()
        } else {
            c
        }
    }

    /// Give eqtb location `p` the value `v` (its objects copied into this
    /// state's stores).
    pub(crate) fn import_cell(&mut self, p: i32, v: &CellValue) {
        self.thaw();
        let obj = v.named.obj();
        if let Some(name) = &v.name {
            self.set_slot_name(p, name);
        }
        if refers(p) {
            self.set_eqtb_entry(p, v.word, obj);
        } else {
            self.set_eqtb(p, v.word);
        }
        if let Some(l) = v.level {
            self.set_xeq_level(p, l);
        }
    }
}

/// A cell of [`TexMachine`]. `Rest` sorts first: a region's writes are
/// applied in cell order, and setting `Rest` replaces everything but the
/// cells after it. `File` sorts before `Line`: patching a state with
/// changed input sets what the host serves before the lines.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MCell {
    Rest,
    /// The glyphs used so far, by font (what the fonts embedded at the end
    /// subset). An accumulator: a region's value is the glyphs it used,
    /// and setting it adds them; its version is that of all the glyphs
    /// used. Read only where the fonts are written.
    Glyphs,
    /// Every control sequence's class to a skipped conditional text
    /// (`skipcache.rs`), derived from eqtb: read where a remembered skip
    /// is used, written where a class changes; setting it does nothing
    /// (the eqtb words set it).
    Classes,
    File(Arc<[u8]>),
    Line(Arc<[u8]>, u32),
    /// With positions as cells (`statehash::POSITION_CELLS`): the line
    /// numbers of the input, which `Rest` then leaves out ([`Positions`]).
    /// Every region reads it at its start and writes it at its end, as
    /// `Rest`; it sorts after `File`, so the positions are set in the
    /// files a rebuild serves.
    Positions,
    /// With the page as a cell (`statehash::PAGE_CELLS`): the page
    /// builder's state (`Tex::page`, §980–§982), which `Rest` then leaves
    /// out. Read and written where the engine accesses it
    /// (`Tex::page`, `Tex::page_mut`); it sorts after `Rest`, so setting
    /// `Rest` keeps it and setting it after replaces it.
    Page,
    /// With them as cells (`statehash::PDF_LAST_CELLS`): pdfTeX's
    /// `last_item` value `k` (`pdf::PdfLast`: `\pdflastlink` and its
    /// siblings), read where `\pdflast…` reads it and written where the
    /// object is made; `Rest` then leaves it out.
    PdfLast(u8),
    /// With them as cells (`statehash::PDF_WORD_CELLS`): the PDF writer's
    /// word `k` (`pdf::word`: an object list's head, the outlines' first,
    /// last and parent, the catalog's open action), read and written
    /// where the writer's routines touch it; `Rest` then leaves it out.
    PdfWord(u8),
    /// With them as a cell (`statehash::MARK_CELLS`): the current marks
    /// of every class (`cur_mark`, §382), read where TeX reads them
    /// (`\topmarks` and its siblings, `fire_up`, `\vsplit`) and written
    /// where they change; `Rest` then leaves them out.
    Marks,
    /// A sealed line's contents (`seal.rs`), by key (its high and low
    /// halves: a `u128` would align every cell, and so every guard, write
    /// and index entry, to 16 bytes).
    Sealed(u64, u64),
    /// What `\write` file `id` holds (the job may read it back: LaTeX's
    /// `.aux` at the end). An accumulator, as `Glyphs`: a region's value
    /// is what it appended; its version is that of all the contents.
    Written(u32),
    Eqtb(i32),
    /// With names as cells (`Tex::name_cells`): hash slot `p`'s link
    /// (`next`, §256), what a lookup that enters a name walks.
    Link(i32),
    /// With names as cells: where the control sequence of these
    /// characters is (or that there is none): what a lookup that does not
    /// find a name read (§259). Setting it does nothing: the slots, which
    /// are eqtb cells, hold the names.
    Name(Arc<[u8]>),
    /// With fonts as cells (`Tex::font_cells`): font slot `f`
    /// (`FontSlot`).
    Font(i32),
    /// The fonts loaded with this name (their slots, areas and sizes, in
    /// the order of loading): what §1260's search and pdfTeX's
    /// `tfm_lookup` read.
    FontName(Arc<[u8]>),
    /// The font expanded from a base font by a ratio (pdfTeX's
    /// `get_expand_font`).
    FontExpand(i32, i32),
    /// The order the fonts were loaded in, which gives their numbers
    /// (tex.web's, pdfTeX's `/F` names): an accumulator, as `Numbering`;
    /// read where a number is printed or the fonts are gone through in
    /// order.
    FontOrder,
    /// The PDF object table's entry `k` (`pdf/objtab.rs`: `ObjLog`; with
    /// `Tex::set_obj_cells`).
    Obj(i32),
    /// A lookup tree's entry: the tree (an object type) and the
    /// identifier (`Id::key`).
    Tree(u8, Arc<[u8]>),
    /// The destination names with their objects, in order of creation:
    /// an accumulator, as `Written`: a region's value is the names it
    /// added; read where they are written out (the end).
    Dests,
    /// What pdfTeX's object numbers depend on (`pdf/vnum.rs`, virtual
    /// numbers): an accumulator, as `Written`: a region's value is the
    /// events it added; read where the job observes a number.
    Numbering,
    /// The final number of virtual object `k` (`final_num`): a question
    /// about `Numbering` (`Machine::derived`), answered by `get`, never
    /// written; setting it does nothing.
    FinalNum(i32),
    /// The object pdfTeX numbers `n` (`of_final`): a question about
    /// `Numbering`, as `FinalNum`.
    OfFinal(i32),
    /// The numbering's counters, not its names (`vnum::Counters`): what
    /// numbers the objects a region makes take, a question about
    /// `Numbering`, as `FinalNum`.
    NumState,
    /// Relocatable numbers (`reloc.rs`): the region used a number of
    /// origin `.0` (count register `.0`; [`ANY_ORIGIN`]: any) as a
    /// number. Never written; `get` answers it the same always, so it
    /// never makes a region dirty: a rebuild that would reuse the region
    /// with its numbers relocated checks that the origin does not move.
    Origin(i32),
    /// Relocatable numbers: an answer the region depends on about number
    /// `.1` of origin `.0`: that `.1 .2 .3` (`<`, `=` or `>`) is what it
    /// was, or (`reloc::SHIFT`) that `.1` and `.1 + .3` move alike. As
    /// `Origin`, never written and always holding; a rebuild that would
    /// relocate the region checks it with `.1` relocated.
    IntCmp(i32, i32, u8, i32),
}

impl MCell {
    /// A name cell of the object table as a cell.
    fn of_name(n: &NameCell) -> Self {
        match n {
            NameCell::Tree(t, i) => Self::Tree(*t, i.key()),
        }
    }
}

/// What a region did to the object table's cells (`ObjLog`, drained
/// after each step): the entries and name cells it read before writing
/// them (its guards), and those it wrote.
#[derive(Clone, Default)]
struct ObjRegion {
    reads: alloc::collections::BTreeSet<i32>,
    writes: alloc::collections::BTreeSet<i32>,
    names_read: alloc::collections::BTreeSet<NameCell>,
    names_written: alloc::collections::BTreeSet<NameCell>,
    /// A step's log overflowed: the region read every entry.
    overflow: bool,
}

impl ObjRegion {
    fn take<H: Host, T: Tracker>(&mut self, tex: &mut Tex<H, T>) {
        let (objs, overflow, names) = tex.pdf.objs.take_log();
        self.overflow |= overflow;
        for k in objs {
            if k >= 0 {
                if !self.writes.contains(&k) {
                    self.reads.insert(k);
                }
            } else {
                self.writes.insert(-1 - k);
            }
        }
        for (w, n) in names {
            if w {
                self.names_written.insert(n);
            } else if !self.names_written.contains(&n) {
                self.names_read.insert(n);
            }
        }
    }
}

impl MCell {
    /// Sealed line `k`'s cell.
    #[must_use]
    pub fn sealed(k: u128) -> Self {
        #[allow(clippy::cast_possible_truncation)] // (the halves)
        Self::Sealed((k >> 64) as u64, k as u64)
    }
}

/// The key of a sealed line's cell.
fn seal_key(hi: u64, lo: u64) -> u128 {
    (u128::from(hi) << 64) | u128::from(lo)
}

/// A value of [`TexMachine`]: equal when the versions are.
pub struct MValue<H: CellHost> {
    version: u128,
    v: V<H>,
}

enum V<H: CellHost> {
    /// Only the version (what a read reports).
    Version,
    Word(Arc<CellValue>),
    /// Eqtb location `p` of a snapshot (exported when it is set).
    Lazy(Arc<Snapshot<H>>, i32),
    Rest(Arc<Snapshot<H>>),
    /// (boxed: a slice's fat pointer would make every value, as every
    /// write a trace keeps holds one, 16 bytes longer)
    File(Arc<Arc<[u8]>>),
    /// A line: the host serves it with its file.
    Line,
    Positions(Arc<Positions>),
    Page(Arc<PageValue>),
    /// The current marks.
    Marks(Arc<MarkMap>),
    /// Glyphs used, by font (to add).
    Glyphs(Arc<Vec<[u64; 4]>>),
    Sealed(Arc<crate::seal::Sealed>),
    /// Bytes appended to a `\write` file.
    Written(Arc<Vec<u8>>),
    /// An object table entry, a lookup tree's entry, a destination name.
    Obj(Arc<crate::pdf::objtab::Entry>),
    Tree(i32),
    /// A number: a hash slot's link, an expanded font.
    Int(i32),
    /// A font's slot.
    Font(Arc<FontSlot>),
    /// Fonts loaded (their slots).
    FontOrder(Arc<Vec<i32>>),
    /// Destination names added.
    Dests(Arc<Vec<(Arc<[u8]>, i32)>>),
    /// Numbering events added.
    Numbering(Arc<Vec<crate::pdf::vnum::NumEvent>>),
}

/// The machine's state as `Rest`'s value: here, or kept in a store and
/// loaded where it is first used (`machine_store.rs`).
struct Snapshot<H: CellHost> {
    held: Held<H>,
}

#[allow(clippy::large_enum_variant)] // (behind an `Arc`)
enum Held<H: CellHost> {
    Here(SnapBody<H>),
    /// In the store as blob `.0` (not changed since: saved again as a
    /// reference to it).
    Stored(u128, alloc::boxed::Box<dyn LazyBody<H>>),
}

/// What a snapshot holds.
pub struct SnapBody<H: CellHost> {
    /// The engine, without its node lists (`lists`).
    tex: Tex<H, CellTracker>,
    lists: Lists,
    started: bool,
    halted: bool,
    /// `MCell::Page`'s version in it (with the page as a cell; 0
    /// without).
    page_version: u128,
}

impl<H: CellHost> SnapBody<H> {
    fn new(tex: Tex<H, CellTracker>, lists: Lists, started: bool, halted: bool) -> Self {
        let page_version = if page_cells() {
            let page = lists.0.first().map_or(&[][..], Vec::as_slice);
            self::page_version(&tex.page, page.iter().map(|c| &c[..]))
        } else {
            0
        };
        Self {
            tex,
            lists,
            started,
            halted,
            page_version,
        }
    }

    /// `MCell::Page`'s value in it.
    fn page_value(&self) -> PageValue {
        PageValue {
            builder: self.tex.page.clone(),
            list: self.lists.0.first().cloned().unwrap_or_default(),
        }
    }
}

/// A snapshot kept in a store: loaded the first time it is asked for
/// (the host's crate has the threads' once-cells this needs).
pub trait LazyBody<H: CellHost>: Send + Sync {
    fn body(&self) -> &SnapBody<H>;
}

impl<H: CellHost> core::ops::Deref for Snapshot<H> {
    type Target = SnapBody<H>;
    fn deref(&self) -> &SnapBody<H> {
        match &self.held {
            Held::Here(b) => b,
            Held::Stored(_, l) => l.body(),
        }
    }
}

impl<H: CellHost> Snapshot<H> {
    fn here(body: SnapBody<H>) -> Self {
        Self {
            held: Held::Here(body),
        }
    }

    /// The store's blob this snapshot is, if it came from one.
    fn stored(&self) -> Option<u128> {
        match &self.held {
            Held::Here(_) => None,
            Held::Stored(h, _) => Some(*h),
        }
    }
}

/// Nodes per shared chunk of a snapshot's node lists.
const LIST_CHUNK: usize = 64;

/// A snapshot's node lists (the page's, the current list's and those of
/// the enclosing levels, in that order), in shared chunks: a list is
/// mostly appended to, and a long one (a box being built over pages)
/// would otherwise be copied whole into every snapshot. A chunk equal to
/// the one before in the same place is shared.
#[derive(Clone, Default)]
struct Lists(Vec<Vec<Arc<[Node]>>>);

/// `a == b`, boxes by identity first (a list's boxes are shared).
fn same_nodes(a: &[Node], b: &[Node]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| match (x, y) {
            (Node::Box(p), Node::Box(q)) => Arc::ptr_eq(p, q) || p == q,
            _ => x == y,
        })
}

impl Lists {
    /// Take `tex`'s node lists out (to clone it without them).
    fn take<H: Host, T: Tracker>(tex: &mut Tex<H, T>) -> Vec<Vec<Node>> {
        let mut v = alloc::vec![
            core::mem::take(&mut tex.page.list).into_vec(),
            core::mem::take(&mut tex.cur_list.list).into_vec(),
        ];
        v.extend(
            tex.nest
                .iter_mut()
                .map(|r| core::mem::take(&mut r.list).into_vec()),
        );
        v
    }

    /// Put lists [`Lists::take`] took back.
    fn put<H: Host, T: Tracker>(tex: &mut Tex<H, T>, lists: Vec<Vec<Node>>) {
        let mut it = lists.into_iter();
        tex.page.list = it.next().unwrap_or_default().into();
        tex.cur_list.list = it.next().unwrap_or_default().into();
        for r in &mut tex.nest {
            r.list = it.next().unwrap_or_default().into();
        }
    }

    /// `lists` in chunks, sharing those of `prev` that are equal.
    fn share(lists: &[Vec<Node>], prev: Option<&Self>) -> Self {
        Self(
            lists
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let old = prev.and_then(|p| p.0.get(i));
                    l.chunks(LIST_CHUNK)
                        .enumerate()
                        .map(|(c, ch)| match old.and_then(|o| o.get(c)) {
                            Some(o) if same_nodes(o, ch) => o.clone(),
                            _ => Arc::from(ch),
                        })
                        .collect()
                })
                .collect(),
        )
    }

    fn lists(&self) -> Vec<Vec<Node>> {
        self.0.iter().map(|l| l.concat()).collect()
    }
}

impl<H: CellHost> Snapshot<H> {
    /// A snapshot of `tex` (its lists shared with `prev`'s where equal).
    fn of(tex: &mut Tex<H, CellTracker>, started: bool, halted: bool, prev: Option<&Self>) -> Self {
        let lists = Lists::take(tex);
        let shared = Lists::share(&lists, prev.map(|p| &p.lists));
        // (the state hash's memo is scratch, which would keep each
        // snapshot's replaced chunks alive: the running engine's only)
        let memo = core::mem::take(&mut tex.hash_memo);
        // (the input and parameter stacks to their live prefixes only,
        // as the state hash reads them: TeX writes a level past
        // `input_ptr` before it reads it, §321, and the arguments past
        // `param_ptr`, §390; the dead parts, a stack's size each, would
        // be most of a fine region's snapshot. [`Snapshot::engine`] gives
        // them their lengths back)
        let stacks = (
            core::mem::take(&mut tex.input_stack),
            core::mem::take(&mut tex.param_stack),
        );
        let timing = CUT_TIMING.load(Relaxed);
        let t0 = if timing { tex.host.nanos() } else { 0 };
        let mut snap = tex.clone();
        let live = (
            tex.input_ptr.min(stacks.0.len()),
            usize::try_from(tex.param_ptr)
                .unwrap_or(0)
                .min(stacks.1.len()),
        );
        snap.input_stack = stacks.0[..live.0].to_vec();
        snap.param_stack = stacks.1[..live.1].to_vec();
        (tex.input_stack, tex.param_stack) = stacks;
        if timing {
            CUT_T[47].fetch_add(tex.host.nanos() - t0, Relaxed);
            if CUT_TIMING_DETAIL.load(Relaxed) {
                Self::time_clone_groups(tex);
            }
        }
        tex.hash_memo = memo;
        Lists::put(tex, lists);
        Self::here(SnapBody::new(snap, shared, started, halted))
    }

    /// `PARTEX_CUT_TIMING`: clone `tex`'s field groups one at a time
    /// ([`CLONE_GROUPS`]), timed, and drop the clones.
    fn time_clone_groups(tex: &Tex<H, CellTracker>) {
        let now = || tex.host.nanos();
        let mut at = [0u64; CLONE_GROUPS.len() + 1];
        at[0] = now();
        drop((
            tex.eqtb_obj.clone(),
            tex.xregs.clone(),
            tex.xeq_level.clone(),
        ));
        at[1] = now();
        drop((
            tex.nest.clone(),
            tex.cur_list.clone(),
            tex.page.clone(),
            tex.align.clone(),
            tex.adjust.clone(),
            tex.cur_box.clone(),
            tex.split_discards.clone(),
        ));
        at[2] = now();
        drop((tex.eqtb.clone(), tex.hash.clone(), tex.save_stack.clone()));
        at[3] = now();
        drop((
            tex.str_pool.clone(),
            tex.str_start.clone(),
            tex.buffer.clone(),
            tex.input_stack.clone(),
            tex.param_stack.clone(),
        ));
        at[4] = now();
        drop(tex.fonts.clone());
        at[5] = now();
        drop(tex.hyph.clone());
        at[6] = now();
        drop(tex.prims.clone());
        at[7] = now();
        drop((tex.fontmap.clone(), tex.fonts_mapped.clone()));
        at[8] = now();
        drop(tex.cur_mark.clone());
        at[9] = now();
        drop(tex.seals.clone());
        at[10] = now();
        drop(tex.cs_cache.clone());
        at[11] = now();
        drop(tex.str_index.clone());
        at[12] = now();
        drop(tex.skip.clone());
        at[13] = now();
        drop(tex.memo.clone());
        at[14] = now();
        drop(tex.pdf.objs.clone());
        at[15] = now();
        drop(tex.pdf.out.clone());
        at[16] = now();
        drop(tex.pdf.ship.clone());
        at[17] = now();
        drop(tex.pdf.fontw.clone());
        at[18] = now();
        drop(tex.pdf.clone());
        at[19] = now();
        drop((
            tex.log_file.clone(),
            tex.write_file.clone(),
            tex.term_buf.clone(),
            tex.dvi.clone(),
            tex.effects.clone(),
            tex.line_log.clone(),
            tex.name_log.clone(),
            tex.font_log.clone(),
            tex.seal_log.clone(),
            tex.objstms_written.clone(),
            tex.glyphs_used.clone(),
        ));
        at[20] = now();
        drop(tex.host.clone());
        at[21] = now();
        for i in 0..CLONE_GROUPS.len() {
            CUT_T[48 + i].fetch_add(at[i + 1] - at[i], Relaxed);
        }
    }

    /// The engine, whole (its stacks at their full lengths again, the
    /// dead parts as a new engine has them: [`Snapshot::of`]).
    fn engine(&self) -> Tex<H, CellTracker> {
        let mut t = self.tex.clone();
        Lists::put(&mut t, self.lists.lists());
        let (input, param) = (
            usize::try_from(t.params.stack_size).unwrap_or(0) + 1,
            usize::try_from(t.params.param_size).unwrap_or(0) + 1,
        );
        if t.input_stack.len() < input {
            t.input_stack
                .resize(input, crate::input::InStateRecord::default());
        }
        if t.param_stack.len() < param {
            t.param_stack.resize(param, None);
        }
        t
    }
}

impl<H: CellHost> MValue<H> {
    /// The engine a `Rest` value holds (for debugging).
    #[must_use]
    pub fn rest_tex(&self) -> Option<Tex<H, CellTracker>> {
        match &self.v {
            V::Rest(s) => Some(s.engine()),
            _ => None,
        }
    }

    fn version(version: u128) -> Self {
        Self {
            version,
            v: V::Version,
        }
    }
}

impl<H: CellHost> Clone for MValue<H> {
    fn clone(&self) -> Self {
        let v = match &self.v {
            V::Version => V::Version,
            V::Word(w) => V::Word(w.clone()),
            V::Lazy(s, p) => V::Lazy(s.clone(), *p),
            V::Rest(s) => V::Rest(s.clone()),
            V::File(f) => V::File(f.clone()),
            V::Line => V::Line,
            V::Positions(p) => V::Positions(p.clone()),
            V::Page(p) => V::Page(p.clone()),
            V::Marks(m) => V::Marks(m.clone()),
            V::Glyphs(g) => V::Glyphs(g.clone()),
            V::Sealed(x) => V::Sealed(x.clone()),
            V::Written(x) => V::Written(x.clone()),
            V::Obj(e) => V::Obj(e.clone()),
            V::Tree(k) => V::Tree(*k),
            V::Int(k) => V::Int(*k),
            V::Font(x) => V::Font(x.clone()),
            V::FontOrder(x) => V::FontOrder(x.clone()),
            V::Dests(x) => V::Dests(x.clone()),
            V::Numbering(x) => V::Numbering(x.clone()),
        };
        Self {
            version: self.version,
            v,
        }
    }
}

impl<H: CellHost> PartialEq for MValue<H> {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

impl<H: CellHost> Eq for MValue<H> {}

impl<H: CellHost> Hash for MValue<H> {
    fn hash<S: Hasher>(&self, h: &mut S) {
        self.version.hash(h);
    }
}

impl<H: CellHost> core::fmt::Debug for MValue<H> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let kind = match &self.v {
            V::Version => "version",
            V::Word(_) | V::Lazy(..) => "word",
            V::Rest(_) => "rest",
            V::File(_) => "file",
            V::Line => "line",
            V::Positions(_) => "positions",
            V::Page(_) => "page",
            V::Marks(_) => "marks",
            V::Glyphs(_) => "glyphs",
            V::Sealed(_) => "sealed",
            V::Written(_) => "written",
            V::Obj(_) => "obj",
            V::Tree(_) => "tree",
            V::Int(_) => "int",
            V::Font(_) => "font",
            V::FontOrder(_) => "font order",
            V::Dests(_) => "dests",
            V::Numbering(_) => "numbering",
        };
        write!(f, "{kind} {:032x}", self.version)
    }
}

/// The line numbers of the input (`MCell::Positions`'s value): the
/// line counter of each input level, and how many lines of each served
/// file open for reading were read. A number means a place in the file of
/// its level or slot, which `files` names, so a rebuild can map it through
/// an edit that moved lines (DESIGN.md §7.15).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Positions {
    /// `line` of the levels 0 to `in_open` (§304: a level's while it is
    /// not the innermost is on `line_stack`, one level up, §328).
    pub lines: Vec<i32>,
    /// By slot (an `\input` level; 256 plus a `\read` stream), a served
    /// file's name and the lines read of it (§31).
    pub files: Vec<(u16, Arc<[u8]>, u32)>,
}

partex_engine::persist_struct!(Positions { lines, files });

impl Positions {
    /// The version: the numbers and the names.
    fn version(&self) -> u128 {
        StableHasher::of(self)
    }
}

/// Where each line of a file's contents begins (as `input_ln` reads
/// them), and where the last one ends.
#[must_use]
pub fn line_starts(data: &[u8]) -> Arc<[usize]> {
    let lines = crate::track::file_lines(data);
    lines
        .iter()
        .map(|l| l.0)
        .chain(core::iter::once(data.len()))
        .collect()
}

/// Line `k` of `data` (with its line end), if there is one.
fn line_in<'a>(data: &'a [u8], starts: &[usize], k: u32) -> Option<&'a [u8]> {
    let k = usize::try_from(k).ok()?;
    let (&from, &to) = (starts.get(k)?, starts.get(k + 1)?);
    Some(&data[from..to])
}

/// Where a machine's region may begin or end ([`Machine::Boundary`]):
/// the line number of the file being read kept apart from the rest of
/// the position, with the file it counts in, so that a rebuild can map
/// it through that file's diff after lines moved (DESIGN §7.16.5, the
/// first piece of sync across moved lines). Equal positions are equal
/// in all three.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    /// Everything but the line and the file: the input stack's shape, the
    /// place in the line or the token list, the pages shipped at a stop
    /// before `\shipout`.
    pub key: u128,
    /// The name of the file `line` counts in (its hash; 0: none).
    pub file: u128,
    /// `line` (§304): the line of the innermost file being read.
    pub line: i32,
}

/// A file's name as a boundary names it (`Pos::file`).
#[must_use]
pub fn file_key(name: &[u8]) -> u128 {
    StableHasher::of(&(1u8, name))
}

/// How a changed file's lines moved: the lines `prefix..old_end` of the
/// old contents became `prefix..new_end` of the new ones, and the lines
/// after moved by `new_end - old_end` (one hunk: the common prefix and
/// suffix; `machinehost`'s diff).
#[derive(Clone, Debug)]
pub struct LineMap {
    pub file: Arc<[u8]>,
    pub prefix: u32,
    pub old_end: u32,
    pub new_end: u32,
}

impl LineMap {
    /// Old line `k`'s new number (0-based), if its content is still
    /// there and what follows it did not change. The line right after
    /// the hunk is gone too: the region that read it went there from
    /// the hunk (an insertion puts new lines in between).
    fn line(&self, k: u32) -> Option<u32> {
        if k < self.prefix {
            Some(k)
        } else if k > self.old_end {
            Some(k - self.old_end + self.new_end)
        } else {
            None
        }
    }

    /// A count of lines read (a line counter, `line`), renamed: a place
    /// inside the hunk gets a count no new place has.
    fn count(&self, c: i64) -> i64 {
        if c <= i64::from(self.prefix) {
            c
        } else if c > i64::from(self.old_end) {
            c - i64::from(self.old_end) + i64::from(self.new_end)
        } else {
            i64::from(i32::MIN) + c
        }
    }
}

/// Rename `b` after the files of `maps` changed (DESIGN §7.16.5, behind
/// `PARTEX_MACHINE_RENAME=1`): the boundaries in them, their `Line`
/// cells (a line gone makes its readers run again) and the `Positions`
/// values (the line counters and the lines read of the files).
pub fn rename_lines<H: CellHost>(b: &mut partex_incr::Build<TexMachine<H>>, maps: &[LineMap]) {
    if maps.is_empty() {
        return;
    }
    let keys: Vec<(u128, &LineMap)> = maps.iter().map(|m| (file_key(&m.file), m)).collect();
    let by_name = |name: &[u8]| maps.iter().find(|m| &*m.file == name);
    let cell = |c: &MCell| match c {
        MCell::Line(f, k) => match by_name(f) {
            Some(m) => m.line(*k).map(|k| MCell::Line(f.clone(), k)),
            None => Some(c.clone()),
        },
        c => Some(c.clone()),
    };
    let boundary = |p: &Pos| match keys.iter().find(|(k, _)| *k == p.file) {
        Some((_, m)) => Pos {
            line: i32::try_from(m.count(i64::from(p.line))).unwrap_or(i32::MIN),
            ..*p
        },
        None => *p,
    };
    let value = |c: &MCell, v: &MValue<H>| match (c, &v.v) {
        (MCell::Positions, V::Positions(p)) => {
            let mut q = (**p).clone();
            let mut changed = false;
            for (slot, name, read) in &mut q.files {
                let Some(m) = by_name(name) else { continue };
                changed = true;
                *read = u32::try_from(m.count(i64::from(*read))).unwrap_or(u32::MAX);
                // (an input level's line counter: level `slot`'s is
                // `lines[slot + 1]`, §304, §328)
                if let Some(l) = q.lines.get_mut(usize::from(*slot) + 1) {
                    *l = i32::try_from(m.count(i64::from(*l))).unwrap_or(i32::MIN);
                }
            }
            changed.then(|| MValue {
                version: q.version(),
                v: V::Positions(Arc::new(q)),
            })
        }
        _ => None,
    };
    b.rename(&partex_incr::build::Renaming {
        cell: &cell,
        boundary: &boundary,
        value: &value,
        guard: &|_, v| v,
        state: &|_| {},
    });
}

/// The engine as a [`Machine`].
pub struct TexMachine<H: CellHost> {
    tex: Tex<H, CellTracker>,
    command_line: Arc<[u8]>,
    started: bool,
    halted: bool,
    /// The state was set or moved: a region begins at the next step.
    fresh: bool,
    /// The last step ended at a boundary (a region may begin next).
    at_boundary: bool,
    /// The file being read at the last boundary (its depth and name): a
    /// boundary in another is where one began or ended (a candidate of
    /// level 2).
    file_at: (usize, i32),
    /// Steps taken (not state).
    steps: u64,
    /// A snapshot of the state (the entry of the region running, or the
    /// exit of the one just cut), the step it was taken at (`u64::MAX`:
    /// the state changed since), and `Rest`'s version then.
    entry: Option<Arc<Snapshot<H>>>,
    snap_at: u64,
    rest_version: u128,
    /// The step at which the runtime last cut a region.
    cut_at: u64,
    /// The lines this region read, and the files it opened to read by
    /// lines.
    region_lines: alloc::collections::BTreeSet<(Arc<[u8]>, u32)>,
    opened: alloc::collections::BTreeSet<Arc<[u8]>>,
    /// The glyphs the region just cut used (its `Glyphs` value).
    glyphs: Arc<Vec<[u64; 4]>>,
    /// Whether a clean span is replayed by taking the old run's state at
    /// its end ([`Machine::replay_exit`]; off, its writes are applied).
    replay_exit: bool,
    /// What this region appended to `\write` files and which it read
    /// back; what the region just cut appended (its `Written` values).
    region_written: alloc::collections::BTreeMap<u32, Vec<u8>>,
    region_written_reads: alloc::collections::BTreeSet<u32>,
    /// Remembered skips under tracking (`Classes`).
    skips: bool,
    /// A restore takes the replaced engine's token lists where unchanged.
    thaw_from: bool,
    written_delta: Arc<alloc::collections::BTreeMap<u32, Arc<Vec<u8>>>>,
    /// What this region did to the object table's cells.
    objs: ObjRegion,
    /// The numbering events the region just cut added (its `Numbering`
    /// value).
    num_delta: Arc<Vec<crate::pdf::vnum::NumEvent>>,
    /// The destination names the region just cut added (its `Dests`
    /// value).
    dests_delta: Arc<Vec<(Arc<[u8]>, i32)>>,
    /// The fonts the region just cut loaded (its `FontOrder` value).
    font_delta: Arc<Vec<i32>>,
    /// Diagnostics, not state: the candidates seen and cut at.
    census: Census,
}

/// The candidates a machine's steps returned and the regions cut at them,
/// by level (a diagnostic: `PARTEX_MACHINE_PARTS` and the cold build's
/// summary print it).
#[derive(Clone, Debug, Default)]
pub struct Census {
    /// Candidates returned, by level (0–3).
    pub seen: [u64; 4],
    /// The clean points among them: outer ones, paragraph starts.
    pub clean: [u64; 2],
    /// Regions cut at one, by its level.
    pub cut: [u64; 4],
    /// The level of each cut, by the region's exit (its `at`).
    pub levels: alloc::collections::BTreeMap<Pos, u8>,
    /// Who observed the numbering in each region that did, by the
    /// region's exit: `final_num` calls, `of_final` calls, whole
    /// readings, distinct arguments.
    pub observers: alloc::collections::BTreeMap<Pos, (u32, u32, u32, usize)>,
    /// The last step's candidate level, if it returned one.
    last: Option<u8>,
}

impl<H: CellHost> Clone for TexMachine<H> {
    fn clone(&self) -> Self {
        Self {
            tex: self.tex.clone(),
            command_line: self.command_line.clone(),
            started: self.started,
            halted: self.halted,
            fresh: self.fresh,
            at_boundary: self.at_boundary,
            file_at: self.file_at,
            steps: self.steps,
            entry: self.entry.clone(),
            snap_at: self.snap_at,
            rest_version: self.rest_version,
            cut_at: self.cut_at,
            region_lines: self.region_lines.clone(),
            opened: self.opened.clone(),
            glyphs: self.glyphs.clone(),
            replay_exit: self.replay_exit,
            region_written: self.region_written.clone(),
            region_written_reads: self.region_written_reads.clone(),
            skips: self.skips,
            thaw_from: self.thaw_from,
            written_delta: self.written_delta.clone(),
            objs: self.objs.clone(),
            num_delta: self.num_delta.clone(),
            dests_delta: self.dests_delta.clone(),
            font_delta: self.font_delta.clone(),
            census: self.census.clone(),
        }
    }
}

impl<H: CellHost> TexMachine<H> {
    /// A job not yet started: `tex` runs `command_line`. Effects are
    /// turned on.
    pub fn new(mut tex: Tex<H, CellTracker>, command_line: &[u8]) -> Self {
        tex.set_effects(true);
        tex.set_tags(RELOCATE.load(Relaxed));
        tex.set_seal_lines(true);
        tex.set_stop_before_ship(true);
        tex.log_lines = true;
        Self {
            tex,
            command_line: command_line.into(),
            started: false,
            halted: false,
            fresh: true,
            at_boundary: false,
            file_at: (0, 0),
            steps: 0,
            entry: None,
            snap_at: u64::MAX,
            rest_version: 0,
            cut_at: u64::MAX,
            region_lines: alloc::collections::BTreeSet::new(),
            opened: alloc::collections::BTreeSet::new(),
            glyphs: Arc::default(),
            replay_exit: true,
            region_written: alloc::collections::BTreeMap::new(),
            region_written_reads: alloc::collections::BTreeSet::new(),
            skips: true,
            thaw_from: true,
            written_delta: Arc::default(),
            objs: ObjRegion::default(),
            num_delta: Arc::default(),
            dests_delta: Arc::default(),
            font_delta: Arc::default(),
            census: Census::default(),
        }
    }

    /// The candidates seen and the regions cut at them, by level.
    #[must_use]
    pub fn census(&self) -> &Census {
        &self.census
    }

    /// Whether a clean span is replayed by taking the old run's state at
    /// its end (on by default; off, its writes are applied).
    pub fn set_replay_exit(&mut self, on: bool) {
        self.replay_exit = on;
    }

    /// Remember skipped conditional text under tracking (on by default;
    /// the `Classes` cell).
    pub fn set_skips(&mut self, on: bool) {
        self.skips = on;
    }

    /// Whether restoring a snapshot takes the replaced engine's token
    /// lists where the snapshot has them unchanged (on by default; off,
    /// every list is copied from the snapshot).
    pub fn set_thaw_from(&mut self, on: bool) {
        self.thaw_from = on;
    }

    /// Seal lines (on by default, `seal.rs`): an edit that changes a
    /// line's words but not its dimensions leaves the page's `Rest` alone.
    pub fn set_seal_lines(&mut self, on: bool) {
        self.tex.set_seal_lines(on);
        self.tex.set_stop_before_ship(on);
    }

    /// The line of the job's main file being read, if the main file is
    /// the only input level (a top-level position: a cold split's).
    #[must_use]
    pub fn top_line(&self) -> Option<i32> {
        let t = &self.tex;
        (self.started
            && !self.halted
            && t.in_open == 1
            && t.cur_input.name > 17
            && t.cur_input.state != TOKEN_LIST)
            .then_some(t.line)
    }

    /// Take `from`'s input position (the input stack, the open files and
    /// the buffer), keeping everything else: a guessed entry at a
    /// position of another state.
    pub fn adopt_position(&mut self, from: &Self) {
        let (t, f) = (&mut self.tex, &from.tex);
        t.line = f.line;
        t.buffer.clone_from(&f.buffer);
        (t.first, t.last, t.max_buf_stack) = (f.first, f.last, f.max_buf_stack);
        t.input_stack.clone_from(&f.input_stack);
        t.input_values = crate::input::InputValues::default();
        t.input_ptr = f.input_ptr;
        t.cur_input = f.cur_input.clone();
        t.in_open = f.in_open;
        t.input_file.clone_from(&f.input_file);
        t.line_stack.clone_from(&f.line_stack);
        t.grp_stack.clone_from(&f.grp_stack);
        t.if_stack.clone_from(&f.if_stack);
        t.eof_seen.clone_from(&f.eof_seen);
        t.source_filename_stack.clone_from(&f.source_filename_stack);
        t.full_source_filename_stack
            .clone_from(&f.full_source_filename_stack);
        self.fresh = true;
        self.snap_at = u64::MAX;
    }

    /// The engine.
    pub fn tex(&self) -> &Tex<H, CellTracker> {
        &self.tex
    }

    /// The engine, to change (a new starting state: its host serving
    /// edited files).
    pub fn tex_mut(&mut self) -> &mut Tex<H, CellTracker> {
        self.snap_at = u64::MAX;
        &mut self.tex
    }

    /// Where the glyphs used differ from `other`'s, and which of
    /// `traces` add the differing glyphs (for debugging).
    pub fn glyph_differences<'a>(
        &self,
        other: &Self,
        traces: impl Iterator<Item = &'a partex_incr::Trace<Self>>,
    ) -> alloc::string::String
    where
        H: 'a,
    {
        let (a, b) = (&self.tex.pdf.ship.fonts, &other.tex.pdf.ship.fonts);
        let mut diff = Vec::new();
        for f in 0..a.len().max(b.len()) {
            let x = a.get(f).map_or([0; 4], |p| p.chars);
            let y = b.get(f).map_or([0; 4], |p| p.chars);
            for w in 0..4 {
                if x[w] != y[w] {
                    diff.push((f, w, x[w] ^ y[w], x[w] & !y[w] != 0));
                }
            }
        }
        let mut out = alloc::format!("{diff:?}; added by:");
        for (i, t) in traces.enumerate() {
            for (c, v, _) in &t.writes {
                if let (MCell::Glyphs, V::Glyphs(g)) = (c, &v.v) {
                    for (f, w, bits, _) in &diff {
                        if g.get(*f).is_some_and(|q| q[*w] & bits != 0) {
                            let _ = core::fmt::Write::write_fmt(
                                &mut out,
                                format_args!(" {i}(font {f})"),
                            );
                        }
                    }
                }
            }
        }
        out
    }

    /// `history` once the job is over.
    pub fn history(&self) -> Option<i32> {
        self.halted.then_some(self.tex.history)
    }

    /// Whether the host serves the file named `name` as an input file
    /// (its lines are cells).
    fn serves(host: &H, name: &[u8]) -> bool {
        !name.is_empty() && host.file(name).is_some() && !host.reads_back(name)
    }

    /// The version of `Glyphs` in `tex`: every glyph used so far.
    fn glyphs_version(tex: &Tex<H, CellTracker>) -> u128 {
        // (by font, the fonts with glyphs used: how far the PDF writer's
        // per-font table reaches is not state, and with fonts as cells a
        // state set from cells reaches as far as the fonts it was given)
        let mut h = StableHasher::new();
        for (f, pf) in tex.pdf.ship.fonts.iter().enumerate() {
            if pf.chars != [0; 4] {
                (f, pf.chars).hash(&mut h);
            }
        }
        h.finish128()
    }

    /// `Rest`'s version from the state hash `state`.
    fn rest_version_of(&self, state: u128) -> u128 {
        let mut h = StableHasher::new();
        h.write_u128(state);
        h.write_u128(self.tex.host.digest());
        (self.started, self.halted).hash(&mut h);
        if position_cells() {
            // (a `Rest` without line numbers is another kind of value)
            h.write_u8(1);
        }
        h.finish128()
    }

    /// `Rest`'s version, computed from scratch.
    fn rest_version_slow(&self) -> u128 {
        let host = &self.tex.host;
        let served = |n: &[u8]| Self::serves(host, n);
        self.rest_version_of(self.tex.rest_hash_served(&served))
    }

    /// A snapshot of the state now, with `Rest`'s version (memoized: what
    /// it costs is what changed since the last one).
    fn take_snapshot(&mut self) {
        self.take_snapshot_with(None);
    }

    /// [`Self::take_snapshot`], `Rest`'s version given if it is known
    /// (the state is a snapshot's, restored and patched only in cells).
    fn take_snapshot_with(&mut self, known: Option<u128>) {
        let timing = CUT_TIMING.load(Relaxed);
        if timing {
            self.time_commits();
        }
        let t0 = if timing { self.tex.host.nanos() } else { 0 };
        if let Some(v) = known.filter(|_| !VERIFY_REST.load(Relaxed)) {
            self.tex.commit();
            self.rest_version = v;
        } else {
            let host = self.tex.host.clone();
            let served = |n: &[u8]| Self::serves(&host, n);
            let h = self.tex.rest_hash_memo(&served);
            self.rest_version = self.rest_version_of(h);
            if let Some(v) = known {
                assert_eq!(
                    self.rest_version, v,
                    "a replayed state's Rest is not the snapshot's (KNOWN_REST)"
                );
            }
        }
        let t1 = if timing { self.tex.host.nanos() } else { 0 };
        let snap = Snapshot::of(
            &mut self.tex,
            self.started,
            self.halted,
            self.entry.as_deref(),
        );
        if timing {
            CUT_T[5].fetch_add(t1 - t0, Relaxed);
            CUT_T[6].fetch_add(self.tex.host.nanos() - t1, Relaxed);
        }
        self.entry = Some(Arc::new(snap));
        self.snap_at = self.steps;
    }

    /// `PARTEX_CUT_TIMING`: the commits of a cut, one piece at a time,
    /// timed (see [`CutTiming`]).
    fn time_commits(&mut self) {
        let clock = |m: &Self| m.tex.host.nanos();
        let a = clock(self);
        let b = clock(self);
        let c = clock(self);
        let host = self.tex.host.clone();
        let now = || host.nanos();
        let js = [
            self.tex.eqtb.commit_timed(&now),
            self.tex.hash.commit_timed(&now),
            self.tex.save_stack.commit_timed(&now),
        ];
        for (j, r) in js.iter().enumerate() {
            let b = 25 + 7 * j;
            for k in 0..3 {
                CUT_T[b + k].fetch_add(r[k], Relaxed);
            }
            CUT_T[b + 3].store(r[3], Relaxed);
            CUT_T[b + 4].fetch_add(r[4], Relaxed);
            CUT_T[b + 5].fetch_max(r[4], Relaxed);
            CUT_T[b + 6].fetch_add(r[5], Relaxed);
        }
        CUT_T[46].store(u64::try_from(self.tex.save_ptr).unwrap_or(0), Relaxed);
        let d = clock(self);
        let mut at = [d; 6];
        let x = &mut self.tex;
        let str_floor = x.str_start.get_all(x.str_ptr);
        x.str_pool.commit_live(x.pool_ptr, str_floor);
        at[1] = clock(self);
        let x = &mut self.tex;
        x.str_start.commit_live(x.str_ptr + 1, x.str_ptr + 1);
        at[2] = clock(self);
        let x = &mut self.tex;
        x.buffer.commit_live(x.first.max(x.last), 0);
        at[3] = clock(self);
        at[4] = clock(self);
        at[5] = clock(self);
        let e = at[5];
        for i in 0..5 {
            CUT_T[10 + i].fetch_add(at[i + 1] - at[i], Relaxed);
        }
        let x = &self.tex;
        let bytes = [
            x.str_pool.len_all() as u64,
            (x.str_start.len_all() * core::mem::size_of::<usize>()) as u64,
            x.buffer.len_all() as u64,
            (x.input_stack.len() * core::mem::size_of::<crate::input::InStateRecord>()) as u64,
            (x.param_stack.len() * 8) as u64,
        ];
        let live = [
            x.pool_ptr as u64,
            ((x.str_ptr + 1) * core::mem::size_of::<usize>()) as u64,
            x.first.max(x.last) as u64,
            (x.input_ptr * core::mem::size_of::<crate::input::InStateRecord>()) as u64,
            (usize::try_from(x.param_ptr).unwrap_or(0) * 4) as u64,
        ];
        for i in 0..5 {
            CUT_T[15 + i].store(bytes[i], Relaxed);
            CUT_T[20 + i].store(live[i], Relaxed);
        }
        for (i, v) in [(0, 1), (1, b - a), (2, c - b), (3, d - c), (4, e - d)] {
            CUT_T[i].fetch_add(v, Relaxed);
        }
        CUT_T[7].store(0, Relaxed);
        CUT_T[8].store(0, Relaxed);
        CUT_T[9].store(self.tex.tok_pool.len() as u64, Relaxed);
    }

    /// Whether the snapshot is of the state now.
    fn snapshot_now(&self) -> Option<&Arc<Snapshot<H>>> {
        self.entry.as_ref().filter(|_| self.snap_at == self.steps)
    }

    /// The version of line `k` of the file `path` as the host serves it
    /// (`None`: no such line, or no such file).
    fn line_version(&self, path: &[u8], k: u32) -> Option<u128> {
        let (data, starts) = self.tex.host.lines(path)?;
        line_in(&data, &starts, k).map(StableHasher::of)
    }

    /// Hand what the step did to `r`: the files it opened and its effects
    /// (reads and writes of cells wait for the region's end).
    fn report<R: Recorder<Self>>(&mut self, r: &mut R) {
        for (name, k) in core::mem::take(&mut self.tex.line_log) {
            if k == u32::MAX {
                self.opened.insert(name);
            } else if !self.tex.host.reads_back(&name) {
                // (a line of a file this job wrote, read back as it is
                // now, is the `Written` cell's; one read before the job
                // opened the file again is the file's as it was served,
                // though the name is the same: decided as it is read)
                self.region_lines.insert((name, k));
            }
        }
        for path in self.tex.host.take_reads() {
            // (a file read by lines is read through its lines)
            if self.opened.contains(&path[..]) {
                continue;
            }
            let v = self.get(&MCell::File(path.as_slice().into()));
            r.read(&MCell::File(path.into()), v.as_ref());
        }
        for e in self.tex.take_effects() {
            r.effect(e);
        }
        if self.tex.pdf.objs.log.on {
            self.objs.take(&mut self.tex);
        }
        let (appended, read) = self.tex.host.take_written();
        // (a file read back holds what it held at the entry and what the
        // region appended: read at the entry even if appended to first)
        self.region_written_reads.extend(read);
        for (id, bytes) in appended {
            self.region_written.entry(id).or_default().extend(bytes);
        }
    }

    /// Object table entry `k`'s value (`None`: no such entry), with the
    /// entry itself if `full`.
    /// The version of a font cell kept by name or by base font and ratio
    /// (`FontName`, `FontExpand`) in `tex`.
    fn font_cell_version(tex: &Tex<H, CellTracker>, c: &MCell) -> u128 {
        match c {
            MCell::FontName(n) => tex.font_name_version(n),
            MCell::FontExpand(b, e) => {
                let k = tex.fonts.expanded.get(&(*b, *e)).copied().unwrap_or(0);
                StableHasher::of(&k.to_le_bytes())
            }
            _ => 0,
        }
    }

    fn obj_value(o: &crate::pdf::objtab::ObjTab, k: i32, full: bool) -> Option<MValue<H>> {
        let e = o.entry(k)?;
        Some(MValue {
            version: o.entry_version(e),
            v: if full {
                V::Obj(Arc::new(e.clone()))
            } else {
                V::Version
            },
        })
    }

    /// A name cell's value (`None`: no such entry), as [`Self::obj_value`].
    fn name_value(o: &crate::pdf::objtab::ObjTab, n: &NameCell, full: bool) -> Option<MValue<H>> {
        match n {
            NameCell::Tree(t, i) => {
                let k = o.tree_entry(usize::from(*t), i)?;
                Some(MValue {
                    version: StableHasher::of(&k.to_le_bytes()),
                    v: if full { V::Tree(k) } else { V::Version },
                })
            }
        }
    }

    /// The version of `\write` file `id` in `tex`: its contents.
    fn written_version(tex: &Tex<H, CellTracker>, id: u32) -> Option<u128> {
        tex.host.written(id).map(|w| StableHasher::of(&w[..]))
    }

    /// Take this snapshot's state, keeping this state's eqtb words (if
    /// `keep_eqtb`), glyphs used and input files (an open file whose
    /// contents changed is read on from the same line).
    /// The hash of a file level's name (a file by its name, not its
    /// string's number: a string made earlier in one run than in another
    /// renumbers every later one); `None` for a token list or a name the
    /// pool does not hold.
    fn file_name_hash(&self, i: &crate::input::InStateRecord) -> Option<u128> {
        let t = &self.tex;
        if i.state == TOKEN_LIST || i.name <= 17 {
            return None;
        }
        // (the file as the host serves it, `input_file[index]`, §300)
        let f = usize::try_from(i.index)
            .ok()
            .and_then(|k| t.input_file.get(k))?
            .as_ref()?;
        Some(file_key(&f.name))
    }

    /// The name hash of the innermost file on the input stack (0: none).
    fn file_below(&self) -> u128 {
        let t = &self.tex;
        let below: Vec<&crate::input::InStateRecord> =
            t.input_stack[..t.input_ptr].iter().collect();
        core::iter::once(&t.cur_input)
            .chain(below.into_iter().rev())
            .find_map(|i| self.file_name_hash(i))
            .unwrap_or(0)
    }

    /// The position as one hash, as `at` hashed it before the line was
    /// kept apart: what seeds the names of the objects a step makes.
    fn position_hash(&self) -> u128 {
        let t = &self.tex;
        let mut h = StableHasher::new();
        if t.stopped_before_ship() {
            (b"ship", self.started, t.in_open, t.line, t.shipped).hash(&mut h);
            return h.finish128();
        }
        (self.started, self.halted, t.in_open, t.input_ptr, t.line).hash(&mut h);
        let i = &t.cur_input;
        (i.state, i.index, i.start, i.loc, i.limit).hash(&mut h);
        match usize::try_from(i.name) {
            Ok(n) if i.state != TOKEN_LIST && n > 17 && n < t.str_ptr => {
                let name = t
                    .str_pool
                    .range_all(t.str_start.get_all(n), t.str_start.get_all(n + 1));
                (1u8, &*name).hash(&mut h);
            }
            _ => (0u8, i.name).hash(&mut h),
        }
        h.finish128()
    }

    /// `MCell::Page`'s version now.
    fn tex_page_version(&self) -> u128 {
        let list = self.tex.page.list.to_vec();
        page_version(&self.tex.page, core::iter::once(&list[..]))
    }

    /// The line numbers of the input now (`MCell::Positions`): what
    /// `Rest` leaves out with positions as cells, `statehash.rs`.
    fn positions(&self) -> Positions {
        let t = &self.tex;
        let live = (t.in_open + 1).min(t.line_stack.len());
        let mut lines = t.line_stack[..live].to_vec();
        lines.push(t.line);
        let host = &t.host;
        let files = (t.input_file.iter().enumerate())
            .chain(t.read_file.iter().enumerate().map(|(i, f)| (256 + i, f)))
            .filter_map(|(i, f)| {
                let f = f.as_ref().filter(|f| Self::serves(host, &f.name))?;
                Some((u16::try_from(i).ok()?, f.name.clone(), f.lines))
            })
            .collect();
        Positions { lines, files }
    }

    /// Set the line numbers of the input to `p`'s: the counters, and
    /// each served file read on from its line (as the host serves it).
    fn set_positions(&mut self, p: &Positions) {
        let t = &mut self.tex;
        let live = (t.in_open + 1).min(t.line_stack.len());
        // (the levels are `Rest`'s, set first: the same)
        debug_assert_eq!(p.lines.len(), live + 1, "positions of other input levels");
        if let Some((&line, stack)) = p.lines.split_last()
            && stack.len() == live
        {
            t.line_stack[..live].copy_from_slice(stack);
            t.line = line;
        }
        for (slot, name, lines) in &p.files {
            let i = usize::from(*slot);
            let f = if i < 256 {
                t.input_file.get_mut(i)
            } else {
                t.read_file.get_mut(i - 256)
            };
            let Some(Some(f)) = f else { continue };
            if f.name != *name || f.lines == *lines {
                continue;
            }
            let starts = match t.host.lines(&f.name) {
                Some((d, s)) if Arc::ptr_eq(&d, &f.data) => s,
                _ => line_starts(&f.data),
            };
            let k = usize::try_from(*lines).unwrap_or(usize::MAX);
            f.pos = starts.get(k).copied().unwrap_or(f.data.len());
            f.lines = *lines;
            f.line_open = false;
        }
    }

    /// The tracker as fresh: its tables reset by what it logged
    /// ([`CellTracker::reset`]), or, with `PARTEX_MACHINE_KEEP_TRACKER=0`,
    /// made again at the next step.
    fn reset_tracker(&mut self) {
        if KEEP_TRACKER.load(Relaxed) {
            self.tex.tracker.reset();
        } else {
            self.tex.tracker = CellTracker::default();
        }
    }

    fn restore_rest(&mut self, s: &Snapshot<H>, keep_eqtb: bool) -> bool {
        let timing = CUT_TIMING.load(Relaxed);
        let clock = || if timing { s.tex.host.nanos() } else { 0 };
        let t0 = clock();
        let mut new = s.engine();
        let t1 = clock();
        let (mut differ, mut links): (Vec<i32>, Vec<i32>) = (Vec::new(), Vec::new());
        if keep_eqtb {
            for c in new.eqtb_differences(&self.tex) {
                match c {
                    Cell::Eqtb(p) => differ.push(p),
                    // (with names as cells, a slot's link is this state's)
                    Cell::HashNext(p) => links.push(p),
                    _ => {}
                }
            }
        }
        let values: Vec<(i32, CellValue)> = differ
            .into_iter()
            .map(|p| (p, self.tex.export_cell(p)))
            .collect();
        // (what is read of this engine's vectors, read before the thaw
        // rebases them into the new one: `Tex::thaw_from`)
        let links: Vec<(i32, i32)> = links
            .into_iter()
            .map(|p| (p, self.tex.slot_link(p)))
            .collect();
        let font_names = (keep_eqtb && new.font_cells).then(|| self.tex.font_names());
        let t2 = clock();
        // (this engine is replaced: its token lists are taken where the
        // snapshot has them unchanged. `PARTEX_MACHINE_THAW=0`: none)
        let thawed = if timing {
            new.thaw_timed(&mut self.tex, self.thaw_from, &clock)
        } else {
            if self.thaw_from {
                new.thaw_from(&mut self.tex);
            } else {
                new.thaw();
            }
            [0; 4]
        };
        let t3 = clock();
        if self.thaw_from
            && crate::journal::rebase()
            && (cfg!(debug_assertions) || VERIFY_REST.load(Relaxed))
        {
            // (the sanitizer's check: a rebase is a thaw)
            let mut full = s.engine();
            full.thaw();
            assert!(
                new.same_thawed(&full),
                "sanitizer: a restore rebased onto the snapshot is not its thaw"
            );
        }
        new.hash_memo = core::mem::take(&mut self.tex.hash_memo);
        for (p, v) in values {
            new.import_cell(p, &v);
        }
        for (p, link) in links {
            new.set_slot_link(p, link);
        }
        new.host = self.tex.host.clone_state_from(&s.tex.host);
        if let Some(names) = &font_names {
            // (and the fonts)
            new.take_fonts_named(&self.tex, names);
        }
        if new.font_cells {
            // (the order of loading is `FontOrder`'s: this state's)
            new.fonts.take_order_from(&self.tex.fonts);
            new.font_ptr = i32::try_from(new.fonts.order.len().saturating_sub(1)).unwrap_or(0);
        }
        if keep_eqtb && page_cells() {
            // (the page builder's state is a cell of its own: this
            // state's)
            new.page = core::mem::take(&mut self.tex.page);
        }
        if keep_eqtb && mark_cells() {
            // (and the current marks)
            new.cur_mark = core::mem::take(&mut self.tex.cur_mark);
        }
        if keep_eqtb && pdf_last_cells() {
            // (and the `\pdflast…` values)
            for k in crate::pdf::PdfLast::ALL {
                *new.pdf.last_mut(k) = self.tex.pdf.last(k);
            }
        }
        if keep_eqtb {
            // (and the writer's words that are cells)
            for k in 0..crate::pdf::word::COUNT {
                if pdf_word_is_cell(&self.tex, k) {
                    *new.pdf.word_mut(k) = self.tex.pdf.word(k);
                }
            }
        }
        if keep_eqtb {
            // (the sealed lines are cells of their own: this state's)
            new.seals = self.tex.seals.clone();
            if new.pdf.objs.log.on {
                // (and the object table's entries and names)
                new.pdf.objs.take_cells_from(&self.tex.pdf.objs);
            }
        }
        // (the numbering is `Numbering`'s: this state's, with its cache;
        // the destination names `Dests`'s)
        new.pdf.objs.alog = self.tex.pdf.objs.alog.clone();
        if new.pdf.objs.log.on {
            new.pdf.objs.take_dests_from(&self.tex.pdf.objs);
        }
        new.pdf.objs.take_ncache(&self.tex.pdf.objs);
        // (the glyphs used are `Glyphs`'s: this state's)
        for (f, pf) in new.pdf.ship.fonts.iter_mut().enumerate() {
            pf.chars = self.tex.pdf.ship.fonts.get(f).map_or([0; 4], |p| p.chars);
        }
        let host = new.host.clone();
        let mut rebased = false;
        for f in new
            .input_file
            .iter_mut()
            .chain(new.read_file.iter_mut())
            .flatten()
        {
            // (a file this job wrote, read back, is the snapshot's as it
            // was read: its contents are the `Written` cell's, not those of
            // the file of that name as served, as `report` counts its
            // lines; LaTeX reads its `.aux` from disk at `\begin{document}`
            // and back at `\end{document}`)
            if !Self::serves(&host, &f.name) {
                continue;
            }
            let Some(cur) = host.file(&f.name) else {
                continue;
            };
            if Arc::ptr_eq(&cur, &f.data) {
                continue;
            }
            let starts = line_starts(&cur);
            let k = usize::try_from(f.lines).unwrap_or(usize::MAX);
            f.pos = starts.get(k).copied().unwrap_or(cur.len());
            f.line_open = false;
            f.data = cur;
            rebased = true;
        }
        if KEEP_TRACKER.load(Relaxed) {
            // (the tables made for this engine serve the new one, and the
            // object log's room)
            new.tracker.adopt(&mut self.tex.tracker);
            new.pdf.objs.adopt_log(&mut self.tex.pdf.objs);
        }
        let t4 = clock();
        drop(core::mem::replace(&mut self.tex, new));
        if timing {
            let t5 = clock();
            let pieces = [
                t1 - t0,
                t2 - t1,
                thawed[0],
                thawed[1],
                thawed[2],
                thawed[3],
                t4 - t3,
                t5 - t4,
                t5 - t0,
            ];
            CUT_T[74].fetch_add(1, Relaxed);
            for (i, v) in pieces.into_iter().enumerate() {
                CUT_T[75 + i].fetch_add(v, Relaxed);
            }
        }
        self.started = s.started;
        self.halted = s.halted;
        rebased
    }

    /// Whether setting cell `c` leaves what `Rest` hashes as it is (with
    /// names, fonts and object numbers as cells).
    fn leaves_rest(&self, c: &MCell) -> bool {
        match c {
            // (a font whose tags changed keeps its metrics in `Rest`)
            MCell::Font(f) => !usize::try_from(*f)
                .ok()
                .and_then(|i| self.tex.fonts.retagged.get(i))
                .copied()
                .unwrap_or(false),
            // (the whole state)
            MCell::Rest => false,
            // (the line numbers `Rest` leaves out only with positions as
            // cells)
            MCell::Positions => position_cells(),
            MCell::Page => page_cells(),
            MCell::Marks => mark_cells(),
            MCell::PdfLast(_) => pdf_last_cells(),
            MCell::PdfWord(k) => pdf_word_is_cell(&self.tex, *k),
            _ => true,
        }
    }
}

/// Cold-start positions: none are known without running (a cold start
/// is one region); a previous run's boundaries are kept (a warm start's
/// regions run up to them, and a region that misses one runs on).
impl<H: CellHost> partex_incr::Split for TexMachine<H> {
    fn split(&self, _parts: usize) -> Vec<Pos> {
        alloc::vec![self.at()]
    }

    fn still_at(&self, _b: &Pos) -> bool {
        true
    }
}

impl<H: CellHost> TexMachine<H> {
    /// The parts of [`Machine::digest`], by name (the sanitizer's
    /// diagnostics say which differ).
    ///
    /// (the machine's cells: `Rest`, and each eqtb word by its own
    /// content, so that which words share a token list does not count)
    #[must_use]
    pub fn digest_parts(&self) -> Vec<(&'static str, u128)> {
        let mut parts = Vec::new();
        let mut h = StableHasher::new();
        let part = |parts: &mut Vec<(&'static str, u128)>, name, h: &mut StableHasher| {
            parts.push((name, core::mem::replace(h, StableHasher::new()).finish128()));
        };
        h.write_u128(self.rest_version_slow());
        if position_cells() {
            h.write_u128(self.positions().version());
        }
        if page_cells() {
            h.write_u128(self.tex_page_version());
        }
        if mark_cells() {
            h.write_u128(marks_version(&self.tex.cur_mark));
        }
        if pdf_last_cells() {
            for k in crate::pdf::PdfLast::ALL {
                h.write_u128(pdf_last_version(self.tex.pdf.last(k)));
            }
        }
        for k in 0..crate::pdf::word::COUNT {
            if pdf_word_is_cell(&self.tex, k) {
                h.write_u128(pdf_last_version(self.tex.pdf.word(k)));
            }
        }
        part(&mut parts, "rest", &mut h);
        for (p, c) in self.tex.eqtb_cell_hashes() {
            (p, c).hash(&mut h);
        }
        part(&mut parts, "eqtb", &mut h);
        for p in self.tex.xreg_locs() {
            (p, self.tex.cell_content(Cell::Eqtb(p))).hash(&mut h);
        }
        self.tex.xeq_level.hash(&mut h);
        part(&mut parts, "xregs, levels", &mut h);
        if self.tex.name_cells {
            // (the hash slots' names by their characters, and links)
            let t = &self.tex;
            for (i, w) in t.hash.slices().flatten().enumerate() {
                if w.bits() != 0 {
                    let p = crate::web::HASH_BASE + i32::try_from(i).unwrap_or(0);
                    (p, t.slot_name(p), w.lh()).hash(&mut h);
                }
            }
        }
        part(&mut parts, "names", &mut h);
        if self.tex.font_cells {
            for f in 0..i32::try_from(self.tex.fonts.metrics.len()).unwrap_or(0) {
                (f, self.tex.font_version(f)).hash(&mut h);
            }
            part(&mut parts, "font slots", &mut h);
            h.write_u128(self.tex.fonts.order_hash);
        }
        part(&mut parts, "font order", &mut h);
        h.write_u128(Self::glyphs_version(&self.tex));
        part(&mut parts, "glyphs", &mut h);
        self.tex.seals.hash_into(&mut h);
        part(&mut parts, "sealed lines", &mut h);
        for id in self.tex.host.written_ids() {
            (id, Self::written_version(&self.tex, id)).hash(&mut h);
        }
        part(&mut parts, "written", &mut h);
        let o = &self.tex.pdf.objs;
        if o.log.on {
            for k in o.keys() {
                (k, Self::obj_value(o, k, false).map(|v| v.version)).hash(&mut h);
            }
            o.hash_names(&mut h);
        }
        part(&mut parts, "objects", &mut h);
        h.write_u128(o.alog.hash);
        h.write_u128(o.dest_hash);
        part(&mut parts, "numbering, dests", &mut h);
        parts
    }

    fn render_effect(e: &Effect, out: &mut Vec<u8>) {
        // The runtime's own link, for its sanitizer: the bytes in program
        // order, which do not depend on how a run cut its output into
        // effects (files are linked by `effects::link`). A byte count's
        // guess and an object's mark are the link's to resolve.
        match e {
            Effect::Write { bytes, .. }
            | Effect::Term(bytes)
            | Effect::ObjStmBytes { bytes, .. } => out.extend_from_slice(bytes),
            Effect::PdfXref { xref, .. } => {
                out.extend_from_slice(alloc::format!("{xref:?}").as_bytes());
            }
            Effect::ObjStm { num, .. } => {
                out.extend_from_slice(alloc::format!("objstm {num}").as_bytes());
            }
            Effect::Shipping(c) => {
                out.extend_from_slice(alloc::format!("page {c}").as_bytes());
            }
            Effect::Diagnostic(d) => {
                out.extend_from_slice(alloc::format!("{d:?}").as_bytes());
            }
            Effect::ObjRef { num, .. } | Effect::ObjStmRef { num, .. } => {
                out.extend_from_slice(alloc::format!("ref {num}").as_bytes());
            }
            Effect::Num(e, _) => {
                out.extend_from_slice(alloc::format!("{e:?}").as_bytes());
            }
            Effect::FontLoad(f) => out.extend_from_slice(alloc::format!("font {f}").as_bytes()),
            Effect::FontRef { slot, .. } | Effect::ObjStmFontRef { slot, .. } => {
                out.extend_from_slice(alloc::format!("font ref {slot}").as_bytes());
            }
            Effect::StreamLength { .. } => out.extend_from_slice(b"length"),
            Effect::Deflate { level, parts, .. } => {
                out.extend_from_slice(alloc::format!("stream {level}").as_bytes());
                for (b, r) in parts {
                    out.extend_from_slice(b);
                    if let Some(r) = r {
                        out.extend_from_slice(alloc::format!("{r:?}").as_bytes());
                    }
                }
            }
            Effect::Close(_)
            | Effect::PdfObject { .. }
            | Effect::Length { .. }
            | Effect::ObjStmStart { .. }
            | Effect::Open { .. }
            | Effect::Origins(_)
            | Effect::Synctex(_)
            | Effect::Display(_)
            | Effect::Flow { .. } => {}
        }
    }
}

impl<H: CellHost> Machine for TexMachine<H> {
    type Cell = MCell;
    type Value = MValue<H>;
    type Effect = Effect;
    type Boundary = Pos;

    fn step<R: Recorder<Self>>(&mut self, r: &mut R) -> Step {
        self.census.last = None;
        if self.halted {
            return Step::Halt;
        }
        // A region begins here if the state was just set or moved, or if
        // the runtime just cut one: it reads `Rest` (the snapshot then is
        // its entry). After a boundary where it did not cut, the region
        // goes on and has read `Rest` already.
        let cut = self.at_boundary && self.cut_at == self.steps;
        if self.fresh || cut {
            if self.snapshot_now().is_none() {
                self.take_snapshot();
            }
            self.fresh = false;
            r.read(&MCell::Rest, Some(&MValue::version(self.rest_version)));
            if position_cells() {
                let v = self.positions().version();
                r.read(&MCell::Positions, Some(&MValue::version(v)));
            }
        }
        self.at_boundary = false;
        let n = self.tex.eqtb.len();
        let links = if self.tex.name_cells {
            self.tex.hash.len()
        } else {
            0
        };
        let fonts = if self.tex.font_cells {
            usize::try_from(self.tex.params.font_max).unwrap_or(0) + 1
        } else {
            0
        };
        self.tex.tracker.reserve(n, links, fonts);
        self.tex.pdf.objs.reserve_log();
        if self.tex.pdf.objs.virt {
            // (the objects this step makes are named by where it is)
            #[allow(clippy::cast_possible_truncation)]
            let seed = self.position_hash() as u64;
            self.tex.pdf.objs.vseed = seed;
            self.tex.pdf.objs.vcount = 0;
        }
        let st = if self.started {
            // (up to the next command read straight from a file: the
            // commands in between are one step, whose cost is theirs)
            let before = self.tex.commands();
            self.tex.set_stop_at_candidate(true);
            let st = self.tex.resume();
            r.cost((self.tex.commands() - before).saturating_sub(1));
            st
        } else {
            // Initialization loads the format into eqtb wholesale, past
            // the accessors: its writes are the words that changed.
            self.started = true;
            let before = self.tex.clone();
            self.tex.thaw();
            self.tex.set_stop_at(1);
            let cl = self.command_line.clone();
            let st = self.tex.start(&cl);
            if self.skips {
                self.tex.set_skip_tracked(true);
            }
            for c in self.tex.eqtb_differences(&before) {
                match c {
                    Cell::Eqtb(p) => r.write(&MCell::Eqtb(p), &MValue::version(0)),
                    Cell::HashNext(p) => r.write(&MCell::Link(p), &MValue::version(0)),
                    _ => {}
                }
            }
            if self.tex.font_cells {
                // (and the format's fonts)
                for f in 0..i32::try_from(self.tex.fonts.metrics.len()).unwrap_or(0) {
                    r.write(&MCell::Font(f), &MValue::version(0));
                }
                r.write(&MCell::FontOrder, &MValue::version(0));
            }
            st
        };
        self.steps += 1;
        self.report(r);
        match st {
            run::Step::Finished(_) => {
                self.halted = true;
                Step::Halt
            }
            run::Step::Checkpoint => {
                if self.tex.cur_input.state == TOKEN_LIST && !self.tex.stopped_before_ship() {
                    return Step::Continue;
                }
                // a boundary: a new region may begin after it, most at a
                // clean point (level 3) or where a file began or ended
                // since the last one (level 2)
                self.at_boundary = true;
                let file = (self.tex.in_open, self.tex.cur_input.name);
                let edge = core::mem::replace(&mut self.file_at, file) != file;
                let clean = if clean_cuts() {
                    self.tex.clean_point()
                } else {
                    None
                };
                if let Some(k) = clean {
                    self.census.clean[k as usize] += 1;
                }
                let level = if clean.is_some() {
                    3
                } else if edge {
                    2
                } else {
                    1
                };
                self.census.seen[usize::from(level)] += 1;
                self.census.last = Some(level);
                if CANON.load(Relaxed) == 2 {
                    canonicalize_dead(&mut self.tex, true);
                }
                Step::Candidate(level)
            }
        }
    }

    fn prepare_cut<R: Recorder<Self>>(&mut self, r: &mut R) {
        if let Some(l) = self.census.last {
            self.census.cut[usize::from(l)] += 1;
            let at = self.at();
            self.census.levels.insert(at, l);
        }
        let t = &self.tex.tracker;
        assert!(
            !t.overflow.get(),
            "the machine's tracker overflowed: its tables are sized by eqtb"
        );
        // The guards: first reads of eqtb words, by their values at the
        // region's entry (a first read comes before any write of the
        // region), and the lines read.
        let entry = self.entry.clone().expect("a region has an entry");
        for p in t.read_locs().into_iter().chain(t.soft_pending()) {
            match p {
                Tracked::Link(q) => {
                    let v = link_version(entry.tex.slot_link(q));
                    r.read(&MCell::Link(q), Some(&MValue::version(v)));
                }
                Tracked::Eqtb(q) => {
                    let v = entry.tex.mcell_content(q);
                    r.read(&MCell::Eqtb(q), Some(&MValue::version(v)));
                }
                Tracked::Font(f) => {
                    let v = entry.tex.font_version(f);
                    r.read(&MCell::Font(f), Some(&MValue::version(v)));
                }
            }
        }
        // The fonts by name and the expanded fonts searched (at the
        // entry) unless the region made them first, and the order of
        // loading observed (at the entry: what the region added comes
        // after).
        let mut fonts_written: Vec<MCell> = Vec::new();
        {
            let mut read = alloc::collections::BTreeSet::new();
            let mut written = alloc::collections::BTreeSet::new();
            for t in core::mem::take(&mut self.tex.font_log) {
                let (c, w) = match t {
                    crate::fonts::FontTouch::Name(n, w) => (MCell::FontName(n), w),
                    crate::fonts::FontTouch::Expand(b, e, w) => (MCell::FontExpand(b, e), w),
                };
                if !written.contains(&c) && read.insert(c.clone()) {
                    let v = Self::font_cell_version(&entry.tex, &c);
                    r.read(&c, Some(&MValue::version(v)));
                }
                if w {
                    written.insert(c);
                }
            }
            fonts_written.extend(written);
        }
        if self.tex.fonts.order_read.get() {
            self.tex.fonts.order_read.set(false);
            if self.tex.font_cells {
                let v = entry.tex.fonts.order_hash;
                r.read(&MCell::FontOrder, Some(&MValue::version(v)));
            }
        }
        let loaded = if self.tex.font_cells {
            let from = entry.tex.fonts.order.len();
            self.tex.fonts.order.slots(from)
        } else {
            Vec::new()
        };
        let any_loaded = !loaded.is_empty();
        self.font_delta = Arc::new(loaded);
        // The names looked for and not found, or entered: read (missing
        // at the entry) unless the region entered them first.
        let mut names_written: alloc::collections::BTreeSet<Arc<[u8]>> =
            alloc::collections::BTreeSet::new();
        {
            let mut read = alloc::collections::BTreeSet::new();
            for (n, entered) in core::mem::take(&mut self.tex.name_log) {
                if !names_written.contains(&n) && read.insert(n.clone()) {
                    let v = name_version(entry.tex.name_location(&n));
                    r.read(&MCell::Name(n.clone()), Some(&MValue::version(v)));
                }
                if entered {
                    names_written.insert(n);
                }
            }
        }
        let lines = core::mem::take(&mut self.region_lines);
        for (path, k) in lines {
            if path.is_empty() || self.tex.host.file(&path).is_none() {
                // (a file not served: a terminal line, or one this job
                // wrote and read back)
                continue;
            }
            let v = self.line_version(&path, k).map(|v| MValue {
                version: v,
                v: V::Line,
            });
            r.read(&MCell::Line(path, k), v.as_ref());
        }
        // The classes to a skip: read by a remembered skip (at the entry),
        // written where one changed.
        if core::mem::take(&mut self.tex.classes_read) {
            r.read(
                &MCell::Classes,
                Some(&MValue::version(entry.tex.class_hash)),
            );
        }
        let classes = core::mem::take(&mut self.tex.classes_written);
        // The numbering, observed (at the entry: what the region added
        // comes after), and what the region added to it.
        let added = if self.tex.pdf.objs.virt {
            self.tex.pdf.objs.alog.since(entry.tex.pdf.objs.alog.len())
        } else {
            Vec::new()
        };
        let whole = core::mem::take(&mut self.tex.pdf.objs.log.forced);
        let answers = core::mem::take(&mut self.tex.pdf.objs.log.answers);
        if whole || !answers.is_empty() {
            // (the whole numbering: the guard a rebuild finds the region
            // by, and the one that decides for a region that read it
            // whole)
            let v = entry.tex.pdf.objs.alog.hash;
            r.read(&MCell::Numbering, Some(&MValue::version(v)));
            let o = core::mem::take(&mut self.tex.pdf.objs.log.observers);
            let at = self.at();
            self.census.observers.insert(at, (o.0, o.1, o.2, o.3.len()));
        }
        if !whole && !answers.is_empty() {
            // Only questions (`Machine::derived`'s contract: no derived
            // guard with a whole reading): each answer as it was at the
            // entry. An object this region made numbers by the counters
            // at the entry, not by any name (`NumState`).
            let own: alloc::collections::BTreeSet<i32> = added
                .iter()
                .filter_map(|e| match e {
                    crate::pdf::vnum::NumEvent::Create(v) => Some(*v),
                    _ => None,
                })
                .collect();
            let entry_sys = entry.tex.pdf.objs.alog.counters.sys;
            let mut state = false;
            let mut guards: alloc::collections::BTreeMap<MCell, u128> =
                alloc::collections::BTreeMap::new();
            for a in answers {
                match a {
                    crate::pdf::objtab::NumAnswer::Final(k, n) => {
                        if own.contains(&k) {
                            state = true;
                        } else {
                            guards.insert(MCell::FinalNum(k), num_answer_version(n));
                        }
                    }
                    crate::pdf::objtab::NumAnswer::Of(n, v) => {
                        if v.is_some_and(|v| own.contains(&v)) || (v.is_none() && n > entry_sys) {
                            state = true;
                        } else {
                            guards.insert(MCell::OfFinal(n), num_answer_version(v));
                        }
                    }
                }
            }
            if state {
                guards.insert(
                    MCell::NumState,
                    num_answer_version(entry.tex.pdf.objs.alog.counters),
                );
            }
            // (this branch is the only place derived guards on the
            // numbering are made, and it runs only without a whole
            // reading: `Machine::derived`'s contract holds by it)
            for (c, v) in guards {
                r.read(&c, Some(&MValue::version(v)));
            }
        }
        let numbered = !added.is_empty();
        self.num_delta = Arc::new(added);
        // The destination names, read (at the entry) and added.
        if core::mem::take(&mut self.tex.pdf.objs.log.dests_read) {
            let v = entry.tex.pdf.objs.dest_hash;
            r.read(&MCell::Dests, Some(&MValue::version(v)));
        }
        let dests = if self.tex.pdf.objs.log.on {
            let from = entry.tex.pdf.objs.dest_names.len();
            self.tex
                .pdf
                .objs
                .dest_names
                .iter()
                .skip(from)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let dested = !dests.is_empty();
        self.dests_delta = Arc::new(dests);
        // The object table's cells read before written (at the entry).
        let objs = core::mem::take(&mut self.objs);
        {
            let o = &entry.tex.pdf.objs;
            let all = if objs.overflow {
                o.keys()
                    .into_iter()
                    .filter(|k| !objs.writes.contains(k))
                    .collect()
            } else {
                Vec::new()
            };
            for &k in objs.reads.iter().chain(&all) {
                let v = Self::obj_value(o, k, false);
                r.read(&MCell::Obj(k), v.as_ref());
            }
            for n in &objs.names_read {
                let v = Self::name_value(o, n, false);
                r.read(&MCell::of_name(n), v.as_ref());
            }
        }
        // The `\write` files read back (their contents at the entry) and
        // appended to.
        for id in core::mem::take(&mut self.region_written_reads) {
            let v = Self::written_version(&entry.tex, id).map(MValue::version);
            r.read(&MCell::Written(id), v.as_ref());
        }
        let appended = core::mem::take(&mut self.region_written);
        self.written_delta = Arc::new(
            appended
                .iter()
                .map(|(id, b)| (*id, Arc::new(b.clone())))
                .collect(),
        );
        // The sealed lines: read (first reads of lines not sealed in this
        // region) and sealed.
        let mut sealed = alloc::collections::BTreeSet::new();
        let mut read = alloc::collections::BTreeSet::new();
        for (k, v) in core::mem::take(&mut self.tex.seal_log) {
            match v {
                None => {
                    sealed.insert(k);
                }
                Some(v) => {
                    if !sealed.contains(&k) && read.insert(k) {
                        r.read(&MCell::sealed(k), Some(&MValue::version(v)));
                    }
                }
            }
        }
        // The glyphs: read where the fonts were written (their value at
        // the entry: what the region added comes after), added to where
        // pages were shipped.
        if core::mem::take(&mut self.tex.glyphs_read) {
            let v = Self::glyphs_version(&entry.tex);
            r.read(&MCell::Glyphs, Some(&MValue::version(v)));
        }
        let used = core::mem::take(&mut self.tex.glyphs_used);
        let any = used.iter().any(|g| *g != [0; 4]);
        self.glyphs = Arc::new(used);
        // Relocatable numbers: a tag left pending is observed here, and
        // the raw token is forgotten (the next `get_next` would); then the
        // origins observed and the answers, as guards that always hold.
        if self.tex.tags_on {
            self.tex.flush_tag();
            self.tex.cur_raw = 0;
        }
        let (observed, answers, overflow) = self.tex.tracker.reloc.take();
        if overflow {
            r.read(
                &MCell::Origin(ANY_ORIGIN),
                Some(&MValue::version(ORIGIN_VERSION)),
            );
        }
        for o in observed {
            r.read(&MCell::Origin(o), Some(&MValue::version(ORIGIN_VERSION)));
        }
        for (o, x, rel, y) in answers {
            let v = int_cmp_version(x, rel, y);
            r.read(&MCell::IntCmp(o, x, rel, y), Some(&MValue::version(v)));
        }
        if CANON.load(Relaxed) == 1 {
            // (at a cut: both the exit snapshot and the run going on hold
            // the dead state's initial values)
            canonicalize_dead(&mut self.tex, false);
            // (and the save stack without its dead and no-op entries)
            self.tex.canonicalize_save_stack();
        }
        let writes = self.tex.tracker.written();
        // The page builder's state: read (at the entry) where it was read
        // first, written where it was changed.
        let (page_read, page_written) = if page_cells() {
            self.tex.tracker.page_touched()
        } else {
            (false, false)
        };
        if page_read {
            r.read(&MCell::Page, Some(&MValue::version(entry.page_version)));
        }
        // The current marks: read (at the entry) where read first,
        // written where changed.
        let (marks_read, marks_written) = if mark_cells() {
            self.tex.tracker.marks_touched()
        } else {
            (false, false)
        };
        if marks_read {
            let v = marks_version(&entry.tex.cur_mark);
            r.read(&MCell::Marks, Some(&MValue::version(v)));
        }
        // The `\pdflast…` values: read (at the entry) where read first,
        // written where made.
        let (last_read, last_written) = if pdf_last_cells() {
            self.tex.tracker.pdf_last_touched()
        } else {
            (0, 0)
        };
        for k in 0..u8::try_from(crate::pdf::PdfLast::ALL.len()).unwrap_or(0) {
            if last_read & (1 << k) != 0 {
                let v = pdf_last_version(entry.tex.pdf.last(pdf_last_of(k)));
                r.read(&MCell::PdfLast(k), Some(&MValue::version(v)));
            }
        }
        // The writer's words: read (at the entry) where read first,
        // written where set (the lists' heads as the object table logged
        // them).
        let (words_read, words_written) = {
            let (r, w) = self.tex.tracker.pdf_words_touched();
            let (hr, hw) = self.tex.pdf.objs.take_heads();
            (r | hr, w | hw)
        };
        for k in 0..crate::pdf::word::COUNT {
            if words_read & (1 << k) != 0 && pdf_word_is_cell(&self.tex, k) {
                let v = pdf_last_version(entry.tex.pdf.word(k));
                r.read(&MCell::PdfWord(k), Some(&MValue::version(v)));
            }
        }
        self.tex.tracker.clear();
        self.opened.clear();
        // the exit, which the runtime reads next, and the next region's
        // entry
        self.take_snapshot();
        self.cut_at = self.steps;
        // (the values written: the exit's, if the recorder keeps them)
        let values = r.wants_values();
        let write = |r: &mut R, c: MCell| {
            let v = if values {
                self.get(&c).expect("a cell written holds a value")
            } else {
                MValue::version(0)
            };
            r.write(&c, &v);
        };
        write(r, MCell::Rest);
        if position_cells() {
            write(r, MCell::Positions);
        }
        if page_written {
            write(r, MCell::Page);
        }
        if marks_written {
            write(r, MCell::Marks);
        }
        for k in 0..u8::try_from(crate::pdf::PdfLast::ALL.len()).unwrap_or(0) {
            if last_written & (1 << k) != 0 {
                write(r, MCell::PdfLast(k));
            }
        }
        for k in 0..crate::pdf::word::COUNT {
            if words_written & (1 << k) != 0 && pdf_word_is_cell(&self.tex, k) {
                write(r, MCell::PdfWord(k));
            }
        }
        if any {
            write(r, MCell::Glyphs);
        }
        if classes {
            write(r, MCell::Classes);
        }
        for k in sealed {
            write(r, MCell::sealed(k));
        }
        for id in appended.keys() {
            write(r, MCell::Written(*id));
        }
        if numbered {
            write(r, MCell::Numbering);
        }
        if dested {
            write(r, MCell::Dests);
        }
        for p in writes {
            write(
                r,
                match p {
                    Tracked::Eqtb(q) => MCell::Eqtb(q),
                    Tracked::Link(q) => MCell::Link(q),
                    Tracked::Font(f) => MCell::Font(f),
                },
            );
        }
        for c in fonts_written {
            write(r, c);
        }
        if any_loaded {
            write(r, MCell::FontOrder);
        }
        for n in names_written {
            write(r, MCell::Name(n));
        }
        for &k in &objs.writes {
            // (an entry taken away again by a later region's restore is
            // absent: its write says so)
            let c = MCell::Obj(k);
            let v = if values {
                self.get(&c).unwrap_or_else(|| MValue::version(0))
            } else {
                MValue::version(0)
            };
            r.write(&c, &v);
        }
        for n in &objs.names_written {
            write(r, MCell::of_name(n));
        }
    }

    fn get(&self, c: &MCell) -> Option<MValue<H>> {
        match c {
            MCell::Link(p) => {
                let k = self.tex.slot_link(*p);
                Some(MValue {
                    version: link_version(k),
                    v: V::Int(k),
                })
            }
            MCell::Name(n) => Some(MValue::version(name_version(self.tex.name_location(n)))),
            MCell::Font(f) => self.tex.export_font(*f).map(|x| MValue {
                version: self.tex.font_version(*f),
                v: V::Font(Arc::new(x)),
            }),
            MCell::FontName(_) => Some(MValue::version(Self::font_cell_version(&self.tex, c))),
            MCell::FontExpand(b, e) => Some(MValue {
                version: Self::font_cell_version(&self.tex, c),
                v: V::Int(self.tex.fonts.expanded.get(&(*b, *e)).copied().unwrap_or(0)),
            }),
            MCell::FontOrder => Some(MValue {
                version: self.tex.fonts.order_hash,
                // (what the region just cut loaded; elsewhere nothing, as
                // for `Numbering`)
                v: V::FontOrder(if self.snapshot_now().is_some() {
                    self.font_delta.clone()
                } else {
                    Arc::default()
                }),
            }),
            MCell::Eqtb(p) => Some(MValue {
                version: self.tex.mcell_content(*p),
                v: match self.snapshot_now() {
                    Some(s) => V::Lazy(s.clone(), *p),
                    None => V::Word(Arc::new(self.tex.export_cell(*p))),
                },
            }),
            MCell::File(path) => self.tex.host.file(path).map(|f| MValue {
                version: StableHasher::of(&f[..]),
                v: V::File(Arc::new(f)),
            }),
            MCell::Glyphs => Some(MValue {
                version: Self::glyphs_version(&self.tex),
                // (what the region just cut added; elsewhere nothing, as
                // what a rebuild compares it by is the version)
                v: V::Glyphs(if self.snapshot_now().is_some() {
                    self.glyphs.clone()
                } else {
                    Arc::default()
                }),
            }),
            MCell::Line(path, k) => self.line_version(path, *k).map(|v| MValue {
                version: v,
                v: V::Line,
            }),
            MCell::Classes => Some(MValue::version(self.tex.class_hash)),
            MCell::Positions => position_cells().then(|| {
                let p = self.positions();
                MValue {
                    version: p.version(),
                    v: V::Positions(Arc::new(p)),
                }
            }),
            MCell::FinalNum(k) => Some(MValue::version(num_answer_version(
                self.tex.pdf.objs.peek_final_num(*k),
            ))),
            MCell::OfFinal(n) => Some(MValue::version(num_answer_version(
                self.tex.pdf.objs.peek_of_final(*n),
            ))),
            MCell::Origin(_) => Some(MValue::version(ORIGIN_VERSION)),
            MCell::IntCmp(_, x, rel, y) => Some(MValue::version(int_cmp_version(*x, *rel, *y))),
            MCell::NumState => Some(MValue::version(num_answer_version(
                self.tex.pdf.objs.alog.counters,
            ))),
            MCell::PdfLast(k) => pdf_last_cells().then(|| {
                let v = self.tex.pdf.last(pdf_last_of(*k));
                MValue {
                    version: pdf_last_version(v),
                    v: V::Int(v),
                }
            }),
            MCell::PdfWord(k) => pdf_word_is_cell(&self.tex, *k).then(|| {
                let v = self.tex.pdf.word(*k);
                MValue {
                    version: pdf_last_version(v),
                    v: V::Int(v),
                }
            }),
            // (from the snapshot of the state now, whose chunks it
            // shares, if there is one)
            MCell::Marks => mark_cells().then(|| MValue {
                version: marks_version(&self.tex.cur_mark),
                v: V::Marks(Arc::new(self.tex.cur_mark.clone())),
            }),
            MCell::Page => page_cells().then(|| {
                if let Some(s) = self.snapshot_now() {
                    return MValue {
                        version: s.page_version,
                        v: V::Page(Arc::new(s.page_value())),
                    };
                }
                let mut builder = self.tex.page.clone();
                let list = core::mem::take(&mut builder.list)
                    .into_vec()
                    .chunks(LIST_CHUNK)
                    .map(Arc::from)
                    .collect();
                MValue {
                    version: self.tex_page_version(),
                    v: V::Page(Arc::new(PageValue { builder, list })),
                }
            }),
            MCell::Sealed(hi, lo) => self.tex.seals.get(seal_key(*hi, *lo)).map(|x| MValue {
                version: x.version(),
                v: V::Sealed(x.clone()),
            }),
            MCell::Written(id) => Self::written_version(&self.tex, *id).map(|version| MValue {
                version,
                // (what the region just cut appended; elsewhere nothing,
                // as for `Glyphs`)
                v: V::Written(
                    self.snapshot_now()
                        .and_then(|_| self.written_delta.get(id).cloned())
                        .unwrap_or_default(),
                ),
            }),
            MCell::Obj(k) => Self::obj_value(&self.tex.pdf.objs, *k, true),
            MCell::Tree(t, i) => {
                Self::name_value(&self.tex.pdf.objs, &NameCell::Tree(*t, Id::of_key(i)), true)
            }
            MCell::Dests => Some(MValue {
                version: self.tex.pdf.objs.dest_hash,
                // (what the region just cut added; elsewhere nothing, as
                // for `Written`)
                v: V::Dests(if self.snapshot_now().is_some() {
                    self.dests_delta.clone()
                } else {
                    Arc::default()
                }),
            }),
            MCell::Numbering => Some(MValue {
                version: self.tex.pdf.objs.alog.hash,
                // (what the region just cut added; elsewhere nothing, as
                // for `Written`)
                v: V::Numbering(if self.snapshot_now().is_some() {
                    self.num_delta.clone()
                } else {
                    Arc::default()
                }),
            }),
            MCell::Rest => Some(match self.snapshot_now() {
                Some(s) => MValue {
                    version: self.rest_version,
                    v: V::Rest(s.clone()),
                },
                None => MValue {
                    version: self.rest_version_slow(),
                    v: V::Rest(Arc::new(Snapshot::of(
                        &mut self.tex.clone(),
                        self.started,
                        self.halted,
                        None,
                    ))),
                },
            }),
        }
    }

    fn set(&mut self, c: &MCell, v: Option<MValue<H>>) {
        self.fresh = true;
        self.snap_at = u64::MAX;
        match (c, v.map(|v| v.v)) {
            (MCell::Eqtb(p), Some(V::Word(w))) => self.tex.import_cell(*p, &w),
            (MCell::Eqtb(p), Some(V::Lazy(s, q))) => {
                let w = s.tex.export_cell(q);
                self.tex.import_cell(*p, &w);
            }
            (MCell::File(path), Some(V::File(f))) => {
                self.tex.host.set_file(path, Some(Arc::unwrap_or_clone(f)));
            }
            (MCell::File(path), None) => self.tex.host.set_file(path, None),
            // (a line is served with its file, set before it; a `\write`
            // file absent before has nothing to take away)
            // (a slot never made: nothing to take away, as it is not in
            // the order of loading)
            // (the questions about the numbering, `FinalNum`, `OfFinal`,
            // `NumState`, as `Name`: setting them does nothing)
            (
                MCell::Line(..)
                | MCell::Classes
                | MCell::Name(_)
                | MCell::FontName(_)
                | MCell::FinalNum(_)
                | MCell::OfFinal(_)
                | MCell::NumState
                | MCell::Origin(_)
                | MCell::IntCmp(..),
                _,
            )
            | (
                MCell::Written(_)
                | MCell::Positions
                | MCell::Numbering
                | MCell::Dests
                | MCell::FontOrder
                | MCell::Font(_),
                None,
            ) => {}
            (MCell::Glyphs, Some(V::Glyphs(g))) => {
                for (f, g) in g.iter().enumerate() {
                    if *g != [0; 4] {
                        let fonts = &mut self.tex.pdf.ship.fonts;
                        if fonts.len() <= f {
                            fonts.resize(f + 1, crate::pdf::ship::PdfFont::default());
                        }
                        for (a, b) in fonts[f].chars.iter_mut().zip(g) {
                            *a |= b;
                        }
                    }
                }
            }
            (MCell::Rest, Some(V::Rest(s))) => {
                self.restore_rest(&s, true);
            }
            (MCell::Positions, Some(V::Positions(p))) => self.set_positions(&p),
            (MCell::Page, Some(V::Page(p))) => self.tex.page = p.builder(),
            (MCell::Marks, Some(V::Marks(m))) => self.tex.cur_mark = Arc::unwrap_or_clone(m),
            (MCell::PdfLast(k), Some(V::Int(v))) => *self.tex.pdf.last_mut(pdf_last_of(*k)) = v,
            (MCell::PdfWord(k), Some(V::Int(v))) => *self.tex.pdf.word_mut(*k) = v,
            (MCell::Sealed(hi, lo), Some(V::Sealed(x))) => {
                self.tex.seals.insert(seal_key(*hi, *lo), x);
            }
            (MCell::Sealed(hi, lo), None) => self.tex.seals.remove(seal_key(*hi, *lo)),
            (MCell::Written(id), Some(V::Written(b))) => self.tex.host.append_written(*id, &b),
            (MCell::Obj(k), Some(V::Obj(e))) => {
                self.tex
                    .pdf
                    .objs
                    .set_entry(*k, Some(Arc::unwrap_or_clone(e)));
            }
            (MCell::Obj(k), None) => self.tex.pdf.objs.set_entry(*k, None),
            (MCell::Numbering, Some(V::Numbering(d))) => {
                for &e in d.iter() {
                    self.tex.pdf.objs.num_event(e);
                }
            }
            (MCell::Tree(t, i), v @ (Some(V::Tree(_)) | None)) => {
                let n = match v {
                    Some(V::Tree(n)) => Some(n),
                    _ => None,
                };
                self.tex
                    .pdf
                    .objs
                    .set_tree_entry(usize::from(*t), &Id::of_key(i), n);
            }
            (MCell::Dests, Some(V::Dests(d))) => self.tex.pdf.objs.add_dests(&d),
            (MCell::Link(p), Some(V::Int(k))) => self.tex.set_slot_link(*p, k),
            (MCell::Font(f), Some(V::Font(x))) => self.tex.import_font(*f, &x),
            (MCell::FontExpand(b, e), Some(V::Int(k))) => {
                if k == 0 {
                    self.tex.fonts.expanded.remove(&(*b, *e));
                } else {
                    self.tex.fonts.expanded.insert((*b, *e), k);
                }
            }
            (MCell::FontOrder, Some(V::FontOrder(d))) => {
                for &f in d.iter() {
                    self.tex.fonts.push_order(f);
                }
                // (the fonts loaded but the null font, which is first)
                self.tex.font_ptr =
                    i32::try_from(self.tex.fonts.order.len().saturating_sub(1)).unwrap_or(0);
            }
            (c, _) => panic!("{c:?} cannot take a version-only or absent value"),
        }
        // (what setting touched is not a region's)
        self.reset_tracker();
        self.tex.seal_log.clear();
        self.tex.classes_read = false;
        self.tex.classes_written = false;
        let _ = self.tex.pdf.objs.take_log();
        self.tex.pdf.objs.log.forced = false;
        self.tex.pdf.objs.log.observers = Default::default();
        self.tex.pdf.objs.log.answers.clear();
        self.tex.pdf.objs.log.dests_read = false;
        self.tex.name_log.clear();
        self.tex.font_log.clear();
        self.tex.fonts.order_read.set(false);
        self.objs = ObjRegion::default();
        let _ = self.tex.host.take_written();
    }

    fn at(&self) -> Pos {
        let t = &self.tex;
        let mut h = StableHasher::new();
        if t.stopped_before_ship() {
            // (before a `\shipout`: named by the file position below the
            // output routine and the pages shipped, not by token lists)
            (b"ship", self.started, t.in_open, t.shipped).hash(&mut h);
            return Pos {
                key: h.finish128(),
                file: self.file_below(),
                line: t.line,
            };
        }
        (self.started, self.halted, t.in_open, t.input_ptr).hash(&mut h);
        let i = &t.cur_input;
        (i.state, i.index, i.start, i.loc, i.limit).hash(&mut h);
        let file = if let Some(f) = self.file_name_hash(i) {
            f
        } else {
            // (a token list's `name` is a control sequence, §390, by
            // location)
            (0u8, i.name).hash(&mut h);
            0
        };
        Pos {
            key: h.finish128(),
            file,
            line: t.line,
        }
    }

    fn seek(&mut self, _b: &Pos) {
        // (the position is in `Rest`, which the replayed region wrote; a
        // snapshot now answers the guards checked here, and is the entry
        // of a region run from here)
        self.fresh = true;
        self.take_snapshot();
    }

    fn dense_cell(c: &MCell) -> Option<u32> {
        // (eqtb words: `Eqtb` sorts after the kinds before it and by
        // location, and nothing sorts between two of them)
        // (a register above 255 past eqtb at its largest, as `index`
        // numbers it: in order, and dense enough to count)
        match c {
            MCell::Eqtb(p) if *p >= crate::xregs::EXT_BASE => {
                u32::try_from(*p - crate::xregs::EXT_BASE)
                    .ok()
                    .map(|i| XREG_INDEX - 3 + i)
            }
            MCell::Eqtb(p) => u32::try_from(*p).ok(),
            MCell::Link(p) => u32::try_from(*p - crate::web::HASH_BASE)
                .ok()
                .map(|i| LINK_INDEX - 3 + i),
            MCell::Font(f) => u32::try_from(*f).ok().map(|i| FONT_INDEX - 3 + i),
            _ => None,
        }
    }

    fn digest(&self) -> Version {
        let mut h = StableHasher::new();
        for (_, p) in self.digest_parts() {
            h.write_u128(p);
        }
        Version(h.finish128())
    }

    fn render(e: &Effect, _cx: &mut LinkCtx<'_, Self>, out: &mut Vec<u8>) {
        Self::render_effect(e, out);
    }
    fn comparable_output(out: Vec<u8>) -> Vec<u8> {
        // (TeX's statistics about its tables: not reproduced, `AGENTS.md`)
        crate::sanitize::mask_statistics(&out)
    }

    fn accumulates(c: &MCell) -> bool {
        matches!(
            c,
            MCell::Glyphs | MCell::Written(_) | MCell::Numbering | MCell::Dests | MCell::FontOrder
        )
    }

    fn origin_number(c: &MCell, v: Option<&MValue<H>>) -> Option<i64> {
        machine_reloc::origin_number(c, v)
    }

    fn guard_relocates(c: &MCell, shifts: &[partex_incr::Shift<MCell>]) -> bool {
        machine_reloc::guard_relocates(c, shifts)
    }

    fn relocate_guard(c: &MCell, shifts: &[partex_incr::Shift<MCell>]) -> MCell {
        machine_reloc::relocate_guard(c, shifts)
    }

    fn relocate_value(
        c: &MCell,
        v: &MValue<H>,
        shifts: &[partex_incr::Shift<MCell>],
    ) -> Result<Option<MValue<H>>, ()> {
        machine_reloc::relocate_value(c, v, shifts)
    }

    fn derived(c: &MCell) -> Option<MCell> {
        matches!(c, MCell::FinalNum(_) | MCell::OfFinal(_) | MCell::NumState)
            .then_some(MCell::Numbering)
    }

    fn replay_exit(
        &mut self,
        span: &[&partex_incr::Trace<Self>],
        d: &mut dyn Iterator<Item = (&MCell, &Option<MValue<H>>)>,
    ) -> bool {
        // (the old run's exit is the last region's `Rest`: a snapshot of
        // the whole engine, eqtb included)
        let Some(last) = span.last() else {
            return false;
        };
        let Some((snap, version)) = last.writes.iter().find_map(|(c, v, _)| match (c, &v.v) {
            (MCell::Rest, V::Rest(s)) => Some((s.clone(), v.version)),
            _ => None,
        }) else {
            return false;
        };
        if !self.replay_exit {
            return false;
        }
        let timing = CUT_TIMING.load(Relaxed);
        let clock = |m: &Self| if timing { m.tex.host.nanos() } else { 0 };
        let t0 = clock(self);
        let rebased = self.restore_rest(&snap, false);
        let t1 = clock(self);
        // (the line numbers are the cell's, which a rebuild may have
        // renamed; `Rest` does not hash them)
        if let Some(p) = last.writes.iter().find_map(|(c, v, _)| match (c, &v.v) {
            (MCell::Positions, V::Positions(p)) => Some(p.clone()),
            _ => None,
        }) {
            self.set_positions(&p);
        }
        // (whether the state's `Rest` stays the snapshot's, whose version
        // the trace keeps: nothing below touches what `Rest` hashes)
        let mut known = KNOWN_REST.load(Relaxed)
            && !rebased
            && self.tex.name_cells
            && self.tex.font_cells
            && self.tex.canon_strings
            && self.tex.pdf.objs.virt;
        for t in span {
            // (writes are sorted by cell: `Glyphs` is first or second,
            // the `Written` files together)
            if let Ok(i) = t.writes.binary_search_by(|(c, _, _)| c.cmp(&MCell::Glyphs)) {
                self.set(&MCell::Glyphs, Some(t.writes[i].1.clone()));
            }
            for c in [MCell::FontOrder, MCell::Dests, MCell::Numbering] {
                if let Ok(i) = t.writes.binary_search_by(|(x, _, _)| x.cmp(&c)) {
                    self.set(&c, Some(t.writes[i].1.clone()));
                }
            }
            let from = t.writes.partition_point(|(c, _, _)| *c < MCell::Written(0));
            for (c, v, _) in t.writes[from..]
                .iter()
                .take_while(|(c, _, _)| matches!(c, MCell::Written(_)))
            {
                self.set(c, Some(v.clone()));
            }
        }
        let t2 = clock(self);
        for (c, v) in d {
            if !Self::accumulates(c) {
                known &= self.leaves_rest(c);
                self.set(c, v.clone());
            }
        }
        let t3 = clock(self);
        self.fresh = true;
        self.snap_at = u64::MAX;
        self.reset_tracker();
        // (the position is `Rest`'s, a snapshot the entry of a region run
        // from here: `seek`, with `Rest`'s version known)
        self.take_snapshot_with(known.then_some(version));
        if timing {
            let t4 = clock(self);
            CUT_T[84].fetch_add(1, Relaxed);
            for (i, v) in [t1 - t0, t2 - t1, t3 - t2, t4 - t3, t4 - t0]
                .into_iter()
                .enumerate()
            {
                CUT_T[85 + i].fetch_add(v, Relaxed);
            }
        }
        true
    }

    fn combine(c: &MCell, a: &MValue<H>, b: &MValue<H>) -> MValue<H> {
        match (c, &a.v, &b.v) {
            (MCell::Glyphs, V::Glyphs(x), V::Glyphs(y)) => {
                let n = x.len().max(y.len());
                let at = |g: &Vec<[u64; 4]>, f: usize| g.get(f).copied().unwrap_or([0; 4]);
                let both = (0..n)
                    .map(|f| {
                        let (p, q) = (at(x, f), at(y, f));
                        [p[0] | q[0], p[1] | q[1], p[2] | q[2], p[3] | q[3]]
                    })
                    .collect();
                MValue {
                    version: b.version,
                    v: V::Glyphs(Arc::new(both)),
                }
            }
            (MCell::Written(_), V::Written(x), V::Written(y)) => MValue {
                version: b.version,
                v: V::Written(Arc::new([&x[..], &y[..]].concat())),
            },
            (MCell::Numbering, V::Numbering(x), V::Numbering(y)) => MValue {
                version: b.version,
                v: V::Numbering(Arc::new([&x[..], &y[..]].concat())),
            },
            (MCell::FontOrder, V::FontOrder(x), V::FontOrder(y)) => MValue {
                version: b.version,
                v: V::FontOrder(Arc::new([&x[..], &y[..]].concat())),
            },
            (MCell::Dests, V::Dests(x), V::Dests(y)) => MValue {
                version: b.version,
                v: V::Dests(Arc::new([&x[..], &y[..]].concat())),
            },
            _ => b.clone(),
        }
    }

    fn index(c: &MCell) -> Option<u32> {
        match c {
            MCell::Rest => Some(0),
            MCell::Glyphs => Some(1),
            MCell::Classes => Some(2),
            MCell::Eqtb(p) if *p >= crate::xregs::EXT_BASE => {
                u32::try_from(*p - crate::xregs::EXT_BASE)
                    .ok()
                    .map(|i| XREG_INDEX + i)
            }
            MCell::Eqtb(p) => u32::try_from(*p).ok().map(|p| p + 3),
            MCell::Link(p) => u32::try_from(*p - crate::web::HASH_BASE)
                .ok()
                .map(|i| LINK_INDEX + i),
            MCell::Font(f) => u32::try_from(*f).ok().map(|i| FONT_INDEX + i),
            MCell::File(_)
            | MCell::Name(_)
            | MCell::FontName(_)
            | MCell::FontExpand(..)
            | MCell::FontOrder
            | MCell::Line(..)
            | MCell::Positions
            | MCell::Page
            | MCell::Marks
            | MCell::PdfLast(_)
            | MCell::PdfWord(_)
            | MCell::Sealed(..)
            | MCell::Written(_)
            | MCell::Obj(_)
            | MCell::Tree(..)
            | MCell::Dests
            | MCell::Numbering
            | MCell::FinalNum(_)
            | MCell::OfFinal(_)
            | MCell::NumState
            | MCell::Origin(_)
            | MCell::IntCmp(..) => None,
        }
    }
}
