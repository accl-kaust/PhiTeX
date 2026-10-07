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
use partex_ssa::fold::{Def, Fold, StepId};

use super::{
    Close, Fam, Recorder, SOFT_PLACE, SVal, Slot, SsaReport, SsaTracker, StepEffects, TexSsa,
    Versions, close_paragraph, line_bounds, open_paragraph, set_value, slot_value,
};
use crate::host::{FileKind, Host, NoHost};
use crate::input::{AlphaFile, InStateRecord, InputValue, same_chain};
use crate::run::Step;
use crate::tex::Tex;
use crate::track::{Query, Tracker, Untracked};

/// How many old steps after the one a run tries to meet are looked at
/// for the place where it ended (item 4: a step that ended elsewhere).
const LOOK_AHEAD: usize = 64;

/// The old steps after a step run again that predict the new steps of
/// its run, at most (with windows: the rebuild's `ahead`).
const CASCADE_AHEAD: usize = 4096;

/// What a cascade of new steps costs (commands, and slots placed or put
/// back) before it is weighed against the old steps after it.
const COLD_FLOOR: u64 = 50_000;

/// The version of each name's φ, with the bytes it was made from.
type PhiVersions = BTreeMap<u32, (Arc<[u8]>, Version)>;

/// A stored name's value as [`Steps::trip_end`] made it (`Steps::value_at`'s,
/// its bytes shared), with the count of the name's store changes then.
type TripValues = BTreeMap<u32, (u64, Option<Result<Option<Arc<[u8]>>, ()>>)>;

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
    /// The steps that loaded each name, by its load id
    /// ([`Steps::phi_seeds`], [`mark_store_readers`]).
    loaders: BTreeMap<u32, BTreeSet<StepId>>,
    /// The names the job stores (the load ids of their addresses).
    stored: BTreeSet<u32>,
    /// The names a producer outside the steps defines ([`define_stream`]:
    /// a tool's output, a node's), with their bytes: stored names no step
    /// opens, whose φ is the value given.
    produced: BTreeMap<u32, Option<Arc<[u8]>>>,
    /// The loads some step read whole (`\pdffilesize`, `\pdfmdfivesum`,
    /// an image, a font's file), not by lines: a rebuild compares their
    /// contents, not only the lines read of them.
    whole: BTreeSet<u32>,
    /// Inside a rebuild: each stored name's φ, what the last trip stored
    /// (absent: it was not there).
    phi: Option<BTreeMap<u32, Option<Arc<[u8]>>>>,
    /// The φ the last trip read, kept after it (DESIGN 3.7, "Trips, as
    /// built"): a name no live step opens keeps it, and a name whose
    /// value is the same keeps its bytes.
    last_phi: BTreeMap<u32, Option<Arc<[u8]>>>,
    /// The version of each name's φ, with the bytes it was made from
    /// ([`Steps::phi_seeds`]: the same bytes, shared, have it).
    phi_vers: core::cell::RefCell<PhiVersions>,
    /// Between two trips of a build: what the next one starts from.
    next: Option<NextTrip>,
    /// A rebuild that stopped (its deadline, its budget, cancelled: DESIGN
    /// 3.7, "A rebuild stopped"): the work it left, which the next
    /// rebuild takes up.
    pending: Option<Pending>,
    /// With [`SsaTracker::keep_complete`], after a trip whose job ended
    /// fatally: each stored name's value as the last complete trip left it
    /// (DESIGN 3.7, "A trip that ended fatally"), which the next trip's φ,
    /// the tools, the link and [`stream_values`] take instead of the
    /// stores; `None` while the stores are the last trip's and it was
    /// complete (or the option is off).
    complete: Option<BTreeMap<u32, Option<Arc<[u8]>>>>,
    /// The queries of the host the open step's run asked, and each
    /// step's, by step id, with their answers' versions (7.17.3, "A
    /// query is asked again").
    cur_queries: Vec<(Query, u128)>,
    queries: BTreeMap<StepId, Vec<(Query, u128)>>,
    /// The version of each font's glyphs used as the job's end last made
    /// them: a ship whose glyphs changed wakes the end only if the union
    /// its rows make now differs ([`glyph_union_now`], DESIGN 4.3, "The
    /// job's end").
    glyph_union: Option<u128>,
    /// The ships' glyph rows the union was last counted from
    /// ([`glyph_union_now`]), and the rows whose definitions a step's run
    /// or removal changed since.
    glyph_count: GlyphCount,
    glyph_dirty: BTreeSet<i64>,
    /// The open step's run's effects in the link's form, its chunks in
    /// program order (7.17.3, "Hits applied inside a step that runs
    /// again", item 2), and each step's, by step id, as its run ended
    /// ("The link after a rebuild is a watch's link").
    pub(super) cur_chunks: Vec<StepEffects>,
    pub(super) effects: Vec<Vec<StepEffects>>,
    /// The files the running step's last run opened, in order, with the
    /// host's handles: a step that runs again opens each on its handle
    /// again ([`Steps::reopen`]), so a writer's state that holds it, and
    /// the later steps' writes to it, are as they were.
    reopen: Vec<(Vec<u8>, FileKind, crate::host::WriteId)>,
    /// The steps whose chunks changed since the link last took them (a
    /// run closed, or the step left the fold): what the link costs (DESIGN
    /// 4.3 item 4, [`super::take_step_changes`]).
    pub(super) fx_changed: Vec<StepId>,
    /// With the columns the link's, each step's chunks rendered
    /// (`effects/flow.rs`, [`super::resolve_flows`]).
    pub(super) flow: Option<super::Flows>,
    /// The run of a step under way past its budget ([`run_step`]).
    watch: Option<Watch>,
    /// How many times each stored name's stores changed (a step's run
    /// stored to it otherwise than its last, a step that stored to it
    /// left the fold), and the value [`Steps::trip_end`] made of it last,
    /// with that count then: made again only when the count moved.
    store_changes: BTreeMap<u32, u64>,
    trip_values: TripValues,
    /// A worker's steps (`par.rs`): the build's data ids end at
    /// `data_base` (its own data are numbered after), and each data its
    /// run loaded, in order, for the commit to load in the build: the
    /// name's load id, the data and whether it was the file.
    data_base: u32,
    loaded_log: Option<Vec<(u32, Arc<[u8]>, bool, u32)>>,
}

/// A worker's run's logs of its step (`par.rs`): what [`step_closed`]
/// takes from the open step, and what the run added to the steps'
/// tables, for the commit to put in the build's.
pub(super) struct StepLogs {
    stores: Vec<StoreEv>,
    loads: Vec<LoadSeen>,
    queries: Vec<(Query, u128)>,
    lines: Vec<(u32, usize, usize)>,
    numbers: BTreeSet<(usize, usize)>,
    pub(super) chunks: Vec<StepEffects>,
    whole: Vec<u32>,
    loaded: Vec<(u32, Arc<[u8]>, bool, u32)>,
    data_base: u32,
}

impl StepLogs {
    /// Whether the run loaded or stored a name in `ids`.
    pub(super) fn touches(&self, ids: impl Fn(u32) -> bool) -> bool {
        self.stores.iter().any(|e| ids(e.id())) || self.loads.iter().any(|l| ids(l.id))
    }

    /// Whether the run stored to a name or opened one for storing.
    pub(super) fn stores(&self) -> bool {
        !self.stores.is_empty()
    }

    /// The load ids in the logs, mapped by `f` (a worker's own ids to the
    /// build's).
    pub(super) fn map_loads(&mut self, f: impl Fn(u32) -> u32) {
        for e in &mut self.stores {
            match e {
                StoreEv::Open(i)
                | StoreEv::Remove(i)
                | StoreEv::Line(i, _)
                | StoreEv::Made(i, _) => {
                    *i = f(*i);
                }
            }
        }
        for l in &mut self.loads {
            l.id = f(l.id);
        }
        for w in &mut self.whole {
            *w = f(*w);
        }
        for l in &mut self.loaded {
            l.0 = f(l.0);
        }
    }
}

impl Steps {
    /// A worker's steps: the build's data by address (`steps`), its own
    /// numbered after them, nothing else.
    pub(super) fn worker_of(steps: &Steps) -> Steps {
        Steps {
            ids: steps.ids.clone(),
            data_base: u32::try_from(steps.datas.len()).unwrap_or(u32::MAX),
            loaded_log: Some(Vec::new()),
            ..Steps::default()
        }
    }

    /// A worker's run of a step ended: its logs, taken.
    pub(super) fn take_logs(&mut self) -> StepLogs {
        StepLogs {
            stores: core::mem::take(&mut self.cur_stores),
            loads: core::mem::take(&mut self.cur_loads),
            queries: core::mem::take(&mut self.cur_queries),
            lines: core::mem::take(&mut self.step_lines),
            numbers: core::mem::take(&mut self.step_numbers),
            chunks: core::mem::take(&mut self.cur_chunks),
            whole: core::mem::take(&mut self.whole).into_iter().collect(),
            loaded: self
                .loaded_log
                .as_mut()
                .map(core::mem::take)
                .unwrap_or_default(),
            data_base: self.data_base,
        }
    }

    /// A worker's run of a step committed: its logs as the open step's,
    /// and its tables' additions made in the build's (the data it loaded
    /// loaded here, its lines' data ids mapped to them).
    pub(super) fn put_logs(&mut self, logs: StepLogs) {
        let mut map: BTreeMap<u32, u32> = BTreeMap::new();
        for (id, data, file, local) in logs.loaded {
            let d = self.loaded(id, &data, file);
            if local >= logs.data_base {
                map.insert(local, d);
            }
        }
        self.whole.extend(logs.whole);
        for e in &logs.stores {
            if !matches!(e, StoreEv::Line(..)) {
                self.stored.insert(e.id());
            }
        }
        self.cur_stores = logs.stores;
        self.cur_loads = logs.loads;
        self.cur_queries = logs.queries;
        self.step_lines = logs
            .lines
            .into_iter()
            .map(|(d, a, b)| {
                (
                    if d >= logs.data_base {
                        map.get(&d).copied().unwrap_or(d)
                    } else {
                        d
                    },
                    a,
                    b,
                )
            })
            .collect();
        self.step_numbers = logs.numbers;
        self.cur_chunks = logs.chunks;
    }

    /// The open step's reopen list (the files its last run opened), as a
    /// worker's run of step `j` gets it.
    pub(super) fn reopen_of(&self, j: StepId) -> Vec<(Vec<u8>, FileKind, crate::host::WriteId)> {
        let mut v = Vec::new();
        for e in self
            .effects
            .get(j as usize)
            .into_iter()
            .flatten()
            .flat_map(|c| c.1.iter())
        {
            if let crate::effects::Effect::Open { file, name, kind } = e {
                v.push((name.clone(), *kind, *file));
            }
        }
        v
    }

    /// Give a worker's run its reopen list ([`Steps::reopen_of`]).
    pub(super) fn set_reopen(&mut self, v: Vec<(Vec<u8>, FileKind, crate::host::WriteId)>) {
        self.reopen = v;
    }

    /// The load ids of the names the job stores.
    pub(super) fn stored_ids(&self) -> BTreeSet<u32> {
        self.stored.clone()
    }

    /// Which result of step `s`'s runs [`Steps::end`] gives now: the edits
    /// before it was taken, and its address (a commit's test that the
    /// input a worker's run began at is still the step's).
    pub(super) fn end_tag(&self, s: StepId) -> Option<(usize, usize, usize)> {
        let (n, i) = self.inputs.get(s as usize)?.as_ref()?;
        Some((*n, Arc::as_ptr(i) as usize, self.edits.len()))
    }
}

/// What the steps' own records hold, roughly, in bytes by part (a
/// report: `PARTEX_SSA_MEM`).
pub(crate) fn steps_mem_report(st: &Steps) -> alloc::string::String {
    use core::mem::size_of;
    let inputs = st.inputs.iter().flatten().count();
    let top: usize = st
        .inputs
        .iter()
        .flatten()
        .map(|(_, i)| i.top.capacity())
        .sum();
    let fx: usize = st.effects.iter().map(Vec::len).sum();
    let fxc: usize = st.effects.iter().map(Vec::capacity).sum();
    alloc::format!(
        "steps: inputs {inputs} ({} B each, lines {} KB); effects {fx} chunks (capacity {fxc}, {} B each) in {} steps; stores {} steps, loads {} steps, queries {} steps",
        size_of::<InputState>(),
        top >> 10,
        size_of::<StepEffects>(),
        st.effects.len(),
        st.stores.len(),
        st.loads.len(),
        st.queries.len(),
    )
}

/// A run of a step that went on past its budget (twice its last run's
/// commands, and some), checked for reads of slots a later definition
/// holds, not placed (DESIGN 7.17.3, "A read resolves by prediction and
/// validation"): it read the arrays' value, is dropped and must stop. Such
/// a run may never reach the step's end: with the output routine's being
/// active, left by a later step's fatal fire, `\end` fires again and
/// again. ([`read_later`].)
struct Watch {
    /// The step's place and the slots its run is placed at.
    key: u64,
    set: BTreeSet<Slot>,
    /// The reads checked so far, and whether one read a later definition.
    scanned: usize,
    later: bool,
}

/// Whether the run of a step under way past its budget read a later
/// definition: its reads made since the last time checked.
pub(super) fn read_later(rr: &mut Recorder) -> bool {
    let Some(w) = rr.st.steps.watch.as_mut() else {
        return false;
    };
    if !w.later {
        let fold = &rr.rt.fold;
        for a in rr.rt.open_step_reads_from(w.scanned) {
            w.scanned += 1;
            if checked(a) && !w.set.contains(&placed_as(a)) && later(fold, a, w.key) {
                w.later = true;
                break;
            }
        }
    }
    w.later
}

/// What trip k+1 of a build starts from (DESIGN 3.7, "Trips, as built").
struct NextTrip {
    /// Each stored name's φ: what trip k stored.
    phi: BTreeMap<u32, Option<Arc<[u8]>>>,
    /// The names whose φ is not trip k's.
    changed: BTreeSet<u32>,
    /// A tool wrote a file: the job's other loads are looked at again.
    files: bool,
}

/// The work a stopped rebuild left (DESIGN 3.7, "A rebuild stopped"):
/// the dirty steps it did not reach, each with why, and the φ its trip
/// served, which the link has not written (it waits for the work).
struct Pending {
    dirty: Dirty,
    phi: BTreeMap<u32, Option<Arc<[u8]>>>,
}

/// Where a step left a file's line ([`Steps::ended_at`]): the data's
/// load id and bytes, where the line begins in it, and its rest.
pub(super) type Ended = (u32, Arc<[u8]>, usize, Vec<u32>);

/// A store a step made: a name opened (`\openout`, a whole definition)
/// or a line appended to it.
#[derive(Clone, PartialEq, Eq)]
enum StoreEv {
    Open(u32),
    Line(u32, Arc<[u8]>),
    /// A command (`\write18`) made the file, these its bytes as they are
    /// (a last line without its end kept so), or removed it (DESIGN 3.7,
    /// "Commands").
    Made(u32, Arc<[u8]>),
    Remove(u32),
}

