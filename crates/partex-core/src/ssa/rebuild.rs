//! A rebuild in place (DESIGN 7.17.3, "The rebuild, as built" and "The
//! forms these take"): the fold's steps run again where an edit's lines
//! or a changed definition reach them, each at its place, and nothing
//! else is visited.
//!
//! Nothing is kept per step but its record and its result. The engine's
//! arrays hold every slot's latest definition; a step that runs again
//! reads the definitions that reach it (its old run's reads predict
//! which, its new run's validate them), and it begins where the step
//! before it ended, that step's result mapped through the edits.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_ssa::Version;
use partex_ssa::fold::{Fold, StepId};

use super::{
    Close, Fam, Recorder, SVal, Slot, SsaReport, SsaTracker, StepEffects, TexSsa, Versions,
    close_paragraph, line_bounds, open_paragraph, set_value, slot_value,
};
use crate::host::{FileKind, Host, NoHost};
use crate::input::{AlphaFile, InStateRecord, InputValue, same_chain};
use crate::run::{CleanPoint, Step};
use crate::tex::Tex;
use crate::track::{Query, Tracker, Untracked};

/// How many old steps after the one a run tries to meet are looked at
/// for the place where it ended (item 4: a step that ended elsewhere).
const LOOK_AHEAD: usize = 64;

/// The fold's steps as a rebuild finds them.
#[derive(Default)]
pub(crate) struct Steps {
    /// The data each file was loaded with last, by load id: what a
    /// rebuild compares a file read whole with.
    files: Vec<Option<Arc<[u8]>>>,
    /// Every data loaded (7.17.3, "Lines are of a data"), and each one's
    /// index by its address (the data is kept, so an address names one).
    datas: Vec<Data>,
    ids: BTreeMap<usize, u32>,
    /// The lines the open step has read: the data, where each begins and
    /// where the next one does.
    pub(super) step_lines: Vec<(u32, usize, usize)>,
    /// The levels whose line number the open step read: the data each
    /// was reading (its address) and where it was at the step's start.
    step_numbers: BTreeSet<(usize, usize)>,
    /// Where each step left the input (its result), by step id, with the
    /// number of edits made before it was taken.
    inputs: Vec<Option<(usize, Arc<InputState>)>>,
    /// The sources' edits, in order.
    edits: Vec<Edit>,
    /// The format's definitions, made at the first rebuild that reads one.
    format: Option<Box<Tex<NoHost, Untracked>>>,
    /// The trace of the rebuild running (`RebuildReport::log`, kept here
    /// so a rebuild that panics leaves it: [`rebuild_log`]).
    pub(super) log: Vec<alloc::string::String>,
    /// The stores and the loads the open step's run made, in order
    /// (DESIGN 7.17.3, "A load reads the store, not the file"), and each
    /// step's, by step id.
    cur_stores: Vec<StoreEv>,
    cur_loads: Vec<LoadSeen>,
    stores: BTreeMap<StepId, Vec<StoreEv>>,
    loads: BTreeMap<StepId, Vec<LoadSeen>>,
    /// The names the job stores (the load ids of their addresses).
    stored: BTreeSet<u32>,
    /// Inside a rebuild: each stored name's φ, what the last trip stored
    /// (absent: it was not there).
    phi: Option<BTreeMap<u32, Option<Arc<[u8]>>>>,
    /// The queries of the host the open step's run asked, and each
    /// step's, by step id, with their answers' versions (7.17.3, "A
    /// query is asked again").
    cur_queries: Vec<(Query, u128)>,
    queries: BTreeMap<StepId, Vec<(Query, u128)>>,
    /// The open step's run's effects in the link's form, its chunks in
    /// program order (7.17.3, "Hits applied inside a step that runs
    /// again", item 2), and each step's, by step id, as its run ended
    /// ("The link after a rebuild is a watch's link").
    pub(super) cur_chunks: Vec<StepEffects>,
    pub(super) effects: Vec<Vec<StepEffects>>,
    /// The steps whose chunks changed since the link last took them (a
    /// run closed, or the step left the fold): what the link costs (DESIGN
    /// 4.3 item 4, [`super::take_step_changes`]).
    pub(super) fx_changed: Vec<StepId>,
}

/// A store a step made: a name opened (`\openout`, a whole definition)
/// or a line appended to it.
#[derive(Clone, PartialEq, Eq)]
enum StoreEv {
    Open(u32),
    Line(u32, Arc<[u8]>),
}

impl StoreEv {
    fn id(&self) -> u32 {
        match self {
            StoreEv::Open(i) | StoreEv::Line(i, _) => *i,
        }
    }
}

/// A load a step made: the name's load id, the version of what it found,
/// and whether it read the φ (no open of the name before it).
#[derive(Clone, Copy)]
struct LoadSeen {
    id: u32,
    ver: Version,
    phi: bool,
}

/// A data a load found (7.17.3, "Lines are of a data"): its name's load
/// id, whether it was the name's file (for a stored name, the φ) and not
/// a value served from the stores, and the lines the steps read of it, by
/// where they begin.
struct Data {
    name: u32,
    bytes: Arc<[u8]>,
    file: bool,
    lines: Vec<LineRead>,
    /// The steps that read a line number of a level reading this data,
    /// by where the level was at the step's start (7.17.3, "A line
    /// number read is kept by position"; `from` and `to` both that).
    numbers: Vec<LineRead>,
}

/// A line a step read: its bytes, and the step's run that read it.
#[derive(Clone, Copy)]
struct LineRead {
    from: usize,
    to: usize,
    step: StepId,
    run: u32,
}

impl Steps {
    /// Name `id` was loaded with `data`, its file's (`file`) or a value
    /// served from the stores; the data's index.
    pub(super) fn loaded(&mut self, id: u32, data: &Arc<[u8]>, file: bool) -> u32 {
        let i = id as usize;
        if self.files.len() <= i {
            self.files.resize(i + 1, None);
        }
        self.files[i] = Some(data.clone());
        let n = u32::try_from(self.datas.len()).unwrap_or(u32::MAX);
        let d = *self.ids.entry(data.as_ptr() as usize).or_insert(n);
        if d == n {
            self.datas.push(Data {
                name: id,
                bytes: data.clone(),
                file,
                lines: Vec::new(),
                numbers: Vec::new(),
            });
        }
        d
    }

    /// The open step read the line of `data` that begins at `from`.
    pub(super) fn line_read(&mut self, data: &[u8], from: usize) {
        let Some(&id) = self.ids.get(&(data.as_ptr() as usize)) else {
            return;
        };
        let to = line_bounds(data, from).1;
        if let Some(l) = self.step_lines.last_mut()
            && l.0 == id
            && l.2 == from
        {
            l.2 = to;
            return;
        }
        self.step_lines.push((id, from, to));
    }

    /// The open step read the line number of a level reading the data at
    /// `addr`, which was at `pos` when the step began.
    pub(super) fn number_read(&mut self, addr: usize, pos: usize) {
        self.step_numbers.insert((addr, pos));
    }

    /// A step begins a run: no stores, loads or queries yet.
    pub(super) fn run_begins(&mut self) {
        self.step_lines.clear();
        self.step_numbers.clear();
        self.cur_stores.clear();
        self.cur_loads.clear();
        self.cur_queries.clear();
        self.cur_chunks.clear();
    }

    /// The open step asked query `q`, answered as `answer` versions.
    pub(super) fn queried(&mut self, q: Query, answer: u128) {
        self.cur_queries.push((q, answer));
    }

    /// The open step opened name `id` for storing.
    pub(super) fn store_opened(&mut self, id: u32) {
        self.stored.insert(id);
        self.cur_stores.push(StoreEv::Open(id));
    }

    /// The open step appended `line` to name `id`.
    pub(super) fn store_line(&mut self, id: u32, line: &[u8]) {
        self.cur_stores.push(StoreEv::Line(id, Arc::from(line)));
    }

    /// Whether a load of name `id` by the step at `key` reads the φ: the
    /// name is not stored, or not opened before it.
    pub(super) fn reads_phi(&self, fold: &Fold<TexSsa>, id: u32, key: u64) -> bool {
        !matches!(self.value_at(fold, id, key), Some(Ok(_)))
    }

    /// The open step loaded name `id`, finding what `ver` versions, from
    /// the φ (`phi`) or the build's own stores.
    pub(super) fn load_seen(&mut self, id: u32, ver: Version, phi: bool) {
        self.cur_loads.push(LoadSeen { id, ver, phi });
    }

