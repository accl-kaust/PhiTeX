//! One command (§1029–§1045 `main_control`'s big switch, §1208–§1298
//! `prefixed_command` and the mode-independent commands): what a node
//! does with the first token of its input.

use crate::exec::LockWait as _;
use std::rc::Rc;

use phitex_defuse::{Value, ValueId};
use phitex_typeset::glue::Glue;

use crate::fields::Par;
use crate::run::{R, Run};
use crate::state::{Entry, Loc};
use crate::val::{
    CAT_CODE_BASE, DEL_CODE_BASE, MATH_CODE_BASE, Macro, Mean, OUTPUT_ROUTINE_LOC, SF_CODE_BASE,
    TOKS_BASE, Tok, cmd, glue_of, int_of, list_of, str_of, toks_value,
};

const BOX_FLAG: i64 = 0o10000000000;
/// A read stream's state when it opened a document's file cut into
/// units (`[LAZY_OPEN, name]`): open, its lines not read yet
/// ([`Run::next_read_line`] reads them).
const LAZY_OPEN: i64 = 3;
pub(crate) const SHIP_OUT_FLAG: i64 = BOX_FLAG + 512;
pub(crate) const LEADER_FLAG: i64 = BOX_FLAG + 513;

/// `PHITEX_HEARTBEAT_FROM`: the command count the per-command trace starts at.
static HEARTBEAT_FROM: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

/// What the stuck check ([`Run::watchdog`]) last saw of a run: its lines
/// read, its assignments' clock, when a line was last read, the command
/// count at its last progress, and the checks made.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Watchdog {
    lines: (i64, usize),
    stamp: u64,
    at: Option<std::time::Instant>,
    cmds_at: u64,
    ticks: u64,
}

impl Run<'_, '_> {
    /// The node's command: expand once, or do one command (§1030). An
    /// expansion is followed by the next command here, as §1030's
    /// `get_x_token` does, where nothing [`Run::run_all`] does between
    /// commands is due ([`Run::expansion_goes_on`]).
    pub(crate) fn command(&mut self) -> R<()> {
        loop {
            phitex_pdf::phase::bump(23);
            // (spec-final §2.4: a run-local count and one branch; the
            // heartbeat's own work every `HB_BATCH` commands)
            self.cmds += 1;
            if self.cmds >= self.hb_at {
                if Self::heartbeat_on() {
                    let done = self.hb_done;
                    self.heartbeat_tick();
                    // (the engine bounds a run on a guess by its work)
                    self.cx.tick(self.cmds - done)?;
                } else {
                    self.hb_done = self.cmds;
                    self.hb_at = self.cmds + Self::HB_BATCH;
                    // (a run whose read moved while it ran stops here, not
                    // at its commit: a loop on what moved never commits)
                    self.cx.tick(Self::HB_BATCH)?;
                }
                self.watchdog()?;
            }
            self.no_pull = true;
            let got = self.get_next();
            self.no_pull = false;
            // (the command loop's one check of the token path's halt)
            self.ck()?;
            if !got {
                return Ok(());
            }
            if crate::exec::obs_flag!("PHITEX_CMD_TRACE") {
                self.cmd_trace();
            }
            if self.cur.cmd > cmd::MAX_COMMAND {
                if self.cur.cmd >= cmd::END_TEMPLATE
                    && self.cur.cmd != cmd::DONT_EXPAND
                    && self.cur.cmd != cmd::SYM
                {
                    return Ok(());
                }
                self.expand();
                self.ck()?;
                if self.expansion_goes_on() {
                    self.pdf_cmd = 0;
                    continue;
                }
                return Ok(());
            }
            self.dispatch()?;
            return self.ck();
        }
    }

    /// Whether the command after an expansion begins at once: what
    /// [`Run::run_all`] checks between commands holds no work (no claim
    /// or scope end to stop at, no sibling pulled, no mark due, no
    /// compaction, a token list with tokens on top: no file to stop at).
    #[inline]
    fn expansion_goes_on(&mut self) -> bool {
        if crate::exec::obs_flag!("PHITEX_ANCHOR_MARKS")
            || self.pulled != 0
            || self.claim_rest
            || self.closed_scope()
            || self.input.arena.len() > self.input.compact_at.max(crate::input::ARENA_COMPACT)
        {
            return false;
        }
        self.input.pop_spent();
        self.input
            .levels
            .last()
            .is_some_and(|l| l.kind == crate::input::kind::TOKS)
    }

    /// (observe) `PHITEX_CMD_TRACE=<node>` (`all`: every node): each
    /// command the node's run does, its token, `cmd`/`chr` and the input
    /// levels (kind, type) it is read from, innermost last.
    #[cold]
    #[inline(never)]
    fn cmd_trace(&self) {
        if !self.cmd_traced() {
            return;
        }
        let node = self.cx.node().raw();
        let levels: Vec<(u8, u8)> = self.input.levels.iter().map(|l| (l.kind, l.ty)).collect();
        eprintln!(
            "cmd-trace {node} #{} {:?} cmd {} chr {} levels {levels:?}",
            self.cmds, self.cur.tok, self.cur.cmd, self.cur.chr
        );
    }

    /// (observe) Whether `PHITEX_CMD_TRACE` names this run's node.
    pub(crate) fn cmd_traced(&self) -> bool {
        let Some(want) = crate::exec::env_str!("PHITEX_CMD_TRACE") else {
            return false;
        };
        want == "all" || want.parse::<u64>().ok() == Some(self.cx.node().raw())
    }

    /// The commands between two heartbeat ticks ([`Run::heartbeat_tick`]).
    pub(crate) const HB_BATCH: u64 = 4096;

    /// Whether the heartbeat's diagnostics are on (`PHITEX_HEARTBEAT=1`).
    fn heartbeat_on() -> bool {
        crate::exec::obs_flag!("PHITEX_HEARTBEAT")
    }

    /// The run's progress, every [`Run::HB_BATCH`] commands (always: a
    /// clock read, a walk of the input's levels and a few compares): a run
    /// that reads no new line of its input (a document line, or a file's
    /// read here) for `Limits::stall`, that runs 20 million commands
    /// without an assignment or a new line, or past `Limits::memory_bytes`
    /// resident (looked at every 64 checks), or past the build's time
    /// limit (`Limits::time`), is stuck, and the evaluation
    /// ends with `EvalError::Stuck` naming it and where it is. A
    /// one-letter edit of amsldoc left a page's run going for minutes at
    /// 5 GB and growing, the watch waiting on it with no way out.
    pub(crate) fn watchdog(&mut self) -> R<()> {
        let lim = self.cx.limits();
        let now = std::time::Instant::now();
        // (input read: the document lines entered, and where each file
        // read here is; a package's lines are a file's)
        let off: usize = self
            .input
            .levels
            .iter()
            .filter(|l| l.kind == crate::input::kind::FILE)
            .map(|l| self.input.file(*l).off)
            .sum();
        let lines = (self.lines_entered, off);
        let (stamp, cmds) = (self.st.stamp(), self.cmds);
        let w = &mut self.wd;
        let Some(at) = w.at else {
            *w = Watchdog {
                lines,
                stamp,
                at: Some(now),
                cmds_at: cmds,
                ticks: 0,
            };
            return Ok(());
        };
        let mut at = at;
        if lines != w.lines {
            w.lines = lines;
            w.at = Some(now);
            at = now;
            w.cmds_at = cmds;
        }
        if stamp != w.stamp {
            w.stamp = stamp;
            w.cmds_at = cmds;
        }
        w.ticks += 1;
        let (still, ticks) = (cmds - w.cmds_at, w.ticks);
        let since = now.duration_since(at);
        let why = if let Some(t) = lim.time
            && self.cx.elapsed().is_some_and(|e| e > t)
        {
            // (the build's time limit, `Options::time_limit`: the
            // scheduler checks it between runs, a run that never ends
            // checks it here)
            Some(format!(
                "the build passed its time limit of {} s",
                t.as_secs()
            ))
        } else if lim.stall.is_some_and(|s| since >= s) {
            Some(format!(
                "it read no new line of its input for {} s",
                since.as_secs()
            ))
        } else if still >= 20_000_000 {
            Some(format!(
                "it ran {} million commands without an assignment or a new line of input, an endless loop",
                still / 1_000_000
            ))
        } else if ticks % 64 == 0
            && let Some(m) = lim.memory_bytes
            && crate::api::rss_kb() << 10 > m
        {
            #[allow(clippy::cast_precision_loss, reason = "a display of bytes")]
            let gb = |b: u64| b as f64 / f64::from(1u32 << 30);
            Some(format!(
                "the process holds {:.1} GB, past the limit of {:.1} GB",
                gb(crate::api::rss_kb() << 10),
                gb(m)
            ))
        } else {
            None
        };
        match why {
            Some(why) => {
                let at = self.where_now();
                Err(crate::run::Halt::Stop(
                    self.cx.stuck(format!("{why}; at {at}")),
                ))
            }
            None => Ok(()),
        }
    }

    /// Where the run is, for a report: the innermost file's name and line,
    /// the command it is on, and how deep its input and its lists are.
    pub(crate) fn where_now(&self) -> String {
        // (the innermost file read here, by line; else the innermost line
        // of a document piece, by its byte in its file: `the main file
        // byte N` for the main text, which the front end turns into a line)
        let place = self.input.levels.iter().rev().find_map(|l| match l.kind {
            crate::input::kind::FILE => {
                let (file, line) = self.input.file(*l).where_(self.v);
                Some(format!("{file}:{line}"))
            }
            crate::input::kind::LINE => {
                let s = &self.input.lines[l.a as usize];
                s.origin.map(|(name, pos, _)| {
                    let name = crate::val::str_of(self.v, name);
                    let name = if name.starts_with('\u{10FFFE}') {
                        "the main file".to_owned()
                    } else {
                        name
                    };
                    if s.lineno > 0 {
                        format!("{name}:{}", s.lineno)
                    } else {
                        format!("{name} byte {pos}")
                    }
                })
            }
            _ => None,
        });
        let mut last = String::new();
        crate::print::print_cs(&mut last, self.cur.tok, i64::from(b'\\'), &mut |c| {
            if char::from_u32(c).is_some_and(char::is_alphabetic) {
                11
            } else {
                12
            }
        });
        let place = place.unwrap_or_else(|| "no file".to_owned());
        format!(
            "{place}, on {last}, input {} levels deep, {} lists open, {} commands run",
            self.input.depth(),
            self.nest.len(),
            self.cmds
        )
    }

    /// Where the heartbeat's first tick of a run is: every command
    /// while a trace window is asked for (`PHITEX_HEARTBEAT_FROM`), every
    /// [`Run::HB_BATCH`] commands with `PHITEX_HEARTBEAT=1`, else every
    /// [`Run::HB_BATCH`] for the stuck check ([`Run::watchdog`]) alone.
    pub(crate) fn heartbeat_start() -> u64 {
        if !crate::exec::obs_flag!("PHITEX_HEARTBEAT") {
            return Self::HB_BATCH;
        }
        if std::env::var_os("PHITEX_HEARTBEAT_FROM").is_some() {
            1
        } else {
            Self::HB_BATCH
        }
    }

