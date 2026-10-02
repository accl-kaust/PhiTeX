//! Part 51: The main program (§1330–§1337), as web2c's `main_body`.
//!
//! partex runs as INITEX: there is no format loading yet, so every job
//! starts from `init_prim`.

use alloc::format;

use crate::cmds::*;
use crate::error::{ERROR_STOP_MODE, FATAL_ERROR_STOP, SPOTLESS, WARNING_ISSUED};
use crate::host::{FileKind, Host};
use crate::input::ux;
use crate::mem::NULL;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// The commands a step runs before a paragraph begins for the paragraph's
/// page builder to be deferred to a step of its own ([`Tex::set_defer_page`]):
/// a picture built in vertical mode before its `\leavevmode`, not the
/// letter that began the paragraph or LaTeX's restart of it with
/// `\noindent` from its `\everypar`.
pub(crate) const DEFER_PAGE_AFTER: u64 = 64;

/// The kinds of clean point ([`Tex::clean_point`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanPoint {
    /// Between paragraphs, on the outer level.
    Outer = 0,
    /// A paragraph's start.
    ParStart = 1,
    /// Before a fire the page builder decided (DESIGN §7.16.1, "The
    /// deferred fire's form"): the step that begins here begins with it.
    Fire = 2,
    /// Before a `\shipout` (SSA mode, with [`Tex::set_stop_before_ship`]):
    /// the step that begins here ships the page, so it alone reads the
    /// sealed lines of the page (`seal.rs`).
    Ship = 3,
    /// At the command after one that read a file whole by name (SSA
    /// mode, with [`Tex::set_stop_after_load`]): what the file's size
    /// (`\pdffilesize`) changes is the step before, which LaTeX's
    /// `\IfFileExists` only tests for being blank.
    Load = 4,
    /// Before the page builder a paragraph's start or end deferred (SSA
    /// mode, with [`Tex::set_defer_page`]): the step that begins here
    /// begins with it, so the paragraph's steps read none of the page's
    /// state, which every paragraph before it on the page changes.
    Page = 5,
    /// A paragraph's start ([`CleanPoint::ParStart`]) on the line its
    /// level began on, in SSA mode with [`Tex::set_defer_page`]: what began
    /// the paragraph read the page's state (its `\parskip`'s page builder,
    /// §1091), the rest of its lines are a step that reads none of it.
    Graf = 6,
}

