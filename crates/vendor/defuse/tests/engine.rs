//! Engine tests, written against the example language in `examples/common`.

#[path = "../examples/common/mod.rs"]
mod common;

use common::interp::{interpret, interpret_env};
use common::{Program, SECTION, Threads, env_values, generate, generate_with};
use defuse::{
    Cause, Engine, EvalError, Field, Greedy, Limits, NodeId, RandomScheduler, Report, Rng, Sequential, Settled,
    Speculation, ValueId, WaitOn,
};

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
fn refs_of(lines: &[String]) -> Vec<String> {
    let (mut e, p) = Program::load(lines.to_vec());
    e.evaluate_sequential().expect("cold");
    p.render(&e)
}

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

// ---------------------------------------------------------- stream claims

#[test]
fn a_stream_claim_ends_at_the_first_node_of_a_class_after_unfolding() {
    let (mut eng, mut prog, _) = load(&[
        "collect l until mark",
        "1",
        "eval \"2  3\"",
        "mark a",
        "4",
        "mark b",
        "z = l",
    ]);
    assert_eq!(out(&eng, &prog, 0), "[1 2 3]");
    assert_eq!(out(&eng, &prog, 4), "4", "claimed nodes still run");
    // The delimiter arrives by unfolding.
    prog.set_line(&mut eng, 3, "eval \"mark a\"".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 0), "[1 2 3]");
    // The first delimiter goes: the claim runs on to the next.
    prog.set_line(&mut eng, 3, "5".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 0), "[1 2 3 5 4]");
    assert_eq!(out(&eng, &prog, 6), "[1 2 3 5 4]");
    // Without any, it would take the reader of its own result: each
    // waits above the other, which the idle-round budget ends, naming the
    // claimer among the waiters.
    prog.set_line(&mut eng, 5, "6".into());
    let e = eng.evaluate_sequential().expect_err("a claim that feeds itself");
    match e {
        EvalError::NoProgress { waiters, .. } => {
            assert!(waiters.iter().any(|w| w.node == prog.nodes[0]), "{waiters:?}");
        }
        e => panic!("{e}"),
    }
}

#[test]
fn a_range_reader_re_anchors_when_its_previous_sibling_is_claimed_or_released() {
    // The last `page` reads the output since the previous `page`. A claim
    // growing over that page takes it out of the siblings (the reader
    // re-anchors on the unit's start); shrinking back returns it.
    let (mut eng, mut prog, _) = load(&["emit 1", "sumnext s 1", "0", "page", "end", "page"]);
    assert_eq!(out(&eng, &prog, 5), "[0 0]");
    prog.set_line(&mut eng, 1, "sumnext s 2".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 5), "[1 1]", "the claimed page is not an anchor");
    prog.set_line(&mut eng, 1, "sumnext s 1".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 5), "[0 0]", "the released page anchors again");
    prog.set_line(&mut eng, 1, "sumnext s 3".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 5), "[1 1]");
}

#[test]
fn a_stream_claim_follows_its_range_through_label_renumbering() {
    // Inserting at one spot halves the label gap each time, so the unit
    // renumbers its labels several times; the claim's recorded range is
    // kept by node, and an edit inside it, before or after a renumbering,
    // still runs the claimer.
    let (mut eng, mut prog, _) = load(&["collect l until mark", "1", "mark a", "2", "z = l"]);
    assert_eq!(out(&eng, &prog, 0), "[1]");
    for k in 0..70 {
        prog.insert_line(&mut eng, 2, format!("{}", k + 10));
        let r = eval(&mut eng, &prog);
        assert!(r.nodes_run >= 1, "insert {k}: the claimer runs");
    }
    assert!(out(&eng, &prog, 0).starts_with("[1 79 78"));
    // Renumberings outside the range: the delimiter and the range's end
    // move while the claimer does not run.
    let after = prog.lines.len() - 1;
    for k in 0..70 {
        prog.insert_line(&mut eng, after, format!("{}", k + 100));
        eval(&mut eng, &prog);
    }
    assert!(out(&eng, &prog, 0).starts_with("[1 79 78"));
    prog.set_line(&mut eng, 1, "5".into());
    eval(&mut eng, &prog);
    assert!(out(&eng, &prog, 0).starts_with("[5 79 78"));
    // The delimiter goes: the range runs on to the next delimiter.
    let mark = prog.lines.iter().position(|l| l == "mark a").unwrap();
    prog.set_line(&mut eng, mark, "6".into());
    let r = eng.evaluate_sequential();
    assert!(matches!(r, Err(EvalError::NoProgress { .. })), "{r:?}");
}

#[test]
fn a_stream_claim_predicate_reads_a_definition_inside_the_claim() {
    let (mut eng, mut prog, _) = load(&[
        "collect l until mark if go", // 0
        "1",                          // 1
        "mark a",                     // 2: go unset: goes on
        "go = true",                  // 3: inside the claim
        "2",                          // 4
        "mark a",                     // 5: the claim ends here
        "3",                          // 6
    ]);
    assert_eq!(out(&eng, &prog, 0), "[1 () true 2]");
    prog.set_line(&mut eng, 3, "go = false".into());
    let r = eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 0), "[1 () false 2 () 3]");
    println!("definition inside the claim changed: nodes run {}", r.nodes_run);
    // The definition comes by unfolding: the claim waits for exactly it.
    prog.set_line(&mut eng, 3, "eval \"go = true\"".into());
    let r = eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 0), "[1 () true 2]");
    println!(
        "definition by unfolding: waits on stmt nodes {}",
        Report::by_op(&r.waits_by_op, "stmt")
    );
}

#[test]
fn a_predicted_stream_claim_confirms_on_every_named_field() {
    // The claim ends at the `mark` where both facts hold; a change in
    // either one re-runs it, under every scheduler.
    let src = [
        "collect l until mark if go and ok guess 3",
        "go = true",
        "1",
        "mark a",
        "ok = true",
        "2",
        "mark a",
        "3",
    ];
    for sched in 0..3 {
        let (mut eng, mut prog) = Program::load(lines(&src));
        let run = |eng: &mut Engine| match sched {
            0 => eng.evaluate_sequential().map(|_| ()),
            1 => eng
                .evaluate(&mut Greedy::speculative(Speculation::Predicted, 64), &mut Sequential)
                .map(|_| ()),
            _ => eng
                .evaluate(&mut RandomScheduler::new(7, true), &mut Sequential)
                .map(|_| ()),
        };
        run(&mut eng).unwrap();
        assert_eq!(prog.result(&eng), reference(&prog.lines));
        assert_eq!(prog.render(&eng)[0], "[true 1 () true 2]");
        for (i, line, want) in [
            (4, "ok = false", "[true 1 () false 2 () 3]"),
            (1, "go = false", "[false 1 () false 2 () 3]"),
            (4, "ok = true", "[false 1 () true 2 () 3]"),
            (1, "go = true", "[true 1 () true 2]"),
        ] {
            prog.set_line(&mut eng, i, line.into());
            run(&mut eng).unwrap();
            assert_eq!(prog.result(&eng), reference(&prog.lines), "{line}");
            assert_eq!(prog.render(&eng)[0], want, "{line}");
        }
    }
}

#[test]
fn a_predicted_stream_claim_speculates_and_confirms() {
    for guess in [3, 2, 4] {
        // The claim ends before sibling 3 (`mark`); guesses off by one each
        // way only cost a re-run.
        let src = format!("collect l until mark guess {guess}");
        let lines = lines(&[&src, "a = 1", "b = a + 1", "c = b + 1", "mark a", "d = 9"]);
        let (mut eng, mut prog) = Program::load(lines);
        let mut sched = Greedy::speculative(Speculation::Predicted, 64);
        eng.evaluate(&mut sched, &mut Sequential).unwrap();
        assert_eq!(prog.result(&eng), reference(&prog.lines), "guess {guess}");
        assert_eq!(prog.render(&eng)[0], "[1 2 3]");
        // An edit upstream: the claimer may run before the chain settles.
        prog.set_line(&mut eng, 1, "a = 5".into());
        let r = eng.evaluate(&mut sched, &mut Sequential).unwrap();
        assert_eq!(prog.result(&eng), reference(&prog.lines), "guess {guess}");
        assert_eq!(prog.render(&eng)[0], "[5 6 7]");
        println!(
            "guess {guess}: speculations {} confirmations {} mispredictions {}",
            r.speculations, r.confirmations, r.mispredictions
        );
    }
}

#[test]
fn freeing() {
    let (mut eng, mut prog, _) = load(&["c = 1", "if c { a = 1  b = 2 } else { d = 3 }"]);
    let before = eng.live_nodes();
    prog.set_line(&mut eng, 0, "c = 0".into());
    let r = eval(&mut eng, &prog);
    // The branch's first statement keeps its place (re-texted from `a = 1`
    // to `d = 3`: children are named by operation and order); the second
    // is freed.
    assert_eq!((r.freed, r.created), (1, 0), "{r:?}");
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
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 3), "1", "y is now inside the block");
    assert_eq!(out(&eng, &prog, 5), "1");
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

/// Per schedule: results and child ids after each step (speculative
/// schedules may run more, so counters are not compared).
type Run = (Vec<Vec<u64>>, Vec<Vec<defuse::NodeId>>);

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
            let (mut results, mut ids) = (Vec::new(), Vec::new());
            let r = eval(&mut eng).unwrap_or_else(|e| panic!("{name}: {e}"));
            add(&mut spec, &r);
            results.push(prog.result(&eng));
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
                ids.push(prog.nodes.iter().flat_map(|&n| eng.children(n)).collect());
            }
            runs.push((results, ids));
        }
        for r in &runs[1..] {
            assert_eq!(r.0, runs[0].0, "seed {seed}: results differ between schedules");
            assert_eq!(r.1, runs[0].1, "seed {seed}: node ids differ between schedules");
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
    // `c` is guessed from its last inputs: the edit to `a` is three
    // reads away (nearer, the guess would wait for the input to run).
    let (mut eng, mut prog) = Program::load(lines(&[
        "a = 1",
        "x = a",
        "y = x",
        "b = y + 1",
        "c = b + 1",
        "d = c + 1",
    ]));
    eng.evaluate_sequential().unwrap();
    prog.set_line(&mut eng, 0, "a = 5".into());
    prog.set_line(&mut eng, 4, "c = b + 1 + 0".into());
    let r = eng
        .evaluate(&mut Greedy::speculative(Speculation::Predicted, 64), &mut Sequential)
        .unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(&prog.render(&eng)[..6], ["5", "5", "5", "6", "7", "8"]);
    assert!(r.mispredictions > 0, "{r:?}");
    // The refutations are counted by the field whose assumed definition
    // changed, and per node.
    let by_field: u64 = r.refuted_by_field.iter().map(|c| c.1).sum();
    assert!(by_field > 0 && by_field <= r.mispredictions, "{r:?}");
    assert!(r.refuted_by_field.windows(2).all(|w| w[0].1 >= w[1].1), "{r:?}");
    assert!(
        r.refuted_nodes > 0 && u64::from(r.refutations_max) <= r.mispredictions,
        "{r:?}"
    );
    assert!(r.refuted_nodes <= r.mispredictions, "{r:?}");
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
    common::REPLAY_STOPS.store(true, std::sync::atomic::Ordering::Relaxed);
    for (seed, n) in [(1, 1_000), (2, 4_000), (3, 16_000)] {
        let (mut eng, prog) = Program::load(generate(seed, n));
        eng.evaluate(&mut Greedy::default(), &mut Threads(4)).unwrap();
        assert_eq!(prog.result(&eng), reference(&prog.lines), "seed {seed}, {n} lines");
    }
}

