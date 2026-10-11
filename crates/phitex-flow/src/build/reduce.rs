//! Evaluation as graph reduction: the design agreed on 2026-10-11 (CORE.md,
//! "Graph reduction").
//!
//! The document is a term that evaluation rewrites in place. What has not
//! been read is a **thunk**, `pending(entry; text[pos..])`: a text (the
//! root, or a file an `\input` read) from a place on, with the state it is
//! entered in. Reducing a thunk runs the reader (`graph::run_piece`) on it:
//! a node per command, each reading versions of names and defining
//! versions; the reduction stops at the first place a command begins in a
//! state a thunk may be entered in that is one of the text's **split
//! points**, and the thunk is replaced by the reduced stretch followed by
//! the thunk of the rest. An `\input` read where the reader is on its own
//! text reduces to the file's thunk followed by the thunk of the rest of
//! the calling line (`RState::file_start`, `RState::rest`); the job's end
//! reduces to nothing (what follows is never demanded); a file's end, to
//! its caller's rest. The reduced stretches in order are the **spine**.
//!
//! The PDF is the root that demands the spine: the page builder is a fold
//! over the spine's material, fed each stretch as it is reduced, its
//! output routines run where it fires and their definitions entering the
//! names index between that stretch's nodes; a node after the fire that
//! read another value is reduced again from there (the stretch's thunk
//! un-reduced from that node) before the fold goes on. Pages become DVI or
//! PDF by pure functions.
//!
//! Split points are not structure: they are the places another thread may
//! reduce a thunk ahead of the spine, on the state predicted for it
//! (**speculation**); the spine adopts that reduction when it arrives at
//! the place in that very state and every name the reduction read has the
//! value it read, and otherwise reduces the thunk itself (keeping the
//! nodes of the speculated reduction that still hold). Nothing is joined,
//! re-cut or muted: a reduction that does not stop at a split point reads
//! on, and a speculated reduction it passed is dropped.
//!
//! An edit **un-reduces** what read a changed value and nothing else. The
//! last evaluation's reductions are a memo by place (a root text place is
//! moved through the edit; a file's by its unchanged content): at each
//! thunk the spine first looks for the memo's reduction of that place,
//! entered in the same state, whose text (and the character after it) is
//! unchanged, and whose reads of the names that may have changed
//! (**diverged**: what the stretches reduced again, the memo's stretches
//! passed over and the page builder's definitions that differ defined)
//! still have the values they had. Such a reduction is adopted whole —
//! its nodes, its paragraphs' lines, its definitions. Otherwise the thunk
//! is reduced again with the memo's reduction as the old run (the nodes
//! before the first changed read are kept, the chain realigns after it).
//!
//! The files the document writes and reads back are values in memory: an
//! evaluation whose reads of them differ from what it wrote is followed by
//! another, over the same memo (only what read them is reduced again),
//! as are derived files (BibTeX, makeindex: [`Build::set_derive`]).

use std::collections::BTreeMap;

use super::*;
use crate::graph::run_piece_win;

/// Where a reduction stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum End {
    /// At a split point: a thunk of the same text follows.
    Split,
    /// At its text's end (a file's: `\endinput` included).
    TextEnd,
    /// At an `\input` whose file is a text of its own.
    Input,
    /// The job ended in it (or a fatal error stopped the reader).
    JobEnd,
}

/// Which text a place is in.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(super) enum TextKey {
    Root,
    File(Arc<str>),
}

/// A text the document reads.
pub(super) struct RText {
    key: TextKey,
    /// Its content's hash (a file's place is the same place again only in
    /// the same content).
    hash: u64,
    chars: Arc<[char]>,
    /// Where a reduction may stop (ascending).
    splits: Arc<Vec<usize>>,
    /// Where each line ends.
    eols: Vec<u32>,
    /// The root: where each job of a session begins.
    jobs: Vec<usize>,
}

impl RText {
    fn new(key: TextKey, chars: Arc<[char]>, splits: Arc<Vec<usize>>) -> RText {
        let eols = chars.iter().enumerate().filter(|(_, c)| **c == '\r').map(|(i, _)| i as u32).collect();
        let jobs = if key == TextKey::Root {
            chars.iter().enumerate().filter(|(_, c)| **c == crate::graph::JOB).map(|(i, _)| i).collect()
        } else {
            Vec::new()
        };
        let hash = hash_of(&*chars);
        RText { key, hash, chars, splits, eols, jobs }
    }

    /// The lines of the text before `pos` (in a session, from its job's
    /// start).
    fn lines_before(&self, pos: usize) -> u32 {
        let n = self.eols.partition_point(|&e| (e as usize) < pos) as u32;
        let j = self.jobs.partition_point(|&s| s <= pos);
        if j == 0 {
            n
        } else {
            n - self.eols.partition_point(|&e| (e as usize) < self.jobs[j - 1]) as u32
        }
    }

    /// The next job's start at or after `pos`.
    fn next_job(&self, pos: usize) -> Option<usize> {
        let j = self.jobs.partition_point(|&s| s < pos);
        self.jobs.get(j).copied()
    }
}

/// A reduction the last evaluation made, kept as the memo of its place.
pub(super) struct OldSeg {
    hash: u64,
    chars: Arc<[char]>,
    start: usize,
    stop: usize,
    end: End,
    entry: Arc<RState>,
    run: Arc<Run>,
    lines: Vec<Option<Lines>>,
    /// The page builder's definitions between its nodes: (node, names).
    fires: Vec<(u32, Vec<(Key, Val)>)>,
}

/// What survives from evaluation to evaluation: the reductions by place,
/// the output routines' runs, the values of written files the reads of
/// which were refuted.
#[derive(Default)]
pub(super) struct RMemo {
    segs: Vec<OldSeg>,
    by_place: FxMap<(TextKey, usize), Vec<usize>>,
    root: Option<Arc<[char]>>,
    routines: Vec<Arc<Memo>>,
    /// A read of a file the run wrote, found refuted: its place (text,
    /// start of the reduction, node), the name, the value it must see.
    file_vals: FxMap<(TextKey, usize, u32, Key), Val>,
    /// A file's split points, by content.
    cuts: FxMap<u64, Arc<Vec<usize>>>,
}

/// A thunk: `text[pos..]` not reduced yet, entered in `entry`.
#[derive(Clone)]
struct Thunk {
    text: usize,
    pos: usize,
    entry: Arc<RState>,
}

/// The rest of a line that called `\input`, while the file is reduced.
struct Cont {
    text: usize,
    pos: usize,
    caller: RState,
    file: usize,
}

/// A reduced stretch of the spine.
struct Seg {
    id: PieceId,
    text: usize,
    start: usize,
    stop: usize,
    end: End,
    entry: Arc<RState>,
    run: Arc<Run>,
    lines: Vec<Option<Lines>>,
    fires: Vec<(u32, Vec<(Key, Val)>)>,
    /// The memo's reduction it is, unchanged.
    old: Option<usize>,
}

/// A thunk another thread reduces ahead of the spine.
enum Slot {
    Waiting(PieceId, Arc<AtomicBool>),
    Running(PieceId, Arc<RState>, Arc<AtomicBool>),
    Done(PieceId, Arc<RState>, Arc<Run>, Vec<Option<Lines>>),
}

struct SpecTask {
    text: usize,
    pos: usize,
    id: PieceId,
    entry: Arc<RState>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Specs {
    queue: VecDeque<SpecTask>,
    /// By text, by place.
    slots: FxMap<usize, BTreeMap<usize, Slot>>,
    done: bool,
}

/// What every thread reads: the names index, the texts, the speculation.
struct Red {
    /// Commands each thread ran (the spine's thread first).
    work: Vec<AtomicU64>,
    names: Names,
    font: Arc<Font>,
    fonts: Arc<FontFiles>,
    opts: Options,
    texts: RwLock<Vec<Arc<RText>>>,
    specs: Mutex<Specs>,
    cv: Condvar,
}

impl Red {
    fn text(&self, t: usize) -> Arc<RText> {
        self.texts.read().expect("texts")[t].clone()
    }
}

/// A reduction's reads of names from before it: the names index at its
/// place (the spine before it: speculated reductions define nothing there
/// until adopted).
struct REnv<'a> {
    r: &'a Red,
    id: PieceId,
    text: &'a RText,
    /// Where the reduced text begins in its text.
    base: usize,
    /// A speculated reduction: given up when the spine passed it, or on
    /// predictions too deep.
    cancel: Option<&'a AtomicBool>,
}

/// Nodes after which a speculated reduction is given up.
const SPEC_NODES: u64 = 1 << 20;