    /// Stored name `id`'s value where the open step, at `key`, is now:
    /// its lines since its last open before here (`Ok`), or the φ (`Err`)
    /// if no open is; `None` if the job does not store it.
    fn value_at(&self, fold: &Fold<TexSsa>, id: u32, key: u64) -> Option<Result<Vec<u8>, ()>> {
        // (`evs`' lines of `id` after its last open, latest first; whether
        // an open is there)
        fn back<'a>(evs: &'a [StoreEv], id: u32, lines: &mut Vec<&'a Arc<[u8]>>) -> bool {
            for e in evs.iter().rev() {
                match e {
                    StoreEv::Open(i) if *i == id => return true,
                    StoreEv::Line(i, l) if *i == id => lines.push(l),
                    _ => {}
                }
            }
            false
        }
        if !self.stored.contains(&id) {
            return None;
        }
        let mut lines: Vec<&Arc<[u8]>> = Vec::new();
        // (the open step's own, then the steps before it, latest first)
        let mut found = back(&self.cur_stores, id, &mut lines);
        if !found {
            let mut before: Vec<(u64, &Vec<StoreEv>)> = self
                .stores
                .iter()
                .filter_map(|(s, evs)| {
                    let k = fold.steps.get(*s as usize)?.key;
                    (k < key && evs.iter().any(|e| e.id() == id)).then_some((k, evs))
                })
                .collect();
            before.sort_unstable_by_key(|a| core::cmp::Reverse(a.0));
            for (_, evs) in before {
                if back(evs, id, &mut lines) {
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return Some(Err(()));
        }
        let mut v = Vec::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
        for l in lines.iter().rev() {
            v.extend_from_slice(l);
            v.push(b'\n');
        }
        Some(Ok(v))
    }

    /// What a load of stored name `id` at `key` is served inside a
    /// rebuild: the store's value there (`None`: read the host's file).
    #[expect(clippy::option_option, reason = "outer: not served; inner: no file")]
    pub(super) fn served(
        &self,
        fold: &Fold<TexSsa>,
        id: u32,
        key: u64,
    ) -> Option<Option<Arc<[u8]>>> {
        let phi = self.phi.as_ref()?;
        match self.value_at(fold, id, key)? {
            Ok(v) => Some(Some(Arc::from(v))),
            Err(()) => phi.get(&id).cloned(),
        }
    }

    /// Step `s`'s result, in the source as it is now.
    fn end(&self, s: StepId) -> Option<InputState> {
        let (g, e) = self.inputs.get(s as usize)?.as_ref()?;
        Some(if *g == self.edits.len() {
            (**e).clone()
        } else {
            e.mapped(&self.edits[*g..])
        })
    }
}

/// The trace's reader paths: the calls under `recs` whose own reads
/// include `a`, each as the routines from the step down to it.
fn who_read(
    rt: &partex_ssa::Runtime<TexSsa>,
    recs: &[partex_ssa::runtime::RecId],
    a: &Slot,
    path: &mut Vec<alloc::string::String>,
    out: &mut Vec<alloc::string::String>,
) {
    for &id in recs {
        if out.len() >= 6 {
            return;
        }
        let r = rt.record(id);
        path.push(alloc::format!("{}", r.func));
        if r.reads.iter().any(|(l, _)| l.addr() == a) {
            out.push(path.join("/"));
        }
        let kids: Vec<partex_ssa::runtime::RecId> = r.children().collect();
        who_read(rt, &kids, a, path, out);
        path.pop();
    }
}

/// Step `id` of the fold ended (its record is closed): the lines it read
/// and where it left the input, its result.
pub(super) fn step_closed(rr: &mut Recorder, id: StepId, input: InputState) -> Vec<u32> {
    let run = rr.rt.fold.steps[id as usize].run;
    let s = &mut rr.st.steps;
    // (its stores and loads replace its old run's; the names it stored to
    // differently, returned)
    let new = core::mem::take(&mut s.cur_stores);
    let old = if new.is_empty() {
        s.stores.remove(&id)
    } else {
        s.stores.insert(id, new.clone())
    }
    .unwrap_or_default();
    let mut changed: Vec<u32> = Vec::new();
    for e in old.iter().chain(new.iter()) {
        let i = e.id();
        if changed.contains(&i) {
            continue;
        }
        let a = old.iter().filter(|x| x.id() == i);
        let b = new.iter().filter(|x| x.id() == i);
        if !a.eq(b) {
            changed.push(i);
        }
    }
    let loads = core::mem::take(&mut s.cur_loads);
    if loads.is_empty() {
        s.loads.remove(&id);
    } else {
        s.loads.insert(id, loads);
    }
    let queries = core::mem::take(&mut s.cur_queries);
    if queries.is_empty() {
        s.queries.remove(&id);
    } else {
        s.queries.insert(id, queries);
    }
    for (f, from, to) in core::mem::take(&mut s.step_lines) {
        let Some(d) = s.datas.get_mut(f as usize) else {
            continue;
        };
        let v = &mut d.lines;
        let at = v.partition_point(|l| l.from <= from);
        v.insert(
            at,
            LineRead {
                from,
                to,
                step: id,
                run,
            },
        );
    }
    for (addr, pos) in core::mem::take(&mut s.step_numbers) {
        let Some(d) = s.ids.get(&addr).and_then(|&d| s.datas.get_mut(d as usize)) else {
            continue;
        };
        let v = &mut d.numbers;
        let at = v.partition_point(|l| l.from <= pos);
        v.insert(
            at,
            LineRead {
                from: pos,
                to: pos,
                step: id,
                run,
            },
        );
    }
    let i = id as usize;
    if s.inputs.len() <= i {
        s.inputs.resize(i + 1, None);
    }
    s.inputs[i] = Some((s.edits.len(), Arc::new(input)));
    // (its effects replace its old run's: the link's chunks)
    if s.effects.len() <= i {
        s.effects.resize(i + 1, Vec::new());
    }
    s.effects[i] = core::mem::take(&mut s.cur_chunks);
    s.fx_changed.push(id);
    changed
}

/// Where a step left the input: its result (DESIGN 3.5), the input's
/// value with the top file level's entries and the scalars.
#[derive(Clone)]
pub(crate) struct InputState {
    /// The job ended in the step.
    finished: bool,
    /// A fire is pending: the next step begins with it (DESIGN 3.15).
    fire: bool,
    line: i32,
    /// The shared parts: the levels, the parameters, the buffer below the
    /// top file level's line and the file levels below the top one.
    v: InputValue,
    /// The top file level's line, from its start to `first` or `last`.
    top: Vec<u8>,
    first: usize,
    last: usize,
    cur: InStateRecord,
    in_open: usize,
    /// The top file level's entries of the per-file arrays.
    file: FileTop,
    pseudo: Vec<crate::input::PseudoFile>,
}

/// The top file level's entries of the per-file arrays: its file and
/// position, the line number saved when it opened, e-TeX's depths and
/// `\everyeof` flag, and its names.
#[derive(Clone)]
struct FileTop {
    file: Option<AlphaFile>,
    line: i32,
    grp: i32,
    ifs: usize,
    eof: bool,
    name: i32,
    full_name: i32,
}

impl InputState {
    pub(crate) fn of<H: Host, T: Tracker>(t: &mut Tex<H, T>, finished: bool) -> Self {
        let v = t.input_value();
        let k = t.in_open;
        InputState {
            finished,
            fire: t.fire_pending,
            line: t.line,
            top: (v.from..t.first.max(t.last)).map(|i| t.buffer[i]).collect(),
            v,
            first: t.first,
            last: t.last,
            cur: t.cur_input.clone(),
            in_open: k,
            file: FileTop {
                file: t.input_file.get(k).cloned().flatten(),
                line: t.line_stack.get(k).copied().unwrap_or(0),
                grp: t.grp_stack.get(k).copied().unwrap_or(0),
                ifs: t.if_stack.get(k).copied().unwrap_or(0),
                eof: t.eof_seen.get(k).copied().unwrap_or(false),
                name: t.source_filename_stack.get(k).copied().unwrap_or(0),
                full_name: t.full_source_filename_stack.get(k).copied().unwrap_or(0),
            },
            pseudo: t.pseudo_files.clone(),
        }
    }

    /// Each open file level's file, bottom up.
    fn files(&self) -> impl Iterator<Item = Option<&AlphaFile>> {
        let lower = self.v.files.files.iter().map(Option::as_ref);
        lower.chain([self.file.file.as_ref()])
    }