/// Every link of the critical chain lands one below the node it names: a
/// read's site, a range's last contributor, the node a raise stands above.
/// A wait's or a hold's node may have risen since. The chain's top is the critical
/// path, and it ends only at a creation (or at height 0).
#[test]
fn the_critical_chain_falls_by_one_through_reads_ranges_and_waits() {
    common::REPLAY_STOPS.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut waits = 0;
    for (seed, n, random) in [(1, 1_000, false), (2, 4_000, false), (3, 2_000, true), (4, 4_000, true)] {
        let (mut eng, prog) = Program::load(generate(seed, n));
        let r = if random {
            eng.evaluate(&mut RandomScheduler::new(seed * 31 + 5, true), &mut Threads(4))
        } else {
            eng.evaluate(&mut Greedy::default(), &mut Threads(4))
        }
        .unwrap();
        assert_eq!(prog.result(&eng), reference(&prog.lines), "seed {seed}");
        waits += r.stops;
        let chain = eng.critical_chain_heights();
        assert_eq!(chain[0].1, r.critical_path, "seed {seed}: {chain:?}");
        for w in chain.windows(2) {
            let (h, cause) = (w[0].1, &w[0].2);
            let (next, hn) = (w[1].0, w[1].1);
            match cause {
                Cause::Changed { site, .. } => {
                    assert_eq!(*site, Some(next), "seed {seed}: {w:?}");
                    assert_eq!(hn + 1, h, "seed {seed}: {w:?}");
                }
                Cause::Output { of } => {
                    assert_eq!(*of, next, "seed {seed}: {w:?}");
                    assert_eq!(hn + 1, h, "seed {seed}: {w:?}");
                }
                Cause::Range { from, .. } => {
                    assert_eq!(*from, Some(next), "seed {seed}: {w:?}");
                    assert_eq!(hn + 1, h, "seed {seed}: {w:?}");
                }
                Cause::Raised { to, by, .. } => {
                    assert_eq!((*to, *by), (h, Some(next)), "seed {seed}: {w:?}");
                    assert_eq!(hn + 1, h, "seed {seed}: {w:?}");
                }
                // (A hold, as a wait: its node may have risen since.)
                Cause::Woken { by, .. } | Cause::Held { by, .. } => {
                    assert_eq!(*by, next, "seed {seed}: {w:?}");
                    assert!(hn + 1 >= h, "seed {seed}: {w:?}");
                }
                Cause::Created { by, from_hint } => {
                    assert!(
                        *by == Some(next) || from_hint.map(|(_, t)| t) == Some(next),
                        "seed {seed}: {w:?}"
                    );
                }
                c => panic!("seed {seed}: {c:?} in a chain"),
            }
        }
        let last = chain.last().unwrap();
        assert!(
            last.1 == 0 || matches!(last.2, Cause::Created { .. } | Cause::Raised { by: None, .. }),
            "seed {seed}: {last:?}"
        );
    }
    assert!(waits > 0, "waits {waits}");
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

// ---------------------------------------------------------- environment

const ENV_SRC: [&str; 9] = [
    "a = x + 1",
    "b = y * 2",
    "c = a + b",
    "{ z = y + 1 }",
    "collect l until mark if go",
    "1",
    "mark a",
    "x = 100",
    "d = x",
];

fn env_eval(eng: &mut Engine, prog: &Program, env: &[(&str, i64)], sched: u8) -> Report {
    eng.set_environment(env_values(eng, env));
    let r = match sched {
        0 => eng.evaluate_sequential(),
        1 => eng.evaluate(&mut Greedy::speculative(Speculation::All, 64), &mut Threads(3)),
        _ => eng.evaluate(&mut RandomScheduler::new(11, true), &mut Sequential),
    }
    .expect("evaluate");
    assert_eq!(
        prog.result(eng),
        interpret_env(&prog.lines, env).expect("interpreter"),
        "{env:?}"
    );
    r
}

#[test]
fn equal_environments_give_identical_ids_and_results() {
    let env = [("x", 1), ("y", 2), ("go", 1)];
    for sched in 0..3 {
        let (mut e1, p1) = Program::load(lines(&ENV_SRC));
        env_eval(&mut e1, &p1, &env, sched);
        let (mut e2, p2) = Program::load(lines(&ENV_SRC));
        // The same environment supplied in another order.
        let rev: Vec<(&str, i64)> = env.iter().rev().copied().collect();
        env_eval(&mut e2, &p2, &rev, sched);
        assert_eq!(p1.nodes, p2.nodes);
        assert_eq!(p1.result(&e1), p2.result(&e2));
        assert_eq!(
            p1.render(&e1),
            ["2", "4", "6", "()", "[1]", "1", "()", "100", "100", "<>"]
        );
        // Supplying an equal environment again runs nothing.
        let r = env_eval(&mut e1, &p1, &env, sched);
        assert_eq!(r.nodes_run, 0, "sched {sched}");
    }
}

#[test]
fn changing_one_environment_field_reruns_only_its_readers() {
    for sched in 0..3 {
        let (mut eng, prog) = Program::load(lines(&ENV_SRC));
        env_eval(&mut eng, &prog, &[("x", 1), ("y", 2), ("go", 1)], sched);
        let ids: Vec<_> = prog.nodes.clone();
        // x: its reader `a`, then `c`.
        let r = env_eval(&mut eng, &prog, &[("x", 5), ("y", 2), ("go", 1)], sched);
        assert_eq!(prog.render(&eng)[..3], ["6", "4", "10"]);
        assert_eq!(
            Report::by_op(&r.runs_by_op, "stmt"),
            2,
            "sched {sched}: {:?}",
            r.runs_by_op
        );
        // y: `b`, `c` and the block's child.
        let r = env_eval(&mut eng, &prog, &[("x", 5), ("y", 3), ("go", 1)], sched);
        assert_eq!(prog.render(&eng)[..4], ["6", "6", "12", "()"]);
        assert_eq!(
            Report::by_op(&r.runs_by_op, "stmt"),
            3,
            "sched {sched}: {:?}",
            r.runs_by_op
        );
        // go goes: only the claim re-runs, and runs on past `mark`.
        let r = env_eval(&mut eng, &prog, &[("x", 5), ("y", 3)], sched);
        assert_eq!(r.nodes_run, 1, "sched {sched}");
        assert_eq!(prog.render(&eng)[4], "[1 () 100 100]");
        // An unread field changes nothing.
        let r = env_eval(&mut eng, &prog, &[("x", 5), ("y", 3), ("w", 9)], sched);
        assert_eq!(r.nodes_run, 0, "sched {sched}");
        assert_eq!(prog.nodes, ids);
    }
}

#[test]
fn a_node_parked_on_a_target_a_claim_makes_inert_is_woken() {
    // `y = x` parks on the eval (a pending definition set) while the eval
    // unfolds; the claim then takes the eval. The parked node must be
    // woken: it re-binds past the now-inert eval, under every schedule.
    let src = ["n = 0", "sumnext n", "eval \"x = 1\"", "y = x", "z = y"];
    for seed in 0..24u64 {
        let (mut eng, mut prog) = Program::load(lines(&src));
        let run = |eng: &mut Engine, m: u64| match m % 3 {
            0 => eng.evaluate_sequential().map(|_| ()),
            1 => eng
                .evaluate(&mut Greedy::speculative(Speculation::All, 64), &mut Threads(3))
                .map(|_| ()),
            _ => eng
                .evaluate(&mut RandomScheduler::new(m, m.is_multiple_of(2)), &mut Sequential)
                .map(|_| ()),
        };
        run(&mut eng, seed).unwrap_or_else(|e| panic!("seed {seed}: {e:?}"));
        assert_eq!(prog.result(&eng), reference(&prog.lines), "seed {seed}");
        prog.set_line(&mut eng, 0, "n = 1".into());
        run(&mut eng, seed).unwrap_or_else(|e| panic!("seed {seed} after edit: {e:?}"));
        assert_eq!(prog.result(&eng), reference(&prog.lines), "seed {seed} after edit");
        prog.set_line(&mut eng, 0, "n = 0".into());
        run(&mut eng, seed).unwrap_or_else(|e| panic!("seed {seed} after undo: {e:?}"));
        assert_eq!(prog.result(&eng), reference(&prog.lines), "seed {seed} after undo");
    }
}

// ---------------------------------------------------------- trace replay

#[test]
fn the_trace_loader_rejects_malformed_input() {
    use common::trace::load_trace;
    let meta = |n: usize| format!("{{\"meta\": {{\"version\": 1, \"document\": \"d\", \"steps\": {n}}}}}\n");
    let cases: Vec<(String, &str)> = vec![
        (
            meta(1) + "{\"id\": \"a\", \"class\": \"c\", \"pos\": 1, \"reads\": [], \"writes\": [], \"bogus\": 1}\n",
            "unknown step key",
        ),
        (
            "{\"meta\": {\"version\": 2, \"document\": \"d\", \"steps\": 0}}\n".to_string(),
            "version",
        ),
        (
            meta(2)
                + "{\"id\": \"a\", \"class\": \"c\", \"pos\": 1, \"reads\": [], \"writes\": []}\n{\"id\": \"a\", \"class\": \"c\", \"pos\": 2, \"reads\": [], \"writes\": []}\n",
            "duplicate step id",
        ),
        (
            meta(2)
                + "{\"id\": \"a\", \"class\": \"c\", \"pos\": 1, \"depth\": 0, \"reads\": [], \"writes\": [], \"scope\": \"open\"}\n{\"id\": \"b\", \"class\": \"c\", \"pos\": 2, \"depth\": 0, \"reads\": [], \"writes\": []}\n",
            "depth",
        ),
        (
            meta(2)
                + "{\"id\": \"a\", \"class\": \"c\", \"pos\": 2, \"reads\": [], \"writes\": []}\n{\"id\": \"b\", \"class\": \"c\", \"pos\": 1, \"reads\": [], \"writes\": []}\n",
            "pos",
        ),
        (
            meta(1)
                + "{\"id\": \"a\", \"class\": \"c\", \"pos\": 1, \"reads\": [], \"writes\": [{\"field\": \"x\", \"locality\": \"nowhere\"}]}\n",
            "locality",
        ),
        (
            meta(1)
                + "{\"id\": \"a\", \"class\": \"c\", \"pos\": 1, \"reads\": [], \"writes\": [], \"claim\": {\"predicted\": {\"n\": 1, \"until\": {\"class\": \"m\", \"inert\": true}}}}\n",
            "stream claim",
        ),
        (meta(1) + "{\"id\": \"a\" \"class\": \"c\"}\n", "expected ','"),
        (
            meta(1) + "{\"id\": \"a\", \"class\": \"c\", \"pos\": 1, \"reads\": []}\n",
            "`writes`",
        ),
        (
            meta(3) + "{\"id\": \"a\", \"class\": \"c\", \"pos\": 1, \"reads\": [], \"writes\": []}\n",
            "meta.steps",
        ),
        (
            meta(1)
                + "{\"id\": \"a\", \"class\": \"c\", \"pos\": 1, \"reads\": [], \"writes\": [], \"scope_kind\": \"A\"}\n",
            "scope_kind",
        ),
        (
            meta(1) + "{\"id\": \"a\", \"class\": \"c\", \"pos\": 1, \"reads\": [], \"writes\": [], \"writes\": []}\n",
            "duplicate key",
        ),
    ];
    for (text, want) in cases {
        let err = load_trace(&text).err().unwrap_or_else(|| panic!("accepted: {text}"));
        assert!(err.contains(want), "{text}: {err} should mention {want:?}");
    }
    // Edits too.
    let e = common::trace::load_edits("{\"op\": \"delete\", \"step\": \"a\", \"writes\": []}\n").expect_err("rejected");
    assert!(e.contains("unknown key"), "{e}");
    let e = common::trace::load_edits("{\"op\": \"rename\", \"step\": \"a\"}\n").expect_err("rejected");
    assert!(e.contains("unknown op"), "{e}");
}

#[test]
fn a_toy_derived_trace_round_trips_and_replays_like_a_cold_run() {
    use common::trace::{self, Doc};
    let lines = common::generate_with(5, 400, true);
    let t = trace::from_toy(&lines, "toy");
    let text = trace::trace_jsonl(&t);
    let back = trace::load_trace(&text).expect("the writer's output loads");
    assert_eq!(back.steps, t.steps);
    assert_eq!(trace::trace_jsonl(&back), text);
    let mut eng = Engine::new();
    let mut doc = Doc::load(&mut eng, &t);
    eng.evaluate_sequential().unwrap();
    let mut e2 = Engine::new();
    let d2 = Doc::load(&mut e2, &t);
    e2.evaluate(&mut RandomScheduler::new(3, true), &mut Sequential)
        .unwrap();
    assert_eq!(doc.result(&eng), d2.result(&e2), "cold results agree across schedulers");
    for (k, e) in trace::generated_edits(&t, 11, 60).iter().enumerate() {
        doc.apply(&mut eng, &e.edit).unwrap();
        eng.evaluate_sequential().unwrap();
        let snap = doc.snapshot();
        let mut e3 = Engine::new();
        let d3 = Doc::load(&mut e3, &snap);
        e3.evaluate(&mut Greedy::speculative(Speculation::All, 64), &mut Threads(3))
            .unwrap();
        assert_eq!(
            doc.result(&eng),
            d3.result(&e3),
            "edit {k}: incremental equals a cold replay"
        );
        assert!(eng.check_invariants().is_empty(), "edit {k}");
    }
}

#[test]
fn deleting_an_outer_opener_rebinds_a_reader_past_an_inner_openers_definition() {
    // `B` opens a scope and defines `f` (an opener's own definitions lie
    // in the enclosing scope); inside `A`, that is invisible to `R`
    // after `A`'s close. Deleting `A` leaves its close unmatched and
    // `B`'s definition at the top level: `R` now reads it, as in a cold
    // evaluation of the edited document.
    use common::trace::{self, Doc};
    let text = concat!(
        "{\"meta\": {\"version\": 1, \"document\": \"m\", \"steps\": 5, \"sequences\": [], \"env\": {}}}\n",
        "{\"id\": \"A\", \"class\": \"a\", \"pos\": 1, \"reads\": [], \"writes\": [], \"scope\": \"open\", \"scope_kind\": \"k1\"}\n",
        "{\"id\": \"B\", \"class\": \"b\", \"pos\": 2, \"reads\": [], \"writes\": [{\"field\": \"f\", \"locality\": \"local\"}], \"scope\": \"open\", \"scope_kind\": \"k2\"}\n",
        "{\"id\": \"cB\", \"class\": \"c\", \"pos\": 3, \"reads\": [], \"writes\": [], \"scope\": \"close\"}\n",
        "{\"id\": \"cA\", \"class\": \"c\", \"pos\": 4, \"reads\": [], \"writes\": [], \"scope\": \"close\"}\n",
        "{\"id\": \"R\", \"class\": \"r\", \"pos\": 5, \"reads\": [\"f\"], \"writes\": []}\n",
    );
    let tr = trace::load_trace(text).expect("loads");
    let mut eng = Engine::new();
    let mut doc = Doc::load(&mut eng, &tr);
    eng.evaluate_sequential().expect("cold");
    let before = doc.result(&eng);
    let edit = trace::load_edits("{\"op\": \"delete\", \"step\": \"A\"}\n").expect("edit");
    doc.apply(&mut eng, &edit[0].edit).expect("apply");
    eng.evaluate_sequential().expect("after the edit");
    let got = doc.result(&eng);
    let mut e2 = Engine::new();
    let d2 = Doc::load(&mut e2, &doc.snapshot());
    e2.evaluate_sequential().expect("cold replay");
    let want = d2.result(&e2);
    assert_eq!(got, want, "incremental != cold after deleting the outer opener");
    assert_ne!(got[3], before[4], "R now reads B's definition");
}

#[test]
fn trace_format_takes_provenance_equal_positions_bounded_unfolds_and_nested_edits() {
    use common::trace::{self, Doc};
    let text = concat!(
        "{\"meta\": {\"version\": 1, \"document\": \"m\", \"steps\": 5, \"sequences\": [], \"env\": {}, \"provenance\": {\"tool\": \"gen\", \"run\": 7}}}\n",
        "{\"id\": \"a\", \"class\": \"assign\", \"pos\": 1, \"reads\": [], \"writes\": [{\"field\": \"x\", \"locality\": \"local\"}]}\n",
        // Static writes and a bounded unfolding together; the same position as `a`.
        "{\"id\": \"u\", \"class\": \"call\", \"pos\": 1, \"reads\": [\"x\"], \"writes\": [{\"field\": \"r\", \"locality\": \"local\"}], \"may_define\": {\"names\": [{\"name\": \"y\", \"locality\": \"local\"}]}, \"children\": [{\"id\": \"u.1\", \"class\": \"assign\", \"pos\": 0, \"reads\": [\"x\"], \"writes\": [{\"field\": \"y\", \"locality\": \"local\"}]}]}\n",
        "{\"id\": \"c\", \"class\": \"collect\", \"pos\": 2, \"reads\": [], \"writes\": [{\"field\": \"l\", \"locality\": \"local\"}], \"claim\": {\"until\": {\"class\": \"mark\", \"inert\": false, \"confirm\": [\"never\"]}}}\n",
        "{\"id\": \"m\", \"class\": \"mark\", \"pos\": 3, \"reads\": [\"y\", \"r\"], \"writes\": []}\n",
        "{\"id\": \"z\", \"class\": \"assign\", \"pos\": 4, \"reads\": [\"y\", \"r\", \"l\"], \"writes\": [{\"field\": \"z\", \"locality\": \"local\"}]}\n",
    );
    let tr = trace::load_trace(text).expect("loads");
    assert!(tr.meta.provenance.is_some());
    assert!(trace::trace_jsonl(&tr).contains("\"provenance\": {\"run\": 7, \"tool\": \"gen\"}"));
    let mut eng = Engine::new();
    let mut doc = Doc::load(&mut eng, &tr);
    let r = eng.evaluate_sequential().expect("cold");
    // The mark (reading inside the claimer's range) is the delimiter on
    // its class alone, its unconfirmed field notwithstanding; the collect
    // takes the unfold's text, not the mark.
    assert_eq!(r.runs, 6, "every step and the child ran once: {}", r.runs);
    let before = doc.result(&eng);
    // A nested edit: the child's reads change; the call runs again.
    let e = trace::load_edits("{\"op\": \"change_reads\", \"step\": \"u.1\", \"reads\": []}\n").expect("edit");
    doc.apply(&mut eng, &e[0].edit).expect("apply");
    let r = eng.evaluate_sequential().expect("after the edit");
    assert!(
        r.nodes_run >= 2 && r.nodes_run <= 5,
        "the call, its child and readers: {}",
        r.nodes_run
    );
    let got = doc.result(&eng);
    let mut e2 = Engine::new();
    let d2 = Doc::load(&mut e2, &doc.snapshot());
    e2.evaluate_sequential().expect("cold replay");
    assert_eq!(got, d2.result(&e2), "incremental == cold after the nested edit");
    assert_ne!(got, before, "the child's output changed");
    // Deleting a nested step and inserting after one.
    let e = trace::load_edits(concat!(
        "{\"op\": \"delete\", \"step\": \"u.1\"}\n",
        "{\"op\": \"insert_after\", \"step\": \"m\", \"new\": {\"id\": \"n\", \"class\": \"assign\", \"pos\": 3, \"reads\": [\"y\"], \"writes\": []}}\n",
    ))
    .expect("edits");
    for e in &e {
        doc.apply(&mut eng, &e.edit).expect("apply");
        eng.evaluate_sequential().expect("after the edit");
        let mut e2 = Engine::new();
        let d2 = Doc::load(&mut e2, &doc.snapshot());
        e2.evaluate_sequential().expect("cold replay");
        assert_eq!(doc.result(&eng), d2.result(&e2), "incremental == cold after {e:?}");
    }
    // Positions may not go backwards, and a bound needs a pending set or children.
    let bad = text.replace(
        "\"pos\": 3, \"reads\": [\"y\", \"r\"]",
        "\"pos\": 0, \"reads\": [\"y\", \"r\"]",
    );
    assert!(trace::load_trace(&bad).is_err());
    let bad = text.replace("\"children\": [{\"id\": \"u.1\", \"class\": \"assign\", \"pos\": 0, \"reads\": [\"x\"], \"writes\": [{\"field\": \"y\", \"locality\": \"local\"}]}]", "\"children\": []");
    assert!(trace::load_trace(&bad).is_err());
}

/// `["i"]` (pending definition set) reads the environment's `file` with
/// `use_env` and defines `x` to it when supplied; `["r"]` reads `x`.
struct EnvToy;

impl defuse::Operation for EnvToy {
    fn defs(&self, a: &[ValueId], v: &defuse::Values) -> Option<Vec<(Field, defuse::Locality)>> {
        match &*v.get(a[0]) {
            defuse::Value::Str(s) if &**s == "i" => None,
            _ => Some(Vec::new()),
        }
    }
    fn run(&self, cx: &mut defuse::Ctx<'_>) -> Result<ValueId, defuse::Stop> {
        let v = cx.values();
        let a = cx.args();
        if matches!(&*v.get(a[0]), defuse::Value::Str(s) if &**s == "i") {
            if let Some(x) = cx.use_env(v.field("file"))? {
                cx.define(v.field("x"), x);
            }
            return Ok(ValueId::UNIT);
        }
        Ok(cx.use_field(v.field("x"))?.unwrap_or_else(|| v.str("none")))
    }
}

#[test]
fn an_environment_read_waits_until_the_field_is_supplied_or_declared_absent() {
    use std::sync::Arc;
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(EnvToy));
    let u = eng.new_unit();
    let i = eng.values().str("i");
    let i = eng.push(u, op, vec![i]);
    let rr = eng.values().str("r");
    let r = eng.push(u, op, vec![rr]);
    eng.add_root(r);
    let out = |e: &Engine| e.output(r).map(|x| e.values().render(x));
    // Neither supplied nor absent: `i` waits for the environment and `r`
    // waits on `i`'s pending definitions; no cycle, no output.
    let rep = eng.evaluate_sequential().expect("stalled is not an error");
    assert_eq!(rep.env_stalled, 2);
    assert_eq!(out(&eng), None);
    assert_eq!(eng.output(i), None);
    // Declared absent: `i` defines nothing, `r` reads none.
    let f = eng.values().field("file");
    eng.set_environment_absent([f]);
    let rep = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(rep.env_stalled, 0);
    assert_eq!(out(&eng).as_deref(), Some("\"none\""));
    // Supplied: both run again.
    let five = eng.values().int(5);
    eng.set_environment([(f, five)]);
    eng.set_environment_absent([]);
    let rep = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(out(&eng).as_deref(), Some("5"));
    assert_eq!(rep.nodes_run, 2);
    // Withdrawn again: `i` waits; its last definition stands until the
    // field is known, so `r` keeps its value (its binding did not change).
    eng.set_environment([]);
    let rep = eng.evaluate_sequential().expect("stalled again");
    assert_eq!(rep.env_stalled, 1);
    assert_eq!(out(&eng).as_deref(), Some("5"));
    let six = eng.values().int(6);
    eng.set_environment([(f, six)]);
    eng.evaluate_sequential().expect("evaluate");
    assert_eq!(out(&eng).as_deref(), Some("6"));
}

