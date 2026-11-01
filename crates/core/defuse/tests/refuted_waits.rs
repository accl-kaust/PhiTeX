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
