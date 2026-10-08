//! The TeX layer's [`phi::Lang`]: its values, its ops, and the step that
//! runs one command of `main_control` (DESIGN 3.17.3).

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::cell::RefCell;
use core::marker::PhantomData;
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use phi::{Arg, Args, Class, ElemId, Lang, NameId, Seq, Step, StepCx, Value, Ver};

use crate::effects::Effect;
use crate::host::{Host, NoHost};
use crate::run::Step as Run;
use crate::ssa::{Fam, SVal, Slot, Versions, set_value, slot_value};
use crate::tex::Tex;
use partex_ssa::Version;

use super::state::PState;
use super::tracker::{Ev, PureTracker};
use super::version::slot_version;

/// The chain the steps' effects go on, in program order.
pub const OUTPUT: phi::Chain = phi::Chain(0);

/// A value of the TeX layer.
#[derive(Clone, Default)]
pub enum Val {
    #[default]
    Unit,
    /// The document's lines (the unfold's input).
    Lines(Seq<Val>),
    /// A line of the document, with its end.
    Line(Arc<[u8]>, Ver),
    /// A step's state.
    State(Arc<PState>, Ver),
    /// A slot's value: what a name is defined to.
    Slot(SVal),
    /// A step's effects.
    Fx(Arc<Vec<Effect>>, Ver),
    /// The job's end: its history.
    Done(i32),
    /// A chain's payloads.
    Chain(Seq<Val>),
}

impl core::fmt::Debug for Val {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Val::Unit => write!(f, "Unit"),
            Val::Lines(s) => write!(f, "Lines({})", s.len()),
            Val::Line(l, _) => write!(f, "Line({:?})", alloc::string::String::from_utf8_lossy(l)),
            Val::State(s, v) => write!(f, "State({s:?}, {:x})", v.0 & 0xffff_ffff),
            Val::Slot(s) => write!(f, "Slot({:x})", s.0.0 & 0xffff_ffff),
            Val::Fx(x, _) => write!(f, "Fx({})", x.len()),
            Val::Done(h) => write!(f, "Done({h})"),
            Val::Chain(s) => write!(f, "Chain({})", s.len()),
        }
    }
}

impl Val {
    /// A line.
    #[must_use]
    pub fn line(b: &[u8]) -> Val {
        Val::Line(Arc::from(b), Ver::of(b))
    }
}

impl Value for Val {
    fn ver(&self) -> Ver {
        match self {
            Val::Unit => Ver::of(&0u8),
            Val::Lines(s) => Ver::node(1, &[s.ver()]),
            Val::Line(_, v) | Val::State(_, v) | Val::Fx(_, v) => *v,
            Val::Slot(s) => Ver(s.0.0),
            Val::Done(h) => Ver::of(&(7u8, *h)),
            Val::Chain(s) => Ver::node(8, &[s.ver()]),
        }
    }
}

/// The ops.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Op {
    #[default]
    Nop,
    /// The document: an unfold of `main_control`, a step a command.
    Main,
    /// A step's effects (operand 0) on [`OUTPUT`].
    Fx,
}

/// The document's main file as the core has it (set by the driver before
/// a run): its bytes, where each line starts, its lines' identities.
pub struct Doc {
    /// The file's name as the host resolved it (`AlphaFile::name`), if
    /// known when the run began.
    pub name: Arc<[u8]>,
    /// The name, learnt when the engine first opens a file holding
    /// [`Doc::bytes`].
    pub name_cell: std::sync::OnceLock<Arc<[u8]>>,
    pub bytes: Arc<[u8]>,
    /// Line `i` starts at `starts[i]`; one more entry, the file's end.
    pub starts: Vec<usize>,
    /// Each line's index by its identity.
    pub index: HashMap<ElemId, usize>,
}

impl Doc {
    /// The main file's name (empty before it is known).
    #[must_use]
    pub fn main(&self) -> &[u8] {
        self.name_cell.get().map_or(&self.name[..], |n| &n[..])
    }

    /// Learn the main file's name from `t`'s open files.
    fn learn<H: Host>(&self, t: &Tex<H, PureTracker>) {
        if self.name_cell.get().is_some() {
            return;
        }
        if let Some(f) = t.input_file[..=t.in_open]
            .iter()
            .flatten()
            .find(|f| f.data[..] == self.bytes[..])
        {
            let _ = self.name_cell.set(f.name.clone());
        }
    }

