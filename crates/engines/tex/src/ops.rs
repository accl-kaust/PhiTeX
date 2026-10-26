//! The operations: `Exec` (one node's text: a paragraph region, a
//! loading command, a whole format), `Put` (definitions that land
//! outside a group that just closed), `Dvi` (the file), `Mark` (the
//! end of the prelude) and `Files` (the files read back next round).
//! The pages' (`Fold`, `Finish`, `Resume`) are in `crate::pages`.

use crate::exec::LockWait as _;
use std::sync::{Arc, OnceLock};

use phitex_defuse::{
    Ctx, Effect, Field, FxHashMap, Locality, MayDefine, Operation, Stop, ValueId,
    Values,
};

use crate::run::{Halt, Ops, R, Run};
use crate::val::{int_of, list_of, str_of};

/// The operations' ids, known once all are registered.
pub(crate) type Shared = Arc<OnceLock<Ops>>;

pub(crate) fn ops(s: &Shared) -> &Ops {
    s.get().expect("operations registered")
}

/// The build's hint registry (`crate::hints::Registry`), for the
/// engine's questions about a node.
fn reg(s: &Shared) -> &crate::hints::Registry {
    &s.get().expect("operations registered").hints
}

/// (measure, `PHITEX_RUN_LOG`) every run attempt of the process: the
/// node's raw id, whether it continued an earlier attempt (none does:
/// false), whether it ran
/// speculatively, and how it ended (`o` finished, `s` stopped, `x`
/// other); drained into [`crate::Stats::run_log`] per evaluation.
pub(crate) static RUN_LOG: std::sync::Mutex<Vec<(u64, bool, bool, u8)>> =
    std::sync::Mutex::new(Vec::new());

/// The exec nodes' claims as their texts declare them: none, a count,
/// pending (decided by the run, its followers waiting).
pub(crate) static EXEC_CLAIMS: [std::sync::atomic::AtomicU64; 3] = [
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
];