impl Env for REnv<'_> {
    fn cs(&self) -> &Arc<CsTable> {
        &self.r.names.cs
    }

    fn folds(&mut self, key: &Key) -> bool {
        matches!(key, Key::Page(_))
    }

    fn fold_read(&mut self, key: &Key, node: u32) -> Option<Val> {
        self.r.names.fold_value(key, (self.id, node), &self.r.font)
    }

    fn limits(&self) -> crate::print::Limits {
        self.r.names.given.limits
    }

    fn interaction_option(&self) -> Option<i32> {
        self.r.names.given.interaction_option
    }

    fn font_file(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.r.fonts.get(name)
    }

    fn xetex(&self) -> bool {
        self.r.names.given.xe.is_some()
    }

    fn native_font(&mut self, name: &str, size: i32, tracing: i32) -> crate::native::Loaded {
        self.r.names.given.native_font(name, size, tracing)
    }

    fn main_memory(&self) -> u64 {
        self.r.opts.main_memory
    }

    fn check(&mut self, ran: u64, words: u64) -> Check {
        if let Some(c) = self.cancel {
            if c.load(Ordering::Acquire) || ran > SPEC_NODES {
                return Check::Abandon;
            }
        }
        let cap = self.r.opts.main_memory;
        if words > cap { Check::Exceeded(cap) } else { Check::Go }
    }

    fn give_up_deep(&mut self) -> bool {
        self.cancel.is_some()
    }

    fn has_between(&mut self, key: &Key) -> bool {
        self.r.names.has_inner(key)
    }

    fn any_between(&mut self) -> bool {
        self.r.names.any_inner()
    }

    fn between(&mut self, key: &Key, after: u32, node: u32) -> Option<(Val, Src)> {
        let names = &self.r.names;
        if !names.has_inner(key) {
            return None;
        }
        match names.reaching(key, (self.id, node), &self.r.font) {
            (Some(p), val, ..) if p.0 == self.id && p.1 != END && p.1 >= after => Some((val, Src::Piece(p.0))),
            _ => None,
        }
    }

    fn split_inputs(&mut self) -> bool {
        true
    }

    fn stop_at(&mut self, pos: usize) -> bool {
        self.text.splits.binary_search(&(self.base + pos)).is_ok()
    }

    fn file(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.r.names.given.file(name)
    }

    fn file_path(&mut self, name: &str) -> Arc<str> {
        self.r.names.given.path(name)
    }

    fn jobname(&mut self) -> Arc<str> {
        self.r.names.given.job.clone()
    }

    fn line_base(&mut self) -> i32 {
        self.r.names.given.base(self.id) as i32
    }

    fn reach(&mut self, key: &Key, node: u32) -> (Val, Src, u32, u32) {
        let (def, val, lo, hi) = self.r.names.reaching(key, (self.id, node), &self.r.font);
        (val, def.map_or(Src::Initial, |d| Src::Piece(d.0)), lo, hi)
    }
}

/// The lines of `run`'s paragraphs: the old ones of equal paragraphs
/// (`old`, with its lines), the others broken now.
fn lines_of(run: &Run, old: Option<(&Run, &[Option<Lines>])>) -> (Vec<Option<Lines>>, usize) {
    let cands = equal_paras(old.map(|o| o.0), run);
    let mut out = Vec::with_capacity(run.paras.len());
    let mut broke = 0;
    let mut off: isize = 0;
    for (i, para) in run.paras.iter().enumerate() {
        let hint = (i as isize + off).max(0) as usize;
        let reuse = old.and_then(|(o, ol)| {
            cands.of(i, hint).find_map(|j| {
                let l = ol.get(j as usize).cloned().flatten()?;
                let q = o.paras.get(j as usize)?;
                (Arc::ptr_eq(q, para) || **q == **para).then_some((j, l))
            })
        });
        match reuse {
            Some((j, l)) => {
                off = j as isize - i as isize;
                out.push(Some(l));
            }
            None => {
                broke += 1;
                out.push(Some(Arc::new(break_lines(para))));
            }
        }
    }
    (out, broke)
}

/// A speculating thread: reduces the thunks the spine has not reached.
fn spec_worker(r: &Red, me: usize) {
    loop {
        let t = {
            let mut g = r.specs.lock().expect("specs");
            loop {
                if g.done {
                    return;
                }
                if let Some(t) = g.queue.pop_front() {
                    let live = matches!(
                        g.slots.get(&t.text).and_then(|m| m.get(&t.pos)),
                        Some(Slot::Waiting(id, ..)) if *id == t.id
                    );
                    if !live || t.cancel.load(Ordering::Acquire) {
                        continue;
                    }
                    g.slots
                        .entry(t.text)
                        .or_default()
                        .insert(t.pos, Slot::Running(t.id, t.entry.clone(), t.cancel.clone()));
                    break t;
                }
                g = r.cv.wait(g).expect("specs");
            }
        };
        let text = r.text(t.text);
        let mut env = REnv { r, id: t.id, text: &text, base: t.pos, cancel: Some(&t.cancel) };
        let run = run_piece(&text.chars[t.pos..], &mut env, None, &[], Some(&t.entry));
        r.work[me].fetch_add(run.ran, Ordering::Relaxed);
        let ok = !run.abandoned && !t.cancel.load(Ordering::Acquire);
        let lines = if ok { lines_of(&run, None).0 } else { Vec::new() };
        {
            let mut g = r.specs.lock().expect("specs");
            let mine = matches!(
                g.slots.get(&t.text).and_then(|m| m.get(&t.pos)),
                Some(Slot::Running(id, ..)) if *id == t.id
            );
            if mine {
                let m = g.slots.entry(t.text).or_default();
                if ok {
                    m.insert(t.pos, Slot::Done(t.id, t.entry, Arc::new(run), lines));
                } else {
                    m.remove(&t.pos);
                }
            }
        }
        r.cv.notify_all();
    }
}

/// The edit of the root text since the memo's: `[start, old_end)` became
/// `[start, new_end)`.
#[derive(Clone, Copy, Debug)]
struct RootEdit {
    start: usize,
    old_end: usize,
    new_end: usize,
}

impl RootEdit {
    fn between(old: &[char], new: &[char]) -> Option<RootEdit> {
        if old.len() == new.len() && old == new {
            return None;
        }
        let start = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        let tail = old[start..].iter().rev().zip(new[start..].iter().rev()).take_while(|(a, b)| a == b).count();
        Some(RootEdit { start, old_end: old.len() - tail, new_end: new.len() - tail })
    }

    /// An old place now (none: inside the edit).
    fn new_of(self, q: usize) -> Option<usize> {
        if q < self.start {
            Some(q)
        } else if q >= self.old_end {
            Some(q - self.old_end + self.new_end)
        } else {
            None
        }
    }

    /// A place now in the old text (none: inside the edit).
    fn old_of(self, p: usize) -> Option<usize> {
        if p < self.start {
            Some(p)
        } else if p >= self.new_end {
            Some(p - self.new_end + self.old_end)
        } else {
            None
        }
    }

    /// Whether the old characters `a..b` are unchanged.
    fn kept(self, a: usize, b: usize) -> bool {
        b <= self.start || a >= self.old_end
    }

    /// Whether the old characters `a..b` hold (or border) the edit.
    fn touches(self, a: usize, b: usize) -> bool {
        a <= self.old_end && b >= self.start
    }
}

/// The page builder's environment in the spine: it reads where it is
/// called, runs the output routine where it fires and installs its
/// definitions between the stretch's nodes; a node after the fire that
/// read another value makes the stretch be reduced again from there.
struct RFold<'a> {
    r: &'a Red,
    memo_old: &'a FxMap<usize, Vec<Arc<Memo>>>,
    k: PieceId,
    node: u32,
    run: Option<Arc<Run>>,
    last_readers: Option<FxMap<Key, u32>>,
    /// Names whose fire reaches nodes that read another value: (name, the
    /// first such node).
    dirty: Vec<(Key, u32)>,
    fires: Vec<(u32, Vec<(Key, Val)>)>,
    memo_new: Vec<Arc<Memo>>,
    routines: usize,
    kept: usize,
}

/// What the page builder (or an output routine's first read) reads at node
/// `node` of stretch `k`: the stretch's own definitions up to it, else the
/// names index there (a fire at that node included).
fn read_at(names: &Names, font: &Arc<Font>, run: Option<&Run>, k: PieceId, node: u32, key: &Key) -> Val {
    let local = run.and_then(|r| r.resolve(key, node as usize + 1));
    let (ipos, ival, ..) = names.reaching(key, (k, node.saturating_add(1)), font);
    match (local, ipos) {
        (Some((d, _)), Some(p)) if p.0 == k && p.1 != END && p.1 >= d => ival,
        (Some((_, v)), _) => v,
        (None, _) => ival,
    }
}