    /// The lines of `bytes` as `input_ln` reads them (each to its end of
    /// line, CR LF one end), with identities `ids` (one per line).
    #[must_use]
    pub fn lines(bytes: &[u8]) -> Vec<core::ops::Range<usize>> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let b = i;
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                i += 1;
            }
            if i < bytes.len() {
                if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                i += 1;
            }
            out.push(b..i);
        }
        out
    }
}

/// The document of the run in progress.
pub static DOC: RwLock<Option<Arc<Doc>>> = RwLock::new(None);

/// What the steps count (the report's).
#[derive(Default, Clone, Copy, Debug)]
pub struct Stats {
    pub steps: u64,
    /// Runs begun (a step not after the one the engine just ran): the
    /// frontier made complete there.
    pub runs: u64,
    /// Names put in the engine at the runs' starts.
    pub loaded: u64,
    pub reads: u64,
    pub defs: u64,
    pub groups: u64,
    pub effects: u64,
}

/// The engine of this thread and what goes with it.
pub struct Engine<H: Host> {
    pub tex: Tex<H, PureTracker>,
    /// The format's state: the value a name no step defines has.
    pub(crate) base: Tex<NoHost, PureTracker>,
    /// The version of the state the engine is in, if it is a step's.
    pub at: Option<Ver>,
    pub stats: Stats,
    /// Every slot a step of this engine defined (W=1: every name with a
    /// definition; the frontier's complement holds the format's values).
    pub(crate) defined: HashSet<Slot>,
}

std::thread_local! {
    static ENGINE: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
}

/// Install `e` as this thread's engine.
pub fn install<H: Host + 'static>(e: Engine<H>) {
    ENGINE.with(|x| *x.borrow_mut() = Some(Box::new(e)));
}

/// Take this thread's engine back.
#[must_use]
pub fn uninstall<H: Host + 'static>() -> Option<Engine<H>> {
    ENGINE.with(|x| {
        let b = x.borrow_mut().take()?;
        b.downcast::<Engine<H>>().ok().map(|b| *b)
    })
}

fn with_engine<H: Host + 'static, R>(f: impl FnOnce(&mut Engine<H>) -> R) -> R {
    ENGINE.with(|x| {
        let mut b = x.borrow_mut();
        let e = b
            .as_mut()
            .and_then(|b| b.downcast_mut::<Engine<H>>())
            .expect("pure SSA: no engine on this thread");
        f(e)
    })
}

/// The core's name of slot `s`.
fn spelling(s: Slot) -> [u8; 10] {
    let mut b = [0u8; 10];
    b[0] = 0xfe;
    b[1] = s.0 as u8;
    b[2..].copy_from_slice(&s.1.to_le_bytes());
    b
}

/// Slot `s`'s value and version in `t` now.
fn value_of<H: Host>(t: &Tex<H, PureTracker>, s: Slot) -> Option<SVal> {
    let x = slot_value(t, s)?;
    let v = slot_version(t, s).unwrap_or(Version::ABSENT.0);
    Some(SVal::held(Version(v), x))
}

/// The order values are put back in (`ssa::rebuild::put`'s): the page's
/// length and tail before its nodes.
fn rank(a: &Slot) -> u8 {
    use crate::track::page;
    match (a.0, u8::try_from(a.1)) {
        (Fam::Page, Ok(page::LIST_LEN)) => 0,
        (Fam::Page, Ok(page::LIST_TAIL)) => 1,
        (Fam::PageNode, _) => 3,
        _ => 2,
    }
}

/// The TeX layer.
pub struct TexLang<H>(PhantomData<H>);

/// The main file's line count read so far in `t` (its level, or where it
/// ended in the step), if open.
fn main_lines<H: Host>(t: &Tex<H, PureTracker>, main: &[u8]) -> Option<u32> {
    t.input_file[..=t.in_open]
        .iter()
        .flatten()
        .find(|f| &f.name[..] == main)
        .map(|f| f.lines)
}

/// Put the core's document at the main file's levels in `t`, read up to
/// line `idx`.
fn place_main<H: Host>(t: &mut Tex<H, PureTracker>, doc: &Doc, idx: usize) {
    let k = t.in_open;
    for f in t.input_file[..=k].iter_mut().flatten() {
        if f.name[..] == *doc.main() {
            f.data = doc.bytes.clone();
            f.pos = doc.starts[idx.min(doc.starts.len() - 1)];
            f.lines = u32::try_from(idx).unwrap_or(u32::MAX);
        }
    }
}

