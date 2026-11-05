//! An edited paragraph's run goes on from inside it.
//!
//! A document node's text is one paragraph or more (every paragraph end
//! is a cut). A run of an edited node (woken by `Edited`) takes
//! checkpoints as it goes: at a command's end, every so many ([`every`])
//! commands, where the run stands in a paragraph of its own text with
//! that text's line being read (above it at most the tokens a command
//! read ahead and put back; no file, no group or conditional of its own,
//! no sibling claimed). A checkpoint is the run's state table at its base
//! level ([`crate::state::State::base_of`]), the nest with the
//! paragraph's list, the conditionals and the counters
//! ([`Run::cont_encode`]), the tokens read ahead, and what the run has produced so far that the continuation
//! does not hold (its contributions, the lines it read, the files it
//! read, its counters), with where it stood in its text: the segment and
//! the character. The checkpoints go with the run's record
//! ([`phitex_defuse::Ctx::park`]), with the text they were taken in.
//!
//! The node's next run, if its text was edited again, goes on from the
//! last checkpoint the edit left in place: the segments before its
//! segment the same, its segment the same up to its character, the
//! node's other arguments the same ([`Run::resume_edit`]). The state is
//! the checkpoint's; the input is the new text from there. The engine
//! keeps the last run's reads with this run's
//! ([`phitex_defuse::Ctx::goes_on_from_edit`]): a change behind the
//! checkpoint runs the node again from its start.
//!
//! `PHITEX_RESUME_CHECK=1`: a run that went on from a checkpoint is run
//! again from its start, and the two runs' products compared
//! ([`Run::product_signature`]); the full run's stands.

use phitex_defuse::{Field, ValueId};

use crate::input::Seg;
use crate::run::{R, Run};
use crate::val::{int_of, list_of};

/// The commands between two checkpoints of a run (`observe`:
/// `PHITEX_CK_EVERY`, for the tests, another number).
/// How many commands a checkpoint's parts' length in entries buys: the
/// next is due that many times its length after it, at least [`every`].
const SPACE: u64 = 4;

/// The commands from a checkpoint to the next: `spacing` (`SPACE` times
/// what the run's parts cost) apart at least, or `every` apart in a dense
/// run (an edited one); doubling in a dense run gone on from a checkpoint
/// before the edit (`resumed`): those it takes stand past the edit, of use
/// only to a later edit further on. `taken`: the checkpoints this run has
/// taken so far.
fn ck_gap(dense: bool, resumed: bool, taken: usize, every: u64, spacing: u64) -> u64 {
    if dense && resumed {
        every << (taken + 1).min(16)
    } else if dense {
        every
    } else {
        every.max(SPACE * spacing)
    }
}