    /// With `PHITEX_HEARTBEAT=1`, every 100,000 commands on this thread:
    /// elapsed seconds, commands, the innermost file and line, and the
    /// run's sizes, on stderr (a diagnostic for long serial runs). The
    /// run's commands since the last tick are added to the thread's.
    #[allow(clippy::too_many_lines)]
    fn heartbeat_tick(&mut self) {
        use std::cell::Cell;
        use std::sync::OnceLock;
        use std::time::Instant;
        static EXIT: OnceLock<u64> = OnceLock::new();
        thread_local! {
            static N: Cell<u64> = const { Cell::new(0) };
            static T0: Cell<Option<Instant>> = const { Cell::new(None) };
        }
        let delta = self.cmds - self.hb_done;
        self.hb_done = self.cmds;
        self.hb_at = if Self::heartbeat_start() == 1 {
            self.cmds + 1
        } else {
            self.cmds + Self::HB_BATCH
        };
        let (before, n) = N.with(|c| {
            let b = c.get();
            c.set(b + delta);
            (b, b + delta)
        });
        let t0 = T0.with(|c| {
            let t = c.get().unwrap_or_else(Instant::now);
            c.set(Some(t));
            t
        });
        let from = *HEARTBEAT_FROM.get_or_init(|| {
            std::env::var("PHITEX_HEARTBEAT_FROM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(u64::MAX)
        });
        let trace = n >= from && n < from.saturating_add(150_000);
        if before / 100_000 == n / 100_000 && !trace {
            return;
        }
        // `PHITEX_HEARTBEAT_EXIT`: the process ends at this many commands
        // (a profile of a run's first commands)
        let exit = *EXIT.get_or_init(|| {
            std::env::var("PHITEX_HEARTBEAT_EXIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(u64::MAX)
        });
        if n >= exit {
            std::process::exit(0);
        }
        // (a run whose state stays the same for 10 M commands loops: its
        // nest, once, to find the construct)
        thread_local! {
            static SAME: Cell<(u64, u64, u64)> = const { Cell::new((0, 0, 0)) };
        }
        let sig = self.st.sig().3;
        let (last_sig, same, dumped) = SAME.with(Cell::get);
        let same = if sig == last_sig { same + 1 } else { 0 };
        SAME.with(|c| c.set((sig, same, dumped)));
        if same >= 100 && dumped == 0 {
            SAME.with(|c| c.set((sig, same, 1)));
            for (k, l) in self.nest.iter().enumerate() {
                eprintln!(
                    "hbloop level {k}: mode {} group {} lid {} owns {} par {:?} list {} extra {}",
                    l.mode,
                    l.group,
                    l.lid,
                    l.owns,
                    l.par,
                    l.list.len(),
                    l.extra.len()
                );
            }
            for l in self.input.levels.iter().rev() {
                let d = match l.kind {
                    crate::input::kind::FILE => {
                        let (file, line) = self.input.file(*l).where_(self.v);
                        format!("file {file}:{line}")
                    }
                    crate::input::kind::LINE => "text".to_owned(),
                    _ => format!("tokens {}/{}", l.a - l.c, l.b - l.c),
                };
                eprintln!("hbloop frame: {d}");
            }
        }
        let (file, line) = self
            .input
            .levels
            .iter()
            .rev()
            .find_map(|l| {
                (l.kind == crate::input::kind::FILE).then(|| self.input.file(*l).where_(self.v))
            })
            .unwrap_or_default();
        let mut last = String::new();
        crate::print::print_cs(&mut last, self.cur.tok, i64::from(b'\\'), &mut |c| {
            if char::from_u32(c).is_some_and(char::is_alphabetic) {
                11
            } else {
                12
            }
        });
        eprintln!(
            "hb{} {:.1}s cmds {n} {file}:{line} last {last} cmd {} mode {} state {:?} nest {} frames {}",
            if trace { "t" } else { "" },
            t0.elapsed().as_secs_f64(),
            self.cur.cmd,
            self.nest.last().map_or(0, |l| l.mode),
            self.st.sig(),
            self.nest.len(),
            self.input.depth()
        );
    }

    /// `report_illegal_case` (§1050): "You can't use" the command "in"
    /// the mode (§211 `print_mode`, §298 `print_cmd_chr`).
    pub(crate) fn report_illegal_case(&mut self, md: &crate::build::Mode) -> R<()> {
        let what = crate::val::prim_name(self.cur.cmd, self.cur.chr).map_or_else(
            || match self.cur.tok.as_cs() {
                Some(n) => format!("\\{n}"),
                None => char::from_u32(self.cur.tok.chr()).map_or_else(String::new, String::from),
            },
            |n| format!("\\{n}"),
        );
        let mode = match md.m {
            1 => "vertical",
            102 => "horizontal",
            203 => "display math",
            -1 => "internal vertical",
            -102 => "restricted horizontal",
            -203 => "math",
            _ => "no",
        };
        self.log("error", &format!("You can't use `{what}' in {mode} mode"))
    }

    /// `reswitch` (§1045): the case for `cur` in the current mode.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn dispatch(&mut self) -> R<()> {
        // (§1210's prefixed commands are the same in every mode, and
        // `\relax` does nothing: no mode read)
        let c = self.cur.cmd;
        // (XeTeX's `othercases` after a run of characters:
        // `check_for_post_char_toks(big_switch)`)
        if self.xe
            && self.ic_prev != crate::xetex::CLASS_BOUNDARY
            && !matches!(
                c,
                cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN | cmd::CHAR_NUM | cmd::NO_BOUNDARY
            )
            && self.ic_post()?
        {
            return Ok(());
        }
        if (cmd::TOKS_REGISTER..=cmd::SET_INTERACTION).contains(&c) {
            return self.prefixed_command();
        }
        if c == cmd::RELAX {
            return Ok(());
        }
        // (a space outside horizontal mode does nothing, §1045)
        if c == cmd::SPACER && self.mode_m()?.abs() != 102 {
            return Ok(());
        }
        let md = self.mode()?;
        if crate::etex::command(self, &md)? {
            return Ok(());
        }
        let am = md.m.abs();
        let (v, h) = (am == 1, am == 102);
        let c = self.cur.cmd;
        if am == 203 {
            if self.math_command(&md)? {
                return Ok(());
            }
        } else if crate::math::non_math(c) {
            return self.insert_dollar_sign();
        }
        match c {
            cmd::RELAX => {}
            cmd::IGNORE => self.do_endv(&md)?,
            cmd::NO_ALIGN if self.cur.chr == 1 => self.align_peek(&md)?,
            cmd::SPACER => {
                if h {
                    self.char_run(&md, false)?;
                }
            }
            cmd::LETTER
            | cmd::OTHER_CHAR
            | cmd::CHAR_GIVEN
            | cmd::CHAR_NUM
            | cmd::NO_BOUNDARY
            | cmd::EX_SPACE => {
                if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if h {
                    self.char_run(&md, false)?;
                }
            }
            cmd::IGNORE_SPACES => {
                // (vertical mode, a document run: a token of a sibling
                // pulled during the scan ends the command unexpanded,
                // put back, so the sibling can be given back at the next
                // command's start; expanded there instead, as here, since
                // vertical mode ignores the blanks an expansion leaves,
                // §1045)
                if self.doc && self.aopen == 0 && v {
                    let p0 = self.pulled;
                    loop {
                        self.get_next_or_end()?;
                        if self.pulled > p0 {
                            self.back_cur();
                            return Ok(());
                        }
                        let c = self.cur.cmd;
                        if c == cmd::SPACER {
                            continue;
                        }
                        if c <= cmd::MAX_COMMAND {
                            break;
                        }
                        if (cmd::CALL..cmd::END_TEMPLATE).contains(&c) {
                            self.macro_call();
                            self.ck()?;
                            continue;
                        }
                        if c >= cmd::END_TEMPLATE && c != cmd::DONT_EXPAND && c != cmd::SYM {
                            break;
                        }
                        self.expand();
                        self.ck()?;
                    }
                } else {
                    self.skip_blanks_x()?;
                }
                // (a run that read past its text: the token found ends
                // the command, put back, so that at the next command's
                // start a sibling whose first token it is can be given
                // back and run as its own node, `Run::give_back_peeked`;
                // the same token is read again, §1045's reswitch)
                if self.doc
                    && self.pulled > 0
                    && self.aopen == 0
                    && (!self.cur.tok.is_cs() || self.cur.cmd != cmd::RELAX)
                {
                    self.back_cur();
                    return Ok(());
                }
                return self.dispatch();
            }
            cmd::STOP => {
                if v {
                    self.its_all_over(&md)?;
                } else if h {
                    self.head_for_vmode(&md);
                }
            }
            cmd::HRULE | cmd::VRULE => {
                if (c == cmd::HRULE) == v {
                    let (w, ht, d) = self.scan_rule_spec(c == cmd::VRULE)?;
                    let node = self.v.list(vec![
                        self.v.int(crate::nodes::T_RULE),
                        self.v.int(i64::from(w)),
                        self.v.int(i64::from(ht)),
                        self.v.int(i64::from(d)),
                    ]);
                    self.append(md.lid, node);
                    if v {
                        self.set_prev_depth(&md, crate::build::IGNORE_DEPTH);
                    } else {
                        self.set_space_factor(&md, 1000);
                    }
                } else if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if h {
                    self.head_for_vmode(&md);
                }
            }
            cmd::VSKIP | cmd::HSKIP | cmd::MSKIP => {
                if (c == cmd::VSKIP) == v {
                    self.append_glue(&md)?;
                } else if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if h {
                    self.head_for_vmode(&md);
                }
            }
            cmd::KERN | cmd::MKERN => {
                // §1061
                let s = self.cur.chr;
                let w = self.scan_dimen(s == 99, false, None)?;
                let k = if s == 99 {
                    phitex_typeset::node::KernKind::Mu
                } else {
                    phitex_typeset::node::KernKind::Explicit
                };
                let node = crate::nodes::kern_node(self.v, w, k);
                self.append(md.lid, node);
            }
            cmd::LEFT_BRACE => self.open_group(&md, crate::val::group::SIMPLE, vec![])?,
            cmd::BEGIN_GROUP => self.open_group(&md, crate::val::group::SEMI_SIMPLE, vec![])?,
            cmd::END_GROUP => {
                if md.group == crate::val::group::SEMI_SIMPLE {
                    self.close_simple(&md)?;
                } else {
                    self.off_save(&md)?;
                }
            }
            cmd::RIGHT_BRACE => self.handle_right_brace(&md)?,
            cmd::HMOVE | cmd::VMOVE => {
                if (c == cmd::HMOVE) == v {
                    let t = self.cur.chr;
                    let d = self.scan_normal_dimen()?;
                    self.scan_box(&md, if t == 0 { i64::from(d) } else { -i64::from(d) })?;
                }
            }
            cmd::LEADER_SHIP => {
                let ctx = LEADER_FLAG - 100 + i64::from(self.cur.chr);
                self.scan_box(&md, ctx)?;
            }
            cmd::MAKE_BOX => self.begin_box(&md, 0)?,
            cmd::START_PAR => {
                if v {
                    let ind = self.cur.chr > 0;
                    self.new_graf(&md, ind)?;
                } else if self.cur.chr > 0 {
                    // §1093
                    let w = self.ipar(Par::parindent)?;
                    let b = crate::build::null_box(self.v, w);
                    self.append(md.lid, b);
                    if h {
                        self.set_space_factor(&md, 1000);
                    }
                }
            }
            cmd::PAR_END => {
                // §1094
                if v {
                    self.normal_paragraph()?;
                    if md.m > 0 {
                        self.finish_candidate()?;
                    }
                } else if h {
                    self.end_graf(&md)?;
                    self.finish_candidate()?;
                }
            }
            cmd::UN_HBOX => {
                if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else {
                    self.unpackage(&md)?;
                }
            }
            cmd::UN_VBOX => {
                if v {
                    self.unpackage(&md)?;
                } else if h {
                    self.head_for_vmode(&md);
                }
            }
            cmd::HALIGN | cmd::VALIGN => {
                if c == cmd::VALIGN && v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if c == cmd::HALIGN && h {
                    self.head_for_vmode(&md);
                } else if c == cmd::HALIGN && v {
                    self.init_align(&md)?;
                } else if c == cmd::HALIGN && md.m == 203 {
                    // §1130: `mmode+halign`: privileged (a display), and
                    // only at the display's own level
                    if md.group == crate::val::group::MATH_SHIFT {
                        self.init_align(&md)?;
                    } else {
                        self.off_save(&md)?;
                    }
                } else if c == cmd::HALIGN {
                    // §1130 `privileged` (§1051)
                    self.report_illegal_case(&md)?;
                } else {
                    // REDUCE-GAP: \valign
                    self.log("error", "this alignment is not supported")?;
                }
            }
            cmd::INSERT => self.begin_insert_or_adjust(&md, false)?,
            cmd::VADJUST => {
                if !v {
                    self.begin_insert_or_adjust(&md, true)?;
                }
            }
            cmd::MARK => {
                // §1101
                let (_, body) = self.scan_toks(false, true);
                self.ck()?;
                let node = self.v.list(vec![
                    self.v.int(crate::nodes::T_MARK),
                    toks_value(self.v, &body),
                ]);
                self.append(md.lid, node);
            }
            cmd::BREAK_PENALTY => {
                // §1103
                let n = self.scan_int()?;
                let node = crate::nodes::penalty_node(self.v, n);
                self.append(md.lid, node);
                if md.m == 1 {
                    self.finish_candidate()?;
                }
            }
            cmd::REMOVE_ITEM => self.delete_last(&md)?,
            cmd::ITAL_CORR => {
                if h {
                    self.append_italic_correction(&md)?;
                } else if am == 203 {
                    let node =
                        crate::nodes::kern_node(self.v, 0, phitex_typeset::node::KernKind::Normal);
                    self.append(md.lid, node);
                }
            }
            cmd::DISCRETIONARY => {
                if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else {
                    self.append_discretionary(&md)?;
                }
            }
            cmd::ACCENT => {
                if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if h {
                    self.make_accent(&md)?;
                }
            }
            cmd::MATH_SHIFT => {
                if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if h {
                    self.init_math(&md)?;
                }
            }
            cmd::TOKS_REGISTER..=cmd::SET_INTERACTION => self.prefixed_command()?,
            cmd::AFTER_ASSIGNMENT => {
                // §1268
                // (the token pending before this one, read first: a run
                // that ends with the state it found publishes nothing,
                // `state::unchanged_transient`, so it must know what
                // it found; a pending token only a run before it set
                // reaches it)
                self.get_loc(crate::fields::cell::AFTERTOKEN)?;
                self.get_token();
                self.ck()?;
                let t = self.cur.tok.encode(self.v);
                self.set_cell(crate::fields::cell::AFTERTOKEN, t, true);
            }
            cmd::AFTER_GROUP => {
                // §1271
                self.get_token();
                self.ck()?;
                let t = self.cur.tok;
                // the innermost group's level keeps the tokens: one of
                // this run's, else the engine scope the node runs in (its
                // `ag` list, run at its close, `Run::unsave_extras`); at
                // the bottom level they are dropped (§1271
                // `cur_level>level_one`)
                if let Some(l) = self.nest.iter_mut().rev().find(|l| l.group != 0) {
                    self.muts += 1;
                    l.ag.push(t);
                } else {
                    let base = self.base_mode()?;
                    if base.depth > 0 {
                        let v = self.v;
                        let mut xs = vec![v.int(base.sid)];
                        xs.extend(self.scope_list("ag", &base)?);
                        xs.push(t.encode(v));
                        self.set("ag", v.list(xs), false);
                    }
                }
            }
            cmd::IN_STREAM => self.open_or_close_in()?,
            cmd::MESSAGE => {
                // §1279
                let ch = self.cur.chr;
                let was = self.lines_sink_begin(Sink::Log);
                let (_, body) = self.scan_toks(false, true);
                self.lines_sink_end(was);
                self.ck()?;
                let mut s = String::new();
                self.show_toks_into(&mut s, &body)?;
                self.log(if ch == 0 { "msg" } else { "errmsg" }, &s)?;
            }
            cmd::CASE_SHIFT => {
                // §1288
                let b = self.cur.chr;
                let (_, body) = self.scan_toks(false, false);
                self.ck()?;
                let table = usize::from(b != crate::val::LC_CODE_BASE);
                let mut out = Vec::with_capacity(body.len());
                for t in body {
                    match t.as_cs() {
                        None => {
                            let ch = t.chr();
                            let e = self.code(table, ch)?;
                            if e != 0 {
                                out.push(Tok::ch(
                                    t.cat().unwrap_or(0),
                                    u32::try_from(e).unwrap_or(0),
                                ));
                            } else {
                                out.push(t);
                            }
                        }
                        Some(_) => out.push(t),
                    }
                }
                self.push_toks(out);
            }
            cmd::XRAY => self.show_whatever()?,
            cmd::EXTENSION => self.do_extension(&md)?,
            _ => {
                // forbidden cases, alignment errors, math in non-math
                // modes (§1046–§1048): errors
                self.report_illegal_case(&md)?;
            }
        }
        Ok(())
    }

    /// `prefixed_command` (§1211–§1269): the common assignments here,
    /// the rest in [`Run::prefixed_rest`].
    pub(crate) fn prefixed_command(&mut self) -> R<()> {
        let c = self.cur.cmd;
        if matches!(
            c,
            cmd::DEF | cmd::LET | cmd::REGISTER | cmd::ADVANCE | cmd::ASSIGN_INT
        ) {
            let gd = self.ipar(Par::globaldefs)?;
            let global = gd > 0;
            match c {
                cmd::DEF => self.def_cmd(if global { 4 } else { 0 }, false, gd, global)?,
                cmd::LET => self.let_cmd(global)?,
                cmd::ASSIGN_INT => self.assign_int(global)?,
                _ => self.do_register_command(global)?,
            }
            return self.after_assignment();
        }
        self.prefixed_rest()
    }

    /// [`Run::prefixed_command`] for prefixes and the other assignments.
    #[allow(clippy::too_many_lines)]
    #[inline(never)]
    fn prefixed_rest(&mut self) -> R<()> {
        let mut a: i32 = 0;
        while self.cur.cmd == cmd::PREFIX {
            if (a / self.cur.chr) % 2 == 0 {
                a += self.cur.chr;
            }
            self.skip_blanks_relax_x()?;
            if self.cur.cmd <= cmd::MAX_NON_PREFIXED {
                // §1212: "You can't use a prefix with"
                self.back_cur();
                return Ok(());
            }
        }
        let protected = crate::etex::take_protected(&mut a);
        // §1214
        let gd = self.ipar(Par::globaldefs)?;
        if gd != 0 {
            if gd < 0 {
                if a >= 4 {
                    a -= 4;
                }
            } else if a < 4 {
                a += 4;
            }
        }
        let global = a >= 4;
        let v = self.v;
        if crate::etex::prefixed(self, global)? {
            return self.after_assignment();
        }
        match self.cur.cmd {
            cmd::SET_FONT => {
                // §1217
                let f = v.int(i64::from(self.cur.chr));
                self.set_cell(crate::fields::cell::FONT, f, global);
            }
            cmd::DEF => self.def_cmd(a, protected, gd, global)?,
            cmd::LET => self.let_cmd(global)?,
            cmd::SHORTHAND_DEF => {
                // §1224
                let n = self.cur.chr;
                let p = self.get_r_token()?;
                self.define_cs(&p, &Mean::Prim("relax", cmd::RELAX, 256), global);
                self.scan_optional_equals()?;
                let m = match n {
                    0 => Mean::CharDef(i64::from(self.scan_char_num()?)),
                    1 => Mean::MathCharDef(i64::from(self.scan_fifteen_bit_int()?)),
                    crate::xetex::UMATHCHAR_DEF | crate::xetex::UMATHCHARNUM_DEF => {
                        let x = self.scan_umath(n == crate::xetex::UMATHCHAR_DEF)?;
                        Mean::MathCharDef(crate::xetex::U_MATH | i64::from(x.cast_unsigned()))
                    }
                    _ => {
                        let r = self.scan_eight_bit_int()?;
                        match n {
                            2 => {
                                if let Some(c) = p.as_cs() {
                                    let f =
                                        self.quantity(cmd::ASSIGN_INT, crate::val::COUNT_BASE + r);
                                    self.ops
                                        .counters
                                        .write()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .insert(c.name(), f);
                                }
                                Mean::Reg(cmd::ASSIGN_INT, crate::val::COUNT_BASE + r)
                            }
                            3 => Mean::Reg(cmd::ASSIGN_DIMEN, crate::val::SCALED_BASE + r),
                            4 => Mean::Reg(cmd::ASSIGN_GLUE, crate::val::SKIP_BASE + r),
                            5 => Mean::Reg(cmd::ASSIGN_MU_GLUE, crate::val::MU_SKIP_BASE + r),
                            _ => Mean::Reg(cmd::ASSIGN_TOKS, TOKS_BASE + r),
                        }
                    }
                };
                self.define_cs(&p, &m, global);
            }
            cmd::READ_TO_CS => {
                // §1225
                let n = self.scan_int()?;
                let _ = self.scan_keyword("to")?;
                let p = self.get_r_token()?;
                let body = self.read_toks(n)?;
                let m = Mean::Macro(Rc::new(Macro::new(0, Vec::new(), body.into())));
                self.define_cs(&p, &m, global);
            }
            cmd::TOKS_REGISTER | cmd::ASSIGN_TOKS => {
                // §1226
                let loc = if self.cur.cmd == cmd::TOKS_REGISTER {
                    TOKS_BASE + self.scan_eight_bit_int()?
                } else {
                    self.cur.chr
                };
                // (XeTeX's `\XeTeXinterchartoks` names its pair of classes)
                let field = if self.xe
                    && self.cur.cmd == cmd::ASSIGN_TOKS
                    && loc == crate::xetex::INTER_CHAR_LOC
                {
                    self.ic_field()?
                } else {
                    self.quantity(cmd::ASSIGN_TOKS, loc)
                };
                self.scan_optional_equals()?;
                self.skip_blanks_relax_x()?;
                if self.cur.cmd != cmd::LEFT_BRACE {
                    // §1227
                    let mut src = None;
                    if self.cur.cmd == cmd::TOKS_REGISTER {
                        let r = self.scan_eight_bit_int()?;
                        src = Some(TOKS_BASE + r);
                    } else if self.cur.cmd == cmd::ASSIGN_TOKS {
                        src = Some(self.cur.chr);
                    }
                    if let Some(s) = src {
                        let x = if self.xe
                            && self.cur.cmd == cmd::ASSIGN_TOKS
                            && s == crate::xetex::INTER_CHAR_LOC
                        {
                            let f = self.ic_field()?;
                            self.read_f(f)?
                        } else {
                            self.qget(cmd::ASSIGN_TOKS, s)?
                        };
                        let val = x.unwrap_or_else(|| v.list(Vec::new()));
                        self.set_f(field, val, global);
                        return self.after_assignment();
                    }
                }
                self.back_cur();
                let (_, mut body) = self.scan_toks(false, false);
                self.ck()?;
                if !body.is_empty() && loc == OUTPUT_ROUTINE_LOC {
                    body.insert(0, Tok::ch(1, u32::from(b'{')));
                    body.push(Tok::ch(2, u32::from(b'}')));
                }
                let val = toks_value(v, &body);
                self.set_f(field, val, global);
            }
            cmd::ASSIGN_INT => self.assign_int(global)?,
            cmd::ASSIGN_DIMEN => {
                let l = self.quantity_loc(cmd::ASSIGN_DIMEN, self.cur.chr);
                self.scan_optional_equals()?;
                let n = self.scan_normal_dimen()?;
                self.set_loc(l, Entry::int(n), global);
            }
            cmd::ASSIGN_GLUE | cmd::ASSIGN_MU_GLUE => {
                let n = self.cur.cmd;
                let l = self.quantity_loc(n, self.cur.chr);
                self.scan_optional_equals()?;
                let g = self.scan_glue(if n == cmd::ASSIGN_MU_GLUE { 3 } else { 2 })?;
                let e = self.st.glue_entry(g);
                self.set_loc(l, e, global);
            }
            cmd::DEF_CODE if self.xe => self.xe_def_code(global)?,
            cmd::DEF_CODE => {
                // §1232
                let base = self.cur.chr;
                let n: i32 = match base {
                    CAT_CODE_BASE => 15,
                    MATH_CODE_BASE => 0o100000,
                    SF_CODE_BASE => 0o77777,
                    DEL_CODE_BASE => 0o77777777,
                    _ => 255,
                };
                let ch = self.scan_char_num()?;
                self.scan_optional_equals()?;
                let mut val = self.scan_int()?;
                if (val < 0 && base < DEL_CODE_BASE) || val > n {
                    // "Invalid code"
                    val = 0;
                }
                let name = match base {
                    CAT_CODE_BASE => "cat",
                    crate::val::LC_CODE_BASE => "lc",
                    crate::val::UC_CODE_BASE => "uc",
                    SF_CODE_BASE => "sf",
                    MATH_CODE_BASE => "mc",
                    _ => "dc",
                };
                let l = self.code_loc(name, ch);
                self.set_loc(l, Entry::int(val), global);
            }
            cmd::DEF_FAMILY => {
                // §1234
                let size = self.cur.chr - crate::val::MATH_FONT_BASE;
                let n = self.scan_fam()?;
                self.scan_optional_equals()?;
                let f = self.scan_font_ident()?;
                let l = self.fam_loc(size, n);
                self.set_loc(l, Entry::raw(Some(v.int(f))), global);
                // (XeTeX: the highest family past sixteen ever set, which
                // math looks at up to)
                if self.xe && n >= 16 {
                    let top = self
                        .get("famxmax")?
                        .map_or(15, |x| crate::val::int_of(v, x));
                    if i64::from(n) > top {
                        self.set("famxmax", v.int(i64::from(n)), true);
                    }
                }
            }
            cmd::REGISTER | cmd::ADVANCE | cmd::MULTIPLY | cmd::DIVIDE => {
                self.do_register_command(global)?;
            }
            cmd::SET_BOX => {
                // §1241
                let n = self.scan_eight_bit_int()?;
                self.scan_optional_equals()?;
                let md = self.mode()?;
                let ctx = BOX_FLAG + i64::from(n) + if global { 256 } else { 0 };
                self.scan_box(&md, ctx)?;
            }
            cmd::SET_AUX => {
                // §1243
                let md = self.mode()?;
                let c = self.cur.chr;
                if c == md.m.abs() {
                    self.scan_optional_equals()?;
                    if c == 1 {
                        let d = self.scan_normal_dimen()?;
                        self.set_prev_depth(&md, d);
                    } else {
                        let n = self.scan_int()?;
                        if n > 0 && n <= 32767 {
                            self.set_space_factor(&md, n);
                        }
                    }
                }
            }
            cmd::SET_PREV_GRAF => {
                // §1244
                let md = self.mode()?;
                self.scan_optional_equals()?;
                let n = self.scan_int()?;
                if n >= 0 {
                    self.set(&format!("pg:{}", md.vlid), v.int(i64::from(n)), true);
                }
            }
            cmd::SET_PAGE_DIMEN => {
                // §1245: `page_so_far[c] := x`, on the main list for the
                // page fold (applied at the next candidate, where the
                // builder's state is the one this command changes)
                let c = self.cur.chr;
                self.scan_optional_equals()?;
                let d = self.scan_normal_dimen()?;
                let m = v.list(vec![
                    v.int(crate::nodes::T_SET_PAGE),
                    v.int(i64::from(c)),
                    v.int(i64::from(d)),
                ]);
                self.append(0, m);
            }
            cmd::SET_PAGE_INT => {
                // §1246
                let c = self.cur.chr;
                self.scan_optional_equals()?;
                let n = self.scan_int()?;
                self.set(
                    if c == 0 { "deadcycles" } else { "inspen" },
                    v.int(i64::from(n)),
                    true,
                );
                // (the text's `\deadcycles` on the main list too, where
                // the page builder and the finishers read it in order with
                // the pages' fires, §1054, §1012; a routine's is its own,
                // given back with its material)
                if c == 0 && self.cx.op() != self.ops.finish {
                    let m = v.list(vec![
                        v.int(crate::nodes::T_SET_PAGE),
                        v.int(crate::pages::DEAD_SLOT),
                        v.int(i64::from(n)),
                    ]);
                    self.append(0, m);
                }
            }
            cmd::SET_BOX_DIMEN => {
                // §1247
                let c = self.cur.chr;
                let b = self.scan_eight_bit_int()?;
                self.scan_optional_equals()?;
                let d = self.scan_normal_dimen()?;
                if let Some(bx) = self.box_reg(b)? {
                    let nb = crate::nodes::box_with_dim(v, bx, c, d);
                    self.modify_box_reg(b, Some(nb))?;
                }
            }
            cmd::SET_SHAPE => {
                // §1248
                self.scan_optional_equals()?;
                let n = self.scan_int()?;
                let mut xs = Vec::new();
                for _ in 0..n.max(0) {
                    let i = self.scan_normal_dimen()?;
                    let w = self.scan_normal_dimen()?;
                    xs.push(v.int(i64::from(i)));
                    xs.push(v.int(i64::from(w)));
                }
                self.set_cell(crate::fields::cell::PARSHAPE, v.list(xs), global);
            }
            cmd::HYPH_DATA => {
                // §1252
                if self.cur.chr == 1 {
                    self.new_patterns()?;
                } else {
                    self.new_hyph_exceptions()?;
                }
            }
            cmd::ASSIGN_FONT_DIMEN => {
                // §1253
                let fd = self.find_font_dimen(true)?;
                self.scan_optional_equals()?;
                let d = self.scan_normal_dimen()?;
                if let Some((f, k)) = fd {
                    self.font_set(f, crate::fields::FontCell::Fd(k), v.int(i64::from(d)), true);
                }
            }
            cmd::ASSIGN_FONT_INT => {
                let n = self.cur.chr;
                let f = self.scan_font_ident()?;
                self.scan_optional_equals()?;
                let x = self.scan_int()?;
                let c = if n == 0 {
                    crate::fields::FontCell::Hc
                } else {
                    crate::fields::FontCell::Sc
                };
                self.font_set(f, c, v.int(i64::from(x)), true);
            }
            cmd::DEF_FONT => self.new_font(global)?,
            cmd::SET_INTERACTION => {
                // §1265
                self.log("println", "")?;
            }
            _ => {}
        }
        self.after_assignment()
    }

    /// §1269: the token saved by `\afterassignment`, if any.
    pub(crate) fn after_assignment(&mut self) -> R<()> {
        if let Some(t) = self.get_loc(crate::fields::cell::AFTERTOKEN)?
            && t != ValueId::UNIT
        {
            let tok = Tok::decode(self.v, t);
            self.back_input(tok);
            self.set_cell(crate::fields::cell::AFTERTOKEN, ValueId::UNIT, true);
        }
        Ok(())
    }

    /// `\def` and its kin (§1218).
    fn def_cmd(&mut self, a: i32, protected: bool, gd: i32, global: bool) -> R<()> {
        // §1218
        let ch = self.cur.chr;
        let mut global = global;
        if ch % 2 == 1 && !global && gd >= 0 {
            global = true;
        }
        let e = ch >= 2;
        let p = self.get_r_token()?;
        let was = e.then(|| self.lines_sink_begin(Sink::Def));
        let (params, body) = self.scan_toks(true, e);
        let hit = was.is_some_and(|w| self.lines_sink_end(w));
        // (the body's list kept for the next scan: the macro's
        // own made from it at its size)
        let rc: Rc<[Tok]> = Rc::from(&body[..]);
        self.toks_buf = body;
        self.ck()?;
        let m = Mean::Macro(Rc::new(Macro::new(
            u8::try_from(a % 4).unwrap_or(0) | if protected { crate::etex::PROTECTED } else { 0 },
            params,
            rc,
        )));
        self.define_cs_owned(p, m, global);
        // (a body that read the lines: its meaning tainted, a
        // global one seen after the run)
        if hit {
            match self.mean_loc(p).filter(|_| !global) {
                Some(l) => self.st.taint(l),
                None => self.lt.strong = true,
            }
        }
        Ok(())
    }

    /// An integer parameter's or `\countdef` register's assignment (§1228).
    fn assign_int(&mut self, global: bool) -> R<()> {
        let l = self.quantity_loc(cmd::ASSIGN_INT, self.cur.chr);
        self.scan_optional_equals()?;
        // (`\c@page=` its own symbol: a restore, docs/spec-deferred-reads.md §2)
        if crate::defer::try_symbolic(self, l, global)? {
            return Ok(());
        }
        let n = self.scan_int()?;
        // (set back to the value reaching the run, which it read: a
        // restore, `crate::defer::restores`)
        if crate::defer::restores(self, l, n, global) {
            return Ok(());
        }
        self.note_reg_def(l);
        self.set_loc(l, Entry::int(n), global);
        Ok(())
    }

    /// `\let` and `\futurelet` (§1221).
    fn let_cmd(&mut self, global: bool) -> R<()> {
        // §1221
        let n = self.cur.chr;
        let p = self.get_r_token()?;
        if n == 0 {
            loop {
                self.get_token();
                self.ck()?;
                if self.cur.cmd != cmd::SPACER {
                    break;
                }
            }
            if self.is_other(b'=') {
                self.get_token();
                self.ck()?;
                if self.cur.cmd == cmd::SPACER {
                    self.get_token();
                    self.ck()?;
                }
            }
        } else {
            self.get_token();
            self.ck()?;
            let q = self.cur.tok;
            self.get_token();
            self.ck()?;
            let t = self.cur.tok;
            self.back_input(t);
            self.back_input(q);
        }
        // (a control sequence's decoded meaning: its entry copied)
        if self.interp.is_none()
            && !(self.cur.cmd == cmd::RELAX && self.cur.chr == crate::val::NO_EXPAND_FLAG)
            && let Some(i) = self.cur.tok.cs_index()
            && !self.cur.tok.is_frozen_cs()
            && let Some(e) = self.st.meaning_entry(
                crate::state::Loc::try_from(crate::fields::MEAN_BASE + i).unwrap_or(u32::MAX),
            )
            && let Some(l) = self.mean_loc(p)
        {
            self.set_loc(l, e, global);
            return Ok(());
        }
        let m = self.cur_mean()?;
        self.define_cs_owned(p, m, global);
        Ok(())
    }

    /// `get_r_token` (§1215).
    fn get_r_token(&mut self) -> R<Tok> {
        loop {
            self.get_name_token()?;
            if self.cur.tok != crate::val::space_tok() {
                break;
            }
        }
        if !self.cur.is_cs() {
            // "Missing control sequence inserted": \inaccessible
            self.back_cur();
            return Ok(Tok::cs("inaccessible"));
        }
        Ok(self.cur.tok)
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn define_cs(&mut self, t: &Tok, m: &Mean, global: bool) {
        self.set_mean(*t, m.clone(), global);
    }

    /// [`Run::define_cs`] of a meaning made for it.
    pub(crate) fn define_cs_owned(&mut self, t: Tok, m: Mean, global: bool) {
        self.set_mean(t, m, global);
    }

    /// `do_register_command` (§1236–§1240).
    #[allow(clippy::too_many_lines)]
    fn do_register_command(&mut self, global: bool) -> R<()> {
        let q = self.cur.cmd;
        let v = self.v;
        // §1237
        // (the register by its location: no field looked up)
        let (l, p): (Loc, u8) = if q == cmd::REGISTER {
            let p = u8::try_from(self.cur.chr).unwrap_or(0);
            let n = self.scan_eight_bit_int()?;
            (self.reg_loc(usize::from(p.min(3)), n), p)
        } else {
            self.get_x_token();
            self.ck()?;
            if self.cur.cmd >= cmd::ASSIGN_INT && self.cur.cmd <= cmd::ASSIGN_MU_GLUE {
                (
                    self.quantity_loc(self.cur.cmd, self.cur.chr),
                    self.cur.cmd - cmd::ASSIGN_INT,
                )
            } else if self.cur.cmd == cmd::REGISTER {
                let p = u8::try_from(self.cur.chr).unwrap_or(0);
                let n = self.scan_eight_bit_int()?;
                (self.reg_loc(usize::from(p.min(3)), n), p)
            } else {
                // "You can't use x after \advance"
                return Ok(());
            }
        };
        if q == cmd::REGISTER {
            self.scan_optional_equals()?;
            // (the register set to its own symbol: a restore, neither a
            // read nor a definition, docs/spec-deferred-reads.md §2)
            if p == 0 && crate::defer::try_symbolic(self, l, global)? {
                return Ok(());
            }
        } else {
            let _ = self.scan_keyword("by")?;
        }
        // (an integer register advanced by a constant it never read: a
        // step of the run's map, nothing read, published as a relation,
        // `State::add`)
        let mut pre = None;
        if p == 0 && q == cmd::ADVANCE && self.map_ok(l, global) {
            let x = self.scan_int()?;
            if self.st.add(self.v, l, x, global) {
                self.muts += 1;
                return Ok(());
            }
            pre = Some(x);
        }
        // (§1238: the old value is read for `\advance`, `\multiply` and
        // `\divide` only; an assignment replaces it unseen)
        let old = if q == cmd::REGISTER {
            None
        } else {
            self.get_loc(l)?
        };
        if p < 2 {
            let cur = i64::from(crate::val::i32_of(v, old));
            let r: Option<i32> = if q < cmd::MULTIPLY {
                let x = if let Some(x) = pre {
                    x
                } else if p == 0 {
                    self.scan_int()?
                } else {
                    self.scan_normal_dimen()?
                };
                if q == cmd::ADVANCE {
                    let s = cur + i64::from(x);
                    // web2c: integer overflow wraps silently
                    // the low 32 bits
                    #[allow(clippy::cast_possible_truncation)]
                    let w = s as i32;
                    Some(w)
                } else {
                    Some(x)
                }
            } else {
                let x = self.scan_int()?;
                let cur = i32::try_from(cur).unwrap_or(0);
                if q == cmd::MULTIPLY {
                    if p == 0 {
                        phitex_typeset::arith::mult_integers(cur, x)
                    } else {
                        phitex_typeset::arith::nx_plus_y(cur, x, 0)
                    }
                } else {
                    let d = phitex_typeset::arith::x_over_n(cur, x);
                    (!d.overflow).then_some(d.value)
                }
            };
            let Some(r) = r else {
                // "Arithmetic overflow"
                return Ok(());
            };
            if p == 0 && q == cmd::REGISTER && crate::defer::restores(self, l, r, global) {
                return Ok(());
            }
            if p == 0 {
                self.note_reg_def(l);
            }
            self.set_loc(l, Entry::int(r), global);
        } else {
            let s = glue_of(v, old);
            let r: Option<Glue> = if q < cmd::MULTIPLY {
                let g = self.scan_glue(p)?;
                if q == cmd::ADVANCE {
                    Some(add_glue(&g, &s))
                } else {
                    Some(g)
                }
            } else {
                let x = self.scan_int()?;
                if q == cmd::MULTIPLY {
                    let m = |a: i32| phitex_typeset::arith::nx_plus_y(a, x, 0);
                    match (m(s.width), m(s.stretch), m(s.shrink)) {
                        (Some(w), Some(st), Some(sh)) => Some(Glue {
                            width: w,
                            stretch: st,
                            shrink: sh,
                            ..s
                        }),
                        _ => None,
                    }
                } else {
                    let d = |a: i32| phitex_typeset::arith::x_over_n(a, x);
                    let (w, st, sh) = (d(s.width), d(s.stretch), d(s.shrink));
                    (!(w.overflow || st.overflow || sh.overflow)).then_some(Glue {
                        width: w.value,
                        stretch: st.value,
                        shrink: sh.value,
                        ..s
                    })
                }
            };
            let Some(r) = r else { return Ok(()) };
            let e = self.st.glue_entry(r);
            self.set_loc(l, e, global);
        }
        Ok(())
    }

    /// `new_font` (§1257–§1260).
    fn new_font(&mut self, global: bool) -> R<()> {
        let u = self.get_r_token()?;
        let t: String = match u.as_cs() {
            Some(n) if n.is_empty() => "FONT".to_owned(),
            Some(n) => n.to_string(),
            None => format!("FONT{}", char::from_u32(u.chr()).unwrap_or('?')),
        };
        self.define_cs(&u, &Mean::Font(0), global);
        self.scan_optional_equals()?;
        let (name, quoted) = self.scan_file_name_q()?;
        // §1258
        let s: i32 = if self.scan_keyword("at")? {
            let s = self.scan_normal_dimen()?;
            if s <= 0 || s >= 0o1000000000 {
                10 * 65536
            } else {
                s
            }
        } else if self.scan_keyword("scaled")? {
            let n = self.scan_int()?;
            if n <= 0 || n > 32768 { -1000 } else { -n }
        } else {
            -1000
        };
        // §1260: a font of the same name and size is that font, its key
        // the font's own ([`crate::fonts::key_of`]): one read, of that
        // font's fields, never of the fonts loaded before
        let f = if self.xe {
            self.xe_read_font_info(&name, quoted, s)?
        } else {
            self.read_font_info(Some(u), &name, s)?
        };
        let v = self.v;
        self.define_cs(&u, &Mean::Font(f), global);
        // §1257: the font identifier
        let id = Tok::cs(&format!("\u{10FFFE}F{f}\u{2}{t}"));
        self.define_cs(&id, &Mean::Font(f), true);
        self.font_set(f, crate::fields::FontCell::Id, v.str(&t), true);
        Ok(())
    }

    /// `read_font_info` (§560) behind §1260's search: the font `name` at
    /// `s` by its key ([`crate::fonts::pick_key`]), loaded here unless a
    /// font of that name and size was loaded before (§1260: that font);
    /// the null font if the TFM can't be had. A load contributes the
    /// font to the line (`fontload`, in TeX's order of loads, which is
    /// where its number comes from, [`crate::pdfout::font_load`]) and is
    /// the last font loaded (`fontlast`, §578).
    pub(crate) fn read_font_info(&mut self, u: Option<Tok>, name: &str, s: i32) -> R<i64> {
        if let Some(f) = self.tfm_font_info(name, s)? {
            return Ok(f);
        }
        // §561 (`start_font_error_message`, §1260's font identifier `u`,
        // `null_cs` where there is none) and web2c's reasons
        let esc = i64::from(self.ipar(crate::fields::Par::escapechar)?);
        let mut m = String::from("Font ");
        if let Some(t) = u {
            crate::print::sprint_cs(&mut m, t, esc);
        } else {
            crate::print::print_esc(&mut m, "csname", esc);
            crate::print::print_esc(&mut m, "endcsname", esc);
        }
        m.push('=');
        if name.contains(' ') {
            m.push('"');
            m.push_str(name);
            m.push('"');
        } else {
            m.push_str(name);
        }
        if s >= 0 {
            m.push_str(" at ");
            crate::print::print_scaled(&mut m, s);
            m.push_str("pt");
        } else if s != -1000 {
            m.push_str(" scaled ");
            m.push_str(&(-s).to_string());
        }
        m.push_str(if self.file(&format!("{name}.tfm"))?.is_some() {
            " not loadable: Bad metric (TFM) file"
        } else if name.len() > 255 {
            " not loadable: Metric (TFM) file name too long"
        } else {
            " not loadable: Metric (TFM) file not found"
        });
        self.log("error", &m)?;
        Ok(0)
    }

    /// [`Run::read_font_info`] of a TFM file: `None` where it can't be
    /// had.
    pub(crate) fn tfm_font_info(&mut self, name: &str, s: i32) -> R<Option<i64>> {
        let key = format!("{name}.tfm");
        let Some(file) = self.file(&key)? else {
            return Ok(None);
        };
        let v = self.v;
        let Some(l) = crate::fonts::load(v, file, s) else {
            return Ok(None);
        };
        // (§1260: `s > 0` is the size; a scaled load's size is the
        // design size scaled, `l.at.size` both ways)
        let size = l.at.size;
        let mut held = false;
        // (pdfTeX: a font given expansion parameters, `\pdffontexpand`'s,
        // is no longer one §1260 finds: a later `\font` of its name and
        // size loads it anew, a font of its own)
        let k = crate::fonts::pick_key(name, size, &mut |k| {
            let info = self.font_get(k, crate::fields::FontCell::Info)?;
            let other = match crate::fonts::info(v, info) {
                Some(i) if i.name == name && i.size == size => {
                    Some(crate::pdfprim::expand_params(self, i64::from(k))?.is_some())
                }
                Some(_) => Some(true),
                None => None,
            };
            held = other == Some(false);
            Ok::<_, crate::run::Halt>(other)
        })?;
        let f = i64::from(k);
        if held {
            return Ok(Some(f));
        }
        self.set("fontlast", v.int(f), true);
        self.font_loaded(f);
        crate::pdfout::font_load(self, name, l.at.size, f);
        let info = v.list(vec![
            v.str(name),
            v.int(i64::from(l.at.size)),
            v.int(i64::from(l.at.design_size)),
            file,
            // (the size the TFM is read at again: the font's own, however
            // the load asked for it, `at` or `scaled`, so that the same
            // font loaded either way has the same fields)
            v.int(i64::from(l.at.size)),
        ]);
        self.font_set(f, crate::fields::FontCell::Info, info, true);
        let params = l.at.params.clone();
        for (k, p) in params.iter().enumerate() {
            let k = i32::try_from(k + 1).unwrap_or(0);
            self.font_set(
                f,
                crate::fields::FontCell::Fd(k),
                v.int(i64::from(*p)),
                true,
            );
        }
        self.font_set(
            f,
            crate::fields::FontCell::Fdn,
            v.int(i64::try_from(params.len()).unwrap_or(0)),
            true,
        );
        let hc = i64::from(self.ipar(Par::defaulthyphenchar)?);
        let sc = i64::from(self.ipar(Par::defaultskewchar)?);
        self.font_set(f, crate::fields::FontCell::Hc, v.int(hc), true);
        self.font_set(f, crate::fields::FontCell::Sc, v.int(sc), true);
        Ok(Some(f))
    }

    /// `\patterns` (§960–§965): the words as §962 reads them (letters
    /// through `\lccode`, values as digits), entered when a paragraph
    /// needs them.
    fn new_patterns(&mut self) -> R<()> {
        self.scan_left_brace()?;
        let lang = phitex_typeset::hyph::cur_lang(self.ipar(Par::language)?);
        let mut words: Vec<ValueId> = Vec::new();
        let mut word = String::new();
        let mut digit_sensed = false;
        let v = self.v;
        loop {
            self.get_x_token();
            self.ck()?;
            match self.cur.cmd {
                cmd::LETTER | cmd::OTHER_CHAR => {
                    let c = u32::try_from(self.cur.chr).unwrap_or(0);
                    if digit_sensed || !(48..=57).contains(&c) {
                        let code = if c == u32::from(b'.') {
                            c
                        } else {
                            u32::try_from(i64::from(self.code(0, c)?)).unwrap_or(0)
                        };
                        word.push(char::from_u32(code).unwrap_or('\0'));
                        digit_sensed = false;
                    } else {
                        word.push(char::from_u32(c).unwrap_or('0'));
                        digit_sensed = true;
                    }
                }
                cmd::SPACER | cmd::RIGHT_BRACE => {
                    if !word.is_empty() {
                        words.push(v.str(&std::mem::take(&mut word)));
                    }
                    digit_sensed = false;
                    if self.cur.cmd == cmd::RIGHT_BRACE {
                        break;
                    }
                }
                _ => {
                    // "Bad \patterns"
                }
            }
        }
        let old = self.get_loc(crate::fields::cell::PATTERNS)?;
        let mut all = old.map(|x| list_of(v, x)).unwrap_or_default();
        all.push(v.list(vec![v.int(i64::from(lang)), v.str("p"), v.list(words)]));
        self.set_cell(crate::fields::cell::PATTERNS, v.list(all), true);
        Ok(())
    }

    /// `\hyphenation` (§934–§939).
    fn new_hyph_exceptions(&mut self) -> R<()> {
        self.scan_left_brace()?;
        let lang = phitex_typeset::hyph::cur_lang(self.ipar(Par::language)?);
        let v = self.v;
        let mut words: Vec<ValueId> = Vec::new();
        let mut word = String::new();
        loop {
            self.get_x_token();
            self.ck()?;
            match self.cur.cmd {
                cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN => {
                    if self.cur.chr == i32::from(b'-') {
                        word.push('-');
                    } else {
                        let c = u32::try_from(self.cur.chr).unwrap_or(0);
                        let lc = i64::from(self.code(0, c)?);
                        if lc != 0 {
                            word.push(
                                char::from_u32(u32::try_from(lc).unwrap_or(0)).unwrap_or('?'),
                            );
                        }
                    }
                }
                cmd::CHAR_NUM => {
                    let c = self.scan_char_num()?;
                    let lc = i64::from(self.code(0, c)?);
                    if lc != 0 {
                        word.push(char::from_u32(u32::try_from(lc).unwrap_or(0)).unwrap_or('?'));
                    }
                }
                cmd::SPACER | cmd::RIGHT_BRACE => {
                    if !word.is_empty() {
                        words.push(v.str(&std::mem::take(&mut word)));
                    }
                    if self.cur.cmd == cmd::RIGHT_BRACE {
                        break;
                    }
                }
                _ => {}
            }
        }
        let old = self.get_loc(crate::fields::cell::PATTERNS)?;
        let mut all = old.map(|x| list_of(v, x)).unwrap_or_default();
        all.push(v.list(vec![v.int(i64::from(lang)), v.str("h"), v.list(words)]));
        self.set_cell(crate::fields::cell::PATTERNS, v.list(all), true);
        Ok(())
    }

    /// `open_or_close_in` (§1275).
    fn open_or_close_in(&mut self) -> R<()> {
        let c = self.cur.chr;
        let n = self.scan_four_bit_int()?;
        let v = self.v;
        let field = format!("read:{n}");
        if self.get(&field)?.is_some_and(|x| x != ValueId::UNIT) {
            self.set(&field, ValueId::UNIT, true);
        }
        if c != 0 {
            self.scan_optional_equals()?;
            let name = self.scan_file_name()?;
            let key = if name.contains('.') {
                name.clone()
            } else {
                format!("{name}.tex")
            };
            // (a document's file the client cut: open, its text read by
            // the first line read from it, `Run::next_read_line`; a run
            // that only tests the stream, `\ifeof` after it opens, reads
            // whether it is cut, not its text, so an edit inside the file
            // reaches only the runs that read its lines)
            if self.read_cut(&key)?.is_some() {
                self.set(&field, v.list(vec![v.int(LAZY_OPEN), v.str(&key)]), true);
                return Ok(());
            }
            // (so too a file the job wrote a round before and reads back
            // per field: a test of the stream reads whether it exists,
            // not its text)
            if self.get(&format!("wopen:{key}"))?.is_none() && self.written_per_field(&key)? {
                if self.file_lazy(&key)?.is_some() {
                    self.set(&field, v.list(vec![v.int(LAZY_OPEN), v.str(&key)]), true);
                }
                return Ok(());
            }
            let Some(x) = self.read_file(&key)? else {
                return Ok(());
            };
            let xs = list_of(v, x);
            if xs.len() >= 2 {
                let text = str_of(v, xs[1]);
                let lines: Vec<ValueId> = crate::expand::split_lines(&text)
                    .iter()
                    .map(|l| v.str(l))
                    .collect();
                let st = v.list(vec![v.int(1), v.list(lines)]);
                self.set(&field, st, true);
            }
        }
        Ok(())
    }

    /// The next line of the open stream whose state `st` field `field`
    /// holds (`[state, lines, position]`): the position moves on, or at
    /// the end (`None`) the stream closes (§486). The lines are kept as
    /// they were read in; only the position changes per line.
    pub(crate) fn next_read_line(&mut self, field: &str, st: ValueId) -> R<Option<String>> {
        let v = self.v;
        // (a stream opened on a file's units: its lines read now)
        let st = match &*v.get(st) {
            Value::List(xs)
                if xs
                    .first()
                    .is_some_and(|&c| crate::val::int_of(v, c) == LAZY_OPEN) =>
            {
                let key = xs.get(1).map(|&k| str_of(v, k)).unwrap_or_default();
                let text = match self.read_file(&key)? {
                    Some(x) => list_of(v, x)
                        .get(1)
                        .map(|&t| str_of(v, t))
                        .unwrap_or_default(),
                    None => String::new(),
                };
                let lines: Vec<ValueId> = crate::expand::split_lines(&text)
                    .iter()
                    .map(|l| v.str(l))
                    .collect();
                v.list(vec![v.int(1), v.list(lines)])
            }
            _ => st,
        };
        let (lines, pos) = match &*v.get(st) {
            Value::List(xs) => (
                xs.get(1).copied(),
                xs.get(2).map_or(0, |&p| crate::val::int_of(v, p)),
            ),
            _ => (None, 0),
        };
        let at = usize::try_from(pos).unwrap_or(0);
        let line = lines.and_then(|l| match &*v.get(l) {
            Value::List(xs) => xs.get(at).copied(),
            _ => None,
        });
        if let (Some(l), Some(lines)) = (line, lines) {
            let next = v.list(vec![v.int(2), lines, v.int(pos + 1)]);
            self.set(field, next, true);
            Ok(Some(str_of(v, l)))
        } else {
            self.set(field, ValueId::UNIT, true);
            Ok(None)
        }
    }

    /// `read_toks` (§482–§486): the lines of stream `n` up to balanced
    /// braces.
    fn read_toks(&mut self, n: i32) -> R<Vec<Tok>> {
        let m = if (0..=15).contains(&n) { n } else { 16 };
        let field = format!("read:{m}");
        let mut out = Vec::new();
        let mut align: i64 = 0;
        loop {
            let st = self.get(&field)?;
            let open = st.filter(|&x| x != ValueId::UNIT);
            let Some(st) = open else {
                // §484: from the terminal: a fatal error in the nonstop
                // modes; a build has no terminal ([`Run::fatal`])
                return self.fatal("*** (cannot \\read from terminal in nonstop modes)");
            };
            let line = self.next_read_line(&field, st)?;
            let text = line.unwrap_or_default();
            let e = i64::from(self.ipar(Par::endlinechar)?);
            let line = crate::input::Line::of_str(&text, e, true);
            let saved = std::mem::replace(&mut self.input, crate::input::Input::new());
            let saved_eof = self.eof;
            self.eof = true;
            self.input.push_line(line);
            let mut res = Ok(());
            loop {
                match self.next_tok() {
                    Ok(Some(t)) => {
                        if !t.is_cs_or_active() {
                            match t.cat() {
                                Some(1) => align += 1,
                                Some(2) => align -= 1,
                                _ => {}
                            }
                        }
                        if align < 0 {
                            // §485: an unmatched } aborts the line
                            align = 0;
                            while let Ok(Some(_)) = self.next_tok() {}
                            break;
                        }
                        out.push(t);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        res = Err(e);
                        break;
                    }
                }
            }
            self.input = saved;
            self.eof = saved_eof;
            res?;
            if align == 0 {
                return Ok(out);
            }
            if self.get(&field)?.is_none_or(|x| x == ValueId::UNIT) {
                return Ok(out);
            }
        }
    }

    /// `show_whatever` (§1293): shown in the log.
    fn show_whatever(&mut self) -> R<()> {
        if self.cur.chr >= 4 {
            return crate::etex::show(self);
        }
        let mut s = String::from("> ");
        match self.cur.chr {
            0 => {
                self.get_token();
                self.ck()?;
                let esc = i64::from(self.ipar(Par::escapechar)?);
                if self.cur.is_cs() {
                    crate::print::sprint_cs(&mut s, self.cur.tok, esc);
                    s.push('=');
                }
                let (m, c, h) = (self.cur_mean()?, self.cur.cmd, self.cur.chr);
                self.print_meaning(&mut s, &m, c, h)?;
            }
            2 => {
                let t = self.the_toks();
                self.ck()?;
                self.show_toks_into(&mut s, &t)?;
            }
            1 => {
                // §1296: `> \\box<n>=`, the box, then "OK" (§1298)
                let n = self.scan_register_num()?;
                self.ck()?;
                let esc = i64::from(self.ipar(Par::escapechar)?);
                let mut head = String::from("> ");
                crate::print::print_esc(&mut head, "box", esc);
                crate::print::print_int(&mut head, i64::from(n));
                head.push('=');
                let mut lines = match self.box_reg(n)? {
                    None => vec!["void".to_owned()],
                    Some(b) => self.show_box_lines(b)?,
                };
                lines[0].insert_str(0, &head);
                let sep = crate::log::BOX_LINE.to_string();
                self.log("box", &lines.join(&sep))?;
                return self.log_ok();
            }
            _ => {
                // \\showlists (§1293's `show_activities`, §218): its
                // display begins with an empty line
                self.log("box", "")?;
                return self.log_ok();
            }
        }
        s.push('.');
        self.show_ended();
        self.log("show", &s)
    }

    /// `\setlanguage` (§1377): a language whatsit in horizontal mode,
    /// the paragraph's `clang` set.
    fn set_language(&mut self, md: &crate::build::Mode) -> R<()> {
        if md.m.abs() != 102 {
            return self.report_illegal_case(md);
        }
        let v = self.v;
        let n = self.scan_int()?;
        let lang = i64::from(phitex_typeset::hyph::cur_lang(n));
        let node = self.language_node(lang)?;
        if let Some(l) = self.level_of(md.lid) {
            l.clang = lang;
        }
        let x = crate::nodes::encode(v, &node, &crate::nodes::Side::default());
        self.append(md.lid, x);
        Ok(())
    }

    /// `do_extension` (§1348).
    fn do_extension(&mut self, md: &crate::build::Mode) -> R<()> {
        let v = self.v;
        match self.cur.chr {
            0 | 2 => {
                // \openout, \closeout (§1351, §1353)
                let c = self.cur.chr;
                let n = self.scan_four_bit_int()?;
                let name = if c == 0 {
                    self.scan_optional_equals()?;
                    self.scan_file_name()?
                } else {
                    String::new()
                };
                let w = v.list(vec![
                    v.str(if c == 0 { "open" } else { "close" }),
                    v.int(i64::from(n)),
                    v.str(&name),
                ]);
                let node = v.list(vec![v.int(crate::nodes::T_WHATSIT), w]);
                self.append(md.lid, node);
            }
            1 => {
                // \write (§1352)
                let k = self.scan_int()?;
                let (_, body) = self.scan_toks(false, false);
                self.ck()?;
                let w = v.list(vec![
                    v.str("write"),
                    v.int(i64::from(k)),
                    toks_value(v, &body),
                ]);
                let node = v.list(vec![v.int(crate::nodes::T_WHATSIT), w]);
                self.append(md.lid, node);
            }
            3 => {
                // \special (§1354)
                let (_, body) = self.scan_toks(false, true);
                self.ck()?;
                let mut s = String::new();
                self.show_toks_into(&mut s, &body)?;
                let w = v.list(vec![v.str("special"), v.str(&s)]);
                let node = v.list(vec![v.int(crate::nodes::T_WHATSIT), w]);
                self.append(md.lid, node);
            }
            4 => {
                // \immediate (§1375)
                self.get_x_token();
                self.ck()?;
                if crate::exec::obs_flag!("PHITEX_CMD_TRACE") && self.cmd_traced() {
                    eprintln!(
                        "cmd-trace {} immediate: {:?} cmd {} chr {}",
                        self.cx.node().raw(),
                        self.cur.tok,
                        self.cur.cmd,
                        self.cur.chr
                    );
                }
                if self.cur.cmd == cmd::EXTENSION && self.cur.chr <= 2 {
                    match self.cur.chr {
                        1 => {
                            let k = self.scan_int()?;
                            let (_, body) = self.scan_toks(false, false);
                            if crate::exec::obs_flag!("PHITEX_CMD_TRACE") && self.cmd_traced() {
                                eprintln!(
                                    "cmd-trace {} immediate write {k}: {} tokens, halt {}",
                                    self.cx.node().raw(),
                                    body.len(),
                                    self.halt.is_some()
                                );
                            }
                            self.ck()?;
                            self.write_out(k, &body)?;
                        }
                        0 => {
                            let n = self.scan_four_bit_int()?;
                            self.scan_optional_equals()?;
                            let name = self.scan_file_name()?;
                            self.open_out(i64::from(n), &name)?;
                        }
                        _ => {
                            let n = self.scan_four_bit_int()?;
                            self.close_out(n)?;
                        }
                    }
                } else {
                    // pdfTeX §1375: `\immediate\pdfobj`, `\pdfxform`, `\pdfximage`
                    if self.cur.cmd == cmd::EXTENSION {
                        return crate::pdfprim::immediate(self, md);
                    }
                    self.back_cur();
                }
            }
            5 => self.set_language(md)?,
            _ => {
                let _ = self.scan_int()?;
            }
        }
        Ok(())
    }

    /// `write_out` (§1370–§1371): the tokens expanded, to the log.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn write_out(&mut self, k: i32, body: &[Tok]) -> R<()> {
        if crate::exec::obs_flag!("PHITEX_CMD_TRACE") && self.cmd_traced() {
            eprintln!("cmd-trace {} write_out {k}", self.cx.node().raw());
        }
        let was = self.lines_sink_begin(Sink::Log);
        let s = self.expand_late(body);
        let hit = self.lines_sink_end(was);
        if crate::exec::obs_flag!("PHITEX_AUX_TRACE") && s.is_err() {
            eprintln!(
                "aux-trace: write_out stream {k} node {} halted expanding its text",
                self.cx.node().raw()
            );
        }
        let s = s?;
        // (web2c's §1370 change: stream 18 is a shell command)
        if k == 18 && self.int32("shellescape")? != 0 {
            return self.shell_command(&s);
        }
        let open = (0..16).contains(&k)
            && !self.closed.contains(&k)
            && self
                .get(&format!("write:{k}"))?
                .is_some_and(|x| x != ValueId::UNIT);
        // (the lines read into a file's text, or a shell command: seen
        // after the run)
        if hit && (open || k == 18) {
            self.lt.strong = true;
        }
        if crate::exec::obs_flag!("PHITEX_AUX_TRACE") && s.is_empty() && self.interp.is_some() {
            let mut shown = String::new();
            self.show_toks_into(&mut shown, body)?;
            let t = self.get("toks:11")?.map(|x| self.v.render(x));
            eprintln!("aux-trace: empty write: body {shown:?}, toks:11 = {t:?}");
        }
        if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
            let w = self
                .get(&format!("write:{k}"))?
                .map(|x| crate::val::show_def(self.v, x));
            eprintln!(
                "aux-trace: write_out stream {k} open={open} ({w:?}) node {} op {:?}: {s}",
                self.cx.node().raw(),
                self.cx.op()
            );
            if s.contains("contentsline") {
                let mut ctx = Vec::new();
                for l in &self.input.levels {
                    match l.kind {
                        crate::input::kind::TOKS => ctx.push(format!("T{}", l.ty)),
                        crate::input::kind::LINE => {
                            let ln = self.input.line(*l);
                            let t: String = ln
                                .buf
                                .iter()
                                .take(50)
                                .filter_map(|&c| char::from_u32(c))
                                .collect();
                            ctx.push(format!("L[{t}]"));
                        }
                        _ => ctx.push("F".to_owned()),
                    }
                }
                eprintln!("aux-trace: context {}", ctx.join(" | "));
            }
        }
        if open {
            let v = self.v;
            let e = v.list(vec![v.int(i64::from(k)), v.str(&s)]);
            let su = v.seq_unit(e);
            self.contribute("writes", su);
            // the file's text (§1370: `print` with `\newlinechar`, then
            // `print_ln`)
            let name = self
                .get(&format!("write:{k}"))?
                .and_then(|x| list_of(v, x).first().map(|&n| str_of(v, n)))
                .unwrap_or_default();
            let nl = i64::from(self.ipar(Par::newlinechar)?);
            let mut line: String = s
                .chars()
                .map(|c| {
                    if i64::from(u32::from(c)) == nl {
                        '\n'
                    } else {
                        c
                    }
                })
                .collect();
            line.push('\n');
            // (XeTeX writes UTF-8, controls in `^^` form; the line read
            // back here is those bytes read as XeTeX reads them)
            let line = if self.xe {
                crate::xetex::file_text(&line)
            } else {
                line
            };
            // a file whose read-back only defines: the definitions this
            // line's read-back makes go with it (`wdef:<field>`); a line
            // that does more (contributes, typesets, opens a scope) makes
            // the file one read back as its text (`UNIT` in their place)
            // (a line written while a line is interpreted goes nowhere:
            // not interpreted in turn)
            let per_field = self.interp.is_none()
                && self.reads_back_per_field(&name)
                && self
                    .get(&format!("{}{name}", crate::run::REREAD))?
                    .is_none();
            let e = if per_field {
                // (a text the stream's earlier lines left open goes on
                // with this one: the file is read back whole)
                let text = match self.take_wpending(k)? {
                    Some(mut t) => {
                        t.push_str(&line);
                        t
                    }
                    None => line.clone(),
                };
                self.interp_point = self.read_point_of(&name)?;
                let r = if self.xe {
                    self.interpret_line(&crate::xetex::reread(&text))
                } else {
                    self.interpret_line(&text)
                };
                self.interp_point = None;
                let r = r?;
                if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
                    eprintln!(
                        "aux-trace: interpreted {name} {text:?} in node {} (read-back point {:?}, only defines {}): {}",
                        self.cx.node().raw(),
                        self.read_point_of(&name)?.map(phitex_defuse::NodeId::raw),
                        self.interp_plain,
                        r.map_or_else(|| "needs more".to_owned(), |x| crate::val::show_def(v, x))
                    );
                }
                if let Some(made) = r
                    && self.interp_plain
                {
                    v.list(vec![v.str(&name), v.str(&line), made, self.interp_reads])
                } else if r.is_some() {
                    self.muts += 1;
                    self.text_written.push(name.clone());
                    v.list(vec![v.str(&name), v.str(&line), ValueId::UNIT])
                } else {
                    self.set(&format!("write:pend:{k}"), v.str(&text), true);
                    v.list(vec![v.str(&name), v.str(&line)])
                }
            } else {
                if crate::exec::obs_flag!("PHITEX_AUX_TRACE") && self.interp.is_none() {
                    eprintln!(
                        "aux-trace: {name} line {line:?} not interpreted in node {}: an earlier line of this run did more than define",
                        self.cx.node().raw()
                    );
                }
                v.list(vec![v.str(&name), v.str(&line), ValueId::UNIT])
            };
            self.contribute_stream(e);
        } else {
            self.log("wlog", &s)?;
        }
        Ok(())
    }

    /// `\\write18{cmd}` with shell escape on (web2c's `runsystem`,
    /// [`crate::shell::runsystem`]): in restricted mode (2) only a
    /// command of [`crate::Options::shell_commands`] is allowed, each
    /// argument quoted; unrestricted (1), any, as written. No command runs
    /// as a process here: an allowed command that is one of the tools
    /// ([`crate::tools`]) is that tool's node, placed where the command
    /// is ([`Run::tool_command`]): a later read of its output reads what
    /// it made of its input as written up to the command this round; any
    /// other command runs here through the host's hook
    /// ([`Run::shell_run`]), or is refused with a diagnostic where the
    /// client gave no hook.
    fn shell_command(&mut self, cmd: &str) -> R<()> {
        let restricted = self.int32("shellescape")? == 2;
        let (allowed, hook) = {
            let o = self
                .ops
                .tool_opts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (o.shell_commands.clone(), o.shell_hook.is_some())
        };
        let quoted = match crate::shell::runsystem(cmd, restricted, &allowed) {
            crate::shell::Decision::QuotationError => {
                self.log(
                    "wlog",
                    &format!("runsystem({cmd})...quotation error in system command."),
                )?;
                return Ok(());
            }
            crate::shell::Decision::Restricted => {
                self.log(
                    "wlog",
                    &format!("runsystem({cmd})...disabled (restricted)."),
                )?;
                return Ok(());
            }
            crate::shell::Decision::Any(c) | crate::shell::Decision::Allowed(c) => c,
        };
        let done = if restricted {
            "executed safely (allowed)"
        } else {
            "executed"
        };
        let words = shell_words(cmd);
        let name = match &words {
            Some(w) => w.first().cloned().unwrap_or_default(),
            None => cmd.split_whitespace().next().unwrap_or_default().to_owned(),
        };
        let Some(tool) = crate::tools::Tool::of_name(&name) else {
            if !hook {
                self.diag(
                    false,
                    &format!("\\write18{{{cmd}}}: `{name}' has no in-process version here and the client runs no commands; not run"),
                );
                return Ok(());
            }
            self.log("wlog", &format!("runsystem({cmd})...{done}."))?;
            // (the command runs here, in this run, in TeX's order: what
            // the run reads after it reads what it made)
            return self.shell_run(&quoted);
        };
        // (a tool's arguments, as its command line has them)
        let Some(args) = words else {
            self.log(
                "wlog",
                &format!("runsystem({cmd})...quotation error in system command."),
            )?;
            return Ok(());
        };
        let Some((output, input)) = tool.command_files(&args[1..]) else {
            return Ok(());
        };
        self.log("wlog", &format!("runsystem({cmd})...{done}."))?;
        // (the command runs here, in this round: a node of its own where
        // the command is, reading the input as written up to there)
        let a = crate::tools::command_args(self.v, tool, &output, &input, &args[1..]);
        self.tool_command(a)
    }

    /// Stream `k`'s written text still incomplete (`\@writeckpt`'s
    /// `\@setckpt{...}{` over several lines), continued by its next line:
    /// TeX state (`write:pend:<k>`, global like the stream), so a line written
    /// in another node goes on with it; taken (cleared) here.
    fn take_wpending(&mut self, k: i32) -> R<Option<String>> {
        let v = self.v;
        let f = format!("write:pend:{k}");
        let t = self
            .get(&f)?
            .filter(|&x| x != ValueId::UNIT)
            .map(|x| str_of(v, x));
        if t.is_some() {
            self.set(&f, ValueId::UNIT, true);
        }
        Ok(t)
    }

    /// The text of stream `n`'s file left open by its last line, read
    /// back to the file's end: what §338 inserts ends it, its errors
    /// (`rberr:<name>`, given again at each read-back) and definitions go
    /// with the file as a line's do.
    fn file_end_read_back(&mut self, n: i32, text: &str) -> R<()> {
        let v = self.v;
        let name = self
            .get(&format!("write:{n}"))?
            .and_then(|x| list_of(v, x).first().map(|&m| str_of(v, m)))
            .unwrap_or_default();
        self.interp_eof = true;
        self.interp_errs.clear();
        self.interp_point = self.read_point_of(&name)?;
        let r = self.interpret_line(text);
        self.interp_point = None;
        self.interp_eof = false;
        let errs = std::mem::take(&mut self.interp_errs);
        let made = r?;
        let mut ds = made.map(|m| list_of(v, m)).unwrap_or_default();
        if made.is_none() {
            // (nothing was being scanned, §336 at `normal`: TeX reads on
            // past the file's end, which a read-back by fields cannot)
            self.diag(
                false,
                &format!("{name}: the text written last is incomplete at its end; not read back"),
            );
        }
        if !errs.is_empty() {
            ds.push(v.list(vec![
                v.str(&format!("rberr:{name}")),
                v.list(errs.iter().map(|e| v.str(e)).collect()),
                v.int(1),
            ]));
        }
        if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
            eprintln!("aux-trace: {name} ends inside a command: {errs:?}");
        }
        self.contribute_stream(v.list(vec![
            v.str(&name),
            v.str(""),
            v.list(ds),
            self.interp_reads,
        ]));
        Ok(())
    }

    /// `\closeout` done (§1374): in a page step or a text paging inline
    /// the stream is closed for what follows; in a document node the
    /// close is an event at its position (a `[name, "", "close"]` stream
    /// entry the writer applies in the pages' order, [`Run::closed`]),
    /// not a definition the page steps would see before their deferred
    /// writes fire.
    pub(crate) fn close_out(&mut self, n: i32) -> R<()> {
        let v = self.v;
        // a text the file's lines left open ends with the file: read
        // back as TeX's `\input` of it reads its end (§362, §336–§339)
        if let Some(text) = self.take_wpending(n)? {
            self.file_end_read_back(n, &text)?;
        }
        let op = self.cx.op();
        if op == self.ops.finish {
            self.set(&format!("write:{n}"), ValueId::UNIT, true);
            return Ok(());
        }
        let name = self
            .get(&format!("write:{n}"))?
            .and_then(|x| list_of(v, x).first().map(|&m| str_of(v, m)));
        if !self.closed.contains(&n) {
            self.muts += 1;
            self.closed.push(n);
        }
        if let Some(name) = name {
            self.contribute_stream(v.list(vec![v.str(&name), v.str(""), v.str("close")]));
        }
        Ok(())
    }

    /// `\openout` done (§1374): the stream names the file (`.tex` if it
    /// has no extension), which starts empty: its text accumulates in
    /// the `streams` sequence, and `wopen:<name>` tells later readers
    /// of the file that this run wrote it.
    pub(crate) fn open_out(&mut self, n: i64, name: &str) -> R<()> {
        let v = self.v;
        let base = name.rsplit('/').next().unwrap_or(name);
        let name = if base.contains('.') {
            name.to_owned()
        } else {
            format!("{name}.tex")
        };
        // (web2c §1374: `kpse_out_name_ok` refuses the name, and
        // `prompt_file_name` ends a nonstop job, §530)
        let ok = self.ops.out_ok.locked().clone();
        let allowed = match ok {
            Some(f) => f(&name),
            None => out_name_ok_p(&name),
        };
        if !allowed {
            self.log("error", &format!("I can't write on file `{name}'"))?;
            return self.fatal("*** (job aborted, file error in nonstop mode)");
        }
        self.opened_for_writing(&name)?;
        // (§1374: the stream is open from here, whatever this run closed
        // of it before)
        if let Ok(k) = i32::try_from(n)
            && self.closed.contains(&k)
        {
            self.muts += 1;
            self.closed.retain(|&c| c != k);
        }
        self.set(&format!("write:{n}"), v.list(vec![v.str(&name)]), true);
        self.set(&format!("wopen:{name}"), v.int(1), true);
        self.contribute_stream(v.list(vec![v.str(&name)]));
        // (web2c's §1374, `log_openout`: the file opened, in the log)
        self.log("openout", &format!("\\openout{n} = `{name}'."))?;
        Ok(())
    }

    /// The file `key` as TeX reads it here: if this run opened it for
    /// writing before this point, the text written to it so far (TeX
    /// reads what it wrote, as LaTeX's `\enddocument` reads the `.aux`
    /// file); else the client's `file:` field (a loop-back field for a
    /// file the job writes: the previous round's text).
    pub(crate) fn read_file(&mut self, key: &str) -> R<Option<ValueId>> {
        if self.get(&format!("wopen:{key}"))?.is_none() {
            return self.file(key);
        }
        self.read_own_written(key)
    }

    /// [`Run::read_file`], a file the job writes left as its units
    /// (`[path, UNIT, order]`, [`Run::file_lazy`]).
    pub(crate) fn read_file_lazy(&mut self, key: &str) -> R<Option<ValueId>> {
        if self.get(&format!("wopen:{key}"))?.is_none() {
            return self.file_lazy(key);
        }
        self.read_own_written(key)
    }

    /// A file this run wrote before this point: the text written so far.
    fn read_own_written(&mut self, key: &str) -> R<Option<ValueId>> {
        let v = self.v;
        let f = v.field("streams");
        let before = self.cx.fold(f, None, None);
        if before.is_err() {
            self.trace_stop("range streams");
        }
        let before = before?;
        let mut all = v.seq_to_vec(before);
        all.extend(
            self.contribs
                .iter()
                .filter(|(g, _)| *g == f)
                .flat_map(|(_, x)| v.seq_to_vec(*x)),
        );
        let text = crate::api::stream_texts(v, &crate::api::doc_order(v, &all))
            .into_iter()
            .find(|(n, _)| n == key)
            .map(|(_, t)| t)
            .unwrap_or_default();
        Ok(Some(v.list(vec![v.str(&format!("./{key}")), v.str(&text)])))
    }

    /// §1371: a token list expanded as `\write` expands it at
    /// `\shipout` (also `\pdfliteral shipout`), shown as a string.
    pub(crate) fn expand_late(&mut self, body: &[Tok]) -> R<String> {
        let mut toks = vec![Tok::ch(1, u32::from(b'{'))];
        toks.extend(body.iter().copied());
        toks.push(Tok::ch(2, u32::from(b'}')));
        toks.push(Tok::cs("\u{10FFFE}endwrite"));
        self.push_toks(toks);
        // §1370: `mode:=0` while the list is expanded
        let old = std::mem::replace(&mut self.mode_zero, true);
        let (_, expanded) = self.scan_toks(false, true);
        self.mode_zero = old;
        self.ck()?;
        self.get_token();
        self.ck()?;
        let mut s = String::new();
        self.show_toks_into(&mut s, &expanded)?;
        Ok(s)
    }

    /// A font loaded as `f` by the text (not by a page step's output
    /// routine or virtual fonts, nor by a text paging inline, whose pages
    /// ship in TeX's order): a `T_FONT_AT` marker on the main list, so
    /// that the writer knows which `\shipout` came first after it.
    pub(crate) fn font_loaded(&mut self, f: i64) {
        let op = self.cx.op();
        if op == self.ops.finish {
            return;
        }
        let v = self.v;
        let at = v.list(vec![v.int(crate::nodes::T_FONT_AT), v.int(f)]);
        self.contribute("items", v.seq_unit(v.list(vec![v.int(0), at])));
    }

    /// A piece's read of the lines before it (`\inputlineno`), noted by
    /// where the number goes ([`LinesTaint`]).
    fn note_lines_read(&mut self) {
        let piece = self.cx.op() == self.ops.exec || self.cx.op() == self.ops.resume;
        // (a number read into the log stands: the log is the run's
        // output, and a number taken back there is the one a run read
        // before the lines ahead of it were in, `D=1` for line 6 at -j8)
        match self.lt.sink {
            Some(Sink::Def) if piece => {
                self.lt.hit = true;
                self.lt.weak = true;
            }
            _ => {
                if crate::exec::obs_flag!("PHITEX_LINES_TRACE") {
                    let ctx: Vec<String> = self
                        .input
                        .levels
                        .iter()
                        .rev()
                        .take(3)
                        .filter(|l| l.kind == crate::input::kind::TOKS)
                        .map(|l| {
                            let toks: Vec<String> = self
                                .input
                                .arena
                                .get(l.rest())
                                .iter()
                                .take(6)
                                .map(|t| self.v.render(t.encode(self.v)))
                                .collect();
                            toks.join(" ")
                        })
                        .collect();
                    eprintln!(
                        "lines-strong: node {} sink {:?} | {}",
                        self.cx.node().raw(),
                        self.lt.sink,
                        ctx.join(" | ")
                    );
                }
                self.lt.strong = true;
            }
        }
    }

    /// The expansion of a body for `sink` begins ([`LinesTaint`]): what
    /// it was before, for [`Run::lines_sink_end`].
    pub(crate) fn lines_sink_begin(&mut self, sink: Sink) -> (Option<Sink>, bool) {
        let was = (self.lt.sink, self.lt.hit);
        self.lt.sink = Some(sink);
        self.lt.hit = false;
        was
    }

    /// The body's expansion ended: whether it read the lines.
    pub(crate) fn lines_sink_end(&mut self, was: (Option<Sink>, bool)) -> bool {
        let hit = self.lt.hit;
        (self.lt.sink, self.lt.hit) = was;
        hit
    }

    /// The run's end: where it read the lines before it only into bodies
    /// whose meanings nothing looked at and nothing after it sees, its
    /// result does not depend on them, and the reads are taken back
    /// ([`phitex_defuse::Ctx::unread`]): an edit that moves those lines
    /// leaves the piece standing. (A number read into the log is output:
    /// its read stands, [`Run::note_lines_read`].)
    pub(crate) fn settle_lines(&mut self) {
        // (`PHITEX_LINES_TRACE=1`, observe: each piece that read them, and
        // why its reads stand)
        let trace =
            crate::exec::obs_flag!("PHITEX_LINES_TRACE") && (self.lt.weak || self.lt.strong);
        if trace {
            let v = self.v;
            let tf: Vec<phitex_defuse::Field> = self
                .st
                .tainted()
                .to_vec()
                .into_iter()
                .map(|l| self.loc_field(l))
                .collect();
            let pubd = self.publication();
            let tainted: Vec<String> = tf.iter().map(|&f| crate::val::field_name(v, f)).collect();
            eprintln!(
                "lines-trace: node {} {:?} read {} tainted {tainted:?} published {}",
                self.cx.node().raw(),
                self.lt,
                self.st.taint_read(),
                pubd.iter().any(|d| tf.contains(&d.0))
            );
        }
        if !self.lt.weak || self.lt.strong || self.st.taint_read() {
            return;
        }
        let v = self.v;
        if !self.st.tainted().is_empty() {
            let tf: Vec<phitex_defuse::Field> = self
                .st
                .tainted()
                .to_vec()
                .into_iter()
                .map(|l| self.loc_field(l))
                .collect();
            if self.publication().iter().any(|d| tf.contains(&d.0)) {
                return;
            }
        }
        LINES_TAKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.cx.unread(v.field("lines"));
        for f in std::mem::take(&mut self.dline_reads) {
            self.cx.unread(f);
        }
    }

    /// The lines of the document file `name` the nodes before this one
    /// began (the `lines` accumulator, its own file's count only).
    pub(crate) fn piece_lines_before(&mut self, name: ValueId) -> R<i64> {
        let v = self.v;
        let lines = v.field("lines");
        // (observe, `PHITEX_READS=lines`: each reader, where)
        if crate::exec::obs_flag!("PHITEX_READS") {
            self.trace_read(lines, false);
        }
        // (its own file's count only: an edit of another file's
        // lines leaves the read standing)
        let p = self.cx.path(&[phitex_defuse::Step::Key(name)]);
        let x = self.cx.fold_path(lines, None, None, p);
        if x.is_err() {
            self.trace_stop("range lines");
        }
        let x = x?;
        self.note_lines_read();
        Ok(x.map_or(0, |k| int_of(v, k)))
    }

    /// The line of a document's piece's line `own` (its node and its line
    /// in that node's text, [`crate::input::Line::owner`]): the lines of
    /// its file before that node's text, as the client gives them where
    /// it cut the text ([`crate::run::dline_field`], an environment read:
    /// an edit above it re-asks only a run whose answer moves, and one
    /// that read it into nothing that stands takes it back,
    /// [`Run::settle_lines`]), and its line there. None where the client
    /// gives none (a piece of a file read back): the fold then.
    pub(crate) fn piece_line(&mut self, own: (u64, u32)) -> R<Option<i64>> {
        let f = crate::run::dline_field(self.v, own.0);
        let Some(x) = self.cx.env(f)? else {
            return Ok(None);
        };
        if !self.dline_reads.contains(&f) {
            self.dline_reads.push(f);
        }
        self.note_lines_read();
        Ok(Some(int_of(self.v, x) + i64::from(own.1)))
    }

    /// The input left as one source line, for `\inputlineno`.
    ///
    /// TeX's `line` (§304): the line of the innermost file read. A line
    /// of a document's piece has its number from the lines of its file
    /// the nodes before it began (the `lines` accumulator: per file, so
    /// no other file's lines count) and those this node began.
    pub(crate) fn input_line_no(&mut self) -> R<i32> {
        let n = match self.file_line_here() {
            Some(crate::run::FileLine::Known(n)) => n,
            Some(crate::run::FileLine::Piece(name, own)) => {
                // (its line from where its node's text is: no fold of the
                // lines the nodes before this one began)
                if let Some(own) = own
                    && let Some(n) = self.piece_line(own)?
                {
                    // (`PHITEX_LINES_CHECK`, observe: against the fold of
                    // the lines the nodes before this one began, at one
                    // thread the same)
                    if crate::exec::obs_flag!("PHITEX_LINES_CHECK") {
                        let before = self.piece_lines_before(name)?;
                        let mine = self
                            .doc_lines
                            .iter()
                            .find(|(n, _)| *n == name)
                            .map_or(0, |&(_, k)| k);
                        if before + mine != n {
                            eprintln!(
                                "lines-check: node {} line {n} by its place, {} by the fold (owner {own:?})",
                                self.cx.node().raw(),
                                before + mine
                            );
                        }
                    }
                    n
                } else {
                    let before = self.piece_lines_before(name)?;
                    let mine = self
                        .doc_lines
                        .iter()
                        .find(|(n, _)| *n == name)
                        .map_or(0, |&(_, k)| k);
                    before + mine
                }
            }
            None => 0,
        };
        if crate::exec::obs_flag!("PHITEX_LINE_TRACE") {
            let levels: Vec<String> = self
                .input
                .levels
                .iter()
                .map(|l| match l.kind {
                    crate::input::kind::TOKS => format!("T{}", l.ty),
                    crate::input::kind::LINE => {
                        let ln = self.input.line(*l);
                        format!(
                            "L(no {} origin {} rescanned {})",
                            ln.lineno,
                            ln.origin.is_some(),
                            ln.rescanned
                        )
                    }
                    _ => format!("F{}", self.input.file(*l).line),
                })
                .collect();
            let frames = self
                .cx
                .args()
                .first()
                .map(|&f| self.v.render(f).chars().take(200).collect::<String>());
            eprintln!(
                "line-trace: inputlineno {n} node {} op {:?} levels {levels:?} parked {} text_at {} frames {frames:?}",
                self.cx.node().raw(),
                self.cx.op(),
                self.parked_levels,
                self.input.text_at
            );
        }
        Ok(i32::try_from(n).unwrap_or(0))
    }
}

