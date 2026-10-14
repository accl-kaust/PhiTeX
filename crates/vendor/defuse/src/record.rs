#![cfg(feature = "observe")]
//! Recording for replay (`observe` builds, `DEFUSE_RECORD=path`).
//!
//! Every public engine call, every answer an operation's static hooks
//! gave, every accumulator combine, projection and provider answer, and
//! every run (each `Ctx` call in order with its answer, and the result)
//! is written as one JSON line. A value is written once, before the first
//! line that names it, so the file replays on its own: the `play` example
//! rebuilds the engine with playback operations that answer from it, with
//! nothing of the client in it.
//!
//! Lines carry `e`, the engine's value-table instance, since a process may
//! record more than one engine.

#![allow(clippy::needless_pass_by_value, clippy::explicit_iter_loop)]

use std::cell::RefCell;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use serde_json::{Value as J, json};

use crate::api::{Claim, Effect, Locality, MayDefine, Monoid, Operation, Stop, Until};
use crate::values::{Field, FxHashSet, Value, ValueId, Values};
use crate::{Ctx, OpId};

thread_local! {
    /// Value ids serialized on this thread since the last line.
    static NAMED: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// Called by `ValueId`'s `Serialize`.
pub(crate) fn named(raw: u64) {
    NAMED.with(|n| n.borrow_mut().push(raw));
}

pub(crate) struct Recorder {
    out: Mutex<(BufWriter<File>, FxHashSet<u64>, FxHashSet<u64>)>,
}

static REC: OnceLock<Option<Recorder>> = OnceLock::new();

/// The recorder, if `DEFUSE_RECORD` names a file.
pub(crate) fn rec() -> Option<&'static Recorder> {
    REC.get_or_init(|| {
        let path = std::env::var_os("DEFUSE_RECORD")?;
        let f = File::create(path).ok()?;
        Some(Recorder {
            out: Mutex::new((BufWriter::new(f), FxHashSet::default(), FxHashSet::default())),
        })
    })
    .as_ref()
}

/// Serializes `x`, noting the value ids in it.
pub(crate) fn j<T: Serialize + ?Sized>(x: &T) -> J {
    serde_json::to_value(x).unwrap_or(J::Null)
}

fn hash_of(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl Recorder {
    /// Writes `line` (tagged with the engine), after the definitions of
    /// the values named since the last line. `once`: skip a line written
    /// before.
    pub(crate) fn line(&self, vals: &Values, mut line: J, once: bool) {
        let names: Vec<u64> = NAMED.with(|n| std::mem::take(&mut *n.borrow_mut()));
        if let J::Object(m) = &mut line {
            m.insert("e".into(), json!(vals.instance()));
        }
        let text = line.to_string();
        let mut g = crate::core::lock(&self.out);
        let (w, seen, lines) = &mut *g;
        if once && !lines.insert(hash_of(&text)) {
            return;
        }
        for raw in names {
            Self::def(vals, w, seen, ValueId::from_raw(raw));
        }
        let _ = writeln!(w, "{text}");
        let _ = w.flush();
    }

    fn def(vals: &Values, w: &mut BufWriter<File>, seen: &mut FxHashSet<u64>, v: ValueId) {
        if !seen.insert(v.raw()) {
            return;
        }
        let val = vals.get(v);
        let (t, x) = match &*val {
            Value::Unit => ("unit", J::Null),
            Value::Bool(b) => ("bool", json!(b)),
            Value::Int(i) => ("int", json!(i)),
            Value::Str(s) => ("str", json!(&**s)),
            Value::List(xs) => {
                for &x in xs.iter() {
                    Self::def(vals, w, seen, x);
                }
                ("list", json!(xs.iter().map(|x| x.raw()).collect::<Vec<_>>()))
            }
            Value::Nil => ("nil", J::Null),
            Value::Seq(_) => {
                let xs = vals.seq_to_vec(v);
                for &x in &xs {
                    Self::def(vals, w, seen, x);
                }
                ("seq", json!(xs.iter().map(|x| x.raw()).collect::<Vec<_>>()))
            }
        };
        let _ = writeln!(
            w,
            "{}",
            json!({"k": "val", "e": vals.instance(), "id": v.raw(), "t": t, "x": x})
        );
    }
}

/// Writes an engine call.
pub(crate) fn api(vals: &Values, f: &str, a: J, r: J) {
    if let Some(rc) = rec() {
        rc.line(vals, json!({"k": "api", "f": f, "a": a, "r": r}), false);
    }
}

/// The calls of the run in progress on this thread.
pub(crate) type Calls = Vec<J>;

/// Notes a `Ctx` call (no-op unless the run is being recorded).
pub(crate) fn call(calls: &RefCell<Option<Calls>>, m: &str, a: J, r: J) {
    if let Some(c) = &mut *calls.borrow_mut() {
        c.push(json!([m, a, r]));
    }
}

pub(crate) fn stop_or<T: Serialize>(r: &Result<T, Stop>) -> J {
    match r {
        Ok(x) => json!({"ok": j(x)}),
        Err(_) => json!("stop"),
    }
}

/// An operation that records what it answers.
pub(crate) struct RecOp {
    pub inner: Arc<dyn Operation>,
    pub id: OpId,
}

/// Collects the items a hook took from its `following` iterator.
struct Taking<'a> {
    it: &'a mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    took: Vec<(OpId, Vec<ValueId>)>,
}