fn every() -> u64 {
    crate::exec::obs_str!("PHITEX_CK_EVERY")
        .and_then(|x| x.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(600)
}

/// A run's checkpoints ([`Run::ck`]).
#[derive(Default)]
pub(crate) struct Checkpoints {
    /// The command count the next one is due at.
    next: u64,
    /// Those taken (each [`Run::ck_take`]'s value), in order: a resumed
    /// run's begin with those before the one it went on from.
    taken: Vec<ValueId>,
    /// Of those taken by this run, by index in `taken`: the reads
    /// `unread` had taken out then ([`phitex_defuse::Ctx::unread_count`]),
    /// to make their count of reads the record's at the park.
    fresh: Vec<(usize, usize)>,
    /// `PHITEX_CK_WHY` (observe): why no checkpoint was taken where one
    /// was due, by reason, the commands it was refused at.
    why: Vec<(String, u64)>,
    /// An edited node's run: its checkpoints every [`every`] commands
    /// whatever they cost, as the next edit is most likely in the same
    /// text again (a letter typed after a letter); another run's are
    /// spaced by their cost ([`space`]).
    dense: bool,
    /// The encodings its checkpoints made, for the next ones.
    memo: crate::state::CkMemo,
    /// Its token list levels' encodings by arena and range.
    levels: phitex_defuse::FxHashMap<(u64, u32, u32), ValueId>,
}

/// The paragraph's mode (tex.web's `hmode`, unrestricted).
const HMODE: i32 = 102;

/// A checkpoint's input decoded ([`Run::ck_input_of`]).
struct CkInput {
    input: crate::input::Input,
    pulled: usize,
    pull_at: Vec<Option<usize>>,
    pull_box: Vec<bool>,
    pull_lev: Option<usize>,
    cut_state: Option<u8>,
}

/// (tests) For each checkpoint taken in this process, whether its run
/// had closed or left its scope by then ([`Run::ck_where`] refuses one
/// there).
#[cfg(test)]
static TAKEN: std::sync::Mutex<Vec<bool>> = std::sync::Mutex::new(Vec::new());

/// Where a checkpoint is taken ([`Run::ck_where`]).
enum At {
    /// In a paragraph of the node's own text, its line at the bottom of
    /// the input ([`Run::ck_where_line`]): an edit after it leaves it.
    Line((usize, u32, u8, i64, Vec<u32>)),
    /// Anywhere else: its input whole ([`Run::ck_input`]), for a run asked
    /// again with its text the same.
    Anywhere,
}

impl Run<'_, '_> {
    /// Whether the run takes checkpoints: an exec run of an edited
    /// node's text.
    pub(crate) fn ck_begin(&mut self) {
        // (an edited node's run, or a run of a node whose last run took
        // them: they stay for its next edit)
        // (every exec run: a run asked again because a read of it moved
        // goes on from its last checkpoint before that read, `resume_edit`;
        // their cost, accl 22812: amsldoc -j1 +6%, Grail +2%)
        if !self.ck_op() {
            return;
        }
        let taken = self.ck.take().map(|c| c.taken).unwrap_or_default();
        // (the last run's checkpoints on this text, for the cutoff: not
        // a second pass's, which ends at its candidate whatever the
        // state there)
        if self.stop_at.is_none()
            && let Some(p) = self.cx.parked_state()
        {
            let v = self.v;
            let xs = list_of(v, p);
            if let Some((&old, cks)) = xs.split_first()
                && list_of(v, old) == self.ck_key()
            {
                self.last_cks = Some(cks.to_vec());
            }
        }
        self.ck = Some(Box::new(Checkpoints {
            next: self.cmds + every(),
            dense: matches!(self.cx.woken_by(), Some(phitex_defuse::Cause::Edited)),
            taken,
            fresh: Vec::new(),
            why: Vec::new(),
            memo: crate::state::CkMemo::default(),
            levels: phitex_defuse::FxHashMap::default(),
        }));
    }

    /// Whether the run's operation takes checkpoints: a piece's run, a
    /// document node's text (exec) or the rest of a piece cut at a
    /// candidate (resume).
    fn ck_op(&self) -> bool {
        let op = self.cx.op();
        op == self.ops.exec || op == self.ops.resume
    }

    /// What a run's checkpoints are taken on: the node's arguments, and
    /// a resume's continuation (a field it reads, not an argument).
    fn ck_key(&self) -> Vec<ValueId> {
        let mut k = self.cx.args().to_vec();
        if self.cx.op() == self.ops.resume {
            k.push(self.cont_val.unwrap_or(ValueId::UNIT));
        }
        k
    }

    /// At a command's end: a checkpoint if one is due and the run stands
    /// where one can be taken.
    pub(crate) fn ck_maybe(&mut self) -> R<()> {
        let Some(c) = self.ck.as_ref() else {
            return Ok(());
        };
        if self.cmds < c.next {
            return Ok(());
        }
        // (inside a conditional too: the continuation carries the
        // conditionals open, `conds` and `if_codes`, and whether the run
        // read those a node before it left, as a cut's does; a test's
        // expansion is inside one command, never at a checkpoint)
        if crate::exec::obs_flag!("PHITEX_CK_WHY") && self.ck_where().is_none() {
            let why = self.ck_refusal();
            self.ck_census(why);
        }
        let Some(at) = self.ck_where() else {
            if crate::exec::obs_flag!("PHITEX_CK_TRACE") {
                eprintln!(
                    "ck: node {} at command {}: none here (levels {:?}, nest {:?}, pulled {}, kids {}, lig {}, chars {:?}, conds {})",
                    self.cx.node().raw(),
                    self.cmds,
                    self.input
                        .levels
                        .iter()
                        .map(|l| (l.kind, l.ty, l.b.saturating_sub(l.a)))
                        .collect::<Vec<_>>(),
                    self.nest
                        .iter()
                        .map(|l| (l.mode, l.group))
                        .collect::<Vec<_>>(),
                    self.pulled,
                    self.kids.len(),
                    self.lig_buf.len(),
                    self.chars_live,
                    self.conds.len()
                );
            }
            return Ok(());
        };
        self.ck_take(at)
    }

    /// `PHITEX_CK_WHY`: one refusal counted under `why`.
    fn ck_census(&mut self, why: String) {
        if let Some(c) = self.ck.as_mut() {
            match c.why.iter_mut().find(|w| w.0 == why) {
                Some(w) => w.1 += 1,
                None => c.why.push((why, 1)),
            }
        }
    }

    /// `PHITEX_CK_WHY`: every reason [`Run::ck_where`] refuses here,
    /// joined by `+`.
    fn ck_refusal(&self) -> String {
        let toks = |l: &crate::input::Level| l.kind == crate::input::kind::TOKS;
        let checks: [(bool, &'static str); 24] = [
            (self.input.levels.is_empty(), "no input"),
            (
                !self.input.levels.iter().skip(1).all(toks)
                    && self.input.levels.iter().any(|l| {
                        l.kind != crate::input::kind::TOKS && l.kind != crate::input::kind::LINE
                    }),
                "a file level",
            ),
            (
                !self.input.levels.iter().skip(1).all(toks),
                "a line above the text's",
            ),
            (self.pulled != 0, "siblings pulled"),
            (
                self.kids.iter().any(|k| self.kid_code(k.0).is_none()),
                "children",
            ),
            (self.claim_rest, "claim rest"),
            (self.fatal_hit, "fatal"),
            (self.cut.is_some(), "cut"),
            (self.close_cont.is_some(), "close cont"),
            (self.fin.is_some(), "finisher"),
            (!self.sealed.is_empty(), "sealed"),
            (!self.lig_buf.is_empty(), "ligature"),
            (self.chars_live.is_some(), "chars live"),
            (self.interp.is_some(), "interpreting a line"),
            (self.scanner.is_some(), "scanner"),
            (!self.incs.is_empty(), "incompleat noad"),
            (!self.seeded.is_empty(), "seeded"),
            (self.defer_hit.is_some(), "defer"),
            (self.mode_zero, "mode zero"),
            (
                self.fin_box.is_some() || self.tool_cut.is_some(),
                "fin box/tool",
            ),
            (self.lt.sink.is_some(), "lines sink"),
            (
                self.nest.len() != 1 && self.nest.iter().any(|l| l.owns && l.mode != HMODE),
                "inside a box of its own",
            ),
            (
                self.nest.len() != 1 || self.nest[0].mode != HMODE,
                "not in a paragraph",
            ),
            (self.nest.iter().any(|l| l.group != 0), "inside a group"),
        ];
        let mut all: Vec<&str> = checks.iter().filter(|c| c.0).map(|c| c.1).collect();
        // (and those of a checkpoint anywhere, when none at the line)
        if !self.input.levels.is_empty() && self.ck_where_line().is_none() {
            let anywhere: [(bool, &'static str); 2] = [
                (
                    self.pull_orig.iter().any(Option::is_some),
                    "a pulled line kept",
                ),
                (
                    self.pull_at.len() != self.pulled || self.pull_box.len() != self.pulled,
                    "pulled places unknown",
                ),
            ];
            all.extend(anywhere.iter().filter(|c| c.0).map(|c| c.1));
        }
        if all.is_empty() {
            "the line, the base table or locals".to_owned()
        } else {
            all.join("+")
        }
    }

    /// Where the run stands, if a checkpoint can be taken there
    /// ([`At`]).
    fn ck_where(&self) -> Option<At> {
        // (none once the run has closed or left the scope it runs in, its
        // effect decided: the command that closed it ends the run, which
        // `Run::run_all` sees after the command, after this; a run that
        // went on from a checkpoint taken there read on past `\end{note}`,
        // enumitem -j4)
        if self.input.levels.is_empty()
            || self.effect != phitex_defuse::Effect::None
            || self.kids.iter().any(|k| self.kid_code(k.0).is_none())
            || self.claim_rest
            || self.fatal_hit
            || self.cut.is_some()
            || self.close_cont.is_some()
            || self.fin.is_some()
            || !self.sealed.is_empty()
            || !self.lig_buf.is_empty()
            || self.chars_live.is_some()
            || self.interp.is_some()
            || self.scanner.is_some()
            || !self.incs.is_empty()
            || !self.seeded.is_empty()
            || self.defer_hit.is_some()
            || self.mode_zero
            || self.fin_box.is_some()
            || self.tool_cut.is_some()
            || self.lt.sink.is_some()
        {
            return None;
        }
        if let Some(at) = self.ck_where_line() {
            return Some(At::Line(at));
        }
        // (anywhere else: in a group or a box, in a file, in a sibling's
        // text, in an alignment (its state in `Run::ck_run_state`); not
        // where a give-back would read a line a park left)
        if self.pull_orig.iter().any(Option::is_some)
            || self.pull_at.len() != self.pulled
            || self.pull_box.len() != self.pulled
            || crate::exec::obs_flag!("PHITEX_CK_LINE_ONLY")
        {
            return None;
        }
        // (`PHITEX_CK_NO=pulled:file:group:vmode:box:line`, observe: no
        // checkpoint anywhere in those places, to find which one a
        // difference comes from)
        if let Some(no) = crate::exec::obs_str!("PHITEX_CK_NO") {
            use crate::input::kind;
            let has = |w: &str| no.split(':').any(|x| x == w);
            if (has("pulled") && self.pulled != 0)
                || (has("file") && self.input.levels.iter().any(|l| l.kind == kind::FILE))
                || (has("line")
                    && self
                        .input
                        .levels
                        .iter()
                        .skip(1)
                        .any(|l| l.kind == kind::LINE))
                || (has("group") && self.nest.iter().any(|l| l.group != 0))
                || (has("box") && self.nest.iter().any(|l| l.owns))
                || (has("vmode") && self.nest.first().is_none_or(|l| l.mode != HMODE))
            {
                return None;
            }
        }
        Some(At::Anywhere)
    }

    /// [`Run::ck_where`] in a paragraph of the node's own text, its line
    /// at the bottom and above it only token lists (the tokens a command
    /// read ahead and put back, kept with the place): the text's segment
    /// the line read is, the line's place and state, its `\endlinechar`
    /// and the characters the run appended to it.
    fn ck_where_line(&self) -> Option<(usize, u32, u8, i64, Vec<u32>)> {
        let toks = |l: &crate::input::Level| l.kind == crate::input::kind::TOKS;
        // (a read-back's macro on the stack: only anywhere, whose input
        // carries its mark)
        if !self.input.levels[1..].iter().all(toks)
            || self.input.levels.iter().any(|l| l.rb)
            || self.pulled != 0
            || self.nest.len() != 1
            || self.nest[0].mode != HMODE
            || self.nest[0].group != 0
        {
            return None;
        }
        let top = *self.input.levels.first()?;
        if top.kind != crate::input::kind::LINE {
            return None;
        }
        let li = self.input.text_at.checked_sub(1)?;
        let Some(Seg::Line(seg)) = self.input.text.get(li) else {
            return None;
        };
        let line = self.input.line(top);
        if line.origin != seg.origin
            || line.buf.len() < seg.buf.len()
            || line.buf[..seg.buf.len()] != seg.buf[..]
            || (
                line.starts,
                line.ends,
                line.rescanned,
                line.chars,
                line.lineno,
            ) != (seg.starts, seg.ends, seg.rescanned, seg.chars, seg.lineno)
        {
            return None;
        }
        Some((
            li,
            u32::try_from(line.loc).ok()?,
            line.state,
            line.endch,
            line.buf[seg.buf.len()..].to_vec(),
        ))
    }

    /// A checkpoint here ([`Run::ck_where`]'s place).
    #[allow(clippy::too_many_lines)]
    fn ck_take(&mut self, at: At) -> R<()> {
        let v = self.v;
        let bytes0 = if crate::exec::obs_flag!("PHITEX_CK_SIZE") {
            v.payload_bytes()
        } else {
            0
        };
        // (the input anywhere: every level and the text from the segment
        // last reached, with what the run pulled, `Run::ck_input`; at its
        // line, only the token lists above it, bottom first: their types
        // and the tokens left)
        let (li, loc, state, endch, extra, ahead, whole_input, enc_cost) = match at {
            At::Line((li, loc, state, endch, extra)) => {
                let ahead: Vec<ValueId> = self.input.levels[1..]
                    .iter()
                    .filter(|l| l.a < l.b)
                    .map(|l| {
                        v.list(vec![
                            v.int(i64::from(l.ty)),
                            crate::val::toks_value(v, self.input.arena.get(l.rest())),
                        ])
                    })
                    .collect();
                (
                    i64::try_from(li).unwrap_or(-1),
                    loc,
                    state,
                    endch,
                    extra,
                    ahead,
                    ValueId::UNIT,
                    0,
                )
            }
            At::Anywhere => {
                let mut memo = self
                    .ck
                    .as_mut()
                    .map(|c| std::mem::take(&mut c.levels))
                    .unwrap_or_default();
                let (input, cost) = self.ck_input(&mut memo);
                if let Some(c) = self.ck.as_mut() {
                    c.levels = memo;
                }
                if crate::exec::obs_flag!("PHITEX_CK_SIZE") {
                    eprintln!("ck-part input {}", v.payload_bytes().saturating_sub(bytes0));
                }
                (-1, 0, 0, 0, Vec::new(), Vec::new(), input, cost)
            }
        };
        // (the next one after four times as many commands as this one's
        // parts are long, at least `every`: what checkpoints cost stays a
        // share of what the run does, however large its table, lists and
        // input; at once as many, tikzpeople -j1 made 518 MB of values
        // in 4213 checkpoints, 15% of its time)
        let spacing = enc_cost
            + self.st.base_len() as u64
            + self.nest.iter().map(|l| l.list.len() as u64).sum::<u64>();
        if let Some(c) = self.ck.as_mut() {
            c.next = self.cmds
                + ck_gap(
                    c.dense,
                    self.resumed_at.is_some(),
                    c.fresh.len(),
                    every(),
                    spacing,
                );
        }
        if crate::exec::obs_flag!("PHITEX_CK_SIZE") {
            eprintln!(
                "ck-size: node {} at command {}: input {enc_cost} touched {} items {} levels {} next {}",
                self.cx.node().raw(),
                self.cmds,
                self.st.touched_len(),
                self.nest.iter().map(|l| l.list.len()).sum::<usize>(),
                self.input.levels.len(),
                self.ck.as_ref().map_or(0, |c| c.next)
            );
        }
        if li < 0 && crate::exec::obs_flag!("PHITEX_CK_TRACE") {
            eprintln!(
                "ck: node {} at command {}: anywhere: levels {:?} text_at {} of {} pulled {} pull_at {:?} nest {:?}",
                self.cx.node().raw(),
                self.cmds,
                self.input
                    .levels
                    .iter()
                    .map(|l| (l.kind, l.ty))
                    .collect::<Vec<_>>(),
                self.input.text_at,
                self.input.text.len(),
                self.pulled,
                self.pull_at,
                self.nest
                    .iter()
                    .map(|l| (l.mode, l.group, l.owns))
                    .collect::<Vec<_>>()
            );
        }
        // (the table at the run's base level: what it defined and stepped;
        // none where a group of its own saved a value)
        // (the encodings made for this run's checkpoints before: most of
        // the table is as it was at the last one)
        let mut memo = self
            .ck
            .as_mut()
            .map(|c| std::mem::take(&mut c.memo))
            .unwrap_or_default();
        let base = self.st.base_of(v, &mut memo);
        let base = base.map(|b| b.to_value(v, &mut |l| self.loc_field_peek(l), &mut memo));
        if let Some(c) = self.ck.as_mut() {
            c.memo = memo;
        }
        let Some(base) = base else {
            if crate::exec::obs_flag!("PHITEX_CK_WHY") {
                self.ck_census("no base table".to_owned());
            }
            if crate::exec::obs_flag!("PHITEX_CK_TRACE") {
                eprintln!(
                    "ck: node {} at command {}: no base table ({})",
                    self.cx.node().raw(),
                    self.cmds,
                    self.st.base_why()
                );
            }
            return Ok(());
        };
        if crate::exec::obs_flag!("PHITEX_CK_SIZE") {
            eprintln!("ck-part base {}", v.payload_bytes().saturating_sub(bytes0));
        }
        let (cont, locals) = self.cont_encode_with(false, false)?;
        if crate::exec::obs_flag!("PHITEX_CK_SIZE") {
            eprintln!("ck-part cont {}", v.payload_bytes().saturating_sub(bytes0));
        }
        if !locals.is_empty() {
            if crate::exec::obs_flag!("PHITEX_CK_WHY") {
                self.ck_census("group-local bindings".to_owned());
            }
            if crate::exec::obs_flag!("PHITEX_CK_TRACE") {
                eprintln!(
                    "ck: node {} at command {}: {} group-local bindings",
                    self.cx.node().raw(),
                    self.cmds,
                    locals.len()
                );
            }
            return Ok(());
        }
        if crate::exec::obs_flag!("PHITEX_CK_TRACE") {
            eprintln!(
                "ck: node {} at command {}: taken at segment {li}, character {loc}",
                self.cx.node().raw(),
                self.cmds
            );
        }
        let int = |x: i64| v.int(x);
        let contribs = v.list(
            self.contribs
                .iter()
                .map(|&(f, x)| v.list(vec![f.0, x]))
                .collect(),
        );
        let lines = v.list(
            self.doc_lines
                .iter()
                .map(|&(n, k)| v.list(vec![n, v.int(k)]))
                .collect(),
        );
        let fread = v.list(
            self.fread
                .iter()
                .map(|(n, h)| v.list(vec![v.str(n), crate::val::int_id(v, h.cast_signed())]))
                .collect(),
        );
        let misc = v.list(vec![
            int(self.rt_made),
            int(self.rt_cells),
            int(self.rt_rels),
            int(i64::try_from(self.pdf_occ).unwrap_or(0)),
            int(i64::from(self.error_count)),
            int(i64::from(self.cand)),
            int(i64::from_ne_bytes(self.pdf_cmd.to_ne_bytes())),
            // (formerly the lines the run began: none kept, as in a
            // continuation, `Run::cont_encode`)
            int(0),
            int(i64::from_ne_bytes(self.exps.to_ne_bytes())),
        ]);
        let mut ys = vec![
            int(li),
            int(i64::from(loc)),
            int(i64::from(state)),
            int(endch),
            v.list(extra.iter().map(|&c| v.int(i64::from(c))).collect()),
            int(i64::from_ne_bytes(self.cmds.to_ne_bytes())),
            cont,
            contribs,
            lines,
            fread,
            misc,
            base,
            v.list(self.cand_marks.iter().map(|m| cand_value(v, m)).collect()),
            v.list(ahead),
        ];
        ys.extend(self.ck_more());
        ys.push(whole_input);
        let ck = v.list(ys);
        #[cfg(test)]
        TAKEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(self.effect != phitex_defuse::Effect::None);
        if crate::exec::obs_flag!("PHITEX_CK_SIZE") {
            eprintln!(
                "ck-bytes: node {} at command {}: {} bytes made",
                self.cx.node().raw(),
                self.cmds,
                v.payload_bytes().saturating_sub(bytes0)
            );
        }
        let unread = self.cx.unread_count();
        if let Some(c) = self.ck.as_mut() {
            c.fresh.push((c.taken.len(), unread));
            c.taken.push(ck);
        }
        // (the reads after it are those a cutoff here or at the next run
        // checks: what the run reads again from now on among them)
        self.cx.reads_begin_again();
        self.cut_off(ck)
    }

    /// The cutoff: at checkpoint `ck`, the node's last run on the same
    /// text had one at the same command in the same state (its count of
    /// reads aside), and its reads after it all hold: the rest of this
    /// run would make what that run made after it. The run ends
    /// ([`Halt::Same`]): that run's products stand
    /// ([`phitex_defuse::Ctx::goes_on_as_last`]), and its checkpoints
    /// after this one go with this run's, their counts of reads moved by
    /// the difference here.
    fn cut_off(&mut self, ck: ValueId) -> R<()> {
        if self.would_cut.is_some() || self.stop_at.is_some() {
            return Ok(());
        }
        let Some(last) = self.last_cks.as_ref() else {
            return Ok(());
        };
        let v = self.v;
        let a = list_of(v, ck);
        let Some(&at) = a.get(5) else { return Ok(()) };
        let Some((j, b)) = last
            .iter()
            .enumerate()
            .map(|(j, &x)| (j, list_of(v, x)))
            .find(|(_, b)| b.get(5) == Some(&at))
        else {
            return Ok(());
        };
        if a.len() != b.len() || (0..a.len()).any(|i| i != 14 && a[i] != b[i]) {
            return Ok(());
        }
        let Some(m) = b.get(14).and_then(|&x| usize::try_from(int_of(v, x)).ok()) else {
            return Ok(());
        };
        if !self.cx.last_reads_hold_from(m) {
            return Ok(());
        }
        if check() {
            self.would_cut = Some((self.cmds, m));
            return Ok(());
        }
        let made = i64::try_from(self.cx.reads_made()).unwrap_or(i64::MAX);
        let delta = made - i64::try_from(m).unwrap_or(0);
        let rest: Vec<ValueId> = last[j + 1..]
            .iter()
            .map(|&x| {
                let mut ys = list_of(v, x);
                if let Some(c) = ys.get_mut(14) {
                    *c = v.int(int_of(v, *c) + delta);
                }
                v.list(ys)
            })
            .collect();
        let Some(mut c) = self.ck.take() else {
            return Ok(());
        };
        c.taken.extend(rest);
        let mut xs = vec![v.list(self.ck_key())];
        xs.extend(c.taken);
        self.cx.park(v.list(xs));
        self.cx.goes_on_as_last(m);
        self.ops
            .cut_offs
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if crate::exec::obs_flag!("PHITEX_CK_RESUMED") {
            eprintln!(
                "ck: node {} cut off at command {}: its last run's products stand",
                self.cx.node().raw(),
                self.cmds
            );
        }
        Err(crate::run::Halt::Same)
    }

    /// What [`Run::ck_input`] encodes of token list level `i`, `l`, after
    /// its tokens: a macro's call, what an error's context shows of its
    /// level (`crate::context`; a body read in place is the level's own
    /// list, not encoded again), and its tokens put back in place
    /// (`crate::input::Unread`).
    fn ck_level_extras(
        &self,
        i: usize,
        l: &crate::input::Level,
        e: &mut Vec<ValueId>,
        cost: &mut u64,
        memo: &mut phitex_defuse::FxHashMap<(u64, u32, u32), ValueId>,
    ) {
        let v = self.v;
        let int = |x: i64| v.int(x);
        if let Some((c, lens)) = self.input.call_of(l) {
            // (a call's level is its own range of the arena: encoded once
            // while the arena stands, as the levels' lists)
            let key = (self.input.arena_id | 1 << 63, l.c, l.b);
            if let Some(&x) = memo.get(&key) {
                *cost += 1;
                e.push(x);
                return self.ck_unread_extra(i, l, e);
            }
            let in_place = c.mac.params.is_empty();
            *cost += (c.mac.params.len() + if in_place { 0 } else { c.mac.body.len() }) as u64;
            let x = v.list(vec![
                crate::val::toks_value(v, &[c.name]),
                crate::val::toks_value(v, &c.mac.params),
                if in_place {
                    int(0)
                } else {
                    crate::val::toks_value(v, &c.mac.body)
                },
                int(i64::from(c.mac.flags)),
                v.list(lens.iter().map(|&n| int(i64::from(n))).collect()),
            ]);
            memo.insert(key, x);
            e.push(x);
        } else if let Some(name) = crate::input::Input::name_of(l) {
            e.push(v.list(vec![crate::val::toks_value(v, &[name])]));
        }
        self.ck_unread_extra(i, l, e);
    }

    /// [`Run::ck_level_extras`]'s tokens put back in place.
    fn ck_unread_extra(&self, i: usize, l: &crate::input::Level, e: &mut Vec<ValueId>) {
        let v = self.v;
        let int = |x: i64| v.int(x);
        // (only while some are still to be read again: one read again, or
        // a record its level's place has left, is no state of the input)
        if let Some(u) = self
            .input
            .unread
            .filter(|u| u.of(i, l) && u.from <= l.a && l.a < u.to)
        {
            if e.len() == 5 {
                e.push(int(0));
            }
            e.push(v.list(vec![
                int(i64::from(u.from - l.c)),
                int(i64::from(u.to - l.c)),
            ]));
        }
    }

    /// The input as a checkpoint taken anywhere holds it
    /// ([`Run::ck_input_restore`]), and how long it is: every level,
    /// bottom first (a token list with all its tokens and how many were
    /// read, a line, a file's rest, each exactly); the text from the
    /// segment last reached (or the last sibling pulled's first, which a
    /// give-back reads) on, with its length and where the run stands in
    /// it; the siblings pulled and where their texts begin.
    fn ck_input(
        &self,
        memo: &mut phitex_defuse::FxHashMap<(u64, u32, u32), ValueId>,
    ) -> (ValueId, u64) {
        use crate::input::kind;
        let v = self.v;
        let int = |x: i64| v.int(x);
        let usz = |x: usize| v.int(i64::try_from(x).unwrap_or(i64::MAX));
        let mut cost = 0u64;
        let levels: Vec<ValueId> = self
            .input
            .levels
            .iter()
            .enumerate()
            .map(|(i, l)| match l.kind {
                kind::TOKS => {
                    let r = crate::input::Range {
                        start: l.c,
                        end: l.b,
                    };
                    // (a list encoded at an earlier checkpoint of the run,
                    // its range in the same arena, is the same list)
                    let key = (self.input.arena_id, l.c, l.b);
                    let t = if let Some(&t) = memo.get(&key) {
                        cost += 1;
                        t
                    } else {
                        let toks = self.input.arena.get(r);
                        cost += toks.len() as u64;
                        let t = crate::val::toks_value(v, toks);
                        memo.insert(key, t);
                        t
                    };
                    let mut e = vec![
                        int(0),
                        int(i64::from(l.ty)),
                        t,
                        int(i64::from(l.a - l.c)),
                        int(i64::from(l.rb)),
                    ];
                    self.ck_level_extras(i, l, &mut e, &mut cost, memo);
                    v.list(e)
                }
                kind::LINE => {
                    cost += 1;
                    v.list(vec![int(1), self.input.line(*l).to_value(v)])
                }
                _ => {
                    cost += 1;
                    v.list(vec![int(2), self.input.file(*l).to_value(v)])
                }
            })
            .collect();
        let text = &self.input.text;
        let mut from = self.input.text_at.saturating_sub(1).min(text.len());
        if let Some(&Some(p)) = self.pull_at.last() {
            from = from.min(p);
        }
        let segs: Vec<ValueId> = text[from..]
            .iter()
            .map(|s| match s {
                Seg::Toks(t) => {
                    cost += t.len() as u64;
                    v.list(vec![int(0), crate::val::toks_value(v, t)])
                }
                Seg::Line(l) => {
                    cost += 1;
                    v.list(vec![int(1), l.to_value(v)])
                }
                Seg::File(f) => {
                    cost += 1;
                    v.list(vec![int(2), f.to_value(v)])
                }
            })
            .collect();
        let opt = |x: Option<usize>| x.map_or(int(-1), usz);
        let pull = v.list(vec![
            usz(self.pulled),
            v.list(self.pull_at.iter().map(|&p| opt(p)).collect()),
            v.list(self.pull_box.iter().map(|&b| int(i64::from(b))).collect()),
            opt(self.pull_lev),
            self.cut_state.map_or(int(-1), |s| int(i64::from(s))),
        ]);
        let x = v.list(vec![
            v.list(levels),
            v.list(vec![
                usz(text.len()),
                usz(self.input.text_at),
                usz(from),
                v.list(segs),
            ]),
            pull,
        ]);
        (x, cost)
    }

    /// [`Run::ck_input`]'s input decoded ([`Run::ck_input_put`] puts it
    /// back): the text's segments before those it holds are empty token
    /// lists (read already, never read again).
    #[allow(clippy::type_complexity)]
    fn ck_input_of(&self, x: ValueId) -> Option<CkInput> {
        use crate::input::kind;
        let v = self.v;
        let n = |x: ValueId| usize::try_from(int_of(v, x)).ok();
        let [levels, text, pull] = list_of(v, x)[..] else {
            return None;
        };
        let [len, at, from, segs] = list_of(v, text)[..] else {
            return None;
        };
        let (Some(len), Some(at), Some(from)) = (n(len), n(at), n(from)) else {
            return None;
        };
        let mut input = crate::input::Input::new();
        input.text = vec![Seg::Toks(Vec::new()); from];
        for e in list_of(v, segs) {
            let seg = match list_of(v, e)[..] {
                [k, t] if int_of(v, k) == 0 => Seg::Toks(crate::val::toks_of(v, Some(t))),
                [k, l] if int_of(v, k) == 1 => Seg::Line(crate::input::Line::of_value(v, l)?),
                [_, f] => Seg::File(crate::run::FileRest::of_value(v, f)?),
                _ => return None,
            };
            input.text.push(seg);
        }
        if input.text.len() != len {
            return None;
        }
        input.text_at = at;
        for e in list_of(v, levels) {
            match list_of(v, e)[..] {
                [k, ty, t, read, rb, ..] if int_of(v, k) == i64::from(kind::TOKS) => {
                    let toks = crate::val::toks_of(v, Some(t));
                    let call = ck_call_of(v, e, &toks, &mut input);
                    input.push_toks_at(
                        &toks,
                        n(read).unwrap_or(0),
                        u8::try_from(int_of(v, ty)).unwrap_or(0),
                    );
                    if let Some(l) = input.levels.last_mut() {
                        l.rb = int_of(v, rb) != 0;
                    }
                    ck_unread_put(v, e, &mut input, call);
                }
                [k, l] if int_of(v, k) == i64::from(kind::LINE) => {
                    input.push_line(crate::input::Line::of_value(v, l)?);
                }
                [_, f] => {
                    input.push_file(crate::run::FileRest::of_value(v, f)?);
                }
                _ => return None,
            }
        }
        let [pulled, pull_at, pull_box, pull_lev, cut_state] = list_of(v, pull)[..] else {
            return None;
        };
        let opt = |x: ValueId| usize::try_from(int_of(v, x)).ok();
        Some(CkInput {
            input,
            pulled: n(pulled)?,
            pull_at: list_of(v, pull_at).into_iter().map(opt).collect(),
            pull_box: list_of(v, pull_box)
                .into_iter()
                .map(|b| int_of(v, b) != 0)
                .collect(),
            pull_lev: opt(pull_lev),
            cut_state: u8::try_from(int_of(v, cut_state)).ok(),
        })
    }

    /// [`Run::ck_input_of`]'s input as the run's.
    fn ck_input_put(&mut self, c: CkInput) {
        self.input = c.input;
        self.pulled = c.pulled;
        self.pull_at = c.pull_at;
        self.pull_box = c.pull_box;
        self.pull_orig = vec![None; c.pulled];
        self.pull_lev = c.pull_lev;
        self.cut_state = c.cut_state;
    }

    /// A checkpoint's parts from its 15th on: the count of the reads
    /// made before it, the nest's lines and places, the page-layer
    /// registers held, the candidate a second pass ends at, the rest of
    /// the run's state ([`Run::ck_run_state`]).
    fn ck_more(&mut self) -> Vec<ValueId> {
        let v = self.v;
        let int = |x: i64| v.int(x);
        vec![
            int(i64::try_from(self.cx.reads_made()).unwrap_or(i64::MAX)),
            // (the nest's lines and places: a paragraph's box reports name
            // the line it began on, §816)
            v.list(
                self.nest
                    .iter()
                    .map(|l| {
                        v.list(vec![
                            line_mark_value(v, l.mode_line),
                            place_value(v, l.mode_place.as_ref()),
                        ])
                    })
                    .collect(),
            ),
            // (the page-layer registers whose next read is ordered after a
            // page firing at a candidate passed, `Run::page_regs_held`:
            // those read since are not)
            v.list(
                self.page_regs_held
                    .clone()
                    .into_iter()
                    .map(|l| self.loc_field(l).0)
                    .collect(),
            ),
            // (the candidate a second pass ends at: a cutoff is only to a
            // checkpoint of a run that ends where this one does)
            int(self.stop_at.map_or(-1, i64::from)),
            self.ck_run_state(),
        ]
    }

    /// What else of the run a checkpoint carries: the insertion classes
    /// put on the list since the last candidate, the registers' first
    /// definitions and first values read, XeTeX's input modes, scripts
    /// and character classes, the files written as text, where the lines
    /// read went, the scope kind read ([`Run::ck_run_restore`]).
    fn ck_run_state(&mut self) -> ValueId {
        let v = self.v;
        let int = |x: i64| v.int(x);
        let mut first: Vec<(crate::state::Loc, u32)> =
            self.reg_first_def.iter().map(|(&l, &k)| (l, k)).collect();
        first.sort_unstable();
        let mut based: Vec<(crate::state::Loc, i32)> =
            self.reg_loaded_base.iter().map(|(&l, &k)| (l, k)).collect();
        based.sort_unstable();
        let first: Vec<ValueId> = first
            .into_iter()
            .map(|(l, k)| v.list(vec![self.loc_field(l).0, int(i64::from(k))]))
            .collect();
        let based: Vec<ValueId> = based
            .into_iter()
            .map(|(l, k)| v.list(vec![self.loc_field(l).0, int(i64::from(k))]))
            .collect();
        let lt = self.lt;
        v.list(vec![
            v.list(
                self.ins_pending
                    .iter()
                    .map(|&n| int(i64::from(n)))
                    .collect(),
            ),
            v.list(first),
            v.list(based),
            v.list(
                self.xenc
                    .iter()
                    .map(|&(n, m)| v.list(vec![n, int(m)]))
                    .collect(),
            ),
            v.list(
                self.xdirs
                    .iter()
                    .map(|&(f, d)| {
                        v.list(vec![int(f), d.map_or(ValueId::UNIT, |d| int(i64::from(d)))])
                    })
                    .collect(),
            ),
            int(i64::from(self.ic_prev)),
            int(i64::from(self.ic_space)),
            v.list(self.text_written.iter().map(|t| v.str(t)).collect()),
            int(i64::from(lt.hit) | i64::from(lt.weak) << 1 | i64::from(lt.strong) << 2),
            // (the scope kind read, if it was: whether the run stands at
            // the top level decides which of its definitions it publishes,
            // `Run::publication`; read before the checkpoint, it is not
            // read again)
            match self.scope_kind_memo {
                None => v.list(Vec::new()),
                Some(k) => v.list(vec![v.list(k.into_iter().collect())]),
            },
            // (the streams an `\immediate\closeout` closed: closed for
            // the run's later writes, `Run::closed`)
            v.list(self.closed.iter().map(|&k| int(i64::from(k))).collect()),
            // (the alignments open: `align_state` as the run has it, if
            // read, and how many are open, both run-local; the rest of an
            // alignment is in the nest (its levels' extras, the rows so
            // far), the table (its preamble) and the input (its templates))
            v.list(self.astate.into_iter().map(int).collect()),
            int(self.aopen),
            self.ck_read_back(),
            // (the children made so far, placed as the run ends: a run
            // that goes on from here places them too)
            v.list(
                self.kids
                    .iter()
                    .map(|(op, args)| {
                        v.list(vec![
                            int(self.kid_code(*op).unwrap_or(-1)),
                            v.list(args.clone()),
                        ])
                    })
                    .collect(),
            ),
        ])
    }

    /// A child's operation as a checkpoint has it: those a run makes
    /// (a group's definitions put back, a tool, a page's PDF side).
    fn kid_code(&self, op: phitex_defuse::OpId) -> Option<i64> {
        [self.ops.put, self.ops.toolnode, self.ops.pdfpage]
            .iter()
            .position(|&o| o == op)
            .and_then(|k| i64::try_from(k).ok())
    }

    /// A read-back's state as the run has it (its macro's level is marked
    /// on the input, `Level::rb`): whether the run read a file back and
    /// is a read-back point, the locations whose value there it noted, a
    /// pending point with the values kept for it, and the files looked
    /// for in vain after the point ([`Run::note_pre`],
    /// [`Run::ck_read_back_restore`]).
    fn ck_read_back(&mut self) -> ValueId {
        let v = self.v;
        let int = |x: i64| v.int(x);
        let mut noted: Vec<crate::state::Loc> = self.pre_noted.iter().copied().collect();
        noted.sort_unstable();
        let noted: Vec<ValueId> = noted.into_iter().map(|l| self.loc_field(l).0).collect();
        let pre: Vec<ValueId> = self
            .pending_pre
            .clone()
            .into_iter()
            .map(|(l, x)| v.list(vec![self.loc_field(l).0, x]))
            .collect();
        v.list(vec![
            int(i64::from(self.read_back_at.is_some())),
            int(i64::from(self.read_back_done)),
            int(i64::from(self.read_point_here)),
            v.list(noted),
            v.list(self.read_point_pending.iter().map(|p| v.str(p)).collect()),
            v.list(pre),
            v.list(self.vain_after_point.iter().map(|p| v.str(p)).collect()),
        ])
    }

    /// [`Run::ck_read_back`]'s state put back.
    fn ck_read_back_restore(&mut self, x: ValueId) {
        let v = self.v;
        let [at, done, here, noted, pending, pre, vain] = list_of(v, x)[..] else {
            return;
        };
        // (its level, if still on the stack, came back marked with the
        // input; the level itself is not needed, only that there was one)
        self.read_back_at = (int_of(v, at) != 0).then(crate::input::Level::default);
        self.read_back_ix.set(None);
        self.read_back_done = int_of(v, done) != 0;
        self.read_point_here = int_of(v, here) != 0;
        self.pre_noted = list_of(v, noted)
            .into_iter()
            .map(|f| self.loc_of(Field(f)))
            .collect();
        self.read_point_pending = list_of(v, pending)
            .first()
            .map(|&p| crate::val::str_of(v, p));
        self.pending_pre.clear();
        self.pending_locs.clear();
        for e in list_of(v, pre) {
            if let [f, y] = list_of(v, e)[..] {
                let l = self.loc_of(Field(f));
                self.pending_pre.push((l, y));
                self.pending_locs.insert(l);
            }
        }
        self.vain_after_point = list_of(v, vain)
            .iter()
            .map(|&p| crate::val::str_of(v, p))
            .collect();
    }

    /// [`Run::ck_run_state`]'s parts put back.
    fn ck_run_restore(&mut self, x: ValueId) {
        let v = self.v;
        let xs = list_of(v, x);
        let [
            pend,
            first,
            based,
            xenc,
            xdirs,
            icp,
            ics,
            written,
            lt,
            skind,
            closed,
            astate,
            aopen,
            read_back,
            kids,
        ] = xs[..]
        else {
            return;
        };
        let ops = [self.ops.put, self.ops.toolnode, self.ops.pdfpage];
        self.kids = list_of(v, kids)
            .into_iter()
            .filter_map(|k| match list_of(v, k)[..] {
                [c, args] => usize::try_from(int_of(v, c))
                    .ok()
                    .and_then(|c| ops.get(c).copied())
                    .map(|op| (op, list_of(v, args))),
                _ => None,
            })
            .collect();
        self.ck_read_back_restore(read_back);
        self.astate = list_of(v, astate).first().map(|&x| int_of(v, x));
        self.aopen = int_of(v, aopen);
        self.closed = list_of(v, closed)
            .iter()
            .filter_map(|&k| i32::try_from(int_of(v, k)).ok())
            .collect();
        let pair = |e: ValueId| match list_of(v, e)[..] {
            [a, b] => Some((a, b)),
            _ => None,
        };
        self.ins_pending = list_of(v, pend)
            .iter()
            .filter_map(|&n| u16::try_from(int_of(v, n)).ok())
            .collect();
        self.reg_first_def.clear();
        for (f, k) in list_of(v, first).into_iter().filter_map(pair) {
            let l = self.loc_of(Field(f));
            self.reg_first_def
                .insert(l, u32::try_from(int_of(v, k)).unwrap_or(0));
        }
        self.reg_loaded_base.clear();
        for (f, k) in list_of(v, based).into_iter().filter_map(pair) {
            let l = self.loc_of(Field(f));
            self.reg_loaded_base
                .insert(l, i32::try_from(int_of(v, k)).unwrap_or(0));
        }
        self.xenc = list_of(v, xenc)
            .into_iter()
            .filter_map(pair)
            .map(|(n, m)| (n, int_of(v, m)))
            .collect();
        self.xdirs = list_of(v, xdirs)
            .into_iter()
            .filter_map(pair)
            .map(|(f, d)| {
                (
                    int_of(v, f),
                    (d != ValueId::UNIT).then(|| u32::try_from(int_of(v, d)).unwrap_or(0)),
                )
            })
            .collect();
        self.ic_prev = i32::try_from(int_of(v, icp)).unwrap_or(0);
        self.ic_space = i32::try_from(int_of(v, ics)).unwrap_or(0);
        self.text_written = list_of(v, written)
            .iter()
            .map(|&t| crate::val::str_of(v, t))
            .collect();
        let b = int_of(v, lt);
        self.scope_kind_memo = list_of(v, skind)
            .first()
            .map(|&k| list_of(v, k).first().copied());
        self.lt.hit = b & 1 != 0;
        self.lt.weak = b & 2 != 0;
        self.lt.strong = b & 4 != 0;
    }

    /// The checkpoints taken, with the run's record: the node's
    /// arguments they were taken in first.
    pub(crate) fn ck_park(&mut self) {
        let Some(c) = self.ck.take() else {
            return;
        };
        if crate::exec::obs_flag!("PHITEX_CK_TRACE") {
            eprintln!(
                "ck: node {} parks {} checkpoints",
                self.cx.node().raw(),
                c.taken.len()
            );
        }
        if crate::exec::obs_flag!("PHITEX_CK_WHY") && !c.why.is_empty() {
            eprintln!(
                "ck-why: node {} {} commands, {} taken, refused: {:?}",
                self.cx.node().raw(),
                self.cmds,
                c.taken.len(),
                c.why
            );
        }
        if c.taken.is_empty() {
            return;
        }
        let v = self.v;
        let mut taken = c.taken;
        // (each count of reads before a checkpoint as the record has
        // them: `unread` may have taken some out since)
        for &(i, at) in &c.fresh {
            let mut ys = list_of(v, taken[i]);
            if let Some(m) = ys.get_mut(14)
                && let Ok(made) = usize::try_from(int_of(v, *m))
            {
                let k = self.cx.reads_kept_of(made, at);
                if k != made {
                    *m = v.int(i64::try_from(k).unwrap_or(i64::MAX));
                    taken[i] = v.list(ys);
                }
            }
        }
        let mut xs = vec![v.list(self.ck_key())];
        xs.extend(taken);
        self.cx.park(v.list(xs));
    }

    /// An edited node's run goes on from its last run's last checkpoint
    /// the edit left in place, if there is one: true then. Its input (the
    /// node's new text, appended) is set to the checkpoint's place.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn resume_edit(&mut self) -> R<bool> {
        if self.no_resume || !self.ck_op() {
            return Ok(false);
        }
        // (an edit of the node's text, or another wake: a read of the last
        // run that moved, with the text the same; then the checkpoint must
        // stand before every read that moved: the reads made before it
        // all hold, `Ctx::last_reads_hold`)
        let (p, edit) = match self.cx.parked_before_edit() {
            Some(p) => (p, true),
            None => match self.cx.parked_state() {
                Some(p) if self.cx.woken_by().is_some() => (p, false),
                _ => {
                    if crate::exec::obs_flag!("PHITEX_CK_TRACE") {
                        eprintln!(
                            "ck: node {} woken by {:?}: no checkpoints",
                            self.cx.node().raw(),
                            self.cx.woken_by()
                        );
                    }
                    return Ok(false);
                }
            },
        };
        let v = self.v;
        let xs = list_of(v, p);
        let Some((&old, cks)) = xs.split_first() else {
            return Ok(false);
        };
        let old_args = list_of(v, old);
        let args = self.ck_key();
        if old_args.len() != args.len()
            || old_args.get(1..) != args.get(1..)
            || (!edit && old_args != args)
        {
            if crate::exec::obs_flag!("PHITEX_CK_TRACE") {
                eprintln!(
                    "ck: node {} woken by {:?}: its arguments moved ({} {:?})",
                    self.cx.node().raw(),
                    self.cx.woken_by(),
                    old_args.len(),
                    (0..old_args.len().min(args.len()))
                        .filter(|&i| old_args[i] != args[i])
                        .collect::<Vec<_>>()
                );
            }
            return Ok(false);
        }
        let old_segs = crate::input::joined(crate::run::decode_segs(v, &old_args));
        let new_segs = self.input.text.clone();
        let shared = old_segs
            .iter()
            .zip(&new_segs)
            .take_while(|(a, b)| a.same(b))
            .count();
        let mut found = None;
        for (k, &ck) in cks.iter().enumerate().rev() {
            let c = list_of(v, ck);
            if c.len() < 14 {
                continue;
            }
            // (a second pass ends at its candidate: never from past it)
            if let Some(k) = self.stop_at {
                let cand = list_of(v, c[10]).get(5).map_or(i64::MAX, |&x| int_of(v, x));
                if cand > i64::from(k) {
                    continue;
                }
            }
            // (one a second pass took, ending at its candidate, is not
            // a whole run's, nor the other way round: a run resumed from
            // a second pass's checkpoint went on in that pass's state,
            // amsldoc -j1 node 1385)
            if c.get(17).map(|&x| int_of(v, x)) != Some(self.stop_at.map_or(-1, i64::from)) {
                continue;
            }
            if !edit {
                // (the same text: the checkpoint stands if the reads made
                // before it hold)
                let made = c.get(14).map_or(usize::MAX, |&x| {
                    usize::try_from(int_of(v, x)).unwrap_or(usize::MAX)
                });
                if made != usize::MAX && self.cx.last_reads_hold(made) {
                    if check() || crate::exec::obs_flag!("PHITEX_CK_RESUMED") {
                        eprintln!(
                            "ck-change: node {} woken by {:?} goes on from checkpoint {k} after {made} reads at command {}{}: {:?}",
                            self.cx.node().raw(),
                            self.cx.woken_by(),
                            int_of(v, c[5]),
                            if int_of(v, c[0]) < 0 {
                                " (anywhere)"
                            } else {
                                ""
                            },
                            self.cx
                                .args()
                                .first()
                                .map(|&x| v.render(x).chars().take(160).collect::<String>())
                                .unwrap_or_default()
                        );
                    }
                    found = Some((k, c));
                    break;
                }
                continue;
            }
            let Ok(li) = usize::try_from(int_of(v, c[0])) else {
                continue;
            };
            let loc = usize::try_from(int_of(v, c[1])).unwrap_or(usize::MAX);
            if li > shared {
                continue;
            }
            let (Some(Seg::Line(a)), Some(Seg::Line(b))) = (old_segs.get(li), new_segs.get(li))
            else {
                continue;
            };
            if loc > a.buf.len()
                || loc > b.buf.len()
                || a.buf[..loc] != b.buf[..loc]
                || (
                    a.origin,
                    a.starts,
                    a.state,
                    a.endch,
                    a.rescanned,
                    a.chars,
                    a.lineno,
                ) != (
                    b.origin,
                    b.starts,
                    b.state,
                    b.endch,
                    b.rescanned,
                    b.chars,
                    b.lineno,
                )
                || (a.ends != b.ends && loc >= a.buf.len().min(b.buf.len()))
            {
                continue;
            }
            found = Some((k, c));
            break;
        }
        let Some((k, c)) = found else {
            if crate::exec::obs_flag!("PHITEX_CK_TRACE") && !edit {
                let made = cks
                    .first()
                    .and_then(|&x| list_of(v, x).get(14).copied())
                    .map_or(0, |x| usize::try_from(int_of(v, x)).unwrap_or(0));
                eprintln!(
                    "ck: node {} woken by {:?}: no checkpoint of {} stands: {}",
                    self.cx.node().raw(),
                    self.cx.woken_by(),
                    cks.len(),
                    self.cx
                        .first_moved_read(made)
                        .unwrap_or_else(|| "(its reads hold)".to_owned())
                );
            }
            return Ok(false);
        };
        let li = usize::try_from(int_of(v, c[0])).unwrap_or(0);
        let loc = usize::try_from(int_of(v, c[1])).unwrap_or(0);
        // (one taken anywhere: its input whole; at a line of the text,
        // that line in the new text)
        let anywhere = match c.get(19) {
            Some(&x) if int_of(v, c[0]) < 0 => match self.ck_input_of(x) {
                Some(i) => Some(i),
                None => return Ok(false),
            },
            _ => None,
        };
        let line = match new_segs.get(li) {
            Some(Seg::Line(l)) if anywhere.is_none() => Some(l.clone()),
            _ if anywhere.is_some() => None,
            _ => return Ok(false),
        };
        let Some(base) = crate::state::Base::of_value(v, c[11], &mut |f| self.loc_of(f)) else {
            return Ok(false);
        };
        // (a resume's segments' file lines, as its continuation gave
        // them: the checkpoint's continuation holds those of its rest)
        let lines = std::mem::take(&mut self.cont_lines);
        self.cont_load(c[6], ValueId::UNIT)?;
        self.st.with_base(v, base);
        if let Some(&ls) = c.get(15) {
            for (l, &x) in self.nest.iter_mut().zip(&list_of(v, ls)) {
                if let [m, p] = list_of(v, x)[..] {
                    l.mode_line = line_mark_of(v, m);
                    l.mode_place = place_of(v, p);
                }
            }
        }
        if let Some(name) = crate::exec::obs_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(name));
            eprintln!(
                "cont {} resumed at command {}: {name}: {}",
                self.cx.node().raw(),
                int_of(v, c[5]),
                self.st.debug_marks(l)
            );
        }
        if let Some(i) = anywhere {
            self.ck_input_put(i);
        } else if let Some(mut line) = line {
            // (the input: the new text, its segment `li` the line on top
            // at the checkpoint's place)
            line.loc = loc;
            line.state = u8::try_from(int_of(v, c[2])).unwrap_or(line.state);
            line.endch = int_of(v, c[3]);
            line.buf.extend(
                list_of(v, c[4])
                    .iter()
                    .map(|&x| u32::try_from(int_of(v, x)).unwrap_or(0)),
            );
            self.input = crate::input::Input::new();
            self.input.text = new_segs;
            self.input.text_at = li + 1;
            self.input.push_line(line);
            for e in list_of(v, c[13]) {
                if let [ty, t] = list_of(v, e)[..] {
                    let toks = crate::val::toks_of(v, Some(t));
                    let r = self.input.arena.alloc(&toks);
                    self.input
                        .push_toks(r, u8::try_from(int_of(v, ty)).unwrap_or(0));
                }
            }
        }
        self.cont_lines = lines;
        self.ck_restore(&c);
        if let Some(&x) = c.get(18) {
            self.ck_run_restore(x);
        }
        self.cand_marks = list_of(v, c[12])
            .iter()
            .filter_map(|&m| cand_of(v, m))
            .collect();
        self.page_regs_held.clear();
        match c.get(16) {
            Some(&hs) => {
                for f in list_of(v, hs) {
                    let l = self.loc_of(Field(f));
                    self.page_regs_held.push(l);
                }
            }
            None => self.note_page_regs_held(),
        }
        self.muts += 1;
        self.resumed_at = Some(self.cmds);
        if check() {
            self.resumed_ck = Some(cks[k]);
        }
        self.ops
            .resumed_edits
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // (the last run's reads made before the checkpoint stand with
        // this run's, the rest dropped; a checkpoint with no count of
        // them, every read kept)
        match c.get(14).map(|&x| usize::try_from(int_of(v, x))) {
            Some(Ok(made)) => self.cx.goes_on_after_reads(made),
            _ => self.cx.goes_on_from_edit(),
        }
        // (the checkpoints up to this one stand for the next run)
        self.ck = Some(Box::new(Checkpoints {
            next: self.cmds + every(),
            dense: matches!(self.cx.woken_by(), Some(phitex_defuse::Cause::Edited)),
            taken: cks[..=k].to_vec(),
            fresh: Vec::new(),
            why: Vec::new(),
            memo: crate::state::CkMemo::default(),
            levels: phitex_defuse::FxHashMap::default(),
        }));
        Ok(true)
    }

    /// What a checkpoint holds besides the continuation: the command
    /// count, the contributions, the lines and files read, the counters.
    fn ck_restore(&mut self, c: &[ValueId]) {
        let v = self.v;
        self.cmds = u64::from_ne_bytes(int_of(v, c[5]).to_ne_bytes());
        self.contribs = list_of(v, c[7])
            .iter()
            .filter_map(|&e| match list_of(v, e)[..] {
                [f, x] => Some((Field(f), x)),
                _ => None,
            })
            .collect();
        self.doc_lines = list_of(v, c[8])
            .iter()
            .filter_map(|&e| match list_of(v, e)[..] {
                [n, k] => Some((n, int_of(v, k))),
                _ => None,
            })
            .collect();
        self.fread = list_of(v, c[9])
            .iter()
            .filter_map(|&e| match list_of(v, e)[..] {
                [n, h] => Some((crate::val::str_of(v, n), int_of(v, h).cast_unsigned())),
                _ => None,
            })
            .collect();
        let m: Vec<i64> = list_of(v, c[10]).iter().map(|&x| int_of(v, x)).collect();
        if let [
            rt_made,
            rt_cells,
            rt_rels,
            pdf_occ,
            errors,
            cand,
            pdf_cmd,
            _lines,
            exps,
        ] = m[..]
        {
            self.rt_made = rt_made;
            self.rt_cells = rt_cells;
            self.rt_rels = rt_rels;
            self.pdf_occ = usize::try_from(pdf_occ).unwrap_or(0);
            self.error_count = u32::try_from(errors).unwrap_or(0);
            self.cand = u32::try_from(cand).unwrap_or(0);
            self.pdf_cmd = u64::from_ne_bytes(pdf_cmd.to_ne_bytes());
            self.exps = u64::from_ne_bytes(exps.to_ne_bytes());
        }
    }

    /// What the run produced, as one string ([`PHITEX_RESUME_CHECK`]):
    /// its definitions, contributions, children, the lines it read, its
    /// scope effect, conditionals and the text it leaves.
    pub(crate) fn product_signature(&mut self) -> String {
        let v = self.v;
        let mut pubd: Vec<String> = self
            .publication()
            .into_iter()
            .map(|(f, x, loc)| format!("{}{loc:?}={x:?}", crate::val::field_name(v, f)))
            .collect();
        pubd.sort();
        let contribs: Vec<String> = self
            .contribs
            .iter()
            .map(|(f, x)| format!("{}+={}", crate::val::field_name(v, *f), v.render(*x)))
            .collect();
        let kids: Vec<String> = self
            .kids
            .iter()
            .map(|(o, a)| format!("{o:?}{:?}", a.iter().map(|x| x.raw()).collect::<Vec<_>>()))
            .collect();
        let lines: Vec<String> = self
            .doc_lines
            .iter()
            .map(|(n, k)| format!("{}:{k}", n.raw()))
            .collect();
        let left: Vec<Vec<u64>> = self
            .leftover()
            .iter()
            .map(|a| a.iter().map(|x| x.raw()).collect())
            .collect();
        format!(
            "pub {pubd:?}\ncontribs {contribs:?}\nkids {kids:?}\nlines {lines:?}\nfread {:?}\neffect {:?}\nconds {:?}\nleft {left:?}\nerrors {}",
            self.fread, self.effect, self.conds, self.error_count
        )
    }
}

