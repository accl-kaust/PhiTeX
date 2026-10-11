//! Engine tests, written against the example language in `examples/common`.

#[path = "../examples/common/mod.rs"]
mod common;

use common::interp::interpret;
use common::{Program, SECTION, Threads, generate, generate_with};
use defuse::{Engine, EvalError, Greedy, RandomScheduler, Report, Rng, Sequential, Speculation};

fn lines(src: &[&str]) -> Vec<String> {
    src.iter().map(|s| (*s).to_string()).collect()
}

fn load(src: &[&str]) -> (Engine, Program, Report) {
    let (mut eng, prog) = Program::load(lines(src));
    let r = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(
        prog.result(&eng),
        reference(&prog.lines),
        "engine != interpreter: {src:?}"
    );
    (eng, prog, r)
}

/// The sequential interpreter's result: the reference.
fn reference(lines: &[String]) -> Vec<u64> {
    interpret(lines).expect("interpreter")
}

fn out(eng: &Engine, prog: &Program, i: usize) -> String {
    prog.render(eng)[i].clone()
}

fn eval(eng: &mut Engine, prog: &Program) -> Report {
    let r = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(
        prog.result(eng),
        reference(&prog.lines),
        "incremental != interpreter: {:?}",
        prog.lines
    );
    r
}

// ---------------------------------------------------------------- binding

#[test]
fn reaching_definitions_across_scopes_and_globals() {
    let (eng, prog, _) = load(&[
        "x = 1",            // 0
        "begin",            // 1
        "x = 2",            // 2
        "a = x",            // 3
        "begin",            // 4
        "global g = 3",     // 5
        "end",              // 6
        "b = g",            // 7
        "end",              // 8
        "c = x",            // 9: the block's x does not reach
        "d = g",            // 10: the global does
        "{ y = 1  w = y }", // 11
        "e = y",            // 12
    ]);
    assert_eq!(out(&eng, &prog, 3), "2");
    assert_eq!(out(&eng, &prog, 7), "3");
    assert_eq!(out(&eng, &prog, 9), "1");
    assert_eq!(out(&eng, &prog, 10), "3");
    assert_eq!(out(&eng, &prog, 12), "()");
    assert_eq!(
        eng.reaching_def(prog.nodes[9], eng.values().field("x")),
        Some(prog.nodes[0])
    );
}

#[test]
fn unmatched_close_is_ignored() {
    let (eng, prog, _) = load(&["x = 1", "end", "y = x"]);
    assert_eq!(out(&eng, &prog, 2), "1");
}

#[test]
fn rebinding_on_insert_and_remove() {
    let (mut eng, mut prog, _) = load(&["x = 1", "y = x + 1", "z = x + 2", "w = 7"]);
    prog.insert_line(&mut eng, 1, "x = 10".into());
    let r = eval(&mut eng, &prog);
    assert_eq!(&prog.render(&eng)[..5], ["1", "10", "11", "12", "7"]);
    assert_eq!(r.rebinds, 2, "exactly the two uses after the new definition");
    assert_eq!(r.nodes_run, 3);
    prog.insert_line(&mut eng, 4, "x = 99".into());
    let r = eval(&mut eng, &prog);
    assert_eq!((r.rebinds, r.nodes_run), (0, 1));
    prog.remove_line(&mut eng, 1);
    let r = eval(&mut eng, &prog);
    assert_eq!(r.rebinds, 2);
    assert_eq!(r.freed, 1);
}

#[test]
fn early_cutoff() {
    let (mut eng, mut prog, _) = load(&["a = 1", "b = a * 0", "c = b + 1", "d = c + 1"]);
    prog.set_line(&mut eng, 0, "a = 2".into());
    let r = eval(&mut eng, &prog);
    assert_eq!((r.nodes_run, r.cutoffs), (2, 1), "a and b run; b's value is unchanged");
}

#[test]
fn layout_reads_only_some_fields() {
    let (mut eng, mut prog, _) = load(&["p = {x: 1, w: 2, color: 3}", "r = layout(p)", "s = r + 1"]);
    prog.set_line(&mut eng, 0, "p = {x: 1, w: 2, color: 4}".into());
    assert_eq!(eval(&mut eng, &prog).nodes_run, 1);
    prog.set_line(&mut eng, 0, "p = {x: 1, w: 5, color: 4}".into());
    assert_eq!(eval(&mut eng, &prog).nodes_run, 3);
}

#[test]
fn pending_definition_sets() {
    let (mut eng, mut prog, _) = load(&["x = 1", "eval \"x = \" + \"2\"", "y = x", "z = 4"]);
    assert_eq!(&prog.render(&eng)[..4], ["1", "()", "2", "4"]);
    prog.set_line(&mut eng, 1, "eval \"q = 3\"".into());
    let r = eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 2), "1");
    assert!(r.rebinds >= 1);
}

