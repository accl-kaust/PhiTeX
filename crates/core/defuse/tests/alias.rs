//! Alias definitions (`Ctx::define_alias`): a node that copies a field it
//! never looks at into another defines the copy as "that field's value
//! before me"; a reader of the copy reads the source there itself, so a
//! change of the source asks the reader again and never the copier.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

/// `[f, x]`: defines f := x.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[g, f]`: defines g := the value of f reaching me, without reading f.
struct Copier {
    runs: AtomicU64,
}

impl Operation for Copier {
    fn name(&self) -> &'static str {
        "copier"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let (g, f) = (Field(cx.args()[0]), Field(cx.args()[1]));
        let me = cx.node();
        cx.define_alias(g, f, me, Locality::Global);
        Ok(ValueId::UNIT)
    }
}

/// `[g]`: reads g, outputs it; counts its runs.
struct Reader {
    runs: AtomicU64,
}

impl Operation for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

#[test]
fn a_change_of_the_source_asks_the_reader_of_the_copy_and_never_the_copier() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let cp = Arc::new(Copier {
        runs: AtomicU64::new(0),
    });
    let copier = e.register(cp.clone());
    let rd = Arc::new(Reader {
        runs: AtomicU64::new(0),
    });
    let reader = e.register(rd.clone());
    let (f, g) = (v.field("count:0"), v.field("count:9"));
    let u = e.new_unit();
    let d = e.push(u, def, vec![f.0, v.int(3)]);
    e.push(u, copier, vec![g.0, f.0]);
    // (a definition of the source after the copier reaches no copy)
    e.push(u, def, vec![f.0, v.int(50)]);
    let r = e.push(u, reader, vec![g.0]);
    let rep = e.evaluate(4).unwrap();
    assert_eq!(e.output(r).and_then(|y| v.as_int(y)), Some(3), "{rep:?}");
    let (c0, r0) = (cp.runs.load(Ordering::SeqCst), rd.runs.load(Ordering::SeqCst));
    e.replace(d, def, vec![f.0, v.int(4)]);
    let rep = e.evaluate(4).unwrap();
    assert_eq!(e.output(r).and_then(|y| v.as_int(y)), Some(4), "{rep:?}");
    assert_eq!(
        cp.runs.load(Ordering::SeqCst),
        c0,
        "the copier is not asked again: {rep:?}"
    );
    assert_eq!(
        rd.runs.load(Ordering::SeqCst),
        r0 + 1,
        "the reader is asked once: {rep:?}"
    );
    assert!(e.check_invariants().is_empty());
}

/// An alias of an alias is followed to the first source.
#[test]
fn an_alias_of_an_alias_is_followed() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let copier = e.register(Arc::new(Copier {
        runs: AtomicU64::new(0),
    }));
    let reader = e.register(Arc::new(Reader {
        runs: AtomicU64::new(0),
    }));
    let (f, g, h) = (v.field("count:0"), v.field("count:9"), v.field("count:8"));
    let u = e.new_unit();
    e.push(u, def, vec![f.0, v.int(7)]);
    e.push(u, copier, vec![g.0, f.0]);
    e.push(u, def, vec![f.0, v.int(8)]);
    e.push(u, copier, vec![h.0, g.0]);
    let r = e.push(u, reader, vec![h.0]);
    let rep = e.evaluate(4).unwrap();
    assert_eq!(e.output(r).and_then(|y| v.as_int(y)), Some(7), "{rep:?}");
    assert!(e.check_invariants().is_empty());
}
