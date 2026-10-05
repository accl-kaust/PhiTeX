//! What each interval between checkpoints of a session read: the cells
//! it read before writing them (its exposed reads) and the cells it
//! wrote. An interval whose exposed reads avoid every cell in which a
//! rebuild's state differs from the previous build's runs the same way
//! (DESIGN.md §7.0, read-set cutoff); `PARTEX_READSETS=0` stops
//! recording, and with it that cutoff.
//!
//! One tracker is shared by an engine and all its clones: the session
//! takes what it recorded at every checkpoint and clears it whenever an
//! engine resumes.

use std::cell::{Cell as StdCell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use partex_core::track::{Cell, LineCodes, Tracker, tokenizes};

/// What happened to a line of an input file (source text as values,
/// DESIGN.md §7.2), the file by its contents' address ([`file_id`]; the
/// tracker keeps the contents alive, so no other file takes that address
/// while events name it) and the line by where it begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEvent {
    /// The file was opened to be read by lines.
    Open(usize),
    /// The line was read into the buffer.
    Start(usize, usize),
    /// It was done, read whole under the codes numbered so (`codes`), or
    /// under codes that changed while it was read (`None`).
    End(usize, usize, Option<u32>),
    /// An error context showed it.
    Shown(usize, usize),
    /// Lines of the file were added or removed since these events: those
    /// about it cannot be used.
    Poison(usize),
}

impl LineEvent {
    pub fn file(self) -> usize {
        match self {
            Self::Open(f)
            | Self::Poison(f)
            | Self::Start(f, _)
            | Self::End(f, _, _)
            | Self::Shown(f, _) => f,
        }
    }
}

/// Cells below this many locations are kept in bit sets.
const DENSE: usize = FAMILIES + 16384;

/// Locations in each of eqtb, the hash slots' names and their links.
const SLOTS: usize = 1 << 20;

struct Bits(Box<[StdCell<u64>]>);

impl Bits {
    fn new() -> Self {
        Self((0..DENSE / 64).map(|_| StdCell::new(0)).collect())
    }

    #[inline]
    fn get(&self, i: usize) -> bool {
        self.0[i >> 6].get() & (1 << (i & 63)) != 0
    }

    /// Set bit `i`: whether it was clear.
    #[inline]
    fn set(&self, i: usize) -> bool {
        let w = &self.0[i >> 6];
        let old = w.get();
        w.set(old | 1 << (i & 63));
        old & (1 << (i & 63)) == 0
    }

    fn clear_bit(&self, i: usize) {
        let w = &self.0[i >> 6];
        w.set(w.get() & !(1 << (i & 63)));
    }

    /// The bits set, cleared.
    fn take(&self, out: &mut Vec<u32>) {
        for (k, w) in self.0.iter().enumerate() {
            let mut b = w.replace(0);
            while b != 0 {
                out.push(u32::try_from(k * 64).unwrap_or(0) + b.trailing_zeros());
                b &= b - 1;
            }
        }
    }
}

/// A cell as a number: eqtb locations as they are, hash slots' names
/// after them, then their links.
#[inline]
fn index(c: Cell) -> Option<usize> {
    let slot = |p: i32| usize::try_from(p).ok().filter(|&i| i < SLOTS);
    let i = match c {
        // (`hash_extra` above about 950000: kept apart)
        Cell::Eqtb(p) => slot(p)?,
        Cell::Hash(p) => SLOTS + slot(p)?,
        Cell::HashNext(p) => 2 * SLOTS + slot(p)?,
        // the other families at the top
        Cell::Font(f) => FAMILIES + usize::try_from(f).ok().filter(|&f| f < FONTS)?,
        Cell::FontTable => FAMILIES + FONTS,
        Cell::Random => FAMILIES + FONTS + 1,
        Cell::Read(n) => FAMILIES + FONTS + 2 + usize::try_from(n).ok().filter(|&n| n < 16)?,
        Cell::Out(n) => FAMILIES + FONTS + 18 + usize::try_from(n).ok().filter(|&n| n < 16)?,
        // (far out: sparse)
        Cell::Str(_) => return None,
    };
    (i < DENSE).then_some(i)
}

/// Where the cells outside eqtb and the hash are numbered, and how many
/// fonts get numbers there.
const FAMILIES: usize = 3 * SLOTS;
const FONTS: usize = 16000;

/// The cell an [`Interval`]'s cell number stands for (see `index`).
pub fn cell_of(i: u32) -> Cell {
    let i = i as usize;
    let small = |k: usize| i32::try_from(k).unwrap_or(0);
    if i >= FAMILIES {
        let k = i - FAMILIES;
        match k {
            _ if k < FONTS => Cell::Font(small(k)),
            _ if k == FONTS => Cell::FontTable,
            _ if k == FONTS + 1 => Cell::Random,
            _ if k < FONTS + 18 => Cell::Read(small(k - FONTS - 2)),
            _ => Cell::Out(small(k - FONTS - 18)),
        }
    } else if i >= 2 * SLOTS {
        Cell::HashNext(small(i - 2 * SLOTS))
    } else if i >= SLOTS {
        Cell::Hash(small(i - SLOTS))
    } else {
        Cell::Eqtb(small(i))
    }
}

