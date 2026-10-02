//! `PARTEX_EVENTS=DIR`: the event log, an observer of a plain build
//! (`DESIGN.md` 5.2). A tracker (the engine's hooks, no probe in the
//! engine) that keeps a table of every definition of eqtb and the hash
//! and a table of every command, and writes them to `DIR` as raw
//! little-endian columns, one file per field, for offline queries
//! (`scripts/events.py`, numpy's `fromfile`).
//!
//! - A *definition* is a command's net write of a cell: two writes in one
//!   command are one definition. The value a cell holds before the
//!   build's first command writes it (the format's) is a definition at
//!   command 0.
//! - `defs_t`, `defs_next`: the command that made it and the command of
//!   the cell's next definition (`u32::MAX`: none).
//! - `defs_last`, `defs_nread`: the last command that read it and how
//!   many commands did (a command's reads of it count once).
//! - `defs_bits`: its word; `defs_level`: the group level of the command
//!   that made it; `defs_kind`, `defs_cell`: the cell (`kinds.txt`).
//! - `cmd_file`, `cmd_line`, `cmd_level`, `cmd_outer`: each command's
//!   file (`files.txt`), line, group level, and whether it begins at an
//!   outer clean point; `ships`: the command of each `\shipout`.
//! - `names.txt`: the name of each eqtb cell with more than one
//!   definition.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;

use partex_core::Tex;
use partex_core::host::Host;
use partex_core::track::{Cell, Tracker};

const NONE: u32 = u32::MAX;
/// Eqtb locations of the registers above 255 start here (`xregs.rs`).
const EXT_BASE: i32 = 0x2000_0000;
const KINDS: [&str; 10] = [
    "eqtb",
    "hash",
    "hash_next",
    "font",
    "font_table",
    "read",
    "out",
    "random",
    "str",
    "eqtb_ext",
];

fn kind(c: Cell) -> (u8, u32) {
    let u = |p: i32| u32::try_from(p).unwrap_or(0);
    match c {
        Cell::Eqtb(p) if p >= EXT_BASE => (9, u(p - EXT_BASE)),
        Cell::Eqtb(p) => (0, u(p)),
        Cell::Hash(p) => (1, u(p)),
        Cell::HashNext(p) => (2, u(p)),
        Cell::Font(f) => (3, u(f)),
        Cell::FontTable => (4, 0),
        Cell::Read(n) => (5, u(n)),
        Cell::Out(n) => (6, u(n)),
        Cell::Random => (7, 0),
        Cell::Str(h) => (8, u(h)),
    }
}

#[derive(Default)]
struct Defs {
    kind: Vec<u8>,
    cell: Vec<u32>,
    t: Vec<u32>,
    next: Vec<u32>,
    last: Vec<u32>,
    nread: Vec<u32>,
    bits: Vec<u64>,
    level: Vec<u8>,
}

#[derive(Default)]
struct State {
    now: u32,
    level: u8,
    /// Each cell's current definition, plus one (0: none yet): dense for
    /// eqtb and the hash, a map for the rest.
    dense: [Vec<u32>; 3],
    other: HashMap<(u8, u32), u32>,
    defs: Defs,
    cmd_file: Vec<u16>,
    cmd_line: Vec<u32>,
    cmd_level: Vec<u8>,
    cmd_outer: Vec<u8>,
    ships: Vec<u32>,
    files: Vec<String>,
    file_ids: HashMap<String, u16>,
    /// The file open at each input level.
    open: Vec<u16>,
    rewrites: u64,
}

impl State {
    fn slot(&mut self, k: u8, i: u32) -> &mut u32 {
        if k <= 2 {
            let v = &mut self.dense[usize::from(k)];
            let i = i as usize;
            if i >= v.len() {
                v.resize(i + 1, 0);
            }
            &mut v[i]
        } else {
            self.other.entry((k, i)).or_insert(0)
        }
    }

    fn new_def(&mut self, k: u8, i: u32, t: u32, bits: u64) -> u32 {
        let d = &mut self.defs;
        let id = u32::try_from(d.t.len()).expect("fewer than 2^32 definitions");
        d.kind.push(k);
        d.cell.push(i);
        d.t.push(t);
        d.next.push(NONE);
        d.last.push(0);
        d.nread.push(0);
        d.bits.push(bits);
        d.level.push(if t == 0 { 1 } else { self.level });
        id
    }
}

/// The event log's tracker.
#[derive(Default)]
pub struct Log {
    s: RefCell<State>,
}