/// §1239: the sum of two glue specifications.
pub(crate) fn add_glue(q: &Glue, r: &Glue) -> Glue {
    let mut g = *q;
    g.width += r.width;
    if g.stretch == 0 {
        g.stretch_order = phitex_typeset::glue::Order::Normal;
    }
    if g.stretch_order == r.stretch_order {
        g.stretch += r.stretch;
    } else if g.stretch_order < r.stretch_order && r.stretch != 0 {
        g.stretch = r.stretch;
        g.stretch_order = r.stretch_order;
    }
    if g.shrink == 0 {
        g.shrink_order = phitex_typeset::glue::Order::Normal;
    }
    if g.shrink_order == r.shrink_order {
        g.shrink += r.shrink;
    } else if g.shrink_order < r.shrink_order && r.shrink != 0 {
        g.shrink = r.shrink;
        g.shrink_order = r.shrink_order;
    }
    g
}

/// kpathsea's `kpse_out_name_ok` (`tex-file.c`, TeX Live 2026, not
/// extended) as TeX Live runs it by default, `openout_any = p` with no
/// output directory: no dot file (a dot at the start or after a `/`
/// begins `./` or `../`), no absolute name, no `..` component. (The
/// client's check, with its `openout_any` and output directories, is
/// [`crate::Options::out_name_ok`]; `phitex_kpse::Kpse::out_name_ok` is
/// the CLI's.)
fn out_name_ok_p(name: &str) -> bool {
    let b = name.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c != b'.' || i > 0 && b[i - 1] != b'/' {
            continue;
        }
        let next = b.get(i + 1).copied();
        let dot_slash = next == Some(b'/');
        let dot_dot_slash = next == Some(b'.') && b.get(i + 2) == Some(&b'/');
        if !dot_slash && !dot_dot_slash {
            return false;
        }
    }
    !(b.first() == Some(&b'/') || b.starts_with(b"../") || b.windows(4).any(|w| w == b"/../"))
}