    /// Put the input where this state has it.
    fn set<H: Host, T: Tracker>(&self, t: &mut Tex<H, T>) {
        t.fire_pending = self.fire;
        t.line = self.line;
        t.cur_input = self.cur.clone();
        t.in_open = self.in_open;
        t.set_input_value(&self.v);
        for (i, &b) in self.top.iter().enumerate() {
            t.buffer[self.v.from + i] = b;
        }
        t.first = self.first;
        t.last = self.last;
        let k = self.in_open;
        let f = &self.file;
        t.input_file[k].clone_from(&f.file);
        t.line_stack[k] = f.line;
        t.grp_stack[k] = f.grp;
        t.if_stack[k] = f.ifs;
        t.eof_seen[k] = f.eof;
        t.source_filename_stack[k] = f.name;
        t.full_source_filename_stack[k] = f.full_name;
        t.pseudo_files.clone_from(&self.pseudo);
    }

    /// This state in the source after `edits` (DESIGN 3.5: a position
    /// after an edit moves by its length, a line number by its lines).
    fn mapped(&self, edits: &[Edit]) -> InputState {
        let mut s = self.clone();
        for e in edits {
            for j in 0..=s.in_open {
                let at = if j == s.in_open {
                    s.file.file.as_mut()
                } else {
                    Arc::make_mut(&mut s.v.files).files[j].as_mut()
                };
                let Some(f) = at else {
                    continue;
                };
                // (each edit applies to the data it replaced)
                if !Arc::ptr_eq(&f.data, &e.old) {
                    continue;
                }
                let d = e.map_file(f);
                // (the line of level j is the line number saved when the
                // level above it opened)
                if j == s.in_open {
                    s.line += d;
                } else if j + 1 == s.in_open {
                    s.file.line += d;
                } else if let Some(l) = Arc::make_mut(&mut s.v.files).lines.get_mut(j + 1) {
                    *l += d;
                }
            }
        }
        s
    }
}

/// Whether strings `a` and `b` of `t`'s pool have the same characters
/// (a number is not observable: DESIGN 3.2, "Allocated numbers are not
/// positioned").
fn same_str<H: Host, T: Tracker>(t: &Tex<H, T>, a: i32, b: i32) -> bool {
    if a == b {
        return true;
    }
    match (usize::try_from(a), usize::try_from(b)) {
        (Ok(x), Ok(y)) if x < t.str_ptr && y < t.str_ptr => t.str_bytes(x) == t.str_bytes(y),
        _ => false,
    }
}

fn same_data(a: &Arc<[u8]>, b: &Arc<[u8]>) -> bool {
    Arc::ptr_eq(a, b) || a[..] == b[..]
}

/// Whether two files are at the same place in the same data.
fn same_file(x: Option<&AlphaFile>, y: Option<&AlphaFile>) -> bool {
    match (x, y) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.pos == y.pos
                && x.line_from == y.line_from
                && x.line_open == y.line_open
                && x.lines == y.lines
                && x.name == y.name
                && same_data(&x.data, &y.data)
        }
        _ => false,
    }
}

/// Whether two results leave the input at the same place (DESIGN 3.15,
/// step 5): the same files at the same positions and lines, the same
/// levels.
fn same_place<H: Host, T: Tracker>(t: &Tex<H, T>, a: &InputState, b: &InputState) -> bool {
    let rec = |x: &InStateRecord, y: &InStateRecord| {
        x.state == y.state
            && x.index == y.index
            && x.start == y.start
            && x.loc == y.loc
            && x.limit == y.limit
            && same_str(t, x.name, y.name)
    };
    let (fa, fb) = (&a.v.files, &b.v.files);
    let lower = Arc::ptr_eq(fa, fb)
        || (fa.lines == fb.lines
            && fa.files.len() == fb.files.len()
            && fa
                .files
                .iter()
                .zip(&fb.files)
                .all(|(x, y)| same_file(x.as_ref(), y.as_ref())));
    a.finished == b.finished
        && a.fire == b.fire
        && a.in_open == b.in_open
        && a.line == b.line
        && a.file.line == b.file.line
        && a.first == b.first
        && a.last == b.last
        && a.v.depth == b.v.depth
        && rec(&a.cur, &b.cur)
        && same_chain(a.v.levels.as_ref(), b.v.levels.as_ref(), rec)
        && same_file(a.file.file.as_ref(), b.file.file.as_ref())
        && lower
}

/// Whether two results at the same place are the same input: the lines
/// in the buffer, the token lists and parameters waiting, the rest.
fn same_input<H: Host, T: Tracker>(t: &Tex<H, T>, a: &InputState, b: &InputState) -> bool {
    let strs = |x: &[i32], y: &[i32]| {
        x.len() == y.len() && x.iter().zip(y).all(|(&p, &q)| same_str(t, p, q))
    };
    let (fa, fb) = (&a.v.files, &b.v.files);
    let lower = Arc::ptr_eq(fa, fb)
        || (fa.grp == fb.grp
            && fa.ifs == fb.ifs
            && fa.eof == fb.eof
            && strs(&fa.names, &fb.names)
            && strs(&fa.full_names, &fb.full_names));
    let (x, y) = (&a.file, &b.file);
    a.v.from == b.v.from
        && same_data(&a.v.below, &b.v.below)
        && a.top == b.top
        && a.cur.list == b.cur.list
        && same_chain(a.v.levels.as_ref(), b.v.levels.as_ref(), |x, y| {
            x.list == y.list
        })
        && a.v.np == b.v.np
        && same_chain(a.v.params.as_ref(), b.v.params.as_ref(), |x, y| x == y)
        && a.pseudo == b.pseudo
        && x.grp == y.grp
        && x.ifs == y.ifs
        && x.eof == y.eof
        && same_str(t, x.name, y.name)
        && same_str(t, x.full_name, y.full_name)
        && lower
}

/// An edit of a data (7.17.3, "an edit is of a data"): runs of changed
/// lines, found by lines ("That holds because an edit is found by
/// lines").
pub(crate) struct Edit {
    /// The index of the data it replaced, that data and the data now.
    data: u32,
    old: Arc<[u8]>,
    new: Arc<[u8]>,
    /// The runs of changed lines, in order.
    hunks: Vec<Hunk>,
}

/// A run of changed lines: the old bytes `of..ot` became the new bytes
/// `nf..nt`, which end `lines` more lines.
#[derive(Clone, Copy, Debug)]
struct Hunk {
    of: usize,
    ot: usize,
    nf: usize,
    nt: usize,
    lines: i32,
}

/// The most lines a diff may change before an edit is one hunk.
const MAX_DIFF: usize = 1000;

/// Whether `x` begins a line of `b` (TeX's line ends, §31: LF, CR, CRLF),
/// or is its end.
fn line_start(b: &[u8], x: usize) -> bool {
    x == 0 || x >= b.len() || b[x - 1] == b'\n' || (b[x - 1] == b'\r' && b[x] != b'\n')
}

/// The lines of `b[from..to]`: where each begins and where the next does.
fn line_spans(b: &[u8], from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut a = from;
    for x in from + 1..=to {
        if x == to || line_start(b, x) {
            if x > a {
                v.push((a, x));
            }
            a = x;
        }
    }
    v
}

/// The line ends in `b`.
fn line_ends(b: &[u8]) -> i32 {
    let n = b
        .iter()
        .enumerate()
        .filter(|&(i, &c)| c == b'\n' || (c == b'\r' && b.get(i + 1) != Some(&b'\n')))
        .count();
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Myers' greedy O(ND) diff of `a` and `b`: the pairs of equal elements it
/// keeps, in order; `None` past `max` differences.
fn myers<T: PartialEq>(a: &[T], b: &[T], max: usize) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len(), b.len());
    let max = max.min(n + m);
    let off = max + 1;
    // (`v[off + k]`: the furthest x on diagonal k; `trace[d]`, the
    // diagonals -d..=d after d differences)
    let mut v = alloc::vec![0usize; 2 * max + 3];
    let mut trace: Vec<Vec<usize>> = Vec::new();
    let mut last = None;
    for d in 0..=max {
        let di = d.cast_signed();
        for k in (-di..=di).step_by(2) {
            let at = |k: isize| off.wrapping_add_signed(k);
            let mut x = if k == -di || (k != di && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x.wrapping_add_signed(-k);
            while x < n && y < m && a[x] == b[y] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x == n && y == m {
                last = Some(d);
                break;
            }
        }
        trace.push(v[off - d..=off + d].to_vec());
        if last.is_some() {
            break;
        }
    }
    let dd = last?;
    // (back from the end: each difference's snake, its diagonal kept)
    let mut pairs = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=dd).rev() {
        let di = d.cast_signed();
        let k = x.cast_signed() - y.cast_signed();
        let p = &trace[d - 1];
        let get = |k: isize| p.get(usize::try_from(k + di - 1).ok()?).copied();
        let pk = if k == -di || (k != di && get(k - 1)? < get(k + 1)?) {
            k + 1
        } else {
            k - 1
        };
        let px = get(pk)?;
        let py = px.wrapping_add_signed(-pk);
        let (mx, my) = if pk == k + 1 {
            (px, py + 1)
        } else {
            (px + 1, py)
        };
        while x > mx && y > my {
            x -= 1;
            y -= 1;
            pairs.push((x, y));
        }
        (x, y) = (px, py);
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        pairs.push((x, y));
    }
    pairs.reverse();
    Some(pairs)
}