#[test]
fn bounded_do_and_eval() {
    let src = [
        "proc p(a) { global g = a }",
        "proc q(a) { t = a }",
        "x = 1",
        "do q(1)",
        "eval \"w = 2\"",
        "y = x",
        "do p(5)",
        "z = g",
    ];
    let (mut eng, mut prog, r) = load(&src);
    assert_eq!(out(&eng, &prog, 5), "1");
    assert_eq!(out(&eng, &prog, 7), "5");
    eprintln!("bounded: sets {} defining {}", r.pending_sets, r.pending_sets_defining);
    assert!(r.pending_sets_defining <= r.pending_sets);
    // A procedure that now exports `x`: its `do` is re-read and binds `y`.
    prog.set_line(&mut eng, 1, "proc q(a) { global x = 7 }".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 5), "7");
    // A literal `eval` that defines `x` locally.
    prog.set_line(&mut eng, 4, "eval \"x = 9\"".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 5), "9");
    // Removing the procedure: `do q` defines nothing.
    prog.set_line(&mut eng, 4, "w = 0".into());
    prog.remove_line(&mut eng, 1);
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 4), "1");
}

#[test]
fn replaced_child_in_a_reused_slot_is_demanded() {
    // The eval's old child is freed and its replacement takes the same slot
    // in the same step: it must still be demanded and run.
    let (mut eng, mut prog, _) = load(&["g = 70", "eval \"u_\" + g + \" = 1\"", "page"]);
    prog.set_line(&mut eng, 0, "g = 71".into());
    eval(&mut eng, &prog);
}

#[test]
fn slot_reused_steps_later_is_a_new_node() {
    // A freed child's slot is taken by nodes created in later evaluations;
    // references to the old node (generation-checked) never alias them.
    let (mut eng, mut prog, _) = load(&["g = 70", "eval \"u_\" + g + \" = 1\"", "page"]);
    prog.set_line(&mut eng, 0, "g = 71".into());
    eval(&mut eng, &prog);
    prog.insert_line(&mut eng, 1, "h = 1".into());
    eval(&mut eng, &prog);
    prog.insert_line(&mut eng, 2, "eval \"v_\" + h + \" = u_71 + 1\"".into());
    eval(&mut eng, &prog);
    for (i, t) in [(0, "g = 72"), (1, "h = 2"), (0, "g = 71"), (1, "h = 3")] {
        prog.set_line(&mut eng, i, t.into());
        eval(&mut eng, &prog);
    }
    eng.collect_garbage();
    prog.set_line(&mut eng, 0, "g = 73".into());
    eval(&mut eng, &prog);
}

// ------------------------------------------------------------ scope kinds

#[test]
fn implicit_scope_opened_by_a_starter_and_ended_by_a_closer() {
    let (mut eng, mut prog, _) = load(&[
        "scope doc",      // 0
        "x = 1",          // 1
        "item a = x + 1", // 2: opens a list scope before itself
        "item b = a + 1", // 3: directly inside the list: no new scope
        "k = kind()",     // 4
        "para",           // 5: ends the list
        "c = b",          // 6: b was local to the list
        "k2 = kind()",    // 7
        "end",            // 8
        "d = x",          // 9
    ]);
    let r = prog.render(&eng);
    assert_eq!(
        r[..10],
        ["()", "1", "2", "3", "\"list\"", "()", "()", "\"doc\"", "()", "()"]
    );
    // A plain line becomes a starter and back.
    prog.set_line(&mut eng, 3, "b = a + 1".into());
    eval(&mut eng, &prog);
    prog.set_line(&mut eng, 2, "a = x + 1".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 4), "\"doc\"");
    assert_eq!(out(&eng, &prog, 6), "3", "no list scope: b is visible");
    prog.set_line(&mut eng, 2, "item a = x + 1".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 4), "\"list\"");
    prog.set_line(&mut eng, 3, "item b = a + 1".into());
    eval(&mut eng, &prog);
    // Removing the closer: the list runs to the end of the doc scope.
    prog.remove_line(&mut eng, 5);
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 5), "3");
    assert_eq!(out(&eng, &prog, 6), "\"list\"");
}

#[test]
fn closing_the_enclosing_scope_closes_the_implicit_one() {
    let (mut eng, mut prog, _) = load(&["scope doc", "item a = 1", "end", "k = kind()", "z = a"]);
    assert_eq!(prog.render(&eng)[3..5], ["\"none\"", "()"]);
    prog.insert_line(&mut eng, 0, "scope doc".into());
    prog.insert_line(&mut eng, 5, "end".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 4), "\"doc\"", "the inner end closed list and doc");
}

#[test]
fn kind_read_binds_across_an_unfold() {
    let (mut eng, mut prog, _) = load(&["scope doc", "eval \"k = kind()\"", "j = k", "end"]);
    assert_eq!(out(&eng, &prog, 2), "\"doc\"");
    prog.set_line(&mut eng, 0, "scope memo".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 2), "\"memo\"");
    prog.set_line(&mut eng, 0, "begin".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 2), "\"none\"");
}

// ---------------------------------------------------------- contributions

#[test]
fn prev_of_the_first_contributor_is_none() {
    let (eng, prog, _) = load(&["p = prev()", "add 1", "q = prev()", "add 2", "r = before()"]);
    assert_eq!(prog.render(&eng)[..5], ["\"none\"", "1", "1", "2", "[1 2]"]);
}