impl Run<'_, '_> {
    /// `PHITEX_RESUME_CHECK`: the checkpoint `ck` a resumed run went on
    /// from against this whole run's at the same command: the parts that
    /// differ, by path (a base slot by its field), as one line.
    pub(crate) fn ck_compare(&self, ck: ValueId) -> String {
        let v = self.v;
        let a = list_of(v, ck);
        let at = a.get(5).map(|&x| int_of(v, x));
        let taken = self
            .ck
            .as_ref()
            .map(|c| c.taken.clone())
            .unwrap_or_default();
        let Some(b) = taken
            .iter()
            .map(|&x| list_of(v, x))
            .find(|b| b.get(5).map(|&x| int_of(v, x)) == at)
        else {
            let cmds: Vec<i64> = taken
                .iter()
                .filter_map(|&x| list_of(v, x).get(5).map(|&y| int_of(v, y)))
                .collect();
            return format!("no checkpoint of the whole run at command {at:?} (its: {cmds:?})");
        };
        if a.len() != b.len() {
            return format!("lengths {} {}", a.len(), b.len());
        }
        let mut out = Vec::new();
        for i in 0..a.len() {
            if a[i] == b[i] {
                continue;
            }
            if i == 11 {
                // (the base table: its slots by field)
                let (sa, sb) = (list_of(v, a[i]), list_of(v, b[i]));
                for j in 0..sa.len().min(sb.len()) {
                    if sa[j] == sb[j] {
                        continue;
                    }
                    if j == 0 {
                        let by = |x: ValueId| -> std::collections::BTreeMap<String, ValueId> {
                            list_of(v, x)
                                .iter()
                                .filter_map(|&s| {
                                    let f = *list_of(v, s).first()?;
                                    Some((crate::val::field_name(v, Field(f)), s))
                                })
                                .collect()
                        };
                        let (ma, mb) = (by(sa[0]), by(sb[0]));
                        for (f, x) in &ma {
                            match mb.get(f) {
                                Some(y) if y == x => {}
                                Some(y) => out.push(format!(
                                    "base {f}: resumed {} whole {}",
                                    short(v, *x),
                                    short(v, *y)
                                )),
                                None => out
                                    .push(format!("base {f}: resumed {} whole none", short(v, *x))),
                            }
                        }
                        for (f, y) in &mb {
                            if !ma.contains_key(f) {
                                out.push(format!("base {f}: resumed none whole {}", short(v, *y)));
                            }
                        }
                    } else {
                        out.push(format!(
                            "base[{j}]: resumed {} whole {}",
                            short(v, sa[j]),
                            short(v, sb[j])
                        ));
                    }
                }
                continue;
            }
            out.push(diff_path(v, &format!("[{i}]"), a[i], b[i]));
        }
        if out.is_empty() {
            "same".to_owned()
        } else {
            out.truncate(24);
            out.join(" | ")
        }
    }
}

