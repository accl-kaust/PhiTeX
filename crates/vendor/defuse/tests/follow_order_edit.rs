//! Follows re-placed by a new parent keep text order after an edit: the
//! edited result equals a cold build of the edited text.
//!
//! `A [v]` defines `x = "a"` and follows `F [v]`, then `S`; `F [v]` follows
//! `P`; `S` reads `x`, defines `x = x + "s"`, follows `G`; `G` follows `Q`;
//! `P` and `Q` read `x` and define `x = x + name`. Editing `A` to `v = 2`
//! re-makes `F` (new args) while `S` stays; `F'` follows `P` with the same
//! args as before.

use std::sync::{Arc, Mutex};

use defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    me: Mutex<Option<OpId>>,
    /// Each reader's last read of `x`.
    saw: Mutex<std::collections::BTreeMap<String, String>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "a" | "s" | "p" | "q" => vec![(v.field("x"), Locality::Global)],
            _ => Vec::new(),
        })
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(match tag(v, a).as_str() {
            "s" | "p" | "q" => vec![v.field("x")],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = self.me.lock().unwrap().unwrap();
        let x = v.field("x");
        let t = tag(v, &a);
        match t.as_str() {
            "a" => {
                cx.define_with(x, v.str("a"), Locality::Global);
                cx.follow(op, vec![v.str("f"), a[1]]);
                cx.follow(op, vec![v.str("s")]);
                Ok(ValueId::UNIT)
            }
            "f" => {
                cx.follow(op, vec![v.str("p")]);
                Ok(ValueId::UNIT)
            }
            "g" => {
                cx.follow(op, vec![v.str("q")]);
                Ok(ValueId::UNIT)
            }
            _ => {
                let prev = cx.use_field(x)?.and_then(|p| v.as_str(p)).unwrap_or_default();
                self.saw.lock().unwrap().insert(t.clone(), prev.clone());
                let now = format!("{prev}{t}");
                cx.define_with(x, v.str(&now), Locality::Global);
                if t == "s" {
                    cx.follow(op, vec![v.str("g")]);
                }
                Ok(v.str(&now))
            }
        }
    }
}

fn build(ver: i64) -> (Engine, OpId, NodeId, Arc<Ops>) {
    let ops = Arc::new(Ops { me: Mutex::new(None), saw: Mutex::new(Default::default()) });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    *ops.me.lock().unwrap() = Some(op);
    let u = eng.new_unit();
    let (a, n) = (eng.values().str("a"), eng.values().int(ver));
    let an = eng.push(u, op, vec![a, n]);
    (eng, op, an, ops)
}

#[test]
fn an_edit_keeps_the_follow_order_of_a_cold_build() {
    let (mut cold, _, _, cops) = build(2);
    cold.evaluate_sequential().unwrap();
    let want = cops.saw.lock().unwrap().clone();
    assert_eq!(want.get("p").map(String::as_str), Some("a"), "cold: P reads A's x");
    let (mut eng, op, an, ops) = build(1);
    eng.evaluate_sequential().unwrap();
    let (a, n) = (eng.values().str("a"), eng.values().int(2));
    eng.replace(an, op, vec![a, n]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(*ops.saw.lock().unwrap(), want, "edited reads equal a cold build's");
}

/// A run that restarts drops what its first pass placed: `R` follows `S`,
/// restarts, then follows `F` and `S`; `F` defines `x`, `S` reads it.
struct Restarter {
    me: Mutex<Option<OpId>>,
    saw: Mutex<Vec<Option<String>>>,
}

impl Operation for Restarter {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "f" { vec![(v.field("x"), Locality::Local)] } else { Vec::new() })
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(if tag(v, a) == "s" { vec![v.field("x")] } else { Vec::new() })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = self.me.lock().unwrap().unwrap();
        match tag(v, &a).as_str() {
            "r" => {
                cx.follow(op, vec![v.str("s")]);
                cx.restart();
                cx.follow(op, vec![v.str("f")]);
                cx.follow(op, vec![v.str("s")]);
                Ok(ValueId::UNIT)
            }
            "f" => {
                cx.define(v.field("x"), v.str("from f"));
                Ok(ValueId::UNIT)
            }
            _ => {
                let x = cx.use_field(v.field("x"))?.and_then(|p| v.as_str(p));
                self.saw.lock().unwrap().push(x);
                Ok(ValueId::UNIT)
            }
        }
    }
}

#[test]
fn a_restarted_run_drops_its_first_pass_follows() {
    let ops = Arc::new(Restarter { me: Mutex::new(None), saw: Mutex::new(Vec::new()) });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    *ops.me.lock().unwrap() = Some(op);
    let u = eng.new_unit();
    let r = eng.values().str("r");
    eng.push(u, op, vec![r]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(*ops.saw.lock().unwrap(), vec![Some("from f".to_string())], "one S, after F");
}

/// An edit that makes a parent place the same follows in another order
/// moves them: `A [1]` follows `S`, `F`; `A [2]` follows `F`, `S`.
struct Swapper {
    me: Mutex<Option<OpId>>,
    saw: Mutex<Vec<Option<String>>>,
}

impl Operation for Swapper {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "f" { vec![(v.field("x"), Locality::Local)] } else { Vec::new() })
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(if tag(v, a) == "s" { vec![v.field("x")] } else { Vec::new() })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = self.me.lock().unwrap().unwrap();
        match tag(v, &a).as_str() {
            "a" => {
                let (f, s) = (vec![v.str("f")], vec![v.str("s")]);
                if v.as_int(a[1]) == Some(1) {
                    cx.follow(op, s);
                    cx.follow(op, f);
                } else {
                    cx.follow(op, f);
                    cx.follow(op, s);
                }
                Ok(ValueId::UNIT)
            }
            "f" => {
                cx.define(v.field("x"), v.str("from f"));
                Ok(ValueId::UNIT)
            }
            _ => {
                let x = cx.use_field(v.field("x"))?.and_then(|p| v.as_str(p));
                self.saw.lock().unwrap().push(x);
                Ok(ValueId::UNIT)
            }
        }
    }
}

