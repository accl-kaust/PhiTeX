//! The operations: `Exec` (one node's text: a paragraph region, a
//! loading command, a whole format), `Put` (definitions that land
//! outside a group that just closed), `Dvi` (the file), `Mark` (the
//! end of the prelude) and `Files` (the files read back next round).
//! The pages' (`Fold`, `Finish`, `Resume`) are in `crate::pages`.

use std::sync::{Arc, OnceLock};

use phitex_defuse::{
    Claim, Ctx, Effect, Field, FxHashMap, Locality, MayDefine, OpId, Operation, Stop, ValueId,
    Values,
};

use crate::run::{Halt, Ops, R, Run};
use crate::val::{int_of, list_of, str_of};

/// The operations' ids, known once all are registered.
pub(crate) type Shared = Arc<OnceLock<Ops>>;

pub(crate) fn ops(s: &Shared) -> Ops {
    s.get().expect("operations registered").clone()
}

/// The build's hint registry (`crate::hints::Registry`), for the
/// engine's questions about a node.
fn reg(s: &Shared) -> &crate::hints::Registry {
    &s.get().expect("operations registered").hints
}

thread_local! {
    /// (measure) the text of the exec node this thread runs, for the
    /// `longrun:` line.
    static LONG_TEXT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// (measure, `PHITEX_RUN_LOG`) every run attempt of the process: the
/// node's raw id, whether it resumed a parked run, whether it ran
/// speculatively, and how it ended (`o` finished, `s` stopped, `x`
/// other); drained into [`crate::Stats::run_log`] per evaluation.
pub(crate) static RUN_LOG: std::sync::Mutex<Vec<(u64, bool, bool, u8)>> =
    std::sync::Mutex::new(Vec::new());

/// The commands every run of the process did (attempts that stopped
/// counted too): Ir per command is a profile's total over this.
pub(crate) static COMMANDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Runs `body` as one node: its definitions, contributions and
/// children go out only if it finishes; a starved or file-waiting run
/// records its need for the client instead. With `parks`, a run that stops inside a command parks
/// its state there ([`Run::park`], [`Ctx::park`]) and the next attempt,
/// if the engine resumes it, continues at that command (`Run::resumed`:
/// the body skips what it did before its commands).
#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
pub(crate) fn run_node_parking(
    cx: &mut Ctx<'_>,
    o: Ops,
    parks: bool,
    mut body: impl FnMut(&mut Run<'_, '_>) -> R<()>,
) -> Result<ValueId, Stop> {
    let trace = crate::exec::env_flag!("PHITEX_TEX_TRACE");
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
    let parks = parks && !crate::exec::env_flag!("PHITEX_NO_PARK");
    let pp = if cx.op() == o.exec {
        crate::prof::start()
    } else {
        None
    };
    let parked = if parks { cx.parked() } else { None };
    let mut r = match parked.map(<Box<dyn std::any::Any + Send>>::downcast::<crate::run::Parked>) {
        Some(Ok(p)) => match Run::from_parked(cx, o.clone(), *p) {
            Ok(r) => r,
            Err(cx) => {
                cx.restart();
                Run::new(cx, o.clone())
            }
        },
        Some(Err(_)) => {
            cx.restart();
            Run::new(cx, o.clone())
        }
        None => Run::new(cx, o.clone()),
    };
    let t_body = std::time::Instant::now();
    let mut res = body(&mut r);
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
    if crate::exec::env_flag!("PHITEX_RUN_LOG") {
        // (measure: every attempt, for `Stats::run_log`)
        let how = match &res {
            Ok(()) | Err(Halt::Cut) => b'o',
            Err(Halt::Stop(_)) => b's',
            _ => b'x',
        };
        RUN_LOG
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((r.cx.node().raw(), r.resumed, r.cx.is_speculative(), how));
    }
    // (a long run, the format's: its commands' time, then its finish)
    let timed = t_body.as_secs() >= 1 && crate::exec::env_flag!("PHITEX_TEX_TIME");
    if timed {
        eprintln!(
            "run: commands {t_body:?} ({}, speculative {}, resumed {}), memory MB resident/peak {:?}",
            match &res {
                Ok(()) => "ok",
                Err(Halt::Stop(_)) => "stop",
                Err(Halt::Starve) => "starve",
                Err(Halt::Fatal) => "fatal",
                Err(Halt::Cut) => "cut",
            },
            r.cx.is_speculative(),
            r.resumed,
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
        Err(Halt::Stop(s)) => {
            if parks
                && !r.cx.is_speculative()
                && let Some(p) = r.park()
            {
                r.cx.park(Box::new(p));
            }
            Err(s)
        }
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

    /// A package load whose record matches (and predictions on,
    /// [`crate::Options::pkg_predict`]): the values its last run defined,
    /// so later loads run on them, confirmed by value when it runs.
    fn predicted_defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        if !crate::pkgrec::is_load(v, args) {
            return None;
        }
        let loads = ops(&self.0)
            .pkg_loads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        let &fr = args.first()?;
        let &f = list_of(v, fr).first()?;
        let text = str_of(v, *list_of(v, f).get(1)?);
        let p = loads.prediction(v, &text)?;
        Some(p.into_iter().map(|(n, _, x)| (v.field(&n), x)).collect())
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    /// What the text says it may define ([`crate::hints`]): the state's
    /// field heads and the control sequences its command can define, or
    /// anything where the pass cannot bound it.
    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        crate::hints::may_define(reg(&self.0), v, args)
    }
    /// For an unbounded region: the fields its hint's guess names (its
    /// commands' rows, its calls' bodies); a wrong `false` is refuted
    /// by the engine, never wrong output.
    fn may_define_field(&self, args: &[ValueId], v: &Values, f: Field) -> bool {
        if !client_only(v, f) {
            return false;
        }
        let name = crate::val::field_name(v, f);
        // (the page state a piece with items on the main list defines,
        // and a `\shipout`'s mark: `crate::pages`)
        crate::pages::is_page_field(&name)
            || crate::hints::hint_of_args(reg(&self.0), v, args).may_define_field(&name)
    }
    /// The meanings and catcodes the text's hint rests on
    /// ([`crate::hints::reads`]): the node waits for them instead of
    /// starting and stopping.
    fn reads(&self, args: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        crate::hints::reads(reg(&self.0), v, args)
    }
    /// The lines of its own text the node begins (`lines`, by file:
    /// [`crate::run::Run::doc_lines`]): readers of `\inputlineno` fold
    /// them without waiting for its run. (A node that claims a sibling
    /// begins the sibling's lines too: its run refutes this, and the
    /// readers after it run again.)
    fn static_contribs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        Some(vec![(v.field("lines"), crate::run::text_lines(v, args))])
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
    /// The siblings the slice takes when its text says so exactly
    /// (a balanced region whose items all end inside it: none).
    fn claim(
        &self,
        args: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        if crate::exec::env_flag!("PHITEX_NO_CLAIMS") {
            return Claim::Pending;
        }
        match crate::hints::hint_of_args(reg(&self.0), v, args)
            .bound
            .as_ref()
            .and_then(|b| b.claim)
        {
            Some(0) => Claim::None,
            Some(n) => Claim::Count(n),
            None => Claim::Pending,
        }
    }
    fn claim_limit(
        &self,
        args: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        if crate::exec::env_flag!("PHITEX_NO_CLAIMS") {
            return None;
        }
        crate::hints::hint_of_args(reg(&self.0), v, args)
            .bound
            .as_ref()
            .and_then(|b| b.claim)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let args = cx.args();
        // (a package load: its run recorded, `crate::pkgrec`)
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
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
            let v = cx.values();
            let fs = crate::run::decode_segs(v, &args);
            LONG_TEXT.with(|c| {
                *c.borrow_mut() = format!(
                    "{} segs, load {load}: {}",
                    fs.len(),
                    v.render(args.first().copied().unwrap_or(ValueId::UNIT))
                        .chars()
                        .take(300)
                        .collect::<String>()
                );
            });
        }
        let res = run_node_parking(cx, o, true, |r| {
            // (a resumed run has done this, and continues its commands)
            if !r.resumed {
                // (a load at the unit's top level only: there a global
                // definition back to the value in force is no effect, and
                // the run publishes its net effect, `State::published`)
                // (`PHITEX_PKG_NONET=1`: no load recorded, for comparison)
                r.rec_load = load
                    && !crate::exec::env_flag!("PHITEX_PKG_NONET")
                    && r.scope_kind()?.is_none();
                r.top = args.get(1).is_some_and(|&t| int_of(r.v, t) != 0);
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
            log.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((node, t0, std::time::Instant::now(), spec));
        }
        if crate::exec::env_flag!("PHITEX_TEX_TIME") && t0.elapsed().as_millis() > 200 {
            let text = LONG_TEXT.with(|c| c.borrow().clone());
            eprintln!(
                "longrun: exec {node} {:.0} ms (ok {}): {text}",
                t0.elapsed().as_secs_f64() * 1e3,
                res.is_ok()
            );
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
    fn defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let mut out: Vec<(Field, Locality)> = Vec::new();
        if let Some(&d) = args.first() {
            for e in list_of(v, d) {
                let xs = list_of(v, e);
                let name = xs.first().map(|&n| str_of(v, n)).unwrap_or_default();
                let g = xs.get(2).is_some_and(|&g| int_of(v, g) != 0);
                out.push((
                    v.field(&name),
                    if g { Locality::Global } else { Locality::Local },
                ));
            }
        }
        if args.get(1).is_some_and(|&c| !list_of(v, c).is_empty()) {
            out.push((v.field("lvlfix"), Locality::Local));
        }
        Some(out)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args();
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
            let mut r = Run::new(cx, o.clone());
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
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    /// The scan's last state and the accumulators the file is made of:
    /// the node waits while a page node or writer step before it is in
    /// work instead of starting, stopping at a range and running again
    /// (an edit's end node ran twice: once stopped at `pages` while the
    /// edited page's finisher ran).
    fn reads(&self, _: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(
            [crate::pages::SCAN, "pages", crate::assemble::LINKED]
                .iter()
                .map(|&f| v.field(f))
                .collect(),
        )
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let o = ops(&self.0);
        crate::assemble::output(cx, &o)
    }
}

/// The link (spec-redesign §1): the exact PDF file from the pages'
/// records, before the end node, which takes the file from it.
pub(crate) struct Link(pub Shared);

impl Operation for Link {
    fn name(&self) -> &'static str {
        "link"
    }
    fn defs(&self, _: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(vec![(v.field(crate::assemble::LINKED), Locality::Global)])
    }
    /// What the file is made of: held until the pages and the line's
    /// records before it are done.
    fn reads(&self, _: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(
            [crate::pages::SCAN, "pages", crate::pdfout::XPAGES]
                .iter()
                .map(|&f| v.field(f))
                .collect(),
        )
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let o = ops(&self.0);
        crate::assemble::link(cx, &o)
    }
}

/// The end of the prelude: the point its final bindings are read at.
pub(crate) struct Mark;

impl Operation for Mark {
    fn name(&self) -> &'static str {
        "mark"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, _: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// Whether a node may define `f`: files (`file:`) come only from the
/// client.
pub(crate) fn client_only(v: &Values, f: Field) -> bool {
    match &*v.get(f.0) {
        phitex_defuse::Value::Str(s) => !s.starts_with("file:"),
        _ => true,
    }
}

/// The files the job writes, read back by the next round
/// (spec-redesign §1, `files(round)`): one node per written file, its
/// name its argument, under a root node (no argument) that places them:
/// for each name in the client's `loopfiles`, the streams opened
/// (`wnames`) and the files a tool makes (`tools`). A file's node reads
/// its own stream only ([`crate::run::stream_bucket`]),
/// so a change in one file's lines runs that file's node alone; its text
/// is sliced by the document's slicer into units, each published as its
/// own loop-back, `wunit:<name>:<hash>:<n>` (its text's hash, `n` its
/// occurrence among equal units), and the file as their order,
/// `wfile:<name>`: `[sliceable, [[unit field, starts], ...]]` (none: the
/// file absent). A reader of the file is a parent whose children are the
/// units' pieces, keyed by content ([`crate::run::Run::doc_file_regions`]):
/// an edit of one unit re-runs that unit's piece, and the order's readers
/// only where the order changed. No value holds the file's whole text.
pub(crate) struct Files(pub Shared);

impl Operation for Files {
    fn name(&self) -> &'static str {
        "files"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let v = cx.values();
        let Some(listed) = cx
            .use_field(v.field("loopfiles"))?
            .filter(|&x| x != ValueId::UNIT)
        else {
            return Ok(ValueId::UNIT);
        };
        let listed: Vec<String> = list_of(v, listed).iter().map(|&n| str_of(v, n)).collect();
        match cx.args().first() {
            None => files_root(cx, listed),
            Some(&n) => {
                let name = str_of(v, n);
                let shared = ops(&self.0);
                file_node(cx, &name, listed.contains(&name), &shared.defs)
            }
        }
    }
}

/// The root of the files' nodes: one child per file the job writes
/// (the client's `loopfiles`, the streams opened this round, the files
/// a tool makes), in that order.
fn files_root(cx: &mut Ctx<'_>, listed: Vec<String>) -> Result<ValueId, Stop> {
    let v = cx.values();
    let mut names = listed;
    let opened = cx.acc_range(v.field(crate::run::WNAMES), None)?;
    let tools = cx.acc_range(v.field("tools"), None)?;
    let made = v
        .seq_to_vec(tools)
        .into_iter()
        .filter_map(|e| list_of(v, e).first().map(|&n| str_of(v, n)));
    for n in v
        .seq_to_vec(opened)
        .into_iter()
        .map(|n| str_of(v, n))
        .chain(made)
    {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let op = cx.op();
    for n in names {
        cx.child(op, vec![v.str(&n)]);
    }
    Ok(ValueId::UNIT)
}

/// The text the job wrote to file `name` this round, from its stream's
/// accumulator alone ([`crate::run::stream_bucket`]), and the
/// definitions its read-back makes.
fn stream_of(cx: &mut Ctx<'_>, name: &str) -> Result<(Option<String>, Option<WrittenDefs>), Stop> {
    let v = cx.values();
    let acc = cx.acc_range(crate::run::stream_bucket(v, name), None)?;
    let text = crate::api::written_texts(v, None, Some(acc))
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, t)| t);
    let defs = crate::api::written_defs(v, None, Some(acc))
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, d)| d);
    Ok((text, defs))
}

/// A written file's read-back definitions: `(field, value)` each.
type WrittenDefs = Vec<(String, ValueId)>;

/// One written file's node ([`Files`]): its read-back's keys and
/// definitions (`wkeys:`, `wdef:`) and its units and order (`wfile:`).
#[allow(clippy::too_many_lines)]
fn file_node(
    cx: &mut Ctx<'_>,
    name: &str,
    listed: bool,
    defs_lock: &std::sync::Mutex<crate::hints::Defs>,
) -> Result<ValueId, Stop> {
    let v = cx.values();
    let (written, ds) = stream_of(cx, name)?;
    // the in-process makeindex run `\write18` asked for that makes this
    // file (`Run::shell_command`), on the files this round wrote
    let made = if written.is_none() {
        makeindex_for(cx, name)?
    } else {
        None
    };
    let text = written.or(made);
    if !listed && text.is_none() && ds.is_none() {
        return Ok(ValueId::UNIT);
    }
    let k = v.field(&format!("wkeys:{name}"));
    let keys_on = if cx.is_loopback(k) {
        true
    } else if crate::api::defines_only(name) {
        // (a file whose read-back only defines, opened this round:
        // declared here too, the opening run's declaration being in
        // effect only once the evaluation is quiet)
        cx.declare_loopback(k, v.list(Vec::new()));
        true
    } else {
        false
    };
    if keys_on {
        let ds = ds.as_deref().unwrap_or_default();
        let mut keys = Vec::new();
        let trace = crate::exec::env_flag!("PHITEX_AUX_TRACE");
        for (f, val) in ds {
            let id = crate::api::wdef_id(name, f);
            let w = v.field(&format!("wdef:{id}"));
            // (a key new this round: its loop-back declared here, its
            // readers bound to it run again on it within the
            // evaluation, `Run::read_back`)
            if !cx.is_loopback(w) {
                cx.declare_loopback(w, v.list(Vec::new()));
            }
            // (a nested file's key names that file)
            keys.push(v.str(if *val == ValueId::UNIT { &id } else { f }));
            // (a key whose value another file's lines publish)
            if *val == ValueId::UNIT {
                continue;
            }
            cx.publish(w, *val);
            if trace {
                let xs = list_of(v, *val);
                let shown = xs
                    .first()
                    .map(|&x| crate::val::show_def(v, x))
                    .unwrap_or_default();
                eprintln!(
                    "aux-trace: publish {name} {f} = {shown} (id {:?})",
                    val.raw()
                );
            }
        }
        if trace {
            eprintln!(
                "aux-trace: publish {name}: {} keys, text {} bytes:\n{}",
                keys.len(),
                text.as_deref().map_or(0, str::len),
                text.as_deref().unwrap_or("")
            );
        }
        cx.publish(k, v.list(keys));
    }
    let f = v.field(&format!("wfile:{name}"));
    if !cx.is_loopback(f) {
        if crate::api::defines_only(name) {
            return Ok(ValueId::UNIT);
        }
        cx.declare_loopback(f, v.list(Vec::new()));
    }
    // a file the job did not write this round is absent next round
    let Some(t) = text else {
        cx.publish(f, v.list(Vec::new()));
        return Ok(ValueId::UNIT);
    };
    if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
        eprintln!("aux-trace: publish wfile {name}: {} bytes:\n{t}", t.len());
    }
    let defs = defs_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let order = written_units(cx, name, &t, &defs);
    cx.publish(f, order);
    Ok(ValueId::UNIT)
}