impl RFold<'_> {
    fn routine_read(&self, overlay: &[(Key, Val)], key: &Key) -> Val {
        if let Some((_, v)) = overlay.iter().rev().find(|(x, _)| x == key) {
            return v.clone();
        }
        read_at(&self.r.names, &self.r.font, self.run.as_deref(), self.k, self.node, key)
    }

    /// The last evaluation's run of this routine on this page, if all it
    /// read is the same.
    fn find_memo(&self, rt: &Routine, overlay: &[(Key, Val)]) -> Option<Arc<Memo>> {
        let cands = self.memo_old.get(&memo_key(&rt.page.b))?;
        cands
            .iter()
            .find(|m| {
                (Arc::ptr_eq(&m.toks, &rt.toks) || m.toks == rt.toks)
                    && (Arc::ptr_eq(&m.page, &rt.page.b) || m.page == rt.page.b)
                    && m.fire == rt.reads
                    && m.overlay == overlay
                    && m.reads.iter().all(|(key, val)| {
                        let key = match key {
                            Key::PdfAns(1, o, n, f) if *o == m.ordinal => &Key::PdfAns(1, rt.fire, *n, *f),
                            k => k,
                        };
                        val.seen_as(&self.routine_read(overlay, key))
                    })
            })
            .cloned()
    }

    /// Runs the routine as nodes (tex.web 1025–1026).
    fn run_routine(&self, rt: &Routine, overlay: &[(Key, Val)]) -> Memo {
        let mut env = RRoutine { fold: self, overlay, fire: rt.fire };
        let entry = output::output_entry(&rt.page, &rt.reads, &rt.toks);
        let run = Arc::new(output::run_routine(&entry, &mut env));
        let paras: Vec<ParaLines> = run
            .paras
            .iter()
            .map(|p| (p.clone(), if p.lb.is_some() { Arc::new(break_lines(p)) } else { Arc::new(Vec::new()) }))
            .collect();
        let mut items = Vec::new();
        let mut pd = IGNORE_DEPTH;
        for it in material(&paras) {
            push_vertical(&mut items, &mut pd, it);
        }
        let lines: Vec<Option<Lines>> = paras.iter().map(|(_, l)| Some(l.clone())).collect();
        let log = logs::texts(&logs::fragment(&[], &run, &lines));
        Memo {
            ordinal: rt.fire,
            toks: rt.toks.clone(),
            page: rt.page.b.clone(),
            fire: rt.reads.clone(),
            overlay: overlay.to_vec(),
            reads: run.reads.iter().map(|(k, _, v)| (k.clone(), v.clone())).collect(),
            exports: run.exports.clone(),
            items,
            log,
            errors: run.errors.clone(),
        }
    }

    /// Whether a node after the fire read `key` and would read another
    /// value now: a read from before the stretch (or between its nodes)
    /// whose value the names index no longer gives there, or a read of the
    /// stretch's own definition the fire's now overrides.
    fn reread(&mut self, run: &Run, key: &Key) -> bool {
        let node = self.node;
        let after: Vec<&(Key, u32, Val)> = run.values_read(key).iter().filter(|x| x.1 > node).collect();
        let names = &self.r.names;
        if after.iter().any(|(_, n, v)| !v.seen_as(&names.read_now(key, (self.k, *n), &self.r.font).1)) {
            return true;
        }
        if after.is_empty() && run.resolve(key, node as usize + 1).is_some() {
            let lr = self.last_readers.get_or_insert_with(|| run.last_readers());
            return lr.get(key).is_some_and(|&j| j > node);
        }
        false
    }
}

impl FoldEnv for RFold<'_> {
    fn xetex(&self) -> bool {
        self.r.names.given.xe.is_some()
    }

    fn read(&mut self, key: &Key) -> Val {
        read_at(&self.r.names, &self.r.font, self.run.as_deref(), self.k, self.node, key)
    }

    fn toks_text(&mut self, toks: &[Tok]) -> Vec<u8> {
        let esc = match self.read(&Key::IntPar(crate::val::IntPar::Escapechar)) {
            Val::Int(e) => e,
            _ => -1,
        };
        let cs = self.r.names.cs.clone();
        crate::pdf::toks_text(toks, &|n| cs.name(n), esc, &mut |c| self.read(&Key::Cat(c)) == Val::Cat(11))
    }

    fn fire(&mut self, defs: Vec<(Key, Val)>, routine: Option<Routine>) -> Fired {
        let mut fired = Fired::default();
        let pos = (self.k, self.node);
        let mut all = defs.clone();
        if let Some(rt) = routine {
            let m = if let Some(m) = self.find_memo(&rt, &defs) {
                self.kept += 1;
                fired.memo = true;
                m
            } else {
                self.routines += 1;
                Arc::new(self.run_routine(&rt, &defs))
            };
            fired.items.clone_from(&m.items);
            if m.ordinal != rt.fire {
                for it in &mut fired.items {
                    if let crate::item::Item::Pdf(p) = it
                        && p.tag[0] == 1
                        && p.tag[1] == m.ordinal
                    {
                        let mut q = (**p).clone();
                        q.tag[1] = rt.fire;
                        *it = crate::item::Item::Pdf(Arc::new(q));
                    }
                }
            }
            fired.log.clone_from(&m.log);
            fired.errors.clone_from(&m.errors);
            for (key, val) in &m.exports {
                if *key == Key::Page(DEAD_CYCLES)
                    && let Val::Int(d) = val
                {
                    fired.dead_cycles = Some(*d);
                }
                match all.iter_mut().find(|x| x.0 == *key) {
                    Some(x) => x.1 = val.clone(),
                    None => all.push((key.clone(), val.clone())),
                }
            }
            self.memo_new.push(m);
        }
        let names = &self.r.names;
        let mut changed: Vec<Key> = Vec::new();
        for (key, val) in &all {
            if names.at(key, pos).as_ref() != Some(val) {
                names.set(pos, key, Some(val));
                changed.push(key.clone());
            }
        }
        match self.fires.iter_mut().find(|x| x.0 == self.node) {
            Some(x) => {
                for (key, val) in all {
                    match x.1.iter_mut().find(|d| d.0 == key) {
                        Some(d) => d.1 = val,
                        None => x.1.push((key, val)),
                    }
                }
            }
            None => self.fires.push((self.node, all)),
        }
        if let Some(run) = self.run.clone() {
            for key in &changed {
                refresh_export(&self.r.names, &self.r.font, self.k, &run, key);
            }
            for key in &changed {
                if matches!(key, Key::Page(_)) {
                    continue;
                }
                if self.reread(&run, key) {
                    self.dirty.push((key.clone(), self.node + 1));
                    fired.halt = true;
                }
            }
        }
        fired
    }
}

/// An output routine's reads: the fold's definitions, then the document's
/// where the page builder is called.
struct RRoutine<'a> {
    fold: &'a RFold<'a>,
    overlay: &'a [(Key, Val)],
    fire: u32,
}

impl Env for RRoutine<'_> {
    fn cs(&self) -> &Arc<CsTable> {
        &self.fold.r.names.cs
    }

    fn pdf_scope(&self) -> (u32, u32) {
        (1, self.fire)
    }

    fn interaction_option(&self) -> Option<i32> {
        self.fold.r.names.given.interaction_option
    }

    fn font_file(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.fold.r.fonts.get(name)
    }

    fn xetex(&self) -> bool {
        self.fold.r.names.given.xe.is_some()
    }

    fn native_font(&mut self, name: &str, size: i32, tracing: i32) -> crate::native::Loaded {
        self.fold.r.names.given.native_font(name, size, tracing)
    }

    fn reach(&mut self, key: &Key, _node: u32) -> (Val, Src, u32, u32) {
        (self.fold.routine_read(self.overlay, key), Src::Initial, 0, END)
    }
}

/// A run's reads of the page builder's quantities checked against the page
/// as the fold has it, up to node `upto` (none: all): the first that saw
/// another value (its name and node).
fn check_page(scan: &Scan, names: &Names, k: PieceId, reads: &[(u8, u32, Val)], fr: &mut usize, upto: Option<u32>) -> Option<(Key, u32)> {
    while let Some((q, node, read)) = reads.get(*fr).filter(|r| upto.is_none_or(|u| r.1 <= u)) {
        *fr += 1;
        let key = Key::Page(*q);
        let now = Val::Int(scan.page_quantity(*q));
        names.set_fold_value(&key, (k, *node), now.clone());
        if now != *read {
            return Some((key, *node));
        }
    }
    None
}

/// Whether the build answers reads of `key` itself (checked at every
/// adoption: cheap, and not in the names index).
fn given_key(key: &Key) -> bool {
    matches!(
        key,
        Key::File(_) | Key::Job | Key::LineBase | Key::PdfAns(..) | Key::PdfSeed | Key::PdfClock | Key::PdfDate
    )
}

/// The spine's maker: one thread, demanded by the page builder.
struct Spine<'a> {
    r: &'a Red,
    memo: &'a RMemo,
    memo_old: FxMap<usize, Vec<Arc<Memo>>>,
    root_edit: Option<RootEdit>,
    threads: usize,
    segs: Vec<Seg>,
    next_id: PieceId,
    /// The last stretch of the spine, and the last place in the labels.
    last_spine: Option<PieceId>,
    tail: Option<PieceId>,
    /// Names whose definitions may differ from the memo's at the same
    /// place (an adopted reduction's reads of them are compared).
    diverged: FxSet<Key>,
    /// The memo's reduction the spine last adopted.
    last_old: Option<usize>,
    conts: Vec<Cont>,
    /// Each file text's entry (where its speculation starts from).
    file_entry: FxMap<usize, Arc<RState>>,
    /// The next split point not speculated yet, by text.
    spec_next: FxMap<usize, usize>,
    posts: FxMap<PieceId, String>,
    memo_new: Vec<Arc<Memo>>,
    stats: Stats,
}

