//! The engine's tracker in pure SSA mode (DESIGN 3.17): the log of one
//! command's state accesses, in order, for its step to replay on the φ
//! core. The engine runs a command against its own tables, a cache of the
//! names; the step checks every read against the definition the core
//! says reaches it, and reports every write as a definition.

use core::cell::{Cell as StdCell, RefCell};

use alloc::vec::Vec;

use crate::ssa::{Fam, Slot};
use crate::track::{Cell, Row, Tracker};

/// One access, in the order the command made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ev {
    /// A read of a slot, with the version the engine saw.
    Read(Slot, u128),
    /// A write of a slot (`global`: a global assignment).
    Write(Slot, bool),
    /// The last write of the slot was a group's restore (§283), not an
    /// assignment.
    Restore(Slot),
    /// A group begins.
    Open,
    /// A group ends (before its restores).
    Close,
}

/// What the layer does with a slot's family.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// A name of the core's index: reads checked, writes defined.
    Name,
    /// Part of the step's state (the nest, the save stack, the
    /// conditionals, the scanner's position).
    State,
    /// The session's interners (the hash's chains, the string pool, their
    /// allocators): append-only, shared by every step, never put back.
    Interner,
    /// Not versioned by this layer yet: not checked (a gap the enforcer
    /// reports).
    Other,
}

/// The kind of slot `s`.
pub(crate) fn kind(s: Slot) -> Kind {
    use crate::track::scalar;
    match s.0 {
        Fam::Eqtb
        | Fam::Font
        | Fam::FontTable
        | Fam::Hyph
        | Fam::Pdf
        | Fam::Dvi
        | Fam::Out
        | Fam::Read
        | Fam::Random
        | Fam::Mark
        | Fam::Page
        | Fam::Sealed
        | Fam::Glyphs
        | Fam::PageNode => Kind::Name,
        // (the outer vertical list's fields, level 0, are names: a
        // paragraph's start reads its `\\prevdepth`, not the state)
        Fam::List if s.1 < i64::from(crate::track::list::COUNT) => Kind::Name,
        Fam::List | Fam::Save | Fam::Cond => Kind::State,
        Fam::Alloc => match u16::try_from(s.1).unwrap_or(u16::MAX) {
            scalar::STR_TOP | scalar::HASH_USED | scalar::HASH_HIGH => Kind::Interner,
            scalar::ALIGN_STATE | scalar::AFTER_TOKEN => Kind::State,
            _ => Kind::Name,
        },
        Fam::Hash | Fam::HashNext | Fam::Str | Fam::Pool | Fam::Name | Fam::Search => {
            Kind::Interner
        }
        _ => Kind::Other,
    }
}

/// The tracker.
#[derive(Default)]
pub struct PureTracker {
    pub(crate) log: RefCell<Vec<Ev>>,
    /// The eqtb entry the next write of which is global.
    global: StdCell<Option<i32>>,
    /// Families seen that this layer does not version (for the report).
    pub(crate) other: RefCell<Vec<Fam>>,
    /// The file levels that ended: each file's name and lines read.
    pub(crate) file_ends: RefCell<Vec<(usize, Vec<u8>, u32)>>,
    /// The files looked up to be read (`Tracker::pure_load`).
    pub(crate) loads: RefCell<Vec<Load>>,
    /// The files the job writes (`\\openout`): by name, the lines
    /// written so far, as the frontier has them (names of the core).
    pub(crate) stores: RefCell<std::collections::HashMap<Vec<u8>, Store>>,
    /// This command's accesses to them: (written, the name), in order.
    pub(crate) store_ev: RefCell<Vec<(bool, Vec<u8>, Option<phi::Ver>)>>,
}

/// A file the job writes: its lines so far (each without its end), and
/// a version made from them in order.
#[derive(Clone, Debug)]
pub(crate) struct Store {
    pub(crate) lines: phi::Seq<super::lang::Val>,
    pub(crate) ver: phi::Ver,
}

impl Store {
    pub(crate) fn empty() -> Store {
        Store {
            lines: phi::Seq::new(),
            ver: phi::Ver::of(&0x7374_6f72u32),
        }
    }

    /// Line `l` appended.
    pub(crate) fn push(&mut self, l: &[u8]) {
        let n = self.lines.len() as u64;
        self.lines.insert(
            self.lines.len(),
            phi::ElemId(n + 1),
            super::lang::Val::line(l),
        );
        self.ver = phi::Ver::node(0x7374_6f72, &[self.ver, phi::Ver::of(l)]);
    }

    /// The file's bytes: each line with its end.
    pub(crate) fn bytes(&self) -> alloc::sync::Arc<[u8]> {
        let mut b = Vec::new();
        for (_, v) in self.lines.iter() {
            if let super::lang::Val::Line(l, _) = v {
                b.extend_from_slice(l);
                b.push(b'\n');
            }
        }
        alloc::sync::Arc::from(b)
    }
}

/// A file looked up: the name asked for, the name found and the
/// contents, and whether it is read by lines.
#[derive(Clone, Debug)]
pub(crate) struct Load {
    pub(crate) name: Vec<u8>,
    pub(crate) found: Option<(alloc::sync::Arc<[u8]>, alloc::sync::Arc<[u8]>)>,
    pub(crate) lines: bool,
}

