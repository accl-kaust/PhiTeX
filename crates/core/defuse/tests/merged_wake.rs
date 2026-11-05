//! A loop that parked at its end waiting for more contributions is woken
//! by one coming, the answer it parked on, and goes on from its state, its
//! last run's reads kept and not checked again: they stood until it ran.
//! A re-ask that comes while it waits in the queue is merged into that
//! wake, one cause told; the read it was for, one of the kept, is checked
//! at the commit all the same, or the loop keeps what it made from a
//! value gone (argumentation-doc at -j4: a loop rest woken by its hole
//! filling, its start page's give-back changed meanwhile, kept a page of
//! the routine's earlier run, 2 to 7 builds in 60).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{At, Cause, Ctx, Engine, Field, FieldAt, NodeId, Operation, Stop, ValueId};

/// `[s, x]`: contributes x to s.
struct Contrib;

impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.contribute(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }

    fn contributes(&self, _: &[ValueId], _: &phitex_defuse::Values, _: Field) -> bool {
        true
    }
}

/// `[x]`: defines "g" = x.
struct DefG;

impl Operation for DefG {
    fn name(&self) -> &'static str {
        "def_g"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        cx.define(v.field("g"), cx.args()[0]);
        Ok(cx.args()[0])
    }

    fn contributes(&self, _: &[ValueId], _: &phitex_defuse::Values, _: Field) -> bool {
        false
    }
}

/// `[s]`: from the start, "g" at the unit's end, then the contributions
/// to s in line order summed onto it; it parks `[cursor, sum]` on s, and
/// a run woken by a contribution past its cursor goes on from there (not
/// reading "g" again: that read is kept). Its output is the sum.
struct Loop(AtomicU64);

impl Operation for Loop {
    fn name(&self) -> &'static str {
        "loop"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let s = Field(cx.args()[0]);
        let state = match (cx.resumed(), cx.woken_by()) {
            (Some(st), Some(Cause::Range { field, by, .. })) if field == s => {
                let parts = v.with_list(st, <[ValueId]>::to_vec).unwrap_or_default();
                let c = v.as_int(parts[0]).unwrap_or(-1);
                (c < 0 || cx.before(NodeId::from_raw(c.unsigned_abs()), by)).then_some(st)
            }
            _ => None,
        };
        let (mut cur, mut sum) = match state {
            Some(st) => {
                self.0.fetch_add(1, Ordering::Relaxed);
                cx.goes_on();
                let parts = v.with_list(st, <[ValueId]>::to_vec).unwrap_or_default();
                let c = v.as_int(parts[0]).unwrap_or(-1);
                (
                    (c >= 0).then(|| NodeId::from_raw(c.unsigned_abs())),
                    v.as_int(parts[1]).unwrap_or(0),
                )
            }
            None => {
                let g = match cx.read_at(v.field("g"), At::End(cx.unit()))? {
                    FieldAt::Def(x, _) => v.as_int(x).unwrap_or(0),
                    _ => 0,
                };
                (None, g)
            }
        };
        while let Some((n, x)) = cx.next_contribution(s, cur)? {
            sum += v.as_int(x).unwrap_or(0);
            cur = Some(n);
        }
        cx.park(v.list(vec![
            v.int(cur.map_or(-1, |n| i64::try_from(n.raw()).unwrap())),
            v.int(sum),
        ]));
        cx.parked_on(s);
        Ok(v.int(sum))
    }

    fn contributes(&self, _: &[ValueId], _: &phitex_defuse::Values, _: Field) -> bool {
        false
    }
}

/// A contributor, "g"'s definer, the loop (100 + 1). Then a contributor
/// added before the definer and "g" changed, in one evaluation: the loop
/// is asked for both, one cause told (the contribution, past its cursor),
/// and it goes on from its state; its sum is the new "g" plus both
/// contributions, at one thread and at four.
#[test]
fn a_re_ask_merged_into_the_parked_answer_checks_the_kept_reads() {
    for threads in [1, 4] {
        for round in 0..4 {
            let e = Engine::new();
            let v = e.values();
            let contrib = e.register(Arc::new(Contrib));
            let def_g = e.register(Arc::new(DefG));
            let lp = Arc::new(Loop(AtomicU64::new(0)));
            let lop = e.register(lp.clone());
            let s = v.field("s");
            let u = e.new_unit();
            e.push(u, contrib, vec![s.0, v.int(1)]);
            let dg = e.push(u, def_g, vec![v.int(100)]);
            let l = e.push(u, lop, vec![s.0]);
            e.evaluate(threads).unwrap();
            assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(101));
            e.insert_before(dg, contrib, vec![s.0, v.int(20)]);
            e.replace(dg, def_g, vec![v.int(1000)]);
            let r = e.evaluate(threads).unwrap();
            assert_eq!(
                e.output(l).and_then(|x| v.as_int(x)),
                Some(1021),
                "-j{threads} round {round}: the loop went on {} times; {r:?}",
                lp.0.load(Ordering::Relaxed)
            );
            assert!(e.check_invariants().is_empty());
        }
    }
}