/// The client supplies what the provider answered (PhiTeX moves the
/// provider's files into its environment): the answers are the same, no
/// reader runs again.
#[test]
fn supplying_the_providers_answer_reruns_no_reader() {
    use std::sync::Arc;
    for absent in [true, false] {
        let mut eng = Engine::new();
        let op = eng.register(Arc::new(EnvToy));
        let five = eng.values().int(5);
        eng.set_env_provider(Box::new(move |v: &defuse::Values, f: Field| {
            (v.as_str(f.0).as_deref() == Some("file")).then_some((!absent).then_some(five))
        }));
        let u = eng.new_unit();
        let i = eng.values().str("i");
        eng.push(u, op, vec![i]);
        let rr = eng.values().str("r");
        let r = eng.push(u, op, vec![rr]);
        eng.add_root(r);
        let rep = eng.evaluate_sequential().expect("evaluate");
        assert_eq!(rep.nodes_run, 2);
        let f = eng.values().field("file");
        if absent {
            eng.set_environment_absent([f]);
        } else {
            eng.set_environment([(f, five)]);
        }
        let rep = eng.evaluate_sequential().expect("evaluate");
        assert_eq!(rep.nodes_run, 0, "absent {absent}");
        // Then withdrawn: the provider's answer is underneath, the same.
        if absent {
            eng.set_environment_absent([]);
        } else {
            eng.set_environment([]);
        }
        let rep = eng.evaluate_sequential().expect("evaluate");
        assert_eq!(rep.nodes_run, 0, "absent {absent}, withdrawn");
    }
}

