//! The operations: `Exec` (one node's text: a paragraph region, a
//! loading command, a whole format), `Put` (definitions that land
//! outside a group that just closed), `Dvi` (the file), `Mark` (the
//! end of the prelude) and `Files` (the files read back next round).

use std::sync::{Arc, OnceLock};

use phitex_defuse::{
    Claim, Ctx, Effect, Field, Locality, MayDefine, OpId, Operation, Stop, ValueId, Values,
};

use crate::run::{Halt, Ops, R, Run};
use crate::val::{int_of, list_of, str_of};

/// The operations' ids, known once all are registered.
pub(crate) type Shared = Arc<OnceLock<Ops>>;

pub(crate) fn ops(s: &Shared) -> Ops {
    s.get().expect("operations registered").clone()
}

/// Runs `body` as one node: its definitions, contributions and
/// children go out only if it finishes; a starved or file-waiting run
/// records its need for the client instead.
pub(crate) fn run_node(
    cx: &mut Ctx<'_>,
    o: Ops,
    body: impl FnOnce(&mut Run<'_, '_>) -> R<()>,
) -> Result<ValueId, Stop> {
    let t0 = std::time::Instant::now();
    let op = cx.op();
    let r = run_node_parking(cx, o, false, body);
    // (a slow node named, for the edit path's phases)
    if crate::exec::env_flag!("PHITEX_TEX_TIME") && t0.elapsed().as_millis() >= 20 {
        eprintln!("slow node: op {op:?} {:?}", t0.elapsed());
    }
    r
}