/// The commands every run of the process did (attempts that stopped
/// counted too): Ir per command is a profile's total over this.
pub(crate) static COMMANDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// [`COMMANDS`] by the run's operation: exec, finish, resume, the rest.
pub(crate) static COMMANDS_BY: [std::sync::atomic::AtomicU64; 4] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// Runs `body` as one node: its definitions, contributions and
/// children go out only if it finishes; a starved or file-waiting run
/// records its need for the client instead. A run that stops starts
/// again from its beginning when the engine runs it again.
#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
pub(crate) fn run_node(
    cx: &mut Ctx<'_>,
    o: &Ops,
    mut body: impl FnMut(&mut Run<'_, '_>) -> R<()>,
) -> Result<ValueId, Stop> {
    let trace = crate::exec::obs_flag!("PHITEX_TEX_TRACE");
    if trace {
        let a = cx
            .args()
            .first()
            .map(|&x| cx.values().render(x))
            .unwrap_or_default();
        eprintln!(
            "run {} {}",
            cx.node().raw(),
            a.chars().take(70).collect::<String>()
        );
    }
    let pp = if cx.op() == o.exec {
        crate::prof::start()
    } else {
        None
    };
    let mut r = Run::new(cx, o);
    let t_body = std::time::Instant::now();
    let mut res = body(&mut r);
    // (a second pass from the start, ending at the candidate the first
    // asked for: deterministic, one at most, `Halt::Recut`)
    if let Err(Halt::Recut(k)) = res {
        let cx = r.cx;
        r = Run::new(cx, o);
        r.stop_at = Some(k);
        // (a second pass to a candidate takes no marks)
        r.marks_on = false;
        res = body(&mut r);
    }
    // the run's end on the page (`crate::pages`): its items folded into
    // the page state, a finisher placed where a page finishes; a page
    // finishing inside the piece is a cut: the run goes again from the
    // top, stopping at that candidate, and places the finisher and its
    // continuation
    //
    // (a restart drops the reads of the pass before: a second pass that
    // reads another page state than the first finds the page finishing
    // elsewhere, or not at all, and the run goes again, stopping where
    // the state it now reads finishes it, or not stopping)
    // (spec-final A1: one pass; a piece's run ends at its first
    // page-builder candidate with input after it, `Halt::Cut`, and its
    // fold places the continuation for the rest)
    if matches!(res, Ok(()) | Err(Halt::Cut)) {
        let tf = pp.and_then(|_| crate::prof::start());
        res = r.page_fold();
        crate::prof::add(crate::prof::FOLD, tf);
    }
    let t_body = t_body.elapsed();
    COMMANDS.fetch_add(r.cmds, std::sync::atomic::Ordering::Relaxed);
    let k = if r.cx.op() == o.exec {
        0
    } else if r.cx.op() == o.finish {
        1
    } else if r.cx.op() == o.resume {
        2
    } else {
        3
    };
    COMMANDS_BY[k].fetch_add(r.cmds, std::sync::atomic::Ordering::Relaxed);
    if crate::exec::prof_flag!("PHITEX_RUN_LOG") {
        // (measure: every attempt, for `Stats::run_log`)
        let how = match &res {
            Ok(()) | Err(Halt::Cut) => b'o',
            Err(Halt::Stop(_)) => b's',
            _ => b'x',
        };
        RUN_LOG
            .locked()
            .push((r.cx.node().raw(), false, r.cx.is_speculative(), how));
    }
    // (a long run, the format's: its commands' time, then its finish)
    let timed = t_body.as_secs() >= 1 && crate::exec::prof_flag!("PHITEX_TEX_TIME");
    if timed {
        eprintln!(
            "run: commands {t_body:?} ({}, speculative {}), memory MB resident/peak {:?}",
            match &res {
                Ok(()) => "ok",
                Err(Halt::Stop(_)) => "stop",
                Err(Halt::Starve) => "starve",
                Err(Halt::Fatal) => "fatal",
                Err(Halt::Cut) => "cut",
                Err(Halt::Recut(_)) => "recut",
            },
            r.cx.is_speculative(),
            crate::api::mem_mb()
        );
    }
    if trace {
        eprintln!(
            "  -> {}",
            match &res {
                Ok(()) => "ok".to_owned(),
                Err(Halt::Stop(_)) => format!("stop (node {})", r.cx.node().raw()),
                Err(Halt::Starve) => "starve".to_owned(),
                Err(Halt::Fatal) => "fatal".to_owned(),
                Err(Halt::Cut) => "cut".to_owned(),
                Err(Halt::Recut(k)) => format!("recut at {k}"),
            }
        );
    }
    match res {
        Ok(()) | Err(Halt::Cut) => {
            // (a run cut at a candidate keeps its open groups and lists:
            // they are its continuation's)
            if r.cut.is_none() {
                r.end_open();
            }
            // (what a continuation republishes and the records the run
            // defines, which may read: a stop here runs the node again
            // from its start)
            if let Err(Halt::Stop(s)) = r
                .drop_unchanged_republished()
                .and_then(|()| r.ready_records())
            {
                return Err(s);
            }
            r.settle_lines();
            // (observe, `PHITEX_MARK_TRACE=1`: a finisher's or exec run's
            // outputs, to compare a run resumed at a mark with a full one)
            if crate::exec::obs_flag!("PHITEX_MARK_TRACE") {
                let v = r.v;
                let node = r.cx.node().raw();
                let mut pubd: Vec<String> = r
                    .st
                    .published(v, r.rec_load)
                    .into_iter()
                    .map(|(l, x, loc)| format!("{}{loc:?}={}", crate::val::field_name(v, r.loc_field(l)), x.raw()))
                    .collect();
                pubd.sort();
                let contribs: Vec<String> = r
                    .contribs
                    .iter()
                    .map(|(f, x)| format!("{}+={}", crate::val::field_name(v, *f), x.raw()))
                    .collect();
                let kids: Vec<String> = r.kids.iter().map(|(o, a)| format!("{o:?}{:?}", a.iter().map(|x| x.raw()).collect::<Vec<_>>())).collect();
                eprintln!(
                    "mark-trace: node {node} effect {:?} pub {pubd:?} contribs {contribs:?} kids {kids:?}",
                    r.effect
                );
            }
            let t = std::time::Instant::now();
            let tf = pp.and_then(|_| crate::prof::start());
            let x = r.finish();
            crate::prof::add(crate::prof::FINISH, tf);
            if pp.is_some() {
                crate::prof::add(crate::prof::RUN, pp);
                crate::prof::run();
            }
            if timed {
                eprintln!(
                    "run: finish {:?}, memory MB resident/peak {:?}",
                    t.elapsed(),
                    crate::api::mem_mb()
                );
            }
            Ok(x)
        }
        Err(Halt::Stop(s)) => Err(s),
        // (a second pass ends at its candidate, before the read that
        // asked for it)
        Err(Halt::Recut(k)) => unreachable!("a second pass passed its candidate {k}"),
        Err(Halt::Starve) => {
            let u = ValueId::UNIT;
            Ok(r.stall("starve", u))
        }
        // (§1335: the job ends there, its log written up to the error,
        // as pdfTeX's close_files_and_terminate leaves it)
        Err(Halt::Fatal) => {
            if !r.fatal_logged {
                let _ = r.log("error", "*** (job aborted, no legal \\end found)");
            }
            // nothing after the error is read: the rest of the input is
            // claimed, and the main list ends here for the page builder
            // (the page it holds is not shipped)
            r.input = crate::input::Input::new();
            r.muts += 1;
            r.claim_rest = true;
            // (for the output: pdfTeX writes no PDF file after one)
            // (`pdf`: pdfTeX says no PDF file was produced, §1335, pdfTeX
            // `close_files_and_terminate`)
            let pdf = r.param("pdfoutput").is_ok_and(|p| p > 0);
            let _ = r.log("fatal", if pdf { "pdf" } else { "" });
            r.end_fatal();
            // (the main list's end, an abort, given to the page builder as
            // a piece's items are: it ends the pages there, and the output
            // is assembled from the pages shipped before)
            if let Err(Halt::Stop(s)) = r.page_fold() {
                return Err(s);
            }
            if let Err(Halt::Stop(s)) = r.ready_records() {
                return Err(s);
            }
            Ok(r.finish())
        }
    }
}

/// One command (§1030) of the node's text: the first token decides; the
/// command reads on into the siblings it claims; what it produced
/// (expansion, tokens put back) and the text it did not read are its
/// children.
pub(crate) struct Exec(pub Shared);