impl Iterator for Taking<'_> {
    type Item = (OpId, Vec<ValueId>);
    fn next(&mut self) -> Option<Self::Item> {
        let x = self.it.next()?;
        self.took.push(x.clone());
        Some(x)
    }
}

impl RecOp {
    fn st(&self, vals: &Values, m: &str, args: &[ValueId], x: J, r: J) {
        if let Some(rc) = rec() {
            rc.line(
                vals,
                json!({"k": "st", "op": self.id, "m": m, "a": j(args), "x": x, "r": r}),
                true,
            );
        }
    }
}

impl Operation for RecOp {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn defs(&self, args: &[ValueId], values: &Values) -> Option<Vec<(Field, Locality)>> {
        let r = self.inner.defs(args, values);
        self.st(values, "defs", args, J::Null, j(&r));
        r
    }
    fn may_define(&self, args: &[ValueId], values: &Values) -> MayDefine {
        let r = self.inner.may_define(args, values);
        self.st(values, "may_define", args, J::Null, j(&r));
        r
    }
    fn may_define_field(&self, args: &[ValueId], values: &Values, f: Field) -> bool {
        let r = self.inner.may_define_field(args, values, f);
        self.st(values, "may_define_field", args, j(&f), j(&r));
        r
    }
    fn reads(&self, args: &[ValueId], values: &Values) -> Option<Vec<Field>> {
        let r = self.inner.reads(args, values);
        self.st(values, "reads", args, J::Null, j(&r));
        r
    }
    fn static_contribs(&self, args: &[ValueId], values: &Values) -> Option<Vec<(Field, ValueId)>> {
        let r = self.inner.static_contribs(args, values);
        self.st(values, "static_contribs", args, J::Null, j(&r));
        r
    }
    fn predicted_defs(&self, args: &[ValueId], values: &Values) -> Option<Vec<(Field, ValueId)>> {
        let r = self.inner.predicted_defs(args, values);
        self.st(values, "predicted_defs", args, J::Null, j(&r));
        r
    }
    fn effect(&self, args: &[ValueId], values: &Values) -> Effect {
        let r = self.inner.effect(args, values);
        self.st(values, "effect", args, J::Null, j(&r));
        r
    }
    fn claim(
        &self,
        args: &[ValueId],
        values: &Values,
        following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        let mut t = Taking {
            it: following,
            took: Vec::new(),
        };
        let r = self.inner.claim(args, values, &mut t);
        self.st(values, "claim", args, j(&t.took), j(&r));
        r
    }
    fn claim_limit(
        &self,
        args: &[ValueId],
        values: &Values,
        following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        let mut t = Taking {
            it: following,
            took: Vec::new(),
        };
        let r = self.inner.claim_limit(args, values, &mut t);
        self.st(values, "claim_limit", args, j(&t.took), j(&r));
        r
    }
    fn until(&self, args: &[ValueId], values: &Values, cand: OpId, cand_args: &[ValueId]) -> Until {
        let r = self.inner.until(args, values, cand, cand_args);
        self.st(values, "until", args, json!([cand, j(cand_args)]), j(&r));
        r
    }
    fn until_value(
        &self,
        args: &[ValueId],
        values: &Values,
        cand: OpId,
        cand_args: &[ValueId],
        value: Option<ValueId>,
    ) -> bool {
        let r = self.inner.until_value(args, values, cand, cand_args, value);
        self.st(
            values,
            "until_value",
            args,
            json!([cand, j(cand_args), j(&value)]),
            j(&r),
        );
        r
    }
    fn until_values(
        &self,
        args: &[ValueId],
        values: &Values,
        cand: OpId,
        cand_args: &[ValueId],
        fields: &[Option<ValueId>],
    ) -> bool {
        let r = self.inner.until_values(args, values, cand, cand_args, fields);
        self.st(
            values,
            "until_values",
            args,
            json!([cand, j(cand_args), j(fields)]),
            j(&r),
        );
        r
    }
    fn until_predict(
        &self,
        args: &[ValueId],
        values: &Values,
        following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        let mut t = Taking {
            it: following,
            took: Vec::new(),
        };
        let r = self.inner.until_predict(args, values, &mut t);
        self.st(values, "until_predict", args, j(&t.took), j(&r));
        r
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let Some(rc) = rec() else {
            return self.inner.run(cx);
        };
        let outer = cx.calls.replace(Some(Vec::new()));
        let r = self.inner.run(cx);
        let calls = cx.calls.replace(outer).unwrap_or_default();
        let vals = cx.values();
        let args = cx.args();
        rc.line(
            vals,
            json!({
                "k": "run", "node": cx.node(), "op": self.id, "a": j(&args),
                "spec": cx.is_speculative(), "calls": calls, "r": stop_or(&r),
            }),
            false,
        );
        r
    }
}

