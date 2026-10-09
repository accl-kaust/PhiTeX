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

use super::files::FileDoc;
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
    /// A file's lines (a source, an unfold's input), versioned by the
    /// file's bytes.
    Lines(Seq<Val>, Ver),
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
    /// The job's state where it began reading file `h` at input level
    /// `level` before its first command (the terminal's line named it):
    /// the job's first step calls the file.
    Enter(Arc<PState>, Ver, u64, u16),
    /// A file the job wrote (`\\openout`): its lines so far.
    Store(Arc<super::tracker::Store>),
}

impl core::fmt::Debug for Val {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Val::Unit => write!(f, "Unit"),
            Val::Lines(s, _) => write!(f, "Lines({})", s.len()),
            Val::Line(l, _) => write!(f, "Line({:?})", alloc::string::String::from_utf8_lossy(l)),
            Val::State(s, v) => write!(f, "State({s:?}, {:x})", v.0 & 0xffff_ffff),
            Val::Slot(s) => write!(f, "Slot({:x})", s.0.0 & 0xffff_ffff),
            Val::Fx(x, _) => write!(f, "Fx({})", x.len()),
            Val::Done(h) => write!(f, "Done({h})"),
            Val::Chain(s) => write!(f, "Chain({})", s.len()),
            Val::Enter(_, v, h, l) => write!(f, "Enter({:x}, {h:x}, {l})", v.0 & 0xffff_ffff),
            Val::Store(s) => write!(f, "Store({})", s.lines.len()),
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
            Val::Line(_, v) | Val::Lines(_, v) | Val::State(_, v) | Val::Fx(_, v) => *v,
            Val::Slot(s) => Ver(s.0.0),
            Val::Done(h) => Ver::of(&(7u8, *h)),
            Val::Chain(s) => Ver::node(8, &[s.ver()]),
            Val::Enter(_, v, h, l) => Ver::node(9, &[*v, Ver::of(&(*h, *l))]),
            Val::Store(s) => s.ver,
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
    /// A file read by `\\input` (its key's number), at input level
    /// `level`: a step a command, while the level reads it.
    File(u64, u16),
}


