//! A pass's writers run once the line is quiet (`Operation::final_only`):
//! a node of such an operation made dirty during an evaluation waits until
//! no other node is queued or running, so it runs once a pass, not once
//! for every rerun of what it reads.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId, Values};

/// `[x, ms]`: contributes x to "s" after `ms` milliseconds.
struct SlowContrib;

impl Operation for SlowContrib {
    fn name(&self) -> &'static str {
        "slow_contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[1]).unwrap_or(0).unsigned_abs();
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            std::hint::spin_loop();
        }
        let x = cx.args()[0];
        cx.contribute(cx.values().field("s"), x);
        Ok(x)
    }
}

/// `[]`: outputs the sum of the contributions to "s" before it, those in
/// so far (one not in yet asks it again when it comes); counts its runs;
/// final-only where built so.
struct Writer {
    runs: AtomicU64,
    fin: bool,
}

impl Operation for Writer {
    fn name(&self) -> &'static str {
        "writer"
    }

    fn final_only(&self) -> bool {
        self.fin
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::Relaxed);
        let v = cx.values();
        let mut sum = 0;
        let mut at = None;
        while let Some((n, x)) = cx.next_contribution(v.field("s"), at)? {
            sum += v.as_int(x).unwrap_or(0);
            at = Some(n);
        }
        Ok(v.int(sum))
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

fn build(fin: bool) -> (u64, Option<i64>) {
    let e = Engine::new();
    let v = e.values();
    let c = e.register(Arc::new(SlowContrib));
    let w = Arc::new(Writer {
        runs: AtomicU64::new(0),
        fin,
    });
    let wop = e.register(w.clone());
    let u = e.new_unit();
    e.push(u, c, vec![v.int(1), v.int(0)]);
    e.push(u, c, vec![v.int(4), v.int(200)]);
    let n = e.push(u, wop, vec![]);
    e.evaluate(2).unwrap();
    assert!(e.check_invariants().is_empty());
    (w.runs.load(Ordering::Relaxed), e.output(n).and_then(|x| v.as_int(x)))
}

/// The writer folds contributions, one of them slow: an ordinary writer
/// runs, meets the slow one not in yet and runs again; a final-only one
/// runs once, when everything else is done.
#[test]
fn a_final_only_node_runs_once_the_line_is_quiet() {
    let (runs, out) = build(false);
    assert_eq!(out, Some(5));
    assert!(
        runs >= 2,
        "an ordinary writer ran before the slow contribution was in, then again: {runs}"
    );
    let (runs, out) = build(true);
    assert_eq!(out, Some(5));
    assert_eq!(runs, 1, "a final-only writer runs once, when the line is quiet");
}

/// `[]`: a file the job writes and reads back (`read_back`): defines "lab"
/// as the sum of the contributions to "s" (a label's page, written to the
/// aux file); final-only, as the files' writers are.
struct AuxWriter(AtomicU64);

impl Operation for AuxWriter {
    fn name(&self) -> &'static str {
        "aux_writer"
    }

    fn final_only(&self) -> bool {
        true
    }

    fn read_back(&self) -> bool {
        true
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let v = cx.values();
        let s = cx.fold(v.field("s"), None, None)?;
        let sum: i64 = v.seq_to_vec(s).iter().filter_map(|&x| v.as_int(x)).sum();
        cx.define(v.field("lab"), v.int(sum));
        Ok(v.int(sum))
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

/// `[]`: reads "lab" as the pass before wrote it (`At::Final`, a `\ref`
/// at the front of the document) and contributes it to "out" (the text
/// that prints it).
struct RefText;

impl Operation for RefText {
    fn name(&self) -> &'static str {
        "ref_text"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let x = match cx.read_at(v.field("lab"), phitex_defuse::At::Final)? {
            phitex_defuse::FieldAt::Def(x, _) => x,
            _ => v.int(0),
        };
        cx.contribute(v.field("out"), x);
        Ok(x)
    }
}

/// `[]`: the output file: the sum of the contributions to "out"; counts
/// its runs; final-only and not read back (the PDF's link).
struct OutWriter(AtomicU64);

