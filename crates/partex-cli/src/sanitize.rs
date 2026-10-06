//! `PARTEX_SANITIZE=1`: the tracking sanitizer (DESIGN.md §7.12).
//!
//! The job runs as usual, writing its outputs, but stops every
//! `PARTEX_SANITIZE_EVERY` commands (default 5000); the commands between
//! two stops are a region. For every `PARTEX_SANITIZE_SAMPLE`-th region
//! (default 1) the session's tracker (`intervals.rs`) says which cells the
//! region read before writing them and which it wrote, and:
//!
//! - **shadow diff**: every cell whose word changed across the region
//!   must have been reported written (an untracked write otherwise);
//! - **poisoning**: a copy of the region's entry state gets another value
//!   in every tracked cell the region did not report reading
//!   (`partex_core`'s `sanitize.rs` says which values) and runs the same
//!   commands. Its output, its cells and the rest of its state (the state
//!   hash without the cells) must come out as the real run's; the
//!   poisoned cells it did not write keep their poison. A difference is a
//!   read that bypassed the tracker, and bisecting the poisoned set names
//!   the cell.
//!
//! The copy sees the files, file dates and times the real run saw (the
//! host logs them and replays them), writes nothing, and prints nothing.
//! A report goes to standard error at the end: each untracked read and
//! write with its region and input line, by kind of cell.
//! `PARTEX_SANITIZE_LIMIT=n` checks at most `n` regions;
//! `PARTEX_SANITIZE_STOP=n` stops after `n` untracked reads;
//! `PARTEX_SANITIZE_LOG=file` appends the findings to `file`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hasher};
use std::rc::Rc;

use partex_core::diag::Diagnostic;
use partex_core::pageir::Page;
use partex_core::{Cell, DateTime, FileKind, Host, OpenedFile, Params, Step, Tex, WriteId};

use crate::intervals::{self, IntervalReads};
use crate::native::NativeHost;

