//! The operations: `Exec` (one node's text: a paragraph region, a
//! loading command, a whole format), `Put` (definitions that land
//! outside a group that just closed), `Dvi` (the file), `Mark` (the
//! end of the prelude) and `Files` (the files read back next round).

use std::sync::{Arc, OnceLock};

use phitex_defuse::{
    Claim, Ctx, Effect, Field, Locality, MayDefine, OpId, Operation, Stop, ValueId, Values,
};

use crate::run::{Halt, Ops, R, Run, decode_frames};
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
    let trace = crate::exec::env_flag!("PHITEX_TEX_TRACE");
    if trace {
        let a = cx
            .args()
            .first()
            .map(|&x| cx.values().render(x))
            .unwrap_or_default();
        eprintln!(
            "run {} {}",
            cx.node().raw() % 100_000,
            a.chars().take(70).collect::<String>()
        );
    }
    let mut r = Run::new(cx, o);
    let res = body(&mut r);
    if trace {
        eprintln!(
            "  -> {}",
            match &res {
                Ok(()) => "ok".to_owned(),
                Err(Halt::Stop(_)) => "stop".to_owned(),
                Err(Halt::Starve) => "starve".to_owned(),
                Err(Halt::Fatal) => "fatal".to_owned(),
            }
        );
    }
    match res {
        Ok(()) => {
            r.end_open();
            Ok(r.finish())
        }
        Err(Halt::Stop(s)) => Err(s),
        Err(Halt::Starve) => {
            let u = ValueId::UNIT;
            Ok(r.stall("starve", u))
        }
        Err(Halt::Fatal) => {
            let u = ValueId::UNIT;
            Ok(r.stall("fatal", u))
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
        let args = cx.args();
        let hint = crate::hints::hint_of_args(cx.values(), &args);
        run_node(cx, ops(&self.0), |r| {
            r.top = args.get(1).is_some_and(|&t| int_of(r.v, t) != 0);
            let fs = decode_frames(r.v, &args);
            let tokens = matches!(fs.first(), Some(crate::run::Frame::Toks(..)));
            crate::hints::count(r, if tokens { 5 } else { 6 });
            r.frames.extend(fs);
            // the hint's assumptions, against what is in force here
            if let Some(m) = crate::hints::verify(r, &hint)? {
                r.log("error", &format!("hint regime violated: {m}"))?;
                crate::hints::count(r, 2);
            }
            // the whole slice runs here: a paragraph region to its end,
            // a format command with its operands, the tokens every
            // expansion leaves (docs/TARGET_GRAPH.md); a paragraph the
            // text did not close, or a group, reads on into the siblings
            // until it is closed (`Run::run_all`)
            r.run_all()?;
            crate::hints::debug_defs(r, &hint);
            if hint.bound.is_some() {
                if let Some(m) = crate::hints::check_defs(r, &hint) {
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
        })
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
pub(crate) struct Dvi;

impl Operation for Dvi {
    fn name(&self) -> &'static str {
        "dvi"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        crate::page::dvi(cx)
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