/// `["a", k]` defines `g` to `k` and places a continuation `["f"]` that
/// reads `g`; `["b"]` reads `g`. Each run's tag and answer is recorded.
struct FollowToy {
    seen: std::sync::Mutex<Vec<(String, Option<i64>)>>,
}

impl defuse::Operation for FollowToy {
    fn defs(&self, _: &[ValueId], _: &defuse::Values) -> Option<Vec<(Field, defuse::Locality)>> {
        None
    }
    fn run(&self, cx: &mut defuse::Ctx<'_>) -> Result<ValueId, defuse::Stop> {
        let v = cx.values();
        let a = cx.args();
        let tag = v.as_str(a[0]).unwrap_or_default();
        let g = v.field("g");
        if tag == "a" {
            cx.define(g, a[1]);
            let f = v.str("f");
            cx.follow(cx.op(), vec![f]);
            return Ok(ValueId::UNIT);
        }
        let x = cx.use_field(g)?;
        self.seen.lock().unwrap().push((tag, x.and_then(|x| v.as_int(x))));
        Ok(x.unwrap_or(ValueId::UNIT))
    }
}

/// A node inserted after one that placed continuations goes after them,
/// as a cold build places it: the continuations read the definitions
/// before them, not the new node's (PhiTeX's page_steps, a paragraph
/// inserted after one that placed follow nodes).
#[test]
fn a_node_inserted_after_a_follower_goes_after_its_continuations() {
    use std::sync::Arc;
    let toy = Arc::new(FollowToy {
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let mut eng = Engine::new();
    let op = eng.register(toy.clone());
    let u = eng.new_unit();
    let a = vec![eng.values().str("a"), eng.values().int(1)];
    let a = eng.push(u, op, a);
    let b = vec![eng.values().str("b")];
    let b = eng.push(u, op, b);
    eng.add_root(b);
    eng.evaluate_sequential().expect("cold");
    assert_eq!(*toy.seen.lock().unwrap(), [("f".to_owned(), Some(1)), ("b".to_owned(), Some(1))]);
    toy.seen.lock().unwrap().clear();
    let n = vec![eng.values().str("a"), eng.values().int(2)];
    eng.insert_after(a, op, n);
    eng.evaluate_sequential().expect("edit");
    // `a`'s continuation keeps reading 1; the new node's reads 2, `b` 2.
    let mut seen = toy.seen.lock().unwrap().clone();
    seen.sort();
    assert_eq!(seen, [("b".to_owned(), Some(2)), ("f".to_owned(), Some(2))], "{seen:?}");
    assert_eq!(eng.output(b).and_then(|x| eng.values().as_int(x)), Some(2));
}

/// `["link", k]` has a pending definition set: it reads `f(k-1)` (`k` 0:
/// the environment's `file`) and defines `f(k)` to it.
struct Link;

impl defuse::Operation for Link {
    fn defs(&self, _: &[ValueId], _: &defuse::Values) -> Option<Vec<(Field, defuse::Locality)>> {
        None
    }
    fn run(&self, cx: &mut defuse::Ctx<'_>) -> Result<ValueId, defuse::Stop> {
        let v = cx.values();
        let a = cx.args();
        let k = v.as_int(a[1]).unwrap_or(0);
        let got = if k == 0 {
            cx.use_env(v.field("file"))?
        } else {
            cx.use_field(v.field(&format!("f{}", k - 1)))?
        };
        if let Some(x) = got {
            cx.define(v.field(&format!("f{k}")), x);
        }
        Ok(ValueId::UNIT)
    }
}

#[test]
fn thousands_of_nodes_stalled_in_a_chain_behind_an_environment_wait_cost_linear_time() {
    use std::sync::Arc;
    let n = 4000;
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Link));
    let u = eng.new_unit();
    let link = eng.values().str("link");
    for k in 0..n {
        let kk = eng.values().int(k);
        eng.push(u, op, vec![link, kk]);
    }
    let t = std::time::Instant::now();
    let rep = eng.evaluate_sequential().expect("stalled, not a cycle");
    assert_eq!(rep.env_stalled, u64::try_from(n).unwrap());
    assert_eq!(rep.env_waits.len(), 1);
    // A second evaluation with nothing new re-walks the same chain.
    let rep = eng.evaluate_sequential().expect("still stalled");
    assert_eq!(rep.env_stalled, u64::try_from(n).unwrap());
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    let f = eng.values().field("file");
    let seven = eng.values().int(7);
    eng.set_environment([(f, seven)]);
    let rep = eng.evaluate_sequential().expect("supplied");
    assert_eq!(rep.env_stalled, 0);
    assert_eq!(rep.nodes_run, u64::try_from(n).unwrap());
}

// ------------------------------------------------------- never hanging

#[test]
fn a_node_past_its_run_budget_ends_the_evaluation_naming_it_and_why() {
    // A loop-back that never settles re-runs its publisher every round;
    // with a budget of three runs the fourth is the error, with the loop
    // field as the cause of the last runs.
    let (mut eng, prog) = Program::load(lines(&["loop n = 0", "n = n + 1"]));
    eng.set_limits(Limits {
        runs_per_node: 3,
        ..Limits::default()
    });
    let e = eng.evaluate_sequential().expect_err("never settles");
    let EvalError::RunawayNode { node, runs, causes } = e else {
        panic!("{e}");
    };
    assert_eq!(runs, 4);
    assert!(prog.node_ids(&eng).contains(&node), "the publisher is named");
    // The node is readable from its id: the message can name it.
    let info = eng.node_info(node).expect("the node is there");
    assert_eq!(info.op, "stmt");
    assert!(
        eng.values().render(info.args[0]).contains("\"n\""),
        "{:?}",
        eng.values().render(info.args[0])
    );
    assert_eq!(info.parent, None);
    assert_eq!(info.unit, prog.unit);
    assert!(
        !causes.is_empty() && causes.iter().all(|c| matches!(c, Cause::Loop { .. })),
        "{causes:?}"
    );
    // Within the default budget the usual non-convergence verdict stands.
    let (mut eng, _) = Program::load(lines(&["loop n = 0", "n = n + 1"]));
    assert!(matches!(
        eng.evaluate_sequential(),
        Err(EvalError::NonConvergence { .. })
    ));
}

/// An executor that loses every result: the engine must not spin on it.
struct Lossy;

impl defuse::Executor for Lossy {
    fn execute(&mut self, _: usize, _: &(dyn Fn(usize) -> defuse::RunResult + Sync), _: &mut Vec<defuse::RunResult>) {}
}

#[test]
fn rounds_that_apply_nothing_end_the_evaluation_listing_the_waiters() {
    let (mut eng, prog) = Program::load(lines(&["x = 1", "y = x + 1", "z = y + 1"]));
    eng.set_limits(Limits {
        idle_rounds: 5,
        ..Limits::default()
    });
    let e = eng
        .evaluate(&mut Greedy::default(), &mut Lossy)
        .expect_err("nothing is ever applied");
    let EvalError::NoProgress {
        rounds,
        waiters,
        report,
    } = e
    else {
        panic!("{e}");
    };
    assert_eq!(rounds, 6);
    // The counters come with the error.
    assert!(report.steps >= 6, "{report:?}");
    let ids = prog.node_ids(&eng);
    assert!(
        !waiters.is_empty() && waiters.iter().all(|w| ids.contains(&w.node)),
        "{waiters:?}"
    );
    assert!(waiters.iter().all(|w| w.on == WaitOn::Queued));
}

#[test]
fn a_deadline_ends_the_evaluation_with_the_nodes_still_to_run() {
    let (mut eng, prog) = Program::load(generate(3, 20));
    eng.set_limits(Limits {
        total_runs: Some(5),
        ..Limits::default()
    });
    let e = eng.evaluate_sequential().expect_err("five runs allowed");
    let EvalError::Deadline { runs, waiters, report } = e else {
        panic!("{e}");
    };
    // The limit is checked between rounds: the first round ran its batch.
    assert!(runs >= 5, "{runs}");
    // The counters come with the error.
    assert_eq!(report.runs, runs, "{report:?}");
    let ids = prog.node_ids(&eng);
    assert!(!waiters.is_empty() && waiters.iter().all(|w| ids.contains(&w.node)));
    // Lifting the limit finishes the rest.
    eng.set_limits(Limits::default());
    eng.evaluate_sequential().expect("the rest");
    assert_eq!(prog.result(&eng), reference(&prog.lines));
    // A zero time limit is reached at the first round.
    let (mut eng, _) = Program::load(generate(3, 20));
    eng.set_limits(Limits {
        time: Some(std::time::Duration::ZERO),
        ..Limits::default()
    });
    assert!(matches!(eng.evaluate_sequential(), Err(EvalError::Deadline { .. })));
}

/// `["deep"]` unfolds a `["deep"]` child without end; `["wide", n]`
/// unfolds n leaves.
struct Unfolder;

impl defuse::Operation for Unfolder {
    fn defs(&self, _: &[ValueId], _: &defuse::Values) -> Option<Vec<(Field, defuse::Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut defuse::Ctx<'_>) -> Result<ValueId, defuse::Stop> {
        let v = cx.values();
        let a = cx.args();
        let me = cx.op();
        match &*v.get(a[0]) {
            defuse::Value::Str(s) if &**s == "deep" => {
                let deep = v.str("deep");
                cx.child(me, vec![deep]);
            }
            defuse::Value::Str(s) if &**s == "wide" => {
                let count = v.as_int(a[1]).unwrap_or(0);
                for index in 0..count {
                    let leaf = v.str("leaf");
                    let key = v.int(index);
                    cx.child(me, vec![leaf, key]);
                }
            }
            _ => {}
        }
        Ok(ValueId::UNIT)
    }
}