/// A nest level's `mode_line` as a value ([`line_mark_of`]).
fn line_mark_value(v: &phitex_defuse::Values, m: Option<crate::run::LineMark>) -> ValueId {
    let Some((n, mine, name, own)) = m else {
        return ValueId::UNIT;
    };
    let opt = |x: Option<ValueId>| v.list(x.into_iter().collect());
    v.list(vec![
        opt(n.map(|n| v.int(n))),
        v.int(mine),
        opt(name),
        opt(own.map(|(a, b)| {
            v.list(vec![
                v.int(i64::from_ne_bytes(a.to_ne_bytes())),
                v.int(i64::from(b)),
            ])
        })),
    ])
}

/// [`line_mark_value`]'s mark.
fn line_mark_of(v: &phitex_defuse::Values, x: ValueId) -> Option<crate::run::LineMark> {
    let [n, mine, name, own] = list_of(v, x)[..] else {
        return None;
    };
    let first = |x: ValueId| list_of(v, x).first().copied();
    Some((
        first(n).map(|n| int_of(v, n)),
        int_of(v, mine),
        first(name),
        first(own).and_then(|o| match list_of(v, o)[..] {
            [a, b] => Some((
                u64::from_ne_bytes(int_of(v, a).to_ne_bytes()),
                u32::try_from(int_of(v, b)).ok()?,
            )),
            _ => None,
        }),
    ))
}