/// A command line split into words as kpathsea's `shell_cmd_is_allowed`
/// reads it in restricted mode (texmfmp.c): blank-separated, a `"..."`
/// quoted part taken literally (a word may join quoted and unquoted
/// parts); `None` when a quote is left open or a `'` appears (kpathsea
/// quotes with `'`, refusing one in the command).
pub(crate) fn shell_words(cmd: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quoted = false;
    for c in cmd.chars() {
        match c {
            '\'' => return None,
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            ' ' | '\t' if !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            _ => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quoted {
        return None;
    }
    if in_word {
        words.push(cur);
    }
    Some(words)
}

/// The runs whose reads of the lines before them were taken back
/// ([`Run::settle_lines`], reported with `PHITEX_TEX_TIME`).
pub(crate) static LINES_TAKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Where a body expanded now goes (`\inputlineno`'s number read into
/// it, [`LinesTaint`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Sink {
    /// A meaning's body (`\edef`).
    Def,
    /// The log or the terminal (`\message`, a `\write` to no open file).
    Log,
}

/// Where a piece's reads of the lines before it (`\inputlineno`) went:
/// only into meanings nothing looks at after (a group's line number kept
/// for an error message), the piece's result does not depend on them
/// ([`Run::settle_lines`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct LinesTaint {
    /// The body being expanded, and whether it read the lines.
    pub sink: Option<Sink>,
    pub hit: bool,
    /// The lines read into a sink, and anywhere else.
    pub weak: bool,
    pub strong: bool,
}
