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
    /// The reader's runs in the first evaluation: one at one thread; on
    /// several, the reader may also run beside the definers before they
    /// commit, and again after (a cold build's nodes run side by side).
    first: u64,
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
    let first = reader.0.load(Ordering::Relaxed);
    if threads == 1 {
        assert_eq!(first, 1, "one run at one thread");
    }
    Doc {
        e,
        def,
        reader,
        n,
        d5,
        d6,
        r,
        first,
    }
}

impl Doc {
    /// The reader's runs since the first evaluation.
    fn runs(&self) -> u64 {
        self.reader.0.load(Ordering::Relaxed) - self.first
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
        assert_eq!(d.runs(), 0, "the reader stands (threads {threads})");
        // (the field named now changes: the witness asks the reader again)
        d.set(d.d6, "p@6", 30);
        assert_eq!(d.eval(threads), 30, "threads {threads}");
        assert_eq!(d.runs(), 1, "threads {threads}");
        // (the old name's field changes: nothing the reader uses)
        d.set(d.d5, "p@5", 40);
        assert_eq!(d.eval(threads), 30, "threads {threads}");
        assert_eq!(d.runs(), 1, "threads {threads}");
    }
}

#[test]
fn a_moved_name_holding_another_value_runs_the_reader_again() {
    for threads in [1, 4] {
        let d = doc(threads);
        d.set(d.n, "n", 6);
        assert_eq!(d.eval(threads), 20, "threads {threads}");
        assert_eq!(d.runs(), 1, "threads {threads}");
        // (and back: the run before the last stands, every read of it
        // answering as it did, `Ctx::prior_run_stands`; no run)
        d.set(d.n, "n", 5);
        assert_eq!(d.eval(threads), 10, "threads {threads}");
        assert_eq!(d.runs(), 1, "threads {threads}");
        // (p@5 read again, through the read that record made)
        d.set(d.d5, "p@5", 11);
        assert_eq!(d.eval(threads), 11, "threads {threads}");
        assert_eq!(d.runs(), 2, "threads {threads}");
    }
}

#[test]
fn the_named_field_changing_runs_the_reader_again() {
    for threads in [1, 4] {
        let d = doc(threads);
        d.set(d.d5, "p@5", 12);
        assert_eq!(d.eval(threads), 12, "threads {threads}");
        assert_eq!(d.runs(), 1, "threads {threads}");
        // (a name no field has: none)
        d.set(d.n, "n", 9);
        d.e.evaluate(threads).unwrap();
        assert_eq!(d.e.output(d.r), Some(ValueId::UNIT), "threads {threads}");
    }
}

/// `[node]`: the field `p@<n>`, `n` the value of field "n" reaching
/// `node` (`Ctx::read_derived_from`), not the reader; counts its runs.
struct DerivedFrom(AtomicU64);

impl Operation for DerivedFrom {
    fn name(&self) -> &'static str {
        "derived_from"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let v = cx.values();
        let at = v.as_int(cx.args()[0]).unwrap_or(0);
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(at.to_ne_bytes()));
        let (x, _) = cx.read_derived_from(v.field("n"), at, "p@", 0, "", PathId::WHOLE)?;
        Ok(x.unwrap_or(ValueId::UNIT))
    }
}