impl StoreEv {
    fn id(&self) -> u32 {
        match self {
            StoreEv::Open(i) | StoreEv::Line(i, _) | StoreEv::Made(i, _) | StoreEv::Remove(i) => *i,
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
    /// An edit replaced it: its readers moved to the new data, and an
    /// input state that held it is mapped through that edit, so once no
    /// line of it is read it is not compared with its file again (a
    /// rebuild's diffs cost the data read now, not every one an edit
    /// ever left behind).
    superseded: bool,
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
        let n = u32::try_from(self.datas.len())
            .unwrap_or(u32::MAX)
            .saturating_add(self.data_base);
        let d = *self.ids.entry(data.as_ptr() as usize).or_insert(n);
        if let Some(l) = &mut self.loaded_log {
            // (a worker's run: the commit loads it in the build)
            l.push((id, data.clone(), file, d));
        }
        if d == n {
            self.datas.push(Data {
                name: id,
                bytes: data.clone(),
                file,
                lines: Vec::new(),
                numbers: Vec::new(),
                superseded: false,
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

    /// The open step read file `data` to its end, at `pos`, finding no
    /// line there: a read of nothing (an edit that adds lines there
    /// touches it), kept apart from the line before.
    pub(super) fn eof_read(&mut self, data: &[u8], pos: usize) {
        if let Some(&id) = self.ids.get(&(data.as_ptr() as usize)) {
            self.step_lines.push((id, pos, pos));
        }
    }

    /// Load `id` was read whole.
    pub(super) fn read_whole(&mut self, id: u32) {
        self.whole.insert(id);
    }

    /// The open step read the line number of a level reading the data at
    /// `addr`, which was at `pos` when the step began.
    pub(super) fn number_read(&mut self, addr: usize, pos: usize) {
        self.step_numbers.insert((addr, pos));
    }

    /// A step begins a run (`rerun`: step `j` runs again): no stores,
    /// loads or queries yet, and the files its last run opened to open
    /// again.
    pub(super) fn run_begins(&mut self, rerun: Option<StepId>) {
        self.step_lines.clear();
        self.step_numbers.clear();
        self.cur_stores.clear();
        self.cur_loads.clear();
        self.cur_queries.clear();
        self.cur_chunks.clear();
        self.reopen.clear();
        let old = rerun.and_then(|j| self.effects.get(j as usize));
        for e in old.into_iter().flatten().flat_map(|c| c.1.iter()) {
            if let crate::effects::Effect::Open { file, name, kind } = e {
                self.reopen.push((name.clone(), *kind, *file));
            }
        }
    }

    /// The handle the running step's last run opened file `name` on, the
    /// first of them not given yet.
    pub(super) fn reopen(&mut self, name: &[u8], kind: FileKind) -> Option<crate::host::WriteId> {
        let i = self
            .reopen
            .iter()
            .position(|(n, k, _)| n == name && *k == kind)?;
        Some(self.reopen.remove(i).2)
    }

    /// The open step asked query `q`, answered as `answer` versions.
    pub(super) fn queried(&mut self, q: Query, answer: u128) {
        self.cur_queries.push((q, answer));
    }

    /// The job's end made each font's glyphs used, of version `union`.
    pub(super) fn glyphs_united(&mut self, union: u128) {
        self.glyph_union = Some(union);
    }

    /// The open step opened name `id` for storing.
    pub(super) fn store_opened(&mut self, id: u32) {
        self.stored.insert(id);
        self.cur_stores.push(StoreEv::Open(id));
    }

    /// A command run by the open step made name `id`'s file (its lines
    /// follow, as an open's).
    pub(super) fn store_made(&mut self, id: u32, contents: Arc<[u8]>) {
        self.stored.insert(id);
        self.cur_stores.push(StoreEv::Made(id, contents));
    }

    /// The names whose store reaching the open step, at `key`, is the
    /// job's (`\openout`, not a command's), with their values there: what
    /// a command run there is handed (DESIGN 3.7, "Commands").
    pub(super) fn reaching(&self, fold: &Fold<TexSsa>, key: u64) -> Vec<(u32, Vec<u8>)> {
        self.stored
            .iter()
            .filter_map(|&id| match self.value_from(fold, id, key)? {
                Ok((Some(v), false)) => Some((id, v)),
                _ => None,
            })
            .collect()
    }

    /// A command the open step ran removed name `id`'s file.
    pub(super) fn store_removed(&mut self, id: u32) {
        self.stored.insert(id);
        self.cur_stores.push(StoreEv::Remove(id));
    }

    /// The stored names whose file is removed at the build's end (a
    /// command removed it after its last open): the link leaves none.
    pub(super) fn removed_at_end(&self, fold: &Fold<TexSsa>) -> Vec<u32> {
        self.stored
            .iter()
            .copied()
            .filter(|&id| matches!(self.value_at(fold, id, u64::MAX), Some(Ok(None))))
            .collect()
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
    /// its lines since its last open before here (`Ok(Some)`), absent if
    /// a command removed it since (`Ok(None)`), or the φ (`Err`) if
    /// neither is; `None` if the job does not store it.
    fn value_at(
        &self,
        fold: &Fold<TexSsa>,
        id: u32,
        key: u64,
    ) -> Option<Result<Option<Vec<u8>>, ()>> {
        Some(self.value_from(fold, id, key)?.map(|(v, _)| v))
    }

    /// [`Steps::value_at`], and whether a command made the value (or
    /// removed the file).
    #[allow(clippy::type_complexity)]
    fn value_from(
        &self,
        fold: &Fold<TexSsa>,
        id: u32,
        key: u64,
    ) -> Option<Result<(Option<Vec<u8>>, bool), ()>> {
        // (`evs`' lines of `id` after its last open, latest first; whether
        // an open is there, or a removal)
        fn back<'a>(
            evs: &'a [StoreEv],
            id: u32,
            lines: &mut Vec<&'a Arc<[u8]>>,
            removed: &mut bool,
            made: &mut Option<&'a Arc<[u8]>>,
        ) -> bool {
            for e in evs.iter().rev() {
                match e {
                    StoreEv::Open(i) if *i == id => return true,
                    StoreEv::Made(i, c) if *i == id => {
                        *made = Some(c);
                        return true;
                    }
                    StoreEv::Remove(i) if *i == id => {
                        *removed = true;
                        return true;
                    }
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
        let (mut removed, mut made) = (false, None);
        // (the open step's own, then the steps before it, latest first)
        let mut found = back(&self.cur_stores, id, &mut lines, &mut removed, &mut made);
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
                if back(evs, id, &mut lines, &mut removed, &mut made) {
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return Some(Err(()));
        }
        if removed {
            return Some(Ok((None, true)));
        }
        // (a command's file as it made it, then the lines after it)
        let mut v = made.map_or_else(Vec::new, |c| c.to_vec());
        for l in lines.iter().rev() {
            v.extend_from_slice(l);
            v.push(b'\n');
        }
        Some(Ok((Some(v), made.is_some())))
    }

    /// The cold build's first trip serves its loads too (`on`), from no φ:
    /// a name the job stored before the load is served its store, any
    /// other the host's file (DESIGN 3.7: the job's writes reach no file
    /// before the link).
    pub(crate) fn serve_cold(&mut self, on: bool) {
        self.phi = on.then(BTreeMap::new);
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
            Ok(v) => Some(v.map(Arc::from)),
            Err(()) => phi.get(&id).cloned(),
        }
    }

    /// What each stored name holds at a trip's end (DESIGN 3.7, "Trips,
    /// as built"): its lines since its last open, or, if no live step
    /// opens it, the φ the trip read. The next trip's φ, and the names
    /// whose value is not that φ; a name whose value is the same keeps
    /// the φ's bytes, shared.
    #[allow(clippy::type_complexity)]
    fn trip_end(
        &mut self,
        fold: &Fold<TexSsa>,
    ) -> (BTreeMap<u32, Option<Arc<[u8]>>>, BTreeSet<u32>) {
        let mut phi = BTreeMap::new();
        let mut changed = BTreeSet::new();
        // (a trip that ended fatally, its stores withheld: the last
        // complete trip's values, DESIGN 3.7, "A trip that ended fatally")
        if let Some(c) = &self.complete {
            for (&id, v) in c {
                let same = match (self.last_phi.get(&id), v) {
                    (Some(Some(w)), Some(v)) => Arc::ptr_eq(w, v) || w[..] == v[..],
                    (Some(None), None) => true,
                    _ => false,
                };
                if !same {
                    changed.insert(id);
                }
                phi.insert(id, v.clone());
            }
            return (phi, changed);
        }
        // (the values made with no step open, kept)
        let keep = self.cur_stores.is_empty();
        let stored: Vec<u32> = self.stored.iter().copied().collect();
        for id in stored {
            // (a name whose stores did not change since: the value made
            // then, its bytes shared)
            let count = self.store_changes.get(&id).copied().unwrap_or(0);
            let now = match self.trip_values.get(&id) {
                Some((c, v)) if *c == count && keep => v.clone(),
                _ => self
                    .value_at(fold, id, u64::MAX)
                    .map(|v| v.map(|v| v.map(Arc::<[u8]>::from))),
            };
            let was = self.last_phi.get(&id);
            let Some(Ok(now)) = now else {
                if keep {
                    self.trip_values.insert(id, (count, now));
                }
                if let Some(w) = was {
                    phi.insert(id, w.clone());
                }
                continue;
            };
            // (absent: a command removed it)
            let Some(now) = now else {
                if !matches!(was, Some(None)) {
                    changed.insert(id);
                }
                if keep {
                    self.trip_values.insert(id, (count, Some(Ok(None))));
                }
                phi.insert(id, None);
                continue;
            };
            let now = match was {
                Some(Some(w)) if Arc::ptr_eq(w, &now) || w[..] == now[..] => w.clone(),
                _ => {
                    changed.insert(id);
                    now
                }
            };
            if keep {
                self.trip_values
                    .insert(id, (count, Some(Ok(Some(now.clone())))));
            }
            phi.insert(id, Some(now));
        }
        (phi, changed)
    }

    /// The loads of the φ that found other than `phi` holds (DESIGN 3.7):
    /// each one's step's key, the step, and the name loaded.
    fn phi_seeds(
        &self,
        fold: &Fold<TexSsa>,
        phi: &BTreeMap<u32, Option<Arc<[u8]>>>,
    ) -> Vec<(u64, StepId, u32)> {
        // (a φ kept from the last trip is the same bytes, shared, or the
        // same bytes read again: its version is the one made then)
        let mut memo = self.phi_vers.borrow_mut();
        let vers: BTreeMap<u32, Version> = phi
            .iter()
            .map(|(&id, v)| {
                let ver = v.as_ref().map_or(Version::ABSENT, |c| {
                    let kept = memo.get_mut(&id).and_then(|(was, ver)| {
                        (Arc::ptr_eq(was, c) || was[..] == c[..]).then(|| {
                            *was = c.clone();
                            *ver
                        })
                    });
                    kept.unwrap_or_else(|| {
                        let ver = Version::of(&c[..]);
                        memo.insert(id, (c.clone(), ver));
                        ver
                    })
                });
                (id, ver)
            })
            .collect();
        let mut out = Vec::new();
        for (&id, v) in &vers {
            for &s in self.loaders.get(&id).into_iter().flatten() {
                let seen = self.loads.get(&s).map_or(&[][..], |l| &l[..]);
                for l in seen.iter().filter(|l| l.id == id && l.phi) {
                    if *v != l.ver {
                        out.push((fold.steps[s as usize].key, s, l.id));
                    }
                }
            }
        }
        out
    }

    /// The runs of lines the steps read (7.17.3, "Lines are of a data"),
    /// each with its data's load id and bytes, where the run begins and
    /// where the line after it does, and the step and the run of it that
    /// read it (an old run's are dead): the view's source spans
    /// (`view.rs`).
    pub(super) fn line_runs(
        &self,
    ) -> impl Iterator<Item = (u32, &Arc<[u8]>, usize, usize, StepId, u32)> + '_ {
        self.datas.iter().flat_map(|d| {
            d.lines
                .iter()
                .map(move |l| (d.name, &d.bytes, l.from, l.to, l.step, l.run))
        })
    }

    /// Where step `s` left the input, in the source as it is now, if a
    /// file's line is on top with some of it not read yet: the data (its
    /// load id and bytes), where the line begins in it, and the rest of
    /// it in the buffer, its end-of-line character included. What the
    /// next step begins with (the view's spans, `view.rs`).
    pub(super) fn ended_at(&self, s: StepId) -> Option<Ended> {
        let e = self.end(s)?;
        if e.finished || e.cur.state == crate::web::TOKEN_LIST {
            return None;
        }
        let f = e.file.file.as_ref()?;
        let d = self
            .datas
            .get(*self.ids.get(&(f.data.as_ptr() as usize))? as usize)?;
        let at = |x: i32| usize::try_from(x).ok()?.checked_sub(e.v.from);
        let rest = e.top.get(at(e.cur.loc)?..=at(e.cur.limit)?)?;
        (!rest.is_empty()).then(|| (d.name, d.bytes.clone(), f.line_from, rest.to_vec()))
    }

    /// [`place_key`] of [`Steps::end`]`(s)`, made without its copy.
    fn end_key(&self, s: StepId) -> Option<PlaceKey> {
        let (g, e) = self.inputs.get(s as usize)?.as_ref()?;
        Some(e.mapped_key(&self.edits[*g..]))
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
            *s.store_changes.entry(i).or_default() += 1;
        }
    }
    let loads = core::mem::take(&mut s.cur_loads);
    // (the names it loads, indexed: its last run's taken out first)
    for l in s.loads.get(&id).into_iter().flatten() {
        if let Some(ss) = s.loaders.get_mut(&l.id) {
            ss.remove(&id);
        }
    }
    for l in &loads {
        s.loaders.entry(l.id).or_default().insert(id);
    }
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
    s.reopen.clear();
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
    /// Stopped before a `\shipout` (the token backed up): the next step
    /// begins with it (`CleanPoint::Ship`).
    ship: bool,
    /// Stopped at the command after one that read a file whole
    /// (`CleanPoint::Load`).
    load: bool,
    /// The page builder a paragraph's end (or start) deferred is pending:
    /// the next step begins with it (`CleanPoint::Page`).
    page: bool,
    /// Stopped at a paragraph's start (`CleanPoint::Graf`), or before the
    /// page builder `new_graf` deferred.
    graf: bool,
    /// An `\endinput` waits for its line's end (§362): live at a
    /// window's boundary (DESIGN 4.3 item 1).
    force_eof: bool,
    line: i32,
    /// The shared parts: the levels, the parameters, the buffer below the
    /// top file level's line and the file levels below the top one.
    v: InputValue,
    /// The top file level's line, from its start to `first` or `last`.
    top: Vec<u32>,
    first: usize,
    last: usize,
    cur: InStateRecord,
    in_open: usize,
    /// The top file level's entries of the per-file arrays.
    file: FileTop,
    pseudo: Vec<crate::input::PseudoFile>,
    /// The engine's flags that expansion sets and puts back around a
    /// scan, as they were where the step stopped.
    scan: Scanning,
}

/// The flags tex.web sets for the length of a scan or an error and puts
/// back after it: `\csname`'s (pdfTeX's `is_in_csname`, which
/// `\ifincsname` reads), §305's `scanner_status` and `warning_index`,
/// web2c's `expand_depth_count`, §256's `no_new_control_sequence`, §527's
/// `name_in_progress`, §96's `OK_to_interrupt`, §76's `deletions_allowed`
/// and §270's `set_box_allowed`. Not values of the families: the engine
/// alone reads them, and a step boundary, between two commands of
/// `main_control`, is never inside a scan, so each holds its resting
/// value there. A run dropped inside a scan (it read a later definition,
/// DESIGN 7.17.3) leaves them as the scan had them: the next run of a
/// step starts from the step's own, as it starts from its input. (A run
/// dropped inside `\csname` left `is_in_csname` set, and LaTeX's `~`,
/// `\ifincsname` true, typeset an other `~` in the text after it.)
#[derive(Clone, Copy)]
struct Scanning {
    in_csname: bool,
    scanner_status: i32,
    warning_index: i32,
    expand_depth: i32,
    no_new_cs: bool,
    name_in_progress: bool,
    ok_to_interrupt: bool,
    deletions_allowed: bool,
    set_box_allowed: bool,
}

impl Scanning {
    fn of<H: Host, T: Tracker>(t: &Tex<H, T>) -> Self {
        Scanning {
            in_csname: t.is_in_csname,
            scanner_status: t.scanner_status,
            warning_index: t.warning_index,
            expand_depth: t.expand_depth_count,
            no_new_cs: t.no_new_control_sequence,
            name_in_progress: t.name_in_progress,
            ok_to_interrupt: t.ok_to_interrupt,
            deletions_allowed: t.deletions_allowed,
            set_box_allowed: t.set_box_allowed,
        }
    }

    fn set<H: Host, T: Tracker>(self, t: &mut Tex<H, T>) {
        t.is_in_csname = self.in_csname;
        t.scanner_status = self.scanner_status;
        t.warning_index = self.warning_index;
        t.expand_depth_count = self.expand_depth;
        t.no_new_control_sequence = self.no_new_cs;
        t.name_in_progress = self.name_in_progress;
        t.ok_to_interrupt = self.ok_to_interrupt;
        t.deletions_allowed = self.deletions_allowed;
        t.set_box_allowed = self.set_box_allowed;
    }
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
            ship: t.ship_stop == 1,
            load: t.load_stop == 2,
            page: t.page_pending,
            graf: t.graf_stop,
            force_eof: t.force_eof,
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
            scan: Scanning::of(t),
        }
    }

    /// Each open file level's file, bottom up.
    fn files(&self) -> impl Iterator<Item = Option<&AlphaFile>> {
        let lower = self.v.files.files.iter().map(Option::as_ref);
        lower.chain([self.file.file.as_ref()])
    }

    /// [`place_key`] of [`InputState::mapped`], made without the copy:
    /// of the levels only the top two's line numbers are in the key.
    fn mapped_key(&self, edits: &[Edit]) -> PlaceKey {
        let mut k = place_key(self);
        let mut top = self.file.file.clone();
        let mut below = self
            .in_open
            .checked_sub(1)
            .and_then(|j| self.v.files.files.get(j).cloned().flatten());
        for e in edits {
            if let Some(f) = top.as_mut().filter(|f| Arc::ptr_eq(&f.data, &e.old)) {
                k.3 += e.map_file(f);
            }
            if let Some(f) = below.as_mut().filter(|f| Arc::ptr_eq(&f.data, &e.old)) {
                k.4 += e.map_file(f);
            }
        }
        k
    }

    /// Put the input where this state has it.
    pub(super) fn set<H: Host, T: Tracker>(&self, t: &mut Tex<H, T>) {
        t.fire_pending = self.fire;
        t.ship_stop = u8::from(self.ship);
        t.load_stop = if self.load { 2 } else { 0 };
        t.page_pending = self.page;
        t.graf_stop = self.graf;
        t.force_eof = self.force_eof;
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
        self.scan.set(t);
    }

    /// This state in the source after `edits` (DESIGN 3.5: a position
    /// after an edit moves by its length, a line number by its lines).
    fn mapped(&self, edits: &[Edit]) -> InputState {
        let mut s = self.clone();
        for e in edits {
            for j in 0..=s.in_open {
                // (each edit applies to the data it replaced; a level it
                // does not apply to leaves the shared levels unshared)
                let applies = if j == s.in_open {
                    s.file.file.as_ref()
                } else {
                    s.v.files.files[j].as_ref()
                }
                .is_some_and(|f| Arc::ptr_eq(&f.data, &e.old));
                if !applies {
                    continue;
                }
                let at = if j == s.in_open {
                    s.file.file.as_mut()
                } else {
                    Arc::make_mut(&mut s.v.files).files[j].as_mut()
                };
                let Some(f) = at else {
                    continue;
                };
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

fn same_data<C: PartialEq>(a: &Arc<[C]>, b: &Arc<[C]>) -> bool {
    Arc::ptr_eq(a, b) || a[..] == b[..]
}

/// Whether two files are at the same place in the same data. Not their
/// `SyncTeX` tags, the files' numbers in the order the job opened them
/// ([`same_tags`]): a run that opens one file fewer before (a chapter
/// `\includeonly` leaves out) numbers every later file one lower, and
/// still comes back to the old steps' places.
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

/// Whether two results' files have the same `SyncTeX` tags. They are
/// read only by `SyncTeX`'s records; with it off (no `-synctex`, no
/// `\synctex` set yet) they are not the input's ([`same_input`]).
fn same_tags(a: &InputState, b: &InputState) -> bool {
    let tag = |f: Option<&AlphaFile>| f.map_or(0, |f| f.synctex_tag);
    tag(a.file.file.as_ref()) == tag(b.file.file.as_ref())
        && a.v.files.files.len() == b.v.files.files.len()
        && a.v
            .files
            .files
            .iter()
            .zip(&b.v.files.files)
            .all(|(x, y)| tag(x.as_ref()) == tag(y.as_ref()))
}

/// Whether two results leave the input at the same place (DESIGN 3.15,
/// step 5): the same files at the same positions and lines, the same
/// levels. A place in a line is its offset in it, from its start or from
/// its end: where the line ends (`limit`, and `first` and `last` after it)
/// is the input's, so a step that ends in a line that an edit before or
/// after that place changed meets its old end, and the next step runs
/// again in its place.
fn same_place<H: Host, T: Tracker>(t: &Tex<H, T>, a: &InputState, b: &InputState) -> bool {
    let rec = |x: &InStateRecord, y: &InStateRecord| {
        // (a file level's place in its line counts from either end: an
        // edit earlier in the line moves it from the start, one later from
        // the end; equal lines, which `same_input` asks for, agree on both)
        let at = x.loc == y.loc
            || (x.state != crate::web::TOKEN_LIST && x.limit - x.loc == y.limit - y.loc);
        x.state == y.state
            && x.index == y.index
            && x.start == y.start
            && at
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
        && a.ship == b.ship
        && a.load == b.load
        && a.page == b.page
        && a.graf == b.graf
        && a.in_open == b.in_open
        && a.line == b.line
        && a.file.line == b.file.line
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
        && a.force_eof == b.force_eof
        && same_data(&a.v.below, &b.v.below)
        && a.top == b.top
        && a.first == b.first
        && a.last == b.last
        && a.cur.limit == b.cur.limit
        && a.cur.same_list(&b.cur)
        && same_chain(a.v.levels.as_ref(), b.v.levels.as_ref(), |x, y| {
            x.same_list(y) && x.limit == y.limit
        })
        && a.v.np == b.v.np
        && same_chain(a.v.params.as_ref(), b.v.params.as_ref(), |x, y| x == y)
        && a.pseudo == b.pseudo
        && x.grp == y.grp
        && x.ifs == y.ifs
        && x.eof == y.eof
        && same_str(t, x.name, y.name)
        && same_str(t, x.full_name, y.full_name)
        && (t.sync.is_none() || same_tags(a, b))
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
    /// where it begins); a read of no line (a file's end met at `from`)
    /// is touched by a hunk that inserts or changes bytes there.
    fn touches(&self, from: usize, to: usize) -> bool {
        let i = self.hunks.partition_point(|h| h.ot < from);
        self.hunks.get(i).is_some_and(|h| {
            to > h.of || (from == to && h.of <= from && (h.ot > from || h.of == from))
        })
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
        f.call = self.pos(f.call);
        f.pos = self.pos(f.pos);
        f.lines = f.lines.saturating_add_signed(d);
        d
    }
}

/// What a rebuild did: the counts AGENTS.md asks each report for.
#[derive(Clone, Debug, Default)]
pub struct RebuildReport {
    pub history: i32,
    /// The rebuild stopped after a step because [`super::SsaTracker::cancel`]
    /// said so; it stops as one past its deadline does.
    pub cancelled: bool,
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
    /// Old steps run again whose definitions all came out as they were:
    /// woken by a version that changed while the content they read did
    /// not, or whose change stayed inside them (their pages, stores and
    /// effects may differ).
    pub steps_same: usize,
    pub retries: usize,
    pub new_steps: usize,
    pub removed: usize,
    /// Cascades that went cold: their new steps had cost more than the
    /// old steps after them would to run, which were retired at once
    /// ([`go_cold`]).
    pub cold: usize,
    /// Definitions whose value changed, the readers they made dirty, the
    /// outside reads validated, the slots set to the definition reaching
    /// a step and back to their latest, the values taken from the
    /// format's definitions.
    pub defs_changed: usize,
    pub readers_marked: usize,
    /// Dirty steps passed over: each definition that marked them reaches
    /// them at the version they read ([`Dirty`]).
    pub readers_kept: usize,
    pub reads_checked: usize,
    pub positioned: usize,
    pub restored: usize,
    pub initial: usize,
    /// Hits applied in the steps run (7.17.3, "Hits applied inside a step
    /// that runs again"), and the commands they stand for.
    pub applied: u64,
    pub skipped: u64,
    /// Why the rebuild stopped short, if it did: a state it cannot make,
    /// after which only a cold build is sound.
    pub unsupported: Option<&'static str>,
    /// Why it stopped at a step boundary, if it did (its deadline, its
    /// budget of commands, cancelled): the work left is kept ([`pending`]),
    /// and the next [`rebuild`], [`rebuild_trips`] or [`settle`] goes on
    /// with it, merged with any new edit's (DESIGN 3.7, "A rebuild
    /// stopped"). The link waits until none is left.
    pub stopped: Option<&'static str>,
    /// The steps left to run (dirty) when it stopped.
    pub pending: usize,
    /// It began with the work a stopped rebuild left.
    pub resumed: bool,
    pub commands: u64,
    /// The trips run (DESIGN 3.7, "Trips, as built"), each one's steps
    /// run, commands and time (ns, by [`Trips::clock`]); whether the build
    /// converged, and if not, the names whose loads read what the trip
    /// before stored; a line for each outside tool's run.
    pub trips: usize,
    pub trip_steps: Vec<usize>,
    pub trip_commands: Vec<u64>,
    pub trip_ns: Vec<u64>,
    pub settled: bool,
    pub unsettled: Vec<Vec<u8>>,
    pub tools: Vec<alloc::string::String>,
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
            "level {} line {} pos {} loc {} state {}{}{}{}{}{}{}",
            self.in_open,
            self.line,
            pos,
            self.cur.loc - self.cur.start,
            self.cur.state,
            if self.finished { " (finished)" } else { "" },
            if self.fire { " (a fire pending)" } else { "" },
            if self.ship {
                " (before a \\shipout)"
            } else {
                ""
            },
            if self.load {
                " (after a file read whole)"
            } else {
                ""
            },
            if self.page {
                " (the page builder pending)"
            } else {
                ""
            },
            if self.graf {
                " (at a paragraph's start)"
            } else {
                ""
            }
        )
    }
}

/// Whether slot `a` is positioned (7.17.3, "Allocated numbers are not
/// positioned"): not a number the build hands out (the pool's strings
/// and end, the hash's slots and allocators, the glues' lineage), not a
/// read of what is not state (the source, lookups, loads, searches), and
/// not a family whose values are not built yet (the fonts, hyphenation).
pub(super) fn positioned(a: &Slot) -> bool {
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
        | Fam::PageNode
        | Fam::Sealed
        | Fam::PdfObj
        | Fam::PdfName
        | Fam::PdfNum => true,
        Fam::Alloc => ![STR_TOP, HASH_USED, HASH_HIGH, GLUE_LINEAGE]
            .iter()
            .any(|&k| a.1 == i64::from(k)),
        // (a font's identifier, `font_id_text`, §256: the one hash text
        // that changes, each `\font` naming the font sets it, so a step
        // run again reads the one that reaches it, not a later step's)
        Fam::Hash => font_id_slot(a.1),
        _ => false,
    }
}

/// Whether hash slot `p` is a font's identifier (§222's `font_id_base`
/// region).
pub(super) fn font_id_slot(p: i64) -> bool {
    use crate::web::{FONT_ID_BASE, UNDEFINED_CONTROL_SEQUENCE};
    (i64::from(FONT_ID_BASE)..i64::from(UNDEFINED_CONTROL_SEQUENCE)).contains(&p)
}

/// Whether a run's read of `a` is checked against the definitions
/// reaching it: a positioned slot's, or a meaning's class
/// ([`Fam::Class`]), whose value the meaning holds ([`placed_as`]).
pub(super) fn checked(a: &Slot) -> bool {
    positioned(a) || a.0 == Fam::Class
}

/// The slot placed for a read of `a`: a meaning's class
/// ([`Fam::Class`]) is the meaning's (`eqtb[p]`, which a rebuild puts
/// back, the class with it); any other slot itself.
pub(super) fn placed_as(a: &Slot) -> Slot {
    if a.0 == Fam::Class {
        Slot(Fam::Eqtb, a.1)
    } else {
        *a
    }
}

/// Whether a definition at or after `key` defines `a`: the arrays then
/// hold a value the step at `key` must not read.
pub(super) fn later(fold: &Fold<TexSsa>, a: &Slot, key: u64) -> bool {
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
    use crate::track::save::{CUR_BOUNDARY, CUR_GROUP, CUR_LEVEL, ENTRY, SAVE_PTR, XCHAIN, XENTRY};
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
    // (e-TeX's chains: their shape, and each entry within it, as the
    // stack's entries below its pointer)
    let shape = slot(XCHAIN);
    let xlen: usize = if later(&rr.rt.fold, &shape, key) {
        match reaching(tex, rr, shape, key, rep) {
            Some(SVal(_, Some(v))) => match &*v {
                super::SValue::XChain { lens, .. } => lens.iter().map(|&n| n as usize).sum(),
                _ => 0,
            },
            _ => 0,
        }
    } else {
        tex.xregs.chain_lens().iter().map(|&n| n as usize).sum()
    };
    let fold = &rr.rt.fold;
    // (with dead entries not definitions, the arrays can hold a later
    // step's dead write of an entry below the pointer that no later
    // definition shows: each is placed, [`super::DEAD_SAVES`])
    let all = super::DEAD_SAVES.load(core::sync::atomic::Ordering::Relaxed);
    for k in [SAVE_PTR, CUR_LEVEL, CUR_GROUP, CUR_BOUNDARY, XCHAIN] {
        let a = slot(k);
        if later(fold, &a, key) {
            next.push(a);
        }
    }
    for k in (0..u32::try_from(at).unwrap_or(0)).map(|p| ENTRY + p) {
        let a = slot(k);
        if all || later(fold, &a, key) {
            next.push(a);
        }
    }
    // (with the shape placed, every entry within it: the arrays hold
    // the entries of the shape they hold, no others)
    let whole = later(fold, &shape, key);
    for k in (0..u32::try_from(xlen).unwrap_or(0)).map(|i| XENTRY.saturating_add(i)) {
        let a = slot(k);
        if whole || later(fold, &a, key) {
            next.push(a);
        }
    }
}

/// The semantic nest's slots placed whole for the step at `key`, as the
/// save stack's are ([`save_stack_whole`]): `cur_list`'s fields, the nest
/// and the alignment state, where a later definition holds the arrays.
/// They are one structure: a field placed over a nest that was not (the
/// step's prediction read the one and not the other) is a state no run
/// makes, which the run may meet before its reads are checked. (A step
/// that began at the fire `new_graf`'s `build_page` deferred, §1091, was
/// placed in a paragraph's mode over the job's end's empty nest, and
/// `pop_nest` found none.)
fn nest_whole<H: Host>(tex: &Tex<H, SsaTracker>, key: u64, next: &mut Vec<Slot>) {
    use crate::track::{align, list};
    let r = tex.tracker.rec.borrow();
    let fold = &r.rt.fold;
    let nest = Slot(Fam::List, i64::from(list::COUNT));
    // (the nest and the alignment's fields)
    for k in list::COUNT..=list::COUNT + align::COUNT {
        let a = Slot(Fam::List, i64::from(k));
        if later(fold, &a, key) {
            next.push(a);
        }
    }
    // (the nest placed holds its levels' fields as they were when it was
    // made: each field goes after it, at its own definition, as deep as
    // the nest placed goes)
    let placed = later(fold, &nest, key);
    let depth = if placed {
        fold.reaching(&nest, key).map_or(0, |d| {
            r.rt.writes(d.rec)
                .get(d.ix as usize)
                .and_then(|(_, v)| nest_depth(v.as_ref()?))
                .unwrap_or(tex.max_nest_stack + 1)
        })
    } else {
        tex.nest.len()
    };
    for a in level_fields(depth) {
        if placed || later(fold, &a, key) {
            next.push(a);
        }
    }
}

/// Whether slot `a` is the PDF object lists' heads (`OBJS`) or what the
/// writer keeps of the fonts (`PDF_FONTS`), which say which objects the
/// lists hold.
fn object_table(a: &Slot) -> bool {
    use crate::pdf::val::field::{OBJS, PDF_FONTS};
    a.0 == Fam::Pdf && (a.1 == i64::from(OBJS) || a.1 == i64::from(PDF_FONTS))
}

/// The PDF object lists placed whole for the step at `key`, as the
/// nest's levels go with the nest (DESIGN 3.12, "PDF object numbers"):
/// with the lists' heads or the fonts' state placed, the heads, the
/// fonts' state and each entry on a list from the heads that reach the
/// step, followed by the links that reach it, where a later definition
/// holds the arrays. A head placed and its entries not would be a list
/// no run made: the entries a later run left link on to objects this
/// run has not made yet; a font the fonts' state says is not used, made
/// again, was pushed on the list it was on, its link pointing back into
/// it, and the next font's walk (`pdf_init_font`) went round for ever
/// (the thesis's cold trip 2). Added to `next`. (An entry read by its
/// number, off the lists, is checked as any read.)
fn objs_whole<H: Host>(
    tex: &Tex<H, SsaTracker>,
    key: u64,
    next: &mut Vec<Slot>,
    rep: &mut RebuildReport,
) {
    use crate::pdf::objtab::{Entry, HEAD_TAB_MAX};
    use crate::pdf::val::field::{OBJS, PDF_FONTS};
    type Objs = (crate::pdf::val::VTab<Entry>, [i32; HEAD_TAB_MAX + 1], i32);
    let objs = &tex.pdf.objs;
    let mut r = tex.tracker.rec.borrow_mut();
    let rr = &mut *r;
    let field = |v: &Option<SVal>| match v.as_ref().and_then(|v| v.1.as_deref()) {
        Some(super::SValue::Field(f)) => Some(f.clone()),
        _ => None,
    };
    let heads = Slot(Fam::Pdf, i64::from(OBJS));
    let fonts = Slot(Fam::Pdf, i64::from(PDF_FONTS));
    if later(&rr.rt.fold, &fonts, key) {
        next.push(fonts);
    }
    let head = if later(&rr.rt.fold, &heads, key) {
        next.push(heads);
        let v = reaching(tex, rr, heads, key, rep);
        field(&v).and_then(|f| f.get::<Objs>().map(|o| o.1))
    } else {
        None
    }
    .unwrap_or(objs.head);
    if !objs.ssa.on {
        // (the entries are the `OBJS` field's)
        return;
    }
    for mut k in head {
        // (as many as there are entries: links from runs that do not
        // agree may go round)
        let mut left = objs.len();
        while k != 0 && left > 0 {
            left -= 1;
            let a = Slot(Fam::PdfObj, i64::from(k));
            k = if later(&rr.rt.fold, &a, key) {
                next.push(a);
                let v = reaching(tex, rr, a, key, rep);
                field(&v)
                    .and_then(|f| f.get::<Option<Entry>>().cloned())
                    .flatten()
                    .map_or(0, |e| e.link)
            } else {
                objs.entry(k).map_or(0, |e| e.link)
            };
        }
    }
}

/// The fields (`track::list::slot`) of the levels from the outermost to
/// depth `d`.
fn level_fields(d: usize) -> impl Iterator<Item = Slot> {
    use crate::track::list;
    (0..=d)
        .flat_map(|d| (0..list::COUNT).map(move |f| Slot(Fam::List, i64::from(list::slot(d, f)))))
}

/// The depth of a value of the nest.
fn nest_depth(v: &SVal) -> Option<usize> {
    match v.1.as_deref()? {
        super::SValue::Nest(n) => Some(n.len()),
        _ => None,
    }
}

/// The values `value` gives `slots`, and with the nest among them, its
/// levels' fields too: the levels it holds are as they were when it was
/// made, and `cur_list` as a run left it. As deep as the nest put back
/// goes (the arrays' if it is not), a deeper level having no place.
fn with_levels<H: Host>(
    tex: &Tex<H, SsaTracker>,
    slots: impl IntoIterator<Item = Slot>,
    mut value: impl FnMut(Slot) -> Option<SVal>,
) -> Vec<(Slot, SVal)> {
    use crate::track::list;
    use crate::track::save::{XCHAIN, XENTRY};
    let nest = Slot(Fam::List, i64::from(list::COUNT));
    let field = |a: &Slot| {
        a.0 == Fam::List && (a.1 < i64::from(list::COUNT) || a.1 >= i64::from(list::STRIDE))
    };
    let shape = Slot(Fam::Save, i64::from(XCHAIN));
    let entry = |a: &Slot| a.0 == Fam::Save && a.1 >= i64::from(XENTRY);
    let mut vals = Vec::new();
    let mut done = BTreeSet::new();
    let (mut with_nest, mut with_shape) = (false, false);
    for a in slots {
        with_nest |= a == nest;
        with_shape |= a == shape;
        if field(&a) || entry(&a) {
            done.insert(a);
        }
        if let Some(v) = value(a) {
            vals.push((a, v));
        }
    }
    if with_shape {
        // (e-TeX's chains put back hold the entries their shape says, each
        // at its own definition: the entries a shape put before dropped
        // or left stand-ins are put with it)
        let n: usize = vals
            .iter()
            .find(|(a, _)| *a == shape)
            .and_then(|(_, v)| match v.1.as_deref() {
                Some(super::SValue::XChain { lens, .. }) => {
                    Some(lens.iter().map(|&n| n as usize).sum())
                }
                _ => None,
            })
            .unwrap_or_else(|| tex.xregs.chain_lens().iter().map(|&n| n as usize).sum());
        for i in 0..u32::try_from(n).unwrap_or(0) {
            let a = Slot(Fam::Save, i64::from(XENTRY.saturating_add(i)));
            if !done.contains(&a)
                && let Some(v) = value(a)
            {
                vals.push((a, v));
            }
        }
    }
    if with_nest {
        let depth = vals
            .iter()
            .find(|(a, _)| *a == nest)
            .map_or(tex.nest.len(), |(_, v)| {
                nest_depth(v).unwrap_or(tex.max_nest_stack + 1)
            });
        for a in level_fields(depth) {
            if !done.contains(&a)
                && let Some(v) = value(a)
            {
                vals.push((a, v));
            }
        }
    }
    vals
}

/// What else says where the engine is, placed whole for the window at
/// `key` beside the nest ([`nest_whole`]): the conditionals and
/// `align_state` (DESIGN 4.3 item 1, "Re-entry"). A window may begin in a
/// conditional's arm or an alignment's entry, where a run that met
/// another `\fi` or `&` than its last one's before its reads are checked
/// would be in a state no run makes.
fn window_whole<H: Host>(tex: &Tex<H, SsaTracker>, key: u64, next: &mut Vec<Slot>) {
    use crate::track::scalar;
    let r = tex.tracker.rec.borrow();
    let fold = &r.rt.fold;
    for a in [
        Slot(Fam::Cond, 0),
        Slot(Fam::Alloc, i64::from(scalar::ALIGN_STATE)),
    ] {
        if later(fold, &a, key) {
            next.push(a);
        }
    }
}

/// A step's definitions: each slot its records wrote, at its last write.
/// The writes of records `recs` (the open step's so far), each slot's
/// last.
fn recs_writes(
    rt: &partex_ssa::Runtime<TexSsa>,
    recs: &[partex_ssa::runtime::RecId],
) -> BTreeMap<Slot, Version> {
    let mut d = BTreeMap::new();
    for &r in recs {
        for (a, v) in rt.writes(r) {
            d.insert(*a, v.as_ref().map_or(Version::ABSENT, |v| v.0));
        }
    }
    d
}

/// Step `s`'s definitions, as the index holds them.
pub(super) fn defs(
    rt: &partex_ssa::Runtime<TexSsa>,
    s: partex_ssa::fold::StepId,
) -> BTreeMap<Slot, Version> {
    let mut d = BTreeMap::new();
    let Some(step) = rt.fold.steps.get(s as usize) else {
        return d;
    };
    for &r in &step.recs {
        for (a, v) in rt.writes(r) {
            // (a slot the step left as it found it is not its definition)
            if rt.fold.defines(a, s) {
                d.insert(*a, v.as_ref().map_or(Version::ABSENT, |v| v.0));
            }
        }
    }
    d
}

/// The version of each font's glyphs used as the ships' latest rows make
/// it (`pdf::ship::glyph_union`, as the job's end makes it); `None` if a
/// row's value is not kept. Counted from the rows that changed since the
/// last time ([`GlyphCount`]): an edit changes a ship or two of hundreds.
fn glyph_union_now(rr: &mut Recorder) -> Option<u128> {
    let (rt, steps) = (&rr.rt, &mut rr.st.steps);
    let count = &mut steps.glyph_count;
    let dirty = core::mem::take(&mut steps.glyph_dirty);
    // (row `n` as the latest definition has it: `Some(None)`, none)
    let row = |n: usize| -> Option<Option<&crate::pdf::ship::Glyphs>> {
        let Some(d) = rt.fold.latest(&Slot(Fam::Glyphs, i64::try_from(n).ok()?)) else {
            return Some(None);
        };
        let (_, v) = rt.writes(d.rec).get(d.ix as usize)?;
        let super::SValue::Field(f) = &**v.as_ref()?.1.as_ref()? else {
            return None;
        };
        Some(Some(f.get::<crate::pdf::ship::Glyphs>()?))
    };
    // (the rows changed, if they are rows counted that are still there;
    // else every row again, up to the first not there)
    let counted = (|| -> Option<()> {
        let mut walk = count.rows.is_empty()
            || dirty
                .iter()
                .any(|&n| usize::try_from(n).map_or(true, |n| n >= count.rows.len()));
        if !walk {
            for &n in &dirty {
                let n = usize::try_from(n).ok()?;
                let Some(r) = row(n)? else {
                    walk = true;
                    break;
                };
                count.set(n, r);
            }
        }
        if walk {
            let mut n = 0;
            while let Some(r) = row(n)? {
                count.set(n, r);
                n += 1;
            }
            count.truncate(n);
        }
        Some(())
    })();
    if counted.is_none() {
        // (a row's value is not kept: counted from none next time, every
        // row again)
        count.truncate(0);
        return None;
    }
    let v = count.version();
    debug_assert_eq!(
        v,
        { Version::of(&crate::pdf::ship::glyph_union(count.rows.iter())).0 },
        "the glyph union counted is the union"
    );
    Some(v)
}

/// The ships' glyph rows as counted: each row, and for each font, how
/// many rows have it, how many have each of its glyphs, and the glyphs
/// some row has. The union (`pdf::ship::glyph_union`) is each font some
/// row has, with the glyphs some row has; a row changed is taken out and
/// the new one put in.
#[derive(Default)]
pub(crate) struct GlyphCount {
    rows: Vec<crate::pdf::ship::Glyphs>,
    fonts: BTreeMap<i32, FontCount>,
    version: Option<u128>,
}

/// A font's count in [`GlyphCount`]: rows that have it, rows that have
/// each glyph, and the glyphs some row has.
type FontCount = (u32, alloc::boxed::Box<[u32; 256]>, [u64; 4]);

impl GlyphCount {
    /// Row `row` counted in (`up`) or out.
    fn count(&mut self, row: &crate::pdf::ship::Glyphs, up: bool) {
        for (f, bits) in row.iter() {
            let (k, c, union) = self
                .fonts
                .entry(*f)
                .or_insert_with(|| (0, alloc::boxed::Box::new([0; 256]), [0; 4]));
            *k = if up { *k + 1 } else { k.saturating_sub(1) };
            for (w, word) in bits.iter().enumerate() {
                let mut x = *word;
                while x != 0 {
                    let b = x.trailing_zeros();
                    let g = w * 64 + b as usize;
                    c[g] = if up { c[g] + 1 } else { c[g].saturating_sub(1) };
                    if c[g] == 0 {
                        union[w] &= !(1 << b);
                    } else {
                        union[w] |= 1 << b;
                    }
                    x &= x - 1;
                }
            }
        }
        self.version = None;
    }

    /// Row `n` is `row` (`n` at most the count of rows).
    fn set(&mut self, n: usize, row: &crate::pdf::ship::Glyphs) {
        match self.rows.get(n) {
            Some(old) if Arc::ptr_eq(old, row) => return,
            Some(old) => {
                let old = old.clone();
                self.count(&old, false);
                self.rows[n] = row.clone();
            }
            None => self.rows.push(row.clone()),
        }
        self.count(row, true);
    }

    /// The rows from `n` on are gone.
    fn truncate(&mut self, n: usize) {
        while self.rows.len() > n {
            if let Some(r) = self.rows.pop() {
                self.count(&r, false);
            }
        }
    }

    /// The union's version, as `pdf::ship::glyphs_union` makes it.
    fn version(&mut self) -> u128 {
        let fonts = &self.fonts;
        *self.version.get_or_insert_with(|| {
            let sets: BTreeMap<i32, [u64; 4]> = fonts
                .iter()
                .filter(|(_, (k, ..))| *k > 0)
                .map(|(f, (_, _, union))| (*f, *union))
                .collect();
            Version::of(&sets).0
        })
    }
}

/// The version of the definition of `a` that reaches `key` (a step's
/// definitions' versions are those [`defs`] gives; `None`: no step before
/// `key` defines it, and its version, the format's, is not kept).
pub(super) fn reaching_version(rr: &Recorder, a: &Slot, key: u64) -> Option<Version> {
    let d = rr.rt.fold.reaching(a, key)?;
    let (_, v) = rr.rt.writes(d.rec).get(d.ix as usize)?;
    Some(v.as_ref().map_or(Version::ABSENT, |v| v.0))
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
    let d = rr.rt.fold.reaching(&a, key);
    value_of(tex, rr, a, d, rep)
}

/// The value definition `d` of `a` made (`None`: the format's).
fn value_of<H: Host>(
    tex: &Tex<H, SsaTracker>,
    rr: &mut Recorder,
    a: Slot,
    d: Option<Def>,
    rep: &mut RebuildReport,
) -> Option<SVal> {
    if let Some(d) = d {
        rr.rt.writes(d.rec).get(d.ix as usize)?.1.clone()
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
        Some(d) => rr.rt.writes(d.rec).get(d.ix as usize)?.1.clone(),
        None => {
            rep.initial += 1;
            initial(tex, &mut rr.st.steps, a)
        }
    }
}

/// What slot `a` holds before any step defines it: the format's
/// definition (DESIGN 7.17.3, "Not built": the first step's definitions
/// are the format's).
pub(super) fn initial<H: Host>(
    tex: &Tex<H, SsaTracker>,
    steps: &mut Steps,
    a: Slot,
) -> Option<SVal> {
    if steps.format.is_none() {
        let mut f = Tex::format_value(tex.params.clone(), tex.format_data.as_deref())?;
        // (as the build set it up before its first step: with effects on,
        // the PDF writer's object streams are symbolic, `Tex::set_effects`;
        // the first page shipped again must find the writer the first
        // build's did, or every later page's writer differs)
        f.pdf.out.symbolic = tex.pdf.out.symbolic;
        // (and with virtual object numbers, its numbers are relocations)
        f.pdf.out.virt = tex.pdf.out.virt;
        f.pdf.out.font_refs = tex.pdf.out.font_refs;
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
pub(super) fn put<H: Host>(tex: &mut Tex<H, SsaTracker>, vals: &[(Slot, SVal)]) {
    if vals.is_empty() {
        return;
    }
    let mut vers: Versions = core::mem::take(&mut tex.tracker.rec.borrow_mut().st.vers);
    // (the page's length, then its tail, before its nodes: the list is
    // made that long with stand-ins, and the nodes read put in place,
    // 7.17.3 item 5; and the nest, whole, before its levels' fields: they
    // are put over the levels it holds)
    let rank = |a: &Slot| match (a.0, u8::try_from(a.1)) {
        (Fam::Page, Ok(crate::track::page::LIST_LEN))
        | (Fam::List, Ok(crate::track::list::COUNT)) => 0,
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

/// Make ready what the first rebuild would make: the format's
/// definitions, which a slot no step defines takes ([`initial`]),
/// decoded from the format (tens of milliseconds a first keystroke
/// would wait for). A build's end can do it after its output is out.
pub fn prepare_rebuilds<H: Host>(tex: &Tex<H, SsaTracker>) {
    let mut r = tex.tracker.rec.borrow_mut();
    let _ = initial(tex, &mut r.st.steps, Slot(Fam::Eqtb, 0));
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
    // (the skips remembered start afresh: the state a rebuild places is
    // not the one they were made in)
    tex.skip.epoch += 1;
    // (a rebuild past its budget stops: `SsaTracker::budget`)
    let budget_from = c0;
    // (the work a stopped rebuild left, taken up: its dirty steps, and
    // the φ its trip served, DESIGN 3.7, "A rebuild stopped")
    let pending = tex.tracker.rec.borrow_mut().st.steps.pending.take();
    rep.resumed = pending.is_some();
    let (mut dirty, pending_phi) = match pending {
        Some(p) => (p.dirty, Some(p.phi)),
        None => (Dirty::default(), None),
    };
    // (a build's trip after the first: its φ is what the trip before
    // stored, DESIGN 3.7, "Trips, as built")
    let next = tex.tracker.rec.borrow_mut().st.steps.next.take();
    {
        // the files loaded, as they are now: a file read by lines changed
        // where its lines did, in each data it was loaded as (7.17.3,
        // "Lines are of a data"); a load reads whether the file is there,
        // and a file read whole reads its contents (each looked up as the
        // kind of file it was loaded as)
        // (the loads the host knows are as they were are not made again,
        // 7.17.3, "A rebuild's file checks cost the files that changed":
        // looked at by reference, and only the others kept, with what is
        // read of them after)
        // (the stored names whose files are as the build's link left them,
        // with the build's values of them)
        let (own, own_phi): (BTreeSet<u32>, Vec<(u32, Stream)>) = if next.is_none() {
            let names: Vec<(u32, Vec<u8>)> = {
                let r = tex.tracker.rec.borrow();
                let s = &r.st.steps;
                s.stored
                    .iter()
                    .filter_map(|&id| Some((id, r.st.loads.get(id as usize)?.0.clone())))
                    .collect()
            };
            let mut own = BTreeSet::new();
            let mut vals = Vec::new();
            for (id, name) in names {
                if tex.host.output_edited(&name) {
                    continue;
                }
                let r = tex.tracker.rec.borrow();
                let s = &r.st.steps;
                // (the last complete trip's, if the trips since ended
                // fatally and withheld their stores)
                let v = match (
                    s.complete.as_ref().and_then(|c| c.get(&id)),
                    s.value_at(&r.rt.fold, id, u64::MAX),
                ) {
                    (Some(v), _) => v.clone(),
                    (None, Some(Ok(v))) => v.map(Arc::<[u8]>::from), // (a command removed it: none)
                    (None, _) => s.last_phi.get(&id).cloned().flatten(),
                };
                own.insert(id);
                vals.push((id, v));
            }
            (own, vals)
        } else {
            (BTreeSet::new(), Vec::new())
        };
        #[allow(clippy::type_complexity)]
        let (files, kept, same, datas, lines): (
            Vec<(u32, Vec<u8>, FileKind, Option<Arc<[u8]>>, bool, bool, bool)>,
            Vec<(u32, Option<Arc<[u8]>>)>,
            usize,
            Vec<(u32, u32, Arc<[u8]>)>,
            BTreeSet<u32>,
        ) = {
            let r = tex.tracker.rec.borrow();
            let s = &r.st.steps;
            let lines: BTreeSet<u32> = s
                .datas
                .iter()
                .filter(|d| !d.lines.is_empty())
                .map(|d| d.name)
                .collect();
            // (a later trip: a stored name's φ is in memory, and the other
            // loads are looked at again only after a tool wrote a file; a
            // build's first trip: a stored name's φ is the build's own, what
            // its link wrote, unless the host says the file was edited
            // since, DESIGN 3.7, "Files are a view")
            let looked: Vec<(u32, &[u8], FileKind, Option<&Arc<[u8]>>)> =
                r.st.loads
                    .iter()
                    .enumerate()
                    .filter_map(|(i, (name, _, kind))| {
                        let id = u32::try_from(i).ok()?;
                        if next
                            .as_ref()
                            .is_some_and(|n| !n.files || s.stored.contains(&id))
                        {
                            return None;
                        }
                        if next.is_none() && own.contains(&id) {
                            return None;
                        }
                        Some((
                            id,
                            &name[..],
                            *kind,
                            s.files.get(i).and_then(Option::as_ref),
                        ))
                    })
                    .collect();
            let checks = {
                let loads: Vec<_> = looked.iter().map(|&(_, n, k, o)| (n, k, o)).collect();
                tex.host.unchanged(&loads)
            };
            let (mut files, mut kept, mut same) = (Vec::new(), Vec::new(), 0);
            for (&(id, name, kind, old), unchanged) in looked.iter().zip(checks) {
                let stored = s.stored.contains(&id);
                if unchanged {
                    same += 1;
                    if stored {
                        kept.push((id, old.cloned()));
                    }
                } else {
                    files.push((
                        id,
                        name.to_vec(),
                        kind,
                        old.cloned(),
                        lines.contains(&id),
                        stored,
                        s.whole.contains(&id),
                    ));
                }
            }
            // (the data found in the files, not served from the stores)
            let datas = s
                .datas
                .iter()
                .enumerate()
                .filter(|(_, d)| {
                    d.file
                        && lines.contains(&d.name)
                        && !(d.superseded && d.lines.is_empty() && d.numbers.is_empty())
                })
                .filter_map(|(i, d)| Some((u32::try_from(i).ok()?, d.name, d.bytes.clone())))
                .collect();
            (files, kept, same, datas, lines)
        };
        let mut edits = Vec::new();
        let mut loads = Vec::new();
        // (a name the job stores: its φ, what the last trip stored, which
        // its file holds, 7.17.3's "A load reads the store")
        let mut phi: BTreeMap<u32, Option<Arc<[u8]>>> = kept.into_iter().collect();
        phi.extend(own_phi);
        // (each file read by lines, as it is now)
        let mut nows: BTreeMap<u32, Arc<[u8]>> = BTreeMap::new();
        let mut read = 0;
        // (a file read by lines that no step read a line of, nor met its
        // end: only its being there was read, `\IfFileExists`; its
        // contents now, for the next check)
        let mut there: Vec<(u32, Arc<[u8]>)> = Vec::new();
        for (id, name, kind, old, lines, stored, whole) in files {
            read += 1;
            let now = tex.host.read_file(&name, kind).map(|f| f.contents);
            match (&old, &now) {
                (Some(o), Some(n)) if lines => {
                    nows.insert(id, n.clone());
                    // (and read whole somewhere: those readers compare it)
                    if whole && o[..] != n[..] {
                        loads.push((id, now.clone()));
                    }
                }
                (Some(old), Some(now)) if old[..] == now[..] => {}
                (None, None) => {}
                _ if stored => {}
                (Some(_), Some(n)) if !whole => there.push((id, n.clone())),
                _ => loads.push((id, now.clone())),
            }
            if stored {
                phi.insert(id, now);
            }
        }
        // (a later trip: each stored name's φ is what the trip before
        // stored, and a name read by lines whose φ changed is edited)
        let later = next.is_some();
        if let Some(n) = next {
            for (id, now) in n.phi {
                if let Some(c) = &now
                    && n.changed.contains(&id)
                    && lines.contains(&id)
                {
                    nows.insert(id, c.clone());
                }
                phi.insert(id, now);
            }
        }
        // (a stopped trip's φ: the files hold the one before it, as the
        // link waited)
        if let Some(p) = pending_phi {
            phi.extend(p);
        }
        if trace {
            note(
                tex,
                alloc::format!("loads: {same} as they were by their stamps, {read} made again"),
            );
            if rep.resumed {
                note(
                    tex,
                    alloc::format!("resumed: {} steps a stopped rebuild left", dirty.len()),
                );
            }
        }
        for (d, name, old) in datas {
            if let Some(e) = nows.get(&name).and_then(|now| Edit::diff(d, &old, now)) {
                edits.push(e);
            }
        }
        // the queries the steps asked, asked again: a step whose answer
        // differs is dirty (one answer per query, for the whole trip; a
        // build's later trips ask none: the first asked for the build)
        let asked: Vec<(StepId, u64, Query, u128)> = if later {
            Vec::new()
        } else {
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
            if now != Some(a) && dirty.mark(key, s) {
                rep.queries += 1;
                if trace {
                    note(tex, alloc::format!("seed: step {s} asked {q:?}"));
                }
            }
        }
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        // (the files only tested for: as they are now, for the next check)
        for (id, now) in there {
            let i = id as usize;
            if trace {
                let name = rr.st.loads.get(i).map_or(&b""[..], |l| &l.0[..]);
                let n = alloc::format!(
                    "load {id} ({}): changed, read for its being there only",
                    alloc::string::String::from_utf8_lossy(name)
                );
                rr.st.steps.log.push(n);
            }
            if let Some(l) = rr.st.loads.get_mut(i) {
                l.1 = Version::of(&now[..]);
            }
            if let Some(f) = rr.st.steps.files.get_mut(i) {
                *f = Some(now);
            }
        }
        // the loads that read the φ, dirty if it is not what they found
        for (key, s, id) in rr.st.steps.phi_seeds(&rr.rt.fold, &phi) {
            rep.phi += 1;
            dirty.mark(key, s);
            if trace {
                let n = alloc::format!("seed: step {s} loaded φ {id}");
                rr.st.steps.log.push(n);
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
                dirty.mark(rr.rt.fold.steps[x as usize].key, x);
                if trace {
                    let name = rr.st.loads.get(id as usize).map(|l| &l.0[..]);
                    let n = alloc::format!(
                        "seed: step {x} loaded {id} ({})",
                        alloc::string::String::from_utf8_lossy(name.unwrap_or_default())
                    );
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
            rr.st.steps.datas[e.data as usize].superseded = true;
            if let Some(l) = rr.st.loads.get_mut(name as usize) {
                l.1 = Version::of(&e.new[..]);
            }
            rr.st.steps.edits.push(e);
        }
        rep.seeds = dirty.len();
        if trace {
            let l = alloc::format!(
                "seeds (step, key): {:?}; edits {:?}",
                dirty.steps().collect::<Vec<_>>(),
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
    // (the steps' runs on workers, waiting for their turns: `par.rs`)
    let speculating = may_speculate(tex);
    #[cfg(not(feature = "std"))]
    let _ = speculating;
    let mut spec = Spec::default();
    while let Some((j, why)) = dirty.pop_first() {
        let (prev, mut target, whyn) = {
            let r = tex.tracker.rec.borrow();
            let fold = &r.rt.fold;
            if !fold.steps[j as usize].live {
                continue;
            }
            // (the trace: why it runs, the slots whose definition reaching
            // it is not the version it read, or a mark)
            let whyn = rep.trace.then(|| match &why {
                Some(reads) => {
                    let key = fold.steps[j as usize].key;
                    reads
                        .iter()
                        .filter(|(a, v)| reaching_version(&r, a, key) != Some(*v))
                        .take(8)
                        .map(|(a, _)| {
                            alloc::format!("{a}={}", super::view::trace_name(tex, &r.st, *a))
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                }
                None => alloc::string::String::from("a mark"),
            });
            // (marked by definitions that changed, each reaching it at the
            // version it read again: its reads are as they were)
            if let Some(reads) = why {
                let key = fold.steps[j as usize].key;
                if reads
                    .iter()
                    .all(|(a, v)| reaching_version(&r, a, key) == Some(*v))
                {
                    rep.readers_kept += 1;
                    if rep.trace {
                        drop(r);
                        note(
                            tex,
                            alloc::format!(
                                "step {j} kept: each definition that marked it reaches it as it read it"
                            ),
                        );
                    }
                    continue;
                }
            }
            let Some(pos) = fold.position(j) else {
                continue;
            };
            if pos == 0 {
                rep.unsupported = Some("an edit in the job's first step (its start)");
                break;
            }
            (fold.order[pos - 1], r.st.steps.end(j), whyn)
        };
        if let Some(w) = whyn {
            note(tex, alloc::format!("step {j} runs for: {w}"));
        }
        let Some(mut input) = tex.tracker.rec.borrow().st.steps.end(prev) else {
            rep.unsupported = Some("a step with no result");
            break;
        };
        // (the step whose old end the runs try to meet, and that end)
        let cursor = j;
        let mut cur = j;
        let mut predict = alloc::vec![j];
        let mut writes: Vec<StepId> = Vec::new();
        // (with windows, the old steps after it as they were: the k-th new
        // step of a run that ended elsewhere, a window about as long as an
        // old one, runs about the text the k-th of them ran, and is
        // predicted by it and its neighbours too, DESIGN 4.3 item 1)
        let ahead: Vec<StepId> = if tex.window() > 0 {
            let r = tex.tracker.rec.borrow();
            let fold = &r.rt.fold;
            fold.position(j).map_or_else(Vec::new, |p| {
                fold.order[p + 1..]
                    .iter()
                    .take(CASCADE_AHEAD)
                    .copied()
                    .collect()
            })
        } else {
            Vec::new()
        };
        // (the old step about where the run is, in `ahead`: one further
        // for each new step, or where a dropped run read)
        let mut at = 0usize;
        // (what the cascade's runs cost, in commands, a slot placed or put
        // back as half of one; the old steps after it, their last runs'
        // commands, looked at once that passes `COLD_FLOOR`)
        let mut spent = 0u64;
        let mut after: Option<u64> = None;
        // (where the old steps after it ended, made once a run of the
        // cascade meets none of the next `LOOK_AHEAD`: `meet_far`)
        let mut ends: Option<Ends> = None;
        // (its run on a worker, if a round made one: DESIGN 3.10)
        #[cfg(feature = "std")]
        let mut worker_run = if speculating {
            speculate(tex, j, &dirty, &mut rep, &mut spec);
            spec.ran.remove(&j)
        } else {
            None
        };
        loop {
            let c0 = tex.commands();
            let moved = rep.positioned + rep.restored;
            #[cfg(feature = "std")]
            let taken = match worker_run.take() {
                Some(a) => match commit(tex, j, prev, a, &mut dirty, &mut rep) {
                    Ok(e) => {
                        if rep.trace {
                            note(tex, alloc::format!("step {j} taken from a worker's run"));
                        }
                        Some(e)
                    }
                    Err((why, c)) => {
                        if rep.trace {
                            note(
                                tex,
                                alloc::format!("step {j}: its worker's run not taken: {why}"),
                            );
                        }
                        tex.tracker.par.stats.borrow_mut().not_taken(why, c);
                        None
                    }
                },
                None => None,
            };
            #[cfg(not(feature = "std"))]
            let taken: Option<InputState> = None;
            let end = match taken {
                Some(e) => e,
                None => run_step(
                    tex, cur, &predict, &writes, &input, &mut dirty, &mut rep, &mut srep,
                ),
            };
            spent += tex.commands() - c0 + ((rep.positioned + rep.restored - moved) / 2) as u64;
            // (past its budget, its deadline, or cancelled: it stops once
            // this step's run is placed, below, its work kept, DESIGN 3.7,
            // "A rebuild stopped")
            if tex.commands() - budget_from > tex.tracker.budget.get() {
                rep.stopped = Some("a rebuild past its budget of commands");
            } else if let Some((clock, end)) = tex.tracker.deadline.get()
                && clock() > end
            {
                rep.stopped = Some("a rebuild past its deadline");
            } else if tex.tracker.cancel.get().is_some_and(|c| c()) {
                rep.stopped = Some("a rebuild cancelled");
                rep.cancelled = true;
            }
            if let Some(s) = dirty.anchor.take()
                && let Some(i) = ahead.iter().position(|&o| o == s)
            {
                at = i;
            }
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
            // passes over the steps between, or it runs on; the next
            // `LOOK_AHEAD` first, then any)
            let met = meet(tex, cur, &end).or_else(|| meet_far(tex, cur, &end, &mut ends));
            if let Some((m, old_end)) = met {
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
                && !input.ship
                && !input.load
                && !input.page
                && !input.graf
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
            // (stopped in a cascade that runs on: the old step after it
            // runs again from where this one ended, as when an input
            // changed; with none after it, it goes on)
            if rep.stopped.is_some() {
                if mark_next(tex, cur, &mut dirty) {
                    break;
                }
                rep.stopped = None;
            }
            // (a cascade that has cost more than running the rest of the
            // job would: the old steps after it go at once, and the rest
            // runs as a cold build does, each new step the fold's last)
            if spent > COLD_FLOOR && spent > *after.get_or_insert_with(|| commands_after(tex, cur))
            {
                go_cold(tex, cur, &mut dirty, &mut rep);
                spec.clear(tex);
                after = Some(u64::MAX);
                // (and it runs to the job's end: the old step's end it was
                // to meet is gone with the steps after it; a run coming
                // back to that place, a chapter `\include`d again ending
                // where its `\include` had ended without it, stopped the
                // cascade there, and the job's writes after it were lost)
                target = None;
            }
            let next = {
                let mut r = tex.tracker.rec.borrow_mut();
                let renumbered = r.rt.fold.renumbered;
                // (the first new step after the step run again is put near
                // the next one, `Fold::insert_after`)
                let Some(n) = r.rt.fold.insert_after(cur, cur == j) else {
                    rep.unsupported = Some("a step not in the fold");
                    break;
                };
                let fold = &r.rt.fold;
                if fold.renumbered != renumbered {
                    // (the keys were made again: the dirty steps' too)
                    dirty.rekey(fold);
                }
                // (a new step's reads are predicted by the old step whose
                // text it runs, and by the step just run: text the old run
                // never read, as a table of contents read for the first
                // time, reads what the step before it read)
                let old = fold
                    .position(n)
                    .and_then(|p| fold.order.get(p + 1).copied())
                    .unwrap_or(cursor);
                predict = alloc::vec![old, cur];
                writes.clear();
                for &o in ahead.iter().skip(at).take(3) {
                    if !predict.contains(&o) {
                        predict.push(o);
                    }
                    writes.push(o);
                }
                at += 1;
                n
            };
            rep.new_steps += 1;
            cur = next;
            input = end;
        }
        if rep.unsupported.is_some() || rep.stopped.is_some() {
            break;
        }
    }
    spec.clear(tex);
    if rep.stopped.is_some() && rep.unsupported.is_none() && !dirty.is_empty() {
        // (the work left: the next rebuild takes it up)
        dirty.anchor = None;
        rep.pending = dirty.len();
        if rep.trace {
            note(
                tex,
                alloc::format!(
                    "stopped ({}): {} steps pending",
                    rep.stopped.unwrap_or_default(),
                    rep.pending
                ),
            );
        }
        let mut r = tex.tracker.rec.borrow_mut();
        let s = &mut r.st.steps;
        let phi = s.phi.clone().unwrap_or_default();
        s.pending = Some(Pending { dirty, phi });
    } else {
        // (a stop with nothing left is a trip that ended)
        rep.stopped = None;
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
    {
        // (the φ this trip read, kept for the next trip's names that no
        // live step opens)
        let mut r = tex.tracker.rec.borrow_mut();
        let s = &mut r.st.steps;
        if let Some(p) = s.phi.take() {
            s.last_phi = p;
        }
        // (the steps this rebuild passed over: what they read and made)
        r.rt.fold.release_removed();
    }
    // (a trip that reached the job's end, normally or fatally: a stopped
    // one is not a trip that ended, and keeps what was published)
    if rep.stopped.is_none() && rep.unsupported.is_none() {
        trip_ended(tex, false);
    }
    rep
}

/// A trip ended (DESIGN 3.7, "A trip that ended fatally"): with
/// [`SsaTracker::keep_complete`] and the job ended fatally, its stores are
/// withheld and the last complete trip's values stand: if this is the
/// first trip since one that was complete, each name's φ the trip read
/// (a `cold` build's, the host's file, which no build writes before its
/// link; else its own [`Steps::last_phi`]). Else its stores are the
/// streams.
pub(crate) fn trip_ended<H: Host>(tex: &mut Tex<H, SsaTracker>, cold: bool) {
    let fatal = tex.history == crate::error::FATAL_ERROR_STOP;
    if !(fatal && tex.tracker.keep_complete.get()) {
        tex.tracker.rec.borrow_mut().st.steps.complete = None;
        return;
    }
    if tex.tracker.rec.borrow().st.steps.complete.is_some() {
        return;
    }
    let mut c = BTreeMap::new();
    if cold {
        let names: Vec<(u32, Vec<u8>, FileKind)> = {
            let r = tex.tracker.rec.borrow();
            r.st.steps
                .stored
                .iter()
                .filter_map(|&id| {
                    let l = r.st.loads.get(id as usize)?;
                    Some((id, l.0.clone(), l.2))
                })
                .collect()
        };
        for (id, name, kind) in names {
            c.insert(id, tex.host.read_file(&name, kind).map(|f| f.contents));
        }
    } else {
        c = tex.tracker.rec.borrow().st.steps.last_phi.clone();
    }
    tex.tracker.rec.borrow_mut().st.steps.complete = Some(c);
}

/// A stream's bytes, or none (no file).
pub type Stream = Option<Arc<[u8]>>;

/// A producer outside the steps (a tool between trips, a node that makes
/// a stream: DESIGN 3.7, "Files are a view") defines stream `name` as
/// `bytes` (`None`: no file). Loads of `name` read it as they read a
/// stored name's φ: from the next trip, or the next rebuild, on, and
/// only its loads whose value differs run again (`Steps::phi_seeds`); the
/// link writes it ([`produced_streams`]). Whether its value changed.
pub fn define_stream<H: Host>(
    tex: &Tex<H, SsaTracker>,
    name: &[u8],
    bytes: Option<Arc<[u8]>>,
) -> bool {
    let mut r = tex.tracker.rec.borrow_mut();
    let rr = &mut *r;
    let (id, fresh) = super::Recorder::intern(&mut rr.st.loads_ix, rr.st.loads.len(), name);
    if fresh {
        rr.st
            .loads
            .push((name.to_vec(), Version::ABSENT, FileKind::Tex));
    }
    let s = &mut rr.st.steps;
    let same = s.produced.get(&id).is_some_and(|was| match (was, &bytes) {
        (Some(a), Some(b)) => a[..] == b[..],
        (None, None) => true,
        _ => false,
    });
    s.stored.insert(id);
    s.produced.insert(id, bytes.clone());
    s.last_phi.insert(id, bytes.clone());
    if let Some(phi) = s.phi.as_mut() {
        phi.insert(id, bytes);
    }
    !same
}

/// The streams producers outside the steps defined ([`define_stream`]),
/// by name, for the link to write.
#[must_use]
pub fn produced_streams<H: Host>(tex: &Tex<H, SsaTracker>) -> Vec<(Vec<u8>, Stream)> {
    let r = tex.tracker.rec.borrow();
    r.st.steps
        .produced
        .iter()
        .filter_map(|(&id, v)| Some((r.st.loads.get(id as usize)?.0.clone(), v.clone())))
        .collect()
}

/// The job's written streams as the stores hold them now, the bytes a
/// load of each is served (DESIGN 3.7, "Files are a view"): `None` for
/// one with no open in the stores (a debugging view, `PARTEX_SSA_STREAMS`).
pub fn stream_values<H: Host>(tex: &Tex<H, SsaTracker>) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
    let r = tex.tracker.rec.borrow();
    let s = &r.st.steps;
    s.stored
        .iter()
        .filter_map(|&id| {
            let name = r.st.loads.get(id as usize)?.0.clone();
            if let Some(v) = s.complete.as_ref().and_then(|c| c.get(&id)) {
                return Some((name, v.as_deref().map(<[u8]>::to_vec)));
            }
            Some((name, s.value_at(&r.rt.fold, id, u64::MAX)?.ok().flatten()))
        })
        .collect()
}

/// The written streams the last trip withheld (DESIGN 3.7, "A trip that
/// ended fatally"; [`SsaTracker::keep_complete`]): each by name, with the
/// last complete trip's bytes (`None`: no file then). The host's link
/// writes these over the files the steps' effects made (the cut `.aux`),
/// after them; empty unless the last trip ended fatally with the option on.
#[must_use]
pub fn withheld_streams<H: Host>(tex: &Tex<H, SsaTracker>) -> Vec<(Vec<u8>, Stream)> {
    let r = tex.tracker.rec.borrow();
    let Some(c) = &r.st.steps.complete else {
        return Vec::new();
    };
    c.iter()
        .filter_map(|(&id, v)| Some((r.st.loads.get(id as usize)?.0.clone(), v.clone())))
        .collect()
}

/// The steps a stopped rebuild left to run (DESIGN 3.7, "A rebuild
/// stopped"): 0 when the program is the source's.
pub fn pending<H: Host>(tex: &Tex<H, SsaTracker>) -> usize {
    tex.tracker
        .rec
        .borrow()
        .st
        .steps
        .pending
        .as_ref()
        .map_or(0, |p| p.dirty.len())
}

fn has_pending<H: Host>(tex: &Tex<H, SsaTracker>) -> bool {
    tex.tracker.rec.borrow().st.steps.pending.is_some()
}

/// What a build's trips call outside the engine (DESIGN 3.7, "Trips, as
/// built").
pub struct Trips<'a, H> {
    /// Trips in a build at most (`PARTEX_SSA_TRIPS`; 1: one trip, no tool
    /// run).
    pub max: usize,
    /// The outside tools, run between two trips on the streams the job
    /// stores, by name, as the trip left them: whether one wrote a file,
    /// and a line for each run.
    /// (The compatibility path: the build's own tools are
    /// [`Trips::native`]; a host that has them passes a closure that runs
    /// nothing.)
    #[allow(clippy::type_complexity)]
    pub tools:
        &'a mut dyn FnMut(&mut H, &[(Vec<u8>, Arc<[u8]>)]) -> (bool, Vec<alloc::string::String>),
    /// The build's BibTeX and makeindex, nodes of its program run at each
    /// trip's end (DESIGN 3.7, "Outside tools are nodes"); `None`: none.
    pub native: Option<&'a super::tools::NativeTools>,
    /// A clock in nanoseconds, for the trips' times.
    pub clock: Option<fn() -> u64>,
}

impl<H> Trips<'_, H> {
    fn now(&self) -> u64 {
        self.clock.map_or(0, |c| c())
    }
}

impl RebuildReport {
    /// Trip `r`, which took `ns`, added to the build's counts.
    fn absorb(&mut self, r: RebuildReport, ns: u64) {
        self.trips += 1;
        self.trip_steps.push(r.steps_run);
        self.trip_commands.push(r.commands);
        self.trip_ns.push(ns);
        self.edits += r.edits;
        self.seeds += r.seeds;
        self.phi += r.phi;
        self.store_readers += r.store_readers;
        self.queries += r.queries;
        self.data_edits += r.data_edits;
        self.steps_run += r.steps_run;
        self.steps_same += r.steps_same;
        self.retries += r.retries;
        self.new_steps += r.new_steps;
        self.removed += r.removed;
        self.cold += r.cold;
        self.defs_changed += r.defs_changed;
        self.readers_marked += r.readers_marked;
        self.readers_kept += r.readers_kept;
        self.reads_checked += r.reads_checked;
        self.positioned += r.positioned;
        self.restored += r.restored;
        self.initial += r.initial;
        self.applied += r.applied;
        self.skipped += r.skipped;
        self.commands += r.commands;
        self.unsupported = self.unsupported.or(r.unsupported);
        self.cancelled |= r.cancelled;
        self.stopped = r.stopped;
        self.pending = r.pending;
        self.resumed |= r.resumed;
        self.history = r.history;
        self.log.extend(r.log);
    }
}

/// Rebuild the job in trips (DESIGN 3.7, "Trips, as built"): trip 1 is
/// [`rebuild`], what the edits reach; then, while the stores the last
/// trip left make seeds, the outside tools whose input changed run and
/// the next trip runs, `trips.max` trips at most. The files are linked
/// after the last.
///
/// The host's output files are scratch until then: a step run again
/// writes its `\openout`/`\write` bytes to the host at once (the job may
/// read them back), so a file another step had written is left partly
/// written (an `\include`'s `.aux` truncated to the lines of the steps
/// run again). The host must link after each rebuild and write back
/// every file the link makes, by the name it was opened with, before
/// its next edit (the CLI's `SsaLinker::link`); else the next rebuild
/// reads the partial file as an edit of it.
pub fn rebuild_trips<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    trace: bool,
    apply: bool,
    trips: &mut Trips<'_, H>,
) -> RebuildReport {
    let t0 = trips.now();
    // (the tools' files, by stamp, once for the build: before the job's
    // loads are, whose check the trips after it keep)
    let tools_stale = trips.native.is_some() && super::tools::look(tex);
    let first = rebuild(tex, trace, apply);
    let ns = trips.now().saturating_sub(t0);
    let ran = first.steps_run > 0;
    let mut rep = RebuildReport {
        trace,
        ..RebuildReport::default()
    };
    rep.absorb(first, ns);
    if !ran && tools_stale && rep.unsupported.is_none() && rep.stopped.is_none() {
        // (no step ran, but a tool's file changed: it runs at the trip's
        // end, and the trips go on from what it defines)
        if trips.max <= 1 {
            one_trip_tools(tex, trips, &mut rep);
            return rep;
        }
        return more_trips(tex, trace, apply, trips, rep);
    }
    if !ran {
        // (nothing ran: the stores are what the loads of the φ read; the
        // trace of the trip, as `more_trips` keeps it)
        rep.settled = rep.unsupported.is_none() && rep.stopped.is_none();
        rep.log.extend(rebuild_log(tex));
        return rep;
    }
    if trips.max <= 1 {
        // (one trip per build: a plain pass; the tools run after it, as a
        // pass's are, for the next build to read)
        one_trip_tools(tex, trips, &mut rep);
        return rep;
    }
    more_trips(tex, trace, apply, trips, rep)
}

/// One trip per build (`PARTEX_SSA_TRIPS=1`): after the trip, the tools
/// whose inputs changed run, as after a pass, and define what the next
/// build's first trip reads; no trip follows.
fn one_trip_tools<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    trips: &mut Trips<'_, H>,
    rep: &mut RebuildReport,
) {
    if rep.unsupported.is_some() || rep.stopped.is_some() {
        return;
    }
    let (mut phi, mut changed) = {
        let r = &mut *tex.tracker.rec.borrow_mut();
        r.st.steps.trip_end(&r.rt.fold)
    };
    if let Some(native) = trips.native {
        let lines = super::tools::derive(tex, &mut phi, &mut changed, native, trips.clock);
        rep.tools.extend(lines);
    }
    // (and the outside tools, whose files the next build's first trip
    // finds changed)
    let streams: Vec<(Vec<u8>, Arc<[u8]>)> = {
        let r = tex.tracker.rec.borrow();
        phi.iter()
            .filter_map(|(&id, v)| Some((r.st.loads.get(id as usize)?.0.clone(), v.clone()?)))
            .collect()
    };
    let (_, lines) = (trips.tools)(&mut tex.host, &streams);
    rep.tools.extend(lines);
}

/// After a cold build, its trip 1, which ran `commands` commands in `ns`:
/// the trips that follow, as in [`rebuild_trips`] (DESIGN 3.7, "The cold
/// build converges too").
pub fn settle<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    trace: bool,
    apply: bool,
    trips: &mut Trips<'_, H>,
    commands: u64,
    ns: u64,
) -> RebuildReport {
    if has_pending(tex) {
        // (a settle that stopped: its work goes on as a rebuild's)
        return rebuild_trips(tex, trace, apply, trips);
    }
    let rep = RebuildReport {
        trace,
        trips: 1,
        trip_steps: alloc::vec![tex.tracker.rec.borrow().rt.fold.order.len()],
        trip_commands: alloc::vec![commands],
        trip_ns: alloc::vec![ns],
        ..RebuildReport::default()
    };
    more_trips(tex, trace, apply, trips, rep)
}

/// The trips after a build's first, which `rep` holds.
fn more_trips<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    trace: bool,
    apply: bool,
    trips: &mut Trips<'_, H>,
    mut rep: RebuildReport,
) -> RebuildReport {
    loop {
        if rep.unsupported.is_some() || rep.stopped.is_some() {
            return rep;
        }
        let (mut phi, mut changed) = {
            let r = &mut *tex.tracker.rec.borrow_mut();
            r.st.steps.trip_end(&r.rt.fold)
        };
        // the build's tools, on the stores as the trip left them: the
        // streams they define are the next trip's φ too (at the bound as
        // well, as a pass's tools run after it)
        let native_lines = match trips.native {
            Some(native) => super::tools::derive(tex, &mut phi, &mut changed, native, trips.clock),
            None => Vec::new(),
        };
        rep.tools.extend(native_lines);
        // the outside tools, on the streams as the trip left them
        let streams: Vec<(Vec<u8>, Arc<[u8]>)> = {
            let r = tex.tracker.rec.borrow();
            phi.iter()
                .filter_map(|(&id, v)| Some((r.st.loads.get(id as usize)?.0.clone(), v.clone()?)))
                .collect()
        };
        let (wrote, lines) = (trips.tools)(&mut tex.host, &streams);
        rep.tools.extend(lines);
        if rep.trips >= trips.max.max(1) {
            // (the bound: converged only if every load of the φ read what
            // the last trip stored; else the names, as latexmk reports)
            let r = tex.tracker.rec.borrow();
            let mut ids: Vec<u32> =
                r.st.steps
                    .phi_seeds(&r.rt.fold, &phi)
                    .into_iter()
                    .map(|x| x.2)
                    .collect();
            ids.sort_unstable();
            ids.dedup();
            rep.unsettled = ids
                .iter()
                .filter_map(|&id| Some(r.st.loads.get(id as usize)?.0.clone()))
                .collect();
            rep.settled = rep.unsettled.is_empty();
            return rep;
        }
        if trace {
            let names: Vec<alloc::string::String> = {
                let r = tex.tracker.rec.borrow();
                changed
                    .iter()
                    .filter_map(|&id| r.st.loads.get(id as usize))
                    .map(|l| alloc::string::String::from_utf8_lossy(&l.0).into_owned())
                    .collect()
            };
            note(
                tex,
                alloc::format!(
                    "trip {}: the stores changed {names:?}{}",
                    rep.trips + 1,
                    if wrote { "; a tool wrote" } else { "" }
                ),
            );
            // (each one's first line that differs from what the trip read)
            let firsts: Vec<alloc::string::String> = {
                let r = tex.tracker.rec.borrow();
                changed
                    .iter()
                    .map(|id| {
                        let was = r.st.steps.last_phi.get(id).cloned().flatten();
                        let now = phi.get(id).cloned().flatten();
                        first_difference(was.as_deref(), now.as_deref())
                    })
                    .collect()
            };
            for (n, f) in names.iter().zip(firsts) {
                note(tex, alloc::format!("  {n}: {f}"));
            }
        }
        tex.tracker.rec.borrow_mut().st.steps.next = Some(NextTrip {
            phi,
            changed,
            files: wrote,
        });
        let t0 = trips.now();
        let r = rebuild(tex, trace, apply);
        let ns = trips.now().saturating_sub(t0);
        if r.steps_run == 0 {
            // (no seed: every load of the φ read what the trip stored)
            rep.settled = r.unsupported.is_none();
            rep.unsupported = rep.unsupported.or(r.unsupported);
            rep.cancelled |= r.cancelled;
            rep.log.extend(r.log);
            rep.log.extend(rebuild_log(tex));
            return rep;
        }
        rep.absorb(r, ns);
    }
}

/// What [`rerun_check`] found (DESIGN 4.3 item 1, "Checked").
#[derive(Clone, Debug, Default)]
pub struct RerunCheck {
    /// The steps run again, and the commands they ran.
    pub steps: usize,
    pub commands: u64,
    /// Runs dropped for a read not placed (normal: the prediction).
    pub retries: usize,
    /// Steps whose run ended elsewhere than their last run, changed a
    /// definition, stored other lines or made other effects than it.
    pub ended_elsewhere: usize,
    pub defs_changed: usize,
    pub stores_changed: usize,
    pub effects_changed: usize,
    /// The first differences, each described.
    pub first: Vec<alloc::string::String>,
}

/// Run every step of the fold again alone, the last first, and compare
/// each run with the step's last one: where it ended, its definitions,
/// its stores and its effects (DESIGN 4.3 item 1, "Checked"). The loads
/// of a stored name read what the cold build's did: the φ it found, or
/// the stores before them. In reverse order the engine's fields outside
/// the families hold another step's leftovers, so a difference is state
/// that a step's boundary leaves outside the families and the input.
pub fn rerun_check<H: Host>(tex: &mut Tex<H, SsaTracker>, apply: bool) -> RerunCheck {
    let mut out = RerunCheck::default();
    let mut rep = RebuildReport::default();
    let order = {
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        // (each stored name's φ: the file the build's loads found before
        // it stored the name, or none)
        let s = &rr.st.steps;
        let phi: BTreeMap<u32, Option<Arc<[u8]>>> = s
            .stored
            .iter()
            .map(|&id| {
                let found = s.datas.iter().rev().find(|d| d.name == id && d.file);
                (id, found.map(|d| d.bytes.clone()))
            })
            .collect();
        rr.st.steps.phi = Some(phi);
        rr.st.vers.epoch += 1;
        rr.rt.open_trip(1);
        rr.on = true;
        rr.rt.fold.order.clone()
    };
    tex.tracker.check = false;
    tex.tracker.apply.set(apply);
    tex.tracker.boundary();
    let mut dirty = Dirty::default();
    let mut srep = SsaReport::default();
    for pos in (1..order.len()).rev() {
        let (j, prev) = (order[pos], order[pos - 1]);
        let (input, old_end, old_defs, old_stores, old_fx) = {
            let r = tex.tracker.rec.borrow();
            let s = &r.st.steps;
            let Some(input) = s.end(prev) else { continue };
            let fx: Vec<Version> = s
                .effects
                .get(j as usize)
                .map(|v| v.iter().map(|e| e.0).collect())
                .unwrap_or_default();
            (
                input,
                s.end(j),
                defs(&r.rt, j),
                s.stores.get(&j).cloned(),
                fx,
            )
        };
        let (retries, commands) = (rep.retries, tex.commands());
        let end = run_step(tex, j, &[j], &[], &input, &mut dirty, &mut rep, &mut srep);
        out.steps += 1;
        out.commands += tex.commands() - commands;
        out.retries += rep.retries - retries;
        let r = tex.tracker.rec.borrow();
        let s = &r.st.steps;
        let mut why = Vec::new();
        if !old_end
            .as_ref()
            .is_some_and(|o| same_place(tex, &end, o) && same_input(tex, &end, o))
        {
            out.ended_elsewhere += 1;
            why.push(alloc::format!(
                "ended at {}, before at {}",
                end.brief(),
                old_end
                    .as_ref()
                    .map_or(alloc::string::String::from("-"), InputState::brief)
            ));
        }
        let new_defs = defs(&r.rt, j);
        let changed: Vec<alloc::string::String> = old_defs
            .keys()
            .chain(new_defs.keys())
            .filter(|a| positioned(a) && old_defs.get(a) != new_defs.get(a))
            .map(|a| {
                let v = |x: Option<&Version>| {
                    x.map_or(alloc::string::String::from("-"), |v| {
                        alloc::format!("{:04x}", v.0 & 0xffff)
                    })
                };
                alloc::format!(
                    "{a}={} {}->{}",
                    super::view::trace_name(tex, &r.st, *a),
                    v(old_defs.get(a)),
                    v(new_defs.get(a))
                )
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !changed.is_empty() {
            out.defs_changed += 1;
            why.push(alloc::format!(
                "definitions changed: {}",
                changed[..changed.len().min(8)].join(" ")
            ));
        }
        if s.stores.get(&j) != old_stores.as_ref() {
            out.stores_changed += 1;
            why.push(alloc::string::String::from("stores changed"));
        }
        let fx: Vec<Version> = s
            .effects
            .get(j as usize)
            .map(|v| v.iter().map(|e| e.0).collect())
            .unwrap_or_default();
        if fx != old_fx {
            out.effects_changed += 1;
            why.push(alloc::format!(
                "effects changed ({} chunks, before {})",
                fx.len(),
                old_fx.len()
            ));
        }
        if !why.is_empty() && out.first.len() < 40 {
            out.first.push(alloc::format!(
                "step {j} (from {}): {}",
                input.brief(),
                why.join("; ")
            ));
        }
    }
    tex.tracker.flush_effects();
    {
        let mut r = tex.tracker.rec.borrow_mut();
        r.flush_output();
        r.on = false;
        r.rt.close_trip();
        r.st.steps.phi = None;
        r.st.steps.log.clear();
    }
    tex.tracker.apply.set(false);
    out
}

/// A line of the rebuild's trace.
/// Where two contents of a stored name first differ, for the trace: the
/// line's number and both versions of it (absent: none).
fn first_difference(was: Option<&[u8]>, now: Option<&[u8]>) -> alloc::string::String {
    let (Some(was), Some(now)) = (was, now) else {
        return alloc::format!(
            "was {}, now {}",
            was.map_or("absent", |_| "there"),
            now.map_or("absent", |_| "there")
        );
    };
    let lines = |b: &[u8]| -> Vec<alloc::string::String> {
        b.split(|&c| c == b'\n')
            .map(|l| alloc::string::String::from_utf8_lossy(l).into_owned())
            .collect()
    };
    let (w, n) = (lines(was), lines(now));
    match (0..w.len().max(n.len())).find(|&i| w.get(i) != n.get(i)) {
        Some(i) => alloc::format!(
            "line {}: was {:?}, now {:?} ({} and {} lines)",
            i + 1,
            w.get(i).map_or("(none)", |l| &l[..]),
            n.get(i).map_or("(none)", |l| &l[..]),
            w.len(),
            n.len()
        ),
        None => alloc::string::String::from("the same lines"),
    }
}

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
fn seed(rr: &mut Recorder, e: &Edit, to: u32, dirty: &mut Dirty, trace: bool) {
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
            dirty.mark(st.key, l.step);
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
            dirty.mark(st.key, l.step);
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
    dirty: &mut Dirty,
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
        rr.st.steps.datas[d as usize].superseded = true;
        rep.data_edits += 1;
        rr.st.steps.edits.push(e);
    }
    if rr.st.steps.edits.len() == k {
        old
    } else {
        old.mapped(&rr.st.steps.edits[k..])
    }
}

/// The steps to run, in key order (item 3), each with why. A step a
/// changed definition marked keeps the slots and the versions its last
/// run read of them, and when its turn comes it is passed over if each
/// reaches it at that version again: a definition that went from one step
/// to the next, as when a step run again in its place ends elsewhere
/// (LaTeX's `.aux` read at `\begin{document}`, its new line in a new
/// step). Any other mark runs it: an edit, a store, a query answered
/// anew, an input that changed, a definition its last run did not read.
#[derive(Default)]
pub(crate) struct Dirty {
    order: BTreeMap<u64, StepId>,
    why: BTreeMap<StepId, Why>,
    /// The table entries a run of this rebuild was dropped for (read at a
    /// later definition, not placed): each step run after it is placed at
    /// them too, where a later definition holds the arrays. A loop's
    /// windows take branches by value, and the reads one window's run
    /// made only now, the next one's makes too ([`MISSED_MAX`] at most;
    /// `eqtb` only, [`predicts_alone`]).
    missed: BTreeSet<Slot>,
    /// The old step whose definition a dropped run read first (the
    /// earliest): where the run is in the old run's text, for the new
    /// steps after it (with windows, the rebuild's `ahead`).
    anchor: Option<StepId>,
}

/// The slots [`Dirty::missed`] keeps at most.
const MISSED_MAX: usize = 4096;

/// Whether a slot may be placed apart from the slots a run reads with it
/// (a prediction made of other runs' misses or writes): an `eqtb` entry.
/// Not a page node without the page's length, a nest level's field
/// without the nest, a save stack entry without its pointer.
fn predicts_alone(a: &Slot) -> bool {
    a.0 == Fam::Eqtb
}

/// Why a step is dirty: `None` runs it, `Some` holds the slots it read at
/// a definition that changed, and the versions it read.
type Why = Option<Vec<(Slot, Version)>>;

impl Dirty {
    /// Step `s`, at `key`, runs; whether it was not dirty yet.
    fn mark(&mut self, key: u64, s: StepId) -> bool {
        self.why.insert(s, None);
        self.order.insert(key, s).is_none()
    }

    /// Step `s`, at `key`, read slot `a` at version `v`, which the
    /// definition that reaches it may no longer be. The first change of
    /// `a` before it to mark it knows what it read: the changes come in
    /// key order, and those after the first are not what it read.
    fn mark_read(&mut self, key: u64, s: StepId, a: Slot, v: Version) {
        self.order.insert(key, s);
        if let Some(r) = self.why.entry(s).or_insert_with(|| Some(Vec::new()))
            && !r.iter().any(|(b, _)| *b == a)
        {
            r.push((a, v));
        }
    }

    /// The first step in key order, and why.
    fn pop_first(&mut self) -> Option<(StepId, Why)> {
        let (_, s) = self.order.pop_first()?;
        Some((s, self.why.remove(&s).flatten()))
    }

    /// The step at `key` is not dirty.
    fn remove(&mut self, key: u64) {
        if let Some(s) = self.order.remove(&key) {
            self.why.remove(&s);
        }
    }

    /// The keys made again (`Fold::renumber`).
    fn rekey(&mut self, fold: &Fold<TexSsa>) {
        self.order = core::mem::take(&mut self.order)
            .into_values()
            .map(|s| (fold.steps[s as usize].key, s))
            .collect();
    }

    fn len(&self) -> usize {
        self.order.len()
    }

    fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The dirty steps and their keys, in key order.
    fn steps(&self) -> impl Iterator<Item = (StepId, u64)> + '_ {
        self.order.iter().map(|(&k, &s)| (s, k))
    }
}

/// The step after `s` in the fold reads where `s` left the input, which
/// changed: it is dirty.
/// Whether there is one.
fn mark_next<H: Host>(tex: &Tex<H, SsaTracker>, s: StepId, dirty: &mut Dirty) -> bool {
    let r = tex.tracker.rec.borrow();
    let fold = &r.rt.fold;
    if let Some(n) = fold
        .position(s)
        .and_then(|p| fold.order.get(p + 1).copied())
    {
        dirty.mark(fold.steps[n as usize].key, n);
        return true;
    }
    false
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
    // (the key first: [`same_place`] compares it before the rest)
    let key = place_key(end);
    fold.order[from..].iter().take(LOOK_AHEAD).find_map(|&s| {
        if r.st.steps.end_key(s)? != key {
            return None;
        }
        let e = r.st.steps.end(s)?;
        same_place(tex, end, &e).then_some((s, e))
    })
}

/// What [`same_place`] compares first, as a key: the flags, the levels'
/// counts, the line numbers and the top level's state and index.
type PlaceKey = (u8, usize, usize, i32, i32, i32, i32, i32);

fn place_key(e: &InputState) -> PlaceKey {
    let flags = u8::from(e.finished)
        | u8::from(e.fire) << 1
        | u8::from(e.ship) << 2
        | u8::from(e.load) << 3
        | u8::from(e.page) << 4
        | u8::from(e.graf) << 5;
    (
        flags,
        e.in_open,
        e.v.depth,
        e.line,
        e.file.line,
        e.cur.state,
        e.cur.index,
        e.cur.start,
    )
}

/// The old steps after a cascade's step, by where each ended
/// ([`place_key`]): [`meet`] past [`LOOK_AHEAD`].
type Ends = BTreeMap<PlaceKey, Vec<StepId>>;

/// [`meet`] among every old step after `cur`, not only the next
/// [`LOOK_AHEAD`]: a cascade that ran past them (the text of many old
/// steps gone, as a chapter `\includeonly` leaves out) meets the old
/// steps where its run comes back to their text, and passes over the
/// ones between. `ends` is made at the first call of a cascade, from the
/// old steps after `cur` then.
fn meet_far<H: Host>(
    tex: &Tex<H, SsaTracker>,
    cur: StepId,
    end: &InputState,
    ends: &mut Option<Ends>,
) -> Option<(StepId, InputState)> {
    let r = tex.tracker.rec.borrow();
    let fold = &r.rt.fold;
    let from = fold.position(cur)? + 1;
    let ends = ends.get_or_insert_with(|| {
        let mut m = Ends::new();
        for &s in &fold.order[from..] {
            if let Some(k) = r.st.steps.end_key(s) {
                m.entry(k).or_default().push(s);
            }
        }
        m
    });
    // (the first one in the fold's order, live and after the run)
    let mut at: Vec<(usize, StepId)> = ends
        .get(&place_key(end))?
        .iter()
        .filter_map(|&s| Some((fold.position(s)?, s)))
        .filter(|&(p, _)| p >= from)
        .collect();
    at.sort_unstable();
    at.into_iter().find_map(|(_, s)| {
        let e = r.st.steps.end(s)?;
        same_place(tex, end, &e).then_some((s, e))
    })
}

/// Step `s` is passed over (item 4): it is gone with its definitions,
/// and a reader of one now reads the definition before it.
/// The commands the live steps after `cur` ran last: what running the
/// rest of the job again would cost.
fn commands_after<H: Host>(tex: &Tex<H, SsaTracker>, cur: StepId) -> u64 {
    let r = tex.tracker.rec.borrow();
    let fold = &r.rt.fold;
    let from = fold.position(cur).map_or(fold.order.len(), |p| p + 1);
    fold.order[from..]
        .iter()
        .map(|&s| r.st.step_commands.get(s as usize).copied().unwrap_or(0))
        .sum()
}

/// A cascade gone cold (the extension's preamble edit: every step after
/// it new, each costing more to place than to run): the live steps after
/// `cur` retired, the last first, so that no definition is later than the
/// new steps, which then run as a cold build's do ([`run_step`], a new
/// step the fold's last).
fn go_cold<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    cur: StepId,
    dirty: &mut Dirty,
    rep: &mut RebuildReport,
) {
    let rest: Vec<StepId> = {
        let r = tex.tracker.rec.borrow();
        let fold = &r.rt.fold;
        let from = fold.position(cur).map_or(fold.order.len(), |p| p + 1);
        fold.order[from..].to_vec()
    };
    if rep.trace {
        note(
            tex,
            alloc::format!(
                "  cold after step {cur}: {} old steps after it retired",
                rest.len()
            ),
        );
    }
    for &s in rest.iter().rev() {
        retire(tex, s, dirty, rep);
    }
    // (the retired steps' records go now: the rest of the job, run as a
    // cold build, makes new ones in their place)
    tex.tracker.rec.borrow_mut().rt.collect_retired();
    rep.cold += 1;
}

fn retire<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    s: StepId,
    dirty: &mut Dirty,
    rep: &mut RebuildReport,
) {
    let vals = {
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        let key = rr.rt.fold.steps[s as usize].key;
        let old = defs(&rr.rt, s);
        for (a, v) in old.iter().filter(|(a, _)| positioned(a)) {
            // (its readers now read the definition that reaches it: an
            // equal one changes nothing for them)
            if reaching_version(rr, a, key) == Some(*v) {
                continue;
            }
            let next = rr.rt.fold.next_after(a, key).map(|d| d.key);
            for x in rr.rt.fold.readers_between(a, key, next) {
                dirty.mark_read(rr.rt.fold.steps[x as usize].key, x, *a, *v);
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
        for &i in &ids {
            *rr.st.steps.store_changes.entry(i).or_default() += 1;
        }
        for l in rr.st.steps.loads.remove(&s).into_iter().flatten() {
            if let Some(ss) = rr.st.steps.loaders.get_mut(&l.id) {
                ss.remove(&s);
            }
        }
        rr.st.steps.queries.remove(&s);
        mark_store_readers(rr, &ids, key, dirty, rep);
        rr.rt.fold.remove(s, old.keys());
        // (its end, which no run starts from again: the input it held,
        // the token lists and the macros' arguments open there, goes)
        if let Some(x) = rr.st.steps.inputs.get_mut(s as usize) {
            *x = None;
        }
        // (its chunks leave the link)
        rr.st.steps.fx_changed.push(s);
        rr.st
            .steps
            .glyph_dirty
            .extend(old.keys().filter(|a| a.0 == Fam::Glyphs).map(|a| a.1));
        dirty.remove(key);
        with_levels(tex, old.keys().filter(|a| positioned(a)).copied(), |a| {
            latest(tex, rr, a, s, rep)
        })
    };
    rep.restored += vals.len();
    rep.removed += 1;
    put(tex, &vals);
}

/// The loads after `key` of the names in `ids`, which a step stored to
/// differently, that read the build's own store: dirty (7.17.3, "A load
/// reads the store"). The loads that read the φ wait for the next trip.
pub(super) fn mark_store_readers(
    rr: &Recorder,
    ids: &[u32],
    key: u64,
    dirty: &mut Dirty,
    rep: &mut RebuildReport,
) {
    let st = &rr.st.steps;
    let mut readers = BTreeSet::new();
    for id in ids {
        for &s in st.loaders.get(id).into_iter().flatten() {
            if st
                .loads
                .get(&s)
                .is_some_and(|seen| seen.iter().any(|l| !l.phi && l.id == *id))
            {
                readers.insert(s);
            }
        }
    }
    for s in readers {
        let Some(k) = rr.rt.fold.steps.get(s as usize).map(|x| x.key) else {
            continue;
        };
        if k > key {
            dirty.mark(k, s);
            rep.store_readers += 1;
        }
    }
}

/// Drop the outputs a run left (one dropped, or none at a step's start):
/// the files it opened are closed, as no link writes them, and the next
/// run opens them on their handles again ([`Steps::reopen`]).
pub(super) fn drop_outputs<H: Host>(tex: &mut Tex<H, SsaTracker>) {
    let fx = tex.take_effects();
    let chunks = core::mem::take(&mut tex.tracker.rec.borrow_mut().st.steps.cur_chunks);
    for e in fx.iter().chain(chunks.iter().flat_map(|c| c.1.iter())) {
        if let crate::effects::Effect::Open { file, .. } = e {
            tex.host.close(*file);
        }
    }
}

/// Run step `j` at its place from `input` (7.17.3 items 2 and 3, "A read
/// resolves by prediction and validation"), the reads of the last runs of
/// the steps `predict` predicting its own, and the writes of the steps
/// `writes` too; mark the readers of the definitions it changed dirty.
/// Its result.
#[allow(clippy::too_many_arguments)]
fn run_step<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    j: StepId,
    predict: &[StepId],
    writes: &[StepId],
    input: &InputState,
    dirty: &mut Dirty,
    rep: &mut RebuildReport,
    srep: &mut SsaReport,
) -> InputState {
    rep.steps_run += 1;
    if rep.trace {
        // (before it runs: a run that panics is traced)
        note(tex, alloc::format!("step {j} begins at {}", input.brief()));
    }
    let c0 = tex.commands();
    let Placing {
        key,
        old,
        mut found,
        budget,
        alone,
        mut next,
    } = placing(tex, j, predict, writes, dirty, rep);
    let mut set: BTreeSet<Slot> = BTreeSet::new();
    let mut touched: BTreeSet<Slot> = BTreeSet::new();
    let mut objs_placed = false;
    let finished = loop {
        let vals = place_values(
            tex,
            key,
            alone,
            &mut found,
            &mut next,
            &mut set,
            &mut objs_placed,
            rep,
        );
        rep.positioned += vals.len();
        put(tex, &vals);
        input.set(tex);
        // (the step begins at the checkpoint where the one before it
        // stopped: `main_control` resumes there, the command counted)
        tex.at_checkpoint = true;
        // (a step's outputs are flushed at its end: a dropped run's are
        // not the next run's)
        drop_outputs(tex);
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
        tex.tracker.rec.borrow_mut().st.steps.watch = Some(Watch {
            key,
            set: core::mem::take(&mut set),
            scanned: 0,
            later: false,
        });
        tex.tracker
            .stop_after
            .set(tex.commands().saturating_add(budget));
        let mut step = tex.resume();
        let mut stopped = false;
        let fin = loop {
            match step {
                Step::Checkpoint => {
                    if rep.trace {
                        let c = tex.commands();
                        *prof.entry((at.0, at.1)).or_default() += c - at.2;
                        at = (tex.in_open, tex.line, c);
                    }
                    // (stopped where it was: [`Tracker::stop_due`])
                    stopped = tex.commands() > tex.tracker.stop_after.get()
                        && tex
                            .tracker
                            .rec
                            .borrow()
                            .st
                            .steps
                            .watch
                            .as_ref()
                            .is_some_and(|w| w.later);
                    if stopped || super::step_ends(tex) {
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
        tex.tracker.stop_after.set(u64::MAX);
        if let Some(w) = tex.tracker.rec.borrow_mut().st.steps.watch.take() {
            set = w.set;
        }
        if fin || stopped {
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
            let miss: Vec<Slot> = if alone {
                r.rt.open_step_reads()
                    .inspect(|_| n += 1)
                    .filter(|a| a.0 == Fam::PageNode && !set.contains(*a))
                    .copied()
                    .collect()
            } else {
                // (and the entry values it saved that are still saved:
                // reads of it, made at its end, [`SsaTracker::entry_saved`])
                let saved = if SOFT_PLACE.load(core::sync::atomic::Ordering::Relaxed) {
                    tex.tracker.entry_saved()
                } else {
                    Vec::new()
                };
                let mut m: Vec<Slot> = r
                    .rt
                    .open_step_reads()
                    .chain(saved.iter())
                    .inspect(|_| n += 1)
                    .filter(|a| checked(a) && !set.contains(&placed_as(a)) && later(fold, a, key))
                    .copied()
                    .collect();
                m.sort_unstable();
                m.dedup();
                m
            };
            rep.reads_checked += n;
            let written = if miss.is_empty() {
                Vec::new()
            } else {
                recs_writes(&r.rt, r.rt.open_step_recs())
                    .into_keys()
                    .collect()
            };
            (miss, written)
        };
        if rep.trace && !miss.is_empty() {
            let r = tex.tracker.rec.borrow();
            let fold = &r.rt.fold;
            // (each with the step whose definition the run read)
            let m: Vec<alloc::string::String> = miss
                .iter()
                .take(8)
                .map(|a| match fold.latest(a) {
                    Some(d) => alloc::format!("{a}@{}", d.step),
                    None => alloc::format!("{a}"),
                })
                .collect();
            drop(r);
            note(
                tex,
                alloc::format!(
                    "  run of step {j} dropped{}: read {} of {} slots at a later definition ({} ...)",
                    if stopped {
                        " (stopped past its budget)"
                    } else {
                        ""
                    },
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
        if dirty.missed.len() < MISSED_MAX {
            dirty
                .missed
                .extend(miss.iter().filter(|a| predicts_alone(a)).copied());
        }
        {
            let r = tex.tracker.rec.borrow();
            let fold = &r.rt.fold;
            let first = miss
                .iter()
                .filter_map(|a| fold.latest(a))
                .filter(|d| d.step != j)
                .min_by_key(|d| d.key);
            if let Some(d) = first {
                dirty.anchor = Some(d.step);
            }
        }
        tex.tracker.rec.borrow_mut().rt.abort_step();
        tex.tracker.drop_softs();
        // (what the dropped run wrote goes back to what reaches the step)
        let vals = {
            let mut r = tex.tracker.rec.borrow_mut();
            let rr = &mut *r;
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
            with_levels(
                tex,
                written.into_iter().filter(positioned).chain(nodes),
                |a| {
                    touched.insert(a);
                    if miss.contains(&a) {
                        None
                    } else {
                        reaching(tex, rr, a, key, rep)
                    }
                },
            )
        };
        put(tex, &vals);
        next = miss.iter().map(placed_as).collect();
        next.sort_unstable();
        next.dedup();
    };
    // the step ends: its definitions replace the old ones
    let new = {
        let input = InputState::of(tex, finished);
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        tex.tracker.end_step(rr, tex.save_ptr);
        let d = defs(&rr.rt, j);
        let stores = step_closed(rr, j, input);
        mark_store_readers(rr, &stores, key, dirty, rep);
        // (the glyph rows it defined or defines: [`glyph_union_now`])
        rr.st.steps.glyph_dirty.extend(
            old.keys()
                .chain(d.keys())
                .filter(|a| a.0 == Fam::Glyphs)
                .map(|a| a.1),
        );
        d
    };
    after_close(
        tex,
        Closing {
            j,
            key,
            old,
            new,
            alone,
            set,
            touched,
            predict,
            c0,
        },
        dirty,
        rep,
    )
}

/// Where a step's run is placed ([`placing`]): its key, its old
/// definitions, the slots its last run and the runs predicting it read
/// whose definition reaching it a later one hides (with that one), its
/// budget of commands, whether it is the fold's last new step (`alone`:
/// the arrays hold what reaches it), and the slots placed whole (the
/// save stack, the nest, a window's state).
pub(super) struct Placing {
    pub(super) key: u64,
    pub(super) old: BTreeMap<Slot, Version>,
    pub(super) found: Vec<(Slot, Option<Def>)>,
    pub(super) budget: u64,
    pub(super) alone: bool,
    pub(super) next: Vec<Slot>,
}

/// Where step `j`'s run is placed, predicted by the steps `predict` and
/// the writes of `writes` ([`run_step`]).
pub(super) fn placing<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    j: StepId,
    predict: &[StepId],
    writes: &[StepId],
    dirty: &Dirty,
    rep: &mut RebuildReport,
) -> Placing {
    #[allow(clippy::type_complexity)]
    let (key, old, found, budget, alone): (
        u64,
        BTreeMap<Slot, Version>,
        Vec<(Slot, Option<Def>)>,
        u64,
        bool,
    ) = {
        let r = tex.tracker.rec.borrow();
        let fold = &r.rt.fold;
        let key = fold.steps[j as usize].key;
        let old = defs(&r.rt, j);
        // (a new step the fold's last, as a cascade gone cold makes them:
        // no definition is later than it, so the arrays hold what reaches
        // it, as in a cold build, but for the page's nodes, which they
        // hold only up to the list's length: those alone are placed,
        // checked and put back; an old step's own old definitions are
        // later than it)
        let alone = old.is_empty() && fold.order.last() == Some(&j);
        // (each with the definition that reaches the step, found where the
        // test for a later one looked)
        // (a name the old run made, as a loop's points, is read by a new
        // run that finds it made: the old run's writes predict it)
        let written = writes.iter().flat_map(|&p| {
            fold.steps[p as usize]
                .recs
                .iter()
                .flat_map(|&q| r.rt.writes(q).iter().map(|(a, _)| a))
                .filter(|a| predicts_alone(a))
        });
        let mut cand: Vec<Slot> = if alone {
            predict
                .iter()
                .flat_map(|&p| fold.reads_of(p))
                .chain(&dirty.missed)
                .filter(|a| a.0 == Fam::PageNode)
                .copied()
                .collect()
        } else {
            predict
                .iter()
                .flat_map(|&p| fold.reads_of(p))
                .chain(&dirty.missed)
                .chain(written)
                .filter(|a| checked(a))
                .map(placed_as)
                .collect()
        };
        if predict.len() > 1 || !writes.is_empty() || !dirty.missed.is_empty() {
            // (the steps predicting it read much the same slots: each
            // looked up once)
            cand.sort_unstable();
            cand.dedup();
        }
        let reads = cand
            .into_iter()
            .filter_map(|a| {
                if a.0 == Fam::PageNode {
                    Some((a, fold.reaching(&a, key)))
                } else {
                    fold.reaching_if_later(&a, key).map(|d| (a, d))
                }
            })
            .collect();
        // (a run past it that read a later definition stops: [`Watch`])
        let last = predict
            .iter()
            .filter_map(|&p| r.st.step_commands.get(p as usize).copied())
            .max();
        let budget = last.unwrap_or(0).saturating_mul(2).saturating_add(10_000);
        (key, old, reads, budget, alone)
    };
    let mut next = Vec::new();
    if !alone {
        save_stack_whole(tex, key, &mut next, rep);
        nest_whole(tex, key, &mut next);
        if tex.window() > 0 {
            window_whole(tex, key, &mut next);
        }
    }
    Placing {
        key,
        old,
        found,
        budget,
        alone,
        next,
    }
}

/// The values a step's run is placed at ([`run_step`]): the definitions
/// reaching it of the slots in `found` and `next` not `set` yet, where a
/// later definition holds the arrays (`set` takes them), and the object
/// table whole with its lists' heads or the fonts' state the first time
/// they are among them ([`objs_whole`]).
#[allow(clippy::too_many_arguments)]
pub(super) fn place_values<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    key: u64,
    alone: bool,
    found: &mut Vec<(Slot, Option<Def>)>,
    next: &mut Vec<Slot>,
    set: &mut BTreeSet<Slot>,
    objs_placed: &mut bool,
    rep: &mut RebuildReport,
) -> Vec<(Slot, SVal)> {
    // (the object table placed whole with its lists' heads or the fonts'
    // state: [`objs_whole`])
    if !alone && !*objs_placed && found.iter().map(|(a, _)| a).chain(&*next).any(object_table) {
        *objs_placed = true;
        objs_whole(tex, key, next, rep);
    }
    // the definitions that reach the step, where a later one is in the
    // arrays
    let mut r = tex.tracker.rec.borrow_mut();
    let rr = &mut *r;
    let mut vals = Vec::with_capacity(found.len() + next.len());
    for (a, d) in found.drain(..) {
        if set.insert(a)
            && let Some(v) = value_of(tex, rr, a, d, rep)
        {
            vals.push((a, v));
        }
    }
    for a in next.drain(..) {
        if set.insert(a)
            && let Some(v) = reaching(tex, rr, a, key, rep)
        {
            vals.push((a, v));
        }
    }
    vals
}

/// What [`after_close`] needs of a step's run closed in the fold.
pub(super) struct Closing<'a> {
    pub(super) j: StepId,
    pub(super) key: u64,
    pub(super) old: BTreeMap<Slot, Version>,
    pub(super) new: BTreeMap<Slot, Version>,
    pub(super) alone: bool,
    pub(super) set: BTreeSet<Slot>,
    pub(super) touched: BTreeSet<Slot>,
    pub(super) predict: &'a [StepId],
    pub(super) c0: u64,
}

/// A step's run closed in the fold (its definitions `new` in place of
/// `old`): the readers of each definition that changed marked dirty, up
/// to the slot's next definition, and the arrays holding the latest
/// definitions again. Its result ([`run_step`]).
pub(super) fn after_close<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    c: Closing<'_>,
    dirty: &mut Dirty,
    rep: &mut RebuildReport,
) -> InputState {
    let Closing {
        j,
        key,
        old,
        new,
        alone,
        set,
        mut touched,
        predict,
        c0,
    } = c;
    let defs_before = rep.defs_changed;
    let mut changed = Vec::new();
    let vals = if alone {
        // (no step after it to read its definitions, and the arrays hold
        // them: the page's nodes placed go back)
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        with_levels(tex, set.iter().copied(), |a| latest(tex, rr, a, j, rep))
    } else {
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        // a definition that changed makes its readers dirty, up to the
        // slot's next definition (item 3); where one of the runs made none,
        // its readers read the definition that reaches the step, and an
        // equal one changes nothing for them
        let slots = old
            .keys()
            .chain(new.keys().filter(|a| !old.contains_key(a)));
        let mut union_same = None;
        // (a font's fields are not placed, their values not built yet,
        // but one that changed still makes its readers dirty: a step run
        // again that sets `\hyphenchar` or a `\fontdimen`, an
        // `\intarray`'s count or entries, is read by later steps)
        // (nor a meaning's class, made by the meaning's writes, which
        // are placed; nor a name, which a lookup that did not find it
        // read, and its hash slot's text, for one that probed: a step
        // that now makes a name a later step made wakes the lookups
        // between, `SsaTracker::name_defined`)
        for a in slots.filter(|a| {
            positioned(a) || matches!(a.0, Fam::Font | Fam::Class | Fam::Name | Fam::Hash)
        }) {
            // (a name and its hash slot only wake their readers: the
            // table keeps every name made, and is not put back; a font's
            // identifier is put back)
            if !matches!(a.0, Fam::Name | Fam::Hash) || positioned(a) {
                touched.insert(*a);
            }
            let (o, n) = (old.get(a).copied(), new.get(a).copied());
            if o == n || ((o.is_none() || n.is_none()) && reaching_version(rr, a, key) == o.or(n)) {
                continue;
            }
            // (a font's field the run did not write again holds what the
            // last run left, its values not placed: a cache made at its
            // first use, `font:cmr10.glue`, is there still)
            if a.0 == Fam::Font && n.is_none() {
                continue;
            }
            // (nor does a name or its hash slot's text the run did not
            // make again: the table keeps every name made, so a step run
            // again finds the names its last run entered and enters none,
            // and the lookups after it find them still)
            if matches!(a.0, Fam::Name | Fam::Hash) && !positioned(a) && n.is_none() {
                continue;
            }
            rep.defs_changed += 1;
            let next = rr.rt.fold.next_after(a, key).map(|d| d.key);
            let mut readers = rr.rt.fold.readers_between(a, key, next);
            // (a ship's glyphs are read by the job's end alone, for each
            // font's union: if the ships' rows make the union the end
            // last made, the end need not run, DESIGN 4.3, "The job's
            // end"; made after the step closed, so a later ship that
            // changes the union again looks again)
            let cut = a.0 == Fam::Glyphs
                && *union_same.get_or_insert_with(|| {
                    glyph_union_now(rr).is_some_and(|u| rr.st.steps.glyph_union == Some(u))
                });
            if cut {
                readers.clear();
            }
            if rep.trace && (changed.len() < 12 || readers.iter().any(|&s| s != j)) {
                // (the trace: the versions, short, and which calls of each
                // reader read it)
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
                let short = |v: Option<Version>| {
                    v.map_or(alloc::string::String::from("-"), |v| {
                        alloc::format!("{:04x}", v.0 & 0xffff)
                    })
                };
                let name = super::view::trace_name(tex, &rr.st, *a);
                changed.push(alloc::format!(
                    "{a}={name} {}->{} (before {}; {} readers{}{}: {})",
                    short(o),
                    short(n),
                    short(reaching_version(rr, a, key)),
                    readers.len(),
                    if cut {
                        ", each font's union as it was"
                    } else {
                        ""
                    },
                    if readers.is_empty() {
                        alloc::string::String::new()
                    } else {
                        alloc::format!(" {:?}", readers.iter().take(4).collect::<Vec<_>>())
                    },
                    who.join("; ")
                ));
            }
            for s in readers {
                if s != j {
                    // (they read the old definition, or, where the last run
                    // made none, the one that reaches the step: passed over
                    // if it is what reaches them again when their turn
                    // comes)
                    let k = rr.rt.fold.steps[s as usize].key;
                    match o.or_else(|| reaching_version(rr, a, key)) {
                        Some(v) => dirty.mark_read(k, s, *a, v),
                        None => {
                            dirty.mark(k, s);
                        }
                    }
                    rep.readers_marked += 1;
                }
            }
        }
        // the arrays hold the latest definitions again
        with_levels(tex, set.union(&touched).copied(), |a| {
            latest(tex, rr, a, j, rep)
        })
    };
    rep.restored += vals.len();
    put(tex, &vals);
    if !old.is_empty() && rep.defs_changed == defs_before {
        rep.steps_same += 1;
    }
    if rep.trace {
        // (why a window ended: DESIGN 4.3 item 1)
        let cut = if tex.window() == 0 || !tex.window_due() {
            // (none, or a clean point's: the end's place names its kind)
            alloc::string::String::new()
        } else if tex.fire_pending {
            alloc::string::String::from(", cut: a fire pending")
        } else if let Some(e) = tex.window_cut() {
            alloc::format!(", cut: {e:?}")
        } else {
            alloc::string::String::from(", cut: the count")
        };
        note(
            tex,
            alloc::format!(
                "step {j} (key {key}, predicted by {predict:?}): {} commands{cut}, changed {}",
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

/// Sources' edits, for glyph origins (`srcmap.rs`): each edit's old and
/// new data and its runs of changed lines' old and new byte ranges.
pub(crate) type Edits = Vec<(Arc<[u8]>, Arc<[u8]>, Vec<[usize; 4]>)>;

/// The sources' edits from the `k`-th on ([`Edits`]).
pub(crate) fn edits_from(rec: &Recorder, k: usize) -> Edits {
    rec.st
        .steps
        .edits
        .get(k..)
        .unwrap_or_default()
        .iter()
        .map(|e| {
            let hunks = e.hunks.iter().map(|h| [h.of, h.ot, h.nf, h.nt]).collect();
            (e.old.clone(), e.new.clone(), hunks)
        })
        .collect()
}

/// A step's run on a worker, waiting for the step's turn (DESIGN 3.10,
/// "Commit"): the step before it and that step's result when its round
/// began (the run began there), the build's tables' lengths then (the
/// worker's own ids are numbered from them), the fold's numbering then,
/// and the run.
#[cfg(feature = "std")]
struct Ahead {
    prev: StepId,
    tag: (usize, usize, usize),
    lens: super::par::BaseLens,
    renumbered: u32,
    done: super::par::Done,
}

/// A rebuild's runs on workers, by step.
#[cfg(feature = "std")]
#[derive(Default)]
pub(super) struct Spec {
    ran: BTreeMap<StepId, Ahead>,
}

#[cfg(not(feature = "std"))]
#[derive(Default)]
pub(super) struct Spec;

/// The fewest commands a step's last run ran for it to go to a worker,
/// and the fewest a round's steps ran together: a smaller round costs
/// more than it saves.
#[cfg(feature = "std")]
const SPEC_MIN: u64 = 64;
#[cfg(feature = "std")]
const ROUND_MIN: u64 = 4_000;

/// Whether this build's steps may run on workers: more than one, and
/// none of what a worker's run cannot hand over (check mode, the entry
/// check, `SyncTeX`, display lists, glyph origins, timing).
fn may_speculate<H: Host>(tex: &Tex<H, SsaTracker>) -> bool {
    tex.tracker.par.workers.get() > 1
        && cfg!(feature = "std")
        && !tex.tracker.check
        && !tex.tracker.timed
        && !super::ENTRY_CHECK.load(core::sync::atomic::Ordering::Relaxed)
        && !tex.synctex_on()
        && tex.dl.is_none()
        && tex.org.is_none()
}

impl Spec {
    /// Drop every run waiting (the steps they ran are gone).
    fn clear<H: Host>(&mut self, tex: &Tex<H, SsaTracker>) {
        #[cfg(feature = "std")]
        for (_, a) in core::mem::take(&mut self.ran) {
            let mut st = tex.tracker.par.stats.borrow_mut();
            match &a.done.ran {
                Ok(r) => st.not_taken("its step gone", r.commands),
                Err(_) => st.tainted += 1,
            }
            drop(st);
            recycle(tex, a.done);
        }
        #[cfg(not(feature = "std"))]
        let _ = tex;
    }
}

/// A worker's tracker back among the shells.
#[cfg(feature = "std")]
fn recycle<H: Host>(tex: &Tex<H, SsaTracker>, d: super::par::Done) {
    if let Some(mut t) = d.tex {
        let shell = core::mem::replace(&mut t.tracker, SsaTracker::new(Recorder::new()));
        let mut shells = tex.tracker.par.shells.borrow_mut();
        if shells.len() < 64 {
            shells.push(shell);
        }
    }
}

/// A round (DESIGN 3.10, "Threads"): step `j`, whose turn it is, and the
/// dirty steps after it, up to twice the workers, each run on a worker
/// from the entry its old run predicts (the step before it's result,
/// the definitions reaching it placed), all at once; the runs wait in
/// `spec` for their steps' turns.
#[cfg(feature = "std")]
fn speculate<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    j: StepId,
    dirty: &Dirty,
    rep: &mut RebuildReport,
    spec: &mut Spec,
) {
    use super::par;
    let workers = tex.tracker.par.workers.get();
    if spec.ran.contains_key(&j) {
        return;
    }
    let (spec_min, round_min) = tex
        .tracker
        .par
        .thresholds
        .get()
        .unwrap_or((SPEC_MIN, ROUND_MIN));
    let cands: Vec<StepId> = {
        let r = tex.tracker.rec.borrow();
        let fold = &r.rt.fold;
        let cost = |s: StepId| r.st.step_commands.get(s as usize).copied().unwrap_or(0);
        // (a step its marks will keep, each definition reaching it as it
        // read it: none of its run is wanted)
        let kept = |s: StepId| {
            let key = fold.steps[s as usize].key;
            dirty.why.get(&s).is_some_and(|w| {
                w.as_ref().is_some_and(|reads| {
                    reads
                        .iter()
                        .all(|(a, v)| reaching_version(&r, a, key) == Some(*v))
                })
            })
        };
        let mut c = alloc::vec![j];
        for (s, _) in dirty.steps() {
            if c.len() >= workers * 2 {
                break;
            }
            if s == j
                || spec.ran.contains_key(&s)
                || !fold.steps[s as usize].live
                || cost(s) < spec_min
                || kept(s)
            {
                continue;
            }
            c.push(s);
        }
        let total: u64 = c.iter().map(|&s| cost(s)).sum();
        if c.len() < 2 || total < round_min {
            return;
        }
        c
    };
    let snap = {
        let r = tex.tracker.rec.borrow();
        par::Snapshot::take(&tex.tracker, &r)
    };
    // (the tables' chunks written since the last snapshot made shared
    // once, not copied for each view)
    tex.eqtb.commit();
    tex.eqtb_obj.commit();
    tex.hash.commit();
    tex.save_stack.commit();
    let (tx, rx) = std::sync::mpsc::channel::<par::Msg>();
    let mut replies = Vec::new();
    let mut jobs = Vec::new();
    let mut meta = Vec::new();
    let renumbered = tex.tracker.rec.borrow().rt.fold.renumbered;
    for s in cands {
        let start = {
            let r = tex.tracker.rec.borrow();
            let fold = &r.rt.fold;
            fold.position(s)
                .filter(|&p| p > 0)
                .map(|p| fold.order[p - 1])
                .and_then(|prev| Some((prev, r.st.steps.end(prev)?, r.st.steps.end_tag(prev)?)))
        };
        let Some((prev, input, tag)) = start else {
            continue;
        };
        let Placing {
            key,
            mut found,
            budget,
            alone,
            mut next,
            ..
        } = placing(tex, s, &[s], &[], dirty, rep);
        if alone {
            continue;
        }
        let mut set = BTreeSet::new();
        let mut objs = false;
        let vals = place_values(
            tex, key, alone, &mut found, &mut next, &mut set, &mut objs, rep,
        );
        // (a stream's file is put in the source as it is now by the
        // build's edits, which a worker has not)
        if vals.iter().any(|(a, _)| a.0 == Fam::Read) {
            continue;
        }
        let (rtx, rrx) = std::sync::mpsc::channel();
        let host = par::WorkerHost {
            job: jobs.len(),
            ask: tx.clone(),
            reply: rrx,
            now: tex.host.now(),
            notes: tex.host.notes(),
            streams: tex.host.wants_streams(),
            commands: tex.host.runs_commands(),
            events: Vec::new(),
            tainted: None,
        };
        let mut shell = tex
            .tracker
            .par
            .shells
            .borrow_mut()
            .pop()
            .unwrap_or_else(|| SsaTracker::new(Recorder::new()));
        {
            let r = tex.tracker.rec.borrow();
            let salt = u32::try_from(r.rt.fold.steps[s as usize].salt).unwrap_or(s);
            par::prepare(&mut shell, &tex.tracker, &r, &snap, key, salt);
        }
        let mut fork = tex.fork_with(host, shell);
        let hash = usize::try_from(crate::web::HASH_BASE).unwrap_or(0) + fork.hash.len();
        if fork.tracker.stamps[0].len() != fork.eqtb.len()
            || fork.tracker.stamps[1].len() != hash
            || fork.tracker.sstamps.len() != fork.save_stack.len() + 16
        {
            fork.size_stamps();
        }
        replies.push(rtx);
        meta.push((s, prev, tag));
        jobs.push(par::Job {
            idx: jobs.len(),
            j: s,
            tex: fork,
            vals,
            input,
            budget,
        });
    }
    if jobs.len() < 2 {
        for job in jobs {
            recycle(
                tex,
                par::Done {
                    j: job.j,
                    tex: Some(Box::new(job.tex)),
                    ran: Err("a round of one"),
                },
            );
        }
        return;
    }
    let base = Arc::new(par::Base::lend(&mut tex.tracker.rec.borrow_mut().st));
    let lens = base.lens();
    for job in &jobs {
        job.tex.tracker.rec.borrow_mut().st.base = Some(base.clone());
    }
    let n = jobs.len() as u64;
    let dones = par::round(&mut tex.host, jobs, workers, tx, &rx, &replies);
    for d in dones.iter().flatten() {
        if let Some(t) = &d.tex {
            t.tracker.rec.borrow_mut().st.base = None;
        }
    }
    par::Base::restore(base, &mut tex.tracker.rec.borrow_mut().st);
    {
        let mut st = tex.tracker.par.stats.borrow_mut();
        st.rounds += 1;
        st.runs += n;
    }
    for (d, (s, prev, tag)) in dones.into_iter().zip(meta) {
        if let Some(done) = d {
            spec.ran.insert(
                s,
                Ahead {
                    prev,
                    tag,
                    lens,
                    renumbered,
                    done,
                },
            );
        }
    }
}

/// The slots `set` (placed for a run that did not happen) back at their
/// latest definitions.
#[cfg(feature = "std")]
fn unplace<H: Host>(tex: &mut Tex<H, SsaTracker>, set: &BTreeSet<Slot>, rep: &mut RebuildReport) {
    let vals = {
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        with_levels(tex, set.iter().copied(), |a| {
            latest(tex, rr, a, StepId::MAX, rep)
        })
    };
    put(tex, &vals);
}

/// Step `j`'s turn, its run made on a worker (`a`): taken as its run if
/// each slot it read from outside it holds there what it read, once the
/// build has placed it as its own run would be placed (DESIGN 3.10,
/// "Commit"), as a run of the step made here (`run_step`) is taken; the
/// step's result. `Err`: why not; the step then runs here.
#[cfg(feature = "std")]
fn commit<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    j: StepId,
    prev: StepId,
    a: Ahead,
    dirty: &mut Dirty,
    rep: &mut RebuildReport,
) -> Result<InputState, (&'static str, u64)> {
    use super::par;
    let Ahead {
        prev: p0,
        tag,
        lens,
        renumbered,
        done,
    } = a;
    let par::Done { tex: w, ran, .. } = done;
    let ran = ran.map_err(|e| (e, 0))?;
    let w = w.ok_or(("a panic", 0))?;
    let cost = ran.commands;
    let shell = |tex: &Tex<H, SsaTracker>, w: Box<Tex<par::WorkerHost, SsaTracker>>| {
        recycle(
            tex,
            par::Done {
                j,
                tex: Some(w),
                ran: Err(""),
            },
        );
    };
    let checked_start = {
        let r = tex.tracker.rec.borrow();
        if p0 != prev || r.st.steps.end_tag(prev) != Some(tag) {
            Err("its start moved")
        } else if r.rt.fold.renumbered != renumbered {
            Err("the keys made again")
        } else if !w
            .tracker
            .worker
            .as_deref()
            .is_some_and(|wk| par::answers_hold(&tex.tracker, &r, wk))
        {
            Err("the fonts' or the names' makers moved")
        } else {
            Ok(())
        }
    };
    if let Err(e) = checked_start {
        shell(tex, w);
        return Err((e, cost));
    }
    // (what it wrote that is not put back by position: the hash's and the
    // pool's entries and their allocators, the meanings' classes and the
    // names, each with its value; anything else, the step runs here)
    let writes = ran.export.writes();
    let moved = tex.str_ptr != ran.str_ptr
        || tex.hash_used != ran.hash_used
        || tex.hash_high != ran.hash_high;
    for (a, v) in &writes {
        let why = if positioned(a) {
            if a.0 == Fam::Dvi {
                Some("the DVI writer")
            } else {
                v.is_none().then_some("a write's value let go")
            }
        } else {
            match a.0 {
                Fam::Class | Fam::Name => None,
                Fam::Hash | Fam::HashNext | Fam::Pool | Fam::Alloc if v.is_none() => {
                    Some("a write's value let go")
                }
                Fam::Hash | Fam::HashNext | Fam::Pool | Fam::Alloc if moved => {
                    Some("the allocators moved")
                }
                Fam::Hash | Fam::HashNext | Fam::Pool | Fam::Alloc => None,
                _ => Some("a write the build cannot take"),
            }
        };
        if let Some(e) = why {
            shell(tex, w);
            return Err((e, cost));
        }
    }
    let map = {
        let wr = w.tracker.rec.borrow();
        let mut r = tex.tracker.rec.borrow_mut();
        par::Remap::new(&wr.st, lens, &mut r.st)
    };
    let map = match map {
        Ok(m) => m,
        Err(e) => {
            shell(tex, w);
            return Err((e, cost));
        }
    };
    // placed as its run here would be, the misses its run would meet
    // placed too
    let Placing {
        key,
        old,
        mut found,
        alone,
        mut next,
        ..
    } = placing(tex, j, &[j], &[], dirty, rep);
    if alone {
        shell(tex, w);
        return Err(("the fold's last step", cost));
    }
    let reads: Vec<(Slot, Version)> = ran
        .export
        .versioned()
        .map(|(a, v)| (map.slot(*a), v))
        .collect();
    let mut set = BTreeSet::new();
    let mut objs = false;
    let mut left = true;
    for _ in 0..8 {
        let vals = place_values(
            tex, key, false, &mut found, &mut next, &mut set, &mut objs, rep,
        );
        rep.positioned += vals.len();
        put(tex, &vals);
        let r = tex.tracker.rec.borrow();
        let fold = &r.rt.fold;
        let classes = ran
            .classes
            .iter()
            .map(|&(p, _)| Slot(Fam::Eqtb, i64::from(p)));
        let mut miss: Vec<Slot> = reads
            .iter()
            .map(|x| x.0)
            .chain(classes)
            .filter(|a| checked(a) && !set.contains(&placed_as(a)) && later(fold, a, key))
            .map(|a| placed_as(&a))
            .collect();
        if miss.is_empty() {
            left = false;
            break;
        }
        miss.sort_unstable();
        miss.dedup();
        next = miss;
    }
    let found_otherwise = left || {
        let r = tex.tracker.rec.borrow();
        let view = super::View {
            tex: &*tex,
            rec: &r.st,
        };
        reads.iter().any(|(a, v)| {
            !matches!(a.0, Fam::Source | Fam::Line | Fam::Class)
                && partex_ssa::Store::version(&view, &partex_ssa::Loc::State(*a)) != *v
        }) || ran.classes.iter().any(|&(p, c)| tex.token_class_of(p) != c)
    };
    if found_otherwise {
        unplace(tex, &set, rep);
        shell(tex, w);
        return Err((
            if left {
                "it read later definitions"
            } else {
                "a read found otherwise"
            },
            cost,
        ));
    }
    // taken: as `run_step` closes a run made here
    rep.steps_run += 1;
    let c0 = tex.commands();
    drop_outputs(tex);
    tex.log_file.buf.clear();
    for f in &mut tex.write_file {
        f.buf.clear();
    }
    let par::Ran {
        mut export,
        end,
        mut logs,
        commands,
        generation,
        ..
    } = ran;
    map.export(&mut export);
    logs.map_loads(|i| map.load(i));
    // (the slots it wrote that no later step defines: the arrays hold its
    // values, as its run here would leave them; and what is not put back
    // by position: the pool's strings in order, then the allocators and
    // the hash)
    let writes: Vec<(Slot, Option<SVal>)> = export.writes();
    let rank = |a: &Slot| match a.0 {
        Fam::Pool => 0,
        Fam::Alloc => 1,
        _ => 2,
    };
    let mut kept: Vec<(Slot, SVal)> = writes
        .iter()
        .filter(|(a, _)| {
            !positioned(a) && matches!(a.0, Fam::Hash | Fam::HashNext | Fam::Pool | Fam::Alloc)
        })
        .filter_map(|(a, v)| Some((*a, v.clone()?)))
        .collect();
    kept.sort_by_key(|(a, _)| (rank(a), a.1));
    {
        use crate::track::scalar::GLUE_LINEAGE;
        let lineage = Slot(Fam::Alloc, i64::from(GLUE_LINEAGE));
        let now = super::scalar_get(tex, GLUE_LINEAGE).unwrap_or(0);
        kept.retain(|(a, v)| {
            *a != lineage || !matches!(v.1.as_deref(), Some(super::SValue::Int(x)) if *x <= now)
        });
    }
    let mut w = w;
    let events = core::mem::take(&mut w.host.events);
    let (wst_sets, wst_written, made_names) = {
        let wr = w.tracker.rec.borrow();
        let made: Vec<(i32, u32)> = w
            .tracker
            .worker
            .as_deref()
            .map(|wk| wk.made_names.borrow().clone())
            .unwrap_or_default();
        (wr.st.load_sets.clone(), wr.st.written.clone(), made)
    };
    {
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        rr.rt.rerun_step(j);
        rr.st.steps.run_begins(Some(j));
        rr.rt.import_step(j, export);
        rr.st.steps.put_logs(logs);
        for (id, (v, k)) in wst_sets {
            rr.st.set_load(map.load(id), v, k);
        }
        rr.st.written.extend(wst_written);
        rr.st.generation += generation;
        let c = &mut rr.st.step_commands;
        if c.len() <= j as usize {
            c.resize(j as usize + 1, 0);
        }
        c[j as usize] = commands;
    }
    for (p, id) in made_names {
        tex.tracker.note_made(key, p, map.name(id));
    }
    tex.commands += commands;
    super::par::replay_events(&mut tex.host, events);
    put(tex, &kept);
    let new = {
        let mut r = tex.tracker.rec.borrow_mut();
        let rr = &mut *r;
        let d = defs(&rr.rt, j);
        let stores = step_closed(rr, j, end);
        mark_store_readers(rr, &stores, key, dirty, rep);
        rr.st.steps.glyph_dirty.extend(
            old.keys()
                .chain(d.keys())
                .filter(|a| a.0 == Fam::Glyphs)
                .map(|a| a.1),
        );
        d
    };
    // (its definitions no later step's hides: in the arrays, as its run
    // here leaves them)
    let mine: Vec<(Slot, SVal)> = {
        let r = tex.tracker.rec.borrow();
        let fold = &r.rt.fold;
        writes
            .into_iter()
            .filter(|(a, _)| positioned(a) && fold.latest(a).is_some_and(|d| d.step == j))
            .filter_map(|(a, v)| Some((a, v?)))
            .collect()
    };
    put(tex, &mine);
    {
        let mut st = tex.tracker.par.stats.borrow_mut();
        st.taken += 1;
        st.commands_taken += commands;
    }
    shell(tex, w);
    Ok(after_close(
        tex,
        Closing {
            j,
            key,
            old,
            new,
            alone: false,
            set,
            touched: BTreeSet::new(),
            predict: &[j],
            c0,
        },
        dirty,
        rep,
    ))
}