#[test]
fn contributions_are_scoped_and_removals_seen_only_later() {
    let (mut eng, mut prog, _) = load(&[
        "add 1",        // 0
        "add 2",        // 1
        "a = before()", // 2
        "take",         // 3: takes 2
        "b = before()", // 4
        "begin",        // 5
        "c = prev()",   // 6: a new scope starts empty
        "add 7",        // 7
        "end",          // 8
        "replace 5",    // 9: takes 1, puts 5
        "d = before()", // 10
        "e = prev()",   // 11
    ]);
    let r = prog.render(&eng);
    assert_eq!(r[2], "[1 2]");
    assert_eq!(r[3], "2");
    assert_eq!(r[4], "[1]");
    assert_eq!(r[6], "\"none\"");
    assert_eq!(r[9], "1");
    assert_eq!(r[10], "[5]");
    assert_eq!(r[11], "5");
    // Removing the take: earlier readers keep their value, later ones see 2.
    prog.remove_line(&mut eng, 3);
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 2), "[1 2]");
    assert_eq!(out(&eng, &prog, 3), "[1 2]");
    assert_eq!(out(&eng, &prog, 8), "2");
    prog.insert_line(&mut eng, 0, "take".into());
    eval(&mut eng, &prog);
    prog.set_line(&mut eng, 6, "x = 1".into());
    eval(&mut eng, &prog);
    prog.set_line(&mut eng, 8, "end".into());
    eval(&mut eng, &prog);
}

#[test]
fn a_middle_contribution_edit_reruns_only_its_readers() {
    let mut src = vec!["add 0".to_string()];
    for i in 1..40 {
        src.push(format!("add {i}"));
        src.push(format!("p{i} = prev()"));
        // A derived field: an ordinary definition from the contribution.
        src.push(format!("n{i} = p{i} * 0"));
    }
    let src: Vec<&str> = src.iter().map(String::as_str).collect();
    let (mut eng, mut prog, _) = load(&src);
    // `add 20` is line 58; its reader p20 is line 59.
    prog.set_line(&mut eng, 58, "add 99".into());
    let r = eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 59), "99");
    // The contributor, its prev-reader, and the derived field (which cuts
    // off: still 0).
    println!("middle contribution edit: {r:?}");
    assert!(r.nodes_run <= 3, "{r:?}");
    assert!(r.cutoffs >= 1, "{r:?}");
}

#[test]
fn random_contribution_edits_match_the_interpreter() {
    let pool = [
        "add 1",
        "add 2",
        "add x",
        "take",
        "replace 9",
        "x = prev()",
        "y = before()",
        "begin",
        "end",
        "x = 3",
        "eval \"add 4\"",
        "if x { add 5 } else { take }",
        "{ add 6  z = prev() }",
    ];
    for seed in 1..=4u64 {
        let mut s: u64 = 0x9e37_79b9_7f4a_7c15 ^ seed;
        let mut next = |m: usize| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            usize::try_from(s % m as u64).unwrap()
        };
        let src: Vec<&str> = (0..30).map(|_| pool[next(pool.len())]).collect();
        let (mut eng, mut prog, _) = load(&src);
        for _ in 0..400 {
            let i = next(prog.lines.len() + 1);
            match next(3) {
                0 if i < prog.lines.len() => prog.remove_line(&mut eng, i),
                1 if i < prog.lines.len() => prog.set_line(&mut eng, i, pool[next(pool.len())].into()),
                _ => prog.insert_line(&mut eng, i, pool[next(pool.len())].into()),
            }
            eval(&mut eng, &prog);
        }
    }
}

#[test]
fn freeing() {
    let (mut eng, mut prog, _) = load(&["c = 1", "if c { a = 1  b = 2 } else { d = 3 }"]);
    let before = eng.live_nodes();
    prog.set_line(&mut eng, 0, "c = 0".into());
    let r = eval(&mut eng, &prog);
    assert_eq!(r.freed, 2);
    assert_eq!(eng.live_nodes(), before - 1);
    let (mut eng, mut prog, _) = load(&["f = fn(a) { t = a * 2  return t + 1 }", "y = f(3)", "w = f(4)"]);
    let live = eng.live_nodes();
    prog.set_line(&mut eng, 2, "w = 9".into());
    eval(&mut eng, &prog);
    // Unreferenced applications go at the next collection (amortized after
    // builds), or when asked.
    assert_eq!(eng.collect_garbage(), 3, "f(4) and its body");
    assert_eq!(eng.live_nodes(), live - 3);
    assert_eq!(out(&eng, &prog, 1), "7");
}

// ------------------------------------------------------------------ claims

#[test]
fn claim_fixed_count_and_edit_inside() {
    let (mut eng, mut prog, _) = load(&["sumnext s 2", "3", "4", "t = s + 1"]);
    assert_eq!(out(&eng, &prog, 0), "7");
    assert_eq!(out(&eng, &prog, 1), "-", "claimed lines do not run on their own");
    assert_eq!(out(&eng, &prog, 3), "8");
    assert_eq!(eng.claimed(prog.nodes[0]).len(), 2);
    prog.set_line(&mut eng, 2, "5".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 3), "9");
    // A line inserted inside the range shifts it.
    prog.insert_line(&mut eng, 1, "10".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 0), "13");
    assert_eq!(out(&eng, &prog, 3), "5", "the last line is released");
}

#[test]
fn claim_up_to_delimiter_and_delimiter_edits() {
    let (mut eng, mut prog, _) = load(&["listto l", "1", "2", "endlist", "z = 9", "y = z"]);
    assert_eq!(out(&eng, &prog, 0), "[1 2]");
    assert_eq!(out(&eng, &prog, 5), "9");
    // Delete the delimiter: the claim runs to the end of the unit.
    prog.set_line(&mut eng, 3, "3".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 0), "[1 2 3]");
    assert_eq!(out(&eng, &prog, 5), "-");
    // Put a delimiter earlier.
    prog.set_line(&mut eng, 2, "endlist".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 0), "[1]");
    assert_eq!(out(&eng, &prog, 3), "3");
}