impl Edit {
    fn diff(data: u32, old: &Arc<[u8]>, new: &Arc<[u8]>) -> Option<Edit> {
        if old[..] == new[..] {
            return None;
        }
        // (the common prefix and suffix, cut where a line begins in both)
        let mut p = old
            .iter()
            .zip(new.iter())
            .take_while(|(a, b)| a == b)
            .count();
        let room = old.len().min(new.len()) - p;
        let s = old[p..]
            .iter()
            .rev()
            .zip(new[p..].iter().rev())
            .take(room)
            .take_while(|(a, b)| a == b)
            .count();
        while p > 0 && !(line_start(old, p) && line_start(new, p)) {
            p -= 1;
        }
        let (mut oe, mut ne) = (old.len() - s, new.len() - s);
        while oe < old.len() && !(line_start(old, oe) && line_start(new, ne)) {
            oe += 1;
            ne += 1;
        }
        let a = line_spans(old, p, oe);
        let b = line_spans(new, p, ne);
        let la: Vec<&[u8]> = a.iter().map(|&(x, y)| &old[x..y]).collect();
        let lb: Vec<&[u8]> = b.iter().map(|&(x, y)| &new[x..y]).collect();
        let at = |v: &[(usize, usize)], i: usize, end: usize| v.get(i).map_or(end, |l| l.0);
        let mut hunks = Vec::new();
        let mut one = |i0: usize, i1: usize, j0: usize, j1: usize| {
            if i0 == i1 && j0 == j1 {
                return;
            }
            let (of, ot) = (at(&a, i0, oe), at(&a, i1, oe));
            let (nf, nt) = (at(&b, j0, ne), at(&b, j1, ne));
            hunks.push(Hunk {
                of,
                ot,
                nf,
                nt,
                lines: line_ends(&new[nf..nt]) - line_ends(&old[of..ot]),
            });
        };
        // (a run of changed lines: its old and new lines paired in order,
        // a hunk each, and the lines left over one more)
        let mut hunk = |i0: usize, i1: usize, j0: usize, j1: usize| {
            let c = (i1 - i0).min(j1 - j0);
            for t in 0..c {
                one(i0 + t, i0 + t + 1, j0 + t, j0 + t + 1);
            }
            one(i0 + c, i1, j0 + c, j1);
        };
        match myers(&la, &lb, MAX_DIFF) {
            Some(pairs) => {
                let (mut i, mut j) = (0, 0);
                for (x, y) in pairs {
                    hunk(i, x, j, y);
                    (i, j) = (x + 1, y + 1);
                }
                hunk(i, la.len(), j, lb.len());
            }
            None => hunk(0, la.len(), 0, lb.len()),
        }
        Some(Edit {
            data,
            old: old.clone(),
            new: new.clone(),
            hunks,
        })
    }

    /// How many hunks old position `x` is after: the text there follows
    /// their new bytes (a position where bytes were inserted stays before
    /// them, which is where the text after it begins).
    fn after(&self, x: usize) -> usize {
        self.hunks
            .partition_point(|h| x > h.ot || (x == h.ot && h.ot > h.of))
    }

    /// Old position `x` in the new data.
    fn pos(&self, x: usize) -> usize {
        match self.after(x) {
            0 => x,
            n => {
                let h = &self.hunks[n - 1];
                x - h.ot + h.nt
            }
        }
    }

    /// Whether a hunk touches the old line `from..to` (or is inserted
    /// where it begins).
    fn touches(&self, from: usize, to: usize) -> bool {
        let i = self.hunks.partition_point(|h| h.ot < from);
        self.hunks.get(i).is_some_and(|h| to > h.of)
    }

    /// File `f`, reading the data this edit replaced, in the new data
    /// (7.17.3, "The input is the step's result"): its positions moved,
    /// its line count by the lines of the hunks before it, returned.
    /// The lines the hunks before old position `x` added.
    fn lines_before(&self, x: usize) -> i32 {
        self.hunks[..self.after(x)].iter().map(|h| h.lines).sum()
    }

    fn map_file(&self, f: &mut AlphaFile) -> i32 {
        let d = self.lines_before(f.pos);
        f.data = self.new.clone();
        f.line_from = self.pos(f.line_from);
        f.pos = self.pos(f.pos);
        f.lines = f.lines.saturating_add_signed(d);
        d
    }
}

/// What a rebuild did: the counts AGENTS.md asks each report for.
#[derive(Clone, Debug, Default)]
pub struct RebuildReport {
    pub history: i32,
    /// The files that changed, and the steps their lines made dirty.
    pub edits: usize,
    pub seeds: usize,
    /// The loads that read a φ that changed (7.17.3, "A load reads the
    /// store"), and the loads a step's changed stores made dirty.
    pub phi: usize,
    pub store_readers: usize,
    /// The steps a query answered anew made dirty (7.17.3, "A query is
    /// asked again").
    pub queries: usize,
    /// The edits made inside the rebuild, of a data a load found anew
    /// (7.17.3, "Lines are of a data").
    pub data_edits: usize,
    /// Steps run (again, or new: `new_steps`), runs dropped for a read
    /// not predicted, old steps passed over.
    pub steps_run: usize,
    pub retries: usize,
    pub new_steps: usize,
    pub removed: usize,
    /// Definitions whose value changed, the readers they made dirty, the
    /// outside reads validated, the slots set to the definition reaching
    /// a step and back to their latest, the values taken from the
    /// format's definitions.
    pub defs_changed: usize,
    pub readers_marked: usize,
    pub reads_checked: usize,
    pub positioned: usize,
    pub restored: usize,
    pub initial: usize,
    /// Hits applied in the steps run (7.17.3, "Hits applied inside a step
    /// that runs again"), and the commands they stand for.
    pub applied: u64,
    pub skipped: u64,
    /// Why the rebuild stopped short, if it did.
    pub unsupported: Option<&'static str>,
    pub commands: u64,
    /// With `trace`: a line per step run, what it changed and where it
    /// ended.
    pub trace: bool,
    pub log: Vec<alloc::string::String>,
}

impl InputState {
    /// A short view: the level, the line, where the top file is, the
    /// offset in the buffer's line.
    fn brief(&self) -> alloc::string::String {
        let pos = self.file.file.as_ref().map_or(0, |f| f.pos);
        alloc::format!(
            "level {} line {} pos {} loc {} state {}{}{}",
            self.in_open,
            self.line,
            pos,
            self.cur.loc - self.cur.start,
            self.cur.state,
            if self.finished { " (finished)" } else { "" },
            if self.fire { " (a fire pending)" } else { "" }
        )
    }
}

/// Whether slot `a` is positioned (7.17.3, "Allocated numbers are not
/// positioned"): not a number the build hands out (the pool's strings
/// and end, the hash's slots and allocators, the glues' lineage), not a
/// read of what is not state (the source, lookups, loads, searches), and
/// not a family whose values are not built yet (the fonts, hyphenation).
fn positioned(a: &Slot) -> bool {
    use crate::track::scalar::{GLUE_LINEAGE, HASH_HIGH, HASH_USED, STR_TOP};
    match a.0 {
        Fam::Eqtb
        | Fam::List
        | Fam::Save
        | Fam::Cond
        | Fam::Mark
        | Fam::Page
        | Fam::Pdf
        | Fam::Dvi
        | Fam::Out
        | Fam::Read
        | Fam::Random
        | Fam::Glyphs
        | Fam::PageNode => true,
        Fam::Alloc => ![STR_TOP, HASH_USED, HASH_HIGH, GLUE_LINEAGE]
            .iter()
            .any(|&k| a.1 == i64::from(k)),
        _ => false,
    }
}

/// Whether a definition at or after `key` defines `a`: the arrays then
/// hold a value the step at `key` must not read.
fn later(fold: &Fold<TexSsa>, a: &Slot, key: u64) -> bool {
    // (a page node always: the engine holds the list, not the nodes
    // past its length, 7.17.3 item 5)
    a.0 == Fam::PageNode || fold.latest(a).is_some_and(|d| d.key >= key)
}