/// A written file's text as units ([`Files`]): each unit published as
/// its loop-back (declared from here, the first round that has it), and
/// the order value.
fn written_units(cx: &mut Ctx<'_>, name: &str, text: &str, defs: &crate::hints::Defs) -> ValueId {
    let v = cx.values();
    let t = crate::hints::text_hints(text, phitex_source::pass::FileKind::Document, defs);
    // (the units cover the text: each from the end of the one before to
    // its slice's end, the last to the text's end)
    let mut ends: Vec<usize> = t.slices.iter().map(|&(_, e)| e).collect();
    ends.retain(|&e| e > 0 && e <= text.len());
    ends.sort_unstable();
    ends.dedup();
    if ends.last() != Some(&text.len()) {
        ends.push(text.len());
    }
    let mut seen: FxHashMap<u64, u32> = FxHashMap::default();
    let mut entries = Vec::with_capacity(ends.len());
    let mut at = 0;
    for e in ends {
        let Some(u) = text.get(at..e) else { continue };
        let starts = at == 0 || text.as_bytes()[at - 1] == b'\n';
        at = e;
        if u.is_empty() {
            continue;
        }
        let h = phitex_defuse::hash_bytes(0x7775_6e69_7400_0001, u.as_bytes());
        let n = seen.entry(h).or_insert(0);
        let key = format!("wunit:{name}:{h:016x}:{n}");
        *n += 1;
        let f = v.field(&key);
        if !cx.is_loopback(f) {
            cx.declare_loopback(f, v.str(""));
        }
        cx.publish(f, v.str(u));
        entries.push(v.list(vec![v.str(&key), v.int(i64::from(starts))]));
    }
    let sliceable = crate::inputs::document_file(name, text);
    v.list(vec![v.int(i64::from(sliceable)), v.list(entries)])
}