#[test]
fn unfolding_past_the_creation_budget_ends_the_evaluation_naming_the_creator() {
    use std::sync::Arc;
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Unfolder));
    eng.set_limits(Limits {
        unfold_depth: 8,
        children_per_node: 3,
        ..Limits::default()
    });
    let u = eng.new_unit();
    let deep = eng.values().str("deep");
    eng.push(u, op, vec![deep]);
    let e = eng.evaluate_sequential().expect_err("unfolds without end");
    let EvalError::RunawayCreation { depth, last, .. } = e else {
        panic!("{e}");
    };
    assert!(depth > 8, "{depth}");
    assert_eq!(last.len(), 1);
    assert!(last[0].1.contains("deep"), "{last:?}");
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Unfolder));
    eng.set_limits(Limits {
        children_per_node: 3,
        ..Limits::default()
    });
    let u = eng.new_unit();
    let wide = eng.values().str("wide");
    let five = eng.values().int(5);
    let w = eng.push(u, op, vec![wide, five]);
    let e = eng.evaluate_sequential().expect_err("five children, three allowed");
    let EvalError::RunawayCreation {
        node, created, last, ..
    } = e
    else {
        panic!("{e}");
    };
    assert_eq!((node, created), (w, 5));
    assert_eq!(last.len(), 4, "the last four children: {last:?}");
}

#[test]
fn trace_steps_exercise_scopes_claims_contributions_folds_and_the_environment() {
    use common::trace::{self, Doc};
    let text = concat!(
        "{\"meta\": {\"version\": 1, \"document\": \"m\", \"steps\": 12, \"sequences\": [\"items\"], \"env\": {\"ts\": 1}}}\n",
        "{\"id\": \"env\", \"class\": \"stamp\", \"pos\": 0, \"reads\": [], \"env_reads\": [\"ts\"], \"writes\": [{\"field\": \"t\", \"locality\": \"global\"}]}\n",
        "{\"id\": \"open\", \"class\": \"begin\", \"pos\": 1, \"reads\": [], \"writes\": [], \"scope\": \"open\", \"scope_kind\": \"A\"}\n",
        "{\"id\": \"fold\", \"class\": \"fold\", \"pos\": 2, \"reads\": [\"t\"], \"writes\": [], \"fold\": {\"sequence\": \"items\", \"step_class\": \"add\"}}\n",
        "{\"id\": \"it1\", \"class\": \"item\", \"pos\": 3, \"reads\": [], \"writes\": [], \"scope\": \"pending\", \"starts_implicit\": \"B\"}\n",
        "{\"id\": \"a1\", \"class\": \"add\", \"pos\": 4, \"reads\": [\"t\"], \"writes\": [], \"contributes\": \"items\", \"fold\": {\"sequence\": \"items\", \"step_class\": \"add\"}}\n",
        "{\"id\": \"it2\", \"class\": \"item\", \"pos\": 5, \"reads\": [], \"writes\": [], \"scope\": \"pending\", \"starts_implicit\": \"B\"}\n",
        "{\"id\": \"a2\", \"class\": \"add\", \"pos\": 6, \"reads\": [], \"writes\": [], \"contributes\": \"items\", \"reads_prev_contribution\": \"items\", \"fold\": {\"sequence\": \"items\", \"step_class\": \"add\"}}\n",
        "{\"id\": \"coll\", \"class\": \"collect\", \"pos\": 7, \"reads\": [], \"writes\": [{\"field\": \"l\", \"locality\": \"local\"}], \"claim\": {\"predicted\": {\"n\": 2, \"until\": {\"class\": \"mark\", \"inert\": false, \"confirm\": [\"t\"]}}}}\n",
        "{\"id\": \"ev\", \"class\": \"eval\", \"pos\": 8, \"reads\": [\"t\"], \"writes\": null, \"may_define\": {\"names\": [{\"name\": \"x\", \"locality\": \"local\"}]}, \"children\": [{\"id\": \"ev.1\", \"class\": \"assign\", \"pos\": 0, \"reads\": [\"t\"], \"writes\": [{\"field\": \"x\", \"locality\": \"local\"}]}], \"cause\": \"eval\", \"prior\": {\"children\": 1}}\n",
        "{\"id\": \"mk\", \"class\": \"mark\", \"pos\": 9, \"reads\": [\"x\"], \"writes\": []}\n",
        "{\"id\": \"close\", \"class\": \"end\", \"pos\": 10, \"reads\": [], \"writes\": [], \"scope\": {\"closes\": 2, \"opens\": 0, \"before\": false}}\n",
        "{\"id\": \"after\", \"class\": \"assign\", \"pos\": 11, \"reads\": [\"x\", \"t\", \"l\"], \"writes\": [{\"field\": \"y\", \"locality\": \"local\"}], \"claim\": {\"count_from\": \"missing\"}}\n",
    );
    let tr = trace::load_trace(text).expect("loads");
    let run = |spec: bool, env: i64| {
        let mut eng = Engine::new();
        let doc = Doc::load(&mut eng, &tr);
        eng.set_environment(vec![(eng.values().field("ts"), eng.values().int(env))]);
        let r = if spec {
            eng.evaluate(&mut Greedy::speculative(Speculation::Predicted, 64), &mut Threads(2))
                .unwrap()
        } else {
            eng.evaluate_sequential().unwrap()
        };
        (
            doc.result(&eng),
            r,
            eng.children(doc.nodes[8]).len(),
            eng.critical_path(),
        )
    };
    let (seq, _, kids, _) = run(false, 1);
    let (spec, r, _, _) = run(true, 1);
    assert_eq!(seq, spec, "schedule independence");
    assert_eq!(kids, 1, "the eval unfolded its child");
    println!(
        "speculations {} confirmations {} mispredictions {}",
        r.speculations, r.confirmations, r.mispredictions
    );
    // The environment: a different stamp changes exactly the readers of t
    // and what depends on them; the same stamp gives the same ids.
    let (other, _, _, _) = run(false, 2);
    assert_ne!(seq, other);
    assert_eq!(seq[1], other[1], "the opener reads nothing");
    let mut eng = Engine::new();
    let mut doc = Doc::load(&mut eng, &tr);
    eng.set_environment(vec![(eng.values().field("ts"), eng.values().int(1))]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(doc.result(&eng), seq);
    eng.set_environment(vec![(eng.values().field("ts"), eng.values().int(2))]);
    let r = eng.evaluate_sequential().unwrap();
    assert_eq!(doc.result(&eng), other);
    assert!(r.nodes_run < 12, "only readers of ts re-ran: {}", r.nodes_run);
    // An edit that makes the eval define nothing releases x's readers.
    doc.apply(&mut eng, &trace::Edit::ChangeWrites("ev".into(), Some(Vec::new())))
        .unwrap();
    eng.evaluate_sequential().unwrap();
    let snap = doc.snapshot();
    let mut e2 = Engine::new();
    let d2 = Doc::load(&mut e2, &snap);
    e2.set_environment(vec![(e2.values().field("ts"), e2.values().int(2))]);
    e2.evaluate_sequential().unwrap();
    assert_eq!(doc.result(&eng), d2.result(&e2));
}

#[test]
fn a_claim_growing_over_a_page_makes_that_page_absent_as_an_anchor() {
    // Found by the typing benchmark's checked edits (shrunk to 24 lines):
    // `sumnext ... 21` takes the `page` at line 9; the last page's segment
    // must then start where the previous live page is, as the interpreter
    // has it, not at the inert one.
    let src = [
        "emit x184_5 + 67",
        "sumnext x184_8 2",
        "x184_19 = 28",
        "~say x184_20 x184_19",
        "p184_21 = {x: 2, w: 28, c: 8}",
        "x184_21 = layout(p184_21)",
        "~dbl x184_22 x184_21",
        "x184_23 = x184_19 + 28",
        "end",
        "page",
        "begin",
        "x185_0 = g29 + 92",
        "~dbl x185_1 g9",
        "emit g18 + 0",
        "x188_19 = x188_15 + 11",
        "s188_20 = \"e188_20 = \"",
        "eval s188_20 + g23",
        "x188_20 = e188_20 + 1",
        "x188_21 = x188_15 + 25",
        "x188_22 = x188_18 * 2",
        "p188_23 = {x: 86, w: 26, c: 3}",
        "x188_23 = layout(p188_23)",
        "end",
        "page",
    ];
    let (mut eng, mut prog, _) = load(&src);
    prog.set_line(&mut eng, 1, "sumnext x184_8 21".into());
    eval(&mut eng, &prog);
    let (mut cold, p2) = Program::load(prog.lines.clone());
    cold.evaluate_sequential().unwrap();
    assert_eq!(p2.result(&cold), reference(&prog.lines), "cold");
}

#[test]
fn an_opener_that_contributes_keeps_its_start_marker() {
    // Reported by the client: an opener step that also contributes to a
    // sequence, followed by a reader of the previous contribution, re-ran
    // without end (binding the opener's marker re-registered it with no
    // value, so every read looked stale). The contribution of an opener is
    // not supported (its marker wins); the reader sees the scope's start.
    use common::trace::{self, Doc};
    let text = concat!(
        "{\"meta\": {\"version\": 1, \"document\": \"m\", \"steps\": 2, \"sequences\": [\"items\"]}}\n",
        "{\"id\": \"a\", \"class\": \"begin\", \"pos\": 0, \"reads\": [], \"writes\": [], \"scope\": \"open\", \"contributes\": \"items\"}\n",
        "{\"id\": \"b\", \"class\": \"x\", \"pos\": 1, \"reads\": [], \"writes\": [], \"reads_prev_contribution\": \"items\"}\n",
    );
    let t = trace::load_trace(text).expect("loads");
    let mut eng = Engine::new();
    let doc = Doc::load(&mut eng, &t);
    let r = eng.evaluate_sequential().unwrap();
    assert!(r.runs <= 4, "runs {}", r.runs);
    let first = doc.result(&eng);
    let r = eng.evaluate_sequential().unwrap();
    assert_eq!(r.nodes_run, 0);
    assert_eq!(doc.result(&eng), first);
}

#[test]
fn the_critical_chain_names_each_wait_and_its_cause() {
    let (eng, prog, r) = load(&["a = 1", "b = a + 1", "c = b + 1", "d = 7"]);
    let ids = prog.node_ids(&eng);
    assert_eq!(r.critical_path, eng.critical_path());
    assert!(r.max_ready >= 1);
    let chain = eng.critical_chain();
    let x = eng.values().field("x");
    let _ = x;
    let names: Vec<NodeId> = chain.iter().map(|c| c.0).collect();
    assert_eq!(names, vec![ids[2], ids[1], ids[0]], "{chain:?}");
    assert!(
        matches!(chain[0].1, defuse::Cause::Changed { site: Some(s), .. } if s == ids[1]),
        "{:?}",
        chain[0]
    );
    assert!(
        matches!(chain[1].1, defuse::Cause::Changed { site: Some(s), .. } if s == ids[0]),
        "{:?}",
        chain[1]
    );
    assert!(
        matches!(chain[2].1, defuse::Cause::Created { by: None, .. }),
        "{:?}",
        chain[2]
    );
}

/// An opener's effect still unknown (it runs again) at the unit's top
/// level may stop opening its scope, whose definitions then reach past its
/// close: a chain read after the close waits for it rather than binding
/// past it, so a batch applying the opener's result first cannot leave the
/// read bound to the old structure. Replacing `scope doc` with a plain
/// line takes `item` out of a list and `para` out of its scope; `before()`
/// then sees the `add 1` the item's scope held.
#[test]
fn a_chain_read_waits_for_a_top_level_opener_that_may_stop_opening() {
    let src = [
        "end",
        "scope doc",
        "cap = 12",
        "item a = prev()",
        "fold \"when k == 2 add 5  global g = k\"",
        "take",
        "add 1",
        "{ add 6  z = prev() }",
        "cap = 9",
        "w = x + 1",
        "{ add 6  z = prev() }",
        "para",
        "eval \"add 4\"",
        "add 3",
        "s = fold.state",
        "y = before()",
        "end",
    ];
    for seed in 0..40 {
        for spec in [false, true] {
            let (mut eng, mut prog) = Program::load(lines(&src));
            eng.evaluate(&mut RandomScheduler::new(seed, spec), &mut Sequential)
                .expect("cold");
            prog.set_line(&mut eng, 1, "cap = 12".to_string());
            eng.evaluate(&mut RandomScheduler::new(seed + 1, spec), &mut Sequential)
                .expect("edit");
            assert_eq!(
                prog.render(&eng),
                {
                    let (mut cold, p) = Program::load(prog.lines.clone());
                    cold.evaluate_sequential().expect("cold");
                    p.render(&cold)
                },
                "seed {seed} spec {spec}"
            );
            assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
        }
    }
}

/// Releases queued line edits to an evaluation between its steps, one
/// per `release` steps on average (applied through the program, so the
/// program's own bookkeeping runs in the callback).
struct Absorber<'a> {
    prog: &'a mut Program,
    edits: std::collections::VecDeque<common::shrink::Ed>,
    rng: Rng,
    applied: usize,
    /// One edit per this many steps, on average (and at the latest after
    /// twice as many calls).
    release: usize,
    held: usize,
    /// Every marked node reported final, and per version the text and
    /// its nodes then (version 0: the loaded text).
    fires: Vec<Settled>,
    texts: Vec<(Vec<String>, Vec<NodeId>)>,
}

