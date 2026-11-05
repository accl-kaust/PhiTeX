//! A derived read (`Step::Derived`, `Ctx::read_derived`): the value of the
//! field named from another field's integer value. A move of that value
//! whose new name holds what the old one held leaves the reader standing;
//! a later change of the new name's field asks it again (the witness filed
//! where the re-ask stood); a change of what the name holds asks it again.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, Operation, PathId, Stop, ValueId};

/// `[name, x]`: defines field `name` = x.
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

/// `[j]`: the field `p@<n + j>`, `n` the value of field "n"; counts its runs.
struct Derived(AtomicU64);

impl Operation for Derived {
    fn name(&self) -> &'static str {
        "derived"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let v = cx.values();
        let j = v.as_int(cx.args()[0]).unwrap_or(0);
        let (x, _) = cx.read_derived(v.field("n"), "p@", j, "", PathId::WHOLE)?;
        Ok(x.unwrap_or(ValueId::UNIT))
    }
}

struct Doc {
    e: Engine,
    def: phitex_defuse::OpId,
    reader: Arc<Derived>,
    n: phitex_defuse::NodeId,
    d5: phitex_defuse::NodeId,
    d6: phitex_defuse::NodeId,
    r: phitex_defuse::NodeId,
}

/// n = 5, p@5 = 10, p@6 = 20, then the reader of `p@<n - 1 + 1>`.
fn doc(threads: usize) -> Doc {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let reader = Arc::new(Derived(AtomicU64::new(0)));
    let rd = e.register(reader.clone());
    let u = e.new_unit();
    let n = e.push(u, def, vec![v.str("n"), v.int(5)]);
    let d5 = e.push(u, def, vec![v.str("p@5"), v.int(10)]);
    let d6 = e.push(u, def, vec![v.str("p@6"), v.int(20)]);
    let r = e.push(u, rd, vec![v.int(0)]);
    e.evaluate(threads).unwrap();
    assert_eq!(e.output(r), Some(v.int(10)));
    Doc {
        e,
        def,
        reader,
        n,
        d5,
        d6,
        r,
    }
}

impl Doc {
    fn runs(&self) -> u64 {
        self.reader.0.load(Ordering::Relaxed)
    }

    fn set(&self, node: phitex_defuse::NodeId, name: &str, x: i64) {
        let v = self.e.values();
        self.e.replace(node, self.def, vec![v.str(name), v.int(x)]);
    }

    fn eval(&self, threads: usize) -> i64 {
        let r = self.e.evaluate(threads).unwrap();
        assert_eq!(self.e.check_invariants(), Vec::<String>::new(), "{r:?}");
        self.e.values().as_int(self.e.output(self.r).unwrap()).unwrap()
    }
}

#[test]
fn a_moved_name_holding_the_old_value_leaves_the_reader_standing() {
    for threads in [1, 4] {
        let d = doc(threads);
        // (n moves to 6, and p@6 now holds what p@5 held)
        d.set(d.n, "n", 6);
        d.set(d.d6, "p@6", 10);
        assert_eq!(d.eval(threads), 10);
        assert_eq!(d.runs(), 1, "the reader stands (threads {threads})");
        // (the field named now changes: the witness asks the reader again)
        d.set(d.d6, "p@6", 30);
        assert_eq!(d.eval(threads), 30, "threads {threads}");
        assert_eq!(d.runs(), 2, "threads {threads}");
        // (the old name's field changes: nothing the reader uses)
        d.set(d.d5, "p@5", 40);
        assert_eq!(d.eval(threads), 30, "threads {threads}");
        assert_eq!(d.runs(), 2, "threads {threads}");
    }
}

#[test]
fn a_moved_name_holding_another_value_runs_the_reader_again() {
    for threads in [1, 4] {
        let d = doc(threads);
        d.set(d.n, "n", 6);
        assert_eq!(d.eval(threads), 20, "threads {threads}");
        assert_eq!(d.runs(), 2, "threads {threads}");
        // (and back: p@5 read again, through the read that run made)
        d.set(d.n, "n", 5);
        assert_eq!(d.eval(threads), 10, "threads {threads}");
        assert_eq!(d.runs(), 3, "threads {threads}");
        d.set(d.d5, "p@5", 11);
        assert_eq!(d.eval(threads), 11, "threads {threads}");
        assert_eq!(d.runs(), 4, "threads {threads}");
    }
}

#[test]
fn the_named_field_changing_runs_the_reader_again() {
    for threads in [1, 4] {
        let d = doc(threads);
        d.set(d.d5, "p@5", 12);
        assert_eq!(d.eval(threads), 12, "threads {threads}");
        assert_eq!(d.runs(), 2, "threads {threads}");
        // (a name no field has: none)
        d.set(d.n, "n", 9);
        d.e.evaluate(threads).unwrap();
        assert_eq!(d.e.output(d.r), Some(ValueId::UNIT), "threads {threads}");
    }
}
