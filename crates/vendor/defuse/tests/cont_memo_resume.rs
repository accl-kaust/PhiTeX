//! A continuing placer's memo of its placements does not outlive a run
//! of it resumed at a mark. Unit [s, a, r]: `s` defines `x`; `a` reads
//! `x`, defines `z` and places final `d(x)` after itself, which defines
//! `y = x / 10` (guessed from the last). `r` goes through items 0..5,
//! before each marking and parking continuing: item 0 places `c(0)` and
//! reads `z`; item 1 reads `y`; item 2 places one child of `op` (`y` 1) or
//! three, of `op2`, `op`, `op` (`y` 2), and waits for the first; items 3
//! and 4 place `c(i + 4 + z)` and wait for it. An edit of `x`: `r` resumes
//! at mark 0 on `y` guessed 1 and stops at item 3, then, from its token,
//! at item 4 (its memo: four placements of `op`); `d` makes `y = 2` in the
//! final phase, `r` resumes at mark 1, places three children at item 2,
//! four in all as the memo counted, of other operations, and stops; its
//! next stop does not take that memo (CLEAN_KEPT_CHECK compares a memo
//! taken with the walk: the stale one failed it).

use std::sync::{Arc, OnceLock};

use defuse::{Ctx, Engine, Field, Locality, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    op: OnceLock<OpId>,
    op2: OnceLock<OpId>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "s" => vec![(v.field("x"), Locality::Global)],
            "a" => vec![(v.field("z"), Locality::Global)],
            "d" => vec![(v.field("y"), Locality::Global)],
            _ => Vec::new(),
        })
    }
    fn final_only(&self, a: &[ValueId], v: &Values) -> bool {
        tag(v, a) == "d"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let (op, op2) = (*self.op.get().unwrap(), *self.op2.get().unwrap());
        let (x, y, z) = (v.field("x"), v.field("y"), v.field("z"));
        match tag(v, &a).as_str() {
            "s" => {
                cx.define_with(x, a[1], Locality::Global);
                return Ok(ValueId::UNIT);
            }
            "a" => {
                let xv = cx.use_field(x)?.unwrap_or(v.int(0));
                cx.define_with(z, xv, Locality::Global);
                cx.follow(op, vec![v.str("d"), xv]);
                return Ok(ValueId::UNIT);
            }
            "d" => {
                cx.define_with(y, v.int(v.as_int(a[1]).unwrap() / 10), Locality::Global);
                return Ok(ValueId::UNIT);
            }
            "c" => return Ok(a[1]),
            _ => {}
        }
        // Parked: on from the token (its item's children placed); resumed
        // at a mark: from that mark's sums.
        let parked = cx.parked().and_then(|t| t.downcast::<(usize, Vec<i64>)>().ok());
        let (mut i, mut sums, mut placed) = match parked {
            Some(t) => (t.0, t.1, true),
            None => match cx.resumed() {
                Some((tok, k)) => (k as usize, tok.downcast_ref::<Vec<i64>>().expect("sums")[..=k as usize].to_vec(), false),
                None => (0, vec![0], false),
            },
        };
        while i < 5 {
            let sum = *sums.last().expect("a sum");
            cx.checkpoint(u32::try_from(i).unwrap(), v.int(sum))?;
            cx.keep_after(Arc::new(sums.clone()));
            cx.park_continuing(Box::new((i, sums.clone())));
            let q = match i {
                0 => {
                    if !placed {
                        cx.child(op, vec![v.str("c"), v.int(0)]);
                    }
                    cx.use_field(z)?.and_then(|q| v.as_int(q)).unwrap_or(0)
                }
                1 => cx.use_field(y)?.and_then(|q| v.as_int(q)).unwrap_or(0),
                2 => {
                    let yv = cx.use_field(y)?.and_then(|q| v.as_int(q)).unwrap_or(0);
                    if !placed {
                        if yv == 1 {
                            cx.child(op, vec![v.str("c"), v.int(1)]);
                        } else {
                            cx.child(op2, vec![v.str("c"), v.int(1)]);
                            cx.child(op, vec![v.str("c"), v.int(0)]);
                            cx.child(op, vec![v.str("c"), v.int(0)]);
                        }
                    }
                    v.as_int(cx.output_of(1)?).unwrap_or(0)
                }
                _ => {
                    let yv = cx.use_field(y)?.and_then(|q| v.as_int(q)).unwrap_or(0);
                    let zv = cx.use_field(z)?.and_then(|q| v.as_int(q)).unwrap_or(0);
                    let k = i64::try_from(i).unwrap() + 4;
                    if !placed {
                        cx.child(op, vec![v.str("c"), v.int(k + zv)]);
                    }
                    // (Its index: after item 2's one child or three.)
                    let ix = if yv == 1 { i - 1 } else { i + 1 };
                    v.as_int(cx.output_of(ix)?).unwrap_or(0)
                }
            };
            sums.push(sum + q);
            i += 1;
            placed = false;
        }
        Ok(v.int(*sums.last().expect("a sum")))
    }
}

#[test]
fn a_placer_resumed_at_a_mark_takes_no_memo_of_before() {
    // (The memo compared with the walk: before any engine reads it.)
    unsafe { std::env::set_var("CLEAN_KEPT_CHECK", "1") };
    let mut eng = Engine::new();
    let ops = Arc::new(Ops { op: OnceLock::new(), op2: OnceLock::new() });
    let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
    let op2 = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
    ops.op.set(op).expect("once");
    ops.op2.set(op2).expect("once");
    let y = eng.values().field("y");
    eng.guess_from_last(y);
    let u = eng.new_unit();
    let s_args = vec![eng.values().str("s"), eng.values().int(10)];
    let s = eng.push(u, op, s_args);
    let a_args = vec![eng.values().str("a")];
    eng.push(u, op, a_args);
    let r_args = vec![eng.values().str("r")];
    let r = eng.push(u, op, r_args);
    let spec = |eng: &mut Engine| eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut defuse::Sequential).expect("evaluate");
    spec(&mut eng);
    // (z, y, item 2's first child 1, c(7 + z), c(8 + z).)
    assert_eq!(eng.output(r).and_then(|q| eng.values().as_int(q)), Some(10 + 1 + 1 + 17 + 18));
    let s_args = vec![eng.values().str("s"), eng.values().int(25)];
    eng.replace(s, op, s_args);
    spec(&mut eng);
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    assert_eq!(eng.output(r).and_then(|q| eng.values().as_int(q)), Some(25 + 2 + 1 + 32 + 33));
}