impl Absorber<'_> {
    fn edit_log_text(&self) -> Vec<String> {
        self.prog.edit_log.iter().map(|e| format!("{e:?}")).collect()
    }

    fn mark_all(&mut self, eng: &mut Engine) {
        for &n in &self.prog.nodes {
            eng.mark(n);
        }
        self.texts.push((self.prog.lines.clone(), self.prog.nodes.clone()));
    }
}

impl defuse::EditSource for Absorber<'_> {
    fn absorb(&mut self, eng: &mut Engine) -> Option<u64> {
        if self.edits.is_empty() {
            return None;
        }
        self.held += 1;
        if self.rng.below(self.release) != 0 && self.held < 2 * self.release {
            return None;
        }
        self.held = 0;
        let e = self.edits.pop_front().expect("an edit");
        common::shrink::apply(eng, self.prog, &e);
        self.applied += 1;
        self.mark_all(eng);
        // The client's version: the edits applied so far.
        Some(self.applied as u64)
    }

    fn settled(&mut self, _: &Engine, items: &[Settled]) -> bool {
        self.fires.extend_from_slice(items);
        true
    }
}

/// Edits absorbed between the steps of an evaluation leave the result a
/// fresh evaluation of the final text gives, under every scheduler, and
/// the report says which version each step saw.
#[test]
#[allow(clippy::too_many_lines)]
fn edits_absorbed_between_steps_give_the_final_texts_result() {
    // Every statement keeps its state at a stop and resumes from it.
    common::REPLAY_STOPS.store(true, std::sync::atomic::Ordering::Relaxed);
    let pool: Vec<String> = std::fs::read_to_string("examples/fuzz_pool.txt")
        .expect("pool")
        .lines()
        .map(String::from)
        .collect();
    let seeds = std::env::var("DEFUSE_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(12);
    for seed in 0..seeds {
        let mut rng = Rng::new(seed * 7919 + 3);
        // Odd seeds: a generated 300-line program (loops included), whose
        // evaluations have many steps for the edits to land in.
        let src: Vec<String> = if seed % 2 == 0 {
            (0..30).map(|_| pool[rng.below(pool.len())].clone()).collect()
        } else {
            generate(seed, 300)
        };
        let release = if seed % 2 == 0 { 4 } else { 16 };
        for (name, kind) in [("seq", 0), ("spec", 1), ("random", 2)] {
            let (mut eng, mut prog) = Program::load(src.clone());
            let eval = |e: &mut Engine, a: &mut dyn defuse::EditSource, k: u64| match kind {
                0 => e.evaluate_absorbing(&mut Greedy::default(), &mut Sequential, a),
                1 => e.evaluate_absorbing(&mut Greedy::speculative(Speculation::All, 512), &mut Threads(3), a),
                _ => e.evaluate_absorbing(
                    &mut RandomScheduler::new(seed * 31 + k, k.is_multiple_of(2)),
                    &mut Sequential,
                    a,
                ),
            };
            let r0 =
                eval(&mut eng, &mut defuse::NoEdits, 0).unwrap_or_else(|e| panic!("{name} seed {seed} cold: {e:?}"));
            assert_eq!(prog.result(&eng), reference(&prog.lines), "{name} seed {seed} cold");
            assert_eq!(r0.edits_applied, Vec::new());
            assert_eq!(r0.version, 0);
            // Thirty edits, released between steps. The example language
            // declares its loop-back fields when the program loads, so
            // `loop` lines stay.
            let mut edits = std::collections::VecDeque::new();
            {
                let mut lines = prog.lines.clone();
                while edits.len() < 30 {
                    let i = rng.below(lines.len() + 1);
                    let e = match rng.below(3) {
                        0 if i < lines.len() => common::shrink::Ed::Rm(i),
                        1 if i < lines.len() => common::shrink::Ed::Set(i, pool[rng.below(pool.len())].clone()),
                        _ => common::shrink::Ed::Ins(i.min(lines.len()), pool[rng.below(pool.len())].clone()),
                    };
                    if !matches!(e, common::shrink::Ed::Ins(..)) && lines[i].starts_with("loop ") {
                        continue;
                    }
                    match &e {
                        common::shrink::Ed::Rm(i) => {
                            lines.remove(*i);
                        }
                        common::shrink::Ed::Set(i, l) => lines[*i] = l.clone(),
                        common::shrink::Ed::Ins(i, l) => lines.insert(*i, l.clone()),
                    }
                    edits.push_back(e);
                }
            }
            let mut a = Absorber {
                prog: &mut prog,
                edits,
                rng: Rng::new(seed + 1000),
                applied: 0,
                release,
                held: 0,
                fires: Vec::new(),
                texts: Vec::new(),
            };
            a.mark_all(&mut eng);
            let mut k = 1;
            let mut versions = Vec::new();
            let mut resettled = 0;
            let mut resumed = 0;
            while !a.edits.is_empty() || k == 1 {
                let r = eval(&mut eng, &mut a, k).unwrap_or_else(|e| panic!("{name} seed {seed}: {e:?}"));
                versions.push((r.version, r.edits_applied.clone()));
                resettled += r.resettled;
                resumed += r.resumed + r.restarted;
                k += 1;
                assert!(k <= 5000, "{name} seed {seed}: edits never released");
            }
            assert_eq!(a.applied, 30);
            let last = versions.last().expect("a report");
            assert_eq!(last.0, 30, "{name} seed {seed}: version {versions:?}");
            let absorbed: usize = versions.iter().map(|v| v.1.len()).sum();
            assert_eq!(absorbed, 30, "{name} seed {seed}: {versions:?}");
            // Each batch carries the client's tag; tags never decrease.
            let tags: Vec<u64> = versions.iter().flat_map(|v| v.1.iter().map(|b| b.0)).collect();
            assert!(tags.windows(2).all(|w| w[0] < w[1]), "{name} seed {seed}: {tags:?}");
            // Each single edit, applied to a cold evaluation of the
            // previous text, matches the interpreter (the reports at the
            // intermediate versions are checked against it below).
            for (v, e) in a.prog.edit_log.clone().iter().enumerate() {
                let (mut e1, mut p1) = Program::load(a.texts[v].0.clone());
                e1.evaluate_sequential().expect("cold");
                common::shrink::apply(&mut e1, &mut p1, e);
                e1.evaluate_sequential().expect("after the edit");
                assert_eq!(p1.lines, a.texts[v + 1].0, "{name} seed {seed}: text {}", v + 1);
                if p1.result(&e1) != reference(&p1.lines) {
                    std::fs::create_dir_all("target/single-edit-failure").expect("dir");
                    std::fs::write("target/single-edit-failure/program.txt", a.texts[v].0.join("\n") + "\n")
                        .expect("w");
                    std::fs::write(
                        "target/single-edit-failure/edits.txt",
                        common::shrink::edits_text(std::slice::from_ref(e)),
                    )
                    .expect("w");
                }
                assert_eq!(
                    p1.result(&e1),
                    reference(&p1.lines),
                    "{name} seed {seed}: single edit {v} -> version {} ({e:?})",
                    v + 1
                );
            }
            // A node reported final at a version has the output the
            // interpreter gives for the text at that version, and no
            // report was ever corrected at its version.
            if resettled != 0 {
                let mut seen: std::collections::BTreeMap<(NodeId, u64), Vec<ValueId>> =
                    std::collections::BTreeMap::default();
                for f in &a.fires {
                    seen.entry((f.node, f.version)).or_default().push(f.output);
                }
                for (&(n, v), outs) in &seen {
                    if outs.len() > 1 {
                        let (lines, nodes) = &a.texts[usize::try_from(v).expect("version")];
                        let i = nodes.iter().position(|&x| x == n);
                        eprintln!(
                            "{name} seed {seed}: node {n:?} version {v} line {i:?} {:?}: {:?}\ntext:\n{}",
                            i.map(|i| lines[i].clone()),
                            outs.iter().map(|&o| eng.values().render(o)).collect::<Vec<_>>(),
                            lines.join("\n")
                        );
                    }
                }
                let order: Vec<String> = a.edit_log_text();
                eprintln!("edits:\n{}", order.join("\n"));
                let bad: Vec<NodeId> = seen.iter().filter(|(_, o)| o.len() > 1).map(|(k, _)| k.0).collect();
                for n in bad {
                    let hist: Vec<String> = a
                        .fires
                        .iter()
                        .filter(|f| f.node == n)
                        .map(|f| format!("v{} {}", f.version, eng.values().render(f.output)))
                        .collect();
                    let expect: Vec<String> = a
                        .texts
                        .iter()
                        .enumerate()
                        .map(|(v, (lines, nodes))| {
                            nodes.iter().position(|&x| x == n).map_or("-".into(), |i| {
                                format!("v{v} {} = {} (line {i})", lines[i], refs_of(lines)[i])
                            })
                        })
                        .collect();
                    eprintln!("history of {n:?}: {hist:?}\nexpected: {expect:#?}");
                }
            }
            assert_eq!(resettled, 0, "{name} seed {seed}: corrected reports");
            // The replaying statements did keep and take up their state.
            assert!(
                kind != 0 || seed % 2 == 0 || resumed > 0,
                "{name} seed {seed}: nothing resumed"
            );
            let refs: Vec<Vec<u64>> = a.texts.iter().map(|t| reference(&t.0)).collect();
            assert!(!a.fires.is_empty(), "{name} seed {seed}: nothing reported final");
            for f in &a.fires {
                let v = usize::try_from(f.version).expect("version");
                let (lines, nodes) = &a.texts[v];
                let i = nodes
                    .iter()
                    .position(|&n| n == f.node)
                    .unwrap_or_else(|| panic!("{name} seed {seed}: {f:?} is not a line of version {v}"));
                if f.output.raw() != refs[v][i] {
                    let hist: Vec<String> = a
                        .fires
                        .iter()
                        .filter(|g| g.node == f.node)
                        .map(|g| format!("v{} {}", g.version, eng.values().render(g.output)))
                        .collect();
                    let expect: Vec<String> = a
                        .texts
                        .iter()
                        .enumerate()
                        .map(|(v, (lines, nodes))| {
                            nodes.iter().position(|&x| x == f.node).map_or("-".into(), |i| {
                                format!("v{v} {} = {} (line {i})", lines[i], refs_of(lines)[i])
                            })
                        })
                        .collect();
                    eprintln!(
                        "node {:?} history: {hist:?}\nexpected: {expect:#?}\nloops: {:?}\ntext:\n{}",
                        f.node,
                        a.prog.loops,
                        lines.join("\n")
                    );
                }
                assert_eq!(
                    f.output.raw(),
                    refs[v][i],
                    "{name} seed {seed}: line {i} of version {v} ({:?}) reported {:?}",
                    lines[i],
                    eng.values().render(f.output)
                );
            }
            // The final text: every line reported at the last version.
            let last_fired: std::collections::BTreeSet<NodeId> =
                a.fires.iter().filter(|f| f.version == 30).map(|f| f.node).collect();
            for (i, n) in prog.nodes.iter().enumerate() {
                if eng.output(*n).is_some() {
                    assert!(
                        last_fired.contains(n),
                        "{name} seed {seed}: line {i} not reported at version 30"
                    );
                }
            }
            let got = prog.result(&eng);
            assert_eq!(
                got,
                reference(&prog.lines),
                "{name} seed {seed}: result after absorbing"
            );
            assert!(
                eng.check_invariants().is_empty(),
                "{name} seed {seed}: {:?}",
                eng.check_invariants()
            );
            let (mut cold, p) = Program::load(prog.lines.clone());
            cold.evaluate_sequential().expect("cold");
            assert_eq!(got, p.result(&cold), "{name} seed {seed}: against a cold evaluation");
        }
    }
}

/// A loop-back field whose only publisher goes away (here: claimed by a
/// list that lost its end) takes its initial value again, as a cold
/// evaluation of the new text gives.
#[test]
fn a_loop_back_field_without_publishers_returns_to_its_initial_value() {
    let (mut eng, mut prog, _) = load(&["loop fwd = 0", "early = fwd + 1", "listto x", "endlist", "fwd = 3"]);
    assert_eq!(out(&eng, &prog, 1), "4");
    prog.remove_line(&mut eng, 3);
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 1), "1");
    prog.insert_line(&mut eng, 3, "endlist".to_string());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 1), "4");
}