#[test]
fn claim_with_pending_delimiter() {
    let (mut eng, mut prog, _) = load(&[
        "stop = \"b\"",
        "listuntil l stop",
        "1",
        "mark a",
        "2",
        "mark b",
        "3",
        "k = 4",
    ]);
    assert_eq!(out(&eng, &prog, 1), "[1 2]");
    assert_eq!(out(&eng, &prog, 6), "3");
    prog.set_line(&mut eng, 0, "stop = \"a\"".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 1), "[1]");
    assert_eq!(out(&eng, &prog, 4), "2", "released");
}

// ----------------------------------------------------------------- scopes

#[test]
fn dynamic_scopes() {
    let (mut eng, mut prog, _) = load(&[
        "begin",        // 0
        "y = 5",        // 1
        "global g = 1", // 2
        "eval \"end\"", // 3: the close comes from unfolding
        "d = y",        // 4: y no longer reaches
        "e = g",        // 5: the global does
    ]);
    assert_eq!(out(&eng, &prog, 4), "()");
    assert_eq!(out(&eng, &prog, 5), "1");
    // Unfold something else: the scope stays open.
    prog.set_line(&mut eng, 3, "eval \"q = 0\"".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 4), "5");
}

#[test]
fn moving_a_close() {
    let (mut eng, mut prog, _) = load(&["x = 0", "begin", "x = 1", "end", "y = x", "z = x + 1"]);
    assert_eq!(out(&eng, &prog, 4), "0");
    prog.remove_line(&mut eng, 3);
    prog.insert_line(&mut eng, 4, "end".into());
    let r = eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 3), "1", "y is now inside the block");
    assert_eq!(out(&eng, &prog, 5), "1");
    assert!(r.scope_region <= 4, "only the moved region was re-bound: {r:?}");
}

// --------------------------------------------------------------- unfolding

#[test]
fn unfolding_binds_at_the_call_site() {
    let (mut eng, mut prog, _) = load(&[
        "proc addk(a) { global s = a + k }", // 0
        "k = 1",                             // 1
        "do addk(1)",                        // 2
        "v1 = s",                            // 3
        "k = 10",                            // 4
        "do addk(1)",                        // 5
        "v2 = s",                            // 6
    ]);
    assert_eq!(out(&eng, &prog, 3), "2");
    assert_eq!(out(&eng, &prog, 6), "11", "same text, its own binding");
    let a = eng.children(prog.nodes[2]);
    let b = eng.children(prog.nodes[5]);
    assert_eq!(a.len(), b.len());
    assert!(a.iter().zip(&b).all(|(x, y)| x != y), "each call site has its own copy");
    prog.set_line(&mut eng, 1, "k = 2".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 3), "3");
    assert_eq!(out(&eng, &prog, 6), "11");
}

#[test]
fn global_from_unfolded_text_reaches_later_siblings() {
    let (eng, prog, _) = load(&[
        "proc setg(a) { global gg = a * 2  loc = 1 }",
        "do setg(5)",
        "u = gg",
        "w = loc",
    ]);
    assert_eq!(out(&eng, &prog, 2), "10");
    assert_eq!(out(&eng, &prog, 3), "()", "the body's locals stay in its scope");
}

#[test]
fn meaning_is_a_field() {
    let (mut eng, mut prog, _) = load(&["meaning put = \"set\"", "~put x 5", "y = x", "z = 1"]);
    assert_eq!(out(&eng, &prog, 2), "5");
    prog.set_line(&mut eng, 0, "meaning put = \"double\"".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 2), "10");
    prog.set_line(&mut eng, 0, "meaning put = \"show\"".into());
    let r = eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 2), "()", "the definition set changed: y re-binds");
    assert!(r.rebinds >= 1);
    // Only uses of the target wait on the pending set.
    let (_, _, r) = load(&["meaning put = \"set\"", "g = 1", "~put x 5", "h = g"]);
    assert_eq!(r.pending_waits, 0, "h does not wait on ~put: {r:?}");
}

// ------------------------------------------------------------ accumulators

#[test]
fn accumulator_segments() {
    let (mut eng, mut prog, _) = load(&["emit 1", "emit 2", "page", "emit 3", "page"]);
    assert_eq!(out(&eng, &prog, 2), "[2 3]");
    assert_eq!(out(&eng, &prog, 4), "[1 3]");
    assert_eq!(prog.render(&eng)[5], "<1 2 3>");
    prog.set_line(&mut eng, 3, "emit 4".into());
    let r = eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 4), "[1 4]");
    assert_eq!(r.nodes_run, 2, "the emit and only the page whose range changed: {r:?}");
    prog.insert_line(&mut eng, 1, "page".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 1), "[1 1]");
    assert_eq!(out(&eng, &prog, 3), "[1 2]");
}

