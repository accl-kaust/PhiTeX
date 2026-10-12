//! The operations: `Exec` (one command of TeX's input), `Put`
//! (definitions that land outside a group that just closed), `Fin`
//! (the page builder, [`crate::page`]), `OutPage` (an output routine
//! begins) and `Dvi` (the file).

use std::sync::{Arc, OnceLock};

use phitex_defuse::{
    Claim, Ctx, Effect, Field, Locality, MayDefine, OpId, Operation, Stop, ValueId, Values,
};

use crate::run::{Halt, Ops, R, Run, decode_frames};
use crate::val::{int_of, list_of, str_of};

/// The operations' ids, known once all are registered.
pub(crate) type Shared = Arc<OnceLock<Ops>>;

fn ops(s: &Shared) -> Ops {
    *s.get().expect("operations registered")
}

/// Runs `body` as one node: its definitions, contributions and
/// children go out only if it finishes; a starved or file-waiting run
/// records its need for the client instead.
fn run_node(cx: &mut Ctx<'_>, o: Ops, body: impl FnOnce(&mut Run<'_, '_>) -> R<()>) -> Result<ValueId, Stop> {
    let mut r = Run::new(cx, o);
    match body(&mut r) {
        Ok(()) => Ok(r.finish()),
        Err(Halt::Stop(s)) => Err(s),
        Err(Halt::Starve) => {
            let u = ValueId::UNIT;
            Ok(r.stall("starve", u))
        }
        Err(Halt::Fatal) => {
            let u = ValueId::UNIT;
            Ok(r.stall("fatal", u))
        }
        Err(Halt::File(name)) => {
            let n = r.v.str(&name);
            Ok(r.stall("file", n))
        }
    }
}

/// One command (§1030): the first token of the node's input decides;
/// what is left of the input is the node's last child.
pub(crate) struct Exec(pub Shared);

impl Operation for Exec {
    fn name(&self) -> &'static str {
        "exec"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        MayDefine::Any
    }
    fn may_define_field(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        client_only(v, f)
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Pending
    }
    fn claim(&self, _: &[ValueId], _: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        Claim::Pending
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let args = cx.args();
        if std::env::var_os("PHITEX_TEX_TRACE").is_some() {
            eprintln!("exec run {} {}", cx.node().raw(), args.first().map(|&a| cx.values().render(a)).unwrap_or_default().chars().take(60).collect::<String>());
        }
        run_node(cx, ops(&self.0), |r| {
            let raw = r.node_raw();
            r.hint = r.get(&format!("hint:{raw}"))?.map_or(0, |h| crate::val::int_at(r.v, h, 0));
            if args.is_empty() {
                return Ok(());
            }
            // after `\end` (or a stall) nothing runs
            if r.int("ended")? != 0 {
                return Ok(());
            }
            if r.hint < 0 {
                r.eof = true;
            }
            let fs = decode_frames(r.v, &args);
            r.frames.extend(fs);
            r.command()
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
                out.push((v.field(&name), if g { Locality::Global } else { Locality::Local }));
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
        for e in list_of(v, a(0)) {
            let xs = list_of(v, e);
            let name = xs.first().map(|&n| str_of(v, n)).unwrap_or_default();
            let g = xs.get(2).is_some_and(|&g| int_of(v, g) != 0);
            let val = xs.get(1).copied().unwrap_or(ValueId::UNIT);
            cx.define_with(v.field(&name), val, if g { Locality::Global } else { Locality::Local });
        }
        if !carry.is_empty() {
            cx.define_with(v.field("lvlfix"), v.list(fix), Locality::Local);
        }
        Ok(ValueId::UNIT)
    }
}

/// The page builder (a root at the end of the document, and again
/// after each `\output`).
pub(crate) struct Fin(pub Shared);

impl Operation for Fin {
    fn name(&self) -> &'static str {
        "fin"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define_field(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        client_only(v, f)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let args = cx.args();
        let tr = std::env::var_os("PHITEX_TEX_TRACE").is_some();
        if tr {
            eprintln!("fin run {}", cx.node().raw());
        }
        let r = run_node(cx, ops(&self.0), |r| {
            let raw = r.node_raw();
            let _ = r.get(&format!("hint:{raw}"))?;
            crate::page::fin(r, &args)
        });
        if tr {
            eprintln!("fin result {:?}", r.as_ref().map(|_| ()).map_err(|_| "stop"));
        }
        r
    }
}

/// An output routine begins (§1025).
pub(crate) struct OutPage(pub Shared);

impl Operation for OutPage {
    fn name(&self) -> &'static str {
        "outpage"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define_field(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        client_only(v, f)
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Pending
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let args = cx.args();
        run_node(cx, ops(&self.0), |r| crate::page::out_page(r, &args))
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

/// The end of the prelude: where its final bindings are read.
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

/// Whether a node may define `f`: the fields only the client sets
/// (`hint:`, `file:`) are never defined by a node.
fn client_only(v: &Values, f: Field) -> bool {
    match &*v.get(f.0) {
        phitex_defuse::Value::Str(s) => !(s.starts_with("hint:") || s.starts_with("file:")),
        _ => true,
    }
}