/// A nest level's `mode_place` as a value ([`place_of`]).
fn place_value(v: &phitex_defuse::Values, p: Option<&crate::run::DiagPlace>) -> ValueId {
    let Some((a, n, b, s, k)) = p else {
        return ValueId::UNIT;
    };
    v.list(vec![
        *a,
        v.int(*n),
        *b,
        v.str(s),
        v.int(i64::try_from(*k).unwrap_or(0)),
    ])
}

/// [`place_value`]'s place.
fn place_of(v: &phitex_defuse::Values, x: ValueId) -> Option<crate::run::DiagPlace> {
    let [a, n, b, s, k] = list_of(v, x)[..] else {
        return None;
    };
    Some((
        a,
        int_of(v, n),
        b,
        crate::val::str_of(v, s),
        usize::try_from(int_of(v, k)).unwrap_or(0),
    ))
}

/// A value shortened for a trace line.
fn short(v: &phitex_defuse::Values, x: ValueId) -> String {
    let s = v.render(x);
    if s.len() > 160 {
        let mut k = 160;
        while !s.is_char_boundary(k) {
            k -= 1;
        }
        format!("{}…", &s[..k])
    } else {
        s
    }
}

/// The first place two values differ, by list index ([`Run::ck_compare`]).
fn diff_path(v: &phitex_defuse::Values, at: &str, a: ValueId, b: ValueId) -> String {
    let (xa, xb) = (list_of(v, a), list_of(v, b));
    if !xa.is_empty() && !xb.is_empty() && xa.len() == xb.len() {
        let d: Vec<usize> = (0..xa.len()).filter(|&i| xa[i] != xb[i]).collect();
        if d.len() == 1 {
            return diff_path(v, &format!("{at}[{}]", d[0]), xa[d[0]], xb[d[0]]);
        }
        if d.len() > 1 && d.len() <= 4 {
            return d
                .iter()
                .map(|&i| diff_path(v, &format!("{at}[{i}]"), xa[i], xb[i]))
                .collect::<Vec<_>>()
                .join(" ; ");
        }
    }
    format!("{at}: resumed {} whole {}", short(v, a), short(v, b))
}

