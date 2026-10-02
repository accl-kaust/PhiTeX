//! The runtime opened to a language that owns its state (`open.rs`):
//! re-entrant calls over a [`Store`], hits that apply their writes, reads
//! verified in order, backdated writes, and the trace.

use partex_ssa::stub::{Addr, Func, Stub, Val};
use partex_ssa::{Config, Found, Loc, MapStore, Runtime, Store, Value};

fn var(s: &str) -> Addr {
    Addr::Var(s.into())
}

/// `f(x) = { y := g(x) + read(a); write(b, y) }`, `g(x) = x + read(c)`,
/// both through the store the body gets back.
fn run(rt: &mut Runtime<Stub>, st: &mut MapStore<Stub>, x: i64) -> i64 {
    let body_g = |st: &mut MapStore<Stub>, rt: &mut Runtime<Stub>| {
        let c = st.get(&var("c"));
        rt.note_read(
            &Loc::State(var("c")),
            partex_ssa::value::version_opt(c.as_ref()),
        );
        Val::Int(x + c.map_or(0, |v| v.int()))
    };
    let r = rt.call(st, Func::Para, &[Val::Int(x)], |st, rt| {
        let y = rt.call(st, Func::Tokenize, &[Val::Int(x)], body_g).int();
        let a = st.get(&var("a"));
        rt.note_read(
            &Loc::State(var("a")),
            partex_ssa::value::version_opt(a.as_ref()),
        );
        let v = Val::Int(y + a.map_or(0, |v| v.int()));
        st.set(&var("b"), Some(v.clone()));
        rt.note_write(&var("b"));
        v
    });
    r.int()
}

#[test]
fn open_calls_hit_apply_and_miss() {
    let mut rt = Runtime::<Stub>::new(Config::default());
    rt.set_default_value(Val::Int(0));
    let mut st = MapStore::<Stub>::default();
    st.set(&var("a"), Some(Val::Int(10)));
    st.set(&var("c"), Some(Val::Int(100)));
    rt.open_trip(0);
    assert_eq!(run(&mut rt, &mut st, 1), 111);
    rt.close_trip();
    assert_eq!((rt.stats.hits, rt.stats.misses), (0, 2));

    // the same state: a hit, whose write is applied to a store where
    // `b` differs
    let mut st2 = MapStore::<Stub>::default();
    st2.set(&var("a"), Some(Val::Int(10)));
    st2.set(&var("c"), Some(Val::Int(100)));
    st2.set(&var("b"), Some(Val::Int(-1)));
    rt.open_trip(0);
    assert_eq!(run(&mut rt, &mut st2, 1), 111);
    rt.close_trip();
    assert_eq!(rt.stats.hits, 1);
    assert_eq!(st2.get(&var("b")).map(|v| v.int()), Some(111));

    // `a` changed: the outer call misses, the inner one hits
    st2.set(&var("a"), Some(Val::Int(20)));
    rt.open_trip(0);
    assert_eq!(run(&mut rt, &mut st2, 1), 121);
    rt.close_trip();
    assert_eq!((rt.stats.hits, rt.stats.misses), (2, 3));
    let text = rt.trace().to_text();
    assert!(
        text.contains("miss=@var:a") || text.contains("miss="),
        "{text}"
    );
    assert!(text.contains(" hit"), "{text}");
    assert!(partex_ssa::Trace::parse(&text).is_ok());
}

#[test]
fn probe_runs_the_body_quietly() {
    let mut rt = Runtime::<Stub>::new(Config::default());
    rt.set_default_value(Val::Int(0));
    let mut st = MapStore::<Stub>::default();
    for _ in 0..2 {
        rt.open_trip(0);
        let args = [Val::Int(7).version()];
        let found = rt.probe(&st, Func::Page, &args);
        let quiet = matches!(found, Found::Hit(_));
        rt.begin_quiet(Func::Page, args.to_vec(), quiet);
        let _ = run(&mut rt, &mut st, 7);
        let id = rt.end(&st, Val::Int(1));
        if let Found::Hit(old) = found {
            // re-recorded equal: interned to the same record
            assert_eq!(old, id);
        }
        rt.close_trip();
    }
    // the probe and the nested call inside the quiet body
    assert_eq!(rt.stats.hits, 2);
}

#[test]
fn backdated_write_keeps_the_version() {
    let mut st = MapStore::<Stub>::default();
    st.set(&var("a"), Some(Val::Int(1)));
    let v0 = st.version(&Loc::State(var("a")));
    st.set(&var("a"), Some(Val::Int(1)));
    assert_eq!(v0, st.version(&Loc::State(var("a"))));
}