/// The save stack's slots placed whole for the step at `key` (DESIGN
/// 7.17.3, "The save stack is placed whole below its pointer"): its
/// pointer, level, group, boundary and e-TeX's chain, and each entry
/// below the pointer that reaches the step, where a later definition
/// holds the arrays. Added to `next`, the slots the step's run is placed
/// at.
fn save_stack_whole<H: Host>(
    tex: &Tex<H, SsaTracker>,
    key: u64,
    next: &mut Vec<Slot>,
    rep: &mut RebuildReport,
) {
    use crate::track::save::{CUR_BOUNDARY, CUR_GROUP, CUR_LEVEL, ENTRY, SAVE_PTR, XCHAIN};
    let mut r = tex.tracker.rec.borrow_mut();
    let rr = &mut *r;
    let slot = |k: u32| Slot(Fam::Save, i64::from(k));
    let ptr = slot(SAVE_PTR);
    let at = if later(&rr.rt.fold, &ptr, key) {
        match reaching(tex, rr, ptr, key, rep) {
            Some(SVal(_, Some(v))) => match *v {
                super::SValue::Int(x) => x,
                _ => 0,
            },
            _ => 0,
        }
    } else {
        tex.save_ptr
    };
    let fold = &rr.rt.fold;
    for k in [SAVE_PTR, CUR_LEVEL, CUR_GROUP, CUR_BOUNDARY, XCHAIN]
        .into_iter()
        .chain((0..u32::try_from(at).unwrap_or(0)).map(|p| ENTRY + p))
    {
        let a = slot(k);
        if later(fold, &a, key) {
            next.push(a);
        }
    }
}

/// A step's definitions: each slot its records wrote, at its last write.
fn defs(
    rt: &partex_ssa::Runtime<TexSsa>,
    recs: &[partex_ssa::runtime::RecId],
) -> BTreeMap<Slot, Version> {
    let mut d = BTreeMap::new();
    for &r in recs {
        for (a, v) in &rt.record(r).writes {
            d.insert(*a, v.as_ref().map_or(Version::ABSENT, |v| v.0));
        }
    }
    d
}

/// The value of `a` that reaches `key`: the definition before it, or the
/// format's.
fn reaching<H: Host>(
    tex: &Tex<H, SsaTracker>,
    rr: &mut Recorder,
    a: Slot,
    key: u64,
    rep: &mut RebuildReport,
) -> Option<SVal> {
    if let Some(d) = rr.rt.fold.reaching(&a, key) {
        rr.rt.record(d.rec).writes.get(d.ix as usize)?.1.clone()
    } else {
        rep.initial += 1;
        initial(tex, &mut rr.st.steps, a)
    }
}

/// The latest value of `a` (7.17.3: after a step, the arrays hold the
/// latest definitions again), unless step `j`'s run left it.
fn latest<H: Host>(
    tex: &Tex<H, SsaTracker>,
    rr: &mut Recorder,
    a: Slot,
    j: StepId,
    rep: &mut RebuildReport,
) -> Option<SVal> {
    match rr.rt.fold.latest(&a) {
        Some(d) if d.step == j => None,
        Some(d) => rr.rt.record(d.rec).writes.get(d.ix as usize)?.1.clone(),
        None => {
            rep.initial += 1;
            initial(tex, &mut rr.st.steps, a)
        }
    }
}

/// What slot `a` holds before any step defines it: the format's
/// definition (DESIGN 7.17.3, "Not built": the first step's definitions
/// are the format's).
fn initial<H: Host>(tex: &Tex<H, SsaTracker>, steps: &mut Steps, a: Slot) -> Option<SVal> {
    if steps.format.is_none() {
        let f = Tex::format_value(tex.params.clone(), tex.format_data.as_deref())?;
        steps.format = Some(Box::new(f));
    }
    let f = steps.format.as_deref()?;
    let v = slot_value(f, a)?;
    // (its version is made by content once it is in place, where the
    // family's versions are: `put`)
    Some(SVal::held(Version::of(&(0xf0u8, a)), v))
}

/// Store `vals` in the engine's arrays (each one field, the value shared),
/// an eqtb entry from the format versioned by its content.
fn put<H: Host>(tex: &mut Tex<H, SsaTracker>, vals: &[(Slot, SVal)]) {
    if vals.is_empty() {
        return;
    }
    let mut vers: Versions = core::mem::take(&mut tex.tracker.rec.borrow_mut().st.vers);
    // (the page's length, then its tail, before its nodes: the list is
    // made that long with stand-ins, and the nodes read put in place,
    // 7.17.3 item 5)
    let rank = |a: &Slot| match (a.0, u8::try_from(a.1)) {
        (Fam::Page, Ok(crate::track::page::LIST_LEN)) => 0,
        (Fam::Page, Ok(crate::track::page::LIST_TAIL)) => 1,
        (Fam::PageNode, _) => 3,
        _ => 2,
    };
    let mut order: Vec<&(Slot, SVal)> = vals.iter().collect();
    order.sort_by_key(|(a, _)| rank(a));
    for (a, v) in order {
        set_value(tex, &mut vers, *a, v);
        if a.0 == Fam::Read {
            // (a stream's file in the source as it is now: the edits
            // made after the value, each to the data it replaced)
            let r = tex.tracker.rec.borrow();
            let f = usize::try_from(a.1)
                .ok()
                .and_then(|n| tex.read_file.get_mut(n))
                .and_then(Option::as_mut);
            if let Some(f) = f {
                for e in &r.st.steps.edits {
                    if Arc::ptr_eq(&f.data, &e.old) {
                        e.map_file(f);
                    }
                }
            }
        }
        if a.0 == Fam::Eqtb && v.0 == Version::of(&(0xf0u8, *a)) {
            let p = i32::try_from(a.1).unwrap_or(0);
            vers.set(*a, tex.cell_content(crate::track::Cell::Eqtb(p)));
        }
    }
    tex.tracker.rec.borrow_mut().st.vers = vers;
}