impl<'a> Spine<'a> {
    fn new_id(&mut self) -> PieceId {
        let id = self.next_id;
        self.next_id += 1;
        assert!(id < VIRT_BASE, "too many reductions");
        id
    }

    /// A place for a stretch of the spine: after the spine's last.
    fn place_spine(&mut self, id: PieceId, text: &RText, pos: usize) {
        {
            let mut l = self.r.names.labels.write().expect("labels");
            l.insert_after(self.last_spine, &[id]);
        }
        if self.tail == self.last_spine {
            self.tail = Some(id);
        }
        self.last_spine = Some(id);
        self.r.names.given.bases.write().expect("bases").insert(id, text.lines_before(pos));
        // (the reads of written files found refuted, at this place)
        for ((tk, start, node, key), v) in &self.memo.file_vals {
            if *tk == text.key && *start == pos {
                self.r.names.set_fold_value(key, (id, *node), v.clone());
            }
        }
    }

    /// A place for a speculated reduction: after everything.
    fn place_spec(&mut self, id: PieceId, text: &RText, pos: usize) {
        {
            let mut l = self.r.names.labels.write().expect("labels");
            l.insert_after(self.tail, &[id]);
        }
        self.tail = Some(id);
        self.r.names.given.bases.write().expect("bases").insert(id, text.lines_before(pos));
    }

    fn unplace(&mut self, id: PieceId) {
        let mut l = self.r.names.labels.write().expect("labels");
        if self.tail == Some(id) {
            self.tail = l.prev(id);
        }
        l.remove(id);
    }

    /// A reduction's reads that do not hold at stretch `id`'s place, each
    /// name with the first node that read it: of the names the build
    /// answers, all; of the others, those that diverged (`all`: every
    /// read). None: the reduction holds there.
    fn stale(&self, id: PieceId, run: &Run, all: bool) -> Vec<(Key, u32)> {
        let names = &self.r.names;
        let font = &self.r.font;
        let holds = |key: &Key, node: u32, val: &Val| -> bool {
            matches!(key, Key::Page(_)) || val.seen_as(&names.read_now(key, (id, node), font).1)
        };
        let mut out: Vec<(Key, u32)> = Vec::new();
        let mut note = |k: &Key, n: u32| match out.last_mut() {
            Some(x) if x.0 == *k => x.1 = x.1.min(n),
            _ => out.push((k.clone(), n)),
        };
        if all || self.diverged.len() > run.reads.len() {
            for (k, n, v) in &run.reads {
                if (all || given_key(k) || self.diverged.contains(k)) && !holds(k, *n, v) {
                    note(k, *n);
                }
            }
            return out;
        }
        for (k, n, v) in &run.reads {
            if given_key(k) && !holds(k, *n, v) {
                note(k, *n);
            }
        }
        let mut keys: Vec<&Key> = self.diverged.iter().collect();
        keys.sort();
        for key in keys {
            let i = run.reads.partition_point(|x| x.0 < *key);
            for (k, n, v) in &run.reads[i..] {
                if k != key {
                    break;
                }
                if !holds(k, *n, v) {
                    note(k, *n);
                }
            }
        }
        out
    }

    /// The memo's reduction of the thunk's place, if its text is the same.
    fn memo_at(&self, th: &Thunk, text: &RText) -> Option<usize> {
        let q = match (&text.key, self.root_edit) {
            (TextKey::Root, Some(e)) => e.old_of(th.pos)?,
            _ => th.pos,
        };
        let cands = self.memo.by_place.get(&(text.key.clone(), q))?;
        cands.iter().copied().find(|&j| {
            let o = &self.memo.segs[j];
            if *o.entry != *th.entry || (o.hash != text.hash && text.key != TextKey::Root) {
                return false;
            }
            let len = text.chars.len();
            match (&text.key, self.root_edit) {
                (TextKey::Root, Some(e)) => {
                    let upto = match o.end {
                        End::TextEnd | End::JobEnd => o.stop,
                        _ => (o.stop + 1).min(o.chars.len()),
                    };
                    if !e.kept(o.start, upto) || e.new_of(o.start) != Some(th.pos) {
                        return false;
                    }
                    let stop = e.new_of(o.stop);
                    match o.end {
                        End::TextEnd => stop == Some(len),
                        _ => stop.is_some_and(|s| s < len || o.end == End::JobEnd),
                    }
                }
                _ => o.chars.len() == len,
            }
        })
    }

    /// The memo's reduction of the same place whatever its text, as the old
    /// run of a reduction again: it, its text, and how much of the new text
    /// to compare it with.
    fn memo_old_run(&self, th: &Thunk, text: &RText) -> Option<(usize, usize)> {
        let q = match (&text.key, self.root_edit) {
            (TextKey::Root, Some(e)) => {
                if th.pos <= e.start {
                    th.pos
                } else {
                    e.old_of(th.pos)?
                }
            }
            _ => th.pos,
        };
        let cands = self.memo.by_place.get(&(text.key.clone(), q))?;
        let j = *cands.iter().find(|&&j| *self.memo.segs[j].entry == *th.entry).or(cands.first())?;
        let o = &self.memo.segs[j];
        let olen = (o.stop + 1).min(o.chars.len()) - o.start;
        let grow = match (&text.key, self.root_edit) {
            (TextKey::Root, Some(e)) if e.touches(o.start, o.stop + 1) => e.new_end as isize - e.old_end as isize,
            (TextKey::Root, _) => 0,
            _ => text.chars.len() as isize - o.chars.len() as isize,
        };
        let window = (olen as isize + grow).max(0) as usize;
        Some((j, window))
    }

    /// The memo's reductions passed over since the last adopted one, up to
    /// `j`: what they defined has diverged.
    fn pass_old(&mut self, j: usize) {
        let from = self.last_old.map_or(0, |x| x + 1);
        for o in self.memo.segs.get(from..j).unwrap_or(&[]) {
            self.diverged.extend(o.run.exports.iter().map(|(k, _)| k.clone()));
            for (_, d) in &o.fires {
                self.diverged.extend(d.iter().map(|(k, _)| k.clone()));
            }
        }
        self.last_old = Some(j);
    }

    /// A speculated reduction of the thunk's place, if one is done or
    /// running on its very entry (waited for); a queued one is taken back.
    fn take_spec(&mut self, th: &Thunk) -> Option<(PieceId, Arc<RState>, Arc<Run>, Vec<Option<Lines>>)> {
        let r = self.r;
        let mut g = r.specs.lock().expect("specs");
        loop {
            let slot = g.slots.get_mut(&th.text).and_then(|m| m.remove(&th.pos))?;
            match slot {
                Slot::Done(id, e, run, lines) => return Some((id, e, run, lines)),
                Slot::Running(id, e, c) if *e == *th.entry => {
                    g.slots.entry(th.text).or_default().insert(th.pos, Slot::Running(id, e, c));
                    g = r.cv.wait(g).expect("specs");
                }
                Slot::Running(id, _, c) | Slot::Waiting(id, c) => {
                    c.store(true, Ordering::Release);
                    drop(g);
                    self.unplace(id);
                    return None;
                }
            }
        }
    }

    /// The speculated reductions of text `t` before `pos` (the spine
    /// passed them) given up.
    fn drop_specs_before(&mut self, t: usize, pos: usize) {
        let r = self.r;
        let gone: Vec<PieceId> = {
            let mut g = r.specs.lock().expect("specs");
            let Some(m) = g.slots.get_mut(&t) else { return };
            let keys: Vec<usize> = m.range(..pos).map(|(k, _)| *k).collect();
            keys.into_iter()
                .filter_map(|k| m.remove(&k))
                .map(|s| match s {
                    Slot::Waiting(id, c) | Slot::Running(id, _, c) => {
                        c.store(true, Ordering::Release);
                        id
                    }
                    Slot::Done(id, ..) => id,
                })
                .collect()
        };
        for id in gone {
            self.unplace(id);
        }
    }

