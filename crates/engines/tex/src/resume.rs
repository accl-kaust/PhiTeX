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

use phitex_defuse::{Cause, Field, ValueId};

use crate::input::Seg;
use crate::run::{R, Run};
use crate::val::{int_of, list_of};

/// The commands between two checkpoints of a run (`observe`:
/// `PHITEX_CK_EVERY`, for the tests, another number).
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
    /// The conditionals the run began inside (`carried_conds`): a
    /// checkpoint has no conditional of its own open.
    conds0: Vec<u8>,
}

/// The paragraph's mode (tex.web's `hmode`, unrestricted).
const HMODE: i32 = 102;

impl Run<'_, '_> {
    /// Whether the run takes checkpoints: an exec run of an edited
    /// node's text.
    pub(crate) fn ck_begin(&mut self) {
        // (an edited node's run, or a run of a node whose last run took
        // them: they stay for its next edit)
        if self.cx.op() != self.ops.exec
            || !(matches!(self.cx.woken_by(), Some(Cause::Edited)) || self.cx.last_parked())
        {
            return;
        }
        let taken = self.ck.take().map(|c| c.taken).unwrap_or_default();
        self.ck = Some(Box::new(Checkpoints {
            next: self.cmds + every(),
            taken,
            conds0: self.conds.clone(),
        }));
    }

    /// At a command's end: a checkpoint if one is due and the run stands
    /// where one can be taken.
    pub(crate) fn ck_maybe(&mut self) -> R<()> {
        let Some(c) = self.ck.as_ref() else {
            return Ok(());
        };
        if self.cmds < c.next || self.conds != c.conds0 {
            return Ok(());
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

    /// Where the run stands, if a checkpoint can be taken there: the
    /// text's segment the line read is, the line's place and state, its
    /// `\endlinechar` and the characters the run appended to it.
    fn ck_where(&self) -> Option<(usize, u32, u8, i64, Vec<u32>)> {
        // (the node's line at the bottom; above it only token lists: the
        // tokens a command read ahead and put back, kept with the place)
        let toks = |l: &crate::input::Level| l.kind == crate::input::kind::TOKS;
        if self.input.levels.is_empty()
            || !self.input.levels[1..].iter().all(toks)
            || self.pulled != 0
            || !self.kids.is_empty()
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
            || !self.pending_pre.is_empty()
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
    fn ck_take(&mut self, at: (usize, u32, u8, i64, Vec<u32>)) -> R<()> {
        let (li, loc, state, endch, extra) = at;
        let v = self.v;
        // (the token lists above the line, bottom first: their types and
        // the tokens left)
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
        if let Some(c) = self.ck.as_mut() {
            c.next = self.cmds + every();
        }
        // (the table at the run's base level: what it defined and stepped;
        // none where a group of its own saved a value)
        let Some(base) = self.st.base_of(v) else {
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
        let base = base.to_value(v, &mut |l| self.loc_field(l));
        let (cont, locals) = self.cont_encode()?;
        if !locals.is_empty() {
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
            int(i64::from_ne_bytes(self.lines_read.to_ne_bytes())),
            int(i64::from_ne_bytes(self.exps.to_ne_bytes())),
        ]);
        let ck = v.list(vec![
            int(i64::try_from(li).unwrap_or(-1)),
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
        ]);
        if let Some(c) = self.ck.as_mut() {
            c.taken.push(ck);
        }
        Ok(())
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
        if c.taken.is_empty() {
            return;
        }
        let v = self.v;
        let mut xs = vec![v.list(self.cx.args().to_vec())];
        xs.extend(c.taken);
        self.cx.park(v.list(xs));
    }

    /// An edited node's run goes on from its last run's last checkpoint
    /// the edit left in place, if there is one: true then. Its input (the
    /// node's new text, appended) is set to the checkpoint's place.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn resume_edit(&mut self) -> R<bool> {
        if self.no_resume || self.cx.op() != self.ops.exec {
            return Ok(false);
        }
        let Some(p) = self.cx.parked_before_edit() else {
            if crate::exec::obs_flag!("PHITEX_CK_TRACE") {
                eprintln!(
                    "ck: node {} woken by {:?}: no checkpoints",
                    self.cx.node().raw(),
                    self.cx.woken_by()
                );
            }
            return Ok(false);
        };
        let v = self.v;
        let xs = list_of(v, p);
        let Some((&old, cks)) = xs.split_first() else {
            return Ok(false);
        };
        let old_args = list_of(v, old);
        let args = self.cx.args().to_vec();
        if old_args.len() != args.len() || old_args.get(1..) != args.get(1..) {
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
            return Ok(false);
        };
        let li = usize::try_from(int_of(v, c[0])).unwrap_or(0);
        let loc = usize::try_from(int_of(v, c[1])).unwrap_or(0);
        let Some(base) = crate::state::Base::of_value(v, c[11], &mut |f| self.loc_of(f)) else {
            return Ok(false);
        };
        self.cont_load(c[6], ValueId::UNIT)?;
        self.st.with_base(v, base);
        // (the input: the new text, its segment `li` the line on top at
        // the checkpoint's place)
        let Some(Seg::Line(mut line)) = new_segs.get(li).cloned() else {
            return Ok(false);
        };
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
        self.cont_lines.clear();
        self.ck_restore(&c);
        self.cand_marks = list_of(v, c[12])
            .iter()
            .filter_map(|&m| cand_of(v, m))
            .collect();
        self.muts += 1;
        self.resumed_at = Some(self.cmds);
        self.ops
            .resumed_edits
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.cx.goes_on_from_edit();
        // (the checkpoints up to this one stand for the next run)
        let conds0 = self.conds.clone();
        self.ck = Some(Box::new(Checkpoints {
            next: self.cmds + every(),
            taken: cks[..=k].to_vec(),
            conds0,
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
            lines_read,
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
            self.lines_read = u64::from_ne_bytes(lines_read.to_ne_bytes());
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
    })
}