impl Operation for Exec {
    fn name(&self) -> &'static str {
        "exec"
    }

    /// A recorded node (a package load, a document node whose text
    /// cannot bound what it defines) whose record matches (and
    /// predictions on, [`crate::Options::pkg_predict`]): the values its
    /// last run defined, so later nodes run on them, confirmed by value
    /// when it runs.
    fn predicted_defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        // (the text first: most nodes are no recorded node, and ask no lock)
        if !crate::pkgrec::is_recorded(reg(&self.0), v, args) {
            return None;
        }
        let loads = ops(&self.0).pkg_loads.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()?;
        if !loads.predict {
            return None;
        }
        let key = crate::pkgrec::args_key(reg(&self.0), v, args)?;
        let p = loads.prediction_of(v, &key)?;
        Some(p.into_iter().map(|(n, _, x)| (v.field(&n), x)).collect())
    }
    /// What the text says it may define ([`crate::hints`]): the state's
    /// field heads and the control sequences its command can define, or
    /// anything where the pass cannot bound it.
    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        crate::hints::may_define(reg(&self.0), v, args)
    }
    /// No scope effect where the text says the command opens and closes
    /// nothing ([`crate::hints`]); otherwise known when it runs (the
    /// scope's kind comes from the run).
    fn effect(&self, args: &[ValueId], v: &Values) -> Effect {
        if crate::hints::effect_none(reg(&self.0), v, args) {
            Effect::None
        } else {
            Effect::Pending
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let args = cx.args().to_vec();
        // (a package load: its run recorded, publishing its net effect,
        // `crate::pkgrec`; a document node's run is recorded at its end,
        // `crate::pkgrec::is_recorded`)
        let load = crate::pkgrec::is_load(cx.values(), &args);
        let _nt = crate::exec::NodeTimer::new(if load { "load" } else { "exec" }, cx.node().raw());
        let hint = crate::hints::hint_of_args(reg(&self.0), cx.values(), &args);
        let (node, t0, spec) = (
            cx.node().raw(),
            std::time::Instant::now(),
            cx.is_speculative(),
        );
        let o = ops(&self.0);
        let log = o.pkg_log.clone();
        let res = run_node(cx, o, |r| {
            {
                // (a load at the unit's top level only: there a global
                // definition back to the value in force is no effect, and
                // the run publishes its net effect, `State::published`)
                // (`PHITEX_PKG_NONET=1`: no load recorded, for comparison)
                r.rec_load = load && r.scope_kind()?.is_none();
                r.top = args.get(1).is_some_and(|&t| int_of(r.v, t) != 0);
                // (what the run reads of its arguments but its text after
                // its start, its hint's counts among them: in its state)
                r.args_key = {
                    use std::hash::{Hash, Hasher};
                    let mut h = phitex_defuse::FxHasher::default();
                    for a in args.get(1..).unwrap_or_default() {
                        a.raw().hash(&mut h);
                    }
                    h.finish()
                };
                let args = r.resolve_units(&args)?;
                let fs = crate::run::decode_segs(r.v, &args);
                let tokens = matches!(fs.first(), Some(crate::input::Seg::Toks(..)));
                crate::hints::count(r, if tokens { 4 } else { 5 });
                if hint.cause.starts_with("region") {
                    crate::hints::count(r, 6);
                }
                r.input.append_text(fs);
                if let Some((c, k)) = crate::run::carried_conds(r.v, &args) {
                    r.conds = c;
                    r.if_codes = k;
                }
                // the hint's assumptions, against what is in force here
                if let Some(m) = crate::hints::verify(r, &hint)? {
                    r.engine_diag(&format!("hint regime violated: {m}"));
                    crate::hints::count(r, 2);
                    r.hint_void = true;
                }
            }
            // the whole slice runs here: a paragraph region to its end,
            // a format command with its operands, the tokens every
            // expansion leaves (docs/TARGET_GRAPH.md); a paragraph the
            // text did not close, or a group, reads on into the siblings
            // until it is closed (`Run::run_all`)
            r.run_all()?;
            crate::hints::debug_defs(r, &hint);
            // (a hint whose regime the run found violated bounds nothing:
            // its definitions are not checked against it)
            // (a run that claimed siblings ran their texts too: it is
            // bounded by its hint and theirs together, and by nothing
            // when one of theirs is unbounded)
            let claimed: Vec<std::sync::Arc<crate::hints::Hint>> = (0..r.pulled)
                .filter_map(|i| r.cx.following(i))
                .map(|(_, a)| crate::hints::hint_of_args(&r.ops.hints, r.v, &a))
                .collect();
            // (a paragraph taken inside a box of the run's own, a
            // figure's float, is the box's: one node, like a picture)
            for (i, h) in claimed.iter().enumerate() {
                if h.cause.starts_with("region") && !r.pull_box.get(i).copied().unwrap_or(false) {
                    crate::hints::count(r, 7);
                }
            }
            // (what it defined outside its bound is the engine's: a
            // static definition list is a guess it widens, counted as
            // `defs_refuted`)
            if hint.bound.is_some() && !r.hint_void && claimed.iter().all(|h| h.bound.is_some()) {
                crate::hints::count(r, 0);
            } else {
                crate::hints::count(r, 1);
                if crate::hints::is_demoted(&hint) {
                    crate::hints::count(r, 3);
                }
            }
            Ok(())
        });
        if load && res.is_ok() {
            log.locked()
                .push((node, t0, std::time::Instant::now(), spec));
        }
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") && t0.elapsed().as_millis() > 200 {
            // (the node's text, rendered only for a long run)
            let v = cx.values();
            let text = format!(
                "{} segs, load {load}: {}",
                crate::run::decode_segs(v, &args).len(),
                v.render(args.first().copied().unwrap_or(ValueId::UNIT))
                    .chars()
                    .take(300)
                    .collect::<String>()
            );
            crate::exec::later(format!(
                "longrun: exec {node} {:.0} ms (ok {}): {text}",
                t0.elapsed().as_secs_f64() * 1e3,
                res.is_ok()
            ));
        }
        res
    }
}