/// Whether `PHITEX_RESUME_CHECK` is set (`observe`).
pub(crate) fn check() -> bool {
    crate::exec::obs_flag!("PHITEX_RESUME_CHECK")
}

/// The first line two signatures differ at, shortened.
pub(crate) fn first_difference(a: &str, b: &str) -> String {
    for (x, y) in a.lines().zip(b.lines()) {
        if x != y {
            let k = x.bytes().zip(y.bytes()).take_while(|(p, q)| p == q).count();
            let lo = k.saturating_sub(30);
            let cut = |s: &str| s.get(lo..(k + 200).min(s.len())).unwrap_or("").to_owned();
            return format!(
                "at {:?}: resumed …{}… whole …{}…",
                &x[..x.find(' ').unwrap_or(0)],
                cut(x),
                cut(y)
            );
        }
    }
    "in the number of lines".to_owned()
}

/// A candidate's mark as a value ([`crate::pages::CandMark`]).
fn cand_value(v: &phitex_defuse::Values, m: &crate::pages::CandMark) -> ValueId {
    let u = |x: u64| v.int(i64::from_ne_bytes(x.to_ne_bytes()));
    let z = |x: usize| v.int(i64::try_from(x).unwrap_or(0));
    v.list(vec![
        v.int(i64::from(m.k)),
        u(m.stamp),
        v.int(i64::from(m.gids)),
        z(m.contribs),
        z(m.kids),
        v.int(m.lines),
        z(m.sig.0),
        z(m.sig.1),
        z(m.sig.2),
        u(m.sig.3),
        z(m.nest),
        v.int(
            i64::from(m.cuttable)
                | i64::from(m.fresh) << 1
                | i64::from(m.restartable) << 2
                | i64::from(m.open_cut) << 3,
        ),
    ])
}