    /// Speculation ahead of the thunk: the next split points of its text.
    fn speculate(&mut self, th: &Thunk) {
        if self.threads < 2 {
            return;
        }
        let text = self.r.text(th.text);
        let ahead = self.threads * 2;
        let mut i = (*self.spec_next.get(&th.text).unwrap_or(&0)).max(text.splits.partition_point(|&s| s <= th.pos));
        let r = self.r;
        let open = {
            let g = r.specs.lock().expect("specs");
            g.slots.get(&th.text).map_or(0, |m| m.range(th.pos + 1..).count())
        };
        let mut n = open;
        let mut tasks = Vec::new();
        while n < ahead && i < text.splits.len() {
            let pos = text.splits[i];
            i += 1;
            if pos >= text.chars.len() {
                break;
            }
            // (a place the memo has a reduction of is the spine's: it
            // adopts it, or reduces it again from its first stale read)
            let at = Thunk { text: th.text, pos, entry: th.entry.clone() };
            if self.memo_old_run(&at, &text).is_some() {
                continue;
            }
            let predicted = match text.key {
                TextKey::Root => Arc::new(RState::entered_after(&RState::start())),
                TextKey::File(_) => self.file_entry.get(&th.text).cloned().unwrap_or_else(|| th.entry.clone()),
            };
            let id = self.new_id();
            self.place_spec(id, &text, pos);
            tasks.push(SpecTask { text: th.text, pos, id, entry: predicted, cancel: Arc::new(AtomicBool::new(false)) });
            n += 1;
        }
        self.spec_next.insert(th.text, i);
        if tasks.is_empty() {
            return;
        }
        {
            let mut g = r.specs.lock().expect("specs");
            for t in tasks {
                g.slots
                    .entry(t.text)
                    .or_default()
                    .insert(t.pos, Slot::Waiting(t.id, t.cancel.clone()));
                g.queue.push_back(t);
            }
        }
        r.cv.notify_all();
    }

    /// Reduces a thunk: adopts the memo's reduction of its place or a
    /// speculated one when their reads hold, else reduces it (keeping what
    /// of theirs still holds). Returns the stretch's index in the spine.
    fn reduce(&mut self, th: &Thunk) -> usize {
        let text = self.r.text(th.text);
        let id = self.new_id();
        self.place_spine(id, &text, th.pos);
        // (the memo: the same text, the same entry, the reads that may
        // have changed unchanged)
        if let Some(j) = self.memo_at(th, &text) {
            let monotone = self.last_old.is_none_or(|l| j > l);
            if monotone {
                self.pass_old(j);
            }
            let o = &self.memo.segs[j];
            if self.stale(id, &o.run, !monotone).is_empty() {
                let stop = match (&text.key, self.root_edit) {
                    (TextKey::Root, Some(e)) => e.new_of(o.stop).unwrap_or(o.stop),
                    _ => o.stop,
                };
                self.stats.kept += 1;
                return self.push(Seg {
                    id,
                    text: th.text,
                    start: th.pos,
                    stop,
                    end: o.end,
                    entry: th.entry.clone(),
                    run: o.run.clone(),
                    lines: o.lines.clone(),
                    fires: Vec::new(),
                    old: Some(j),
                });
            }
            // (not adopted: what it defined has diverged)
            let o = &self.memo.segs[j];
            let keys: Vec<Key> = o
                .run
                .exports
                .iter()
                .map(|(k, _)| k.clone())
                .chain(o.fires.iter().flat_map(|(_, d)| d.iter().map(|x| x.0.clone())))
                .collect();
            self.diverged.extend(keys);
        }
        // (a speculated reduction of this place)
        let mut old: Option<(Arc<Run>, Vec<Option<Lines>>, Arc<[char]>, usize, usize)> = None;
        let mut dirty: Vec<(Key, u32)> = Vec::new();
        if let Some((sid, entry, run, lines)) = self.take_spec(th) {
            self.unplace(sid);
            dirty = self.stale(id, &run, true);
            if *entry == *th.entry && dirty.is_empty() {
                self.stats.ran += 1;
                self.stats.commands_run += run.ran;
                let (stop, end) = self.end_of(&text, th.pos, &run);
                return self.push(Seg { id, text: th.text, start: th.pos, stop, end, entry: th.entry.clone(), run, lines, fires: Vec::new(), old: None });
            }
            let olen = (th.pos + run.out().pos + 1).min(text.chars.len()) - th.pos;
            old = Some((run, lines, text.chars.clone(), th.pos, olen));
        }
        if old.is_none()
            && let Some((j, window)) = self.memo_old_run(th, &text)
        {
            let o = &self.memo.segs[j];
            dirty = self.stale(id, &o.run, true);
            old = Some((o.run.clone(), o.lines.clone(), o.chars.clone(), o.start, window));
        }
        let mut env = REnv { r: self.r, id, text: &text, base: th.pos, cancel: None };
        let run = match &old {
            Some((orun, _, ochars, ostart, window)) => {
                let oend = (ostart + orun.out().pos + 1).min(ochars.len());
                run_piece_win(&text.chars[th.pos..], *window, &mut env, Some((orun, &ochars[*ostart..oend])), &dirty, Some(&th.entry))
            }
            None => run_piece(&text.chars[th.pos..], &mut env, None, &[], Some(&th.entry)),
        };
        let run = Arc::new(run);
        self.r.work[0].fetch_add(run.ran, Ordering::Relaxed);
        if old.is_some() {
            // (a reduction made again from its stale reads: un-reduced)
            self.stats.reruns += 1;
        }
        let (lines, broke) = lines_of(&run, old.as_ref().map(|(r, l, ..)| (&**r, &l[..])));
        self.stats.breaks_run += broke;
        self.stats.ran += 1;
        self.stats.commands_run += run.ran;
        let (stop, end) = self.end_of(&text, th.pos, &run);
        self.push(Seg { id, text: th.text, start: th.pos, stop, end, entry: th.entry.clone(), run, lines, fires: Vec::new(), old: None })
    }

    /// Where a reduction of `text` from `start` stopped, and why.
    fn end_of(&self, text: &RText, start: usize, run: &Run) -> (usize, End) {
        let out = run.out();
        let stop = (start + out.pos).min(text.chars.len());
        let ended = run.exports.binary_search_by(|(k, _)| k.cmp(&Key::Ended)).is_ok_and(|i| run.exports[i].1 != Val::Int(0));
        let end = if ended || out.halted() {
            End::JobEnd
        } else if run.input().is_some() {
            End::Input
        } else if stop >= text.chars.len() || (text.key != TextKey::Root && out.ends_file()) {
            End::TextEnd
        } else {
            End::Split
        };
        (stop, end)
    }

    /// A stretch joins the spine: its definitions enter the names index at
    /// its end; one reduced now has diverged what it defines.
    fn push(&mut self, seg: Seg) -> usize {
        for (key, _) in &seg.run.exports {
            refresh_export(&self.r.names, &self.r.font, seg.id, &seg.run, key);
        }
        if seg.old.is_none() {
            self.diverged.extend(seg.run.exports.iter().map(|(k, _)| k.clone()));
        }
        self.segs.push(seg);
        self.segs.len() - 1
    }

    /// Stretch `si` reduced again from the nodes reading `dirty`: its
    /// definitions refreshed, what changed diverged.
    fn again(&mut self, si: usize, dirty: &[(Key, u32)]) {
        let text = self.r.text(self.segs[si].text);
        let (id, start, entry, orun) = {
            let s = &self.segs[si];
            (s.id, s.start, s.entry.clone(), s.run.clone())
        };
        let oend = (start + orun.out().pos + 1).min(text.chars.len());
        let window = oend - start;
        let mut env = REnv { r: self.r, id, text: &text, base: start, cancel: None };
        let run = Arc::new(run_piece_win(&text.chars[start..], window, &mut env, Some((&orun, &text.chars[start..oend])), dirty, Some(&entry)));
        self.stats.reruns += 1;
        self.stats.commands_run += run.ran;
        self.r.work[0].fetch_add(run.ran, Ordering::Relaxed);
        let changed = changed_names(Some(&orun), &run);
        for key in changed.iter().chain(orun.exports.iter().map(|(k, _)| k)) {
            refresh_export(&self.r.names, &self.r.font, id, &run, key);
        }
        self.diverged.extend(changed);
        let (lines, broke) = lines_of(&run, Some((&orun, &self.segs[si].lines)));
        self.stats.breaks_run += broke;
        let (stop, end) = self.end_of(&text, start, &run);
        let s = &mut self.segs[si];
        if s.stop != stop || s.end != end {
            s.old = None;
        }
        s.run = run;
        s.lines = lines;
        s.stop = stop;
        s.end = end;
    }

