//! The cost of an edit that replaces or grows a unit's text, against a
//! cold evaluation of the new text (from the client's report).
#![allow(clippy::many_single_char_names)] // the client's operation reads best this way

use std::sync::Arc;

use defuse::*;

/// `["i", kind, k]`: a node whose definitions, claim and scope effect are
/// known only when it runs (like every node of the text). Kinds: `d`
/// reads `x` and the contributions to `acc` before it, defines `x` and
/// contributes to `acc`; `o`/`e` open/close a
/// scope; `c` claims its following sibling; `u` unfolds two `d`
/// children; `r` reads the loop-back field `lb`; `p` publishes `lb`.
struct Item;

static ITEM: std::sync::OnceLock<OpId> = std::sync::OnceLock::new();

fn item_text(n: usize, salt: i64) -> Vec<(String, i64)> {
    (0..n)
        .map(|i| {
            let kind = if i == n - 1 {
                "p"
            } else if i % 10 == 0 {
                "o"
            } else if i % 10 == 9 {
                "e"
            } else if i % 50 == 5 {
                "c"
            } else if i % 25 == 7 {
                "u"
            } else if i == 3 || i == n / 2 {
                "r"
            } else {
                "d"
            };
            (kind.to_owned(), i64::try_from(i).unwrap_or(0) + salt)
        })
        .collect()
}

impl Operation for Item {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> defuse::Effect {
        defuse::Effect::Pending
    }
    fn claim(&self, _: &[ValueId], _: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        Claim::Pending
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let kind = match a.get(1).map(|&x| v.get(x)).as_deref() {
            Some(Value::Str(s)) => s.to_string(),
            _ => String::new(),
        };
        let k = match a.get(2).map(|&x| v.get(x)).as_deref() {
            Some(Value::Int(n)) => *n,
            _ => 0,
        };
        let x = v.field("x");
        let mut effect = defuse::Effect::None;
        let mut claim = 0;
        match kind.as_str() {
            "d" => {
                let old = cx.use_field(x)?.and_then(|y| v.as_int(y)).unwrap_or(0);
                // the contributions so far, from the unit's start
                let before = cx.acc_range(v.field("acc"), None)?;
                let m = i64::try_from(v.seq_to_vec(before).len()).unwrap_or(0);
                cx.define(x, v.int(old.wrapping_mul(31).wrapping_add(k + m)));
                let s = v.seq_unit(v.int(k));
                cx.contribute(v.field("acc"), s);
            }
            "o" => effect = defuse::Effect::Open,
            "e" => effect = defuse::Effect::Close,
            "c" => {
                claim = usize::from(cx.following(0).is_some());
                cx.define(v.field("y"), v.int(k));
            }
            "u" => {
                let op = ITEM.get().copied().unwrap();
                cx.child(op, vec![v.str("i"), v.str("d"), v.int(k)]);
                cx.child(op, vec![v.str("i"), v.str("d"), v.int(k + 1)]);
            }
            "r" => {
                let l = cx.use_field(v.field("lb"))?.and_then(|y| v.as_int(y)).unwrap_or(0);
                cx.define(v.field("z"), v.int(l + k));
            }
            "p" => cx.publish(v.field("lb"), v.int(k % 3)),
            _ => {}
        }
        cx.set_effect(effect);
        cx.claim(claim);
        Ok(ValueId::UNIT)
    }
}

fn item_engine() -> (Engine, OpId) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Item));
    let _ = ITEM.set(op);
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(defuse::SeqMonoid));
    let lb = eng.values().field("lb");
    let zero = eng.values().int(0);
    eng.declare_loopback(lb, zero);
    (eng, op)
}

fn item_args(v: &Values, kind: &str, k: i64) -> Vec<ValueId> {
    vec![v.str("i"), v.str(kind), v.int(k)]
}

/// An edit that replaces the whole text costs about what a cold
/// evaluation of the new text costs (a cold evaluation is an edit from
/// nothing).
#[test]
fn replacing_the_whole_text_costs_a_cold_evaluation() {
    let n = 1000;
    let old = item_text(n, 0);
    let new = item_text(n, 7);
    // cold, new text
    let (mut cold, op) = item_engine();
    let u = cold.new_unit();
    for (kind, k) in &new {
        let a = item_args(cold.values(), kind, *k);
        cold.push(u, op, a);
    }
    let t0 = std::time::Instant::now();
    let rc = cold.evaluate_sequential().expect("cold");
    let tc = t0.elapsed();
    // old text, then every node replaced by the new text's
    let (mut eng, op) = item_engine();
    let u = eng.new_unit();
    let mut ids = Vec::new();
    for (kind, k) in &old {
        let a = item_args(eng.values(), kind, *k);
        ids.push(eng.push(u, op, a));
    }
    eng.evaluate_sequential().expect("old");
    for (id, (kind, k)) in ids.iter().zip(&new) {
        let a = item_args(eng.values(), kind, *k);
        eng.replace(*id, op, a);
    }
    let t0 = std::time::Instant::now();
    let re = eng.evaluate_sequential().expect("edit");
    let te = t0.elapsed();
    eprintln!("cold {tc:?}: {rc:?}");
    eprintln!("edit {te:?}: {re:?}");
    assert!(
        te.as_secs_f64() <= 1.5 * tc.as_secs_f64(),
        "edit {te:?} against cold {tc:?}"
    );
}

/// The same, the edit growing the text from 200 nodes to 1000 (the new
/// nodes inserted after the last old one).
#[test]
fn growing_the_text_costs_a_cold_evaluation() {
    let n = 1000;
    let old = item_text(200, 0);
    let new = item_text(n, 7);
    let (mut cold, op) = item_engine();
    let u = cold.new_unit();
    for (kind, k) in &new {
        let a = item_args(cold.values(), kind, *k);
        cold.push(u, op, a);
    }
    let t0 = std::time::Instant::now();
    let rc = cold.evaluate_sequential().expect("cold");
    let tc = t0.elapsed();
    let (mut eng, op) = item_engine();
    let u = eng.new_unit();
    let mut ids = Vec::new();
    for (kind, k) in &old {
        let a = item_args(eng.values(), kind, *k);
        ids.push(eng.push(u, op, a));
    }
    eng.evaluate_sequential().expect("old");
    for (id, (kind, k)) in ids.iter().zip(&new) {
        let a = item_args(eng.values(), kind, *k);
        eng.replace(*id, op, a);
    }
    let mut last = *ids.last().unwrap();
    for (kind, k) in &new[ids.len()..] {
        let a = item_args(eng.values(), kind, *k);
        last = eng.insert_after(last, op, a);
    }
    let t0 = std::time::Instant::now();
    let re = eng.evaluate_sequential().expect("edit");
    let te = t0.elapsed();
    eprintln!("cold {tc:?}: {rc:?}");
    eprintln!("edit {te:?}: {re:?}");
    assert!(
        te.as_secs_f64() <= 1.5 * tc.as_secs_f64(),
        "edit {te:?} against cold {tc:?}"
    );
}
