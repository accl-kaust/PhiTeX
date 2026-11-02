//! A text node refuted in an evaluation runs again only once the text
//! nodes before it on its unit have committed (`sched::work`): a chain of
//! text nodes each reading what the one before defined (LaTeX's state from
//! paragraph to paragraph: `\everypar`, `\if@nobreak`) ran, at several
//! threads, once for every commit before it (a Grail node 74 times, accl
//! 22515); now at most twice, its guess and its run on the true input.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId};

/// `[field]`: reads `field` (absent: 0), defines it as one more, after a
/// moment's work (so the runs of several threads overlap).
struct Step;

impl Operation for Step {
    fn name(&self) -> &'static str {
        "step"
    }

    fn text(&self) -> bool {
        true
    }

    fn alike(&self) -> bool {
        false
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        let x = cx.read(f)?.and_then(|x| cx.values().as_int(x)).unwrap_or(0);
        std::thread::sleep(std::time::Duration::from_micros(200));
        let y = cx.values().int(x + 1);
        cx.define(f, y);
        Ok(y)
    }
}

#[test]
fn a_refuted_text_node_waits_for_the_text_before_it() {
    let e = Engine::new();
    let v = e.values();
    let step = e.register(Arc::new(Step));
    let u = e.new_unit();
    let n = 300;
    let nodes: Vec<_> = (0..n).map(|_| e.push(u, step, vec![v.field("s").0])).collect();
    let rep = e.evaluate(8).unwrap();
    for (k, &x) in nodes.iter().enumerate() {
        assert_eq!(e.output(x).and_then(|y| v.as_int(y)), Some(k as i64 + 1), "node {k}: {rep:?}");
    }
    // (a guess and the run on the true input: at most two runs a node)
    assert!(
        rep.runs <= 2 * n as u64,
        "{} runs for {n} nodes in a chain at 8 threads: a refuted node ran again before the node before it committed",
        rep.runs
    );
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[field, x, ms]`: a text node that may define `field` (only it), takes
/// `ms` milliseconds and defines it := x.
struct Slow;

impl Operation for Slow {
    fn name(&self) -> &'static str {
        "slow"
    }

    fn text(&self) -> bool {
        true
    }

    fn may_define(&self, args: &[ValueId], _: &phitex_defuse::Values) -> phitex_defuse::MayDefine {
        phitex_defuse::MayDefine::Names(vec![Field(args[0])])
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[2]).unwrap_or(0);
        std::thread::sleep(std::time::Duration::from_millis(ms.try_into().unwrap_or(0)));
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[from, to]`: a text node reading `from` (absent: 0) and defining `to`
/// as it.
struct Read;

impl Operation for Read {
    fn name(&self) -> &'static str {
        "read"
    }

    fn text(&self) -> bool {
        true
    }

    fn may_define(&self, args: &[ValueId], _: &phitex_defuse::Values) -> phitex_defuse::MayDefine {
        phitex_defuse::MayDefine::Names(vec![Field(args[1])])
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or_else(|| cx.values().int(0));
        cx.define(Field(cx.args()[1]), x);
        Ok(x)
    }
}

#[test]
fn a_read_past_a_node_not_yet_run_that_may_define_it_waits_for_it() {
    let e = Engine::new();
    let v = e.values();
    let slow = e.register(Arc::new(Slow));
    let read = e.register(Arc::new(Read));
    let u = e.new_unit();
    // (the readers dealt to the other workers' blocks, run at once)
    e.push(u, slow, vec![v.field("f").0, v.int(7), v.int(50)]);
    let rs: Vec<_> = (0..40)
        .map(|k| e.push(u, read, vec![v.field("f").0, v.field(&format!("g{k}")).0]))
        .collect();
    let rep = e.evaluate(4).unwrap();
    for &r in &rs {
        assert_eq!(e.output(r).and_then(|y| v.as_int(y)), Some(7), "{rep:?}");
    }
    assert_eq!(rep.runs, 41, "a reader ran before the node that may define f had: {rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

#[test]
fn a_read_past_a_node_not_yet_run_that_cannot_define_it_goes_on() {
    let e = Engine::new();
    let v = e.values();
    let slow = e.register(Arc::new(Slow));
    let read = e.register(Arc::new(Read));
    let u = e.new_unit();
    e.push(u, slow, vec![v.field("h").0, v.int(7), v.int(50)]);
    let rs: Vec<_> = (0..40)
        .map(|k| e.push(u, read, vec![v.field("f").0, v.field(&format!("g{k}")).0]))
        .collect();
    let rep = e.evaluate(4).unwrap();
    for &r in &rs {
        assert_eq!(e.output(r).and_then(|y| v.as_int(y)), Some(0), "{rep:?}");
    }
    assert_eq!(rep.runs, 41, "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}
