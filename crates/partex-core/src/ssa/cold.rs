//! A cold build on workers (DESIGN 3.10, "Cold builds", as built): the
//! job past its preamble guessed wide, then made exact by the rebuild's
//! own machinery.
//!
//! The build runs in turn until it has passed `\begin{document}` and is
//! at the end of a line of the main file (the base). The rest of the
//! main file is cut at its paragraphs (the CST's, `phitex-syntax`: at
//! brace depth 0), grouped into a few runs a worker; every run starts
//! from the base, its input moved to its first line, and runs step after
//! step to the next run's first line, all at once (their pages shown
//! provisionally as they ship). Their numbers are wrong past the first:
//! a run starts with the counters, the page, the allocators as they were
//! at the base. That is fine.
//!
//! Each run's steps go into the build's fold in the source's order, as
//! if the build had run them: their records, their definitions in the
//! arrays, the input left where each ended. A step whose start is not
//! where the step before it ended, or that read a slot holding otherwise
//! there than what it read, or whose writes the build cannot take, is
//! dirty, as an edit's readers are. Then the rebuild runs the dirty steps
//! in order (`rebuild::run_dirty`, on workers too: DESIGN 3.10's rounds),
//! each from where the step before it ended, the definitions reaching it
//! placed; a step whose definitions change makes their readers dirty in
//! turn; a run that ends elsewhere goes on, and passes over the old steps
//! it meets. When nothing is dirty the fold is the job's: the outputs are
//! a build in turn's (PDF object numbers aside), whatever the guesses and
//! the workers' timing were. A gap (a run cut short by a font loaded, a
//! command) is a step whose start is not the end before it: the rebuild
//! runs the text between.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::AtomicU32;

use super::par::{self, ChunkJob};
use super::rebuild::{self, InputState, recycle_tracker};
use super::{Recorder, SsaTracker};
use crate::host::Host;
use crate::tex::Tex;

/// The most runs (paragraphs grouped evenly past them).
const MAX_RUNS: usize = 4096;

/// Discovery (the fonts the runs load, loaded first): its rounds at most,
/// and the steps each of its runs runs at most.
const DISCOVERY_ROUNDS: usize = 4;
const DISCOVERY_STEPS: usize = 12;

/// A cold build's runs from its base: the main file's data, and each
/// run's input and its last line.
pub(super) struct Plan {
    main: Arc<[u8]>,
    runs: Vec<(InputState, i32)>,
}

/// A cold build's runs (DESIGN 3.10, "Cold builds").
#[derive(Default)]
pub(super) struct Cold {
    /// The main file's data, the line of its `\begin{document}`, and the
    /// first lines of its paragraphs after it.
    main: Option<Arc<[u8]>>,
    begin: i32,
    paras: Vec<i32>,
    off: bool,
    salts: Arc<AtomicU32>,
    /// The build's tables' lengths when the runs began.
    lens: core::cell::Cell<par::BaseLens>,
    t0: Option<std::time::Instant>,
    /// `PHITEX_SSA_COLD_TRACE=1`: what it did, on stderr.
    trace: bool,
}

/// The first line (from 1) of each paragraph of `src` that begins after
/// line `after`, up to the one holding `\end{document}`.
fn paragraphs(src: &[u8], after: i32) -> Vec<i32> {
    let Ok(text) = core::str::from_utf8(src) else {
        return Vec::new();
    };
    let tree = phitex_syntax::Tree::parse(text);
    let mut out = Vec::new();
    let mut line = 1i32;
    let mut at = 0usize;
    for &end in tree.para_ends() {
        // (where the paragraph after this one begins: its first line)
        line += src[at..end.min(src.len())]
            .iter()
            .fold(0i32, |n, &b| n + i32::from(b == b'\n'));
        at = end.min(src.len());
        if at >= src.len() {
            break;
        }
        if line > after {
            out.push(line);
        }
        if src[at..].starts_with(b"\\end{document}") {
            break;
        }
    }
    out
}

impl Cold {
    /// The build is at a step's end (`run_applying`, before the step
    /// closes): at the base, the runs that build the rest of the job
    /// ([`Cold::prepare`], then [`Cold::build`] once the step is closed).
    pub(super) fn at_base<H: Host>(&mut self, tex: &mut Tex<H, SsaTracker>) -> Option<Plan> {
        if self.off {
            return None;
        }
        if self.main.is_none() && !self.find_main(tex) {
            self.off = true;
            return None;
        }
        let main = self.main.clone()?;
        let here = InputState::of(tex, false);
        if !here.ends_line(&main, self.begin) {
            return None;
        }
        // (once: what follows is the job's)
        self.off = true;
        let line = here.main_line(&main)?;
        let starts: Vec<i32> = self
            .paras
            .iter()
            .copied()
            .filter(|&l| l > line + 1)
            .collect();
        if starts.is_empty() {
            return None;
        }
        Some(self.plan(&main, &here, &starts))
    }