/// Rebuild the job in place after its sources changed (DESIGN 7.17.3):
/// the steps that read an edited line run again, and those whose reads a
/// changed definition reaches, in program order; the files are linked
/// from the steps' records afterwards (`effects::link`).
pub fn rebuild<H: Host>(tex: &mut Tex<H, SsaTracker>, trace: bool, apply: bool) -> RebuildReport {
    let mut rep = RebuildReport {
        trace,
        ..RebuildReport::default()
    };
    let c0 = tex.commands();
    let mut dirty: BTreeMap<u64, StepId> = BTreeMap::new();
    {
        // the files loaded, as they are now: a file read by lines changed
        // where its lines did, in each data it was loaded as (7.17.3,
        // "Lines are of a data"); a load reads whether the file is there,
        // and a file read whole reads its contents (each looked up as the
        // kind of file it was loaded as)
        #[allow(clippy::type_complexity)]
        let (files, datas): (
            Vec<(u32, Vec<u8>, FileKind, Option<Arc<[u8]>>, bool, bool)>,
            Vec<(u32, u32, Arc<[u8]>)>,
        ) = {
            let r = tex.tracker.rec.borrow();
            let s = &r.st.steps;
            let lines: BTreeSet<u32> = s
                .datas
                .iter()
                .filter(|d| !d.lines.is_empty())
                .map(|d| d.name)
                .collect();
            let files =
                r.st.loads
                    .iter()
                    .enumerate()
                    .filter_map(|(i, (name, _, kind))| {
                        let id = u32::try_from(i).ok()?;
                        let stored = s.stored.contains(&id);
                        let old = s.files.get(i).cloned().flatten();
                        Some((id, name.clone(), *kind, old, lines.contains(&id), stored))
                    })
                    .collect();
            // (the data found in the files, not served from the stores)
            let datas = s
                .datas
                .iter()
                .enumerate()
                .filter(|(_, d)| d.file && lines.contains(&d.name))
                .filter_map(|(i, d)| Some((u32::try_from(i).ok()?, d.name, d.bytes.clone())))
                .collect();
            (files, datas)
        };
        let mut edits = Vec::new();
        let mut loads = Vec::new();
        // (a name the job stores: its φ, what the last trip stored, which
        // its file holds, 7.17.3's "A load reads the store")
        let mut phi: BTreeMap<u32, Option<Arc<[u8]>>> = BTreeMap::new();
        // (each file read by lines, as it is now)
        let mut nows: BTreeMap<u32, Arc<[u8]>> = BTreeMap::new();
        // (a load the host knows is as it was is not made again: 7.17.3,
        // "A rebuild's file checks cost the files that changed")
        let checks = {
            let loads: Vec<_> = files
                .iter()
                .map(|(_, name, kind, old, ..)| (&name[..], *kind, old.as_ref()))
                .collect();
            tex.host.unchanged(&loads)
        };
        let (mut same, mut read) = (0, 0);
        for ((id, name, kind, old, lines, stored), unchanged) in files.into_iter().zip(checks) {
            let now = if unchanged {
                same += 1;
                old.clone()
            } else {
                read += 1;
                tex.host.read_file(&name, kind).map(|f| f.contents)
            };
            match (&old, &now) {
                _ if unchanged => {}
                (Some(_), Some(now)) if lines => {
                    nows.insert(id, now.clone());
                }
                (Some(old), Some(now)) if old[..] == now[..] => {}
                (None, None) => {}
                _ if stored => {}
                _ => loads.push((id, now.clone())),
            }
            if stored {
                phi.insert(id, now);
            }
        }
        if trace {
            note(
                tex,
                alloc::format!("loads: {same} as they were by their stamps, {read} made again"),
            );
        }
        for (d, name, old) in datas {
            if let Some(e) = nows.get(&name).and_then(|now| Edit::diff(d, &old, now)) {
                edits.push(e);
            }
        }
        // the queries the steps asked, asked again: a step whose answer
        // differs is dirty (one answer per query, for the whole trip)
        let asked: Vec<(StepId, u64, Query, u128)> = {
            let r = tex.tracker.rec.borrow();
            let fold = &r.rt.fold;
            r.st.steps
                .queries
                .iter()
                .flat_map(|(&s, qs)| {
                    let key = fold.steps[s as usize].key;
                    qs.iter().map(move |(q, a)| (s, key, q.clone(), *a))
                })
                .collect()
        };
        let mut answers: BTreeMap<Query, Option<u128>> = BTreeMap::new();
        for (s, key, q, a) in asked {
            let now = *answers.entry(q.clone()).or_insert_with(|| match &q {
                Query::ModDate(name) => {
                    Some(Version::of(&tex.host.file_mod_date(name).unwrap_or_default()).0)
                }
                Query::Now => Some(tex.clock_answer()),
                Query::Timer => Some(Version::of(&tex.host.seconds_and_micros()).0),
                Query::TimerStart => Some(a),
                Query::Terminal => None,
            });
            if now != Some(a) && dirty.insert(key, s).is_none() {
                rep.queries += 1;
                if trace {
                    note(tex, alloc::format!("seed: step {s} asked {q:?}"));
                }
            }
        }
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        // the loads that read the φ, dirty if it is not what they found
        for (&s, seen) in &rr.st.steps.loads {
            for l in seen.iter().filter(|l| l.phi) {
                let Some(now) = phi.get(&l.id) else { continue };
                if now
                    .as_ref()
                    .map_or(Version::ABSENT, |c| Version::of(&c[..]))
                    != l.ver
                {
                    rep.phi += 1;
                    dirty.insert(rr.rt.fold.steps[s as usize].key, s);
                    if trace {
                        let n = alloc::format!("seed: step {s} loaded φ {}", l.id);
                        rr.st.steps.log.push(n);
                    }
                }
            }
        }
        rep.edits = edits.len() + loads.len();
        if rep.edits == 0 && dirty.is_empty() {
            return rep;
        }
        rr.st.steps.phi = Some(phi);
        for (id, now) in loads {
            let a = Slot(Fam::Load, i64::from(id));
            for x in rr.rt.fold.readers_between(&a, 0, None) {
                dirty.insert(rr.rt.fold.steps[x as usize].key, x);
                if trace {
                    let n = alloc::format!("seed: step {x} loaded {id}");
                    rr.st.steps.log.push(n);
                }
            }
            if let Some(l) = rr.st.loads.get_mut(id as usize) {
                l.1 = now
                    .as_ref()
                    .map_or(Version::ABSENT, |c| Version::of(&c[..]));
            }
        }
        for e in edits {
            let name = rr.st.steps.datas[e.data as usize].name;
            let n = rr.st.steps.loaded(name, &e.new, true);
            seed(rr, &e, n, &mut dirty, trace);
            if let Some(l) = rr.st.loads.get_mut(name as usize) {
                l.1 = Version::of(&e.new[..]);
            }
            rr.st.steps.edits.push(e);
        }
        rep.seeds = dirty.len();
        if trace {
            let l = alloc::format!(
                "seeds (step, key): {:?}; edits {:?}",
                dirty.iter().map(|(k, s)| (*s, *k)).collect::<Vec<_>>(),
                rr.st.steps.edits[rr.st.steps.edits.len().saturating_sub(rep.edits)..]
                    .iter()
                    .map(|e| (e.data, &e.hunks[..e.hunks.len().min(4)]))
                    .collect::<Vec<_>>()
            );
            rr.st.steps.log.push(l);
        }
        rr.st.vers.epoch += 1;
        rr.rt.open_trip(1);
        rr.on = true;
    }
    tex.tracker.check = false;
    tex.tracker.apply.set(apply);
    let applied = (tex.tracker.applied.get(), tex.tracker.skipped.get());
    tex.tracker.boundary();
    let mut srep = SsaReport::default();
    while let Some((_, j)) = dirty.pop_first() {
        let (prev, mut target) = {
            let r = tex.tracker.rec.borrow();
            let fold = &r.rt.fold;
            if !fold.steps[j as usize].live {
                continue;
            }
            let Some(pos) = fold.position(j) else {
                continue;
            };
            if pos == 0 {
                rep.unsupported = Some("an edit in the job's first step (its start)");
                break;
            }
            (fold.order[pos - 1], r.st.steps.end(j))
        };
        let Some(mut input) = tex.tracker.rec.borrow().st.steps.end(prev) else {
            rep.unsupported = Some("a step with no result");
            break;
        };
        // (the step whose old end the runs try to meet, and that end)
        let cursor = j;
        let mut cur = j;
        let mut predict = j;
        loop {
            let c0 = tex.commands();
            let end = run_step(tex, cur, predict, &input, &mut dirty, &mut rep, &mut srep);
            if let Some(t) = target.take() {
                target = Some(data_edits(tex, &end, t, &mut dirty, &mut rep));
            }
            if rep.trace {
                let t = target
                    .as_ref()
                    .map_or(alloc::string::String::from("-"), InputState::brief);
                note(
                    tex,
                    alloc::format!(
                        "  from {}\n  ended {}\n  old end {t}",
                        input.brief(),
                        end.brief()
                    ),
                );
            }
            if let Some(t) = &target
                && same_place(tex, &end, t)
            {
                if !same_input(tex, &end, t) {
                    mark_next(tex, cur, &mut dirty);
                }
                break;
            }
            // (it ended elsewhere: at the end of an old step after it, which
            // passes over the steps between, or it runs on)
            if let Some((m, old_end)) = meet(tex, cur, &end) {
                let passed = {
                    let r = tex.tracker.rec.borrow();
                    let fold = &r.rt.fold;
                    let from = fold.position(cur).unwrap_or(0) + 1;
                    let to = fold.position(m).unwrap_or(0);
                    fold.order[from..=to].to_vec()
                };
                for s in passed {
                    retire(tex, s, &mut dirty, &mut rep);
                }
                if !same_input(tex, &end, &old_end) {
                    mark_next(tex, cur, &mut dirty);
                }
                break;
            }
            // (a place can come again: a loop's fires, inside its token
            // lists, DESIGN §7.16.1; a step that ran commands or began
            // with a fire went on)
            if tex.commands() == c0
                && !input.fire
                && same_place(tex, &end, &input)
                && same_input(tex, &end, &input)
            {
                rep.unsupported = Some("a step that read nothing");
                break;
            }
            if end.finished {
                // (the job ended here: the old steps after it are gone)
                let rest = {
                    let r = tex.tracker.rec.borrow();
                    let fold = &r.rt.fold;
                    let from = fold.position(cur).unwrap_or(0) + 1;
                    fold.order[from..].to_vec()
                };
                for s in rest {
                    retire(tex, s, &mut dirty, &mut rep);
                }
                break;
            }
            let next = {
                let mut r = tex.tracker.rec.borrow_mut();
                let renumbered = r.rt.fold.renumbered;
                let Some(n) = r.rt.fold.insert_after(cur) else {
                    rep.unsupported = Some("a step not in the fold");
                    break;
                };
                let fold = &r.rt.fold;
                if fold.renumbered != renumbered {
                    // (the keys were made again: the dirty steps' too)
                    dirty = dirty
                        .into_values()
                        .map(|s| (fold.steps[s as usize].key, s))
                        .collect();
                }
                // (a new step's reads are predicted by the old step whose
                // text it runs)
                predict = fold
                    .position(n)
                    .and_then(|p| fold.order.get(p + 1).copied())
                    .unwrap_or(cursor);
                n
            };
            rep.new_steps += 1;
            cur = next;
            input = end;
        }
        if rep.unsupported.is_some() {
            break;
        }
    }
    tex.tracker.flush_effects();
    {
        let mut r = tex.tracker.rec.borrow_mut();
        r.flush_output();
        r.on = false;
        r.rt.close_trip();
    }
    rep.commands = tex.commands() - c0;
    rep.applied = tex.tracker.applied.get() - applied.0;
    rep.skipped = tex.tracker.skipped.get() - applied.1;
    tex.tracker.apply.set(false);
    // (the arrays hold the latest definitions: `history` is the job's)
    rep.history = tex.history;
    rep.log = rebuild_log(tex);
    tex.tracker.rec.borrow_mut().st.steps.phi = None;
    rep
}