/// An accumulator that records its combines.
pub(crate) struct RecMonoid {
    pub inner: Arc<dyn Monoid>,
    pub field: Field,
}

impl Monoid for RecMonoid {
    fn empty(&self, values: &Values) -> ValueId {
        let r = self.inner.empty(values);
        if let Some(rc) = rec() {
            rc.line(
                values,
                json!({"k": "mono", "f": j(&self.field), "m": "empty", "r": j(&r)}),
                true,
            );
        }
        r
    }
    fn combine(&self, values: &Values, a: ValueId, b: ValueId) -> ValueId {
        let r = self.inner.combine(values, a, b);
        if let Some(rc) = rec() {
            rc.line(
                values,
                json!({"k": "mono", "f": j(&self.field), "m": "combine", "a": [j(&a), j(&b)], "r": j(&r)}),
                true,
            );
        }
        r
    }
}

/// A projection that records its answers.
pub(crate) fn rec_projection(id: u16, p: crate::api::Projection) -> crate::api::Projection {
    Arc::new(move |v: ValueId, values: &Values| {
        let r = p(v, values);
        if let Some(rc) = rec() {
            rc.line(values, json!({"k": "proj", "p": id, "a": j(&v), "r": j(&r)}), true);
        }
        r
    })
}

/// An environment provider that records its answers.
pub(crate) fn rec_provider(p: Box<crate::api::EnvProvider>) -> Box<crate::api::EnvProvider> {
    Box::new(move |values: &Values, f: Field| {
        let r = p(values, f);
        if let Some(rc) = rec() {
            rc.line(values, json!({"k": "env", "f": j(&f), "r": j(&r)}), false);
        }
        r
    })
}

/// An edit source that marks where its edits were absorbed.
pub(crate) struct RecSource<'a> {
    pub inner: &'a mut dyn crate::api::EditSource,
}

impl crate::api::EditSource for RecSource<'_> {
    fn absorb(&mut self, eng: &mut crate::Engine) -> Option<u64> {
        if let Some(rc) = rec() {
            rc.line(eng.values(), json!({"k": "absorb", "phase": "begin"}), false);
        }
        let r = self.inner.absorb(eng);
        if let Some(rc) = rec() {
            rc.line(eng.values(), json!({"k": "absorb", "phase": "end", "r": r}), false);
        }
        r
    }
    fn settled(&mut self, eng: &crate::Engine, items: &[crate::api::Settled]) -> bool {
        self.inner.settled(eng, items)
    }
}