/// Definitions made by a group's end for outside it: `[[name, value,
/// global]…]`, and the names whose binding level is still below the
/// enclosing group (`lvlfix`, §1079).
pub(crate) struct Put(pub Shared);

impl Operation for Put {
    fn name(&self) -> &'static str {
        "put"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args().to_vec();
        let o = ops(&self.0);
        let v = cx.values();
        // REDUCE-GAP: a static definition set must be defined exactly;
        // `Put` defines through the context directly.
        let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
        let mut carry = Vec::new();
        for n in list_of(v, a(1)) {
            carry.push(n);
        }
        let mut fix = Vec::new();
        if !carry.is_empty() {
            let mut r = Run::new(cx, o);
            let res: R<Vec<ValueId>> = (|| {
                let md = r.mode()?;
                let mut fix = r.scope_list("lvlfix", &md)?;
                for n in &carry {
                    let name = str_of(v, *n);
                    let x = r.get(&name)?;
                    // the definition being made is the binding's value
                    let lvl = list_of(v, a(0))
                        .iter()
                        .find_map(|&e| {
                            let xs = list_of(v, e);
                            (xs.first().map(|&k| str_of(v, k)) == Some(name.clone()))
                                .then(|| xs.get(1).map_or(0, |&y| crate::val::int_at(v, y, 0)))
                        })
                        .unwrap_or_else(|| x.map_or(0, |y| crate::val::int_at(v, y, 0)));
                    if lvl < md.depth {
                        fix.push(*n);
                    }
                }
                let mut xs = vec![v.int(md.sid)];
                xs.extend(fix);
                Ok(xs)
            })();
            match res {
                Ok(xs) => fix = xs,
                Err(Halt::Stop(s)) => return Err(s),
                Err(_) => {}
            }
        }
        let mut names = Vec::new();
        for e in list_of(v, a(0)) {
            let xs = list_of(v, e);
            let name = xs.first().map(|&n| str_of(v, n)).unwrap_or_default();
            let g = xs.get(2).is_some_and(|&g| int_of(v, g) != 0);
            let val = xs.get(1).copied().unwrap_or(ValueId::UNIT);
            let f = v.field(&name);
            names.push(f.0);
            crate::exec::widths::def(f);
            cx.define_with(f, val, if g { Locality::Global } else { Locality::Local });
        }
        // the fields defined, for the prelude's export (as `Run::finish`)
        if !names.is_empty() {
            let s = v.seq_unit(v.list(names));
            cx.contribute(v.field("defs"), s);
        }
        if !carry.is_empty() {
            cx.define_with(v.field("lvlfix"), v.list(fix), Locality::Local);
        }
        Ok(ValueId::UNIT)
    }
}

/// The DVI file.
pub(crate) struct Dvi(pub Shared);

impl Operation for Dvi {
    fn name(&self) -> &'static str {
        "dvi"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let o = ops(&self.0);
        let file = crate::assemble::output(cx, o)?;
        let v = cx.values();
        cx.define_with(v.field(crate::assemble::END_FILE), file, Locality::Global);
        Ok(ValueId::UNIT)
    }
}

/// The link (spec-redesign §1): the exact PDF file from the pages'
/// records, before the end node, which takes the file from it.
pub(crate) struct Link(pub Shared);

impl Operation for Link {
    fn name(&self) -> &'static str {
        "link"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let o = ops(&self.0);
        crate::assemble::link(cx, o)
    }
}

/// The end of the prelude: the point its final bindings are read at.
pub(crate) struct Mark;