/// [`run_node`]; with `parks`, a run that stops inside a command parks
/// its state there ([`Run::park`], [`Ctx::park`]) and the next attempt,
/// if the engine resumes it, continues at that command (`Run::resumed`:
/// the body skips what it did before its commands).
pub(crate) fn run_node_parking(
    cx: &mut Ctx<'_>,
    o: Ops,
    parks: bool,
    body: impl FnOnce(&mut Run<'_, '_>) -> R<()>,
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
    let parked = if parks { cx.parked() } else { None };
    let mut r = match parked.map(<Box<dyn std::any::Any + Send>>::downcast::<crate::run::Parked>) {
        Some(Ok(p)) => match Run::from_parked(cx, o.clone(), *p) {
            Ok(r) => r,
            Err(cx) => {
                cx.restart();
                Run::new(cx, o)
            }
        },
        Some(Err(_)) => {
            cx.restart();
            Run::new(cx, o)
        }
        None => Run::new(cx, o),
    };
    let t_body = std::time::Instant::now();
    let res = body(&mut r);
    let t_body = t_body.elapsed();
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
            }
        );
    }
    match res {
        Ok(()) => {
            r.end_open();
            let t = std::time::Instant::now();
            let x = r.finish();
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
            r.claim_rest = true;
            // (for the output: pdfTeX writes no PDF file after one)
            // (`pdf`: pdfTeX says no PDF file was produced, §1335, pdfTeX
            // `close_files_and_terminate`)
            let pdf = r.param("pdfoutput").is_ok_and(|p| p > 0);
            let _ = r.log("fatal", if pdf { "pdf" } else { "" });
            r.end_fatal();
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
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    /// What the text says it may define ([`crate::hints`]): the state's
    /// field heads and the control sequences its command can define, or
    /// anything where the pass cannot bound it.
    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        crate::hints::may_define(v, args)
    }
    /// For an unbounded region: the fields its hint's guess names (its
    /// commands' rows, its calls' bodies); a wrong `false` is refuted
    /// by the engine, never wrong output.
    fn may_define_field(&self, args: &[ValueId], v: &Values, f: Field) -> bool {
        client_only(v, f)
            && crate::hints::hint_of_args(v, args).may_define_field(&crate::val::field_name(v, f))
    }
    /// The meanings and catcodes the text's hint rests on
    /// ([`crate::hints::reads`]): the node waits for them instead of
    /// starting and stopping.
    fn reads(&self, args: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        crate::hints::reads(v, args)
    }
    /// No scope effect where the text says the command opens and closes
    /// nothing ([`crate::hints`]); otherwise known when it runs (the
    /// scope's kind comes from the run).
    fn effect(&self, args: &[ValueId], v: &Values) -> Effect {
        if crate::hints::effect_none(v, args) {
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
        match crate::hints::hint_of_args(v, args)
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
        crate::hints::hint_of_args(v, args)
            .bound
            .as_ref()
            .and_then(|b| b.claim)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args();
        let hint = crate::hints::hint_of_args(cx.values(), &args);
        // (a package load: its run recorded, `crate::pkgrec`)
        let load = crate::pkgrec::is_load(cx.values(), &args);
        let (node, t0, spec) = (
            cx.node().raw(),
            std::time::Instant::now(),
            cx.is_speculative(),
        );
        let o = ops(&self.0);
        let log = o.pkg_log.clone();
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
                let fs = crate::run::decode_segs(r.v, &args);
                let tokens = matches!(fs.first(), Some(crate::input::Seg::Toks(..)));
                crate::hints::count(r, if tokens { 5 } else { 6 });
                r.input.append_text(fs);
                if let Some((c, k)) = crate::run::carried_conds(r.v, &args) {
                    r.conds = c;
                    r.if_codes = k;
                }
                // the hint's assumptions, against what is in force here
                if let Some(m) = crate::hints::verify(r, &hint)? {
                    r.log("error", &format!("hint regime violated: {m}"))?;
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
                .map(|(_, a)| crate::hints::hint_of_args(r.v, &a))
                .collect();
            if hint.bound.is_some() && !r.hint_void && claimed.iter().all(|h| h.bound.is_some()) {
                if let Some(m) = crate::hints::check_defs(r, &hint, &claimed)? {
                    if std::env::var_os("PHITEX_PARLISTS_TRACE").is_some() {
                        eprintln!("parlists: {}", m.chars().take(600).collect::<String>());
                    }
                    r.log("error", &m)?;
                    crate::hints::count(r, 3);
                }
                crate::hints::count(r, 0);
            } else {
                crate::hints::count(r, 1);
                if crate::hints::is_demoted(&hint) {
                    crate::hints::count(r, 4);
                }
            }
            Ok(())
        });
        if load && res.is_ok() {
            log.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((node, t0, std::time::Instant::now(), spec));
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
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let o = ops(&self.0);
        crate::page::dvi(cx, &o)
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

/// The files the job writes, read back by the next round: for each name
/// in the client's `loopfiles`, the text its streams accumulated
/// (`streams`, in the order the page builder's `wplan` gives) is
/// published as the next round's `wfile:<name>`.
pub(crate) struct Files;

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
        let names: Vec<String> = cx
            .use_field(v.field("loopfiles"))?
            .map(|x| list_of(v, x).iter().map(|&n| str_of(v, n)).collect())
            .unwrap_or_default();
        if names.is_empty() {
            return Ok(ValueId::UNIT);
        }
        // (the `wdef:` loop-backs declared so far: a new one runs this
        // again)
        let _ = cx.use_env(v.field("wdefset"))?;
        let plan = cx.acc_range(v.field("wplan"), None)?;
        let all = cx.acc_range(v.field("streams"), None)?;
        let texts = crate::api::written_texts(v, Some(plan), Some(all));
        let defs = crate::api::written_defs(v, Some(plan), Some(all));
        for name in &names {
            let k = v.field(&format!("wkeys:{name}"));
            if !cx.is_loopback(k) {
                continue;
            }
            let ds = defs
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, d)| d.as_slice())
                .unwrap_or_default();
            let mut keys = Vec::new();
            for (f, val) in ds {
                let w = v.field(&format!("wdef:{f}"));
                if cx.is_loopback(w) {
                    cx.publish(w, *val);
                    keys.push(v.str(f));
                }
            }
            cx.publish(k, v.list(keys));
        }
        for name in names {
            let f = v.field(&format!("wfile:{name}"));
            if !cx.is_loopback(f) {
                continue;
            }
            // a file the job did not write this round is absent next round
            let x = texts.iter().find(|(n, _)| *n == name).map_or_else(
                || v.list(Vec::new()),
                |(_, t)| v.list(vec![v.str(&format!("./{name}")), v.str(t)]),
            );
            cx.publish(f, x);
        }
        Ok(ValueId::UNIT)
    }
}
