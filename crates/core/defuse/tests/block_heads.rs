//! The head of a worker's block (a node dirty before the evaluation, not
//! run yet) runs before the re-asked readers that stand after it on the
//! line: a reader run behind a definer still to run reads what that
//! definer will change and runs again once it has. Taken the other way,
//! a worker ran every reader its block's first definer re-asked before
//! its second definer, and again after each: amsldoc at -j8 fell into
//! 13-15 thousand runs against 4 in a quarter of its builds, its
//! preamble's last block run after 2203 nodes (365 in the rest).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, NodeId, Operation, Stop, ValueId};

/// `[f, x]`: defines `f` := x.
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

/// `[f...]`: outputs the sum of the fields reaching it. Counts its runs.
struct Sum(AtomicU64);

impl Operation for Sum {
    fn name(&self) -> &'static str {
        "sum"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let fs = cx.args().to_vec();
        let mut t = 0;
        for f in fs {
            t += cx.read(Field(f))?.and_then(|x| cx.values().as_int(x)).unwrap_or(0);
        }
        Ok(cx.values().int(t))
    }
}

/// 64 definers (`f_i` := 0), then 200 readers of all of them. An edit
/// sets every `f_i` := 1: the 64 run first, each reader once after them,
/// at one thread; at eight, a reader runs at most twice (one that started
/// before the last definer of another worker's block committed).
#[test]
fn a_blocks_definers_run_before_the_readers_they_re_ask() {
    const DEFS: usize = 64;
    const READERS: u64 = 200;
    for threads in [1, 8] {
        let e = Engine::new();
        let sum = Arc::new(Sum(AtomicU64::new(0)));
        let def = e.register(Arc::new(Def));
        let sum_op = e.register(sum.clone());
        let v = e.values();
        let u = e.new_unit();
        let fs: Vec<ValueId> = (0..DEFS).map(|i| v.field(&format!("f{i}")).0).collect();
        let defs: Vec<NodeId> = fs.iter().map(|&f| e.push(u, def, vec![f, v.int(0)])).collect();
        let readers: Vec<NodeId> = (0..READERS).map(|_| e.push(u, sum_op, fs.clone())).collect();
        e.evaluate(threads).unwrap();
        for (d, &f) in defs.iter().zip(&fs) {
            e.replace(*d, def, vec![f, v.int(1)]);
        }
        sum.0.store(0, Ordering::SeqCst);
        let rep = e.evaluate(threads).unwrap();
        for r in &readers {
            assert_eq!(e.output(*r).and_then(|x| v.as_int(x)), Some(DEFS as i64), "-j{threads}");
        }
        let runs = sum.0.load(Ordering::SeqCst);
        let bound = if threads == 1 { READERS } else { 2 * READERS };
        assert!(
            runs <= bound,
            "-j{threads}: {runs} reader runs for {READERS} readers (at most {bound}): the readers ran between the definers: {rep:?}"
        );
        assert!(
            e.check_invariants().is_empty(),
            "-j{threads}: {:?}",
            e.check_invariants()
        );
    }
}
