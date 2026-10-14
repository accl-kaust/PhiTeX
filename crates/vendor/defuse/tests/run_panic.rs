//! A run that panics ends the evaluation with `EvalError::OpPanicked`
//! naming the node, its operation and the message, on any executor.

use std::sync::atomic::{AtomicBool, Ordering};

use defuse::{Ctx, Engine, EvalError, Field, Greedy, Locality, Operation, Sequential, Stop, ValueId, Values};

struct Boom(AtomicBool);

impl Operation for Boom {
    fn name(&self) -> &'static str {
        "boom"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        if v.as_int(cx.args()[0]) == Some(3) && self.0.load(Ordering::Relaxed) {
            todo!("path three")
        }
        Ok(cx.args()[0])
    }
}

struct Pool(usize);

impl defuse::Executor for Pool {
    fn parallelism(&self) -> usize {
        self.0
    }
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> defuse::RunResult + Sync), out: &mut Vec<defuse::RunResult>) {
        let res: Vec<defuse::RunResult> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..n).map(|i| s.spawn(move || run(i))).collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        out.extend(res);
    }
}

#[test]
fn a_panicking_run_fails_the_evaluation() {
    for threads in [0usize, 1, 4] {
        let boom = std::sync::Arc::new(Boom(AtomicBool::new(true)));
        let mut eng = Engine::new();
        let op = eng.register(boom.clone());
        let u = eng.new_unit();
        let ids: Vec<_> = (0..6).map(|i| { let x = eng.values().int(i); eng.push(u, op, vec![x]) }).collect();
        let r = if threads == 0 {
            eng.evaluate(&mut Greedy::default(), &mut Sequential)
        } else {
            eng.evaluate(&mut Greedy::default(), &mut Pool(threads))
        };
        match r {
            Err(EvalError::OpPanicked { node, op, message }) => {
                assert_eq!(node, ids[3]);
                assert_eq!(op, "boom");
                assert!(message.contains("path three"), "{message}");
            }
            other => panic!("threads {threads}: {other:?}"),
        }
        // Fixed, the node runs again and the evaluation finishes.
        boom.0.store(false, Ordering::Relaxed);
        let rep = if threads == 0 {
            eng.evaluate(&mut Greedy::default(), &mut Sequential)
        } else {
            eng.evaluate(&mut Greedy::default(), &mut Pool(threads))
        }
        .unwrap();
        assert_eq!(eng.output(ids[3]), Some(eng.values().int(3)));
        assert!(!rep.run_times.is_empty());
    }
}