/// The file `ind` the makeindex runs of this round make (`tools`:
/// `[ind, [args…]]`, the last run making it), each in process on the
/// files the job wrote (TeX's characters), read from their streams as
/// makeindex asks for them, and the styles found as the job finds files;
/// none where no run makes it (the file stays absent).
fn makeindex_for(cx: &mut Ctx<'_>, want: &str) -> Result<Option<String>, Stop> {
    let v = cx.values();
    let runs = cx.acc_range(v.field("tools"), None)?;
    let mut out = None;
    for e in v.seq_to_vec(runs) {
        let xs = list_of(v, e);
        let (Some(&ind), Some(&args)) = (xs.first(), xs.get(1)) else {
            continue;
        };
        if str_of(v, ind) != want {
            continue;
        }
        let args: Vec<String> = list_of(v, args).iter().map(|&a| str_of(v, a)).collect();
        // (a style named with -s: read first, as the job reads a file)
        let mut styles = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "-s"
                && let Some(s) = it.next()
            {
                let name = if s.contains('.') {
                    s.clone()
                } else {
                    format!("{s}.ist")
                };
                let f = v.field(&format!("file:{name}"));
                let x = cx.use_env(f)?;
                let text = x
                    .and_then(|x| list_of(v, x).get(1).copied())
                    .map(|t| str_of(v, t));
                styles.push((s.clone(), name, text));
            }
        }
        let mi_args: Vec<Vec<u8>> = args.iter().map(|a| latin1_bytes(a)).collect();
        // (the written files makeindex asks for, each read from its
        // stream when asked: run again until it asks for no new one)
        let mut texts: Vec<(String, String)> = Vec::new();
        let mut asked: Vec<String> = Vec::new();
        let made = loop {
            let mut files = MiFiles {
                texts: &texts,
                styles: &styles,
                missing: Vec::new(),
            };
            let o = phitex_makeindex::run(
                &mi_args,
                b"version 2.18 [TeX Live 2026] (kpathsea + Thai support)",
                &mut files,
            );
            let new: Vec<String> = files
                .missing
                .into_iter()
                .filter(|n| !asked.contains(n))
                .collect();
            if new.is_empty() {
                break o.ind;
            }
            for n in new {
                if let (Some(t), _) = stream_of(cx, &n)? {
                    texts.push((n.clone(), t));
                }
                asked.push(n);
            }
        };
        if let Some(f) = made {
            out = Some(f.contents.iter().map(|&b| char::from(b)).collect());
        }
    }
    Ok(out)
}