    /// The main file and its paragraphs, found at the first step's end
    /// with it open; whether there is a `\begin{document}` and some.
    fn find_main<H: Host>(&mut self, tex: &Tex<H, SsaTracker>) -> bool {
        let Some(Some(f)) = tex.input_file.get(1) else {
            return false;
        };
        let data = f.data.clone();
        let begin = (1i32..)
            .zip(data.split(|&b| b == b'\n'))
            .find(|(_, l)| l.trim_ascii_start().starts_with(b"\\begin{document}"))
            .map(|(n, _)| n);
        let Some(begin) = begin else {
            return false;
        };
        let paras = paragraphs(&data, begin);
        if paras.is_empty() {
            return false;
        }
        self.begin = begin;
        self.paras = paras;
        self.main = Some(data);
        self.t0 = Some(std::time::Instant::now());
        self.trace = std::env::var_os("PHITEX_SSA_COLD_TRACE").is_some();
        true
    }

    /// The runs from the base `here` (from `starts`, grouped): each one's
    /// input and its last line.
    fn plan(&self, main: &Arc<[u8]>, here: &InputState, starts: &[i32]) -> Plan {
        // (a run a paragraph, as many as `MAX_RUNS` at most, evenly)
        let mut firsts: Vec<i32> = if starts.len() > MAX_RUNS {
            (0..MAX_RUNS)
                .map(|k| starts[k * starts.len() / MAX_RUNS])
                .collect()
        } else {
            starts.to_vec()
        };
        firsts.dedup();
        if self.trace {
            std::eprintln!(
                "phitex: cold: base at {}; {} runs",
                here.brief(),
                firsts.len() + 1
            );
        }
        // (the first from the base itself to the first paragraph, each
        // other from its line to the next one's; a line the base cannot
        // be moved to is a gap, the rebuild's to run)
        let stop = |j: usize| firsts.get(j).map_or(i32::MAX, |&n| n - 1);
        let mut plan: Vec<(InputState, i32)> = alloc::vec![(here.clone(), stop(0))];
        for (j, &l) in firsts.iter().enumerate() {
            if let Some(input) = here.moved_to(main, l) {
                plan.push((input, stop(j + 1)));
            }
        }
        Plan {
            main: main.clone(),
            runs: plan,
        }
    }

    /// What each run would make first, made in the base's step, before it
    /// ends (DESIGN 3.10, "Cold builds", discovery): the PDF file opened,
    /// which a run's first page would open (a step's effect, the link's to
    /// name), and the TFM fonts the runs load ([`Cold::discover`]). A
    /// build in turn makes them later, where a step first wants them: a
    /// parallel build's PDF numbers its objects and names its fonts
    /// otherwise, which it may (DESIGN "Parallel builds").
    pub(super) fn prepare<H: Host>(&self, tex: &mut Tex<H, SsaTracker>, plan: &Plan) {
        let pdf = !tex.unicode && tex.int_par(crate::web::PDF_OUTPUT_CODE) > 0;
        if pdf && tex.ensure_pdf_open().is_err() {
            return;
        }
        self.discover(tex, &plan.main, &plan.runs);
    }