pub fn wanted() -> bool {
    std::env::var_os("PARTEX_SANITIZE").is_some_and(|v| !v.is_empty() && v != "0")
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// An answer the host gave the real run, replayed to the copy.
#[derive(Clone)]
enum Answer {
    Read(Vec<u8>, FileKind, Option<(Vec<u8>, std::sync::Arc<[u8]>)>),
    Open(Vec<u8>, FileKind, Option<(WriteId, Vec<u8>)>),
    Seconds((i32, i32)),
    ModDate(Vec<u8>, Option<Vec<u8>>),
    CreationDate(Vec<u8>),
}

#[derive(Clone)]
enum Role {
    /// The real run: answers from the native host, logged.
    Real(Rc<RefCell<Vec<Answer>>>),
    /// A copy: answers replayed from the real run's log, in order.
    Copy(Rc<Vec<Answer>>, usize),
}

/// The sanitizer's host: the native host for the real run, a replay of it
/// for copies; every output hashed per file, per region.
#[derive(Clone)]
struct SanHost {
    base: Rc<RefCell<NativeHost>>,
    role: Role,
    /// What the region wrote: a hash per file (`u32::MAX`: the terminal).
    out: BTreeMap<u32, DefaultHasher>,
    /// The same, byte for byte, when debugging.
    bytes: Option<BTreeMap<u32, Vec<u8>>>,
    /// Text outputs (the log, the terminal) are hashed by line, with the
    /// numbers of TeX's memory statistics masked (poisoning changes what
    /// is allocated): each one's unfinished line.
    lines: BTreeMap<u32, Vec<u8>>,
}

/// A line of TeX's internal statistics (see `xtask/src/mask.rs`): its
/// numbers are not compared.
fn statistics(line: &[u8]) -> bool {
    let has = |k: &[u8]| line.windows(k.len()).any(|w| w == k);
    has(b" out of ") || has(b"Memory usage") || has(b"memory locations")
}

impl SanHost {
    fn log(&self, a: Answer) {
        if let Role::Real(l) = &self.role {
            l.borrow_mut().push(a);
        }
    }

    /// The next logged answer `pick` accepts: in order, else any.
    fn replay<R>(&mut self, pick: impl Fn(&Answer) -> Option<R>) -> Option<R> {
        let Role::Copy(log, at) = &mut self.role else {
            return None;
        };
        if let Some(r) = log.get(*at).and_then(&pick) {
            *at += 1;
            return Some(r);
        }
        log.iter().find_map(pick)
    }

    fn put(&mut self, file: u32, bytes: &[u8]) {
        if let Some(pending) = self.lines.get_mut(&file) {
            pending.extend_from_slice(bytes);
            let h = self.out.entry(file).or_default();
            while let Some(n) = pending.iter().position(|&b| b == b'\n') {
                let mut line: Vec<u8> = pending.drain(..=n).collect();
                if statistics(&line) {
                    line.retain(|b| !b.is_ascii_digit());
                }
                h.write(&line);
            }
        } else {
            self.out.entry(file).or_default().write(bytes);
        }
        if let Some(b) = &mut self.bytes {
            b.entry(file).or_default().extend_from_slice(bytes);
        }
    }

    fn real(&self) -> bool {
        matches!(self.role, Role::Real(_))
    }
}

impl Host for SanHost {
    fn read_file(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile> {
        let got = self.replay(|a| match a {
            Answer::Read(n, k, f) if n == name && *k == kind => Some(f.clone()),
            _ => None,
        });
        let f = got.unwrap_or_else(|| {
            self.base
                .borrow_mut()
                .read_file(name, kind)
                .map(|f| (f.name, f.contents))
        });
        self.log(Answer::Read(name.to_vec(), kind, f.clone()));
        f.map(|(name, contents)| OpenedFile { name, contents })
    }

    fn open_write(&mut self, name: &[u8], kind: FileKind) -> Option<(WriteId, Vec<u8>)> {
        let r = if self.real() {
            let r = self.base.borrow_mut().open_write(name, kind);
            self.log(Answer::Open(name.to_vec(), kind, r.clone()));
            r
        } else {
            self.replay(|a| match a {
                Answer::Open(n, k, r) if n == name && *k == kind => Some(r.clone()),
                _ => None,
            })
            .flatten()
        };
        if let Some((id, n)) = &r
            && n.ends_with(b".log")
        {
            self.lines.insert(id.0, Vec::new());
        }
        r
    }

    fn write(&mut self, file: WriteId, bytes: &[u8]) {
        self.put(file.0, bytes);
        if self.real() {
            self.base.borrow_mut().write(file, bytes);
        }
    }

    fn close(&mut self, file: WriteId) {
        self.put(file.0, b"\0close");
        if self.real() {
            self.base.borrow_mut().close(file);
        }
    }

    fn term_write(&mut self, bytes: &[u8]) {
        self.put(u32::MAX, bytes);
        if self.real() {
            self.base.borrow_mut().term_write(bytes);
        }
    }

    fn term_read_line(&mut self) -> Option<Vec<u8>> {
        None
    }

    fn now(&self) -> DateTime {
        self.base.borrow().now()
    }

    fn creation_date(&mut self) -> Vec<u8> {
        let d = self
            .replay(|a| match a {
                Answer::CreationDate(d) => Some(d.clone()),
                _ => None,
            })
            .unwrap_or_else(|| self.base.borrow_mut().creation_date());
        self.log(Answer::CreationDate(d.clone()));
        d
    }

    fn file_mod_date(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let d = self
            .replay(|a| match a {
                Answer::ModDate(n, d) if n == name => Some(d.clone()),
                _ => None,
            })
            .unwrap_or_else(|| self.base.borrow_mut().file_mod_date(name));
        self.log(Answer::ModDate(name.to_vec(), d.clone()));
        d
    }

    fn seconds_and_micros(&mut self) -> (i32, i32) {
        let t = self
            .replay(|a| match a {
                Answer::Seconds(t) => Some(*t),
                _ => None,
            })
            .unwrap_or_else(|| self.base.borrow_mut().seconds_and_micros());
        self.log(Answer::Seconds(t));
        t
    }

    fn diagnostic(&mut self, _d: &Diagnostic) {}

    fn page_written(&mut self, _page: &Page) {}

    fn deflate(&mut self, level: i32, data: &[u8]) -> Option<Vec<u8>> {
        crate::zlib::deflate_stream(level, data)
    }

    fn cache_get(&mut self, key: u128) -> Option<Vec<u8>> {
        self.base.borrow_mut().cache_get(key)
    }

    fn cache_put(&mut self, key: u128, value: &[u8]) {
        if self.real() {
            self.base.borrow_mut().cache_put(key, value);
        }
    }
}

type Engine = Tex<SanHost, IntervalReads>;

/// Run `t` until command `n` or the end: `history` if it ended.
fn run_to(t: &mut Engine, n: u64) -> Option<i32> {
    loop {
        t.set_stop_at(n);
        match t.resume() {
            Step::Finished(h) => return Some(h),
            Step::Checkpoint if t.commands() >= n => return None,
            Step::Checkpoint => {}
        }
    }
}

/// What one region of the real run did.
struct Region {
    entry: Engine,
    exit: Engine,
    ended: Option<i32>,
    to: u64,
    read: HashSet<Cell>,
    written: HashSet<Cell>,
    /// Where it began: commands and input line.
    at: String,
}

/// Parts of the state that are dead between two commands, and hold ids
/// (token lists, glue) that poisoning shifts: main control reads the next
/// token into `cur_cmd` … `cur_tok` before it looks at them, and every
/// command scans `cur_val` before it uses it.
const SCRATCH: [&str; 2] = ["scalars: cur_val", "scalars: current token"];

/// Why a poisoned copy differs from the real run.
fn differs(
    r: &Region,
    copy_entry: &Engine,
    copy: &Engine,
    poisoned: &HashSet<Cell>,
) -> Option<String> {
    let h = |t: &Engine| -> Vec<(u32, u64, Vec<u8>)> {
        let host = t.host();
        host.out
            .iter()
            .map(|(k, v)| {
                let rest = host.lines.get(k).cloned().unwrap_or_default();
                (*k, v.clone().finish(), rest)
            })
            .collect()
    };
    if h(&r.exit) != h(copy) {
        return Some("output".to_owned());
    }
    let (a, b) = (r.exit.untracked_state_parts(), copy.untracked_state_parts());
    let parts: Vec<&str> = a
        .iter()
        .zip(&b)
        .filter(|(x, y)| x.1 != y.1 && !SCRATCH.contains(&x.0))
        .map(|(x, _)| x.0)
        .collect();
    if !parts.is_empty() {
        return Some(format!("state: {}", parts.join(", ")));
    }
    let mut changed: BTreeSet<Cell> = r.entry.changed_cells(&r.exit).into_iter().collect();
    changed.extend(copy_entry.changed_cells(copy));
    changed.extend(r.written.iter().filter(|c| poisoned.contains(c)).copied());
    for c in changed {
        let poisoned_kept = poisoned.contains(&c) && !r.written.contains(&c);
        if poisoned_kept {
            if copy.cell_raw(c) != copy_entry.cell_raw(c) {
                return Some(format!("cell {c:?}: written only when poisoned"));
            }
        } else if r.exit.cell_content(c) != copy.cell_content(c) {
            return Some(format!("cell {c:?}"));
        }
    }
    None
}

thread_local! {
    /// The last panic of a copy, for the report.
    static PANIC: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Run the region again from its entry with `cells` poisoned (their
/// outputs kept byte for byte if `bytes`): the copy at its entry and its
/// end, and the cells poisoned; or why it went wrong.
fn run_copy(
    r: &Region,
    cells: &[Cell],
    bytes: bool,
) -> Result<(Engine, Engine, HashSet<Cell>), String> {
    let mut copy = r.entry.clone();
    let log = match &r.exit.host().role {
        Role::Real(l) => Rc::new(l.borrow().clone()),
        Role::Copy(..) => unreachable!("the real run's host"),
    };
    copy.host_mut().role = Role::Copy(log, 0);
    copy.host_mut().bytes = bytes.then(BTreeMap::new);
    let poisoned: HashSet<Cell> = copy.poison_cells(cells).into_iter().collect();
    let copy_entry = copy.clone();
    let to = r.to;
    let loud = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        PANIC.with(|p| *p.borrow_mut() = info.to_string());
    }));
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let e = run_to(&mut copy, to);
        (copy, e)
    }));
    std::panic::set_hook(loud);
    let Ok((copy, ended)) = ended else {
        return Err(format!("panicked: {}", PANIC.with(|p| p.borrow().clone())));
    };
    // (the copy's reads went into the shared tracker)
    intervals::clear(copy.tracker());
    if ended != r.ended || copy.commands() != r.exit.commands() {
        return Err("stopped elsewhere".to_owned());
    }
    Ok((copy_entry, copy, poisoned))
}