/// Every step that applied runs is counted once in the batch-width
/// histogram, in the power-of-two bucket of its width.
#[test]
fn the_report_counts_steps_by_the_width_of_what_they_applied() {
    let (eng, _, r) = load(&["x = 1", "y = 2", "z = x + y", "w = z + 1"]);
    let steps: u64 = r.applied_widths.iter().sum();
    assert!(steps >= 2 && steps <= r.steps, "{r:?}");
    assert_eq!(r.applied_widths.len(), 2, "{:?}", r.applied_widths);
    assert!(r.applied_widths[1] >= 1, "{:?}", r.applied_widths);
    drop(eng);
}

/// Reports marked nodes final as they come, with the absorb call each
/// report came before.
#[derive(Default)]
struct Recorder {
    calls: usize,
    fires: Vec<(usize, Settled)>,
}

impl defuse::EditSource for Recorder {
    fn absorb(&mut self, _: &mut Engine) -> Option<u64> {
        self.calls += 1;
        None
    }

    fn settled(&mut self, _: &Engine, items: &[Settled]) -> bool {
        self.fires.extend(items.iter().map(|&s| (self.calls, s)));
        true
    }
}

/// A marked node is reported final, with the text's version, once the
/// evaluation's level passes it: the first node of a chain before the
/// evaluation ends, the last one at its end. Every marked node is
/// reported again at a new version, and after an edit with its new
/// output; nothing is reported twice for one version and output.
#[test]
fn marked_nodes_are_reported_final_as_the_evaluation_passes_them() {
    let (mut eng, mut prog) = Program::load(lines(&["x = 3", "y = x + 1", "z = y + 1"]));
    for &n in &prog.nodes {
        eng.mark(n);
    }
    eng.set_version(7);
    let mut rec = Recorder::default();
    let r = eng
        .evaluate_absorbing(&mut Greedy::default(), &mut Sequential, &mut rec)
        .expect("evaluate");
    assert_eq!(prog.result(&eng), reference(&prog.lines));
    assert_eq!((r.settled, r.resettled), (3, 0));
    let order: Vec<NodeId> = rec.fires.iter().map(|f| f.1.node).collect();
    assert_eq!(order, prog.nodes, "{:?}", rec.fires);
    assert!(rec.fires.iter().all(|f| f.1.version == 7), "{:?}", rec.fires);
    assert!(rec.fires[0].0 < rec.fires[2].0, "x before the end: {:?}", rec.fires);
    assert_eq!(rec.fires[0].0, 2, "x at the second step: {:?}", rec.fires);
    assert!(
        rec.fires[2].0 < rec.calls,
        "z before the last absorb call: {:?}",
        rec.fires
    );
    for (i, &n) in prog.nodes.iter().enumerate() {
        assert_eq!(Some(rec.fires[i].1.output), eng.output(n));
    }
    assert_eq!(eng.values().render(rec.fires[2].1.output), "5");
    // The same version again: nothing new.
    let mut again = Recorder::default();
    let r = eng
        .evaluate_absorbing(&mut Greedy::default(), &mut Sequential, &mut again)
        .expect("evaluate");
    assert_eq!(r.settled, 0);
    assert_eq!(again.fires, Vec::new());
    // A new version: every marked node, the outputs unchanged.
    eng.set_version(8);
    let mut v8 = Recorder::default();
    let r = eng
        .evaluate_absorbing(&mut Greedy::default(), &mut Sequential, &mut v8)
        .expect("evaluate");
    assert_eq!((r.settled, r.resettled), (3, 0));
    assert!(v8.fires.iter().all(|f| f.1.version == 8), "{:?}", v8.fires);
    assert_eq!(
        v8.fires.iter().map(|f| f.1.output).collect::<Vec<_>>(),
        rec.fires.iter().map(|f| f.1.output).collect::<Vec<_>>()
    );
    // An edit: new outputs, at the new version.
    prog.set_line(&mut eng, 0, "x = 10".to_string());
    eng.set_version(9);
    let mut v9 = Recorder::default();
    let r = eng
        .evaluate_absorbing(&mut Greedy::default(), &mut Sequential, &mut v9)
        .expect("evaluate");
    assert_eq!(prog.result(&eng), reference(&prog.lines));
    assert_eq!((r.settled, r.resettled), (3, 0));
    assert_eq!(
        v9.fires
            .iter()
            .map(|f| eng.values().render(f.1.output))
            .collect::<Vec<_>>(),
        ["10", "11", "12"]
    );
    // Unmarked nodes are not reported; a source that does not take the
    // reports leaves them to `take_settled`.
    eng.unmark(prog.nodes[2]);
    eng.set_version(10);
    let r = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(r.settled, 2);
    let kept = eng.take_settled();
    assert_eq!(kept.iter().map(|s| s.node).collect::<Vec<_>>(), prog.nodes[..2]);
    assert!(kept.iter().all(|s| s.version == 10));
    assert_eq!(eng.take_settled(), Vec::new());
}

/// `mark_op` marks every node an operation creates from then on, and an
/// edit queue carries marks and the reports between threads.
#[test]
fn an_edit_queue_carries_marks_and_the_reports_of_final_nodes() {
    let (mut eng, prog) = Program::load(lines(&["x = 3", "y = x + 1"]));
    let queue = defuse::EditQueue::new();
    let mut q = queue.clone();
    queue.push(
        defuse::Edit::Mark {
            node: prog.nodes[1],
            on: true,
        },
        1,
    );
    let r = eng
        .evaluate_absorbing(&mut Greedy::default(), &mut Sequential, &mut q)
        .expect("evaluate");
    assert_eq!((r.version, r.settled), (1, 1));
    let got = queue.take_settled();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!((got[0].node, got[0].version), (prog.nodes[1], 1));
    assert_eq!(eng.values().render(got[0].output), "4");
    assert_eq!(queue.take_settled(), Vec::new());
}

/// A range reader anchored on a previous sibling reads from the unit's
/// start once that sibling is removed.
#[test]
fn a_range_reader_whose_anchor_is_removed_reads_from_the_start() {
    let (mut eng, mut prog, _) = load(&["emit x2_3 + 82", "page", "page"]);
    assert_eq!(out(&eng, &prog, 2), "[0 0]");
    prog.remove_line(&mut eng, 1);
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 1), "[1 0]");
}

/// A range reader anchored on a previous sibling chosen by its text
/// re-anchors when that sibling's text changes.
#[test]
fn a_range_reader_whose_anchor_is_replaced_re_anchors() {
    let (mut eng, mut prog, _) = load(&["emit g21 + 84", "page", "page"]);
    assert_eq!(out(&eng, &prog, 2), "[0 0]");
    prog.set_line(&mut eng, 1, "s = fold.state".into());
    eval(&mut eng, &prog);
    assert_eq!(out(&eng, &prog, 2), "[1 0]");
}

/// `["set", k]` defines `p` as `k` (declared by its text); `["clr"]`
/// defines `p` as unit; `["w"]` may define `c` (a dynamic set bounded by
/// its text), reads `p`, and when `p` is unit narrows its set to nothing,
/// then outputs `c`; otherwise it defines `c` as `p`.
struct Narrowing;

impl defuse::Operation for Narrowing {
    fn defs(&self, a: &[ValueId], v: &defuse::Values) -> Option<Vec<(Field, defuse::Locality)>> {
        match narrowing_tag(v, a).as_str() {
            "set" | "clr" => Some(vec![(v.field("p"), defuse::Locality::Local)]),
            _ => None,
        }
    }
    fn may_define(&self, _: &[ValueId], v: &defuse::Values) -> defuse::MayDefine {
        defuse::MayDefine::Names(vec![v.field("c")])
    }
    fn run(&self, cx: &mut defuse::Ctx<'_>) -> Result<ValueId, defuse::Stop> {
        let v = cx.values();
        let a = cx.args();
        match narrowing_tag(v, &a).as_str() {
            "set" => {
                cx.define(v.field("p"), a[1]);
                Ok(a[1])
            }
            "clr" => {
                cx.define(v.field("p"), ValueId::UNIT);
                Ok(ValueId::UNIT)
            }
            _ => {
                let p = cx.use_field(v.field("p"))?.unwrap_or(ValueId::UNIT);
                if p == ValueId::UNIT {
                    cx.narrow(defuse::MayDefine::Nothing);
                    Ok(cx.use_field(v.field("c"))?.unwrap_or(ValueId::UNIT))
                } else {
                    cx.define(v.field("c"), p);
                    Ok(p)
                }
            }
        }
    }
}