impl Operation for Mark {
    fn name(&self) -> &'static str {
        "mark"
    }
    fn run(&self, _: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// Whether a node may define `f`: files (`file:`) come only from the
/// client.
pub(crate) fn client_only(v: &Values, f: Field) -> bool {
    v.with_str(f.0, |s| !s.starts_with("file:")).unwrap_or(true)
}

/// The files the job writes, read back where the job reads them: one
/// node per written file, its name its argument, under a root node (no
/// argument) that places them: for each name in the client's `loopfiles`
/// and the streams opened (`wnames`). Their definitions are global and
/// stand at the line's end, and the readers read each one's last
/// definition (`At::Final`, [`lb_read`]): read early and written late, a
/// file is a fixed point of the evaluation, as latexmk's runs make it,
/// each reader run again as the definition it read moves. A file's node
/// reads its own stream only ([`crate::run::stream_bucket`]), so a
/// change in one file's lines runs that file's node alone. A file read
/// back per field is its key set (`wkeys:<name>`) and one field per key
/// (`wdef:<id>`): a reader of a label reads that label's field. Another
/// is sliced by the document's slicer into units, each its own field,
/// `wunit:<name>:<hash>:<n>` (its text's hash, `n` its occurrence among
/// equal units), and the file is their order, `wfile:<name>`:
/// `[sliceable, [[unit field, starts], ...]]` (none: the file absent). A
/// reader of the file is a parent whose children are the units' pieces,
/// keyed by content ([`crate::run::Run::doc_file_regions`]): an edit of
/// one unit re-runs that unit's piece, and the order's readers only
/// where the order changed. The file's whole text goes to its tools
/// (`wtext:<name>`, [`crate::tools`]) and its size (`wsize:<name>`) to
/// a lookup's. A file whose text the client gave and that the job
/// writes as given (`wsame:<name>`, [`WSAME`]) is read back from that
/// text where it is read ([`crate::run::Run::seed_written`]).
pub(crate) struct Files(pub Shared);

impl Operation for Files {
    fn name(&self) -> &'static str {
        "files"
    }
    /// The written files are read back: a pass reads them whole as the
    /// pass before left them.
    fn read_back(&self) -> bool {
        true
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let v = cx.values();
        let Some(listed) = cx
            .read(v.field("loopfiles"))?
            .filter(|&x| x != ValueId::UNIT)
        else {
            return Ok(ValueId::UNIT);
        };
        let listed: Vec<String> = list_of(v, listed).iter().map(|&n| str_of(v, n)).collect();
        // (the written text is complete once every page's finisher is
        // placed: where the pages end, defined by the page builder)
        cx.read(v.field(crate::pages::PAGES_END))?;
        match cx.args().first() {
            None => files_root(cx, listed),
            Some(&n) => {
                let name = str_of(v, n);
                let shared = ops(&self.0);
                file_node(cx, &name, &shared.defs)?;
                // (what a tool makes of it by default: run here, on the
                // text as the job writes it, so a reader of the output
                // depends on what the tool makes, not on its input)
                for (o, args) in crate::tools::made_from(cx, &name)? {
                    let x = cx.apply(shared.tool, args)?;
                    define_traced(cx, crate::tools::made_field(v, &o), x);
                }
                Ok(ValueId::UNIT)
            }
        }
    }
}

/// Loop-back field `f`'s last definition on the line, wherever the
/// reader is (`At::Final`), else the environment's: a file the job
/// writes, read back as the job writes it.
pub(crate) fn lb_read(cx: &mut Ctx<'_>, f: Field) -> Result<Option<ValueId>, Stop> {
    Ok(match cx.read_at(f, phitex_defuse::At::Final)? {
        phitex_defuse::FieldAt::Def(x, _) => Some(x),
        _ => None,
    })
}

/// The files the job wrote (the files' root defines it, its first value
/// the files the client gave the texts of): each keeps its node, and
/// what it holds, in an evaluation that does not write it
/// ([`file_node`]).
pub(crate) const WSEEN: &str = "wseen";

/// `wsame:<name>`: whether file `name`, whose text the client gave, is
/// written as given (true in the environment, its node finding
/// otherwise): while it is, its readers read that text
/// ([`crate::run::Run::lb_known`]).
pub(crate) const WSAME: &str = "wsame";

/// The root of the files' nodes: one child per file the job writes
/// (the client's `loopfiles`, the files written before, `wseen`, the
/// streams opened), in that order.
fn files_root(cx: &mut Ctx<'_>, listed: Vec<String>) -> Result<ValueId, Stop> {
    let v = cx.values();
    let mut names = listed;
    let seen = v.field(WSEEN);
    let opened: Vec<String> = v
        .seq_to_vec(cx.fold(v.field(crate::run::WNAMES), None, None)?)
        .into_iter()
        .map(|n| str_of(v, n))
        .collect();
    let before = lb_read(cx, seen)?;
    // (a job that writes no file, now or before: no files' nodes)
    if names.is_empty() && opened.is_empty() && before.is_none() {
        return Ok(ValueId::UNIT);
    }
    let mut written: Vec<String> = before
        .map(|x| list_of(v, x).iter().map(|&n| str_of(v, n)).collect())
        .unwrap_or_default();
    for n in opened {
        if !written.contains(&n) {
            written.push(n);
        }
    }
    for n in &written {
        if !names.contains(n) {
            names.push(n.clone());
        }
    }
    define_traced(cx, seen, v.list(written.iter().map(|n| v.str(n)).collect()));
    let op = cx.op();
    for n in names {
        cx.child(op, vec![v.str(&n)]);
    }
    Ok(ValueId::UNIT)
}