#[test]
fn sequences_are_canonical() {
    let eng = Engine::new();
    let v = eng.values();
    let items: Vec<_> = [1, 1, 2, 1, 1, 1, 3].iter().map(|&i| v.int(i)).collect();
    let mut left = defuse::ValueId::NIL;
    for &x in &items {
        left = v.seq_concat(left, v.seq_unit(x));
    }
    let mut right = defuse::ValueId::NIL;
    for &x in items.iter().rev() {
        right = v.seq_concat(v.seq_unit(x), right);
    }
    let a = v.seq_concat(
        v.seq_concat(v.seq_unit(items[0]), v.seq_unit(items[1])),
        v.seq_unit(items[2]),
    );
    let b = v.seq_concat(
        v.seq_unit(items[3]),
        v.seq_concat(v.seq_unit(items[4]), v.seq_unit(items[5])),
    );
    let mid = v.seq_concat(v.seq_concat(a, b), v.seq_unit(items[6]));
    assert_eq!(left, right);
    assert_eq!(left, mid);
    assert_eq!(v.seq_to_vec(left), items);
}

// --------------------------------------------------------------- loop-back

#[test]
fn fixpoint_convergence_and_errors() {
    let (eng, prog, r) = load(&["loop later = 0", "early = later + 1", "later = 5 * 2", "print early"]);
    assert_eq!(out(&eng, &prog, 3), "11");
    assert_eq!(r.rounds, 2);
    let (mut eng, _p) = Program::load(lines(&["loop n = 0", "n = n + 1"]));
    eng.set_max_rounds(10);
    assert_eq!(eng.evaluate_sequential(), Err(EvalError::NonConvergence { rounds: 10 }));
    let (mut eng, _p) = Program::load(lines(&["loop n = 0", "n = 1", "n = 2"]));
    assert!(matches!(
        eng.evaluate_sequential(),
        Err(EvalError::ConflictingPublication { .. })
    ));
}

#[test]
fn non_unique_fixpoint_matches_cold_after_edits() {
    let (mut eng, mut prog, _) = load(&["loop a = 0", "b = 5", "a = b", "c = a"]);
    assert_eq!(out(&eng, &prog, 3), "5");
    // Every value is now a fixpoint; cold evaluation stays at 0.
    prog.set_line(&mut eng, 2, "a = a".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 3), "0");
    prog.set_line(&mut eng, 2, "a = b".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 3), "5");
}

#[test]
fn cycles_are_reported_and_recoverable() {
    let (mut eng, mut prog) = Program::load(lines(&["f = fn(g) { return g(g) }", "y = f(f)", "z = 1"]));
    let r = eng.evaluate_sequential();
    assert!(matches!(r, Err(EvalError::Cycle(_))), "{r:?}");
    prog.set_line(&mut eng, 1, "y = 2".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 1), "2");
}

// ------------------------------------------------- schedules, speculation

type Eval = Box<dyn FnMut(&mut Engine) -> Result<Report, EvalError>>;

fn schedules(seed: u64) -> Vec<(String, Eval)> {
    let mut v: Vec<(String, Eval)> = Vec::new();
    v.push(("greedy".into(), Box::new(|e: &mut Engine| e.evaluate_sequential())));
    v.push((
        "greedy+spec".into(),
        Box::new(|e: &mut Engine| e.evaluate(&mut Greedy::speculative(Speculation::All, 64), &mut Sequential)),
    ));
    v.push((
        "threads4+spec".into(),
        Box::new(|e: &mut Engine| e.evaluate(&mut Greedy::speculative(Speculation::Predicted, 1024), &mut Threads(4))),
    ));
    for (k, spec) in [(1, false), (2, true), (3, true)] {
        let mut s = RandomScheduler::new(seed * 31 + k, spec);
        v.push((
            format!("random{k} spec={spec}"),
            Box::new(move |e: &mut Engine| e.evaluate(&mut s, &mut Sequential)),
        ));
    }
    v
}

fn random_edit(rng: &mut Rng, eng: &mut Engine, prog: &mut Program) {
    let n = prog.lines.len();
    let i = 8 + rng.below(n - 12);
    match rng.below(5) {
        0 => {
            let mut l = prog.lines[i].clone();
            if let Some(p) = l.rfind(|c: char| c.is_ascii_digit()) {
                l.insert(p + 1, char::from(b'0' + u8::try_from(rng.below(10)).expect("digit")));
            }
            prog.set_line(eng, i, l);
        }
        1 => {
            let mut l = prog.lines[i].clone();
            if let Some(p) = l.rfind(|c: char| c.is_ascii_digit()) {
                l.remove(p);
            }
            prog.set_line(eng, i, l);
        }
        2 => {
            if let Some(name) = prog.lines[i].strip_prefix('x').and_then(|r| r.split(' ').next()) {
                let k = rng.below(9);
                prog.insert_line(eng, i + 1, format!("x{name} = {k}"));
            }
        }
        3 => {
            if prog.lines[i] == "end" {
                prog.remove_line(eng, i);
                prog.insert_line(eng, i - 1, "end".into());
            } else {
                prog.remove_line(eng, i);
            }
        }
        _ => {
            let l = if rng.chance(1, 2) {
                "meaning put = \"double\""
            } else {
                "begin"
            };
            prog.insert_line(eng, i, l.into());
        }
    }
}

/// Per schedule: results, canonical counters and child ids after each step.
type Run = (Vec<Vec<u64>>, Vec<[u64; 7]>, Vec<Vec<defuse::NodeId>>);

fn canonical(r: &Report) -> [u64; 7] {
    [
        r.nodes_run,
        r.cutoffs,
        r.rebinds,
        r.created,
        r.freed,
        r.rounds,
        r.restarts,
    ]
}

