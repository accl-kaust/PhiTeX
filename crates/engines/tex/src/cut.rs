//! Content-defined cuts: a run ends where its text ends, with a
//! paragraph, a box, a group or a conditional of its own open, and the
//! rest of its work is a continuation node of its own.
//!
//! A document's text is cut into slices (`crate::hints`): its paragraph
//! regions, its commands, and, in a long slice, more cuts the text alone
//! chooses (`hints::cdc_starts`: a line start whose line before hashes
//! to a pattern, a minimum of bytes past the last cut, as rsync's and
//! restic's chunkers cut, so an edit moves the cuts next to it and no
//! other). A run reaching its text's end with something of its own open
//! read on into the next slice (`Run::pull`), claimed, for as long as it
//! stayed open: one node for a whole long block, run from its start
//! again for any edit of it or any read of it that moved.
//!
//! Here such a run may end there instead ([`Run::text_cut`]), where the
//! state is nameable: at its own base level of the input (no file, no
//! token list left, no command in progress), with nothing of it outside
//! what a continuation carries (the nest with its lists, the groups'
//! bindings, the conditionals, the counters, `Run::cont_encode`) and
//! nothing of it pending for the page (no item on the main list since
//! its last candidate). Its definitions are published as any run's;
//! the rest is a resume (`crate::pages::Resume`) placed after it, as at
//! a page-builder candidate (`Run::fold_here`): it reads the global
//! state the way any node does, through its fields, and runs the next
//! slices' text, claimed. An edit before the cut runs the first part
//! again and the continuation only if what it reads moved (its
//! continuation is a field the first part defines); an edit after the
//! cut runs the continuation only.
//!
//! A cut is taken only where the run has done [`Ops::cut_min`] commands
//! or more since its start (a node's overhead stays a small share of
//! its work: plain text reads on as before, a picture's code or a long
//! table of commands is cut), and where what the continuation carries is
//! at most [`MAX_RESIDUE`] items (never a huge half-built box). A cut is
//! only a split of sequential work: the output is the same wherever the
//! cuts are, and none at all (`cut_min` = `u64::MAX`).
//!
//! `PHITEX_CUT_CHECK=1`: a continuation, once loaded, is
//! encoded again and compared with the one it was given; a difference
//! is printed (`cut check: ... DIFFERENT`): the state did not go across
//! the cut whole.
//!
//! [`Ops::cut_min`]: crate::run::Ops::cut_min

use phitex_defuse::ValueId;

use crate::run::Run;
use crate::val::{int_of, list_of};

/// The items (the lists' and the groups' bindings) a continuation may
/// carry at most.
pub(crate) const MAX_RESIDUE: usize = 20_000;

/// The commands a run does before it may be cut, by default.
pub(crate) const CUT_MIN: u64 = 3000;

impl Run<'_, '_> {
    /// At the end of the run's text with something of its own open
    /// ([`Run::open_span`]), where the run would read on into the next
    /// slice: it ends here, its rest a continuation (true), if the state
    /// is nameable here and the cut pays (see the module).
    pub(crate) fn text_cut(&mut self) -> bool {
        if let Some(why) = self.text_cut_refusal() {
            if crate::exec::obs_flag!("PHITEX_CUT_TRACE") {
                eprintln!(
                    "cut: node {} at its text's end after {} commands: none ({why})",
                    self.cx.node().raw(),
                    self.cmds
                );
            }
            return false;
        }
        // (the next slice must be a text's: the rest runs it. Found so,
        // no read: the run reads nothing of the text after its own, so an
        // edit there runs only the continuation, `Ctx::ends_before`)
        if self.cx.ends_before(self.pulled, self.ops.exec) {
            return false;
        }
        if crate::exec::obs_flag!("PHITEX_CUT_TRACE") {
            eprintln!(
                "cut: node {} ({}) at its text's end after {} commands, pulled {}: cut, nest {:?}, conds {}",
                self.cx.node().raw(),
                self.op_name(),
                self.cmds,
                self.pulled,
                self.nest
                    .iter()
                    .map(|l| (l.mode, l.group, l.owns, l.list.len()))
                    .collect::<Vec<_>>(),
                self.conds.len()
            );
        }
        self.muts += 1;
        self.cut = Some(self.cand);
        self.text_cut = true;
        self.ops
            .text_cuts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        true
    }