    /// The job from the base on, by `plan`: the runs made on workers, their
    /// steps taken into the fold, the dirty ones run again until none is.
    /// The job's `history`.
    pub(super) fn build<H: Host>(
        &mut self,
        tex: &mut Tex<H, SsaTracker>,
        plan: Plan,
    ) -> Option<i32> {
        let Plan { main, runs: plan } = plan;
        let main = &main;
        // (every run from the base: a view of the engine there, kept for
        // the runs, its tables shared by chunks with the build's)
        tex.eqtb.commit();
        tex.eqtb_obj.commit();
        tex.hash.commit();
        tex.save_stack.commit();
        let base = tex.fork_with(crate::host::NoHost, crate::track::Untracked);
        let snap = {
            let r = tex.tracker.rec.borrow();
            par::Snapshot::take(&tex.tracker, &r)
        };
        // (in batches of twice the workers, each taken in as it ends: what
        // waits is records, never a view of the engine)
        let batch = 2 * tex.tracker.par.workers.get().max(1);
        let mut dirty = rebuild::Dirty::default();
        let mut taken = 0usize;
        for part in plan.chunks(batch) {
            let runs = self.runs(tex, &base, &snap, main, part.to_vec(), 1 << 22);
            taken += self.take_all(tex, runs, &mut dirty);
        }
        if self.trace {
            std::eprintln!(
                "phitex: cold: {taken} steps taken in, {} dirty ({} ms)",
                dirty.len(),
                self.ms()
            );
        }
        if taken == 0 {
            // (nothing made: the build goes on in turn from the base)
            return None;
        }
        let rep = rebuild::RebuildReport {
            trace: self.trace,
            ..Default::default()
        };
        let from = tex.commands();
        // (a step run again finds its typesetting calls' records from its
        // run on a worker, and applies those whose reads still hold: its
        // paragraph's lines are not broken again where only the page
        // around them moved)
        let apply = tex.tracker.apply.replace(true);
        let (_, rep) = rebuild::run_dirty(tex, dirty, rep, from);
        tex.tracker.apply.set(apply);
        if self.trace {
            for l in rebuild::rebuild_log(tex) {
                std::eprintln!("phitex: cold: {l}");
            }
            std::eprintln!(
                "phitex: cold: correction: {} steps run ({} new, {} passed over), {} kept, {} commands ({} ms)",
                rep.steps_run,
                rep.new_steps,
                rep.removed,
                rep.readers_kept,
                tex.commands() - from,
                self.ms()
            );
        }
        // (the fold's last step: the job's end, or where the build goes
        // on in turn, the arrays holding the latest definitions)
        let tail = rebuild::fold_tail(tex)?;
        if tail.finished() {
            return Some(tex.history);
        }
        tail.set(tex);
        tex.at_checkpoint = true;
        None
    }

    /// Discovery (round 1, wide and shallow): the runs of `plan`, a few
    /// steps each, from the base; the TFM fonts those that loaded one
    /// loaded, loaded here before the base is taken
    /// ([`Tex::preload_font`]), and those runs again, until none loads
    /// one. A run then finds the fonts the runs before it load loaded, as
    /// in turn, and is not cut short where it loads one: it reuses it. (The
    /// fonts' numbers are not the order a build in turn loads them in: the
    /// PDF's object numbers and resource names differ, which a parallel
    /// build's output may, DESIGN "Parallel builds".)
    fn discover<H: Host>(
        &self,
        tex: &mut Tex<H, SsaTracker>,
        main: &Arc<[u8]>,
        plan: &[(InputState, i32)],
    ) {
        if tex.unicode {
            return;
        }
        let mut todo: Vec<usize> = (0..plan.len()).collect();
        for _ in 0..DISCOVERY_ROUNDS {
            if todo.is_empty() {
                break;
            }
            tex.eqtb.commit();
            tex.eqtb_obj.commit();
            tex.hash.commit();
            tex.save_stack.commit();
            let base = tex.fork_with(crate::host::NoHost, crate::track::Untracked);
            let snap = {
                let r = tex.tracker.rec.borrow();
                par::Snapshot::take(&tex.tracker, &r)
            };
            let batch = 2 * tex.tracker.par.workers.get().max(1);
            let mut fonts = Vec::new();
            let mut next = Vec::new();
            for part in todo.chunks(batch) {
                let runs = part.iter().map(|&i| plan[i].clone()).collect();
                let dones = self.runs(tex, &base, &snap, main, runs, DISCOVERY_STEPS);
                for (&i, d) in part.iter().zip(dones) {
                    let Some(d) = d else { continue };
                    if !d.fonts.is_empty() {
                        fonts.extend(d.fonts);
                        next.push(i);
                    }
                    recycle_tracker(tex, d.tracker);
                }
            }
            drop(base);
            if fonts.is_empty() {
                break;
            }
            // (loaded in the base's step: its writes, as a `\font` there)
            let mut loaded = 0usize;
            for (name, area, size) in &fonts {
                loaded += usize::from(tex.preload_font(name, area, *size));
            }
            if self.trace {
                std::eprintln!(
                    "phitex: cold: discovery: {} runs loaded {} fonts, {loaded} loaded first ({} ms)",
                    next.len(),
                    fonts.len(),
                    self.ms()
                );
            }
            todo = next;
        }
        if self.trace {
            std::eprintln!("phitex: cold: discovery done ({} ms)", self.ms());
        }
    }

    /// The time since the cold build began, for its trace.
    fn ms(&self) -> u128 {
        self.t0.map_or(0, |t| t.elapsed().as_millis())
    }

