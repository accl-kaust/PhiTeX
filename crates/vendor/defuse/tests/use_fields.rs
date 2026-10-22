//! `Ctx::use_fields` / `use_fields_proj` read as `use_field` /
//! `use_field_proj` one at a time: the same values, stops and re-asks.
//! Unit [s, d_0 .. d_{M-1}, r]: `d_i` defines `f<i>` for even `i` (odd
//! ones are the environment's, `e<i>`... unset); `s` defines `x`, which
//! `d_0` reads (so `r` stops on a definer in work); `r` reads all `M`
//! fields, at once or one at a time, and outputs their sum (projected by
//! parity in the projected form). Edits of `x` and of a `d`'s value move
//! the output alike either way.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, ProjId, Stop, ValueId, Values};

const M: i64 = 40;

struct Ops {
    batch: bool,
    proj: std::sync::OnceLock<ProjId>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn fields(v: &Values) -> Vec<Field> {
    (0..M).map(|i| v.field(&format!("f{i}"))).collect()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "s" => vec![(v.field("x"), Locality::Global)],
            "d" => vec![(v.field(&format!("f{}", v.as_int(a[1]).unwrap())), Locality::Global)],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "s" => {
                cx.define_with(v.field("x"), a[1], Locality::Global);
                Ok(ValueId::UNIT)
            }
            "d" => {
                let i = v.as_int(a[1]).unwrap();
                let x = if i == 0 { cx.use_field(v.field("x"))?.and_then(|y| v.as_int(y)).unwrap_or(0) } else { 0 };
                cx.define_with(v.field(&format!("f{i}")), v.int(v.as_int(a[2]).unwrap() + x), Locality::Global);
                Ok(ValueId::UNIT)
            }
            _ => {
                let fs = fields(v);
                let p = *self.proj.get().unwrap();
                let projected = v.as_int(a[1]) == Some(1);
                let vals: Vec<Option<ValueId>> = match (self.batch, projected) {
                    (true, false) => {
                        let mut out = Vec::new();
                        cx.use_fields(&fs, &mut out)?;
                        out
                    }
                    (true, true) => {
                        let ps: Vec<(Field, ProjId)> = fs.iter().map(|&f| (f, p)).collect();
                        let mut out = Vec::new();
                        cx.use_fields_proj(&ps, &mut out)?;
                        out
                    }
                    (false, false) => fs.iter().map(|&f| cx.use_field(f)).collect::<Result<_, _>>()?,
                    (false, true) => fs.iter().map(|&f| cx.use_field_proj(f, p)).collect::<Result<_, _>>()?,
                };
                Ok(v.int(vals.iter().map(|x| x.and_then(|y| v.as_int(y)).unwrap_or(-1)).sum()))
            }
        }
    }
}

fn build(batch: bool) -> (Engine, NodeId, Vec<NodeId>, NodeId, NodeId, OpId) {
    let mut eng = Engine::new();
    let ops = Arc::new(Ops {
        batch,
        proj: std::sync::OnceLock::new(),
    });
    let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
    let p = eng.register_projection(Arc::new(|x: ValueId, v: &Values| v.int(v.as_int(x).unwrap_or(0) % 2)));
    ops.proj.set(p).expect("once");
    let u = eng.new_unit();
    let s_args = vec![eng.values().str("s"), eng.values().int(1)];
    let s = eng.push(u, op, s_args);
    let mut ds = Vec::new();
    for i in (0..M).step_by(2) {
        let d = vec![eng.values().str("d"), eng.values().int(i), eng.values().int(10 * i)];
        ds.push(eng.push(u, op, d));
    }
    let r_args = vec![eng.values().str("r"), eng.values().int(0)];
    let r = eng.push(u, op, r_args);
    let rp_args = vec![eng.values().str("r"), eng.values().int(1)];
    let rp = eng.push(u, op, rp_args);
    (eng, s, ds, r, rp, op)
}

#[test]
fn many_fields_read_at_once_read_as_one_at_a_time() {
    for seq in [true, false] {
        let mut outs = Vec::new();
        for batch in [false, true] {
            let (mut eng, s, ds, r, rp, op) = build(batch);
            let run = |eng: &mut Engine| {
                if seq {
                    eng.evaluate_sequential()
                } else {
                    eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256), &mut defuse::Sequential)
                }
                .expect("evaluate");
                assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
                (eng.output(r).and_then(|y| eng.values().as_int(y)), eng.output(rp).and_then(|y| eng.values().as_int(y)))
            };
            let mut o = vec![run(&mut eng)];
            let a = vec![eng.values().str("s"), eng.values().int(5)];
            eng.replace(s, op, a);
            o.push(run(&mut eng));
            let d = vec![eng.values().str("d"), eng.values().int(4), eng.values().int(7)];
            eng.replace(ds[2], op, d);
            o.push(run(&mut eng));
            outs.push(o);
        }
        assert_eq!(outs[0], outs[1], "seq {seq}: one at a time / at once");
    }
}