struct Inner {
    /// Whether to record at all (`PARTEX_READSETS=0`: not).
    on: bool,
    exposed: Bits,
    written: Bits,
    /// Cells past the bit sets (registers above 255, far out).
    exposed_far: RefCell<BTreeSet<Cell>>,
    written_far: RefCell<BTreeSet<Cell>>,
    /// The last read, if it was exposed only by that read (`retract`).
    newest: StdCell<Option<Cell>>,
    /// Whether to follow lines (`on`, and not `PARTEX_TOKEN_DEPS=0`).
    lines_on: bool,
    /// Writes to category codes and `\endlinechar` so far, in all the
    /// engines (never reset: a line begun and ended at the same count
    /// was read under one set of codes, whichever runs came between; it
    /// starts from the clock, so a checkpoint loaded from another process
    /// does not meet its count by chance).
    generation: StdCell<u64>,
    lines: RefCell<Vec<LineEvent>>,
    /// The codes lines were read under, and the codes the live engine
    /// had at the last generation taken (forgotten at every `take`: the
    /// session switches engines only between intervals, and two engines
    /// at the same count can have different codes).
    codes: RefCell<(Vec<LineCodes>, HashMap<LineCodes, u32>)>,
    codes_at: StdCell<Option<(u64, u32)>>,
    /// The files [`LineEvent`]s name, kept alive (`prune`).
    pins: RefCell<HashMap<usize, Arc<[u8]>>>,
}

/// The session's tracker.
#[derive(Clone)]
pub struct IntervalReads(Rc<Inner>);

impl Default for IntervalReads {
    fn default() -> Self {
        let on = std::env::var_os("PARTEX_READSETS").is_none_or(|v| v != "0");
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() ^ u64::from(d.subsec_nanos()) << 32)
            << 16;
        Self(Rc::new(Inner {
            on,
            lines_on: on && crate::tokendeps::enabled(),
            exposed: Bits::new(),
            written: Bits::new(),
            exposed_far: RefCell::default(),
            written_far: RefCell::default(),
            newest: StdCell::new(None),
            generation: StdCell::new(seed),
            lines: RefCell::default(),
            codes: RefCell::default(),
            codes_at: StdCell::new(None),
            pins: RefCell::default(),
        }))
    }
}

/// What an interval read and wrote, as cell numbers (see `index`), with
/// the cells past the bit sets, and where it ended (commands).
#[derive(Clone, Default)]
pub struct Interval {
    pub exposed: Vec<u32>,
    pub far: Vec<Cell>,
    pub written: Vec<u32>,
    pub written_far: Vec<Cell>,
    pub to: u64,
    /// Whether it was recorded (not with `PARTEX_READSETS=0`).
    pub recorded: bool,
    pub lines: Vec<LineEvent>,
    /// The characters of each PDF font it shipped (`Tex::take_chars_shipped`).
    pub chars: Vec<(usize, [u64; 4])>,
    /// The PDF object streams it wrote (`Tex::take_objstms_written`).
    pub objstms: Vec<partex_core::ObjStmWritten>,
}

impl IntervalReads {
    /// What was recorded since the last `take` or `clear`, cleared.
    pub fn take(&self) -> Interval {
        let mut i = Interval::default();
        if !self.0.on {
            return i;
        }
        i.recorded = true;
        self.0.exposed.take(&mut i.exposed);
        self.0.written.take(&mut i.written);
        i.far = core::mem::take(&mut *self.0.exposed_far.borrow_mut())
            .into_iter()
            .collect();
        i.written_far = core::mem::take(&mut *self.0.written_far.borrow_mut())
            .into_iter()
            .collect();
        self.0.newest.set(None);
        i.lines = core::mem::take(&mut *self.0.lines.borrow_mut());
        self.0.codes_at.set(None);
        i
    }

    /// Codes numbered `id` by [`LineEvent::End`].
    pub fn codes(&self, id: u32) -> Option<LineCodes> {
        self.0
            .codes
            .borrow()
            .0
            .get(usize::try_from(id).ok()?)
            .cloned()
    }

    /// Whether lines are followed (see [`LineEvent`]).
    pub fn follows_lines(&self) -> bool {
        self.0.lines_on
    }

    /// The contents of the file [`file_id`] `id` names, if events name it.
    #[cfg(test)]
    pub fn pinned(&self, id: usize) -> Option<Arc<[u8]>> {
        self.0.pins.borrow().get(&id).cloned()
    }

    /// Every file events have named since the last `prune`.
    pub fn pins(&self) -> Vec<Arc<[u8]>> {
        self.0.pins.borrow().values().cloned().collect()
    }

    /// Keep `new` alive as events name it now (`old`'s were moved to it).
    pub fn pin(&self, new: &Arc<[u8]>) {
        if self.0.lines_on {
            self.0.pins.borrow_mut().insert(file_id(new), new.clone());
        }
    }