    /// The page builder fed stretch `si`'s material, from its item `skip`:
    /// where a fire's definitions or a page quantity reach a node that read
    /// another value, the stretch is reduced again from there and the
    /// feeding goes on from the item it stopped at.
    fn scan_seg(&mut self, si: usize, scan: &mut Scan) {
        let mut skip = 0usize;
        let mut times = 0u32;
        loop {
            let (id, run, lines) = {
                let s = &self.segs[si];
                (s.id, s.run.clone(), s.lines.clone())
            };
            let paras: Vec<ParaLines> =
                run.paras.iter().zip(&lines).map(|(p, l)| (p.clone(), l.clone().unwrap_or_default())).collect();
            let nodes = run.para_nodes().to_vec();
            let freads = page_reads(&run);
            let memo_old = std::mem::take(&mut self.memo_old);
            let mut fold = RFold {
                r: self.r,
                memo_old: &memo_old,
                k: id,
                node: 0,
                run: Some(run.clone()),
                last_readers: None,
                dirty: Vec::new(),
                fires: std::mem::take(&mut self.segs[si].fires),
                memo_new: Vec::new(),
                routines: 0,
                kept: 0,
            };
            let mut fr = 0usize;
            let mut i = 0usize;
            let mut again: Option<(Vec<(Key, u32)>, usize)> = None;
            'feed: for (pj, pl) in paras.iter().enumerate() {
                let node = nodes.get(pj).copied().unwrap_or(0);
                for it in material(std::slice::from_ref(pl)) {
                    if i < skip {
                        while freads.get(fr).is_some_and(|r| r.1 <= node) {
                            fr += 1;
                        }
                        i += 1;
                        continue;
                    }
                    if let Some(d) = check_page(scan, &self.r.names, id, &freads, &mut fr, Some(node)) {
                        again = Some((vec![d], i));
                        break 'feed;
                    }
                    fold.node = node;
                    scan.feed(it, (id, i as u32), &mut fold);
                    i += 1;
                    if scan.halt {
                        scan.halt = false;
                        again = Some((std::mem::take(&mut fold.dirty), i));
                        break 'feed;
                    }
                }
            }
            if again.is_none()
                && let Some(d) = check_page(scan, &self.r.names, id, &freads, &mut fr, None)
            {
                again = Some((vec![d], i));
            }
            self.stats.routines += fold.routines;
            self.stats.routines_kept += fold.kept;
            self.memo_new.append(&mut fold.memo_new);
            self.segs[si].fires = std::mem::take(&mut fold.fires);
            drop(fold);
            self.memo_old = memo_old;
            match again {
                None => break,
                Some((dirty, at)) => {
                    times += 1;
                    assert!(times < RUN_LIMIT, "a stretch was reduced again {RUN_LIMIT} times for the page builder (a livelock)");
                    self.again(si, &dirty);
                    skip = at;
                }
            }
        }
        // (the page builder's definitions here: as the memo's, or diverged)
        let s = &self.segs[si];
        let same = s.old.is_some_and(|j| {
            let o = &self.memo.segs[j].fires;
            o.len() == s.fires.len()
                && o.iter().all(|(n, d)| s.fires.iter().any(|(m, e)| m == n && e.len() == d.len() && d.iter().all(|x| e.contains(x))))
        });
        if !same {
            let mut keys: Vec<Key> = s.fires.iter().flat_map(|(_, d)| d.iter().map(|x| x.0.clone())).collect();
            if let Some(j) = s.old {
                keys.extend(self.memo.segs[j].fires.iter().flat_map(|(_, d)| d.iter().map(|x| x.0.clone())));
            }
            self.diverged.extend(keys);
        }
    }

    /// What follows stretch `si` in the term: the thunk of the rest of its
    /// text, a file's thunk, its caller's rest, the next job's, or nothing.
    fn next(&mut self, si: usize) -> Option<Thunk> {
        let (t, stop, end, id, out, input) = {
            let s = &self.segs[si];
            (s.text, s.stop, s.end, s.id, s.run.out(), s.run.input().cloned())
        };
        match end {
            End::Split => Some(Thunk { text: t, pos: stop, entry: Arc::new(RState::entered_after(&out)) }),
            End::Input => {
                let f = input.expect("an input");
                let ft = self.file_text(&f);
                let entry = Arc::new(RState::file_start(&out));
                self.file_entry.insert(ft, entry.clone());
                self.conts.push(Cont { text: t, pos: stop, caller: out, file: ft });
                Some(Thunk { text: ft, pos: 0, entry })
            }
            End::TextEnd => match self.conts.pop() {
                Some(c) => {
                    self.posts.entry(id).or_insert_with(|| RAW.to_string()).push(')');
                    self.drop_specs_before(c.file, usize::MAX);
                    Some(Thunk { text: c.text, pos: c.pos, entry: Arc::new(RState::rest(Some(&out), &c.caller)) })
                }
                None => None,
            },
            End::JobEnd => {
                // (where the job ended in the root text: there, or at the
                // `\input` the files open were read from)
                let at = if t == 0 { stop } else { self.conts.first().map_or(stop, |c| c.pos) };
                // (the files open when the job ended close at its end)
                for c in std::mem::take(&mut self.conts).into_iter().rev() {
                    self.posts.entry(id).or_insert_with(|| RAW.to_string()).push_str(" )");
                    self.drop_specs_before(c.file, usize::MAX);
                }
                let j = self.r.text(0).next_job(at)?;
                self.drop_specs_before(0, j);
                Some(Thunk { text: 0, pos: j, entry: Arc::new(RState::start()) })
            }
        }
    }

    /// The text of a file an `\input` read (its split points by content).
    fn file_text(&mut self, f: &FileText) -> usize {
        let chars: Arc<[char]> = f.text.chars().collect::<Vec<char>>().into();
        let hash = hash_of(&*chars);
        {
            let g = self.r.texts.read().expect("texts");
            if let Some(i) = g.iter().position(|x| x.key == TextKey::File(f.name.clone()) && x.hash == hash) {
                return i;
            }
        }
        let splits = match self.memo.cuts.get(&hash) {
            Some(s) => s.clone(),
            None => Arc::new(splits_of(&cut_every(&f.text, self.r.opts.cut_lines))),
        };
        let mut g = self.r.texts.write().expect("texts");
        g.push(Arc::new(RText::new(TextKey::File(f.name.clone()), chars, splits)));
        g.len() - 1
    }
}

/// The places between consecutive texts of a cut.
fn splits_of(texts: &[Arc<[char]>]) -> Vec<usize> {
    let mut v = Vec::with_capacity(texts.len());
    let mut at = 0usize;
    for t in texts {
        at += t.len();
        v.push(at);
    }
    v
}

fn dump_seg(out: &mut String, i: usize, s: &Seg, text: &RText, cs: &CsTable) {
    let how = if s.old.is_some() { "kept" } else { "ran" };
    let r = &s.run;
    let _ = writeln!(
        out,
        "piece {i} (id {}) [{how}; {} nodes, {} run]: {} reads from before it, {} definitions after it, {} paragraphs; {:?} at {}; {:?}",
        s.id,
        r.nodes_len(),
        r.ran,
        r.read_count(),
        r.exports.len(),
        r.paras.len(),
        text.key,
        s.start,
        text.chars[s.start..s.stop.min(text.chars.len())].iter().take(40).collect::<String>()
    );
    for (i, n) in r.chain.nodes.iter().enumerate() {
        let _ = write!(out, "  n{i} {:?} {}", n.kind, Shown(&n.cmd, cs));
        for rd in n.reads.iter() {
            match rd.src {
                Src::Node(d) => {
                    let _ = write!(out, " | reads {} = {} from n{d}", Shown(&rd.key, cs), Shown(&rd.val, cs));
                }
                Src::Piece(p) => {
                    let _ = write!(out, " | reads {} = {} from piece id {p}", Shown(&rd.key, cs), Shown(&rd.val, cs));
                }
                Src::Initial => {
                    let _ = write!(out, " | reads {} = {} initial", Shown(&rd.key, cs), Shown(&rd.val, cs));
                }
            }
        }
        if let Some(c) = n.phi() {
            let _ = write!(out, " | test={}", c.test);
        }
        for d in n.defs() {
            let _ = write!(out, " | defines {}{} = {}", if d.global { "global " } else { "" }, Shown(&d.key, cs), Shown(&d.val, cs));
        }
        out.push('\n');
    }
}

impl Build {
    /// An update by graph reduction ([`self`]): the root text cut, then
    /// evaluations over the memo until the loop-back settles.
    pub(super) fn reduce_update(&mut self, src: &str, opts: Options) -> Output {
        let t0 = Instant::now();
        self.builds += 1;
        self.etex_last = opts.etex;
        if opts.cut_lines != self.cut_lines {
            self.cut_lines = opts.cut_lines;
            self.texts.clear();
            self.src.clear();
            self.starts.clear();
            self.rmemo = RMemo::default();
        }
        let (texts, starts) = self.cut(src);
        self.src.clear();
        self.src.push_str(src);
        self.starts = starts;
        self.texts.clone_from(&texts);
        let root: Arc<[char]> = texts.iter().flat_map(|t| t.iter().copied()).collect::<Vec<char>>().into();
        let splits = Arc::new(splits_of(&texts));
        let time_cut = t0.elapsed();
        self.pdf.rounds = 0;
        self.file_rounds = 0;
        self.derived.clear();
        self.derived_side.clear();
        self.notes.clear();
        let mut carried = Stats::default();
        let mut rounds = 0;
        loop {
            rounds += 1;
            if let Some(b) = &self.board {
                b.round.store(rounds, Ordering::Relaxed);
                b.pages.store(0, Ordering::Relaxed);
                b.phase.store(1, Ordering::Relaxed);
            }
            let t_round = Instant::now();
            let last = rounds >= self.max_rounds.max(1);
            match self.reduce_evaluate(opts, root.clone(), splits.clone(), time_cut, last) {
                Eval::Done(out) => {
                    let mut out = *out;
                    carried.round_walls.push(t_round.elapsed());
                    out.stats.absorb(carried);
                    out.stats.rounds = rounds;
                    out.stats.forced_last = last;
                    out.stats.wall = t0.elapsed();
                    return out;
                }
                Eval::Again(mut st) => {
                    st.round_walls.push(t_round.elapsed());
                    carried.absorb(st);
                }
            }
        }
    }