    /// The runs `plan` (each from its input, to its last line, `max`
    /// steps at most), on workers, each from the base (`base`, `snap`: the
    /// engine and what the build answers there).
    fn runs<H: Host>(
        &self,
        tex: &mut Tex<H, SsaTracker>,
        base: &Tex<crate::host::NoHost, crate::track::Untracked>,
        snap: &par::Snapshot,
        main: &Arc<[u8]>,
        plan: Vec<(InputState, i32)>,
        max: usize,
    ) -> Vec<Option<par::ChunkDone>> {
        let (tx, rx) = std::sync::mpsc::channel::<par::Msg>();
        let mut replies = Vec::new();
        let mut jobs = Vec::new();
        for (input, stop) in plan {
            let (rtx, rrx) = std::sync::mpsc::channel();
            let host = par::WorkerHost {
                job: jobs.len(),
                ask: tx.clone(),
                reply: rrx,
                now: tex.host.now(),
                notes: tex.host.notes(),
                streams: tex.host.wants_streams(),
                commands: tex.host.runs_commands(),
                events: Vec::new(),
                tainted: None,
                provisional: true,
            };
            let mut shell = tex
                .tracker
                .par
                .shells
                .borrow_mut()
                .pop()
                .unwrap_or_else(|| SsaTracker::new(Recorder::new()));
            tex.tracker.par.stamps_out(&mut shell);
            {
                let r = tex.tracker.rec.borrow();
                par::prepare(&mut shell, &tex.tracker, &r, snap, u64::MAX, 0);
            }
            let t = std::time::Instant::now();
            let mut fork = base.fork_with(host, shell);
            tex.tracker.par.stats.borrow_mut().fork_ns +=
                u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
            let hash = usize::try_from(crate::web::HASH_BASE).unwrap_or(0) + fork.hash.len();
            if fork.tracker.stamps[0].len() != fork.eqtb.len()
                || fork.tracker.stamps[1].len() != hash
                || fork.tracker.sstamps.len() != fork.save_stack.len() + 16
            {
                fork.size_stamps();
            }
            replies.push(rtx);
            jobs.push(ChunkJob {
                tex: fork,
                fix: Vec::new(),
                input,
                main: main.clone(),
                stop,
                salts: self.salts.clone(),
                max,
                fonts_from: base.font_ptr,
            });
        }
        let base = Arc::new(par::Base::lend(&mut tex.tracker.rec.borrow_mut().st));
        let lens = base.lens();
        for job in &jobs {
            job.tex.tracker.rec.borrow_mut().st.base = Some(base.clone());
        }
        let workers = tex.tracker.par.workers.get();
        let t0 = self.t0;
        let mut first: Option<u64> = None;
        let t = std::time::Instant::now();
        let mut dones =
            par::chunk_round(&mut tex.host, jobs, workers, tx, &rx, &replies, &mut || {
                if first.is_none() {
                    first = t0.map(|t| u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX));
                }
            });
        for d in dones.iter_mut().flatten() {
            if let Some(t) = &mut d.tracker {
                t.rec.borrow_mut().st.base = None;
                tex.tracker.par.stamps_back(t);
            }
        }
        par::Base::restore(base, &mut tex.tracker.rec.borrow_mut().st);
        let mut st = tex.tracker.par.stats.borrow_mut();
        st.passes += 1;
        st.round_ns += u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
        st.first_page_ns = st.first_page_ns.or(first);
        st.runs += dones
            .iter()
            .flatten()
            .map(|d| d.steps.len() as u64)
            .sum::<u64>();
        drop(st);
        self.lens.set(lens);
        dones
    }

    /// Every run's steps taken into the fold in order, as the build's
    /// (`rebuild::cold_guess`), the dirty ones marked in `dirty`: how many
    /// steps were taken in.
    fn take_all<H: Host>(
        &self,
        tex: &mut Tex<H, SsaTracker>,
        runs: Vec<Option<par::ChunkDone>>,
        dirty: &mut rebuild::Dirty,
    ) -> usize {
        let mut taken = 0usize;
        for d in runs.into_iter().flatten() {
            if self.trace {
                std::eprintln!(
                    "phitex: cold: a run: {} steps from {}, {:?}",
                    d.steps.len(),
                    d.steps
                        .first()
                        .map_or_else(alloc::string::String::new, |s| s.start.brief()),
                    d.why
                );
            }
            let Some(w) = d.tracker else { continue };
            for s in d.steps {
                if let Some(d) = rebuild::cold_guess(tex, &w, self.lens.get(), s) {
                    dirty.mark_cold(d);
                }
                taken += 1;
            }
            if let Some(why) = d.why
                && self.trace
            {
                std::eprintln!("phitex: cold: a run cut short: {why}");
            }
            recycle_tracker(tex, Some(w));
        }
        taken
    }
}