    /// Let go of the files no event of `intervals` (nor one not taken
    /// yet) names.
    pub fn prune(&self, intervals: &[(u64, Interval)]) {
        // (the pins are few, the events many and in runs of one file)
        let mut pins = self.0.pins.borrow_mut();
        let mut unnamed: Vec<usize> = pins.keys().copied().collect();
        let lines = self.0.lines.borrow();
        let mut last = None;
        for f in intervals
            .iter()
            .flat_map(|(_, iv)| &iv.lines)
            .chain(lines.iter())
            .map(|e| e.file())
        {
            if unnamed.is_empty() {
                break;
            }
            if last != Some(f) {
                last = Some(f);
                unnamed.retain(|&u| u != f);
            }
        }
        pins.retain(|id, _| !unnamed.contains(id));
    }

    /// The number of an eqtb location, as in [`Interval`].
    pub fn eqtb_index(p: i32) -> Option<u32> {
        index(Cell::Eqtb(p)).and_then(|i| u32::try_from(i).ok())
    }

    /// The number of a cell, as in [`Interval`] (`None`: kept apart, in
    /// its `far` lists).
    pub fn cell_index(c: Cell) -> Option<u32> {
        index(c).and_then(|i| u32::try_from(i).ok())
    }
}

/// The address that names a file's contents in [`LineEvent`]s.
pub fn file_id(data: &[u8]) -> usize {
    data.as_ptr() as usize
}

impl Tracker for IntervalReads {
    const LINES: bool = true;

    fn lines_open(&self, data: &Arc<[u8]>) {
        if self.0.lines_on {
            let id = file_id(data);
            self.0.pins.borrow_mut().insert(id, data.clone());
            self.0.lines.borrow_mut().push(LineEvent::Open(id));
        }
    }

    fn line_start(&self, data: &[u8], from: usize) -> u64 {
        if self.0.lines_on {
            self.0
                .lines
                .borrow_mut()
                .push(LineEvent::Start(file_id(data), from));
        }
        self.0.generation.get()
    }

    fn line_end(
        &self,
        data: &[u8],
        from: usize,
        generation: u64,
        codes: impl FnOnce() -> Option<LineCodes>,
    ) {
        let s = &*self.0;
        if !s.lines_on {
            return;
        }
        let now = s.generation.get();
        let id = if generation != now {
            None
        } else if let Some((g, id)) = s.codes_at.get()
            && g == now
        {
            Some(id)
        } else {
            codes().map(|c| {
                let mut t = s.codes.borrow_mut();
                let (all, ids) = &mut *t;
                let id = *ids.entry(c.clone()).or_insert_with(|| {
                    all.push(c);
                    u32::try_from(all.len() - 1).unwrap_or(u32::MAX)
                });
                s.codes_at.set(Some((now, id)));
                id
            })
        };
        s.lines
            .borrow_mut()
            .push(LineEvent::End(file_id(data), from, id));
    }

    fn line_shown(&self, data: &[u8], from: usize) {
        if self.0.lines_on {
            self.0
                .lines
                .borrow_mut()
                .push(LineEvent::Shown(file_id(data), from));
        }
    }

    #[inline]
    fn read(&self, cell: Cell) {
        let s = &*self.0;
        if !s.on {
            return;
        }
        match index(cell) {
            Some(i) => {
                if !s.written.get(i) && s.exposed.set(i) {
                    s.newest.set(Some(cell));
                }
            }
            None => {
                if !s.written_far.borrow().contains(&cell)
                    && s.exposed_far.borrow_mut().insert(cell)
                {
                    s.newest.set(Some(cell));
                }
            }
        }
    }

    #[inline]
    fn write(&self, cell: Cell) {
        let s = &*self.0;
        if let Cell::Eqtb(p) = cell
            && s.lines_on
            && tokenizes(p)
        {
            s.generation.set(s.generation.get() + 1);
        }
        if !s.on {
            return;
        }
        match index(cell) {
            Some(i) => {
                s.written.set(i);
            }
            None => {
                s.written_far.borrow_mut().insert(cell);
            }
        }
    }

    fn retract(&self, cell: Cell) {
        let s = &*self.0;
        if s.newest.get() == Some(cell) {
            s.newest.set(None);
            match index(cell) {
                Some(i) => s.exposed.clear_bit(i),
                None => {
                    s.exposed_far.borrow_mut().remove(&cell);
                }
            }
        }
    }
}

/// The sessions' tracker.
pub type SessionTracker = IntervalReads;

/// What `t` recorded since the last call, cleared.
pub fn take(t: &SessionTracker) -> Interval {
    t.take()
}

/// Forget what `t` recorded.
pub fn clear(t: &SessionTracker) {
    let _ = take(t);
}

/// The tracker for another engine of the session (the same one).
pub fn share(t: &SessionTracker) -> SessionTracker {
    t.clone()
}
