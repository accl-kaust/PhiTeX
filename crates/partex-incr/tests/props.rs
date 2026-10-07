//! Property tests: random programs and random edits, every accelerated
//! path against the stub's independent interpreter, byte for byte
//! (`DESIGN.md` §7.12, edit fuzzing). A hand-rolled seeded generator
//! keeps the workspace free of dependencies; a failure names its seed.

use std::fmt::Write as _;

use partex_incr::build::{self, Build};
use partex_incr::rounds::{self, Holes};
use partex_incr::stub::generate::{self, Rng, Shape};
use partex_incr::stub::{Cell, Program, oracle};
use partex_incr::{Executor, Machine, Sequential, run_plain};

#[cfg(feature = "threads")]
use partex_incr::Threads;

fn shape(r: &mut Rng, acc: bool) -> Shape {
    Shape {
        vars: 2 + r.below(10),
        labels: 1 + r.below(4),
        work: 0,
        raw: r.below(3) > 0,
        acc,
    }
}

fn random_program(seed: u64, max_len: usize) -> (Rng, Shape, Program) {
    random_program_with(seed, max_len, false)
}

/// A random program, with the accumulator (`mark`, `fonts`) if `acc`.
fn random_program_with(seed: u64, max_len: usize, acc: bool) -> (Rng, Shape, Program) {
    let mut r = Rng(seed);
    let s = shape(&mut r, acc);
    let len = r.below(max_len);
    let p = Program::from_text(&generate::program(&mut r, len, &s));
    (r, s, p)
}

/// Every switch of incremental builds, and a few grains.
/// The program's lines, to take an edit back ([`revert`]).
fn lines(p: &Program) -> Vec<(u32, String)> {
    p.ids()
        .into_iter()
        .map(|id| (id, p.line(id).to_owned()))
        .collect()
}

/// Take back the one-line edit that made `p` from `before`: the cells
/// changed.
fn revert(p: &mut Program, before: &[(u32, String)]) -> Vec<Cell> {
    let now = lines(p);
    match now.len().cmp(&before.len()) {
        core::cmp::Ordering::Equal => match now.iter().zip(before).find(|(a, b)| a.1 != b.1) {
            Some((_, (id, text))) => p.replace(*id, text),
            None => Vec::new(),
        },
        core::cmp::Ordering::Greater => {
            let (id, _) = now
                .iter()
                .find(|(id, _)| before.iter().all(|(b, _)| b != id))
                .expect("the line inserted");
            p.delete(*id)
        }
        core::cmp::Ordering::Less => {
            let i = before
                .iter()
                .position(|(id, _)| now.iter().all(|(n, _)| n != id))
                .expect("the line deleted");
            let after = i.checked_sub(1).map(|j| before[j].0);
            p.insert_after(after, &before[i].1)
        }
    }
}

/// What a burst of typing does: a random edit, or (one time in three)
/// the last edit taken back. `last` is the program before the last edit.
fn burst_edit(r: &mut Rng, p: &mut Program, s: &Shape, last: &mut Vec<(u32, String)>) -> Vec<Cell> {
    let before = lines(p);
    let changed = if !last.is_empty() && r.below(3) == 0 {
        revert(p, last)
    } else {
        generate::edit(r, p, s)
    };
    *last = before;
    changed
}

fn build_configs() -> Vec<build::Config> {
    let d = build::Config {
        sanitize: true,
        ..build::Config::default()
    };
    vec![
        d.clone(),
        build::Config {
            grain: 1,
            fine_grain: 1,
            ..d.clone()
        },
        build::Config {
            grain: 7,
            fine_grain: 3,
            ..d.clone()
        },
        build::Config {
            early_cutoff: false,
            ..d.clone()
        },
        build::Config {
            readset_cutoff: false,
            grain: 5,
            ..d.clone()
        },
        build::Config {
            flat: false,
            grain: 4,
            ..d.clone()
        },
        build::Config {
            incremental: false,
            ..d
        },
    ]
}

fn plain(p: &Program) -> (partex_incr::Version, Vec<u8>) {
    let (m, out) = run_plain(p.machine());
    (m.digest(), out)
}