impl Operation for OutWriter {
    fn name(&self) -> &'static str {
        "out_writer"
    }

    fn final_only(&self) -> bool {
        true
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let v = cx.values();
        let s = cx.fold(v.field("out"), None, None)?;
        Ok(v.int(v.seq_to_vec(s).iter().filter_map(|&x| v.as_int(x)).sum()))
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

/// A document whose `\ref` reads what the aux writer wrote: the first pass
/// prints 0, the aux writer writes 5, a second pass prints 5. The output
/// writer runs once, after the last pass, not once a pass: the first
/// pass's file would be written only to be written again. Again after an
/// edit that moves the label (two passes again): once.
#[test]
fn the_output_is_written_once_after_the_last_pass() {
    let e = Engine::new();
    let v = e.values();
    let c = e.register(Arc::new(SlowContrib));
    let aux = Arc::new(AuxWriter(AtomicU64::new(0)));
    let out = Arc::new(OutWriter(AtomicU64::new(0)));
    let (aop, oop) = (e.register(aux.clone()), e.register(out.clone()));
    let r = e.register(Arc::new(RefText));
    let u = e.new_unit();
    e.push(u, r, vec![]);
    e.push(u, c, vec![v.int(1), v.int(0)]);
    let k = e.push(u, c, vec![v.int(4), v.int(0)]);
    e.push(u, aop, vec![]);
    let o = e.push(u, oop, vec![]);
    e.evaluate(2).unwrap();
    assert!(e.check_invariants().is_empty());
    assert_eq!(e.output(o).and_then(|x| v.as_int(x)), Some(5), "the second pass's");
    assert_eq!(
        out.0.load(Ordering::Relaxed),
        1,
        "the output written once, after the last pass"
    );

    e.replace(k, c, vec![v.int(6), v.int(0)]);
    e.evaluate(2).unwrap();
    assert!(e.check_invariants().is_empty());
    assert_eq!(e.output(o).and_then(|x| v.as_int(x)), Some(7));
    assert_eq!(out.0.load(Ordering::Relaxed), 2, "once for the edit's two passes");
}

/// `[at, f, x]`: a page-layer node (settles at quiet points): in one
/// commit stands the layer's frontier at node `at` (-1: none) and defines
/// f := x.
struct FrontPage;

impl Operation for FrontPage {
    fn name(&self) -> &'static str {
        "front_page"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args().to_vec();
        let at = v.as_int(a[0]).and_then(|x| u64::try_from(x).ok());
        cx.set_frontier(at.map(phitex_defuse::NodeId::from_raw));
        cx.define(Field(a[1]), a[2]);
        Ok(a[2])
    }

    fn settles_at_quiet(&self) -> bool {
        true
    }
}

/// `[f, ms]`: text that reads the page value f, then after `ms`
/// milliseconds contributes it to "s" (a `\label` on the page it reads).
struct PageContrib;

impl Operation for PageContrib {
    fn name(&self) -> &'static str {
        "page_contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args().to_vec();
        let x = cx.read(Field(a[0]))?.unwrap_or_else(|| v.int(0));
        let ms = v.as_int(a[1]).unwrap_or(0).unsigned_abs();
        std::thread::sleep(Duration::from_millis(ms));
        cx.contribute(v.field("s"), x);
        Ok(x)
    }
}

/// A file read back (the `.aux`, and the tool run on it, BibTeX) is
/// written on the text its pass has written whole: at a quiet point that
/// lets a run held at the layer's frontier go, the writer waits for the
/// next quiet point, after that run. Let go with it, the writer read the
/// file as the held run had yet to write it, and ran again once it had
/// (Grail at -j8 wrote its `.aux` and ran BibTeX on it four times, -j1
/// once: mc 86379). Line: F (page: the frontier, pn), A, C (a
/// contribution to "s"), T (reads pn, slow, contributes it to "s"), W
/// (the aux writer: the sum of "s"). Edited at 2 threads: C, re-asking W
/// (final-only, deferred to the quiet point), and F, standing the
/// frontier at A and changing pn, so T is held there. At the quiet point
/// both are due; T's run is slow, so W let go beside it reads "s" before
/// T's new contribution is in, every time.
#[test]
fn a_read_back_writer_waits_for_the_runs_held_at_the_frontier() {
    for _ in 0..4 {
        let e = Engine::new();
        let v = e.values();
        let c = e.register(Arc::new(SlowContrib));
        let front = e.register(Arc::new(FrontPage));
        let t_op = e.register(Arc::new(PageContrib));
        let aux = Arc::new(AuxWriter(AtomicU64::new(0)));
        let aop = e.register(aux.clone());
        let u = e.new_unit();
        let pn = v.field("pn");
        let f = e.push(u, front, vec![v.int(-1), pn.0, v.int(1)]);
        let a = e.push(u, c, vec![v.int(0), v.int(0)]);
        let k = e.push(u, c, vec![v.int(10), v.int(0)]);
        let t = e.push(u, t_op, vec![pn.0, v.int(150)]);
        let w = e.push(u, aop, vec![]);
        e.evaluate(2).unwrap();
        assert!(e.check_invariants().is_empty());
        assert_eq!(e.output(w).and_then(|x| v.as_int(x)), Some(11));
        let at = i64::try_from(a.raw()).expect("a id");
        let held = e.frontier_held();
        aux.0.store(0, Ordering::Relaxed);
        e.replace(k, c, vec![v.int(20), v.int(0)]);
        e.replace(f, front, vec![v.int(at), pn.0, v.int(2)]);
        let rep = e.evaluate(2).unwrap();
        assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
        assert_eq!(e.output(t).and_then(|x| v.as_int(x)), Some(2), "{rep:?}");
        assert!(e.frontier_held() > held, "T was held at the frontier: {rep:?}");
        assert_eq!(e.output(w).and_then(|x| v.as_int(x)), Some(22), "{rep:?}");
        assert_eq!(
            aux.0.load(Ordering::Relaxed),
            1,
            "the aux writer ran once, after the held run: {rep:?}"
        );
    }
}