/// A written file's text, its read-back's definitions and whether it is
/// read back as its text (a line of it did more than define,
/// [`crate::api::written_as_text`]), from its stream's accumulator alone
/// ([`crate::run::stream_bucket`]).
fn stream_of(
    cx: &mut Ctx<'_>,
    name: &str,
) -> Result<(Option<String>, Option<WrittenDefs>, bool), Stop> {
    let v = cx.values();
    let acc = cx.fold(crate::run::stream_bucket(v, name), None, None)?;
    if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
        for e in v.seq_to_vec(acc) {
            eprintln!(
                "aux-trace: stream entry for {name} at node {}: {}",
                cx.node().raw(),
                v.render(e).chars().take(120).collect::<String>()
            );
        }
    }
    let text = crate::api::written_texts(v, Some(acc))
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, t)| t);
    let defs = crate::api::written_defs(v, Some(acc))
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, d)| d);
    let as_text = crate::api::written_as_text(v, Some(acc))
        .iter()
        .any(|n| n == name);
    Ok((text, defs, as_text))
}

/// A written file's read-back definitions: `(field, value)` each.
type WrittenDefs = Vec<(String, ValueId)>;

/// One written file's node ([`Files`]): its text and size (`wtext:`,
/// `wsize:`), its read-back's keys and definitions (`wkeys:`, `wdef:`)
/// or its units and order (`wfile:`, `wunit:`), and for a file whose
/// text the client gave, whether it is written as given (`wsame:`).
fn file_node(cx: &mut Ctx<'_>, name: &str, defs_lock: &std::sync::Mutex<crate::hints::Defs>) -> Result<ValueId, Stop> {
    let v = cx.values();
    let k = v.field(&format!("wkeys:{name}"));
    let wt = crate::tools::text_field(v, name);
    let t0 = std::time::Instant::now();
    let (text, ds, as_text) = stream_of(cx, name)?;
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "files-node: {name}: the stream read in {:?}, {} bytes, as text {as_text}",
            t0.elapsed(),
            text.as_ref().map_or(0, String::len)
        );
    }
    // (a file not opened for writing now stays as it was left: TeX's next
    // run finds the file an earlier run wrote, which a job that writes a
    // file only where it is missing reads instead of writing it again)
    if text.is_none() && ds.is_none() {
        return keep_file(cx, name);
    }
    if let Some(seed) = seed_text(cx, name)? {
        let same = text.as_deref().is_some_and(|t| v.with_str(seed, |s| s == t).unwrap_or(false));
        define_traced(cx, v.field(&format!("{WSAME}:{name}")), v.bool(same));
    }
    let tv = text.as_deref().map_or(ValueId::UNIT, |t| v.str(t));
    define_traced(cx, wt, tv);
    define_traced(cx, crate::tools::size_field(v, name), crate::tools::size_value(v, tv));
    // (read back per field while every line written to it only defines;
    // as its text once one does more, `wkeys:` then marked so)
    if as_text {
        define_traced(cx, k, crate::api::text_mark(v));
    } else {
        let ds = ds.as_deref().unwrap_or_default();
        // (the file exists, whatever its lines define; a nested file's
        // key names that file)
        let keys = crate::api::key_set(v, name, ds);
        let trace = crate::exec::obs_flag!("PHITEX_AUX_TRACE");
        for (f, val) in ds {
            // (a key whose value another file's lines define)
            if *val == ValueId::UNIT {
                continue;
            }
            let id = crate::api::wdef_id(name, f);
            define_traced(cx, v.field(&format!("wdef:{id}")), *val);
            if trace {
                let xs = list_of(v, *val);
                let shown = xs
                    .first()
                    .map(|&x| crate::val::show_def(v, x))
                    .unwrap_or_default();
                eprintln!("aux-trace: define {name} {f} = {shown} (id {:?})", val.raw());
            }
        }
        if trace {
            eprintln!(
                "aux-trace: define {name}: {} keys, text {} bytes:\n{}",
                list_of(v, keys).len(),
                text.as_deref().map_or(0, str::len),
                text.as_deref().unwrap_or("")
            );
        }
        define_traced(cx, k, keys);
        return Ok(ValueId::UNIT);
    }
    let f = v.field(&format!("wfile:{name}"));
    // a file the job did not write is absent
    let Some(t) = text else {
        define_traced(cx, f, v.list(Vec::new()));
        return Ok(ValueId::UNIT);
    };
    if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
        eprintln!("aux-trace: define wfile {name}: {} bytes:\n{t}", t.len());
    }
    let defs = defs_lock.locked().clone();
    let t0 = std::time::Instant::now();
    let order = written_units(cx, name, &t, &defs);
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
        eprintln!("files-node: {name}: its units in {:?}", t0.elapsed());
    }
    define_traced(cx, f, order);
    Ok(ValueId::UNIT)
}

/// The text the client gave for file `name`, one the job writes, as the
/// build began (`crate::run::SEEDS`).
fn seed_text(cx: &mut Ctx<'_>, name: &str) -> Result<Option<ValueId>, Stop> {
    let v = cx.values();
    let listed = cx
        .read(v.field(crate::run::SEEDS))?
        .is_some_and(|ns| list_of(v, ns).iter().any(|&n| str_of(v, n) == name));
    if !listed {
        return Ok(None);
    }
    cx.read(v.field(&format!("{}:{name}", crate::run::SEEDS)))
}