#[test]
fn an_edit_that_reorders_kept_follows_moves_them() {
    let ops = Arc::new(Swapper { me: Mutex::new(None), saw: Mutex::new(Vec::new()) });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    *ops.me.lock().unwrap() = Some(op);
    let u = eng.new_unit();
    let (a, one) = (eng.values().str("a"), eng.values().int(1));
    let an = eng.push(u, op, vec![a, one]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(ops.saw.lock().unwrap().last(), Some(&None), "S before F reads nothing");
    let two = eng.values().int(2);
    eng.replace(an, op, vec![a, two]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(ops.saw.lock().unwrap().last(), Some(&Some("from f".to_string())), "S after F now");
}

/// `A [v]` defines `y = v`, follows `F` (fixed args) then `S`; `F` follows
/// `P`; `S` reads `y` and `x`, follows `G`; `G` follows `Q`. An edit of `A`
/// re-runs `S` (its `y` changed) while `F` and `P` stay: `P` stays before
/// `S` and `S` still reads `P`'s `x`.
struct Kept {
    me: Mutex<Option<OpId>>,
    saw: Mutex<Vec<(String, String)>>,
}

impl Operation for Kept {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "a" => vec![(v.field("x"), Locality::Local), (v.field("y"), Locality::Local)],
            "p" | "s" | "q" => vec![(v.field("x"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(match tag(v, a).as_str() {
            "s" => vec![v.field("x"), v.field("y")],
            "p" | "q" => vec![v.field("x")],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = self.me.lock().unwrap().unwrap();
        let t = tag(v, &a);
        let x = v.field("x");
        match t.as_str() {
            "a" => {
                cx.define(x, v.str("a"));
                cx.define(v.field("y"), a[1]);
                cx.follow(op, vec![v.str("f")]);
                cx.follow(op, vec![v.str("s")]);
                Ok(ValueId::UNIT)
            }
            "f" => {
                cx.follow(op, vec![v.str("p")]);
                Ok(ValueId::UNIT)
            }
            "g" => {
                cx.follow(op, vec![v.str("q")]);
                Ok(ValueId::UNIT)
            }
            _ => {
                if t == "s" {
                    let _ = cx.use_field(v.field("y"))?;
                }
                let prev = cx.use_field(x)?.and_then(|p| v.as_str(p)).unwrap_or_default();
                self.saw.lock().unwrap().push((t.clone(), prev.clone()));
                cx.define(x, v.str(&format!("{prev}{t}")));
                if t == "s" {
                    cx.follow(op, vec![v.str("g")]);
                }
                Ok(ValueId::UNIT)
            }
        }
    }
}

#[test]
fn kept_follows_of_a_kept_follow_stay_before_a_rerun_sibling() {
    let ops = Arc::new(Kept { me: Mutex::new(None), saw: Mutex::new(Vec::new()) });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    *ops.me.lock().unwrap() = Some(op);
    let u = eng.new_unit();
    let (a, one) = (eng.values().str("a"), eng.values().int(1));
    let an = eng.push(u, op, vec![a, one]);
    eng.evaluate_sequential().unwrap();
    ops.saw.lock().unwrap().clear();
    let two = eng.values().int(2);
    eng.replace(an, op, vec![a, two]);
    eng.evaluate_sequential().unwrap();
    let saw = ops.saw.lock().unwrap().clone();
    assert!(saw.contains(&("s".to_string(), "ap".to_string())), "S reads P's x after the edit: {saw:?}");
}

/// A new follow placed after a kept one goes after the kept one's own
/// follows: `A [v]` follows `F` (fixed) then `S [v]` (new per `v`); `F`
/// follows `P`, which defines `x`; `S` reads `x`.
struct NewAfterKept {
    me: Mutex<Option<OpId>>,
    saw: Mutex<Vec<Option<String>>>,
}

impl Operation for NewAfterKept {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "p" { vec![(v.field("x"), Locality::Local)] } else { Vec::new() })
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(if tag(v, a) == "s" { vec![v.field("x")] } else { Vec::new() })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = self.me.lock().unwrap().unwrap();
        match tag(v, &a).as_str() {
            "a" => {
                cx.follow(op, vec![v.str("f")]);
                cx.follow(op, vec![v.str("s"), a[1]]);
                Ok(ValueId::UNIT)
            }
            "f" => {
                cx.follow(op, vec![v.str("p")]);
                Ok(ValueId::UNIT)
            }
            "p" => {
                cx.define(v.field("x"), v.str("from p"));
                Ok(ValueId::UNIT)
            }
            _ => {
                let x = cx.use_field(v.field("x"))?.and_then(|p| v.as_str(p));
                self.saw.lock().unwrap().push(x);
                Ok(ValueId::UNIT)
            }
        }
    }
}

#[test]
fn a_new_follow_after_a_kept_one_goes_after_its_follows() {
    let ops = Arc::new(NewAfterKept { me: Mutex::new(None), saw: Mutex::new(Vec::new()) });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    *ops.me.lock().unwrap() = Some(op);
    let u = eng.new_unit();
    let (a, one) = (eng.values().str("a"), eng.values().int(1));
    let an = eng.push(u, op, vec![a, one]);
    eng.evaluate_sequential().unwrap();
    let two = eng.values().int(2);
    eng.replace(an, op, vec![a, two]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(ops.saw.lock().unwrap().last(), Some(&Some("from p".to_string())), "new S after P");
}