/// Run the region again from its entry with `cells` poisoned: why it
/// differs, if it does.
fn trial(r: &Region, cells: &[Cell]) -> Option<String> {
    match run_copy(r, cells, false) {
        Err(why) => Some(why),
        Ok((copy_entry, copy, poisoned)) => differs(r, &copy_entry, &copy, &poisoned),
    }
}

/// For `PARTEX_SANITIZE_DEBUG`: where the output of a copy with `cells`
/// poisoned first differs from an unpoisoned copy's.
fn output_difference(r: &Region, cells: &[Cell]) -> String {
    let (Ok((_, plain, _)), Ok((_, poisoned, _))) =
        (run_copy(r, &[], true), run_copy(r, cells, true))
    else {
        return String::new();
    };
    let (Some(real), Some(other)) = (&plain.host().bytes, &poisoned.host().bytes) else {
        return String::new();
    };
    for (id, u) in real {
        let v = other.get(id).map_or(&[][..], |v| &v[..]);
        if u[..] != *v {
            let at = u.iter().zip(v).take_while(|(p, q)| p == q).count();
            let show = |w: &[u8]| {
                String::from_utf8_lossy(&w[at.saturating_sub(120)..(at + 120).min(w.len())])
                    .into_owned()
            };
            return format!(
                "\n    file {id} at byte {at}:\n    real:     {:?}\n    poisoned: {:?}",
                show(u),
                show(v)
            );
        }
    }
    String::new()
}