/// A written file not opened now ([`file_node`]): what it held defined
/// again, its text, size, key set and each key's definitions, its units
/// and order, as the readers read them; a file whose text the client
/// gave, never written, that text (`wsame:` stays true).
fn keep_file(cx: &mut Ctx<'_>, name: &str) -> Result<ValueId, Stop> {
    let v = cx.values();
    let k = v.field(&format!("wkeys:{name}"));
    let mut fields = vec![
        crate::tools::text_field(v, name),
        k,
        v.field(&format!("wfile:{name}")),
        crate::tools::size_field(v, name),
        v.field(&format!("{WSAME}:{name}")),
    ];
    if let Some(keys) = lb_read(cx, k)?
        && keys != crate::api::text_mark(v)
    {
        for key in list_of(v, keys) {
            let key = str_of(v, key);
            if key == crate::api::WRITTEN_KEY {
                continue;
            }
            let id = crate::api::wdef_id(name, &key);
            fields.push(v.field(&format!("wdef:{id}")));
        }
    }
    // (and its units', `wunit:`, as its order names them)
    let wf = fields[2];
    if let Some(order) = lb_read(cx, wf)?
        && let Some(&es) = list_of(v, order).get(1)
    {
        for e in list_of(v, es) {
            if let Some(&u) = list_of(v, e).first() {
                fields.push(v.field(&str_of(v, u)));
            }
        }
    }
    for f in fields {
        if let Some(x) = lb_read(cx, f)? {
            define_traced(cx, f, x);
        }
    }
    Ok(ValueId::UNIT)
}

/// A written file's text as units ([`Files`]): each unit defined as its
/// field, and the order value.
fn written_units(cx: &mut Ctx<'_>, name: &str, text: &str, defs: &crate::hints::Defs) -> ValueId {
    let v = cx.values();
    let (order, units) = units_of(v, name, text, defs);
    for (key, u) in units {
        define_traced(cx, v.field(&key), v.str(u));
    }
    order
}