    /// Why the run may not end here with a continuation, if it may not.
    fn text_cut_refusal(&self) -> Option<&'static str> {
        let op = self.cx.op();
        let checks: [(bool, &'static str); 26] = [
            (
                !(op == self.ops.exec || op == self.ops.resume) || !self.doc,
                "not a document's piece",
            ),
            (
                self.cmds < self.ops.cut_min.load(std::sync::atomic::Ordering::Relaxed),
                "few commands",
            ),
            (self.stop_at.is_some(), "a second pass"),
            (
                self.input.levels.iter().any(|l| match l.kind {
                    crate::input::kind::TOKS => l.a < l.b,
                    crate::input::kind::LINE => self.input.line(*l).has_more(),
                    _ => true,
                }),
                "input left",
            ),
            (
                self.effect != phitex_defuse::Effect::None,
                "the scope closed",
            ),
            (
                self.kids.iter().any(|k| {
                    ![
                        self.ops.put,
                        self.ops.toolnode,
                        self.ops.pdfpage,
                        self.ops.xdvpage,
                    ]
                    .contains(&k.0)
                }),
                "children",
            ),
            (self.claim_rest || self.fatal_hit || self.eof, "the end"),
            (
                self.cut.is_some() || self.close_cont.is_some() || self.tool_cut.is_some(),
                "cut already",
            ),
            (self.fin.is_some() || self.fin_box.is_some(), "a finisher"),
            (!self.sealed.is_empty(), "sealed lines"),
            (
                !self.lig_buf.is_empty() || self.chars_live.is_some(),
                "a word in progress",
            ),
            (self.interp.is_some(), "interpreting a line"),
            (self.scanner.is_some(), "a scan"),
            (!self.incs.is_empty(), "an incompleat noad"),
            (!self.seeded.is_empty(), "seeded read-backs"),
            (self.defer_hit.is_some(), "a deferred read"),
            (
                self.rb.is_some()
                    || self.read_back_at.is_some()
                    || self.read_point_pending.is_some()
                    || !self.pending_pre.is_empty(),
                "a read-back",
            ),
            (self.mode_zero, "mode zero"),
            (self.lt.sink.is_some(), "a lines sink"),
            (self.pic_child.is_some(), "a picture's child"),
            (self.aopen != 0, "an alignment"),
            (!self.ins_pending.is_empty(), "insertions pending"),
            (!self.closed.is_empty(), "streams closed"),
            (
                self.nest.iter().any(|l| !matches!(l.mode.abs(), 1 | 102)),
                "math",
            ),
            (
                self.main_items_since(self.cand_marks.last().map_or(0, |m| m.contribs)),
                "items pending for the page",
            ),
            (self.residue() > MAX_RESIDUE, "a large residue"),
        ];
        checks.iter().find(|c| c.0).map(|c| c.1)
    }

    /// What a continuation would carry here, in items: the nest's lists
    /// and the open groups' bindings and saves.
    fn residue(&self) -> usize {
        self.nest.iter().map(|l| l.list.len()).sum::<usize>() + self.st.saved_len()
    }

    /// `PHITEX_CUT_CHECK`: a resume's continuation `cont`, loaded, is
    /// encoded again and compared with itself (the frames aside: those
    /// are the input, which the run has begun to read).
    pub(crate) fn cut_check(&mut self, cont: ValueId) {
        let v = self.v;
        let Ok((again, _)) = self.cont_encode_with(false, true) else {
            eprintln!(
                "cut check: node {}: no encoding (a stop)",
                self.cx.node().raw()
            );
            return;
        };
        let a = crate::val::list_of(v, cont);
        let b = crate::val::list_of(v, again);
        let diffs: Vec<String> = (1..a.len().max(b.len()))
            .filter(|&i| a.get(i) != b.get(i))
            .map(|i| {
                diff_path(
                    v,
                    &format!("[{i}]"),
                    a.get(i).copied().unwrap_or(ValueId::UNIT),
                    b.get(i).copied().unwrap_or(ValueId::UNIT),
                )
            })
            .collect();
        if diffs.is_empty() {
            eprintln!("cut check: node {}: same", self.cx.node().raw());
        } else {
            eprintln!(
                "cut check: node {}: DIFFERENT {}",
                self.cx.node().raw(),
                diffs.join(" | ")
            );
        }
    }
}

/// Whether `PHITEX_CUT_CHECK` is set (any build: the tests' check).
pub(crate) fn check() -> bool {
    crate::exec::env_flag!("PHITEX_CUT_CHECK")
}

/// A nest level's `mode_line` as a value ([`line_mark_of`]).
pub(crate) fn line_mark_value(
    v: &phitex_defuse::Values,
    m: Option<crate::run::LineMark>,
) -> ValueId {
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
pub(crate) fn line_mark_of(v: &phitex_defuse::Values, x: ValueId) -> Option<crate::run::LineMark> {
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
pub(crate) fn place_value(v: &phitex_defuse::Values, p: Option<&crate::run::DiagPlace>) -> ValueId {
    let Some((a, n, b, s, k, o)) = p else {
        return ValueId::UNIT;
    };
    v.list(vec![
        *a,
        v.int(*n),
        *b,
        v.str(s),
        v.int(i64::try_from(*k).unwrap_or(0)),
        o.map_or(ValueId::UNIT, |o| {
            v.int(i64::from_ne_bytes(o.to_ne_bytes()))
        }),
    ])
}

/// [`place_value`]'s place.
pub(crate) fn place_of(v: &phitex_defuse::Values, x: ValueId) -> Option<crate::run::DiagPlace> {
    let [a, n, b, s, k, o] = list_of(v, x)[..] else {
        return None;
    };
    Some((
        a,
        int_of(v, n),
        b,
        crate::val::str_of(v, s),
        usize::try_from(int_of(v, k)).unwrap_or(0),
        (o != ValueId::UNIT).then(|| u64::from_ne_bytes(int_of(v, o).to_ne_bytes())),
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

/// The first place two values differ, by list index ([`Run::cut_check`]).
pub(crate) fn diff_path(v: &phitex_defuse::Values, at: &str, a: ValueId, b: ValueId) -> String {
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