/// A derived read whose base is read before another node (a name made
/// with another piece's count, docs/spec-symbol-reads.md B2): n = 5 at
/// that node, then p@5, p@6 and n = 100 between it and the reader, which
/// reads `p@<n there>`. A move of n there whose new name holds the old
/// answer leaves the reader standing, though n reaching the reader never
/// moved; a change of the named field after that node asks it again; a
/// move onto a name holding another value runs it again.
#[test]
fn a_derived_read_across_nodes_follows_the_base_where_it_is_read() {
    for threads in [1, 4] {
        let e = Engine::new();
        let v = e.values();
        let def = e.register(Arc::new(Def));
        let reader = Arc::new(DerivedFrom(AtomicU64::new(0)));
        let rd = e.register(reader.clone());
        let u = e.new_unit();
        let n = e.push(u, def, vec![v.str("n"), v.int(5)]);
        let at = e.push(u, def, vec![v.str("mark"), v.int(0)]);
        let d5 = e.push(u, def, vec![v.str("p@5"), v.int(10)]);
        let d6 = e.push(u, def, vec![v.str("p@6"), v.int(20)]);
        e.push(u, def, vec![v.str("n"), v.int(100)]);
        let raw = i64::from_ne_bytes(at.raw().to_ne_bytes());
        let r = e.push(u, rd, vec![v.int(raw)]);
        e.evaluate(threads).unwrap();
        assert_eq!(e.output(r), Some(v.int(10)), "threads {threads}");
        let first = reader.0.load(Ordering::Relaxed);
        let runs = || reader.0.load(Ordering::Relaxed) - first;
        let eval = || {
            let rep = e.evaluate(threads).unwrap();
            assert_eq!(e.check_invariants(), Vec::<String>::new(), "{rep:?}");
            v.as_int(e.output(r).unwrap()).unwrap()
        };
        let set = |node, name: &str, x: i64| e.replace(node, def, vec![v.str(name), v.int(x)]);
        // (n moves to 6 where it is read, p@6 now holding what p@5 held)
        set(n, "n", 6);
        set(d6, "p@6", 10);
        assert_eq!(eval(), 10, "threads {threads}");
        assert_eq!(runs(), 0, "the reader stands (threads {threads})");
        // (the field named now changes after the base's node)
        set(d6, "p@6", 30);
        assert_eq!(eval(), 30, "threads {threads}");
        assert_eq!(runs(), 1, "threads {threads}");
        // (the old name's field: nothing the reader uses)
        set(d5, "p@5", 40);
        assert_eq!(eval(), 30, "threads {threads}");
        assert_eq!(runs(), 1, "threads {threads}");
        // (n moves back, onto p@5's other value)
        set(n, "n", 5);
        assert_eq!(eval(), 40, "threads {threads}");
        assert_eq!(runs(), 2, "threads {threads}");
    }
}

/// `[node]`: [`DerivedFrom`]'s read keyed by field "k"
/// (`Ctx::read_derived_keyed`); counts its runs.
struct DerivedKeyed(AtomicU64);

impl Operation for DerivedKeyed {
    fn name(&self) -> &'static str {
        "derived_keyed"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let v = cx.values();
        let at = v.as_int(cx.args()[0]).unwrap_or(0);
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(at.to_ne_bytes()));
        let (x, _) = cx.read_derived_keyed(v.field("n"), at, v.field("k"), "p@", 0, "", PathId::WHOLE)?;
        Ok(x.unwrap_or(ValueId::UNIT))
    }
}