/// A written file's text as units: the order value (`wfile:<name>`) and
/// each unit's field (`wunit:<name>:<hash>:<n>`) with its text.
pub(crate) fn units_of<'t>(
    v: &Values,
    name: &str,
    text: &'t str,
    defs: &crate::hints::Defs,
) -> (ValueId, Vec<(String, &'t str)>) {
    let (ends, sliceable) = unit_ends(name, text, defs);
    let mut seen: FxHashMap<u64, u32> = FxHashMap::default();
    let mut entries = Vec::with_capacity(ends.len());
    let mut units = Vec::with_capacity(ends.len());
    let mut at = 0;
    for &e in ends.iter() {
        let Some(u) = text.get(at..e) else { continue };
        let starts = at == 0 || text.as_bytes()[at - 1] == b'\n';
        at = e;
        if u.is_empty() {
            continue;
        }
        let h = phitex_defuse::hash_bytes(0x7775_6e69_7400_0001, u.as_bytes());
        let n = seen.entry(h).or_insert(0);
        // (built, not formatted: a key per unit of every file written)
        let mut key = String::with_capacity(name.len() + 32);
        key.push_str("wunit:");
        key.push_str(name);
        key.push(':');
        for i in (0..16).rev() {
            let d = u32::try_from((h >> (4 * i)) & 0xf).unwrap_or(0);
            key.push(char::from_digit(d, 16).unwrap_or('0'));
        }
        key.push(':');
        crate::print::print_int(&mut key, i64::from(*n));
        *n += 1;
        entries.push(v.list(vec![v.str(&key), v.int(i64::from(starts))]));
        units.push((key, u));
    }
    (
        v.list(vec![v.int(i64::from(sliceable)), v.list(entries)]),
        units,
    )
}

/// Where a written file's units end ([`units_of`]: its slices' ends as
/// the document's slicer cuts it, under the format's definitions
/// `defs`) and whether it slices as a document's text: a function of
/// the text and the definitions, kept by both (a file a round writes
/// as the round before wrote it is not analysed again: amsldoc's index
/// file, 40 kB, 125 ms).
fn unit_ends(name: &str, text: &str, defs: &crate::hints::Defs) -> (Arc<[usize]>, bool) {
    type Kept = (
        std::sync::Weak<phitex_source::pass::Definitions>,
        Arc<[usize]>,
        bool,
    );
    type Last = std::sync::Mutex<FxHashMap<String, (Arc<str>, Kept)>>;
    static MEMO: std::sync::OnceLock<std::sync::Mutex<FxHashMap<(u64, usize), Kept>>> =
        std::sync::OnceLock::new();
    // (the file's text the last time it was cut, with its cuts: the part
    // of the text an edit left keeps its units, the rest is cut again,
    // [`units_again`])
    static LAST: std::sync::OnceLock<Last> = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(|| std::sync::Mutex::new(FxHashMap::default()));
    let key = (
        phitex_defuse::hash_bytes(0x756e_6974_656e_6473, text.as_bytes()),
        Arc::as_ptr(defs) as usize,
    );
    if let Some((d, ends, sl)) = memo.locked().get(&key)
        && d.upgrade().is_some_and(|d| Arc::ptr_eq(&d, defs))
    {
        return (ends.clone(), *sl);
    }
    let last = LAST.get_or_init(|| std::sync::Mutex::new(FxHashMap::default()));
    let before = last.locked().get(name).cloned().filter(|(_, (d, _, _))| {
        d.upgrade().is_some_and(|d| Arc::ptr_eq(&d, defs))
    });
    let cut = |t: &str| -> Vec<usize> {
        let h = crate::hints::text_hints(t, phitex_source::pass::FileKind::Document, defs);
        h.slices.iter().map(|&(_, e)| e).collect()
    };
    let mut ends: Vec<usize> = match &before {
        Some((old, (_, old_ends, _))) => units_again(old, old_ends, text, cut),
        None => cut(text),
    };
    // (the units cover the text: each from the end of the one before to
    // its slice's end, the last to the text's end)
    ends.retain(|&e| e > 0 && e <= text.len());
    ends.sort_unstable();
    ends.dedup();
    if ends.last() != Some(&text.len()) {
        ends.push(text.len());
    }
    // (`PHITEX_UNITS_CHECK=1`, observe: the cuts made again against the
    // whole text's)
    if before.is_some() && crate::exec::obs_flag!("PHITEX_UNITS_CHECK") {
        let mut whole = cut(text);
        whole.retain(|&e| e > 0 && e <= text.len());
        whole.sort_unstable();
        whole.dedup();
        if whole.last() != Some(&text.len()) {
            whole.push(text.len());
        }
        eprintln!("units-check: {name}: {} units, the same as the whole text's: {}", ends.len(), whole == ends);
    }
    // (the job's own text: what kind of file it is comes from its text)
    let sliceable = crate::inputs::document_text(text);
    let ends: Arc<[usize]> = ends.into();
    let mut m = memo.locked();
    if m.len() > 64 {
        m.clear();
    }
    m.insert(key, (Arc::downgrade(defs), ends.clone(), sliceable));
    drop(m);
    last.locked().insert(
        name.to_owned(),
        (Arc::from(text), (Arc::downgrade(defs), ends.clone(), sliceable)),
    );
    (ends, sliceable)
}

/// The units of `text`, a written file's text that was `old` when it was
/// cut at `old_ends`: the units wholly in the part both texts begin with
/// kept, those wholly in the part both end with kept (moved), each but
/// the one next to the change (a unit's end can depend on the text after
/// it, `\section` taking its arguments), and the text between cut again
/// by `cut`, at a unit's start, as a text of its own. The cuts only
/// decide how the file is read back in pieces, never what is read.
fn units_again(old: &str, old_ends: &[usize], text: &str, cut: impl Fn(&str) -> Vec<usize>) -> Vec<usize> {
    let (o, n) = (old.as_bytes(), text.as_bytes());
    let pre = o.iter().zip(n).take_while(|(a, b)| a == b).count();
    let max_suf = o.len().min(n.len()) - pre;
    let suf = o.iter().rev().zip(n.iter().rev()).take(max_suf).take_while(|(a, b)| a == b).count();
    if pre == o.len() && pre == n.len() {
        return old_ends.to_vec();
    }
    // (a cut at a line's start, so that the text cut again begins as a
    // file's line does)
    let at_line = |b: &[u8], e: usize| e == 0 || b.get(e - 1) == Some(&b'\n');
    // the kept prefix: the units ending at or before the change, but the
    // last of them
    let left: Vec<usize> = old_ends.iter().copied().filter(|&e| e <= pre).collect();
    let l_keep = left.len().saturating_sub(1);
    let mut l_at = 0;
    let mut kept = Vec::new();
    for &e in &left[..l_keep] {
        if at_line(o, e) {
            l_at = e;
        }
    }
    kept.extend(left[..l_keep].iter().copied().filter(|&e| e <= l_at));
    // the kept suffix: the units starting at or after the change's end
    // in the old text, but the first of them; moved to the new text
    let o_end = o.len() - suf;
    let shift = |e: usize| e + n.len() - o.len();
    let starts: Vec<usize> = std::iter::once(0).chain(old_ends.iter().copied()).collect();
    let right: Vec<usize> = starts.iter().copied().filter(|&s| s >= o_end && s < o.len()).collect();
    let r_from = right.iter().copied().skip(1).find(|&s| at_line(o, s));
    let r_at = r_from.map_or(n.len(), shift);
    if r_at < l_at {
        return cut(text);
    }
    let mid = cut(&text[l_at..r_at]);
    let mut ends = kept;
    ends.extend(mid.into_iter().map(|e| e + l_at));
    ends.push(r_at);
    if let Some(r) = r_from {
        ends.extend(old_ends.iter().copied().filter(|&e| e > r).map(shift));
    }
    ends
}

/// Defines a file's read-back field, globally at the files' node; under
/// `observe` with `PHITEX_ROUND_TRACE`, each definition is printed with
/// its value's hash and the node (`round-trace: <field> <hash> node <n>
/// (field id <id>)`): the fields that moved between the node's runs are
/// those printed with a new hash; the id is the one the engine's rerun
/// causes name.
fn define_traced(cx: &mut Ctx<'_>, f: Field, x: ValueId) {
    if crate::exec::obs_flag!("PHITEX_ROUND_TRACE") {
        let v = cx.values();
        eprintln!(
            "round-trace: {} {:016x} node {} (field id {})",
            crate::val::field_name(v, f),
            x.raw(),
            cx.node().raw(),
            f.0.raw()
        );
    }
    cx.define_with(f, x, Locality::Global);
}