    /// One evaluation: the spine reduced from the root's first thunk, the
    /// page builder fed as it grows, then the pages, the file, the log.
    fn reduce_evaluate(&mut self, opts: Options, root: Arc<[char]>, splits: Arc<Vec<usize>>, time_cut: Duration, last: bool) -> Eval {
        let t_setup = Instant::now();
        let threads = opts.threads.max(1);
        let mut given = Given::new(self.inputs.clone(), self.job.clone(), self.files.clone());
        given.finder.clone_from(&self.finder);
        given.terminal.clone_from(&self.terminal);
        given.interaction = self.interaction;
        given.interaction_option = self.interaction_option;
        given.limits = self.limits;
        given.pdf_date = self.pdf_date;
        given.pdf_answers = self.pdf.answers.clone();
        given.pdf_seed = self.pdf.seed;
        given.pdf_clock = self.pdf.clock;
        given.xe.clone_from(&self.xe);
        let labels: Arc<RwLock<Labels>> = Arc::default();
        let names = Names::new(given, opts.etex, self.cs.clone(), self.format.clone(), labels);
        names.file_guess.write().expect("file guesses").clone_from(&self.file_guess);
        let memo = std::mem::take(&mut self.rmemo);
        let root_edit = match &memo.root {
            Some(old) => RootEdit::between(old, &root),
            None => None,
        };
        let r = Red {
            work: (0..threads).map(|_| AtomicU64::new(0)).collect(),
            names,
            font: self.font.clone(),
            fonts: self.fonts.clone(),
            opts,
            texts: RwLock::new(vec![Arc::new(RText::new(TextKey::Root, root.clone(), splits))]),
            specs: Mutex::default(),
            cv: Condvar::new(),
        };
        let mut memo_old: FxMap<usize, Vec<Arc<Memo>>> = FxMap::default();
        for m in &memo.routines {
            memo_old.entry(memo_key(&m.page)).or_default().push(m.clone());
        }
        let mut stats = Stats { work: vec![0; threads], tasks: vec![0; threads], ..Stats::default() };
        stats.time_cut = time_cut;
        stats.time_setup = t_setup.elapsed();
        let mut scan = Scan::default();
        let (segs, posts, memo_new, sstats) = std::thread::scope(|s| {
            for id in 1..threads {
                let r = &r;
                std::thread::Builder::new()
                    .name(format!("flow-spec-{id}"))
                    .stack_size(256 << 20)
                    .spawn_scoped(s, move || spec_worker(r, id))
                    .expect("spawn a worker");
            }
            let mut sp = Spine {
                r: &r,
                memo: &memo,
                memo_old,
                root_edit,
                threads,
                segs: Vec::new(),
                next_id: 0,
                last_spine: None,
                tail: None,
                diverged: FxSet::default(),
                last_old: None,
                conts: Vec::new(),
                file_entry: FxMap::default(),
                spec_next: FxMap::default(),
                posts: FxMap::default(),
                memo_new: Vec::new(),
                stats: Stats::default(),
            };
            let mut th = Some(Thunk { text: 0, pos: 0, entry: Arc::new(RState::start()) });
            while let Some(t) = th.take() {
                sp.speculate(&t);
                let si = sp.reduce(&t);
                sp.scan_seg(si, &mut scan);
                let (tx, start, stop) = {
                    let g = &sp.segs[si];
                    (g.text, g.start, g.stop)
                };
                let _ = start;
                sp.drop_specs_before(tx, stop);
                th = sp.next(si);
                if let Some(b) = &self.board {
                    b.settled.store(sp.segs.len(), Ordering::Relaxed);
                }
            }
            // (the end of the document: as if it ended with `\end`)
            {
                let memo_old = std::mem::take(&mut sp.memo_old);
                let (k, node, run) = sp.segs.last().map_or((0, 0, None), |g| (g.id, g.run.nodes_len().saturating_sub(1) as u32, Some(g.run.clone())));
                let mut fold = RFold {
                    r: &r,
                    memo_old: &memo_old,
                    k,
                    node,
                    run,
                    last_readers: None,
                    dirty: Vec::new(),
                    fires: Vec::new(),
                    memo_new: Vec::new(),
                    routines: 0,
                    kept: 0,
                };
                scan.finish((k, u32::MAX), &mut fold);
                sp.stats.routines += fold.routines;
                sp.stats.routines_kept += fold.kept;
                sp.memo_new.append(&mut fold.memo_new);
                if let Some(g) = sp.segs.last_mut() {
                    for (n, d) in fold.fires {
                        g.fires.push((n, d));
                    }
                }
            }
            {
                let mut g = r.specs.lock().expect("specs");
                g.done = true;
                g.queue.clear();
                for m in g.slots.values() {
                    for s in m.values() {
                        if let Slot::Waiting(_, c) | Slot::Running(_, _, c) = s {
                            c.store(true, Ordering::Release);
                        }
                    }
                }
            }
            r.cv.notify_all();
            (sp.segs, sp.posts, sp.memo_new, sp.stats)
        });
        stats.work = r.work.iter().map(|w| w.load(Ordering::Relaxed)).collect();
        stats.ran = sstats.ran;
        stats.kept = sstats.kept;
        stats.reruns = sstats.reruns;
        stats.commands_run = sstats.commands_run;
        stats.breaks_run = sstats.breaks_run;
        stats.routines = sstats.routines;
        stats.routines_kept = sstats.routines_kept;
        let texts: Vec<Arc<RText>> = r.texts.read().expect("texts").clone();
        let names = &r.names;
        let order: Vec<PieceId> = segs.iter().map(|g| g.id).collect();
        let dense: FxMap<PieceId, usize> = order.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        let t_join = Instant::now();
        // (the pages' bodies: the last evaluation's for an equal page)
        let pages: Vec<Page>;
        let orgs: Vec<PageOrg>;
        {
            let (p, o) = scan.take_pages();
            pages = p;
            orgs = o;
        }
        let numbers = dvi::numbers(std::iter::once(self.format_fonts()).chain(segs.iter().map(|g| &g.run.fonts[..])));
        let buf = names.given.limits.dvi_buf_size;
        let old_pages = std::mem::take(&mut self.pages);
        let mut bodies: Vec<Arc<dvi::Body>> = Vec::with_capacity(pages.len());
        for (i, pg) in pages.iter().enumerate() {
            match old_pages.get(i) {
                Some((op, b)) if op == pg => {
                    bodies.push(b.clone());
                    stats.pages_reused += 1;
                }
                _ => {
                    bodies.push(Arc::new(dvi::page_body_in(pg, &numbers, &[], &[], 0, buf)));
                    stats.pages_shipped += 1;
                }
            }
        }
        let loaded_by_piece: Vec<u32> = {
            let mut m = dvi::numbers([self.format_fonts()]);
            segs.iter()
                .map(|g| {
                    dvi::extend(&mut m, &g.run.fonts);
                    m.len() as u32
                })
                .collect()
        };
        let mut errors = Vec::new();
        let mut pdf_answers = None;
        let pdf_tail: crate::page::PdfEvents = Arc::new(std::mem::take(&mut scan.pdf));
        let (out, pdf, again) = if pages.first().is_some_and(|p| p.ship.pdf.is_some()) {
            let format_fonts = self.format_fonts().len() as u32;
            let ship_fonts: Vec<u32> = orgs
                .iter()
                .map(|o| match dense.get(&o.call.0).and_then(|d| d.checked_sub(1)) {
                    Some(d) => loaded_by_piece.get(d).or(loaded_by_piece.last()).copied().unwrap_or(format_fonts),
                    None => format_fonts,
                })
                .collect();
            let w = pdf_file(names, &r.font, &r.fonts, order.last().copied(), &numbers, ship_fonts, &pages, &orgs, &pdf_tail, &mut bodies);
            errors.extend(w.errors.into_iter().map(|e| format!("pdf: {e}")));
            pdf_answers = Some(w.answers);
            (Vec::new(), w.bytes, w.again)
        } else {
            let xdv = names.given.xe.is_some();
            let (out, again) = dvi::assemble_as(&numbers, &pages, &mut bodies, buf, xdv);
            (out, Vec::new(), again)
        };
        stats.bodies_again = again;
        // (the log: each stretch's part, the page builder's calls' pages)
        let marks: Vec<(u32, u32)> = scan.marks.clone();
        let call_logs: CallLogs = scan.call_logs.clone();
        let frags: Vec<Frag> = segs
            .iter()
            .map(|g| {
                let t = &texts[g.text];
                logs::fragment(&t.chars[g.start..g.stop.min(t.chars.len())], &g.run, &g.lines)
            })
            .collect();
        let virt = RwLock::new(Virt::default());
        let (log, writes, probes) = {
            let frefs: Vec<&Frag> = frags.iter().collect();
            let end = order.len().checked_sub(1).map_or((0, 0), |j| (j, segs[j].run.nodes_len()));
            let places = logs::page_places(&frefs, &marks, pages.len(), end);
            let mut page_io = Vec::with_capacity(pages.len());
            for (j, (pg, at)) in pages.iter().zip(&places).enumerate() {
                let io = match at.and_then(|(x, i)| segs.get(x).map(|g| (g, i))) {
                    Some((g, i)) if bodies[j].io && pg.builder => {
                        let t = &texts[g.text];
                        let mut env = JoinEnv { names, font: &r.font, fonts: &r.fonts, k: g.id, cut: false, split: false, stop: usize::MAX, virt: &virt };
                        let rr = regen(&mut env, &t.chars[g.start..], &g.run);
                        let pos = crate::pdf::content::write_positions(pg);
                        crate::graph::page_io(&rr.chain, i, &mut env, &pg.b, &pos, &mut errors)
                    }
                    _ => String::new(),
                };
                page_io.push(io);
            }
            let parts: Vec<logs::Part<'_>> = segs
                .iter()
                .zip(&frags)
                .map(|(g, f)| logs::Part { frag: f, base: names.given.base(g.id) as usize, post: posts.get(&g.id).map(String::as_str) })
                .collect();
            logs::replay_probed(logs::gather(&parts, &marks, &call_logs, &pages, &page_io), opts.etex)
        };
        // (each read of a file the run wrote, against what it had written of
        // it by the reading node)
        let mut refuted: Vec<((TextKey, usize, u32, Key), Val)> = Vec::new();
        for (x, node, files) in &probes {
            let Some(g) = segs.get(*x) else { continue };
            for (key, nd, read) in g.run.all_reads() {
                let Key::File(name) = key else { continue };
                if nd != node {
                    continue;
                }
                let now = match files.iter().find(|f| *f.0 == **name) {
                    Some((_, text)) => Val::Bytes(text.chars().map(|c| u32::from(c).min(255) as u8).collect()),
                    None => names.given.file(name).map_or(Val::Void, Val::Bytes),
                };
                if now != *read {
                    refuted.push(((texts[g.text].key.clone(), g.start, *nd, key.clone()), now.clone()));
                }
                self.file_guess.insert(name.clone(), now);
            }
        }
        // (each stretch's file and lines before it, for `source_place`)
        self.places = segs
            .iter()
            .map(|g| {
                let file = match &texts[g.text].key {
                    TextKey::Root => None,
                    TextKey::File(n) => Some(n.clone()),
                };
                (g.id, (file, names.given.base(g.id)))
            })
            .collect();
        let mut files_read: Vec<(String, Option<String>)> = Vec::new();
        let mut read_names: FxSet<Arc<str>> = FxSet::default();
        for g in &segs {
            let mut fr: Vec<(u32, &Arc<str>, bool)> = g
                .run
                .all_reads()
                .iter()
                .filter_map(|(key, n, v)| match key {
                    Key::File(name) => Some((*n, name, matches!(v, Val::Bytes(_)))),
                    _ => None,
                })
                .collect();
            fr.sort_by_key(|x| x.0);
            for (_, name, found) in fr {
                if read_names.insert(name.clone()) {
                    files_read.push((name.to_string(), found.then(|| names.given.path(name).to_string())));
                }
            }
        }
        let mut dump = String::new();
        for (i, g) in segs.iter().enumerate() {
            for e in &g.run.errors {
                errors.push(format!("piece {i}: {e}"));
            }
            stats.commands += g.run.nodes_len() as u64;
            if opts.dump {
                dump_seg(&mut dump, i, g, &texts[g.text], &names.cs);
            }
        }
        for e in &scan.errors {
            errors.push(format!("page builder: {e}"));
        }
        if opts.dump {
            for (i, pg) in pages.iter().enumerate() {
                let _ = writeln!(dump, "page {}: {} items", i + 1, pg.b.list.len());
            }
        }
        let jobs = job_of(segs.iter().map(|g| g.text == 0 && texts[0].chars.get(g.start) == Some(&crate::graph::JOB)));
        let mut job_pages = vec![0usize; jobs.last().map_or(1, |j| j + 1)];
        for o in &orgs {
            let j = dense.get(&o.call.0).and_then(|&d| jobs.get(d)).copied().unwrap_or(0);
            job_pages[j] += 1;
        }
        stats.time_join = t_join.elapsed();
        stats.pages = pages.len();
        stats.pieces = segs.len();
        // (the loop-back: what was written and is read again)
        let mut again_why: Option<&str> = None;
        let mut memo_file_vals = memo.file_vals;
        if !refuted.is_empty() {
            if last {
                errors.push("the files read back did not settle".into());
            } else {
                for (k, v) in refuted {
                    memo_file_vals.insert(k, v);
                }
                again_why = Some("read-backs");
            }
        }
        if again_why.is_none()
            && let Some(a) = pdf_answers.as_ref()
            && *a != *self.pdf.answers
        {
            if self.pdf.rounds < PDF_ROUNDS && !last {
                self.pdf.rounds += 1;
                again_why = Some("pdf");
            } else {
                errors.push(format!("! TeX capacity exceeded, sorry [pdf read-back rounds={PDF_ROUNDS}]."));
            }
        }
        if let Some(a) = pdf_answers {
            self.pdf.answers = Arc::new(a);
        }
        if again_why.is_none() && self.converge && !last && self.file_rounds < FILE_ROUNDS {
            let xetex = self.xe.is_some();
            let mut changed = false;
            for (name, text) in &writes {
                if !read_names.contains(name.as_str()) {
                    continue;
                }
                let bytes: Vec<u8> = if xetex {
                    text.as_bytes().to_vec()
                } else {
                    text.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).collect()
                };
                if names.given.file(name).as_deref() != Some(&bytes[..]) {
                    Arc::make_mut(&mut self.files).insert(Arc::from(name.as_str()), Arc::from(bytes));
                    changed = true;
                }
            }
            if let Some(d) = self.derive.as_ref() {
                let made = {
                    let files = &self.files;
                    let known = |n: &str| files.contains_key(n);
                    d.lock().expect("derive").derive(&writes, &known)
                };
                for f in made {
                    self.notes.push(f.note);
                    if let Some(side) = f.side {
                        self.derived_side.retain(|x| x.0 != side.0);
                        self.derived_side.push(side);
                    }
                    let Some(bytes) = f.bytes else { continue };
                    if f.name.is_empty() {
                        continue;
                    }
                    self.derived.retain(|x| x.0 != f.name);
                    self.derived.push((f.name.clone(), bytes.clone()));
                    if self.files.get(f.name.as_str()).is_none_or(|t| **t != *bytes) {
                        Arc::make_mut(&mut self.files).insert(Arc::from(f.name.as_str()), Arc::from(bytes));
                        if read_names.contains(f.name.as_str()) {
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                self.file_rounds += 1;
                again_why = Some("files");
            }
        }
        // (the memo for the next evaluation)
        let mut new_memo = RMemo { root: Some(root), routines: memo_new, file_vals: memo_file_vals, ..RMemo::default() };
        new_memo.cuts = memo.cuts;
        for t in &texts {
            if t.key != TextKey::Root {
                new_memo.cuts.insert(t.hash, t.splits.clone());
            }
        }
        for g in &segs {
            let t = &texts[g.text];
            let j = new_memo.segs.len();
            new_memo.by_place.entry((t.key.clone(), g.start)).or_default().push(j);
            new_memo.segs.push(OldSeg {
                hash: t.hash,
                chars: t.chars.clone(),
                start: g.start,
                stop: g.stop,
                end: g.end,
                entry: g.entry.clone(),
                run: g.run.clone(),
                lines: g.lines.clone(),
                fires: g.fires.clone(),
            });
        }
        self.rmemo = new_memo;
        self.pieces = segs
            .iter()
            .map(|g| {
                let t = &texts[g.text];
                Kept { id: g.id, text: t.chars[g.start..g.stop.min(t.chars.len())].into(), run: Some(g.run.clone()), lines: g.lines.clone() }
            })
            .collect();
        self.dense = dense;
        self.pages = pages.into_iter().zip(bodies).collect();
        self.page_orgs = orgs;
        self.marks = marks;
        self.call_logs = call_logs;
        self.pdf.tail = pdf_tail;
        if again_why.is_some() {
            return Eval::Again(stats);
        }
        stats.pdf_rounds = self.pdf.rounds;
        let origins = self.pages.iter().map(|(_, b)| b.pdf.as_ref().map_or_else(Vec::new, |p| p.origins.0.clone())).collect();
        Eval::Done(Box::new(Output {
            origins,
            dvi: out,
            pdf,
            stats,
            errors,
            dump,
            log,
            writes,
            job_pages,
            files_read,
            derived: self.derived.clone(),
            derived_side: self.derived_side.clone(),
            notes: self.notes.clone(),
        }))
    }
}