/// The `.ind` each written `.idx` makes under `makeindex NAME.idx` (no
/// style): what a read-back of the index reads, for
/// [`crate::Build::read_back_texts`].
pub(crate) fn makeindex_plain(texts: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (name, _) in texts {
        let Some(base) = name.strip_suffix(".idx") else {
            continue;
        };
        let mut files = MiFiles {
            texts,
            styles: &[],
            missing: Vec::new(),
        };
        let o = phitex_makeindex::run(
            &[latin1_bytes(name)],
            b"version 2.18 [TeX Live 2026] (kpathsea + Thai support)",
            &mut files,
        );
        if let Some(f) = o.ind {
            out.push((
                format!("{base}.ind"),
                f.contents.iter().map(|&b| char::from(b)).collect(),
            ));
        }
    }
    out
}

/// TeX's characters as bytes (Latin-1; a character past 255 as `?`).
fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect()
}

/// makeindex's files: the job's written files, and the styles read.
struct MiFiles<'a> {
    texts: &'a [(String, String)],
    styles: &'a [(String, String, Option<String>)],
    /// The names asked for and not in `texts`.
    missing: Vec<String>,
}

impl MiFiles<'_> {
    fn text(&mut self, name: &[u8]) -> Option<&str> {
        let n: String = name.iter().map(|&b| char::from(b)).collect();
        let n = n.strip_prefix("./").unwrap_or(&n).to_owned();
        let texts = self.texts;
        let hit = texts.iter().find(|(m, _)| *m == n);
        if hit.is_none() && !self.missing.contains(&n) {
            self.missing.push(n);
        }
        hit.map(|(_, t)| t.as_str())
    }
}

impl phitex_makeindex::Files for MiFiles<'_> {
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.text(name).map(latin1_bytes)
    }
    fn readable(&mut self, name: &[u8]) -> bool {
        self.text(name).is_some()
    }
    fn style(&mut self, name: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let n: String = name.iter().map(|&b| char::from(b)).collect();
        self.styles
            .iter()
            .find(|(s, f, _)| *s == n || *f == n)
            .and_then(|(_, f, t)| {
                t.as_ref()
                    .map(|t| (latin1_bytes(&format!("./{f}")), Some(latin1_bytes(t))))
            })
    }
    fn out_name_ok(&mut self, _name: &[u8]) -> Result<(), Vec<u8>> {
        Ok(())
    }
    fn stdin(&mut self) -> Vec<u8> {
        Vec::new()
    }
}