/// A step: one command.
fn command<H: Host + 'static>(st: &Val, cx: &mut StepCx<'_, TexLang<H>>) -> Step<Val> {
    let Val::State(ps, sv) = st else {
        panic!("pure SSA: a step's state is {st:?}");
    };
    if let Some(h) = ps.finished {
        return Step::Done(Val::Done(h));
    }
    let doc = DOC
        .read()
        .expect("pure SSA: the document")
        .clone()
        .expect("pure SSA: no document");
    let cur = cx.cursor();
    let idx0 = if cur == phi::END {
        doc.starts.len() - 1
    } else {
        *doc.index.get(&cur).expect("pure SSA: a cursor not in the document")
    };
    with_engine::<H, _>(|e| {
        e.stats.steps += 1;
        if e.at != Some(*sv) {
            // (a run begins here: its frontier, the engine's arrays, made
            // complete at the step's place, then the state)
            start_run(e, cx);
            ps.set(&mut e.tex);
            place_main(&mut e.tex, &doc, idx0);
            e.tex.at_checkpoint = true;
        }
        e.tex.tracker.log.borrow_mut().clear();
        e.tex.tracker.file_ends.borrow_mut().clear();
        let r = e.tex.resume();
        let fx = e.tex.take_effects();
        let log = core::mem::take(&mut *e.tex.tracker.log.borrow_mut());
        finish(e, &doc, idx0, r, fx, &log, ps, cx)
    })
}

/// A run begins at this step (not the one the engine just ran): every
/// name a step of the build defined put in the engine as the definition
/// reaching here has it (or the format's value), so that the arrays are
/// the run's frontier, exact from its first command on.
fn start_run<H: Host + 'static>(e: &mut Engine<H>, cx: &mut StepCx<'_, TexLang<H>>) {
    e.stats.runs += 1;
    let mut vals: Vec<(Slot, SVal)> = Vec::with_capacity(e.defined.len());
    let defined: Vec<Slot> = e.defined.iter().copied().collect();
    for s in defined {
        let n = cx.name(&spelling(s));
        if let Some(v) = core_value(e, cx, n, s) {
            vals.push((s, v));
        }
    }
    e.stats.loaded += vals.len() as u64;
    vals.sort_by_key(|(s, _)| rank(s));
    let mut vers = Versions::default();
    for (s, v) in &vals {
        set_value(&mut e.tex, &mut vers, *s, v);
    }
}

/// Name `n`'s (slot `s`'s) value where the step is: its definition's, or
/// the format's.
fn core_value<H: Host + 'static>(
    e: &Engine<H>,
    cx: &mut StepCx<'_, TexLang<H>>,
    n: NameId,
    s: Slot,
) -> Option<SVal> {
    match cx.read(n) {
        Some(v) => match &*v {
            Val::Slot(x) => Some(x.clone()),
            v => panic!("pure SSA: name {s:?} holds {v:?}"),
        },
        None => value_of(&e.base, s),
    }
}

/// The version name `n` has where the step is.
fn core_version<H: Host + 'static>(
    e: &Engine<H>,
    cx: &mut StepCx<'_, TexLang<H>>,
    n: NameId,
    s: Slot,
) -> u128 {
    match cx.read(n) {
        Some(v) => v.ver().0,
        None => slot_version(&e.base, s).unwrap_or(Version::ABSENT.0),
    }
}