#[test]
fn schedule_independence_ids_and_speculation() {
    let mut spec = Report::default();
    let seeds = std::env::var("DEFUSE_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    for seed in 0..seeds {
        let src = generate(seed, 300);
        let mut runs: Vec<Run> = Vec::new();
        for (name, mut eval) in schedules(seed) {
            let (mut eng, mut prog) = Program::load(src.clone());
            let (mut results, mut reports, mut ids) = (Vec::new(), Vec::new(), Vec::new());
            let r = eval(&mut eng).unwrap_or_else(|e| panic!("{name}: {e}"));
            add(&mut spec, &r);
            results.push(prog.result(&eng));
            reports.push(canonical(&r));
            let mut rng = Rng::new(seed + 1000);
            for step in 0..20 {
                random_edit(&mut rng, &mut eng, &mut prog);
                let r = eval(&mut eng).unwrap_or_else(|e| panic!("{name}: {e}"));
                add(&mut spec, &r);
                let got = prog.result(&eng);
                assert_eq!(
                    got,
                    reference(&prog.lines),
                    "{name} seed {seed} step {step}: incremental != interpreter"
                );
                results.push(got);
                reports.push(canonical(&r));
                ids.push(prog.nodes.iter().flat_map(|&n| eng.children(n)).collect());
            }
            runs.push((results, reports, ids));
        }
        for r in &runs[1..] {
            assert_eq!(r.0, runs[0].0, "seed {seed}: results differ between schedules");
            assert_eq!(r.2, runs[0].2, "seed {seed}: node ids differ between schedules");
            assert_eq!(
                r.1, runs[0].1,
                "seed {seed}: canonical counters differ between schedules"
            );
        }
    }
    eprintln!("{spec:?}");
    assert!(spec.speculations > 0, "{spec:?}");
    assert!(spec.confirmations > 0, "{spec:?}");
    assert!(spec.fuel_outs > 0, "{spec:?}");
}

fn add(t: &mut Report, r: &Report) {
    t.speculations += r.speculations;
    t.confirmations += r.confirmations;
    t.mispredictions += r.mispredictions;
    t.fuel_outs += r.fuel_outs;
}

#[test]
fn wrong_prediction_is_corrected() {
    let (mut eng, mut prog) = Program::load(lines(&["a = 1", "b = a + 1", "c = b + 1", "d = c + 1"]));
    eng.evaluate_sequential().unwrap();
    prog.set_line(&mut eng, 0, "a = 5".into());
    prog.set_line(&mut eng, 2, "c = b + 1 + 0".into());
    let r = eng
        .evaluate(&mut Greedy::speculative(Speculation::Predicted, 64), &mut Sequential)
        .unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(&prog.render(&eng)[..4], ["5", "6", "7", "8"]);
    assert!(r.mispredictions > 0, "{r:?}");
}

#[test]
fn wrong_shape_prediction_rolls_back() {
    let src = [
        "c = 1",
        "if c { global z = 1  q = 2 } else { global y = 2 }",
        "w = z",
        "v = y",
    ];
    let (mut eng, mut prog) = Program::load(lines(&src));
    eng.evaluate_sequential().unwrap();
    prog.set_line(&mut eng, 0, "c = 0".into());
    let r = eng
        .evaluate(&mut Greedy::speculative(Speculation::All, 64), &mut Sequential)
        .unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(prog.result(&eng), reference(&prog.lines));
    let (mut c, _) = Program::load(prog.lines.clone());
    c.evaluate_sequential().unwrap();
    assert_eq!(
        eng.live_nodes(),
        c.live_nodes(),
        "no provisional nodes left over: {r:?}"
    );
}

#[test]
fn speculation_off_on_random_identical_on_generated_programs() {
    for seed in 0..3 {
        let src = generate(seed + 50, 800);
        let mut res = Vec::new();
        for spec in [false, true] {
            let (mut e, p) = Program::load(src.clone());
            e.evaluate(&mut RandomScheduler::new(seed, spec), &mut Sequential)
                .unwrap_or_else(|e| panic!("{e:?}"));
            res.push(p.result(&e));
        }
        let (mut e, p) = Program::load(src.clone());
        e.evaluate(&mut Greedy::speculative(Speculation::All, 512), &mut Threads(3))
            .unwrap();
        res.push(p.result(&e));
        let (mut e, p) = Program::load(src);
        e.evaluate_sequential().unwrap();
        res.push(p.result(&e));
        assert!(res.windows(2).all(|w| w[0] == w[1]));
    }
}

/// The bound we claim: the cold critical path (longest chain of dependent
/// waits, as heights) is at most this, independent of program size, for the
/// generated programs without unbounded evaluated text (whose inherent
/// dependency depth is bounded by their section length: chains never run
/// from one section into the next, except through the header's names,
/// procedure names and the layout segments).
#[allow(clippy::cast_possible_truncation)]
const CRITICAL_PATH_BOUND: u32 = SECTION as u32;