/// A small set of cells whose poisoning still makes the copy differ, by
/// delta debugging (Zeller's ddmin): try each of `n` chunks, then each
/// complement, then finer chunks. Usually one cell.
fn bisect(r: &Region, cells: &[Cell], why: String) -> (Vec<Cell>, String) {
    let (mut cells, mut why) = (cells.to_vec(), why);
    let mut n = 2;
    let mut trials = 0;
    'outer: while cells.len() > 1 && trials < 80 {
        let size = cells.len().div_ceil(n);
        let chunks: Vec<Vec<Cell>> = cells.chunks(size).map(<[Cell]>::to_vec).collect();
        for c in &chunks {
            trials += 1;
            if let Some(w) = trial(r, c) {
                (cells, why, n) = (c.clone(), w, 2);
                continue 'outer;
            }
        }
        if chunks.len() > 2 {
            for k in 0..chunks.len() {
                let rest: Vec<Cell> = chunks
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| i != k)
                    .flat_map(|(_, c)| c.iter().copied())
                    .collect();
                trials += 1;
                if let Some(w) = trial(r, &rest) {
                    (cells, why, n) = (rest, w, (n - 1).max(2));
                    continue 'outer;
                }
            }
        }
        if n >= cells.len() {
            break;
        }
        n = (2 * n).min(cells.len());
    }
    (cells, why)
}