/// A line of the rebuild's trace.
fn note<H: Host>(tex: &Tex<H, SsaTracker>, line: alloc::string::String) {
    tex.tracker.rec.borrow_mut().st.steps.log.push(line);
}

/// The trace of the last rebuild, taken (after a panic in one, what it
/// traced so far).
pub fn rebuild_log<H: Host>(tex: &Tex<H, SsaTracker>) -> Vec<alloc::string::String> {
    core::mem::take(&mut tex.tracker.rec.borrow_mut().st.steps.log)
}

/// Make the steps whose lines edit `e` changed dirty, and move the other
/// lines of its data to where they are in the new one, data `to`.
fn seed(rr: &mut Recorder, e: &Edit, to: u32, dirty: &mut BTreeMap<u64, StepId>, trace: bool) {
    let fold = &rr.rt.fold;
    let Some(d) = rr.st.steps.datas.get_mut(e.data as usize) else {
        return;
    };
    let old = core::mem::take(&mut d.lines);
    let mut kept = Vec::with_capacity(old.len());
    for l in old {
        let st = &fold.steps[l.step as usize];
        if !(st.live && st.run == l.run) {
            continue;
        }
        if e.touches(l.from, l.to) {
            // (its run again reads the line anew)
            dirty.insert(st.key, l.step);
            if trace {
                let n = alloc::format!(
                    "seed: step {} reads data {} [{}, {}), {} hunks",
                    l.step,
                    e.data,
                    l.from,
                    l.to,
                    e.hunks.len()
                );
                rr.st.steps.log.push(n);
            }
            continue;
        }
        kept.push(LineRead {
            from: e.pos(l.from),
            to: e.pos(l.to),
            ..l
        });
    }
    // (the line numbers read: dirty where the lines before them changed
    // in number, else moved)
    let numbers = core::mem::take(&mut rr.st.steps.datas[e.data as usize].numbers);
    let mut moved = Vec::with_capacity(numbers.len());
    for l in numbers {
        let st = &fold.steps[l.step as usize];
        if !(st.live && st.run == l.run) {
            continue;
        }
        if e.lines_before(l.from) != 0 {
            dirty.insert(st.key, l.step);
            continue;
        }
        let p = e.pos(l.from);
        moved.push(LineRead {
            from: p,
            to: p,
            ..l
        });
    }
    if let Some(n) = rr.st.steps.datas.get_mut(to as usize) {
        n.lines.extend(kept);
        n.lines.sort_by_key(|l| l.from);
        n.numbers.extend(moved);
        n.numbers.sort_by_key(|l| l.from);
    }
}

/// Step `j`'s run left the input at `end`, and its old run at `old`: a
/// file level whose data is other than the old run's (a load that found
/// another φ or another stored value) gets the edit from the one to the
/// other (7.17.3, "Lines are of a data"), whose line readers are dirty;
/// the old end, mapped through those edits.
fn data_edits<H: Host>(
    tex: &Tex<H, SsaTracker>,
    end: &InputState,
    old: InputState,
    dirty: &mut BTreeMap<u64, StepId>,
    rep: &mut RebuildReport,
) -> InputState {
    if end.in_open != old.in_open {
        return old;
    }
    let mut r = tex.tracker.rec.borrow_mut();
    let rr = &mut *r;
    let k = rr.st.steps.edits.len();
    for (x, y) in end.files().zip(old.files()) {
        let (Some(x), Some(y)) = (x, y) else { continue };
        if x.name != y.name || Arc::ptr_eq(&x.data, &y.data) {
            continue;
        }
        let ids = &rr.st.steps.ids;
        let (Some(&d), Some(&n)) = (
            ids.get(&(y.data.as_ptr() as usize)),
            ids.get(&(x.data.as_ptr() as usize)),
        ) else {
            continue;
        };
        // (the same bytes again: an edit with no hunks)
        let e = Edit::diff(d, &y.data, &x.data).unwrap_or_else(|| Edit {
            data: d,
            old: y.data.clone(),
            new: x.data.clone(),
            hunks: Vec::new(),
        });
        if rep.trace {
            let l = alloc::format!(
                "  data edit: {} ({} lines read) to {}, {} hunks",
                d,
                rr.st.steps.datas[d as usize].lines.len(),
                n,
                e.hunks.len()
            );
            rr.st.steps.log.push(l);
        }
        seed(rr, &e, n, dirty, rep.trace);
        rep.data_edits += 1;
        rr.st.steps.edits.push(e);
    }
    if rr.st.steps.edits.len() == k {
        old
    } else {
        old.mapped(&rr.st.steps.edits[k..])
    }
}

/// The step after `s` in the fold reads where `s` left the input, which
/// changed: it is dirty.
fn mark_next<H: Host>(tex: &Tex<H, SsaTracker>, s: StepId, dirty: &mut BTreeMap<u64, StepId>) {
    let r = tex.tracker.rec.borrow();
    let fold = &r.rt.fold;
    if let Some(n) = fold
        .position(s)
        .and_then(|p| fold.order.get(p + 1).copied())
    {
        dirty.insert(fold.steps[n as usize].key, n);
    }
}

/// The old step after `cur` (the step just run), within [`LOOK_AHEAD`],
/// that ended where `end` is, with its old end.
fn meet<H: Host>(
    tex: &Tex<H, SsaTracker>,
    cur: StepId,
    end: &InputState,
) -> Option<(StepId, InputState)> {
    let r = tex.tracker.rec.borrow();
    let fold = &r.rt.fold;
    let from = fold.position(cur)? + 1;
    fold.order[from..].iter().take(LOOK_AHEAD).find_map(|&s| {
        let e = r.st.steps.end(s)?;
        same_place(tex, end, &e).then_some((s, e))
    })
}

/// Step `s` is passed over (item 4): it is gone with its definitions,
/// and a reader of one now reads the definition before it.
fn retire<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    s: StepId,
    dirty: &mut BTreeMap<u64, StepId>,
    rep: &mut RebuildReport,
) {
    let vals = {
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        let key = rr.rt.fold.steps[s as usize].key;
        let old = defs(&rr.rt, &rr.rt.fold.steps[s as usize].recs.clone());
        for a in old.keys().filter(|a| positioned(a)) {
            let next = rr.rt.fold.next_after(a, key).map(|d| d.key);
            for x in rr.rt.fold.readers_between(a, key, next) {
                dirty.insert(rr.rt.fold.steps[x as usize].key, x);
                rep.readers_marked += 1;
            }
        }
        // (its stores are gone: the loads after it of those names read
        // without them)
        let mut ids: Vec<u32> = rr
            .st
            .steps
            .stores
            .remove(&s)
            .unwrap_or_default()
            .iter()
            .map(StoreEv::id)
            .collect();
        ids.dedup();
        rr.st.steps.loads.remove(&s);
        rr.st.steps.queries.remove(&s);
        mark_store_readers(rr, &ids, key, dirty, rep);
        rr.rt.fold.remove(s);
        // (its chunks leave the link)
        rr.st.steps.fx_changed.push(s);
        dirty.remove(&key);
        let mut vals = Vec::new();
        for a in old.keys().filter(|a| positioned(a)) {
            if let Some(v) = latest(tex, rr, *a, s, rep) {
                vals.push((*a, v));
            }
        }
        vals
    };
    rep.restored += vals.len();
    rep.removed += 1;
    put(tex, &vals);
}

/// The loads after `key` of the names in `ids`, which a step stored to
/// differently, that read the build's own store: dirty (7.17.3, "A load
/// reads the store"). The loads that read the φ wait for the next trip.
fn mark_store_readers(
    rr: &Recorder,
    ids: &[u32],
    key: u64,
    dirty: &mut BTreeMap<u64, StepId>,
    rep: &mut RebuildReport,
) {
    if ids.is_empty() {
        return;
    }
    for (&s, seen) in &rr.st.steps.loads {
        let Some(k) = rr.rt.fold.steps.get(s as usize).map(|x| x.key) else {
            continue;
        };
        if k > key && seen.iter().any(|l| !l.phi && ids.contains(&l.id)) {
            dirty.insert(k, s);
            rep.store_readers += 1;
        }
    }
}

