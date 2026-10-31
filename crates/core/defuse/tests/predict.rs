//! A prediction confirmed by a definition of the same value and the same
//! visibility: a global definition by a node standing outside every
//! scope confirms a predicted definition (filed at the node's depth),
//! and its readers are not asked again.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

/// `[f, x]`: predicted to define f := x; its run, slow, defines f := x
/// globally.
struct GlobalDef;

impl Operation for GlobalDef {
    fn name(&self) -> &'static str {
        "global_def"
    }

    fn predicted_defs(&self, args: &[ValueId], _: &Values) -> Option<Vec<(Field, ValueId)>> {
        Some(vec![(Field(args[0]), args[1])])
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        std::thread::sleep(std::time::Duration::from_millis(50));
        cx.define_with(Field(cx.args()[0]), cx.args()[1], Locality::Global);
        Ok(cx.args()[1])
    }
}

/// `[f]`: reads f, outputs it; counts its runs.
struct Reader {
    runs: AtomicU64,
}

impl Operation for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let x = cx.read(Field(cx.args()[0]))?;
        Ok(x.unwrap_or(ValueId::UNIT))
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

#[test]
fn a_global_definition_outside_every_scope_confirms_its_prediction() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(GlobalDef));
    let rd = Arc::new(Reader {
        runs: AtomicU64::new(0),
    });
    let reader = e.register(rd.clone());
    let f = v.field("count:0");
    let u = e.new_unit();
    e.push(u, def, vec![f.0, v.int(3)]);
    let r = e.push(u, reader, vec![f.0]);
    let rep = e.evaluate(4).unwrap();
    assert_eq!(e.output(r).and_then(|y| v.as_int(y)), Some(3));
    assert_eq!(rd.runs.load(Ordering::SeqCst), 1, "the reader ran once: {rep:?}");
    assert_eq!(rep.predictions_refuted, 0, "{rep:?}");
    assert!(rep.predictions_confirmed >= 1, "{rep:?}");
    assert!(e.check_invariants().is_empty());
}