#[derive(Default)]
struct Report {
    regions: u64,
    checked: u64,
    poisoned: u64,
    read: u64,
    /// Untracked reads: cell name to (count, first region's place, why).
    reads: BTreeMap<String, (u64, String, String)>,
    writes: BTreeMap<String, (u64, String)>,
    /// Sections of the untracked state each region changed.
    sections: BTreeMap<&'static str, u64>,
    /// The last region's untracked state, by section.
    last_parts: Option<Vec<(&'static str, u128)>>,
    /// Cells found read untracked.
    known: HashSet<Cell>,
    /// Regions whose tracked writes are all eqtb cells.
    eqtb_only: u64,
    /// The adapter's round trip failing: first cell to (count, place).
    round_trip: BTreeMap<String, (u64, String)>,
}

fn name(t: &Engine, c: Cell) -> String {
    let n = String::from_utf8_lossy(&t.cell_names(&[c])[0]).into_owned();
    match c {
        Cell::Eqtb(p) => format!("{n} [{}]", t.eqtb_loc_name(p)),
        _ => n,
    }
}

fn place(t: &Engine, from: u64, to: u64) -> String {
    let (file, line) = t.input_location();
    format!(
        "commands {from}..{to} from {}:{line}",
        String::from_utf8_lossy(&file)
    )
}

/// The eqtb slice of the engine's `Machine` adapter (`DESIGN.md` §7.0):
/// the entry with the region's recorded writes applied must have the
/// exit's eqtb, by content.
fn adapter_round_trip(r: &Region, rep: &mut Report) {
    let mut patched = r.entry.clone();
    let mut written: Vec<Cell> = r.written.iter().copied().collect();
    written.sort_unstable();
    let rest = patched.apply_writes(&r.exit, &written);
    intervals::clear(patched.tracker());
    if rest.is_empty() {
        rep.eqtb_only += 1;
    }
    let bad = patched.eqtb_differences(&r.exit);
    if !bad.is_empty() {
        let e = rep
            .round_trip
            .entry(name(&r.exit, bad[0]))
            .or_insert((0, r.at.clone()));
        e.0 += 1;
    }
}

fn check(r: &Region, rep: &mut Report, debug: bool) {
    rep.checked += 1;
    let at = &r.at;
    // shadow diff: every change reported as a write
    for c in r.entry.changed_cells(&r.exit) {
        if !r.written.contains(&c) {
            let e = rep
                .writes
                .entry(name(&r.exit, c))
                .or_insert((0, at.clone()));
            e.0 += 1;
        }
    }
    adapter_round_trip(r, rep);
    let after = r.exit.untracked_state_parts();
    if let Some(before) = &rep.last_parts {
        for (x, y) in before.iter().zip(&after) {
            if x.1 != y.1 {
                *rep.sections.entry(x.0).or_default() += 1;
            }
        }
    }
    rep.last_parts = Some(after);
    // poisoning (cells already found read untracked are left alone: each
    // is reported once)
    let cells: Vec<Cell> = r
        .entry
        .cells_in_use()
        .into_iter()
        .filter(|c| !r.read.contains(c) && !rep.known.contains(c))
        .collect();
    rep.poisoned += cells.len() as u64;
    rep.read += r.read.len() as u64;
    let Some(mut why) = trial(r, &cells) else {
        return;
    };
    let mut left = cells;
    // several culprits: bisect one, take it out, try the rest again
    for _ in 0..4 {
        let (found, what) = bisect(r, &left, why);
        let mut key = found
            .iter()
            .take(3)
            .map(|&c| name(&r.exit, c))
            .collect::<Vec<_>>()
            .join(" + ");
        if found.len() > 3 {
            let _ = write!(key, " and {} more", found.len() - 3);
        }
        let detail = if debug && found.len() <= 3 && what == "output" {
            output_difference(r, &found)
        } else {
            String::new()
        };
        eprintln!("phitex sanitize: untracked read of {key}, {at} ({what}){detail}");
        let e = rep.reads.entry(key).or_insert((0, at.clone(), what));
        e.0 += 1;
        if found.len() == left.len() {
            break;
        }
        rep.known.extend(found.iter().copied());
        let gone: HashSet<Cell> = found.into_iter().collect();
        left.retain(|c| !gone.contains(c));
        match trial(r, &left) {
            Some(w) => why = w,
            None => break,
        }
    }
}

/// Run the job under the sanitizer; returns `history`.
pub fn run(host: NativeHost, mut params: Params, command_line: &[u8]) -> i32 {
    if params.interaction.is_none() {
        params.interaction = Some(1); // nonstop: no terminal
    }
    let every = env_u64("PARTEX_SANITIZE_EVERY", 5000).max(1);
    let sample = env_u64("PARTEX_SANITIZE_SAMPLE", 1).max(1);
    let limit = env_u64("PARTEX_SANITIZE_LIMIT", u64::MAX);
    let debug = std::env::var_os("PARTEX_SANITIZE_DEBUG").is_some();
    let stop_after = env_u64("PARTEX_SANITIZE_STOP", u64::MAX);
    let host = SanHost {
        base: Rc::new(RefCell::new(host)),
        role: Role::Real(Rc::default()),
        out: BTreeMap::new(),
        bytes: None,
        lines: BTreeMap::from([(u32::MAX, Vec::new())]),
    };
    let tracker = IntervalReads::default();
    let mut tex = Tex::new(host, intervals::share(&tracker), params);
    // (clones share eqtb and the hash in chunks; no checkpoints of its
    // own)
    tex.set_checkpoint_interval(1 << 40);
    tex.set_stop_at(every);
    let mut rep = Report::default();
    let mut step = tex.start(command_line);
    let mut history = match step {
        Step::Finished(h) => h,
        Step::Checkpoint => 0,
    };
    let started = std::time::Instant::now();
    while step == Step::Checkpoint {
        rep.regions += 1;
        let to = tex.commands() + every;
        if let Role::Real(l) = &tex.host().role {
            l.borrow_mut().clear();
        }
        tex.host_mut().out.clear();
        intervals::clear(&tracker);
        let wanted = rep.regions % sample == 0 && rep.checked < limit;
        let at = if wanted {
            place(&tex, tex.commands(), to)
        } else {
            String::new()
        };
        let entry = wanted.then(|| tex.clone());
        let ended = run_to(&mut tex, to);
        if let Some(h) = ended {
            history = h;
        }
        let iv = intervals::take(&tracker);
        if let Some(entry) = entry {
            let cells = |v: &[u32], far: &[Cell]| -> HashSet<Cell> {
                v.iter()
                    .map(|&i| intervals::cell_of(i))
                    .chain(far.iter().copied())
                    .collect()
            };
            let r = Region {
                entry,
                exit: tex,
                ended,
                to,
                read: cells(&iv.exposed, &iv.far),
                written: cells(&iv.written, &iv.written_far),
                at,
            };
            check(&r, &mut rep, debug);
            tex = r.exit;
            if rep.checked % 100 == 0 {
                eprintln!(
                    "phitex sanitize: {} regions checked, {} untracked reads, {:.1} s",
                    rep.checked,
                    rep.reads.len(),
                    started.elapsed().as_secs_f64()
                );
            }
            if rep.reads.len() as u64 >= stop_after {
                eprintln!("phitex sanitize: stopping after {stop_after} untracked reads");
                break;
            }
        }
        step = ended.map_or(Step::Checkpoint, Step::Finished);
    }
    print_report(&rep, started.elapsed());
    history
}

#[allow(clippy::cast_precision_loss)] // (averages)
fn print_report(rep: &Report, took: std::time::Duration) {
    eprintln!(
        "\nphitex sanitize: {} regions, {} checked in {:.1} s; {:.0} cells read and {:.0} poisoned per region",
        rep.regions,
        rep.checked,
        took.as_secs_f64(),
        rep.read as f64 / rep.checked.max(1) as f64,
        rep.poisoned as f64 / rep.checked.max(1) as f64
    );
    eprintln!(
        "phitex sanitize: {} untracked reads, {} untracked writes",
        rep.reads.len(),
        rep.writes.len()
    );
    for (k, (n, at, why)) in &rep.reads {
        eprintln!("  read  {k}: {n} regions, first {at} ({why})");
    }
    for (k, (n, at)) in &rep.writes {
        eprintln!("  write {k}: {n} regions, first {at}");
    }
    // (`PARTEX_SANITIZE_LOG=file`: the findings appended there too, with
    // the job's directory, for runs whose standard error is not kept, as
    // the e2e cases')
    if let Some(log) = std::env::var_os("PARTEX_SANITIZE_LOG") {
        let dir = std::env::current_dir().unwrap_or_default();
        let mut text = format!(
            "{}: {} regions checked, {} untracked reads, {} untracked writes\n",
            dir.display(),
            rep.checked,
            rep.reads.len(),
            rep.writes.len()
        );
        for (k, (n, at, why)) in &rep.reads {
            let _ = writeln!(text, "  read  {k}: {n} regions, first {at} ({why})");
        }
        for (k, (n, at)) in &rep.writes {
            let _ = writeln!(text, "  write {k}: {n} regions, first {at}");
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            let _ = std::io::Write::write_all(&mut f, text.as_bytes());
        }
    }
    eprintln!(
        "phitex sanitize: adapter round trip (eqtb): {} failures; {} regions wrote only eqtb cells",
        rep.round_trip.len(),
        rep.eqtb_only
    );
    for (k, (n, at)) in &rep.round_trip {
        eprintln!("  eqtb  {k}: {n} regions, first {at}");
    }
    eprintln!("phitex sanitize: untracked state changed by regions (not cells yet):");
    for (k, n) in &rep.sections {
        eprintln!("  {k}: {n} regions");
    }
}