/// Run step `j` at its place from `input` (7.17.3 items 2 and 3, "A read
/// resolves by prediction and validation"), the reads of step `predict`'s
/// last run predicting its own; mark the readers of the definitions it
/// changed dirty. Its result.
#[allow(clippy::too_many_arguments)]
fn run_step<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    j: StepId,
    predict: StepId,
    input: &InputState,
    dirty: &mut BTreeMap<u64, StepId>,
    rep: &mut RebuildReport,
    srep: &mut SsaReport,
) -> InputState {
    rep.steps_run += 1;
    if rep.trace {
        // (before it runs: a run that panics is traced)
        note(tex, alloc::format!("step {j} begins at {}", input.brief()));
    }
    let c0 = tex.commands();
    let (key, old, mut next): (u64, BTreeMap<Slot, Version>, Vec<Slot>) = {
        let r = tex.tracker.rec.borrow();
        let fold = &r.rt.fold;
        let key = fold.steps[j as usize].key;
        let old = defs(&r.rt, &fold.steps[j as usize].recs);
        let reads = fold.steps[predict as usize]
            .reads
            .iter()
            .filter(|a| positioned(a) && later(fold, a, key))
            .copied()
            .collect();
        (key, old, reads)
    };
    save_stack_whole(tex, key, &mut next, rep);
    let mut set: BTreeSet<Slot> = BTreeSet::new();
    let mut touched: BTreeSet<Slot> = BTreeSet::new();
    let finished = loop {
        // the definitions that reach the step, where a later one is in
        // the arrays
        let vals = {
            let mut r = tex.tracker.rec.borrow_mut();
            let rr = &mut *r;
            let mut vals = Vec::with_capacity(next.len());
            for a in next.drain(..) {
                if set.insert(a)
                    && let Some(v) = reaching(tex, rr, a, key, rep)
                {
                    vals.push((a, v));
                }
            }
            vals
        };
        rep.positioned += vals.len();
        put(tex, &vals);
        input.set(tex);
        // (the step begins at the checkpoint where the one before it
        // stopped: `main_control` resumes there, the command counted)
        tex.at_checkpoint = true;
        // (a step's outputs are flushed at its end: a dropped run's are
        // not the next run's)
        drop(tex.take_effects());
        tex.log_file.buf.clear();
        // (nor are the bytes a dropped run left waiting in a `\write`
        // stream with no file, which a consistent run never does: a
        // step's end flushes every stream with one)
        for f in &mut tex.write_file {
            f.buf.clear();
        }
        let mut open = Some(open_paragraph(tex, false, srep, Some(j)));
        // (the trace's profile: the step's commands by the file line of
        // the candidate they followed)
        let mut prof: BTreeMap<(usize, i32), u64> = BTreeMap::new();
        let mut at = (tex.in_open, tex.line, tex.commands());
        let mut step = tex.resume();
        let fin = loop {
            match step {
                Step::Checkpoint => {
                    if rep.trace {
                        let c = tex.commands();
                        *prof.entry((at.0, at.1)).or_default() += c - at.2;
                        at = (tex.in_open, tex.line, c);
                    }
                    if matches!(
                        tex.clean_point(),
                        Some(CleanPoint::Outer | CleanPoint::Fire)
                    ) {
                        break false;
                    }
                    step = tex.resume();
                }
                Step::Finished(h) => {
                    rep.history = h;
                    break true;
                }
            }
        };
        if fin {
            tex.tracker.end_open_calls(&*tex);
        }
        if rep.trace && prof.values().sum::<u64>() > 10_000 {
            let mut top: Vec<((usize, i32), u64)> = prof.into_iter().collect();
            top.sort_unstable_by_key(|e| core::cmp::Reverse(e.1));
            let top: Vec<alloc::string::String> = top
                .iter()
                .take(12)
                .map(|((l, n), c)| alloc::format!("{l}:{n} {c}"))
                .collect();
            note(
                tex,
                alloc::format!(
                    "  step {j} ended at level {} line {}; commands by line: {}",
                    tex.in_open,
                    tex.line,
                    top.join(", ")
                ),
            );
        }
        close_paragraph(tex, &mut open, srep, Close::Call);
        // its reads of a slot a later definition holds, not set: it read
        // that later value, and runs again with the slot set too
        let (miss, written) = {
            let r = tex.tracker.rec.borrow();
            let fold = &r.rt.fold;
            let mut n = 0;
            let miss: Vec<Slot> =
                r.rt.open_step_reads()
                    .inspect(|_| n += 1)
                    .filter(|a| positioned(a) && !set.contains(*a) && later(fold, a, key))
                    .copied()
                    .collect();
            rep.reads_checked += n;
            let written = if miss.is_empty() {
                Vec::new()
            } else {
                defs(&r.rt, r.rt.open_step_recs()).into_keys().collect()
            };
            (miss, written)
        };
        if rep.trace && !miss.is_empty() {
            let m: Vec<alloc::string::String> =
                miss.iter().take(8).map(|a| alloc::format!("{a}")).collect();
            note(
                tex,
                alloc::format!(
                    "  run of step {j} dropped: read {} of {} slots at a later definition ({} ...)",
                    miss.len(),
                    set.len() + miss.len(),
                    m.join(" ")
                ),
            );
        }
        if miss.is_empty() {
            break fin;
        }
        rep.retries += 1;
        tex.tracker.rec.borrow_mut().rt.abort_step();
        // (what the dropped run wrote goes back to what reaches the step)
        let vals = {
            let mut r = tex.tracker.rec.borrow_mut();
            let rr = &mut *r;
            let mut vals = Vec::new();
            // (the page's length put back makes the nodes past the list's
            // end stand-ins again: the nodes set go back with it, 7.17.3
            // item 5)
            let len = Slot(Fam::Page, i64::from(crate::track::page::LIST_LEN));
            let nodes: Vec<Slot> = if written.contains(&len) {
                set.iter()
                    .filter(|a| a.0 == Fam::PageNode)
                    .copied()
                    .collect()
            } else {
                Vec::new()
            };
            for a in written.into_iter().filter(positioned).chain(nodes) {
                touched.insert(a);
                if !miss.contains(&a)
                    && let Some(v) = reaching(tex, rr, a, key, rep)
                {
                    vals.push((a, v));
                }
            }
            vals
        };
        put(tex, &vals);
        next = miss;
    };
    // the step ends: its definitions replace the old ones
    let new = {
        let input = InputState::of(tex, finished);
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        let d = defs(&rr.rt, rr.rt.open_step_recs());
        rr.rt.end_step();
        let stores = step_closed(rr, j, input);
        mark_store_readers(rr, &stores, key, dirty, rep);
        d
    };
    let mut changed = Vec::new();
    let vals = {
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        // a definition that changed makes its readers dirty, up to the
        // slot's next definition (item 3)
        for a in old.keys().chain(new.keys()).filter(|a| positioned(a)) {
            touched.insert(*a);
            if old.get(a) == new.get(a) {
                continue;
            }
            rep.defs_changed += 1;
            let next = rr.rt.fold.next_after(a, key).map(|d| d.key);
            let readers = rr.rt.fold.readers_between(a, key, next);
            if rep.trace && changed.len() < 12 {
                // (the trace: which calls of each reader read it)
                let mut who = Vec::new();
                for &s in readers.iter().take(3) {
                    let mut path = Vec::new();
                    who_read(
                        &rr.rt,
                        &rr.rt.fold.steps[s as usize].recs,
                        a,
                        &mut path,
                        &mut who,
                    );
                }
                changed.push(alloc::format!(
                    "{a} ({} readers: {})",
                    readers.len(),
                    who.join("; ")
                ));
            }
            for s in readers {
                if s != j {
                    dirty.insert(rr.rt.fold.steps[s as usize].key, s);
                    rep.readers_marked += 1;
                }
            }
        }
        // the arrays hold the latest definitions again
        let mut vals = Vec::new();
        for a in set.iter().chain(touched.iter()) {
            if let Some(v) = latest(tex, rr, *a, j, rep) {
                vals.push((*a, v));
            }
        }
        vals
    };
    rep.restored += vals.len();
    put(tex, &vals);
    if rep.trace {
        note(
            tex,
            alloc::format!(
                "step {j} (key {key}, predicted by {predict}): {} commands, changed {}",
                tex.commands() - c0,
                changed.join(", ")
            ),
        );
    }
    tex.tracker
        .rec
        .borrow()
        .st
        .steps
        .end(j)
        .expect("the step's result")
}