/// [`cand_value`]'s mark.
fn cand_of(v: &phitex_defuse::Values, x: ValueId) -> Option<crate::pages::CandMark> {
    let n: Vec<i64> = list_of(v, x).iter().map(|&y| int_of(v, y)).collect();
    let [
        k,
        stamp,
        gids,
        contribs,
        kids,
        lines,
        s0,
        s1,
        s2,
        s3,
        nest,
        b,
    ] = n[..]
    else {
        return None;
    };
    let u = |x: i64| u64::from_ne_bytes(x.to_ne_bytes());
    let z = |x: i64| usize::try_from(x).unwrap_or(0);
    Some(crate::pages::CandMark {
        k: u32::try_from(k).ok()?,
        stamp: u(stamp),
        gids: u32::try_from(gids).ok()?,
        contribs: z(contribs),
        kids: z(kids),
        lines,
        sig: (z(s0), z(s1), z(s2), u(s3)),
        nest: z(nest),
        cuttable: b & 1 != 0,
        fresh: b & 2 != 0,
        restartable: b & 4 != 0,
        open_cut: b & 8 != 0,
        par_cut: false,
    })
}

/// A macro's call as [`Run::ck_level_extras`] encodes it, a record of
/// `input` again (`toks`: its level's list): its index, or a
/// [`crate::input::Level::named`] level's name and `true`.
fn ck_call_of(
    v: &phitex_defuse::Values,
    e: ValueId,
    toks: &[crate::val::Tok],
    input: &mut crate::input::Input,
) -> (u32, bool) {
    // (a macro's call, as encoded: its level's again)
    let call = list_of(v, e).get(5).and_then(|&c| match list_of(v, c)[..] {
        // (a macro without parameters: its name, [`Level::named`])
        [name] => {
            let name = *crate::val::toks_of(v, Some(name)).first()?;
            Some((name, None, Vec::new()))
        }
        [name, params, body, flags, lens] => {
            let name = *crate::val::toks_of(v, Some(name)).first()?;
            let params = crate::val::toks_of(v, Some(params));
            let body = if params.is_empty() {
                toks.to_vec()
            } else {
                crate::val::toks_of(v, Some(body))
            };
            let mac = crate::val::Macro::new(
                u8::try_from(int_of(v, flags)).unwrap_or(0),
                params,
                body.into(),
            );
            let lens: Vec<u32> = list_of(v, lens)
                .into_iter()
                .map(|x| u32::try_from(int_of(v, x)).unwrap_or(0))
                .collect();
            Some((name, Some(std::rc::Rc::new(mac)), lens))
        }
        _ => None,
    });
    call.map_or(
        (crate::input::NO_CALL, false),
        |(name, mac, lens)| match mac {
            Some(mac) => (input.new_call(name, &mac, &lens), false),
            None => (name.raw(), true),
        },
    )
}