#[test]
fn critical_path_is_bounded() {
    let paths = |unbounded: bool| -> Vec<u32> {
        [1_000, 4_000, 16_000, 64_000]
            .into_iter()
            .map(|n| {
                let (mut e, _) = Program::load(generate_with(7, n, unbounded));
                e.evaluate_sequential().unwrap();
                e.critical_path()
            })
            .collect()
    };
    let bounded = paths(false);
    eprintln!("critical paths, literal-prefix evals only: {bounded:?}");
    for &c in &bounded {
        assert!(
            c <= CRITICAL_PATH_BOUND,
            "critical path {c} > {CRITICAL_PATH_BOUND}: {bounded:?}"
        );
    }
    assert!(
        bounded[3] < 2 * bounded[0],
        "critical path grows with program size: {bounded:?}"
    );
    // With 1 in 10 evals starting with a computed piece (0.03% of lines),
    // each such eval may define any name, and its text depends on names
    // looked up across the previous one: a real chain, growing with the
    // number of them. Reported, not bounded.
    eprintln!("critical paths, with unbounded evals: {:?}", paths(true));
}

#[test]
fn generated_programs_match_the_interpreter() {
    for (seed, n) in [(1, 1_000), (2, 4_000), (3, 16_000)] {
        let (mut eng, prog) = Program::load(generate(seed, n));
        eng.evaluate(&mut Greedy::default(), &mut Threads(4)).unwrap();
        assert_eq!(prog.result(&eng), reference(&prog.lines), "seed {seed}, {n} lines");
    }
}

#[test]
fn record_sizes() {
    let (node, edge) = Engine::record_sizes();
    assert!(node <= 128, "node record {node} bytes");
    assert!(edge <= 24, "use edge {edge} bytes");
}

#[test]
fn probes_count_only_in_profile_builds() {
    defuse::obs::reset();
    let (_eng, _prog, _) = load(&["x = 1", "y = x + 1"]);
    let snap = defuse::obs::snapshot();
    let walks = snap.counters.iter().find(|c| c.0 == "walk").map_or(0, |c| c.1);
    if defuse::obs::enabled() {
        assert!(walks > 0, "{snap}");
        assert!(snap.spans.iter().any(|s| s.0 == "evaluate" && s.1 > 0), "{snap}");
    } else {
        assert!(snap.counters.is_empty() && snap.spans.is_empty());
        assert_eq!(defuse::obs::chrome_trace(), "{\"traceEvents\":[]}");
    }
}

#[test]
fn opener_replaced_by_a_definition_reaches_past_the_old_close() {
    // The new line's definition registers while the line still opened; the
    // re-binding must not leave readers after the old close bound past it.
    let (mut eng, mut prog, _) = load(&["begin", "x = 3", "end", "z = x"]);
    prog.set_line(&mut eng, 0, "x = 0".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 3), "3");
    prog.set_line(&mut eng, 0, "begin".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 3), "()");
}

#[test]
fn opener_turned_pending_closer_keeps_the_bindings_it_reflected() {
    // `begin` -> `end` (pending: items exist): the order keeps the opener's
    // bound effect until the closer decides, then re-binds from it.
    let (mut eng, mut prog, _) = load(&["begin", "item a = prev()", "eval \"add 4\"", "replace 9"]);
    prog.set_line(&mut eng, 0, "end".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 3), "4");
}

#[test]
fn a_pending_close_waits_readers_of_an_openers_marker() {
    let (eng, prog, _) = load(&["add 1", "item a = prev()", "add 1", "begin", "end", "y = before()"]);
    assert_eq!(out(&eng, &prog, 5), "[1 1]");
}

#[test]
fn a_redone_pending_close_leaves_depths_consistent() {
    // Re-binding while a decided close was back in work must not record
    // definitions at the depth without it.
    let (eng, prog, _) = load(&[
        "begin",
        "begin",
        "begin",
        "scope doc",
        "y = 2",
        "end",
        "begin",
        "item a = 1",
        "begin",
        "begin",
        "begin",
        "begin",
        "z = 1",
        "end",
        "x = 1",
        "w = x + 1",
    ]);
    assert_eq!(out(&eng, &prog, 15), "2");
}

// ------------------------------------------------------------------ folds

/// A fold over `n` elements of heights from `h`, then `extra` lines.
fn fold_program(n: usize, h: &dyn Fn(usize) -> i64, text: &str, extra: &[String]) -> Vec<String> {
    let mut src = vec![format!("fold \"{text}\"")];
    for i in 0..n {
        src.push(format!("add {}", h(i)));
    }
    src.extend(extra.iter().cloned());
    src
}

fn outputs(eng: &Engine, prog: &Program) -> Vec<String> {
    let acc = eng.acc_value(prog.unit, common::out_field(eng.values()));
    acc.map_or_else(Vec::new, |a| {
        eng.values()
            .seq_to_vec(a)
            .iter()
            .map(|&x| eng.values().render(x))
            .collect()
    })
}

/// Pseudo-random heights 1..=5.
fn fold_height(i: usize) -> i64 {
    1 + i64::try_from(((i as u64).wrapping_mul(2_654_435_761) >> 7) % 5).unwrap()
}

/// 1000 elements at capacity 63: 48 emitted outputs.
fn fold_thousand(text: &str) -> Vec<String> {
    let mut src = fold_program(1000, &fold_height, text, &[]);
    src.insert(0, "cap = 63".into());
    src
}

/// Line of element `i` in [`fold_thousand`].
const fn elem(i: usize) -> usize {
    i + 2
}

fn load_lines(src: &[String]) -> (Engine, Program, Report) {
    let src: Vec<&str> = src.iter().map(String::as_str).collect();
    load(&src)
}