/// The command ran on names that were right as far as its first group
/// event: replay the rest to the core (reads checked, definitions, groups),
/// consume the lines it read, emit its effects, and the next state.
fn finish<H: Host + 'static>(
    e: &mut Engine<H>,
    doc: &Doc,
    idx0: usize,
    r: Run,
    fx: Vec<Effect>,
    log: &[Ev],
    prev: &PState,
    cx: &mut StepCx<'_, TexLang<H>>,
) -> Step<Val> {
    use super::tracker::{Kind, kind};
    // (each slot's last write and last restore)
    let mut last_w: HashMap<Slot, usize> = HashMap::new();
    let mut last_r: HashMap<Slot, usize> = HashMap::new();
    for (i, x) in log.iter().enumerate() {
        match *x {
            Ev::Write(s, _) => {
                last_w.insert(s, i);
            }
            Ev::Restore(s) => {
                last_r.insert(s, i);
            }
            _ => {}
        }
    }
    let mut written: HashSet<Slot> = HashSet::new();
    for (i, x) in log.iter().enumerate() {
        match *x {
            Ev::Read(s, v) => {
                if kind(s) != Kind::Name || written.contains(&s) {
                    continue;
                }
                e.stats.reads += 1;
                let n = cx.name(&spelling(s));
                let want = core_version(e, cx, n, s);
                // (the frontier is exact by construction: a read that is not
                // the reaching definition is state the layer does not model)
                assert!(
                    want == v,
                    "pure SSA: the enforcer: {s:?} read {v:x}, the definition reaching it is {want:x}"
                );
            }
            Ev::Write(s, global) => {
                written.insert(s);
                if kind(s) == Kind::Name {
                    e.defined.insert(s);
                }
                if kind(s) != Kind::Name
                    || last_w.get(&s) != Some(&i)
                    || last_r.get(&s).is_some_and(|&j| j > i)
                {
                    continue;
                }
                let Some(v) = value_of(&e.tex, s) else {
                    continue;
                };
                e.stats.defs += 1;
                let n = cx.name(&spelling(s));
                let l = cx.lit(Val::Slot(v));
                cx.define(n, Arg::Local(l), global);
            }
            Ev::Restore(_) => {}
            Ev::Open => {
                e.stats.groups += 1;
                cx.open_group();
            }
            Ev::Close => {
                cx.close_group();
            }
        }
    }
    // the lines of the main file it read
    doc.learn(&e.tex);
    let ended = e
        .tex
        .tracker
        .file_ends
        .borrow()
        .iter()
        .rev()
        .find(|(n, _)| n[..] == *doc.main())
        .map(|(_, l)| *l);
    let now = main_lines(&e.tex, doc.main()).or(ended);
    if let Some(l) = now {
        let l = usize::try_from(l).unwrap_or(usize::MAX);
        for _ in idx0..l {
            cx.next();
        }
    }
    if !fx.is_empty() {
        e.stats.effects += 1;
        let ver = fx_version(&fx);
        let l = cx.lit(Val::Fx(Arc::new(fx), ver));
        cx.leaf(Op::Fx, Class::Effect(OUTPUT), &[Arg::Local(l)]);
    }
    let finished = match r {
        Run::Checkpoint => None,
        Run::Finished(h) => Some(h),
    };
    let (mut ps, v) = PState::of(&mut e.tex, doc.main(), finished);
    let ver = Ver(v);
    e.at = Some(ver);
    // (the next step's key: where it begins, a token list's place by its
    // kind only (its address is the allocator's), and how many steps
    // began there before it)
    let t = &e.tex;
    let tl = t.cur_input.state == partex_engine::web::TOKEN_LIST;
    let base = phi::ver::hash64(&(
        cx.cursor().0,
        t.in_open,
        t.line,
        t.input_ptr,
        t.cur_input.state,
        t.cur_input.index,
        if tl { 0 } else { t.cur_input.loc },
    ));
    let k = if base == prev.kbase { prev.k + 1 } else { 0 };
    ps.kbase = base;
    ps.k = k;
    let key = phi::ver::hash64(&(base, k));
    Step::Next {
        st: Val::State(Arc::new(ps), ver),
        key,
    }
}

/// Effects' version: their content, as the old SSA mode's chunks
/// (`StepEffects`).
fn fx_version(fx: &Vec<Effect>) -> Ver {
    let mut s = partex_engine::persist::Saver::default();
    partex_engine::persist::Persist::save(fx, &mut s);
    Ver(Version::of(&s.into_bytes()).0)
}

impl<H: Host + 'static> Lang for TexLang<H> {
    type Val = Val;
    type Op = Op;

    fn eval(op: Op, args: &Args<'_, Self>) -> Val {
        match op {
            Op::Fx => args.get(0).clone(),
            _ => Val::Unit,
        }
    }

    fn step(op: Op, st: &Val, _args: &Args<'_, Self>, cx: &mut StepCx<'_, Self>) -> Step<Val> {
        match op {
            Op::Main => command::<H>(st, cx),
            _ => unimplemented!("{op:?} is not an unfold"),
        }
    }

    fn as_seq(v: &Val) -> Option<&Seq<Val>> {
        match v {
            Val::Lines(s) | Val::Chain(s) => Some(s),
            _ => None,
        }
    }

    fn chain_val(items: Seq<Val>) -> Val {
        Val::Chain(items)
    }

    fn op_tag(op: Op) -> u64 {
        phi::ver::hash64(&alloc::format!("tex/{op:?}"))
    }
}