#[test]
fn plain_and_recorded_runs_match_the_oracle() {
    for seed in 0..400 {
        let (_, _, p) = random_program(seed, 120);
        let want = oracle(&p);
        let (digest, out) = plain(&p);
        assert_eq!(out, want, "plain run, seed {seed}\n{}", p.text());
        let cfg = build::Config {
            grain: 1 + seed % 9,
            ..build::Config::default()
        };
        let b = Build::new(p.machine(), &cfg);
        assert_eq!(b.output(&Sequential), want, "recorded build, seed {seed}");
        #[cfg(feature = "threads")]
        assert_eq!(b.output(&Threads(4)), want, "parallel link, seed {seed}");
        assert_eq!(b.final_state().digest(), digest, "final state, seed {seed}");
    }
}

#[test]
fn incremental_rebuilds_match_scratch() {
    incremental_rebuilds(false);
}

/// As [`incremental_rebuilds_match_scratch`], with an accumulating cell
/// (TeX's glyphs used): a region kept from an older run knows what it
/// added, not the whole value it left.
#[test]
fn incremental_rebuilds_with_an_accumulator_match_scratch() {
    incremental_rebuilds(true);
}

fn incremental_rebuilds(acc: bool) {
    // (with the accumulator, longer runs of edits, some taken back: a
    // glyph added and taken away again)
    let edits = if acc { 24 } else { 8 };
    let mut checked = 0;
    for seed in 0..150 {
        for (ci, cfg) in build_configs().iter().enumerate() {
            let (mut r, s, mut p) = random_program_with(seed, 90, acc);
            let mut b = Build::new(p.machine(), cfg);
            let mut last = Vec::new();
            for e in 0..edits {
                let changed = if acc {
                    burst_edit(&mut r, &mut p, &s, &mut last)
                } else {
                    generate::edit(&mut r, &mut p, &s)
                };
                // `sanitize` also compares against a fresh build.
                b.rebuild(p.machine(), &changed, cfg);
                let (digest, want) = plain(&p);
                assert_eq!(oracle(&p), want);
                assert_eq!(
                    b.output(&Sequential),
                    want,
                    "seed {seed}, config {ci}, edit {e}\n{}",
                    p.text()
                );
                assert_eq!(
                    b.final_state().digest(),
                    digest,
                    "seed {seed}, config {ci}, edit {e}"
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 150 * build_configs().len() * edits);
}

#[test]
fn several_edits_at_once() {
    for seed in 0..200 {
        let (mut r, s, mut p) = random_program(seed + 10_000, 60);
        let cfg = build::Config {
            sanitize: true,
            grain: 4,
            fine_grain: 2,
            ..build::Config::default()
        };
        let mut b = Build::new(p.machine(), &cfg);
        for _ in 0..3 {
            let mut changed = Vec::new();
            for _ in 0..=r.below(5) {
                changed.extend(generate::edit(&mut r, &mut p, &s));
            }
            b.rebuild(p.machine(), &changed, &cfg);
            assert_eq!(b.output(&Sequential), oracle(&p), "seed {seed}");
        }
    }
}

/// Rebuilds stopped early (a newer edit arriving, `DESIGN.md` §7.11),
/// each followed by another edit or none (a file whose time alone
/// changed), then one run to its end, and regions merged while no edit
/// waits, as a watch does: the same as a build from scratch (the
/// sanitizer compares them too).
#[test]
fn stopped_rebuilds_then_one_to_the_end_match_scratch() {
    let mut stopped = 0;
    // (stopped inside a span, which was given up)
    let mut inside = 0;
    let mut in_a_row = 0;
    // (links taken again: the build said no effect changed)
    let mut same = 0;
    for seed in 0..300 {
        for (ci, cfg) in build_configs().iter().enumerate() {
            // (every other program with the accumulator)
            let (mut r, s, mut p) = random_program_with(seed + 20_000, 90, seed % 2 == 1);
            let mut b = Build::new(p.machine(), cfg);
            let mut last = Vec::new();
            assert!(b.take_effects_changed());
            // (the effects, in program order, as a link that reads only
            // them takes them: TeX's)
            let effects = |b: &Build<_>| -> Vec<_> {
                b.traces().flat_map(|t| t.effects.iter().cloned()).collect()
            };
            let mut linked = effects(&b);
            for e in 0..6 {
                // (a few rebuilds that stop after some looks, then one that
                // does not)
                let mut run = 0;
                for _ in 0..r.below(5) {
                    let changed = if !b.settled() && r.below(4) == 0 {
                        Vec::new()
                    } else {
                        burst_edit(&mut r, &mut p, &s, &mut last)
                    };
                    let after = r.below(4);
                    let looks = std::cell::Cell::new(0);
                    let stop = || {
                        looks.set(looks.get() + 1);
                        looks.get() > after
                    };
                    if !b.rebuild_or_stop(p.machine(), &changed, cfg, &stop) {
                        let ts: Vec<_> = b.traces().collect();
                        for (i, w) in ts.windows(2).enumerate() {
                            assert_eq!(
                                w[0].exit, w[1].entry,
                                "regions do not chain after a stop (region {i}), seed {seed}, config {ci}"
                            );
                        }
                        stopped += 1;
                        if b.stats.given_up_cost > 0 {
                            inside += 1;
                        }
                        run += 1;
                        if run > 1 {
                            in_a_row += 1;
                        }
                        assert!(!b.settled());
                    }
                }
                let changed = if !b.settled() && r.below(4) == 0 {
                    Vec::new()
                } else {
                    burst_edit(&mut r, &mut p, &s, &mut last)
                };
                assert!(b.rebuild_or_stop(p.machine(), &changed, cfg, &|| false));
                assert!(b.settled());
                let effects_changed = b.take_effects_changed();
                if r.below(4) == 0 {
                    coarsens_unindexed_as_indexed(
                        &b,
                        cfg.grain,
                        &format!("seed {seed}, config {ci}"),
                    );
                }
                if r.below(2) == 0 {
                    // (while no edit waits: fine regions merged back, the
                    // same effects)
                    b.coarsen(cfg.grain, u32::try_from(r.below(3)).unwrap());
                    b.check_replay(&b.final_state().digest());
                    assert!(!b.take_effects_changed(), "coarsening changed the effects");
                }
                let (digest, want) = plain(&p);
                if effects_changed {
                    linked = effects(&b);
                } else {
                    same += 1;
                    assert_eq!(
                        linked,
                        effects(&b),
                        "seed {seed}, config {ci}, edit {e}: effects"
                    );
                }
                assert_eq!(
                    b.output(&Sequential),
                    want,
                    "seed {seed}, config {ci}, edit {e}\n{}",
                    p.text()
                );
                assert_eq!(
                    b.final_state().digest(),
                    digest,
                    "seed {seed}, config {ci}, edit {e}"
                );
            }
        }
    }
    assert!(
        stopped > 1000 && inside > 100 && in_a_row > 300 && same > 100,
        "{stopped} rebuilds stopped ({inside} inside a span), {in_a_row} after another, {same} links taken again"
    );
}

/// A copy of `b` not indexed (as a save's) merges its regions as an
/// indexed one does.
fn coarsens_unindexed_as_indexed(b: &Build<partex_incr::stub::Stub>, grain: u64, what: &str) {
    let copy = || {
        let (initial, fin, seq, generation, changed) = b.parts();
        let seq = seq.into_iter().map(|(k, t)| (k, t.clone())).collect();
        Build::from_parts(
            initial.clone(),
            fin.clone(),
            seq,
            generation,
            changed.clone(),
        )
    };
    let (mut plain_copy, mut indexed) = (copy(), copy());
    indexed.index();
    plain_copy.coarsen(grain, 0);
    indexed.coarsen(grain, 0);
    let shape = |b: &Build<partex_incr::stub::Stub>| -> Vec<String> {
        b.traces()
            .map(|t| {
                let writes: Vec<_> = t.writes.iter().map(|(c, _, v)| (c, v)).collect();
                format!(
                    "{:?} {:?} {:?} {writes:?} {}",
                    t.entry, t.exit, t.guards, t.cost
                )
            })
            .collect()
    };
    assert_eq!(shape(&plain_copy), shape(&indexed), "{what}");
}

fn check_rounds<E: Executor>(p: &Program, exec: &E, what: &str) {
    let (digest, want) = plain(p);
    for holes in [Holes::Off, Holes::Suspend, Holes::Speculate] {
        for parts in [1, 2, 5, 13] {
            for max_rounds in [32, 1] {
                let cfg = rounds::Config {
                    parts,
                    holes,
                    max_rounds,
                    ..rounds::Config::default()
                };
                let o = rounds::run(&p.machine(), &cfg, exec);
                assert_eq!(
                    o.output(exec),
                    want,
                    "{what}, {holes:?}, {parts} parts, cap {max_rounds}\n{}",
                    p.text()
                );
                assert_eq!(
                    o.final_state.digest(),
                    digest,
                    "{what}, {holes:?}, {parts} parts"
                );
            }
        }
    }
}

#[test]
fn rounds_reach_the_sequential_fixpoint() {
    for seed in 0..250 {
        let (_, _, p) = random_program(seed + 20_000, 150);
        check_rounds(&p, &Sequential, &format!("seed {seed}, sequential"));
        #[cfg(feature = "threads")]
        check_rounds(&p, &Threads(4), &format!("seed {seed}, threads"));
    }
}

/// Counters threaded through every region: holes make them link-time
/// values, so rounds stop depending on the number of regions.
fn counter_program(blocks: usize) -> Program {
    let mut s = String::from("set x0 0\nset x1 100\n");
    for i in 0..blocks {
        let _ = write!(
            s,
            "\ninc x0\nemit x0\nlabel L0 x0\nadd x1 x0\nsay block {i}\nref L1\nalloc\nemit x1\n"
        );
        if i == 3 {
            s.push_str("label L1 x1\n");
        }
    }
    Program::from_text(&s)
}

#[test]
fn holes_make_counters_free() {
    let p = counter_program(40);
    let want = oracle(&p);
    let run = |holes| {
        let cfg = rounds::Config {
            parts: 20,
            holes,
            ..rounds::Config::default()
        };
        let o = rounds::run(&p.machine(), &cfg, &Sequential);
        assert_eq!(o.output(&Sequential), want, "{holes:?}");
        o.stats
    };
    let off = run(Holes::Off);
    let spec = run(Holes::Speculate);
    let susp = run(Holes::Suspend);
    assert!(
        off.rounds >= 10,
        "without holes each round fixes about one region: {off:?}"
    );
    assert!(spec.rounds <= 3, "{spec:?}");
    assert!(susp.rounds <= 3, "{susp:?}");
}

#[test]
fn suspended_regions_resume_when_their_entry_is_known() {
    let mut s = String::from("set x0 0\n");
    for i in 0..30 {
        let _ = write!(
            s,
            "\ninc x0\nif x0 > {} then say odd {i}\nemit x0\ninc x0\n",
            2 * i
        );
    }
    let p = Program::from_text(&s);
    let cfg = rounds::Config {
        parts: 10,
        holes: Holes::Suspend,
        ..rounds::Config::default()
    };
    let o = rounds::run(&p.machine(), &cfg, &Sequential);
    assert_eq!(o.output(&Sequential), oracle(&p));
    assert!(
        o.stats.suspended > 0 && o.stats.resumed > 0,
        "{:?}",
        o.stats
    );
    let cfg = rounds::Config {
        holes: Holes::Speculate,
        ..cfg
    };
    let o = rounds::run(&p.machine(), &cfg, &Sequential);
    assert_eq!(o.output(&Sequential), oracle(&p));
    assert_eq!(o.stats.suspended, 0);
}

fn big_program(len: usize) -> Program {
    let mut r = Rng(7);
    let s = Shape {
        vars: 48,
        labels: 16,
        work: 0,
        raw: false,
        acc: false,
    };
    Program::from_text(&generate::program(&mut r, len, &s))
}

#[test]
fn a_local_edit_reexecutes_locally() {
    let mut p = big_program(4000);
    let cfg = build::Config::default();
    let mut b = Build::new(p.machine(), &cfg);
    let full = b.stats.executed_cost;
    let id = p.ids()[2000];
    let changed = p.replace(id, "say edited");
    b.rebuild(p.machine(), &changed, &cfg);
    assert_eq!(b.output(&Sequential), oracle(&p));
    assert!(b.stats.executed_cost * 10 < full, "{:?} of {full}", b.stats);
    // A label's value is read only at the link step.
    let id = p.ids()[100];
    let changed = p.replace(id, "label L3 x7");
    b.rebuild(p.machine(), &changed, &cfg);
    assert_eq!(b.output(&Sequential), oracle(&p));
    assert!(b.stats.executed_cost * 10 < full, "{:?}", b.stats);
    // Allocation numbers are link-time symbols, not state.
    let id = p.ids()[50];
    let changed = p.insert_after(Some(id), "alloc");
    b.rebuild(p.machine(), &changed, &cfg);
    assert_eq!(b.output(&Sequential), oracle(&p));
    assert!(b.stats.executed_cost * 10 < full, "{:?}", b.stats);
}

#[test]
fn composed_traces_replay_to_the_same_state() {
    for seed in 0..100 {
        let (_, _, p) = random_program(seed + 30_000, 80);
        let cfg = build::Config {
            grain: 3,
            ..build::Config::default()
        };
        let b = Build::new(p.machine(), &cfg);
        let mut traces = b.traces();
        let first = traces.next().expect("a region").clone();
        let whole = traces.fold(first, |acc, t| acc.compose(t));
        let m = p.machine();
        assert!(whole.holds(&m));
        let mut m2 = m.clone();
        whole.apply(&mut m2, &[]);
        assert_eq!(m2.digest(), b.final_state().digest(), "seed {seed}");
    }
}

#[test]
fn flat_recording_records_what_maps_record() {
    for seed in 0..200 {
        let (_, _, p) = random_program(seed + 40_000, 80);
        for grain in [1, 3, 16] {
            let with = |flat| {
                let cfg = build::Config {
                    grain,
                    flat,
                    ..build::Config::default()
                };
                let b = Build::new(p.machine(), &cfg);
                b.traces().map(|t| format!("{t:?}")).collect::<Vec<_>>()
            };
            assert_eq!(with(true), with(false), "seed {seed}, grain {grain}");
        }
    }
}

/// One random edit: replace, insert or delete a line.
fn random_edit(r: &mut Rng, s: &Shape, p: &mut Program) {
    let ids = p.ids();
    if ids.is_empty() {
        p.insert_after(None, &generate::statement(r, s));
        return;
    }
    let id = ids[r.below(ids.len())];
    match r.below(3) {
        0 => {
            p.replace(id, &generate::statement(r, s));
        }
        1 => {
            p.insert_after(Some(id), &generate::statement(r, s));
        }
        _ => {
            p.delete(id);
        }
    }
}

/// Warm starts (§7.7): rounds seeded with a previous run's regions, from
/// parallel rounds or from a recorded build, reach the sequential result
/// after random edits; with no edit they run nothing.
#[test]
#[allow(clippy::many_single_char_names)]
fn warm_rounds_reach_the_sequential_fixpoint() {
    for seed in 0..200 {
        let (mut r, s, mut p) = random_program(seed + 40_000, 150);
        for holes in [Holes::Off, Holes::Suspend, Holes::Speculate] {
            for (parts, max_rounds) in [(5, 32), (13, 32), (7, 1)] {
                let cfg = rounds::Config {
                    parts,
                    holes,
                    max_rounds,
                    ..rounds::Config::default()
                };
                let cold = rounds::run(&p.machine(), &cfg, &Sequential);
                let warm = rounds::Warm::from_outcome(&cold);
                // Unchanged: every region validates in the first sweep.
                let again = rounds::run_warm(&p.machine(), &cfg, &Sequential, Some(&warm));
                let (digest, want) = plain(&p);
                assert_eq!(again.output(&Sequential), want, "seed {seed}, unchanged");
                assert_eq!(again.final_state.digest(), digest, "seed {seed}");
                assert_eq!(again.stats.runs, 0, "seed {seed}: {:?}", again.stats);
                let b = Build::new(p.machine(), &build::Config::default());
                let from_build = rounds::Warm::from_build(&b, parts);
                let mut q = p.clone();
                for _ in 0..=r.below(3) {
                    random_edit(&mut r, &s, &mut q);
                }
                let (digest, want) = plain(&q);
                for (what, w) in [("outcome", &warm), ("build", &from_build)] {
                    let o = rounds::run_warm(&q.machine(), &cfg, &Sequential, Some(w));
                    assert_eq!(
                        o.output(&Sequential),
                        want,
                        "seed {seed}, warm from {what}, {holes:?}, {parts} parts\n{}",
                        q.text()
                    );
                    assert_eq!(o.final_state.digest(), digest, "seed {seed}, {what}");
                    // and a warm start from that warm run
                    let w2 = rounds::Warm::from_outcome(&o);
                    let o2 = rounds::run_warm(&q.machine(), &cfg, &Sequential, Some(&w2));
                    assert_eq!(o2.output(&Sequential), want, "seed {seed}, rewarmed");
                    assert_eq!(o2.stats.runs, 0, "seed {seed}: {:?}", o2.stats);
                }
            }
        }
        random_edit(&mut r, &s, &mut p);
    }
}

/// A warm start after a one-line edit re-runs only the regions whose
/// guards the edit breaks, instead of wasting round 1 on cold guesses.
#[test]
fn a_warm_start_reruns_little() {
    let mut p = counter_program(60);
    let cfg = rounds::Config {
        parts: 20,
        holes: Holes::Speculate,
        ..rounds::Config::default()
    };
    let cold = rounds::run(&p.machine(), &cfg, &Sequential);
    let id = p.ids()[p.len() / 2];
    p.insert_after(Some(id), "say inserted");
    let warm = rounds::Warm::from_outcome(&cold);
    let o = rounds::run_warm(&p.machine(), &cfg, &Sequential, Some(&warm));
    assert_eq!(o.output(&Sequential), oracle(&p));
    assert!(o.stats.runs <= 2, "{:?} (cold: {:?})", o.stats, cold.stats);
    assert!(cold.stats.runs > 10, "{:?}", cold.stats);
}

/// A program whose third region asks a question about the accumulator
/// (`has 2`) after the first added to it (`mark 1`), with blank lines
/// between the regions.
fn asking_program() -> Program {
    Program::from_text("mark 1\n\nsay a\n\nhas 2\n\nsay b\n")
}

/// One edit of `asking_program`'s first line, rebuilt with derived
/// guards on or off: the regions re-executed, and the output checked.
fn dirty_after(first: &str, derived: bool) -> usize {
    let cfg = build::Config {
        grain: 1,
        sanitize: true,
        derived_guards: derived,
        ..build::Config::default()
    };
    let mut p = asking_program();
    let mut b = Build::new(p.machine(), &cfg);
    let id = p.ids()[0];
    let changed = p.replace(id, first);
    b.rebuild(p.machine(), &changed, &cfg);
    assert_eq!(
        b.output(&Sequential),
        oracle(&p),
        "{first}, derived {derived}"
    );
    b.stats.dirty_regions
}

/// A derived guard whose answer holds while its source changed: the
/// region that asked replays; off, its guard on the source re-runs it.
#[test]
fn a_derived_guard_that_holds_replays_the_region() {
    assert_eq!(dirty_after("mark 3", true), 1, "only the edited region");
    assert_eq!(dirty_after("mark 3", false), 2, "the asking one too");
}

/// A derived guard whose answer changed: the region re-runs.
#[test]
fn a_derived_guard_that_fails_reruns_the_region() {
    assert_eq!(dirty_after("mark 2", true), 2);
}

/// Composing a region that added to the accumulator with one that asked
/// about it: the composed region reads the accumulator whole (no derived
/// guard, a guard on it at the composed entry). Composing two that did
/// not add keeps the question.
#[test]
fn composition_over_a_writer_falls_back_to_the_source_guard() {
    let cfg = build::Config {
        grain: 1,
        ..build::Config::default()
    };
    let p = asking_program();
    let b = Build::new(p.machine(), &cfg);
    let t: Vec<_> = b.traces().cloned().collect();
    let asks = t
        .iter()
        .position(|t| t.guards.iter().any(|(c, _)| matches!(c, Cell::UsedHas(_))))
        .expect("a region asks");
    assert!(asks >= 2, "the question is not in the first regions");
    let m = p.machine();
    let entry = |c: &Cell| Some(partex_incr::version_of(&m.get(c)));
    // (every region from the first one to the asking one)
    let whole = t[1..=asks]
        .iter()
        .try_fold(t[0].clone(), |acc, n| acc.compose_with(n, &entry))
        .expect("composable");
    assert!(!whole.answered(&Cell::Used), "{:?}", whole.guards);
    assert!(whole.reads(&Cell::Used));
    assert!(whole.holds(&m));
    // (the regions after the mark: none adds, the question stays)
    let rest = t[2..=asks]
        .iter()
        .try_fold(t[1].clone(), |acc, n| acc.compose_with(n, &entry))
        .expect("composable");
    assert!(rest.answered(&Cell::Used), "{:?}", rest.guards);
}

/// Every guard keeps its real version: the traces of random programs
/// that ask questions hold on replay (the sanitizer's chain check), and
/// the derived ones are answered by `get` as recorded.
#[test]
fn derived_guards_keep_their_real_versions() {
    for seed in 0..60 {
        let (_, _, p) = random_program_with(seed + 50_000, 80, true);
        let cfg = build::Config {
            grain: 1,
            ..build::Config::default()
        };
        let b = Build::new(p.machine(), &cfg);
        b.check_replay(&b.final_state().digest());
        let mut m = p.machine();
        for t in b.traces() {
            assert!(t.holds(&m), "seed {seed}");
            t.apply(&mut m, &[]);
        }
    }
}

/// The build of a program whose lines are numbered by their places, as
/// a file's are (`Program::renumbered`), renamed after an edit that
/// moved them (`Build::rename`), then rebuilt: the same output and final
/// state as a build from scratch. A line whose value the edit changed
/// (its text, or its successor, which an insertion or a deletion moves)
/// is gone, and a region that read it runs again; every other line is
/// renamed to its new place, and so are the boundaries.
fn renamed_rebuild(
    b: &mut Build<partex_incr::stub::Stub>,
    old: &Program,
    edited: &Program,
    changed: &[Cell],
    cfg: &build::Config,
) -> Program {
    use partex_incr::build::Renaming;
    use std::collections::BTreeSet;
    let (new, map) = edited.renumbered();
    let gone: BTreeSet<u32> = changed
        .iter()
        .filter_map(|c| match c {
            Cell::Line(id) => Some(*id),
            _ => None,
        })
        .collect();
    // (the old build's ids are `old`'s places; `edited` kept them for
    // the lines it did not add)
    let _ = old;
    let line = |id: u32| {
        (!gone.contains(&id))
            .then(|| map.get(&id).copied())
            .flatten()
    };
    let m = new.machine();
    let state = |f: &mut partex_incr::stub::Stub| f.renamed(&new, &map);
    let cell = |c: &Cell| match c {
        Cell::Line(id) => line(*id).map(Cell::Line),
        c => Some(c.clone()),
    };
    // (a boundary is where a line begins: renamed if the line is still
    // there, its text changed or not; a deleted one's, a name no new
    // position has)
    let boundary = |b: &Option<u32>| b.map(|id| map.get(&id).copied().unwrap_or(u32::MAX - id));
    let value = |_: &Cell, _: &partex_incr::stub::Val| None;
    // (a renamed line holds its successor's new id: its version is the
    // new program's)
    let guard = |c: &Cell, v: partex_incr::Version| match c {
        Cell::Line(_) => partex_incr::version_of(&m.get(c)),
        _ => v,
    };
    b.rename(&Renaming {
        cell: &cell,
        boundary: &boundary,
        value: &value,
        guard: &guard,
        state: &state,
    });
    // (the changed lines by their new places)
    let changed: Vec<Cell> = changed
        .iter()
        .filter_map(|c| match c {
            Cell::Line(id) => map.get(id).map(|n| Cell::Line(*n)),
            c => Some(c.clone()),
        })
        .collect();
    b.rebuild(new.machine(), &changed, cfg);
    new
}

/// Random edits that insert, delete and replace lines, on programs whose
/// lines are numbered by place: each rebuild after a rename matches a
/// build from scratch (and the sanitizer's chain check).
#[test]
fn renamed_rebuilds_match_scratch() {
    let mut checked = 0;
    for seed in 0..120 {
        for (ci, cfg) in build_configs().iter().enumerate() {
            if !cfg.incremental {
                continue;
            }
            let (mut r, s, p) = random_program_with(seed + 70_000, 60, seed % 2 == 0);
            let (mut p, _) = p.renumbered();
            let mut b = Build::new(p.machine(), cfg);
            for e in 0..6 {
                let mut edited = p.clone();
                let changed = generate::edit(&mut r, &mut edited, &s);
                p = renamed_rebuild(&mut b, &p, &edited, &changed, cfg);
                let (digest, want) = plain(&p);
                assert_eq!(
                    b.output(&Sequential),
                    want,
                    "seed {seed}, config {ci}, edit {e}\n{}",
                    p.text()
                );
                assert_eq!(
                    b.final_state().digest(),
                    digest,
                    "seed {seed}, config {ci}, edit {e}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0);
}

/// Two fixed cases: a guarded line's content moved (a line inserted
/// before it: its region replays, renamed), and a guarded line's content
/// changed (its region runs again).
#[test]
fn a_moved_line_replays_and_a_changed_one_reruns() {
    let cfg = build::Config {
        grain: 1,
        sanitize: true,
        ..build::Config::default()
    };
    let src = "set x1 1\n\nsay a\n\nset x2 2\n\nemit x2\n\nsay end\n";
    let (p, _) = Program::from_text(src).renumbered();
    // (a line inserted at the top: every later line moves by one)
    let mut b = Build::new(p.machine(), &cfg);
    let mut edited = p.clone();
    let changed = edited.insert_after(None, "say first");
    let new = renamed_rebuild(&mut b, &p, &edited, &changed, &cfg);
    assert_eq!(b.output(&Sequential), oracle(&new));
    assert!(
        b.stats.executed_cost <= 3,
        "only the new line's region runs: {:?}",
        b.stats
    );
    // (the line `set x2 2`, now line 5, changed: its region and the
    // reader of x2 run)
    let mut edited = new.clone();
    let id = edited.ids()[5];
    let changed = edited.replace(id, "set x2 3");
    let newer = renamed_rebuild(&mut b, &new, &edited, &changed, &cfg);
    assert_eq!(b.output(&Sequential), oracle(&newer));
    assert!(b.stats.dirty_regions >= 2, "{:?}", b.stats);
}

/// The audit is a diagnostic: with it on, a rebuild does the same (its
/// statistics and output), and it accounts for every old region and
/// command re-executed.
#[test]
fn the_audit_changes_nothing_and_counts_every_span() {
    let mut spans = 0;
    for seed in 0..80 {
        let (mut r, s, mut p) = random_program(seed + 70_000, 90);
        let off = build::Config {
            grain: 3,
            fine_grain: 1,
            ..build::Config::default()
        };
        let on = build::Config {
            audit: true,
            ..off.clone()
        };
        let mut quiet = Build::new(p.machine(), &off);
        let mut audited = Build::new(p.machine(), &on);
        for e in 0..6 {
            let changed = generate::edit(&mut r, &mut p, &s);
            quiet.rebuild(p.machine(), &changed, &off);
            audited.rebuild(p.machine(), &changed, &on);
            assert_eq!(quiet.stats, audited.stats, "seed {seed}, edit {e}");
            assert_eq!(quiet.output(&Sequential), audited.output(&Sequential));
            assert!(quiet.audit.is_empty());
            let regions: usize = audited.audit.iter().map(|x| x.old_costs.len()).sum();
            let cost: u64 = audited.audit.iter().map(|x| x.cost).sum();
            assert_eq!(
                regions, audited.stats.dirty_regions,
                "seed {seed}, edit {e}"
            );
            assert_eq!(cost, audited.stats.executed_cost, "seed {seed}, edit {e}");
            // (a region the run reached at its entry is dirty only by a
            // cell of D it read)
            assert!(audited.audit.iter().all(|x| !x.synced || !x.why.is_empty()));
            spans += audited.audit.len();
        }
    }
    assert!(spans > 100, "{spans} spans audited");
}