impl Tracker for Log {
    const VALUES: bool = true;
    fn read(&self, _: Cell) {}
    fn write(&self, _: Cell) {}
    fn read_value(&self, cell: Cell, bits: u64) {
        let s = &mut *self.s.borrow_mut();
        let (k, i) = kind(cell);
        let now = s.now;
        let cur = *s.slot(k, i);
        let d = if cur == 0 {
            let d = s.new_def(k, i, 0, bits);
            *s.slot(k, i) = d + 1;
            d
        } else {
            cur - 1
        } as usize;
        if s.defs.last[d] != now || s.defs.nread[d] == 0 {
            s.defs.last[d] = now;
            s.defs.nread[d] += 1;
        }
    }
    fn write_value(&self, cell: Cell, old: u64, new: u64) {
        let s = &mut *self.s.borrow_mut();
        let (k, i) = kind(cell);
        let now = s.now;
        let cur = *s.slot(k, i);
        if cur != 0 && s.defs.t[(cur - 1) as usize] == now {
            // (a command's net write)
            s.defs.bits[(cur - 1) as usize] = new;
            s.rewrites += 1;
            return;
        }
        let prev = if cur == 0 {
            s.new_def(k, i, 0, old)
        } else {
            cur - 1
        };
        s.defs.next[prev as usize] = now;
        let d = s.new_def(k, i, now, new);
        *s.slot(k, i) = d + 1;
    }
    fn file(&self, depth: usize, name: Option<&[u8]>) {
        let s = &mut *self.s.borrow_mut();
        if let Some(n) = name {
            let n = String::from_utf8_lossy(n).into_owned();
            let next = u16::try_from(s.files.len()).unwrap_or(u16::MAX);
            let id = *s.file_ids.entry(n.clone()).or_insert(next);
            if usize::from(id) == s.files.len() {
                s.files.push(n);
            }
            s.open.truncate(depth);
            s.open.resize(depth, u16::MAX);
            s.open.push(id);
        } else {
            s.open.truncate(depth);
        }
    }
    fn page(&self) {
        let s = &mut *self.s.borrow_mut();
        let now = s.now;
        s.ships.push(now);
    }
    fn command(&self, n: u64, depth: usize, line: i32, level: i32, outer: bool) {
        let s = &mut *self.s.borrow_mut();
        s.now = u32::try_from(n).expect("fewer than 2^32 commands");
        s.level = u8::try_from(level).unwrap_or(u8::MAX);
        let file = s
            .open
            .get(depth.min(s.open.len().saturating_sub(1)))
            .copied()
            .unwrap_or(u16::MAX);
        s.cmd_file.push(file);
        s.cmd_line.push(u32::try_from(line).unwrap_or(0));
        s.cmd_level.push(s.level);
        s.cmd_outer.push(u8::from(outer));
    }
}

fn column<T: Copy>(
    dir: &Path,
    name: &str,
    v: &[T],
    bytes: impl Fn(T) -> Vec<u8>,
) -> std::io::Result<()> {
    let mut w = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(dir.join(name))?);
    for &x in v {
        w.write_all(&bytes(x))?;
    }
    w.flush()
}

/// Write the log of `tex`'s build to `dir`.
pub fn write<H: Host>(tex: &Tex<H, Log>, dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let s = tex.tracker().s.borrow();
    let d = &s.defs;
    column(dir, "defs_kind.u8", &d.kind, |x| vec![x])?;
    column(dir, "defs_cell.u32", &d.cell, |x| x.to_le_bytes().to_vec())?;
    column(dir, "defs_t.u32", &d.t, |x| x.to_le_bytes().to_vec())?;
    column(dir, "defs_next.u32", &d.next, |x| x.to_le_bytes().to_vec())?;
    column(dir, "defs_last.u32", &d.last, |x| x.to_le_bytes().to_vec())?;
    column(dir, "defs_nread.u32", &d.nread, |x| {
        x.to_le_bytes().to_vec()
    })?;
    column(dir, "defs_bits.u64", &d.bits, |x| x.to_le_bytes().to_vec())?;
    column(dir, "defs_level.u8", &d.level, |x| vec![x])?;
    column(dir, "cmd_file.u16", &s.cmd_file, |x| {
        x.to_le_bytes().to_vec()
    })?;
    column(dir, "cmd_line.u32", &s.cmd_line, |x| {
        x.to_le_bytes().to_vec()
    })?;
    column(dir, "cmd_level.u8", &s.cmd_level, |x| vec![x])?;
    column(dir, "cmd_outer.u8", &s.cmd_outer, |x| vec![x])?;
    column(dir, "ships.u32", &s.ships, |x| x.to_le_bytes().to_vec())?;
    std::fs::write(dir.join("files.txt"), s.files.join("\n") + "\n")?;
    std::fs::write(dir.join("kinds.txt"), KINDS.join("\n") + "\n")?;
    // (the names of the eqtb cells defined more than once)
    let mut count: HashMap<u32, u32> = HashMap::new();
    for (k, c) in d.kind.iter().zip(&d.cell) {
        if *k == 0 {
            *count.entry(*c).or_default() += 1;
        }
    }
    let mut cells: Vec<u32> = count
        .into_iter()
        .filter(|&(_, n)| n > 1)
        .map(|(c, _)| c)
        .collect();
    cells.sort_unstable();
    let meta = format!(
        "commands {}\ndefinitions {}\nrewrites {}\nships {}\n",
        s.cmd_line.len(),
        d.t.len(),
        s.rewrites,
        s.ships.len()
    );
    // (the names are read through the engine's accessors, which tell the
    // tracker)
    drop(s);
    let mut names = String::new();
    for c in cells {
        let name = tex
            .eqtb_loc_name(i32::try_from(c).unwrap_or(0))
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\t', "\\t");
        let _ = writeln!(names, "{c}\t{name}");
    }
    std::fs::write(dir.join("names.txt"), names)?;
    std::fs::write(dir.join("meta.txt"), meta)
}