/// The level just pushed again ([`ck_call_of`]): its call, and its
/// tokens put back in place as [`Run::ck_level_extras`] encodes them.
fn ck_unread_put(
    v: &phitex_defuse::Values,
    e: ValueId,
    input: &mut crate::input::Input,
    (call, named): (u32, bool),
) {
    if let Some(l) = input.levels.last_mut() {
        l.call = call;
        l.named = named;
    }
    if let Some(&u) = list_of(v, e).get(6)
        && let [from, to] = list_of(v, u)[..]
        && let Some(l) = input.levels.last().copied()
    {
        let at = |x| l.c + u32::try_from(int_of(v, x)).unwrap_or(0);
        input.unread = Some(crate::input::Unread {
            level: u32::try_from(input.levels.len() - 1).unwrap_or(u32::MAX),
            c: l.c,
            b: l.b,
            from: at(from),
            to: at(to),
        });
    }
}

#[cfg(test)]
mod tests {
    /// A dense run gone on from a checkpoint before its edit spaces its
    /// checkpoints doubling, capped; a dense run that did not go on, and a
    /// run not dense, do not.
    #[test]
    fn checkpoints_double_apart_only_where_a_dense_run_went_on() {
        let gap = super::ck_gap;
        assert_eq!(
            [0, 1, 2, 3].map(|k| gap(true, true, k, 600, 10_000)),
            [1200, 2400, 4800, 9600],
            "doubling, whatever the parts cost"
        );
        assert_eq!(gap(true, true, 15, 600, 0), 600 << 16);
        assert_eq!(gap(true, true, 40, 600, 0), 600 << 16, "capped");
        assert_eq!(gap(true, false, 5, 600, 10_000), 600, "dense: every");
        assert_eq!(
            gap(false, true, 5, 600, 10_000),
            40_000,
            "not dense: by what they cost"
        );
        assert_eq!(gap(false, false, 5, 600, 1), 600, "at least every");
    }

    /// A paragraph that closes the group an earlier node opened, its
    /// first checkpoint due inside a conditional of its own that the
    /// closing command's expansion ends: the checkpoint came at that
    /// command, where the run's scope effect is set and the run ends
    /// (its rest runs outside the scope); a run that went on from it read
    /// on inside the closed group (enumitem -j4: `\end{note}` lost). No
    /// checkpoint is taken once a run has closed its scope, at -j1.
    #[test]
    fn no_checkpoint_after_the_run_closed_its_scope() {
        use std::fmt::Write as _;
        use std::sync::Arc;
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../oracle/fixtures");
        let read = |p: &std::path::Path| -> String {
            std::fs::read(p)
                .expect("a file")
                .iter()
                .map(|&c| char::from(c))
                .collect()
        };
        let s = read(&fx.join("dvi/plaintext.tex"));
        let at = s.find("\\output={}\n").expect("plaintext.tex's body");
        let prelude = s[..at].replace(
            "% (\\input hyphen: no hyphenation patterns on the graph yet)\n",
            &read(&fx.join("hyph/hyphen.tex")),
        );
        let kp = std::sync::Mutex::new({
            let p = std::env::var("PATH").unwrap_or_default();
            std::env::split_paths(&p)
                .find(|d| d.join("tex").is_file())
                .map(|d| phitex_kpse::Kpse::new(&d, "tex", "tex"))
        });
        let finder: crate::Finder = Arc::new(move |key: &str| {
            let font = crate::font_name(key);
            let name = font.unwrap_or(key);
            let fmt = match font.and_then(|n| n.rsplit('.').next()) {
                Some("tfm") => phitex_kpse::Format::Tfm,
                Some("map") => phitex_kpse::Format::FontMap,
                Some("vf") => phitex_kpse::Format::Vf,
                _ => phitex_kpse::Format::Tex,
            };
            let mut k = kp.lock().ok()?;
            let p = k.as_mut()?.find_file(name.as_bytes(), fmt, false)?;
            let p = String::from_utf8(p).ok()?;
            Some((p.clone(), std::fs::read(&p).ok()?))
        });
        let mut doc = String::from(
            "\\hsize=300pt\n\\def\\close{\\fi\\endgroup}\\begingroup\\count1=5 Set.\n\n\\iftrue ",
        );
        for k in 0..700 {
            let _ = write!(doc, "w{}rd{}\\kern0pt", ["o", "a", "e"][k % 3], k % 17);
            doc.push(if k % 10 == 9 { '\n' } else { ' ' });
        }
        doc.push_str("\\relax\\close then \\the\\count1.\n\n\\bye\n");
        let opts = crate::Options {
            finder,
            threads: Some(1),
            ..crate::Options::default()
        };
        let b = crate::Build::with(opts, Some(&prelude), &doc);
        assert!(b.stats.nodes_run > 0, "the document ran");
        let taken = super::TAKEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert!(!taken.is_empty(), "checkpoints were taken");
        assert!(
            !taken.iter().any(|&closed| closed),
            "a checkpoint was taken after its run closed its scope"
        );
    }
}