fn narrowing_tag(v: &defuse::Values, a: &[ValueId]) -> String {
    match a.first().map(|&x| v.get(x)).as_deref() {
        Some(defuse::Value::Str(s)) => s.to_string(),
        _ => String::new(),
    }
}

/// A node that learns from its first read that it defines nothing narrows
/// its declared set: the readers after it stop waiting on it. 400 nodes,
/// `p` set by every 10th and cleared by the next: without narrowing every
/// `w` waits on the one before; with it the critical path is constant,
/// with no refutation, and the results are those of every schedule.
#[test]
fn a_run_narrowing_its_definition_set_releases_the_readers_after_it() {
    let build = || {
        let mut eng = Engine::new();
        let op = eng.register(std::sync::Arc::new(Narrowing));
        let u = eng.new_unit();
        let mut ids = Vec::new();
        for i in 0..400i64 {
            let v = eng.values();
            let a = match i % 10 {
                0 => vec![v.str("set"), v.int(i)],
                1 => vec![v.str("clr")],
                _ => vec![v.str("w")],
            };
            ids.push(eng.push(u, op, a));
        }
        (eng, op, ids)
    };
    let mut outs = Vec::new();
    for sched in 0..4 {
        let (mut eng, op, ids) = build();
        let r = match sched {
            0 => eng.evaluate_sequential(),
            1 => eng.evaluate(&mut RandomScheduler::new(7, true), &mut Sequential),
            2 => eng.evaluate(&mut Greedy::speculative(Speculation::All, 64), &mut Threads(3)),
            _ => eng.evaluate(&mut Greedy::default(), &mut Threads(4)),
        }
        .expect("evaluate");
        assert_eq!(r.declarations_refuted, 0, "schedule {sched}: {r:?}");
        if sched == 0 {
            assert!(eng.critical_path() <= 6, "critical path {}", eng.critical_path());
        }
        // An edit: a `w` sees `p` set and defines `c`; the readers after it
        // in its group read it.
        let v = eng.values();
        let a = vec![v.str("set"), v.int(1000)];
        eng.replace(ids[51], op, a);
        let r2 = match sched {
            0 => eng.evaluate_sequential(),
            1 => eng.evaluate(&mut RandomScheduler::new(8, true), &mut Sequential),
            2 => eng.evaluate(&mut Greedy::speculative(Speculation::All, 64), &mut Threads(3)),
            _ => eng.evaluate(&mut Greedy::default(), &mut Threads(4)),
        }
        .expect("evaluate");
        assert_eq!(r2.declarations_refuted, 0, "schedule {sched} after the edit: {r2:?}");
        outs.push(ids.iter().map(|&i| eng.output(i)).collect::<Vec<_>>());
    }
    assert!(outs.windows(2).all(|w| w[0] == w[1]), "schedules differ");
    let v = build().0.values().int(1000);
    assert_eq!(outs[0][55], Some(v), "a w after the new set reads its c");
}

/// Each node that runs again reports why: its own text, or the value it
/// read changing (with the old and new values).
#[test]
fn rerun_causes_name_the_text_and_the_changed_value() {
    let (mut eng, mut prog) = Program::load(lines(&["a = 1", "b = a + 1", "c = b"]));
    eng.evaluate_sequential().unwrap();
    prog.set_line(&mut eng, 0, "a = 5".into());
    let r = eng.evaluate_sequential().unwrap();
    let why = |i: usize| r.rerun_causes.iter().find(|c| c.0 == prog.nodes[i]).map(|c| c.1);
    assert_eq!(why(0), Some(defuse::Rerun::Text), "{:?}", r.rerun_causes);
    let Some(defuse::Rerun::Changed { old, new, .. }) = why(1) else {
        panic!("{:?}", r.rerun_causes)
    };
    let v = eng.values();
    assert_eq!((v.as_int(old), v.as_int(new)), (Some(1), Some(5)));
    assert!(
        matches!(why(2), Some(defuse::Rerun::Changed { .. })),
        "{:?}",
        r.rerun_causes
    );
}

/// `mark_op` marks the nodes its operation creates from then on, unfolded
/// children too: each is reported settled once final.
#[test]
fn mark_op_reports_the_nodes_it_creates() {
    struct Op(std::sync::OnceLock<defuse::OpId>);
    impl defuse::Operation for Op {
        fn defs(&self, _: &[defuse::ValueId], _: &defuse::Values) -> Option<Vec<(defuse::Field, defuse::Locality)>> {
            Some(Vec::new())
        }
        fn run(&self, cx: &mut defuse::Ctx<'_>) -> Result<defuse::ValueId, defuse::Stop> {
            let a = cx.args();
            if cx.values().as_int(a[0]) == Some(0) {
                for k in 1..4 {
                    let x = cx.values().int(k);
                    cx.child(*self.0.get().expect("registered"), vec![x]);
                }
            }
            Ok(a[0])
        }
    }
    let mut eng = Engine::new();
    let o = std::sync::Arc::new(Op(std::sync::OnceLock::new()));
    let op = eng.register(o.clone());
    o.0.set(op).expect("once");
    eng.mark_op(op);
    let u = eng.new_unit();
    let z = eng.values().int(0);
    eng.push(u, op, vec![z]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(eng.take_settled().len(), 4);
}

/// A rerun that newly defines a scope-local, its scope effect the same as
/// before: the local ends with the scope. S opens a scope (decided when it
/// runs) and defines `x` in it only when `c` is 1; a reader after the
/// scope's close sees the outer `x`, before and after `c` changes.
#[test]
fn a_reruns_new_scope_local_ends_with_its_scope() {
    use defuse::{Effect, Field, Locality, Stop, ValueId, Values};
    struct Op;
    fn tag(v: &Values, a: &[ValueId]) -> String {
        v.as_str(a[0]).unwrap_or_default()
    }
    impl defuse::Operation for Op {
        fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
            match tag(v, a).as_str() {
                "c" => Some(vec![(v.field("c"), Locality::Local)]),
                "x" => Some(vec![(v.field("x"), Locality::Local)]),
                "s" => Some(vec![(v.field("x"), Locality::Scope)]),
                _ => Some(Vec::new()),
            }
        }
        fn effect(&self, a: &[ValueId], v: &Values) -> Effect {
            match tag(v, a).as_str() {
                "s" => Effect::Pending,
                "close" => Effect::Close,
                _ => Effect::None,
            }
        }
        fn run(&self, cx: &mut defuse::Ctx<'_>) -> Result<ValueId, Stop> {
            let v = cx.values();
            let a = cx.args();
            match tag(v, &a).as_str() {
                "c" | "x" => {
                    let f = v.field(&tag(v, &a));
                    cx.define(f, a[1]);
                    Ok(a[1])
                }
                "s" => {
                    cx.set_effect(Effect::Open);
                    let c = cx.use_field(v.field("c"))?.and_then(|x| v.as_int(x));
                    if c == Some(1) {
                        cx.define_in_scope(v.field("x"), v.int(99));
                    }
                    Ok(ValueId::UNIT)
                }
                "read" => Ok(cx.use_field(v.field("x"))?.unwrap_or(ValueId::UNIT)),
                _ => Ok(ValueId::UNIT),
            }
        }
    }
    for threads in [0usize, 4] {
        let mut eng = Engine::new();
        let op = eng.register(std::sync::Arc::new(Op));
        let u = eng.new_unit();
        let s = |e: &Engine, t: &str| e.values().str(t);
        let (cs, xs) = (s(&eng, "c"), s(&eng, "x"));
        let (zero, one) = (eng.values().int(0), eng.values().int(1));
        let c = eng.push(u, op, vec![cs, zero]);
        eng.push(u, op, vec![xs, one]);
        let sn = s(&eng, "s");
        eng.push(u, op, vec![sn]);
        let close = s(&eng, "close");
        eng.push(u, op, vec![close]);
        let rd = s(&eng, "read");
        let r = eng.push(u, op, vec![rd]);
        let eval = |e: &mut Engine| {
            if threads == 0 {
                e.evaluate_sequential()
            } else {
                e.evaluate(&mut Greedy::default(), &mut Threads(threads))
            }
        };
        eval(&mut eng).unwrap();
        assert_eq!(eng.output(r).and_then(|x| eng.values().as_int(x)), Some(1), "threads {threads}: cold");
        eng.replace(c, op, vec![cs, one]);
        eval(&mut eng).unwrap();
        assert_eq!(eng.output(r).and_then(|x| eng.values().as_int(x)), Some(1), "threads {threads}: after the rerun");
        assert!(eng.check_reads().is_empty(), "threads {threads}: {:?}", eng.check_reads());
    }
}

/// A final node that declares it defines and contributes nothing, placed
/// between an accumulator's contributor and a range reader: the reader does
/// not wait for it (it is held until the final phase), and a final node the
/// final phase itself places runs too.
#[test]
fn a_final_node_declaring_nothing_holds_no_reader() {
    use defuse::{Field, Locality, Stop, ValueId, Values};
    struct Op(std::sync::OnceLock<defuse::OpId>);
    fn tag(v: &Values, a: &[ValueId]) -> String {
        v.as_str(a[0]).unwrap_or_default()
    }
    impl defuse::Operation for Op {
        fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
            Some(Vec::new())
        }
        fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
            match tag(v, a).as_str() {
                "add" => Some(vec![(v.field("acc"), v.int(2))]),
                _ => Some(Vec::new()),
            }
        }
        fn final_only(&self, a: &[ValueId], v: &Values) -> bool {
            matches!(tag(v, a).as_str(), "page" | "late" | "spawner")
        }
        fn run(&self, cx: &mut defuse::Ctx<'_>) -> Result<ValueId, Stop> {
            let v = cx.values();
            let a = cx.args();
            let acc = v.field("acc");
            match tag(v, &a).as_str() {
                "add" => {
                    cx.contribute(acc, v.int(2));
                    Ok(ValueId::UNIT)
                }
                "line" => {
                    // Places a final page (held until the final phase).
                    cx.child(*self.0.get().expect("registered"), vec![v.str("page")]);
                    Ok(ValueId::UNIT)
                }
                "spawner" => {
                    // A final node placing another final node.
                    cx.child(*self.0.get().expect("registered"), vec![v.str("late")]);
                    Ok(ValueId::UNIT)
                }
                "read" => cx.acc_range(acc, None),
                _ => Ok(ValueId::UNIT),
            }
        }
    }
    let mut eng = Engine::new();
    let o = std::sync::Arc::new(Op(std::sync::OnceLock::new()));
    let op = eng.register(o.clone());
    o.0.set(op).expect("once");
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, std::sync::Arc::new(defuse::SumMonoid));
    let u = eng.new_unit();
    let t = |e: &Engine, s: &str| e.values().str(s);
    let (add, line, read, spawner) = (t(&eng, "add"), t(&eng, "line"), t(&eng, "read"), t(&eng, "spawner"));
    eng.push(u, op, vec![add]);
    eng.push(u, op, vec![line]);
    let r = eng.push(u, op, vec![read]);
    let sp = eng.push(u, op, vec![spawner]);
    let rep = eng.evaluate_sequential().expect("no stall");
    assert_eq!(eng.output(r).and_then(|x| eng.values().as_int(x)), Some(2));
    // (The reader never waited on the held page.)
    assert_eq!(rep.stops, 0, "{rep:?}");
    // The page the final spawner placed ran.
    let kids = eng.children(sp);
    assert_eq!(kids.len(), 1);
    assert!(eng.output(kids[0]).is_some(), "the late final node ran");
}