/// A keyed derived read (docs/spec-symbol-reads.md B4): key k = [5, p@5]
/// says the name was p@5 when n gave 5. n moving to 6 (a picture
/// inserted before) reads p@5 still, though p@6 holds another value: the
/// reader stands. The key then saying [6, p@6], with p@6 holding that
/// value (the next round's file), it stands again; a change of p@6 runs
/// it; with the key saying the digits now, the name is read as TeX
/// reads it. The plain read (`read_derived_from`) runs again on the move.
#[test]
fn a_keyed_derived_read_follows_the_name_its_key_says() {
    for threads in [1, 4] {
        let e = Engine::new();
        let v = e.values();
        let def = e.register(Arc::new(Def));
        let reader = Arc::new(DerivedKeyed(AtomicU64::new(0)));
        let rd = e.register(reader.clone());
        let plain = Arc::new(DerivedFrom(AtomicU64::new(0)));
        let pd = e.register(plain.clone());
        let u = e.new_unit();
        let key = |d: i64, g: &str| v.list(vec![v.int(d), v.str(g)]);
        let k = e.push(u, def, vec![v.str("k"), key(5, "p@5")]);
        let n = e.push(u, def, vec![v.str("n"), v.int(5)]);
        let at = e.push(u, def, vec![v.str("mark"), v.int(0)]);
        e.push(u, def, vec![v.str("p@5"), v.int(10)]);
        let d6 = e.push(u, def, vec![v.str("p@6"), v.int(20)]);
        let raw = i64::from_ne_bytes(at.raw().to_ne_bytes());
        let r = e.push(u, rd, vec![v.int(raw)]);
        let rp = e.push(u, pd, vec![v.int(raw)]);
        e.evaluate(threads).unwrap();
        assert_eq!(e.output(r), Some(v.int(10)), "threads {threads}");
        assert_eq!(e.output(rp), Some(v.int(10)), "threads {threads}");
        let first = reader.0.load(Ordering::Relaxed);
        let runs = || reader.0.load(Ordering::Relaxed) - first;
        let pfirst = plain.0.load(Ordering::Relaxed);
        let eval = |node| {
            let rep = e.evaluate(threads).unwrap();
            assert_eq!(e.check_invariants(), Vec::<String>::new(), "{rep:?}");
            v.as_int(e.output(node).unwrap()).unwrap()
        };
        let set = |node, name: &str, x: ValueId| e.replace(node, def, vec![v.str(name), x]);
        // (n moves to 6: the name moved since the key was written)
        set(n, "n", v.int(6));
        assert_eq!(eval(r), 10, "threads {threads}");
        assert_eq!(runs(), 0, "the keyed reader stands (threads {threads})");
        assert_eq!(e.output(rp), Some(v.int(20)), "threads {threads}");
        assert_eq!(
            plain.0.load(Ordering::Relaxed) - pfirst,
            1,
            "the plain reader runs (threads {threads})"
        );
        // (the next round's file: the key says p@6, which holds the value)
        set(k, "k", key(6, "p@6"));
        set(d6, "p@6", v.int(10));
        assert_eq!(eval(r), 10, "threads {threads}");
        assert_eq!(runs(), 0, "threads {threads}");
        // (the name's field changes: TeX's read)
        set(d6, "p@6", v.int(30));
        assert_eq!(eval(r), 30, "threads {threads}");
        assert_eq!(runs(), 1, "threads {threads}");
    }
}

/// A derived read across nodes that stood where its name moved (its
/// last run's records name the old field) is asked through its path when
/// the field its name gives now changes: the counts of the fields it
/// recorded did not move. Read by those counts, it stood on 10 with p@6
/// at 30, the invariant check's stale read.
#[test]
fn a_derived_read_that_stood_where_its_name_moved_follows_the_new_name() {
    for threads in [1, 4] {
        let e = Engine::new();
        let v = e.values();
        let def = e.register(Arc::new(Def));
        let plain = Arc::new(DerivedFrom(AtomicU64::new(0)));
        let pd = e.register(plain.clone());
        let u = e.new_unit();
        let n = e.push(u, def, vec![v.str("n"), v.int(5)]);
        let at = e.push(u, def, vec![v.str("mark"), v.int(0)]);
        e.push(u, def, vec![v.str("p@5"), v.int(10)]);
        let d6 = e.push(u, def, vec![v.str("p@6"), v.int(20)]);
        let raw = i64::from_ne_bytes(at.raw().to_ne_bytes());
        let rp = e.push(u, pd, vec![v.int(raw)]);
        e.evaluate(threads).unwrap();
        let eval = |node| {
            let rep = e.evaluate(threads).unwrap();
            assert_eq!(e.check_invariants(), Vec::<String>::new(), "{rep:?}");
            v.as_int(e.output(node).unwrap()).unwrap()
        };
        let set = |node, name: &str, x: ValueId| e.replace(node, def, vec![v.str(name), x]);
        set(n, "n", v.int(6));
        assert_eq!(eval(rp), 20, "a {threads}");
        set(d6, "p@6", v.int(10));
        assert_eq!(eval(rp), 10, "b {threads}");
        set(d6, "p@6", v.int(30));
        assert_eq!(eval(rp), 30, "c {threads}");
    }
}