/// What the steps count (the report's).
#[derive(Default, Clone, Copy, Debug)]
pub struct Stats {
    pub steps: u64,
    /// Steps whose state was put in the engine (not the one it was in).
    pub placed: u64,
    /// Frontiers made whole (`defined_reaching`): a run begun where no
    /// step the engine ran is before it.
    pub runs: u64,
    /// Frontiers brought on from the step the engine ran last
    /// (`defined_since`).
    pub since: u64,
    /// Names put in the engine to make the frontiers.
    pub loaded: u64,
    pub reads: u64,
    pub defs: u64,
    pub groups: u64,
    pub effects: u64,
    /// Files called (`\\input` levels).
    pub calls: u64,
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
    /// The start of the step the engine ran last, and the versions of
    /// what it defined (in the engine's arrays already).
    pub(crate) last: Option<phi::Here>,
    pub(crate) last_defs: HashMap<Slot, Version>,
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

/// The families, in the order their names spell them.
const FAMS: [Fam; 33] = [
    Fam::Eqtb,
    Fam::Hash,
    Fam::HashNext,
    Fam::Font,
    Fam::FontTable,
    Fam::Read,
    Fam::Out,
    Fam::Random,
    Fam::Str,
    Fam::Source,
    Fam::Line,
    Fam::Name,
    Fam::Load,
    Fam::Search,
    Fam::Pool,
    Fam::Alloc,
    Fam::Unknown,
    Fam::List,
    Fam::Save,
    Fam::Hyph,
    Fam::Glyphs,
    Fam::HyphWord,
    Fam::Pdf,
    Fam::Dvi,
    Fam::Page,
    Fam::Cond,
    Fam::Mark,
    Fam::PageNode,
    Fam::Sealed,
    Fam::Class,
    Fam::PdfObj,
    Fam::PdfName,
    Fam::PdfNum,
];

/// The core's name of slot `s`.
fn spelling(s: Slot) -> [u8; 10] {
    let mut b = [0u8; 10];
    b[0] = 0xfe;
    b[1] = u8::try_from(FAMS.iter().position(|f| *f == s.0).unwrap_or(0xff)).unwrap_or(0xff);
    b[2..].copy_from_slice(&s.1.to_le_bytes());
    b
}

/// The slot a name spells, if it is one.
fn slot_of(sp: &[u8]) -> Option<Slot> {
    if sp.len() != 10 || sp[0] != 0xfe {
        return None;
    }
    let f = *FAMS.get(usize::from(sp[1]))?;
    Some(Slot(f, i64::from_le_bytes(sp[2..].try_into().ok()?)))
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

/// The file an unfold reads, and its input level, if it is a file's.
type Reading = Option<(Arc<FileDoc>, usize)>;

/// Place file `d` at its level in `t`: the source's bytes, read up to
/// line `idx` (the first step of a file's unfold begins after its first
/// line, which the `\input` that called it read).
fn place_file<H: Host>(t: &mut Tex<H, PureTracker>, d: &FileDoc, level: usize, idx: usize) {
    let n = d.starts.len() - 1;
    let at = if idx == 0 { n.min(1) } else { idx };
    let f = t
        .input_file
        .get_mut(level)
        .and_then(Option::as_mut)
        .filter(|f| f.name[..] == d.name[..])
        .expect("pure SSA: a file's unfold at a level that does not read it");
    f.data = d.bytes.clone();
    f.pos = d.starts[at.min(n)];
    f.lines = u32::try_from(at).unwrap_or(u32::MAX);
}

/// A step: one command.
fn command<H: Host + 'static>(op: Op, st: &Val, cx: &mut StepCx<'_, TexLang<H>>) -> Step<Val> {
    if let Val::Enter(ps, v, h, level) = st {
        // (the file the job began reading before its first command)
        let d = super::files::doc(*h).expect("pure SSA: the first file");
        let src = cx
            .source(&super::files::key(&d.asked))
            .expect("pure SSA: the first file's source");
        let init = cx.lit(Val::State(ps.clone(), *v));
        cx.call(Op::File(*h, *level), src, Arg::Local(init), &[]);
        return Step::Call {
            key: phi::ver::hash64(&(0x656e_7465u32, *h)),
        };
    }
    if let Val::Done(h) = st {
        // (the job ended in a file this unfold called)
        return Step::Done(Val::Done(*h));
    }
    let Val::State(ps, sv) = st else {
        panic!("pure SSA: a step's state is {st:?}");
    };
    if let Some(h) = ps.finished {
        return Step::Done(Val::Done(h));
    }
    let file: Reading = match op {
        Op::File(h, level) => Some((
            super::files::doc(h).expect("pure SSA: a file's unfold with no file"),
            usize::from(level),
        )),
        _ => None,
    };
    let cur = cx.cursor();
    let idx0 = match &file {
        None => 0,
        Some((d, _)) if cur == phi::END => d.starts.len() - 1,
        Some((d, _)) => *d.index.get(&cur).expect("pure SSA: a cursor not in the file"),
    };
    with_engine::<H, _>(|e| {
        e.stats.steps += 1;
        // (the engine's arrays made the frontier at this step: the names
        // defined since the step it ran last, or every name)
        frontier(e, cx);
        if e.at != Some(*sv) {
            e.stats.placed += 1;
            ps.set(&mut e.tex);
        }
        if let Some((d, level)) = &file {
            place_file(&mut e.tex, d, *level, idx0);
        }
        e.tex.at_checkpoint = true;
        e.last = Some(cx.here());
        e.tex.tracker.log.borrow_mut().clear();
        e.tex.tracker.file_ends.borrow_mut().clear();
        e.tex.tracker.loads.borrow_mut().clear();
        e.tex.tracker.store_ev.borrow_mut().clear();
        let r = e.tex.resume();
        let fx = e.tex.take_effects();
        let log = core::mem::take(&mut *e.tex.tracker.log.borrow_mut());
        finish(e, file.as_ref(), idx0, r, fx, &log, ps, cx)
    })
}

/// Make the engine's arrays the frontier at this step (the run's: every
/// name as the definition reaching here has it, or the format's value),
/// from where they are: after the step the engine ran last, the names
/// defined since it (`defined_since`: its own definitions and those of
/// the steps between, and the names of groups closed between); else (a
/// run begins elsewhere) every name the build defined. Not recorded as
/// reads: the step's reads are registered after it runs.
fn frontier<H: Host + 'static>(e: &mut Engine<H>, cx: &mut StepCx<'_, TexLang<H>>) {
    let since = e.last.and_then(|p0| cx.defined_since(p0));
    let mut vals: Vec<(Slot, SVal)> = Vec::new();
    let put = |e: &mut Engine<H>, s: Slot, v: Option<&Val>, vals: &mut Vec<(Slot, SVal)>| {
        let x = match v {
            Some(Val::Slot(x)) => x.clone(),
            Some(v) => panic!("pure SSA: name {s:?} holds {v:?}"),
            None => match value_of(&e.base, s) {
                Some(x) => x,
                None => return,
            },
        };
        // (a value the step before made: in place already)
        if e.last_defs.get(&s) == Some(&x.0) {
            return;
        }
        vals.push((s, x));
    };
    if let Some(list) = since {
        e.stats.since += 1;
        for (n, v) in list {
            let sp = cx.spelling(n);
            if let Some(name) = store_of(sp) {
                put_store(e, name, v.as_deref());
                continue;
            }
            let Some(s) = slot_of(sp) else {
                continue;
            };
            put(e, s, v.as_deref(), &mut vals);
        }
    } else {
        e.stats.runs += 1;
        let all = cx.defined_reaching();
        let mut seen: HashSet<Slot> = HashSet::with_capacity(all.len());
        let mut stores: HashSet<Vec<u8>> = HashSet::new();
        for (n, v) in all {
            let sp = cx.spelling(n);
            if let Some(name) = store_of(sp) {
                stores.insert(name.to_vec());
                put_store(e, name, Some(&*v));
                continue;
            }
            let Some(s) = slot_of(sp) else {
                continue;
            };
            seen.insert(s);
            put(e, s, Some(&*v), &mut vals);
        }
        // (a name the engine holds a later definition of, none reaching
        // here: the format's value)
        let defined: Vec<Slot> = e.defined.iter().copied().filter(|s| !seen.contains(s)).collect();
        for s in defined {
            put(e, s, None, &mut vals);
        }
        e.tex
            .tracker
            .stores
            .borrow_mut()
            .retain(|k, _| stores.contains(k));
        e.last_defs.clear();
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
    file: Option<&(Arc<FileDoc>, usize)>,
    idx0: usize,
    r: Run,
    fx: Vec<Effect>,
    log: &[Ev],
    prev: &PState,
    cx: &mut StepCx<'_, TexLang<H>>,
) -> Step<Val> {
    use super::tracker::{Kind, kind};
    e.last_defs.clear();
    if *DEBUG {
        std::eprintln!(
            "pure step {} (line {}, commands {}): {:?}",
            e.stats.steps,
            e.tex.line,
            e.tex.commands(),
            log
        );
    }
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
            Ev::Write(s, _) => {
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
                e.last_defs.insert(s, v.0);
                let n = cx.name(&spelling(s));
                let l = cx.lit(Val::Slot(v));
                cx.define(n, Arg::Local(l), global_now(&e.tex, s));
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
    // the files the job writes: each read checked (before the command
    // wrote it), each written defined, as it is now
    let sev = core::mem::take(&mut *e.tex.tracker.store_ev.borrow_mut());
    let mut swritten: HashSet<Vec<u8>> = HashSet::new();
    for (w, name, got) in sev {
        if w {
            swritten.insert(name);
            continue;
        }
        if swritten.contains(&name) {
            continue;
        }
        let n = cx.name(&store_spelling(&name));
        let want = cx.read(n).map(|v| v.ver());
        assert!(
            want == got,
            "pure SSA: the enforcer: the file {} the job writes read as {got:?}, the definition reaching it is {want:?}",
            alloc::string::String::from_utf8_lossy(&name)
        );
    }
    for name in swritten {
        let s = e.tex.tracker.stores.borrow().get(&name).cloned();
        if let Some(s) = s {
            let n = cx.name(&store_spelling(&name));
            let l = cx.lit(Val::Store(Arc::new(s)));
            cx.define(n, Arg::Local(l), true);
            e.stats.defs += 1;
        }
    }
    // the files it looked up: sources; an `\input` level it opened, left
    // reading at the top, a call of the file's unfold
    let loads = core::mem::take(&mut *e.tex.tracker.loads.borrow_mut());
    let mut call: Option<(Arg<'_>, u64, usize)> = None;
    for l in &loads {
        let k = super::files::key(&l.name);
        let Some((name, bytes)) = &l.found else {
            // (absent: a file that appears wakes the step)
            let _ = cx.source(&k);
            continue;
        };
        let h = super::files::key_hash(&k);
        let d = super::files::doc_or_insert(h, || FileDoc::new(&l.name, name, bytes.clone(), None));
        assert!(
            d.bytes[..] == bytes[..],
            "pure SSA: the host gave {} other bytes than the build's source",
            alloc::string::String::from_utf8_lossy(name)
        );
        let arg = cx.source_or_insert(&k, d.value.clone());
        let t = &e.tex;
        let j = t.in_open;
        if l.lines
            && j > 0
            && t.input_file[j]
                .as_ref()
                .is_some_and(|f| Arc::ptr_eq(&f.data, bytes))
        {
            call = Some((arg, h, j));
        }
    }
    // the lines of its own file it read, and whether the file's level
    // ended
    let mut done = false;
    if let Some((d, level)) = file {
        let t = &e.tex;
        let open = t
            .input_file
            .get(*level)
            .and_then(Option::as_ref)
            .filter(|f| *level <= t.in_open && f.name[..] == d.name[..]);
        let read = match open {
            Some(f) => Some(f.lines),
            None => {
                done = true;
                t.tracker
                    .file_ends
                    .borrow()
                    .iter()
                    .rev()
                    .find(|(j, n, _)| *j == *level && n[..] == d.name[..])
                    .map(|x| x.2)
            }
        };
        if let Some(l) = read {
            let l = usize::try_from(l).unwrap_or(usize::MAX);
            for _ in idx0..l {
                cx.next();
            }
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
    let main = file.map_or(&b""[..], |(d, _)| &d.name[..]);
    let (mut ps, v) = PState::of(&mut e.tex, main, finished);
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
    let st = Val::State(Arc::new(ps), ver);
    if done {
        assert!(call.is_none(), "pure SSA: a file ended and another began in one command");
        return Step::Done(st);
    }
    if let Some((arg, h, j)) = call {
        e.stats.calls += 1;
        let init = cx.lit(st);
        cx.call(
            Op::File(h, u16::try_from(j).expect("input levels fit u16")),
            arg,
            Arg::Local(init),
            &[],
        );
        return Step::Call { key };
    }
    Step::Next { st, key }
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
            Op::Main | Op::File(..) => command::<H>(op, st, cx),
            _ => unimplemented!("{op:?} is not an unfold"),
        }
    }

    fn as_seq(v: &Val) -> Option<&Seq<Val>> {
        match v {
            Val::Lines(s, _) | Val::Chain(s) => Some(s),
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

/// `PHITEX_PURE_DEBUG=1`: each step's accesses on stderr.
static DEBUG: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var("PHITEX_PURE_DEBUG").is_ok_and(|v| v == "1"));

/// Whether slot `s`'s value now is global: an eqtb entry at level one
/// (assigned `\global`, or at the job's level, where the two are one),
/// as the save stack decides at a group's end (§283); any other name's
/// value is (no group puts it back).
fn global_now<H: Host>(t: &Tex<H, PureTracker>, s: Slot) -> bool {
    if s.0 != Fam::Eqtb {
        return true;
    }
    let p = i32::try_from(s.1).unwrap_or(0);
    let level = if crate::ssa::word_level(p) {
        t.peek_xeq_level(p)
    } else {
        i32::from(t.peek_eqtb(p).b1())
    };
    level <= crate::web::LEVEL_ONE
}

/// The core's name of the file the job writes as `name`.
fn store_spelling(name: &[u8]) -> Vec<u8> {
    let mut b = b"\xfdstore:".to_vec();
    b.extend_from_slice(name);
    b
}

/// The file a name spells, if it is a store's.
fn store_of(sp: &[u8]) -> Option<&[u8]> {
    sp.strip_prefix(b"\xfdstore:")
}

/// The file the job writes as `name`, as the frontier has it (`None`: no
/// `\openout` of it reaches here).
fn put_store<H: Host>(e: &mut Engine<H>, name: &[u8], v: Option<&Val>) {
    let mut st = e.tex.tracker.stores.borrow_mut();
    match v {
        Some(Val::Store(s)) => {
            st.insert(name.to_vec(), (**s).clone());
        }
        Some(v) => panic!("pure SSA: a store holds {v:?}"),
        None => {
            st.remove(name);
        }
    }
}