/// Where [`Tex::start`] or [`Tex::resume`] stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Between two commands (see [`Tex::set_checkpoint_interval`]).
    Checkpoint,
    /// The job is over; `history` (web2c's exit status is 1 if it is
    /// above `warning_issued`).
    Finished(i32),
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §1332: run a whole job, from initialization to
    /// `close_files_and_terminate`. `command_line` is what web2c's
    /// `t_open_in` puts into the buffer: the arguments joined by spaces.
    /// Returns `history`.
    pub fn run(&mut self, command_line: &[u8]) -> i32 {
        let mut step = self.start(command_line);
        loop {
            match step {
                Step::Checkpoint => step = self.resume(),
                Step::Finished(history) => return history,
            }
        }
    }

    /// Like [`Tex::run`], but stop for a checkpoint (0: never) between
    /// two commands of main control, when the whole state is in `self`: a
    /// clone taken then can [`Tex::resume`] the job from there.
    ///
    /// Checkpoints are placed by what the job does, not by counting, at
    /// a command read straight from a file (token lists may be pending
    /// below it):
    /// - where an input file began or ended;
    /// - once per block of `every` input lines: just after the block's
    ///   first shipped page (the page builder is empty then, so an edit
    ///   whose effects stay on its page converges at the next one), or,
    ///   in a block that ships no page (the preamble, a long page), at
    ///   the start of the next block.
    ///
    /// Two runs whose states agree somewhere therefore take their later
    /// checkpoints at the same places, which is what early cutoff
    /// compares.
    pub fn set_checkpoint_interval(&mut self, every: u64) {
        self.checkpoint_every = every;
    }

    /// Is this a checkpoint (see [`Tex::set_checkpoint_interval`])?
    pub(crate) fn checkpoint_due(&mut self) -> bool {
        if self.stop_at != 0 && self.commands >= self.stop_at {
            self.stop_at = 0;
            return true;
        }
        if self.stop_at_candidate
            && (self.cur_input.state != crate::web::TOKEN_LIST || self.fire_pending)
        {
            return true;
        }
        if self.checkpoint_every == 0 {
            return false;
        }
        // (token lists may wait below the file: LaTeX's `\include` reads
        // its file from inside a macro, whose rest stays on the stack)
        if self.cur_input.state == crate::web::TOKEN_LIST {
            return false;
        }
        let every = i64::try_from(self.checkpoint_every).unwrap_or(i64::MAX);
        let block = i64::from(self.line) / every;
        let (file, at_block) = self.checkpoint_at;
        let due = if self.in_open != file {
            true
        } else if block == at_block {
            // after each page shipped: the page builder is empty there, the
            // quiet point where an edited run can meet the previous one
            // (early cutoff, §7.0), so a small edit replays about a page
            self.shipped != self.block_entered.1
        } else if self.block_entered.0 == block {
            // the first page shipped in this block
            self.shipped != self.block_entered.1
        } else {
            // a new block: take it if the last one shipped no page
            let passed = self.block_entered.0 != at_block;
            self.block_entered = (block, self.shipped);
            passed
        };
        if due {
            self.checkpoint_at = (self.in_open, block);
            self.block_entered = (block, self.shipped);
            self.dense_last = false;
            return true;
        }
        // (apart from the others: they fall where they would without)
        let at = (self.in_open, self.line);
        if self.dense && self.mode().abs() == crate::web::VMODE && at != self.dense_at {
            self.dense_at = at;
            self.dense_last = true;
            return true;
        }
        false
    }

    /// Also stop for a checkpoint at each new line of a file read in
    /// vertical mode, between paragraphs (a watch session's resume points
    /// near an edit). The other checkpoints stay where they are.
    pub fn set_dense(&mut self, on: bool) {
        self.dense = on;
    }

    /// Whether the checkpoint just stopped at is only one of
    /// [`Tex::set_dense`]'s.
    #[must_use]
    pub fn dense_checkpoint(&self) -> bool {
        self.dense_last
    }

    /// Stop for a checkpoint before command number `n` (counting as
    /// [`Tex::commands`] does), wherever the input is; 0: no such stop.
    /// The sanitizer's region boundaries (`sanitize.rs`): two runs that
    /// behave alike stop at the same point.
    pub fn set_stop_at(&mut self, n: u64) {
        self.stop_at = n;
    }

    /// Stop (or not) for a checkpoint before every command read straight
    /// from a file, with no token list pending above it: the boundaries
    /// of a machine's steps (`machine.rs`). Exhausted token lists on top
    /// are then popped at `big_switch` ([`Self::pop_exhausted_lists`]).
    pub fn set_stop_at_candidate(&mut self, on: bool) {
        self.stop_at_candidate = on;
    }

    /// Defer (or not) the page builder's fires to the next `big_switch`,
    /// where each is a candidate and a clean point of its own
    /// ([`CleanPoint::Fire`]; DESIGN §7.16.1, "The deferred fire's
    /// form"): the boundaries of an SSA build's steps. Only with
    /// [`Self::set_stop_at_candidate`] and `machine::clean_cuts()` on.
    pub fn set_defer_fire(&mut self, on: bool) {
        self.defer_fire = on;
    }

    /// `big_switch`'s candidate test: the exhausted token lists on top
    /// popped, so that the test sees the file, then
    /// [`Self::checkpoint_due`]. Not before a fire: the output routine
    /// goes on the stack above them, as in tex.web (§311 shows them).
    pub(crate) fn candidate_due(&mut self) -> Result<bool, Jump> {
        if self.stop_at_candidate && crate::machine::clean_cuts() && !self.fire_pending {
            self.pop_exhausted_lists()?;
        }
        Ok(self.checkpoint_due())
    }

    /// Whether a fire the page builder decides now waits for the next
    /// `big_switch` ([`Self::set_defer_fire`]).
    pub(crate) fn deferring_fire(&self) -> bool {
        self.defer_fire && self.stop_at_candidate && crate::machine::clean_cuts()
    }

    /// §357 early: pop the exhausted token lists on top of the input
    /// stack, as the next `get_next` would before reading anything, so
    /// that a command read from the file right after a macro, a
    /// parameter, `\everypar` or an output routine is seen with the file
    /// on top. Only in machine mode (`stop_at_candidate`, with
    /// `machine::set_clean_cuts` on), at `big_switch`.
    ///
    /// Nothing can observe the difference: no token is read between here
    /// and that `get_next`, so `show_context` (§311) cannot run in
    /// between, and `end_token_list` (§324) does to the reference counts,
    /// the parameter stack and `align_state` (a `u_template`'s) what it
    /// would do there. An exhausted `v_template` whose `end_template`
    /// was backed up (§1131 looks for it below) is never on top here:
    /// the backed-up list above it still holds that token (§325).
    pub(crate) fn pop_exhausted_lists(&mut self) -> Result<(), Jump> {
        while self.cur_input.state == crate::web::TOKEN_LIST && self.cur_input.loc == NULL {
            self.end_token_list()?;
        }
        Ok(())
    }

    /// Whether the job, at a candidate boundary (a command about to be
    /// read from a file), is at a clean point (DESIGN §7.16.1), and which
    /// kind. Both kinds have a file on top of the input stack (token
    /// lists may wait below it, §300) and no output routine active
    /// (§1025), in any fixed group context: the group level is not
    /// tested, since the save stack, `cur_level`, `cur_group` and
    /// `cur_boundary` are in `Rest` (`statehash.rs`, "tables"), which a
    /// region's entry guards (LaTeX reads the `.aux` file inside a group,
    /// and every environment is one). The kinds:
    /// - [`CleanPoint::Outer`]: the outer level (`nest_ptr = 0`) in
    ///   vertical mode with the contribution list empty (§215, §994);
    /// - [`CleanPoint::ParStart`]: the first candidate since `new_graf`
    ///   began a paragraph at `nest_ptr = 1` (§1091), its `\everypar`
    ///   list exhausted (popped at `big_switch`), with the nest still
    ///   there (`nest_ptr = 1`, horizontal mode). What the list holds
    ///   then (the indentation box, what `\everypar` appended) is in
    ///   `Rest`;
    /// - [`CleanPoint::Fire`]: a fire is pending (only with the deferral
    ///   on, [`Self::set_defer_fire`]), whatever is on top of the input
    ///   stack;
    /// - [`CleanPoint::Ship`]: stopped before a `\shipout` (SSA mode,
    ///   [`Self::set_stop_before_ship`]), whatever is on top of the input
    ///   stack, the output routine active as a rule;
    /// - [`CleanPoint::Load`]: stopped at the command after one that read
    ///   a file whole by name (SSA mode, [`Self::set_stop_after_load`]),
    ///   whatever is on top of the input stack;
    /// - [`CleanPoint::Page`]: before the page builder a paragraph's end
    ///   (or start) deferred (SSA mode, [`Self::set_defer_page`]), whatever
    ///   is on top of the input stack;
    /// - [`CleanPoint::Graf`]: a [`CleanPoint::ParStart`] on the line the
    ///   paragraph's level began on, in SSA mode.
    ///
    /// The mode is tested with its sign (§211): internal vertical and
    /// restricted horizontal modes are not clean. The paragraph's flag
    /// is cleared here, at the first candidate outside an output routine
    /// after it was set: if the paragraph ended or a level was pushed
    /// before (`\par` right after it), there is no start. (An output
    /// routine that `new_graf`'s `build_page` fires runs before the
    /// paragraph's first command; its stops before a `\shipout`, with a
    /// token list on top, leave the flag alone.)
    pub fn clean_point(&mut self) -> Option<CleanPoint> {
        // (a fire pending: token lists may be on top, and a paragraph's
        // start waits for the next candidate after the routine)
        if self.fire_pending {
            return Some(CleanPoint::Fire);
        }
        // (stopped before a `\shipout`, inside an output routine as a rule:
        // a step's boundary in SSA mode, not the machine's)
        if T::VALUES && self.ship_stop == 1 {
            return Some(CleanPoint::Ship);
        }
        // (stopped after a file read whole: SSA mode's too)
        if T::VALUES && self.load_stop == 2 {
            return Some(CleanPoint::Load);
        }
        // (the page builder a paragraph's end, or its start, deferred)
        if T::VALUES && self.page_pending {
            return Some(CleanPoint::Page);
        }
        if self.cur_input.state == crate::web::TOKEN_LIST || self.output_active() {
            return None;
        }
        let start = core::mem::take(&mut self.par_start);
        match self.nest_ptr() {
            0 if self.mode() == VMODE && self.nodes().is_empty() => Some(CleanPoint::Outer),
            1 if start && self.mode() == HMODE => {
                if T::VALUES && self.defer_page && self.cur_list.ml == self.line {
                    // (a step boundary, SSA mode's: `mode_line` is the line
                    // now, the line its next step begins on)
                    self.graf_stop = true;
                    Some(CleanPoint::Graf)
                } else {
                    Some(CleanPoint::ParStart)
                }
            }
            _ => None,
        }
    }

    /// Stop (or not) for a checkpoint before each `\shipout` (the page
    /// shipped then reads its lines, `seal.rs`): a machine's step
    /// boundaries, inside the output routine.
    pub fn set_stop_before_ship(&mut self, on: bool) {
        self.stop_before_ship = on;
    }

    /// Stop (or not) for a checkpoint at the command after one that read
    /// a file whole by name ([`CleanPoint::Load`]).
    pub fn set_stop_after_load(&mut self, on: bool) {
        self.stop_after_load = on;
    }

    /// Defer (or not) the page builder after a paragraph's end, and after
    /// its start in a step that ran commands before it
    /// ([`DEFER_PAGE_AFTER`]), to the next command, a checkpoint
    /// ([`CleanPoint::Page`]), and stop (or not) at a paragraph's start
    /// ([`CleanPoint::Graf`]): a paragraph's lines are then a step that
    /// reads none of the page's state, which every paragraph before it on
    /// the page changes.
    pub fn set_defer_page(&mut self, on: bool) {
        self.defer_page = on;
    }

    /// §1094's `build_page` after a paragraph, or, deferred, at the next
    /// command ([`Self::set_defer_page`]): nothing runs in between. (An
    /// empty contribution list has nothing to build.)
    pub(crate) fn build_page_after_par(&mut self) -> Result<(), Jump> {
        if T::VALUES && self.defer_page && !self.output_active() && !self.level_list_is_empty(0) {
            self.page_pending = true;
            Ok(())
        } else {
            self.build_page()
        }
    }

    /// The open step began now (SSA mode): [`DEFER_PAGE_AFTER`] counts the
    /// commands from here.
    pub(crate) fn mark_step_start(&mut self) {
        self.step_began = self.commands;
    }

    /// Whether the job stopped just before a `\shipout`.
    #[must_use]
    pub fn stopped_before_ship(&self) -> bool {
        self.ship_stop == 1
    }

    /// The number of commands main control has begun so far.
    pub fn commands(&self) -> u64 {
        self.commands
    }

    /// Start the job; see [`Tex::run`].
    pub fn start(&mut self, command_line: &[u8]) -> Step {
        let r = self.run_inner(command_line);
        self.finish_step(r)
    }

    /// Continue the job after [`Step::Checkpoint`].
    pub fn resume(&mut self) -> Step {
        self.thaw();
        let r = self.run_main();
        self.finish_step(r)
    }

    /// A checkpoint's state flat again, to run on (a no-op for a running
    /// engine).
    /// [`Tex::thaw`], reusing what it can of `old`'s token lists (a
    /// running engine this one replaces: `TokStore::thaw_from`).
    pub(crate) fn thaw_from(&mut self, old: &mut Self) {
        self.thaw_vectors_from(old);
        self.thaw();
    }

    /// Whether this engine's vectors and token lists, thawed, are
    /// `other`'s (also thawed): the `JVec`s whole, the `Flat`s to the live
    /// prefixes the state hash reads, the token lists and their records
    /// (the sanitizer's check of a restore that rebased, `machine.rs`).
    pub(crate) fn same_thawed(&self, other: &Self) -> bool {
        let jvecs = self.eqtb.slices().eq(other.eqtb.slices())
            && self.hash.slices().eq(other.hash.slices())
            && self.save_stack.slices().eq(other.save_stack.slices());
        let prefix = |a: &crate::flat::Flat<u8>, b: &crate::flat::Flat<u8>, n: usize| {
            a.prefix(n).iter().eq(b.prefix(n).iter())
        };
        let flats = prefix(&self.str_pool, &other.str_pool, self.pool_ptr)
            && self
                .str_start
                .prefix(self.str_ptr + 1)
                .iter()
                .eq(other.str_start.prefix(self.str_ptr + 1).iter())
            && prefix(&self.buffer, &other.buffer, self.first.max(self.last))
            && self.input_stack[..self.input_ptr] == other.input_stack[..self.input_ptr]
            && {
                let n = usize::try_from(self.param_ptr).unwrap_or(0);
                self.param_stack[..n] == other.param_stack[..n]
            };
        jvecs && flats && self.eqtb_obj == other.eqtb_obj
    }

    /// The `JVec`s and `Flat`s thawed by rebasing `old`'s running vectors
    /// (`JVec::thaw_from`, `Flat::thaw_from`; `PARTEX_RESTORE_REBASE=0`:
    /// thawed anew).
    fn thaw_vectors_from(&mut self, old: &mut Self) {
        self.eqtb.thaw_from(&mut old.eqtb);
        self.hash.thaw_from(&mut old.hash);
        self.save_stack.thaw_from(&mut old.save_stack);
        self.str_pool.thaw_from(&mut old.str_pool);
        self.str_start.thaw_from(&mut old.str_start);
        self.buffer.thaw_from(&mut old.buffer);
    }

    /// [`Tex::thaw_from`] (or, `from` off, [`Tex::thaw`]), timed by
    /// `clock`: the token store, the `JVec`s, the `Flat`s and the rest
    /// (`PARTEX_CUT_TIMING`).
    pub(crate) fn thaw_timed(
        &mut self,
        old: &mut Self,
        from: bool,
        clock: &dyn Fn() -> u64,
    ) -> [u64; 4] {
        let t0 = clock();
        let t1 = clock();
        if from {
            self.eqtb.thaw_from(&mut old.eqtb);
            self.hash.thaw_from(&mut old.hash);
            self.save_stack.thaw_from(&mut old.save_stack);
        }
        self.eqtb.thaw();
        self.hash.thaw();
        self.save_stack.thaw();
        let t2 = clock();
        if from {
            self.str_pool.thaw_from(&mut old.str_pool);
            self.str_start.thaw_from(&mut old.str_start);
            self.buffer.thaw_from(&mut old.buffer);
        }
        self.str_pool.thaw();
        self.str_start.thaw();
        self.buffer.thaw();
        let t3 = clock();
        self.memo.reserve();
        [t1 - t0, t2 - t1, t3 - t2, clock() - t3]
    }

    pub(crate) fn thaw(&mut self) {
        self.eqtb.thaw();
        self.hash.thaw();
        self.save_stack.thaw();
        self.str_pool.thaw();
        self.str_start.thaw();
        self.buffer.thaw();
        self.memo.reserve();
    }

    /// A checkpoint: a clone that shares with the previous snapshot
    /// everything that has not changed since (DESIGN.md §5.3). A plain
    /// `clone` gives the same state; it only cannot record what it shared,
    /// so the next clone copies again what changed since the last
    /// snapshot. Neither writes to what it clones, so a snapshot can be
    /// read from several threads (`Tex` is `Sync` when its host and
    /// tracker are).
    #[must_use]
    pub fn snapshot(&mut self) -> Self
    where
        H: Clone,
        T: Clone,
    {
        self.commit();
        self.clone()
    }

    /// Make the running state the base the next clones share.
    pub(crate) fn commit(&mut self) {
        self.eqtb.commit();
        self.hash.commit();
        self.save_stack.commit();
        self.commit_flats();
    }

    /// The `Flat`s' part of [`Tex::commit`], each by its live prefix
    /// (what lies past it is dead: TeX writes it before it reads it) and,
    /// for the strings, above their floor (`flat.rs`, `commit_live`):
    /// - the string pool to `pool_ptr` and `str_start` to `str_ptr`
    ///   (§38–§44): characters past `pool_ptr` are written by
    ///   `append_char` before any read; below `str_start[str_ptr]` lie
    ///   the strings made, which change only when `str_ptr` goes down
    ///   (`strings_reopened`);
    /// - the buffer to `max(first, last)`: a line is read into
    ///   `buffer[first..last]` before it is scanned (§31, §362), every
    ///   input level's line lies below `first` (§328, §331), and
    ///   `show_context` reads each level's line to its `limit` (§318);
    /// - the input stack to `input_ptr` (§321 writes a level before
    ///   `input_ptr` passes it; `show_context` and §1335 store `cur_input`
    ///   at `input_stack[input_ptr]` before they read it, §311);
    /// - the parameter stack to `param_ptr` (§390 stores the arguments
    ///   before it raises `param_ptr`).
    ///
    /// These are the prefixes the state hash reads (`statehash.rs`,
    /// "strings", "input: stack", "input: buffer", "input: scanner").
    fn commit_flats(&mut self) {
        // (`get_all`: a checkpoint's vectors are frozen, and commit as is)
        let str_floor = self.str_start.get_all(self.str_ptr);
        self.str_pool.commit_live(self.pool_ptr, str_floor);
        self.str_start
            .commit_live(self.str_ptr + 1, self.str_ptr + 1);
        self.buffer.commit_live(self.first.max(self.last), 0);
    }

    fn finish_step(&mut self, mut r: Result<(), Jump>) -> Step {
        if r == Err(Jump::Checkpoint) {
            // a host that splices outputs cuts them here
            self.flush_outputs();
            return Step::Checkpoint;
        }
        // §81: `jump_out` closes the files (and if that jumps out again, the
        // inner closing runs to completion first).
        while r == Err(Jump::JumpOut) {
            r = self.close_files_and_terminate();
            self.update_terminal();
        }
        // §1332: `final_end: do_final_end` (web2c: `update_terminal` and
        // exit with status by `history`).
        self.update_terminal();
        Step::Finished(self.history())
    }

    fn run_inner(&mut self, command_line: &[u8]) -> Result<(), Jump> {
        self.set_history(FATAL_ERROR_STOP); // in case we quit during initialization
        // §74 (web2c): `-interaction` overrides `error_stop_mode`.
        if let Some(i) = self.params.interaction {
            self.set_interaction(i);
        }
        // `initialize` (§8): the global variables got their starting values
        // in `Tex::new`; now the INITEX table entries.
        if self.params.flavor == crate::params::Flavor::PdfTex {
            // pdfTeX: the timer starts (a read of the host's clock)
            let e = self.host.seconds_and_micros();
            self.clock_read(crate::track::Query::TimerStart, &e);
            self.set_epoch(e);
        }
        if !self.init_tables()? {
            return Err(Jump::FinalEnd);
        }
        self.fix_date_and_time();
        // §55: initialize the output routines.
        self.set_selector(TERM_ONLY);
        self.tally = 0;
        self.term_offset = 0;
        self.file_offset = 0;
        self.offsets_wrote(true, true);
        self.print_banner();
        // §1337: get the first line of input and prepare to start.
        self.init_input_routines(command_line)?;
        let entered = self.enable_etex_if_requested()?;
        let loc = self.cur_input.loc;
        if !entered && (self.format_ident == 0 || self.buffer[ux(loc)] == b'&') {
            let Some(data) = self.open_fmt_file() else {
                return Err(Jump::FinalEnd);
            };
            if !self.load_fmt_file(&data) {
                return Err(Jump::FinalEnd);
            }
            self.format_data = Some(data);
            while self.cur_input.loc < self.cur_input.limit
                && self.buffer[ux(self.cur_input.loc)] == b' '
            {
                self.cur_input.loc += 1;
            }
        }
        self.run_after_format()
    }

    /// §1332's `initialize` and §1337 up to the format's load: the
    /// tables as INITEX makes them (`false`: the pool's strings could not
    /// be started).
    fn init_tables(&mut self) -> Result<bool, Jump> {
        self.init_charset();
        self.init_output();
        if !self.get_strings_started()? {
            return Ok(false);
        }
        self.init_eqtb();
        self.init_xeq_level();
        self.init_hash();
        self.init_nest();
        // §1216
        let s = self.pool_str(b"inaccessible");
        self.set_text(FROZEN_PROTECTION, s);
        // §1369
        let s = self.pool_str(b"endwrite");
        self.set_text(END_WRITE, s);
        self.set_eq_level(END_WRITE, LEVEL_ONE);
        self.set_eq_type(END_WRITE, OUTER_CALL);
        self.set_equiv(END_WRITE, NULL);
        // §1301
        self.format_ident = if self.params.ini {
            self.pool_str(b" (INITEX)")
        } else {
            0
        };
        self.init_prim()?; // call `primitive` for each primitive
        // §1337 (INITEX without a format): the null font. A loaded format
        // replaces it.
        self.init_null_font();
        self.init_str_ptr = self.str_ptr;
        self.init_pool_ptr = self.pool_ptr;
        self.pool_ready();
        Ok(true)
    }

    /// §1337 after the format's load, to `main_control`.
    fn run_after_format(&mut self) -> Result<(), Jump> {
        if self.etex_ex() {
            self.term_bytes(b"entering extended mode\n");
        }
        if self.end_line_char_inactive() {
            self.cur_input.limit -= 1;
        } else {
            let c = self.int_par(END_LINE_CHAR_CODE);
            self.buffer[ux(self.cur_input.limit)] = u8::try_from(c).unwrap_or(0);
        }
        if self.mltex_enabled_p {
            self.term_bytes(b"MLTeX v2.2 enabled\n");
        }
        self.fix_date_and_time();
        self.fonts.used.fill(false);
        if self.params.flavor == crate::params::Flavor::PdfTex {
            // pdfTeX: the default random seed
            let (s, m) = self.epoch();
            self.random.random_seed = m.wrapping_mul(1000).wrapping_add(s % 1_000_000);
            self.random.init_randoms(self.random.random_seed);
            self.random_wrote();
        }
        // §75: initialize the print `selector` based on `interaction`.
        let sel = if self.interaction() == BATCH_MODE {
            NO_PRINT
        } else {
            TERM_ONLY
        };
        self.set_selector(sel);
        let loc = self.cur_input.loc;
        if loc < self.cur_input.limit && self.cat_code(i32::from(self.buffer[ux(loc)])) != ESCAPE {
            self.start_input()?; // \input assumed
        }
        self.set_history(SPOTLESS); // ready to go!
        // §1030: `main_control` begins with `\everyjob`.
        self.begin_toks_at(EVERY_JOB_LOC, EVERY_JOB_TEXT)?;
        self.run_main()
    }

    fn run_main(&mut self) -> Result<(), Jump> {
        self.main_control()?; // come to life
        self.final_cleanup()?; // prepare for death
        self.close_files_and_terminate()
    }

    /// §61 (web2c): the rest of "initialize the output routines": the
    /// banner on the terminal (`wterm` leaves `term_offset` alone).
    fn print_banner(&mut self) {
        let banner = self.banner();
        self.term_bytes(banner);
        self.term_bytes(self.params.version_string);
        if self.format_ident == 0 {
            let mut b = b" (preloaded format=".to_vec();
            b.extend_from_slice(&self.params.dump_name);
            b.extend_from_slice(b")\n");
            self.term_bytes(&b);
        } else {
            self.slow_print(self.format_ident);
            self.print_ln();
        }
        if self.params.shell_escape {
            self.term_bytes(b" ");
            if self.params.restricted_shell {
                self.term_bytes(b"restricted ");
            }
            self.term_bytes(b"\\write18 enabled.\n");
        }
        self.update_terminal();
    }

    /// §331: initialize the input routines, with web2c's `t_open_in` and
    /// §37's `init_terminal`.
    fn init_input_routines(&mut self, command_line: &[u8]) -> Result<(), Jump> {
        self.input_ptr = 0;
        self.max_in_stack = 0;
        self.source_filename_stack[0] = 0;
        self.full_source_filename_stack[0] = 0;
        self.in_open = 0;
        self.grp_stack[0] = 0;
        self.if_stack[0] = 0;
        self.set_open_parens(0);
        self.max_buf_stack = 0;
        self.param_ptr = 0;
        self.max_param_stack = 0;
        self.buffer.fill(0);
        self.scanner_status = NORMAL;
        self.warning_index = NULL;
        self.first = 1;
        self.cur_input.state = NEW_LINE;
        self.cur_input.start = 1;
        self.cur_input.index = 0;
        self.line = 0;
        self.cur_input.name = 0;
        self.force_eof = false;
        self.set_align_state(1_000_000);
        if !self.init_terminal(command_line)? {
            return Err(Jump::FinalEnd);
        }
        self.cur_input.limit = i32::try_from(self.last).unwrap_or(0);
        self.first = self.last + 1; // `init_terminal` has set `loc` and `last`
        Ok(())
    }

    /// §37: get the terminal input started.
    fn init_terminal(&mut self, command_line: &[u8]) -> Result<bool, Jump> {
        // web2c's `t_open_in`: the command line goes into the buffer,
        // without trailing blanks.
        let mut k = self.first;
        if k + command_line.len() + 1 >= self.buffer.len() {
            return Ok(false);
        }
        for &c in command_line {
            self.buffer[k] = c;
            k += 1;
        }
        let mut last = k;
        while last > self.first && matches!(self.buffer[last - 1], b' ' | b'\t') {
            last -= 1;
        }
        self.last = last;
        loop {
            if self.last > self.first {
                let mut loc = self.first;
                while loc < self.last && self.buffer[loc] == b' ' {
                    loc += 1;
                }
                if loc < self.last {
                    self.cur_input.loc = i32::try_from(loc).unwrap_or(0);
                    return Ok(true);
                }
            }
            self.term_bytes(b"**");
            self.update_terminal();
            if !self.input_ln_terminal()? {
                // this shouldn't happen
                self.term_bytes(b"\n! End of file on the terminal... why?\n");
                return Ok(false);
            }
            // (the loop tests the line; web2c prints this only for an
            // all-blank typed line)
            let mut loc = self.first;
            while loc < self.last && self.buffer[loc] == b' ' {
                loc += 1;
            }
            if loc < self.last {
                self.cur_input.loc = i32::try_from(loc).unwrap_or(0);
                return Ok(true);
            }
            self.term_bytes(b"Please type the name of your input file.\n");
        }
    }

    /// §524: find the format named by `&name` on the first line, or the
    /// default one; returns its contents.
    fn open_fmt_file(&mut self) -> Option<alloc::sync::Arc<[u8]>> {
        let mut j = ux(self.cur_input.loc);
        if self.buffer[j] == b'&' {
            self.cur_input.loc += 1;
            j = ux(self.cur_input.loc);
            self.buffer[self.last] = b' ';
            while self.buffer[j] != b' ' {
                j += 1;
            }
            let mut name = self.buffer[ux(self.cur_input.loc)..j].to_vec();
            name.extend_from_slice(b".fmt");
            if let Some(f) = self.host.read_file(&name, FileKind::Fmt) {
                self.cur_input.loc = i32::try_from(j).unwrap_or(0);
                return Some(f.contents);
            }
            let mut m = b"Sorry, I can't find the format `".to_vec();
            m.extend_from_slice(&name);
            m.extend_from_slice(b"'; will try `");
            m.extend_from_slice(&self.params.dump_name);
            m.extend_from_slice(b".fmt'.\n");
            self.term_bytes(&m);
            self.update_terminal();
        }
        // now pull out all the stops: try for the system plain file
        let mut name = self.params.dump_name.clone();
        name.extend_from_slice(b".fmt");
        if let Some(f) = self.host.read_file(&name, FileKind::Fmt) {
            self.cur_input.loc = i32::try_from(j).unwrap_or(0);
            return Some(f.contents);
        }
        let mut m = b"I can't find the format file `".to_vec();
        m.extend_from_slice(&name);
        m.extend_from_slice(b"'!\n");
        self.term_bytes(&m);
        None
    }

    /// §1335: prepare for death.
    fn final_cleanup(&mut self) -> Result<(), Jump> {
        let c = self.cur_chr; // 0 for \end, 1 for \dump
        if c != 1 {
            self.set_int_par(NEW_LINE_CHAR_CODE, -1);
        }
        if self.job_name() == 0 {
            self.open_log_file()?;
        }
        while self.input_ptr > 0 {
            if self.cur_input.state == TOKEN_LIST {
                self.end_token_list()?;
            } else {
                self.end_file_reading();
            }
        }
        while self.open_parens() > 0 {
            self.print_str(b" )");
            self.set_open_parens(self.open_parens() - 1);
        }
        if self.cur_level() > LEVEL_ONE {
            self.print_nl(b"(");
            self.print_esc(b"end occurred ");
            self.print_str(b"inside a group at level ");
            self.print_int(self.cur_level() - LEVEL_ONE);
            self.print_char(b')');
            if self.etex_ex() {
                self.show_save_groups();
            }
        }
        self.cond_read();
        while !self.cond_stack.is_empty() {
            self.print_nl(b"(");
            self.print_esc(b"end occurred ");
            self.print_str(b"when ");
            self.print_cmd_chr(IF_TEST, self.cur_if);
            if self.if_line != 0 {
                self.print_str(b" on line ");
                self.print_line_no(usize::MAX, self.if_line);
            }
            self.print_str(b" was incomplete)");
            self.pop_cond();
        }
        if self.history() != SPOTLESS
            && (self.history() == WARNING_ISSUED || self.interaction() < ERROR_STOP_MODE)
            && self.selector() == TERM_AND_LOG
        {
            self.set_selector(TERM_ONLY);
            self.print_nl(b"(see the transcript file for additional information)");
            self.set_selector(TERM_AND_LOG);
        }
        if c == 1 {
            if self.params.ini {
                return self.store_fmt_file();
            }
            self.print_nl(b"(\\dump is performed only by INITEX)");
        }
        Ok(())
    }

    /// §1333: the full version of `close_files_and_terminate`.
    pub(crate) fn close_files_and_terminate(&mut self) -> Result<(), Jump> {
        // §1378: finish the extensions.
        for k in 0..16 {
            self.out_read(u8::try_from(k).unwrap_or(0));
            if self.write_open(k)
                && let Some(id) = self.write_file[k].id.take()
            {
                let buf = core::mem::take(&mut self.write_file[k].buf);
                self.write_file_bytes(id, &buf);
                self.write_file_close(id);
                // (the stream keeps its name: TeX leaves `write_open` set;
                // its file is closed, a write of `Out(k)`)
                self.out_wrote(u8::try_from(k).unwrap_or(0));
            }
        }
        self.set_int_par(NEW_LINE_CHAR_CODE, -1);
        if self.int_par(TRACING_STATS_CODE) > 0 {
            self.output_statistics();
        }
        if self.params.flavor == crate::params::Flavor::PdfTex {
            // pdfTeX §1333
            if !self.pdf_output_fixed().0 {
                self.fix_pdfoutput()?;
            }
        }
        if self.pdf_output_fixed().1 > 0 {
            if self.history() == crate::error::FATAL_ERROR_STOP {
                self.print_err(b" ==> Fatal error occurred, no output PDF file produced!");
            } else {
                self.finish_pdf_file()?;
                if self.log_opened() {
                    self.pdf_statistics();
                }
            }
        } else {
            self.finish_dvi_file()?;
        }
        if self.log_opened() {
            self.wlog_bytes(b"\n");
            self.flush_log();
            if self.log_open()
                && let Some(id) = self.log_file.id.take()
            {
                self.out_close(id);
                self.out_wrote(crate::streams::LOG);
            }
            self.set_selector(self.selector() - 2);
            if self.selector() == TERM_ONLY {
                self.print_nl(b"Transcript written on ");
                self.print_file_name(0, self.log_name(), 0);
                self.print_char(b'.');
            }
        }
        self.print_ln();
        self.update_terminal();
        Ok(())
    }

    /// §1334: output statistics about this job.
    fn output_statistics(&mut self) {
        if !self.log_opened() {
            return;
        }
        let i = |x: usize| i32::try_from(x).unwrap_or(i32::MAX);
        let p = &self.params;
        let strings = i(self.str_ptr) - i(self.init_str_ptr);
        let fonts = self.font_ptr - FONT_BASE;
        self.font_table_read();
        self.exceptions_read();
        let hc = self.hyph.hyph_count;
        let s = format!(
            " \nHere is how much of TeX's memory you used:\n \
             {strings} string{} out of {}\n \
             {} string characters out of {}\n \
             {} words of memory out of {}\n \
             {} multiletter control sequences out of {}+{}\n \
             {} words of font info for {fonts} font{}, out of {} for {}\n \
             {hc} hyphenation exception{} out of {}\n \
             {}i,{}n,{}p,{}b,{}s stack positions out of {}i,{}n,{}p,{}b,{}s\n",
            if strings == 1 { "" } else { "s" },
            p.max_strings - i(self.init_str_ptr),
            i(self.pool_ptr) - i(self.init_pool_ptr),
            p.pool_size - i(self.init_pool_ptr),
            {
                let (var_used, dyn_used) = self.memory_usage();
                var_used + dyn_used
            },
            p.main_memory,
            self.cs_count,
            HASH_SIZE,
            p.hash_extra,
            self.fmem_ptr,
            if fonts == 1 { "" } else { "s" },
            p.font_mem_size,
            p.font_max - FONT_BASE,
            if hc == 1 { "" } else { "s" },
            p.hyph_size,
            self.max_in_stack,
            self.max_nest_stack,
            self.max_param_stack,
            self.max_buf_stack + 1,
            self.max_save_stack + 6,
            p.stack_size,
            p.nest_size,
            p.param_size,
            p.buf_size,
            p.save_size,
        );
        self.wlog_bytes(s.as_bytes());
    }
}

impl Tex<crate::host::NoHost, crate::track::Untracked> {
    /// The format's definitions (DESIGN 7.17.3): an engine that made its
    /// tables as a job does (§1332, §1337) and loaded the format `data`
    /// (none: INITEX's tables), and that runs nothing. What a slot holds
    /// in it is what the slot holds before any step of a build from that
    /// format defines it.
    pub(crate) fn format_value(params: crate::params::Params, data: Option<&[u8]>) -> Option<Self> {
        let mut t = Tex::new(crate::host::NoHost, crate::track::Untracked, params);
        if !t.init_tables().ok()? {
            return None;
        }
        if let Some(d) = data
            && !t.load_fmt_file(d)
        {
            return None;
        }
        Some(t)
    }
}