impl PureTracker {
    fn push(&self, e: Ev) {
        self.log.borrow_mut().push(e);
    }

    fn read_slot(&self, s: Slot, v: u128) {
        match kind(s) {
            Kind::Interner => {}
            Kind::Other => self.other_fam(s.0),
            _ => self.push(Ev::Read(s, v)),
        }
    }

    fn write_slot(&self, s: Slot, global: bool) {
        match kind(s) {
            Kind::Interner => {}
            Kind::Other => self.other_fam(s.0),
            _ => self.push(Ev::Write(s, global)),
        }
    }

    fn other_fam(&self, f: Fam) {
        let mut o = self.other.borrow_mut();
        if !o.contains(&f) {
            o.push(f);
        }
    }
}

impl Tracker for PureTracker {
    const VALUES: bool = true;
    const SOFT_READS: bool = true;
    const PURE: bool = true;

    fn read(&self, _cell: Cell) {}
    fn soft_read(&self, _cell: Cell, _level: i32) {}
    fn soft_read_eqtb(&self, _cell: Cell, _level: i32, _content: impl FnOnce() -> u128) {}
    fn write(&self, cell: Cell) {
        if let Cell::Eqtb(p) = cell {
            let g = self.global.take() == Some(p);
            self.write_slot(Slot(Fam::Eqtb, i64::from(p)), g);
        }
    }
    fn read_eqtb(&self, cell: Cell, content: impl Fn(bool) -> u128) {
        if let Cell::Eqtb(p) = cell {
            self.read_slot(Slot(Fam::Eqtb, i64::from(p)), content(false));
        }
    }
    fn restored(&self, cell: Cell, _level: i32) {
        if let Cell::Eqtb(p) = cell {
            self.push(Ev::Restore(Slot(Fam::Eqtb, i64::from(p))));
        }
    }
    fn row_read(&self, row: Row, content: impl FnOnce() -> u128) {
        let s = Slot::row(row);
        if matches!(kind(s), Kind::Name | Kind::State) {
            self.read_slot(s, content());
        } else {
            self.read_slot(s, 0);
        }
    }
    fn value_read(&self, row: Row, version: impl FnOnce() -> u128) {
        let s = Slot::row(row);
        if matches!(kind(s), Kind::Name | Kind::State) {
            self.read_slot(s, version());
        } else {
            self.read_slot(s, 0);
        }
    }
    fn row_wrote(&self, row: Row, _version: u128) {
        self.write_slot(Slot::row(row), true);
    }
    fn value_wrote(&self, row: Row) {
        self.write_slot(Slot::row(row), true);
    }
    fn stop_due(&self, _n: u64) -> bool {
        // (one command a step: the engine stops before each)
        true
    }
    fn pure_group(&self, open: bool) {
        self.push(if open { Ev::Open } else { Ev::Close });
    }
    fn pure_global(&self, p: i32) {
        self.global.set(Some(p));
    }
    fn reopen(&self, name: &[u8], kind: crate::host::FileKind) -> Option<crate::host::WriteId> {
        // (a file's handle is its name's: the same in every run of a
        // step and on every worker, never an allocator's count)
        let h = phi::ver::hash64(&(name, kind as u8));
        Some(crate::host::WriteId(0x8000_0000 | (h as u32 & 0x7fff_ffff)))
    }
    fn store_open(&self, _stream: u8, name: &[u8]) {
        self.stores
            .borrow_mut()
            .insert(name.to_vec(), Store::empty());
        self.store_ev.borrow_mut().push((true, name.to_vec(), None));
    }
    fn store_line(&self, name: &[u8], line: &[u8]) {
        if let Some(s) = self.stores.borrow_mut().get_mut(name) {
            s.push(line);
            self.store_ev.borrow_mut().push((true, name.to_vec(), None));
        }
    }
    fn stored(&self, name: &[u8]) -> Option<Option<alloc::sync::Arc<[u8]>>> {
        // (a file the job wrote before here: what it wrote; else the
        // host's; read either way)
        let st = self.stores.borrow();
        let s = st.get(name);
        self.store_ev
            .borrow_mut()
            .push((false, name.to_vec(), s.map(|s| s.ver)));
        s.map(|s| Some(s.bytes()))
    }
    fn pure_file_end(&self, level: usize, name: &[u8], lines: u32) {
        self.file_ends
            .borrow_mut()
            .push((level, name.to_vec(), lines));
    }
    fn pure_load(&self, name: &[u8], found: Option<(&[u8], &alloc::sync::Arc<[u8]>)>, lines: bool) {
        self.loads.borrow_mut().push(Load {
            name: name.to_vec(),
            found: found.map(|(n, c)| (alloc::sync::Arc::from(n), c.clone())),
            lines,
        });
    }
}

impl PureTracker {
    /// The families seen that this layer does not version yet.
    #[must_use]
    pub fn others(&self) -> Vec<Fam> {
        self.other.borrow().clone()
    }
}