#[test]
fn fold_over_a_thousand_elements_emits_48_outputs() {
    let (eng, prog, r) = load_lines(&fold_thousand("x = out"));
    let outs = outputs(&eng, &prog);
    assert_eq!(outs.len(), 48);
    assert_eq!(Report::by_op(&r.runs_by_op, "output"), 48);
    println!(
        "cold: outputs {} waits on stmt nodes {}",
        outs.len(),
        Report::by_op(&r.waits_by_op, "stmt")
    );
}

#[test]
fn a_fold_edit_recomputes_only_until_the_state_realigns() {
    let (mut eng, mut prog, _) = load_lines(&fold_thousand("x = out"));
    let before = outputs(&eng, &prog);
    // Element 500 one taller: output 25 grows, the state after it is the
    // same as before (the next output starts at the same element).
    prog.set_line(&mut eng, elem(500), format!("add {}", fold_height(500) + 1));
    let r = eval(&mut eng, &prog);
    let after = outputs(&eng, &prog);
    let changed: Vec<usize> = (0..48).filter(|&k| before[k] != after[k]).map(|k| k + 1).collect();
    assert_eq!(changed, [25]);
    let recomputed = Report::by_op(&r.runs_by_op, "output");
    println!(
        "+1 at 500: outputs recomputed {recomputed}, nodes run {}, cutoffs {}",
        r.nodes_run, r.cutoffs
    );
    assert_eq!(recomputed, 1);
    assert!(r.nodes_run < 40, "{r:?}");
    // One shorter: the state carried forward changes and the recompute runs
    // on until the outputs line up again (output 47).
    prog.set_line(&mut eng, elem(500), format!("add {}", fold_height(500) - 1));
    let r = eval(&mut eng, &prog);
    let after = outputs(&eng, &prog);
    let changed: Vec<usize> = (0..48).filter(|&k| before[k] != after[k]).map(|k| k + 1).collect();
    assert_eq!(changed.first(), Some(&25));
    assert_eq!(changed.last(), Some(&46));
    let recomputed = Report::by_op(&r.runs_by_op, "output");
    println!(
        "-1 at 500: outputs recomputed {recomputed}, nodes run {}, cutoffs {}",
        r.nodes_run, r.cutoffs
    );
    assert!(recomputed >= 22, "{r:?}");
    // A capacity change reaches every step.
    prog.set_line(&mut eng, 0, "cap = 64".into());
    let r = eval(&mut eng, &prog);
    println!(
        "cap 63 -> 64: outputs recomputed {}, nodes run {}",
        Report::by_op(&r.runs_by_op, "output"),
        r.nodes_run
    );
}

#[test]
fn an_output_prepends_elements_and_defines_a_field_read_later() {
    let text = "when k == 10 add 30  when k == 10 add 31  when k == 10 global g = 7";
    let mut src = fold_thousand(text);
    src[elem(600)] = "add g - 5".into();
    let (mut eng, mut prog, _) = load_lines(&src);
    let outs = outputs(&eng, &prog);
    println!("outputs with prepends: {}", outs.len());
    // The two prepended elements follow the element that overflowed
    // output 10 (3 high): 33, then 31 starts output 12.
    assert_eq!(outs[10], "33");
    // Element 600 read g = 7 from output 10's text: height 2.
    assert_eq!(out(&eng, &prog, elem(600)), "2");
    // Moving the prepend to output 12 re-runs from output 10 on.
    prog.set_line(
        &mut eng,
        1,
        "fold \"when k == 12 add 30  when k == 12 add 31  when k == 12 global g = 7\"".into(),
    );
    let r = eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, elem(600)), "2");
    println!(
        "prepend moved: outputs recomputed {}",
        Report::by_op(&r.runs_by_op, "output")
    );
    // Reading the state as of an element: the nearest step at or before it.
    prog.insert_line(&mut eng, elem(700), "s = fold.state".into());
    eval(&mut eng, &prog);
    assert!(out(&eng, &prog, elem(700)).starts_with('['));
}

#[test]
fn speculation_resting_on_a_freed_node_is_refuted() {
    // Removing the second fold frees the node a speculative `item` run
    // assumed about; that run must not stay unconfirmed forever.
    let src = [
        "cap = 12",
        "fold \"when k == 1 add 8  when k == 1 add 2\"",
        "add 7",
        "fold \"when k == 1 add 8  when k == 1 add 2\"",
        "add 7",
        "{ add 6  z = prev() }",
        "item a = prev()",
        "add 1",
    ];
    let (mut eng, mut prog) = Program::load(lines(&src));
    eng.evaluate(&mut Greedy::speculative(Speculation::All, 512), &mut Threads(3))
        .unwrap();
    prog.remove_line(&mut eng, 3);
    eng.evaluate(&mut Greedy::speculative(Speculation::All, 512), &mut Threads(3))
        .unwrap();
    assert_eq!(prog.result(&eng), reference(&prog.lines));
    assert_eq!(eng.check_invariants(), [] as [std::string::String; 0]);
}

#[test]
fn a_global_inside_a_skipped_scope_hands_its_users_on() {
    // The new output's global state follows a global defined inside the
    // `begin` scope the definition walk steps over.
    let (mut eng, mut prog, _) = load(&[
        "fold \"when k == 1 add 8  when k == 1 add 2\"",
        "cap = 12",
        "add 7",
        "add 3",
        "begin",
        "add 3",
        "add 7",
        "end",
        "s = fold.state",
    ]);
    prog.insert_line(&mut eng, 7, "add 7".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 9), "[7 3]");
}
