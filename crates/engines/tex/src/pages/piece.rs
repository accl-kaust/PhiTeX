//! Layer 2's side of the pages: a text piece on the main vertical list.
//!
//! - The candidates (§994's `build_page` calls) a piece passes, with
//!   what the builder reads of eqtb there ([`Run::finish_candidate`]).
//! - A piece ends at every candidate it can end at with more than blank
//!   input left (a cut, [`Run::fold_here`]): its rest is a [`Resume`]
//!   placed after it, after the finishers of the pages firing there,
//!   its continuation its arguments ([`Run::cont_encode`],
//!   [`Run::cont_load`]). A cut is how the text is divided into nodes;
//!   no page decides it.
//! - At its end a piece gives its profile and its items as units
//!   ([`Run::contribute_units`]) and defines what it leaves the next
//!   piece of the main list ([`Run::define_main_tail`]).
//! - A finisher of a page firing at a candidate inside a piece that
//!   could not end there runs its routine on the piece's state at the
//!   candidate (its check data, [`Run::preload_candidate`]), and the
//!   piece's later definitions stand over the routine's
//!   ([`Run::give_way`]).

use super::{Run, R, group, Halt, Par, nodes, list_of, int_of, ValueId, Field, str_of, InsertClass, glue_value, glue_of, PITEMS, Locality, Loc, BEFORE_PIECE, PAGE_FIELDS, MayDefine, Values, Line, Operation, toks_value, Seg, ContSlot, some, ContSaved, State, ContState, GDef, replaced, FoldState, IGNORE_DEPTH, toks_of, Shared, Effect, Ctx, Stop, ops, run_node};

/// What a run had done when it passed a candidate: at its end, a page
/// finishing at its last candidate finishes at the end of the piece iff
/// nothing of this changed after it; a finisher's check of a page
/// finishing at an earlier one takes what the run defined after it
/// (`stamp`, `gids`: [`State::defined_since_in_order`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "a candidate's four independent facts")]
pub(crate) struct CandMark {
    pub k: u32,
    pub stamp: u64,
    pub gids: u32,
    pub contribs: usize,
    pub kids: usize,
    pub lines: i64,
    /// The state table's shape at the candidate (definitions at the base
    /// level, saves, local definitions in groups, the clock) and the
    /// nest's depth: a group closed after the candidate restores without
    /// advancing the clock, and the page was finished inside it.
    pub sig: (usize, usize, usize, u64),
    pub nest: usize,
    /// Whether the run could end at the candidate, its rest a resume
    /// ([`Run::finish_candidate`]): not inside a group or a conditional
    /// of its own, with input left.
    pub cuttable: bool,
    /// Whether items came on the main list since the candidate before:
    /// a page can fire here (§994).
    pub fresh: bool,
    /// Whether a second pass can end the run here ([`Halt::Recut`]): a
    /// candidate it could cut at, or a paragraph's start on the main
    /// vertical list (the level just pushed, below it only groups of the
    /// run's that own no list; the continuation carrying the levels, the
    /// paragraph's list and the input `\everypar` left, §1091).
    pub restartable: bool,
    /// Whether a second pass can end the run here though a group or a
    /// conditional of its own is open (owning no list): the candidate a
    /// page fires at that the run read the page after
    /// ([`Run::order_after_own_page`]).
    pub open_cut: bool,
}

impl Run<'_, '_> {
    /// Whether the run is on the main vertical list, at most a paragraph
    /// just begun above it (`nest_ptr <= 1` of §1091, §1200; `mode =
    /// vmode` of §1076, §1094, §1100, §1103, §812).
    pub(crate) fn at_outer(&mut self) -> R<bool> {
        let base = self.base_mode()?;
        if base.lid != 0 || base.km != 1 {
            return Ok(false);
        }
        let owning: Vec<(i64, i32)> = self
            .nest
            .iter()
            .filter(|l| l.owns)
            .map(|l| (l.group, l.mode))
            .collect();
        // (§1145's `nest_ptr=1`: a display's math level right above the
        // outer vertical list, the paragraph before it broken into lines)
        Ok(match owning[..] {
            [] => true,
            [(g, m)] => (g == 0 && m.abs() == 102) || (g == group::MATH_SHIFT && m == 203),
            _ => false,
        })
    }

    /// A finish candidate: tex.web calls `build_page` here. The run
    /// notes what the builder reads of eqtb at this call (§987's page
    /// parameters, §1008's registers of the insertion classes it put on
    /// the list since the last candidate), then the candidate marker.
    /// At the candidate it is to stop at (its second pass after a cut),
    /// the run ends.
    pub(crate) fn finish_candidate(&mut self) -> R<()> {
        self.finish_candidate_at(false)
    }

    /// [`Run::finish_candidate`]; `newgraf`: the candidate of a
    /// paragraph's start (§1091), after its level is pushed, its indent
    /// box appended and `\everypar` put on the input.
    pub(crate) fn finish_candidate_at(&mut self, newgraf: bool) -> R<()> {
        if !self.at_outer()? {
            return Ok(());
        }
        let k = self.cand;
        // (§994: `build_page` with nothing contributed does nothing, so a
        // page fires only at a candidate with items contributed since the
        // one before: the last of this run, or for a resume's first one
        // the candidate its piece was cut at, where nothing was pending;
        // a piece's first candidate otherwise may see items an earlier
        // piece left after its last)
        let fresh = match self.cand_marks.last() {
            Some(m) => self.main_items_since(m.contribs),
            None => self.cx.op() != self.ops.resume || self.main_items_since(0),
        };
        self.note_candidate(k)?;
        self.cand += 1;
        // (or the candidate a second pass ends at, `Halt::Recut`)
        let can = self.cuttable_here();
        // (a paragraph's start: its level just pushed on top, below it
        // only groups the run opened on the main vertical list, which own
        // no list: the continuation carries every level, §1091's list
        // and the input `\everypar` left, as a cut inside such groups)
        let restartable = can
            || (newgraf
                && self.conds.is_empty()
                && self.nest.split_last().is_some_and(|(top, below)| {
                    top.owns && top.group == 0 && below.iter().all(|l| !l.owns && l.group != 0)
                }));
        // (a rest of blank input makes no node: the piece runs it, spaces
        // and an empty line's `\par` in vertical mode, §1070)
        let open_cut = self.open_span() && self.cuttable_open();
        let cuttable = fresh
            && ((can && !self.rest_blank()?) || ((restartable || open_cut) && self.stop_at == Some(k)));
        self.cand_marks.push(CandMark {
            k,
            stamp: self.st.stamp(),
            gids: self.st.gid_mark(),
            contribs: self.contribs.len(),
            kids: self.kids.len(),
            lines: self.lines_entered,
            sig: self.st.sig(),
            nest: self.nest.len(),
            cuttable,
            fresh,
            restartable,
            open_cut,
        });
        self.st.mark_candidate();
        if crate::exec::obs_flag!("PHITEX_CAND_TRACE") {
            self.trace_candidate(k, newgraf, fresh, cuttable);
        }
        // (every candidate with text after it is a cut, as every
        // paragraph end is: the rest of the piece is a continuation
        // placed after it, after the finisher of a page firing there,
        // which reads the state there forward and what a routine leaves.
        // Nothing decides it after the piece: no page's finisher reaches
        // back into the piece it fires in)
        // (inside a group or a conditional of the run's own, only where
        // the page is made to fire, above: elsewhere the piece runs on
        // through the candidate, and a page firing there has its
        // finisher run on the piece's state there, [`Run::preload_candidate`])
        if cuttable {
            if self.open_span() && crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
                eprintln!(
                    "page-trace: node {} cut inside its own span at candidate {k}: nest {:?} conds {}",
                    self.cx.node().raw(),
                    self.nest
                        .iter()
                        .map(|l| (l.group, l.owns))
                        .collect::<Vec<_>>(),
                    self.conds.len()
                );
            }
            self.cut = Some(k);
            return Err(Halt::Cut);
        }
        Ok(())
    }

    /// What the builder reads of eqtb at candidate `k` noted on the main
    /// list (§987's page parameters, §1008's registers of the insertion
    /// classes put on it since the last candidate), then its marker.
    pub(crate) fn note_candidate(&mut self, k: u32) -> R<()> {
        let v = self.v;
        if self.st.take_page_params() {
            if crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
                eprintln!(
                    "page-trace: node {} candidate {k}: page params changed, vsize {}",
                    self.cx.node().raw(),
                    self.ipar(Par::vsize)?
                );
            }
            self.put_page_params();
        } else if k == 0 && !self.own_page_params_put() {
            // (the parameters reaching the piece, unless an item of it
            // already noted the ones it set, `Run::append`: that marker
            // is later in force and this one, after it, would undo it)
            self.put_page_params_reaching();
        }
        for n in std::mem::take(&mut self.ins_pending) {
            // (the class's registers as this run left them when it
            // defined one; else as they reach the fold, read there:
            // `Run::resolve_page_params`)
            let own = self.ins_registers_defined(n);
            let m = if own {
                self.ins_params_marker(n)?
            } else {
                v.list(vec![v.int(nodes::T_INS_PARAMS), v.int(i64::from(n))])
            };
            self.contribute("items", v.seq_unit(v.list(vec![v.int(0), m])));
        }
        let m = v.list(vec![v.int(nodes::T_BUILD_PAGE), v.int(i64::from(k))]);
        self.contribute("items", v.seq_unit(v.list(vec![v.int(0), m])));
        Ok(())
    }

    /// Whether the run can end at the candidate it is at, its rest a
    /// resume (items contributed since the one before aside).
    pub(crate) fn cuttable_here(&mut self) -> bool {
        let op = self.cx.op();
        // (inside a group or a conditional of the run's own only where
        // the builder is made to fire: a penalty of `eject_penalty` or
        // less put on the list since the candidate before, §1005; the
        // routine runs there, and the rest reads what it defines (a page
        // so far saved in a box, a column height), so the rest is a
        // node after the routine. Never inside a list of the run's own)
        // (a paragraph's start is inside its own level, as any candidate
        // inside a span of the run's own: a page firing there is rare,
        // and its finisher runs on the state there,
        // [`Run::preload_candidate`]; one node a paragraph, not two)
        let open_ok = !self.open_span()
            || (self.nest.iter().all(|l| !l.owns && l.group != 0)
                && self.forced_since(self.cand_marks.last().map_or(0, |m| m.contribs)));
        (op == self.ops.exec || op == self.ops.resume)
            && (self.top || self.doc)
            && open_ok
            && self.has_input()
    }

    /// Whether the run could end at the candidate it is at inside groups
    /// or conditionals of its own, whatever forced the builder: the
    /// groups own no list, the continuation carries them.
    pub(crate) fn cuttable_open(&mut self) -> bool {
        let op = self.cx.op();
        (op == self.ops.exec || op == self.ops.resume)
            && (self.top || self.doc)
            && self.nest.iter().all(|l| !l.owns && l.group != 0)
            && self.has_input()
    }

    /// A read of the page (`\pagegoal`, `\pagetotal`, ...) after a
    /// candidate of the run's own a page fires at (`k`): TeX ran that
    /// page's routine there, and what the run reads after it (the page,
    /// what the routine set: LaTeX's `\vsize` for a float it placed) is
    /// the routine's. The run goes again from its start and ends at that
    /// candidate ([`Halt::Recut`]), its rest a resume placed after the
    /// page's finisher.
    pub(crate) fn order_after_own_page(&mut self, k: u32) -> R<()> {
        let op = self.cx.op();
        let piece = (op == self.ops.exec || op == self.ops.resume) && (self.top || self.doc);
        if !piece || self.cut.is_some() || self.stop_at.is_some() {
            return Ok(());
        }
        let Some(m) = self.cand_marks.iter().find(|m| m.k == k) else {
            return Ok(());
        };
        if !m.fresh || !(m.restartable || m.open_cut) {
            return Ok(());
        }
        if crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
            eprintln!(
                "page-trace: node {} ends at candidate {k}: a read of the page after a page fired there",
                self.cx.node().raw()
            );
        }
        Err(Halt::Recut(k))
    }

    /// A read of the objects the pages shipped before this run
    /// (`crate::pdfout::page_offset`), or an object made whose number
    /// counts them (`crate::pdfprim`), in a piece that ran through a
    /// candidate a page can fire at: pdfTeX ships that page before the
    /// command, and the page builder, not this run, knows whether it
    /// fires. The run goes again from its start and ends at the first
    /// such candidate ([`Halt::Recut`], [`Run::finish_candidate`])
    /// whether or not a page fires there (the piece's boundaries are its
    /// text's alone), its rest a resume placed after the pages firing
    /// there, which reads their objects. Where no pass can end at it (inside a conditional of the
    /// run's own), the read stands where no page fires at the run's
    /// candidates ([`Run::page_fires_in_own`]). (`why`: the reader, for
    /// the trace.)
    #[allow(
        clippy::assertions_on_constants,
        reason = "a debug or observe build stops on the defect"
    )]
    pub(crate) fn order_after_pages(&mut self, why: &str) -> R<()> {
        let op = self.cx.op();
        let piece = (op == self.ops.exec || op == self.ops.resume) && (self.top || self.doc);
        if !piece || self.cut.is_some() {
            return Ok(());
        }
        let Some(m) = self.cand_marks.iter().find(|m| m.fresh) else {
            return Ok(());
        };
        let (k, restartable) = (m.k, m.restartable);
        // (no pass can end there: complete where no page fires at any of
        // the run's candidates, the page builder's fold over the profiles
        // to its own says: a read of the page state, [`Feedback::PageSoFar`];
        // where one can, it ends there whatever the pages do)
        if !restartable && !self.page_fires_in_own()? {
            return Ok(());
        }
        if !restartable {
            // (a candidate inside a span of the run's own no pass can end
            // at: a page firing there would be missing from the number)
            let msg = format!(
                "the pages' objects read in a piece after a candidate a page can fire at and no pass can end at ({why}, candidate {k}): a number here may miss that page's objects (node {})",
                self.cx.node().raw()
            );
            assert!(!cfg!(any(debug_assertions, feature = "observe")), "{msg}");
            self.engine_diag(&msg);
            return Ok(());
        }
        if crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
            eprintln!(
                "page-trace: node {} ends at candidate {k}: a read of the pages' objects after it ({why})",
                self.cx.node().raw()
            );
        }
        Err(Halt::Recut(k))
    }

    /// `PHITEX_CAND_TRACE`: the candidate `k` and what decided its cut.
    pub(crate) fn trace_candidate(&mut self, k: u32, newgraf: bool, fresh: bool, cuttable: bool) {
        let v = self.v;
        let op = self.cx.op();
        if self.cand_marks.len() == 1 {
            let args = self.cx.args();
            let first = args
                .first()
                .map(|&x| v.render(x))
                .unwrap_or_default()
                .chars()
                .take(160)
                .collect::<String>();
            eprintln!("cand-text: node {} {first}", self.cx.node().raw());
        }
        eprintln!(
            "cand: node {} op {} k {k} newgraf {newgraf} fresh {fresh} input {} nest {:?} conds {} forced {} cuttable {cuttable}",
            self.cx.node().raw(),
            if op == self.ops.exec {
                "exec"
            } else if op == self.ops.resume {
                "resume"
            } else {
                "other"
            },
            self.has_input(),
            self.nest
                .iter()
                .map(|l| (l.mode, l.group, l.owns))
                .collect::<Vec<_>>(),
            self.conds.len(),
            self.forced_since(
                self.cand_marks
                    .len()
                    .checked_sub(2)
                    .and_then(|i| self.cand_marks.get(i))
                    .map_or(0, |m| m.contribs)
            ),
        );
    }

    /// Whether this run put an item on the main list (a node, or the
    /// removal of one) in its contributions from the `from`-th on; the
    /// markers for the builder and the writer are not items.
    pub(crate) fn main_items_since(&self, from: usize) -> bool {
        let v = self.v;
        let f = v.field("items");
        self.contribs
            .get(from..)
            .unwrap_or_default()
            .iter()
            .filter(|&&(g, _)| g == f)
            .any(|&(_, s)| {
                v.seq_to_vec(s).iter().any(|&e| {
                    let xs = list_of(v, e);
                    xs.len() == 2
                        && int_of(v, xs[0]) == 0
                        && matches!(
                            nodes::tag(v, xs[1]),
                            0..=nodes::T_UNSET
                                | nodes::T_DELETE
                                | nodes::T_VBOX
                                | nodes::T_END
                                | nodes::T_ABORT
                        )
                })
            })
    }

    /// Whether this run put a penalty of `eject_penalty` or less on the
    /// main list in its contributions from the `from`-th on (§1005: the
    /// builder fires the page there, unless the page holds no box).
    pub(crate) fn forced_since(&self, from: usize) -> bool {
        let v = self.v;
        let f = v.field("items");
        self.contribs
            .get(from..)
            .unwrap_or_default()
            .iter()
            .filter(|&&(g, _)| g == f)
            .any(|&(_, s)| {
                v.seq_to_vec(s).iter().any(|&e| {
                    let xs = list_of(v, e);
                    xs.len() == 2
                        && int_of(v, xs[0]) == 0
                        && nodes::tag(v, xs[1]) == nodes::T_PENALTY
                        && list_of(v, xs[1])
                            .get(1)
                            .is_some_and(|&p| int_of(v, p) <= -10_000)
                })
            })
    }

    /// The siblings this run's text says it takes: an exec node's hint
    /// (`None` where the text does not say), a resume's guess (its
    /// fourth argument).
    pub(crate) fn claim_guess(&self) -> Option<usize> {
        let v = self.v;
        let args = self.cx.args();
        let op = self.cx.op();
        if op == self.ops.resume {
            return args
                .get(3)
                .and_then(|&x| v.as_int(x))
                .and_then(|n| usize::try_from(n).ok());
        }
        if op != self.ops.exec {
            return None;
        }
        crate::hints::hint_of_args(&self.ops.hints, v, args)
            .bound
            .as_ref()
            .and_then(|b| b.claim)
    }

    /// Whether this run put a page parameter marker on the main list.
    pub(crate) fn own_page_params_put(&self) -> bool {
        let v = self.v;
        self.own_outer_items()
            .iter()
            .any(|&x| nodes::tag(v, x) == nodes::T_PAGE_PARAMS)
    }

    /// The items this run put on the main list, in order (its `items`
    /// contributions tagged with list 0).
    pub(crate) fn own_outer_items(&self) -> Vec<ValueId> {
        let v = self.v;
        let f = v.field("items");
        let mut out = Vec::new();
        for &(g, s) in &self.contribs {
            if g != f {
                continue;
            }
            for e in v.seq_to_vec(s) {
                let xs = list_of(v, e);
                if xs.len() == 2 && int_of(v, xs[0]) == 0 {
                    out.push(xs[1]);
                }
            }
        }
        out
    }

    /// The files this run wrote, opened or closed (its `streams`
    /// contributions from the `from`-th contribution on), as the fields
    /// `wstream:<name>`: a page finishing at a candidate before them
    /// ships its `\write`s before them in TeX's order ([`Run::check_data`]).
    /// (The line of PDF records is ordered by the link instead: a
    /// piece's records carry their candidate, `pdfout::order_line`.)
    pub(crate) fn streams_since(&self, from: usize) -> Vec<Field> {
        let v = self.v;
        let f = v.field("streams");
        let mut out: Vec<Field> = Vec::new();
        for &(g, x) in self.contribs.get(from..).unwrap_or_default() {
            if g != f {
                continue;
            }
            for e in v.seq_to_vec(x) {
                // (an entry tagged with its page: its own; a candidate
                // marker: none)
                if crate::api::is_cand_marker(v, e) {
                    continue;
                }
                let entry = list_of(v, crate::api::stream_entry(v, e));
                if let Some(&n) = entry.first() {
                    let w = v.field(&format!("wstream:{}", str_of(v, n)));
                    if !out.contains(&w) {
                        out.push(w);
                    }
                }
            }
        }
        out
    }

    /// A `T_INS_PARAMS` marker with class `n`'s registers as this run
    /// sees them (§1008).
    pub(crate) fn ins_params_marker(&mut self, n: u16) -> R<ValueId> {
        let c = self.insert_class(n)?;
        Ok(self.ins_marker(n, &c))
    }

    /// A `T_INS_PARAMS` marker with class `n`'s registers `c`.
    pub(crate) fn ins_marker(&self, n: u16, c: &InsertClass) -> ValueId {
        let v = self.v;
        v.list(vec![
            v.int(nodes::T_INS_PARAMS),
            v.int(i64::from(n)),
            v.int(c.box_height_plus_depth.map_or(-1, i64::from)),
            v.int(i64::from(c.box_is_hbox)),
            v.int(i64::from(c.count)),
            v.int(i64::from(c.dimen)),
            glue_value(v, &c.skip),
        ])
    }

    /// Whether this run defined one of class `n`'s registers (its box,
    /// `\count`, `\dimen` or `\skip`).
    pub(crate) fn ins_registers_defined(&mut self, n: u16) -> bool {
        let bl = self.box_loc(i64::from(n));
        if self.st.defined_after(bl, 0) {
            return true;
        }
        (0..3).any(|r| {
            let l = self.reg_loc(r, i32::from(n));
            self.st.defined_after(l, 0)
        })
    }

    pub(crate) fn insert_class(&mut self, n: u16) -> R<InsertClass> {
        let b = self.box_reg(i32::from(n))?;
        let v = self.v;
        Ok(InsertClass {
            box_height_plus_depth: b.map(|b| nodes::box_dim(v, b, 3) + nodes::box_dim(v, b, 2)),
            box_is_hbox: b.is_some_and(|b| nodes::box_is_h(v, b)),
            count: self.reg_int(0, n)?,
            dimen: self.reg_int(1, n)?,
            skip: glue_of(v, self.reg(2, i32::from(n))?),
        })
    }

    /// The `[tag]` page parameter markers of `raw` (the parameters
    /// reaching the piece, [`Run::put_page_params_reaching`]) given their
    /// values, when the piece defined a page parameter itself: the
    /// marker means the values in force where the piece began, and both
    /// the run's own state at its end and the fold after it hold the
    /// piece's later values (multicol sets `\vsize`: a first pass and a
    /// pass stopped at the candidate resolved them differently and never
    /// agreed). A piece that leaves them alone reads nothing here.
    pub(crate) fn pin_reaching_page_params(&mut self, raw: &mut [ValueId]) -> R<()> {
        let v = self.v;
        let names = [
            "param:vsize",
            "param:maxdepth",
            "param:topskip",
            "param:holdinginserts",
        ];
        let mut defined = false;
        for n in names {
            let l = self.loc_of(v.field(n));
            if self.st.defined_after(l, 0) {
                defined = true;
            }
        }
        let has = raw
            .iter()
            .any(|&x| nodes::tag(v, x) == nodes::T_PAGE_PARAMS && list_of(v, x).len() == 1);
        if !defined || !has {
            return Ok(());
        }
        let mut xs = Vec::with_capacity(4);
        for n in names {
            xs.push(self.eng_read(v.field(n))?);
        }
        let int = |x: Option<ValueId>| x.map_or(0, |x| int_of(v, x));
        let marker = v.list(vec![
            v.int(nodes::T_PAGE_PARAMS),
            v.int(int(xs[0])),
            v.int(int(xs[1])),
            glue_value(v, &glue_of(v, xs[2])),
            v.int(int(xs[3])),
        ]);
        for x in raw.iter_mut() {
            if nodes::tag(v, *x) == nodes::T_PAGE_PARAMS && list_of(v, *x).len() == 1 {
                *x = marker;
            }
        }
        Ok(())
    }

    /// The run's end on the page. A piece with items for the main list
    /// defines what it leaves the next piece of the main list
    /// ([`Run::define_main_tail`]), gives its
    /// profile and its items as units ([`Run::contribute_units`]); its
    /// own run never reads
    /// the page. A piece whose run ended at a page-builder candidate with
    /// input after it leaves the rest to a resume piece
    /// ([`Run::fold_here`], spec-final A1). A piece with nothing
    /// for the main list touches nothing. A finisher's end defines what
    /// its routine gives back ([`RTN`]).
    pub(crate) fn page_fold(&mut self) -> R<()> {
        let op = self.cx.op();
        let piece = (op == self.ops.exec || op == self.ops.resume) && (self.top || self.doc);
        let fin = op == self.ops.finish;
        if !piece && !fin {
            return Ok(());
        }
        // (`\end` inside groups: what they own goes to the main list)
        if self.claim_rest {
            self.end_open();
        }
        let raw = self.own_outer_items();
        if fin {
            return self.define_rtn(raw);
        }
        if raw.is_empty() {
            return Ok(());
        }
        // (a piece whose run ended at a candidate: the resume is its to
        // place)
        if self.cut.is_none() {
            self.define_main_tail(&raw)?;
            return self.fold_apart(raw);
        }
        self.fold_here(raw)
    }

    /// The piece's units: its vertical profile ([`crate::vprof::VPROF`])
    /// and its items ([`PITEMS`]), at its position.
    pub(crate) fn contribute_units(
        &mut self,
        raw: &[ValueId],
        last: Option<u32>,
        reads: ValueId,
        defs: ValueId,
        locals: Vec<ValueId>,
    ) {
        let v = self.v;
        let (ints, marks) = crate::vprof::encode_marks(v, raw);
        crate::vprof::count_unit(&ints);
        let pid = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
        // (its vertical profile's unit, the version a page builder folds,
        // in its items' unit too: a finisher placed on another version
        // knows its placement stale, [`Run::finisher_args`])
        let vunit = crate::vprof::unit(v, pid, &ints, marks);
        // (decoded here, beside the other pieces, not on the steps' chain)
        let _ = crate::vprof::decoded(v, vunit);
        self.contribute(crate::vprof::VPROF, v.seq_unit(vunit));
        let shape = crate::nodes::unit_shape(v, pid, raw, &self.line_fonts).unwrap_or(ValueId::UNIT);
        let it = v.list(vec![
            v.int(pid),
            v.int(last.map_or(-1, i64::from)),
            v.list(raw.to_vec()),
            reads,
            defs,
            v.list(locals),
            // (unused)
            v.int(0),
            // (the items' shape, their lines sealed: what a PDF page's
            // finisher reads, `crate::pages::seal`; none: the items)
            shape,
            vunit,
        ]);
        self.contribute(PITEMS, v.seq_unit(it));
        self.contribute(crate::pages::LINK_ITEMS, it);
    }

    /// What the piece leaves the next one of the main list
    /// ([`crate::build::MAIN_PD`], [`crate::build::MAIN_SF`],
    /// [`crate::build::MAIN_LAST`]): each defined where its items change
    /// it, from its items alone where they decide it (equal folded onto
    /// the document's start and onto a start no document has), else
    /// folded onto the one value reaching the piece, read: a paragraph's
    /// end is its own, and the pieces read one another's only where an
    /// item leaves the previous standing (spec-redesign §1, exec row).
    pub(crate) fn define_main_tail(&mut self, raw: &[ValueId]) -> R<()> {
        use crate::build::{MAIN_LAST, MAIN_PD, MAIN_SF, MAIN_SINCE, MainTail};
        let v = self.v;
        if raw.is_empty() {
            return Ok(());
        }
        let p = MainTail::probe(v, raw.len());
        let a = MainTail::START.after(v, raw);
        let b = p.after(v, raw);
        // (each field: the items' own value where they decide it;
        // nothing where they leave it as it was, the definition before
        // reaching on; else the value reaching here, read, folded)
        if a.pd == b.pd {
            self.cx
                .define_with(v.field(MAIN_PD), v.int(i64::from(a.pd)), Locality::Global);
        } else if b.pd != p.pd {
            let pd = self
                .eng_read(v.field(MAIN_PD))?
                .map_or(MainTail::START.pd, |x| {
                    i32::try_from(int_of(v, x)).unwrap_or(MainTail::START.pd)
                });
            let t = MainTail {
                pd,
                ..MainTail::START
            }
            .after(v, raw);
            self.cx
                .define_with(v.field(MAIN_PD), v.int(i64::from(t.pd)), Locality::Global);
        }
        if a.sf == b.sf {
            self.cx
                .define_with(v.field(MAIN_SF), v.int(i64::from(a.sf)), Locality::Global);
        } else if b.sf != p.sf {
            let sf = self
                .eng_read(v.field(MAIN_SF))?
                .map_or(1000, |x| i32::try_from(int_of(v, x)).unwrap_or(1000));
            let t = MainTail {
                sf,
                ..MainTail::START
            }
            .after(v, raw);
            self.cx
                .define_with(v.field(MAIN_SF), v.int(i64::from(t.sf)), Locality::Global);
        }
        // (the last item and the contributions since the last candidate
        // apart: a piece that changes one and leaves the other reads
        // nothing of the piece before)
        let last_own = a.last == b.last;
        let since_own = a.since == b.since;
        if last_own {
            self.cx
                .define_with(v.field(MAIN_LAST), a.last_value(), Locality::Global);
        }
        if since_own {
            self.cx
                .define_with(v.field(MAIN_SINCE), a.since_value(v), Locality::Global);
        }
        let last_moves = !last_own && b.last != p.last;
        let since_moves = !since_own && b.since != p.since;
        // (the count moved by the items' own number alone, no candidate
        // among them and nothing before reached into, `\\bigskip` after
        // a paragraph: at least that many, the count before not read,
        // `MainTail::since_at_least`)
        let adds = since_moves && !last_moves && b.since.checked_sub(p.since) == Some(a.since);
        if adds {
            self.cx.define_with(
                v.field(MAIN_SINCE),
                MainTail::since_at_least(v, a.since),
                Locality::Global,
            );
        }
        if last_moves || (since_moves && !adds) {
            let last = self.eng_read(v.field(MAIN_LAST))?;
            let since = self.eng_read(v.field(MAIN_SINCE))?;
            let (last, since, at_least) = MainTail::of_last(v, last, since);
            let t = if at_least {
                // (a deletion reaching back past a count known only as
                // a lower bound: the list itself, a real dependency)
                self.main_tail_full(raw)?
            } else {
                let pd = self
                    .eng_read(v.field(MAIN_PD))?
                    .map_or(MainTail::START.pd, |x| {
                        i32::try_from(int_of(v, x)).unwrap_or(MainTail::START.pd)
                    });
                MainTail {
                    last,
                    since,
                    pd,
                    ..MainTail::START
                }
                .after(v, raw)
            };
            if last_moves {
                self.cx
                    .define_with(v.field(MAIN_LAST), t.last_value(), Locality::Global);
            }
            if since_moves {
                self.cx
                    .define_with(v.field(MAIN_SINCE), t.since_value(v), Locality::Global);
            }
        }
        Ok(())
    }

    /// The candidate the run passed last, when nothing happened after
    /// it: a page finishing there finishes at the piece's end. A group
    /// that closed after it is something (its local definitions, in force
    /// at the candidate, are gone at the end: amsbook's
    /// `\cleardoublepage` is `\clearpage{\pagestyle{empty}...\newpage}`).
    pub(crate) fn last_candidate(&self) -> Option<u32> {
        self.cand_marks.last().and_then(|m| {
            (m.stamp == self.st.stamp()
                && m.sig == self.st.sig()
                && m.gids == self.st.gid_mark()
                && m.nest == self.nest.len()
                && m.contribs == self.contribs.len()
                && m.kids == self.kids.len()
                && m.lines == self.lines_entered)
                .then_some(m.k)
        })
    }

    /// What a finisher needs when a page can finish inside the piece
    /// with commands after it ([`Run::preload_candidate`],
    /// [`Run::give_way`]): per candidate the names the run defined after
    /// it (and its local definitions at any time: a scope closing after
    /// the candidate hides them from a finisher placed after the piece).
    /// The first part is unit. Units otherwise.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn check_data(&mut self, last: Option<u32>) -> (ValueId, ValueId) {
        let v = self.v;
        let n = self.cand_marks.len();
        let inside_possible = !(n == 1 && last == Some(0)) && n > 0;
        if !inside_possible {
            return (ValueId::UNIT, ValueId::UNIT);
        }
        let gids = self.st.gid_mark();
        let mut locals: Vec<(Loc, Field)> = self
            .st
            .defined_local_since(0, gids)
            .into_iter()
            .map(|l| (l, self.loc_field(l)))
            .collect();
        // (by field once for every candidate: a candidate's names are
        // these merged with its own, the first of a field kept)
        locals.sort_unstable_by_key(|x| (x.1.0.raw(), x.0));
        // (every definition of the run, by field once, a standing one
        // first: a candidate's are those made after it, filtered in
        // order, [`State::defined_since_in_order`])
        let mut defs: Vec<RunDef> = self
            .st
            .defined_stamped()
            .into_iter()
            .map(|(l, stamp, local, gid)| {
                (
                    self.loc_field(l),
                    self.st.def_stands(l),
                    l,
                    stamp,
                    local,
                    gid,
                )
            })
            .collect();
        defs.sort_by_key(|x| (x.0.0.raw(), !x.1));
        let marks = self.cand_marks.clone();
        // (a location defined after a candidate with its value there,
        // where the run had it: the page's routine, which runs at the
        // candidate, reads that, [`Run::preload_candidate`]; no value:
        // the routine reads it as it reaches the finisher)
        let at = self.st.at_candidates(v);
        // (each location some candidate holds, by field (then location),
        // with its value where the run ends: a candidate's restored
        // bindings are these filtered, in order)
        let mut held: Vec<(Field, Loc, Option<ValueId>)> = Vec::new();
        for l in at.locs() {
            let f = self.loc_field(l);
            held.push((f, l, self.st.id_of(l, v)));
        }
        held.sort_unstable_by_key(|x| (x.0.0.raw(), x.1));
        // (a candidate's `[value]` lists, one per value: the same value
        // stands at many candidates)
        let kvs: std::cell::RefCell<phitex_defuse::FxHashMap<Option<ValueId>, ValueId>> =
            std::cell::RefCell::default();
        let kv_list = |x: Option<ValueId>| {
            *kvs.borrow_mut()
                .entry(x)
                .or_insert_with(|| v.list(x.into_iter().collect()))
        };
        let before = v.str(BEFORE_PIECE);
        // (`[value]` or `[]` (absent) where the run had it; one it had
        // neither read nor defined before is what reached the piece,
        // [`BEFORE_PIECE`], the finisher reads it there)
        let kv_at = |i: usize, l: Loc| match at.get(i, l) {
            Some(Some(x)) => kv_list(x),
            Some(None) => before,
            None => ValueId::UNIT,
        };
        // (the files written after each candidate)
        let streams: Vec<Vec<Field>> = marks
            .iter()
            .map(|m| self.streams_since(m.contribs))
            .collect();
        let is_last: Vec<bool> = marks.iter().map(|m| Some(m.k) == last).collect();
        // (with whether the run could end there: a finisher of a page
        // firing at a candidate where it could not runs its routine as if
        // there, and the piece's definitions after it stand)
        let cuts: Vec<ValueId> = marks
            .iter()
            .zip(&is_last)
            .map(|(m, &l)| v.int(i64::from(m.cuttable && !l)))
            .collect();
        let src = NameSources {
            n,
            marks: &marks,
            is_last: &is_last,
            locals: &locals,
            defs: &defs,
            held: &held,
            streams: &streams,
            at: &at,
        };
        // (the candidates in the run's order, their stamps, groups and
        // contributions growing: each name's stretches found from where
        // its sources change, not from every candidate's names)
        let monotone = marks.windows(2).all(|w| {
            w[0].stamp <= w[1].stamp && w[0].gids <= w[1].gids && w[0].contribs <= w[1].contribs
        });
        let mut stretches = if monotone {
            src.by_field(&kv_at)
        } else {
            src.by_candidate(&kv_at)
        };
        if crate::exec::obs_flag!("PHITEX_STRETCHES_CHECK") && monotone {
            let mut a = stretches.clone();
            let mut b = src.by_candidate(&kv_at);
            a.sort_unstable_by_key(|x| (x.0.raw(), x.3));
            b.sort_unstable_by_key(|x| (x.0.raw(), x.3));
            assert!(
                a == b,
                "check data stretches differ: {} by field, {} by candidate",
                a.len(),
                b.len()
            );
        }
        // (by field, then by the stretch's first candidate: one list
        // whatever order the run met them in)
        stretches.sort_unstable_by_key(|x| (x.0.raw(), x.3));
        // (one list of the stretches in a row, five values each: no list
        // made per stretch)
        // (and the stretches without their values, five values each, the
        // value's kind in its place, [`names_index_at`]: a finisher reads
        // which names stand at its candidate through this, and a name's
        // value only where it uses it, [`Run::fin_lazy_value`])
        let n = |x: usize| v.int(i64::try_from(x).unwrap_or(i64::MAX));
        let mut index = Vec::with_capacity(stretches.len() * 5);
        for &(f, g, kv, lo, hi) in &stretches {
            index.extend([f, v.int(i64::from(g)), v.int(kv_kind(v, kv)), n(lo), n(hi)]);
        }
        let stretches = stretches
            .into_iter()
            .flat_map(|(f, g, kv, lo, hi)| [f, v.int(i64::from(g)), kv, n(lo), n(hi)])
            .collect();
        (
            ValueId::UNIT,
            v.list(vec![v.list(stretches), v.list(cuts), v.list(index)]),
        )
    }

    /// A piece's end when no page needs finding in its own run: its
    /// insertion class markers given the registers where it ends, its
    /// units. Its page parameter markers stay `[tag]`: the page builder
    /// reads the parameters reaching it where it is ([`Run::page_params_at`]),
    /// so a piece reads none (an output routine setting `\vsize` would
    /// refute every piece after it).
    pub(crate) fn fold_apart(&mut self, mut raw: Vec<ValueId>) -> R<()> {
        let last = self.last_candidate();
        // (what its commands read, before the reads that resolve its
        // markers: those are no command's)
        let (reads, defs) = self.check_data(last);
        // (its insertion class markers that are not its own stay `[tag,
        // n]`: read where the piece is by the step that folds it,
        // [`Run::resolve_params_of`], as its page parameters are; a
        // piece with a footnote does not wait for the routine before it
        // that empties the class's box)
        self.pin_reaching_page_params(&mut raw)?;
        self.contribute_units(&raw, last, reads, defs, Vec::new());
        Ok(())
    }

    /// A piece whose run ended at a candidate ([`Run::finish_candidate`]):
    /// it gives its items up to the candidate as its units (the page is
    /// the page builder's to find, as for any piece) and places the
    /// resume piece after it, which runs the rest from the continuation,
    /// its arguments ([`Run::cont_encode`]). Nothing here reads the page:
    /// where the cut is is the piece's text's alone. A stop here stops
    /// the run: the end is a command of its own ([`Run::begin_command`]).
    pub(crate) fn fold_here(&mut self, raw: Vec<ValueId>) -> R<()> {
        let t0 = std::time::Instant::now();
        let r = self.fold_here_(raw);
        phitex_pdf::phase::add(18, t0);
        r
    }

    pub(crate) fn fold_here_(&mut self, mut raw: Vec<ValueId>) -> R<()> {
        let v = self.v;
        // (its input read up to the candidate: the siblings it takes are
        // known, and so are the fields it defines)
        let n = self.claim_count();
        if n != 0 {
            self.cx.claim(n);
        }
        let mut defs = self.published_fields();
        defs.extend(PAGE_FIELDS.map(|f| v.field(f)));
        self.cx.narrow(MayDefine::Names(defs));
        self.begin_command();
        self.define_main_tail(&raw)?;
        let Some(k) = self.cut else { return Ok(()) };
        if crate::exec::obs_flag!("PHITEX_TEX_TRACE") {
            eprintln!("  cut {}: at candidate {k}", self.cx.node().raw());
        }
        // (what a page firing at an earlier candidate reads there: the
        // names the run defined after it, its locals at the cut being
        // later than it, `Run::preload_candidate`)
        let (reads, defs) = self.check_data(Some(k));
        self.pin_reaching_page_params(&mut raw)?;
        let (cont, locals) = self.cont_encode()?;
        self.contribute_units(&raw, Some(k), reads, defs, locals);
        let flags = i64::from(self.top) | (i64::from(self.doc) << 1);
        let pid = v.int(i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes()));
        // (the siblings the rest of the piece will take where the piece's
        // text says so: what it takes less what it took before the cut;
        // the nodes after the resume run on that instead of waiting for
        // the resume's run, and a run that takes others restructures
        // them; unknown, -1: decided by the resume's run)
        let rest = self
            .claim_guess()
            .map_or(-1, |k| i64::try_from(k.saturating_sub(n)).unwrap_or(-1));
        // (the rest of the piece after it, its continuation its arguments:
        // the finisher of a page firing at the cut, or at a candidate of
        // the piece before it, the page builder places right after the
        // piece, so before the rest, which reads what their routines
        // leave; a `\write18`'s tool node the run ended at right before
        // it, `Run::tool_command`)
        if let Some(t) = self.tool_cut {
            self.cx.follow(self.ops.toolnode, list_of(v, t));
        }
        self.cx.follow(
            self.ops.resume,
            vec![cont, v.int(flags), pid, v.int(rest), v.int(i64::from(k))],
        );
        Ok(())
    }

    /// A finisher whose page fired at a candidate inside its piece (one
    /// the piece could not end at, a group or conditional of its own
    /// open there): the routine ran on the piece's state at the
    /// candidate ([`Run::preload_candidate`]); the piece's definitions
    /// after the candidate that stand at its end are later in TeX's order
    /// than the routine's, so where the routine defined one of them
    /// globally, the piece's stands (the value reaching here, put back).
    /// (Arguments past the fourth: the rest of the piece's items, the
    /// piece's node, the candidate, unused, what it defined after the
    /// candidate.)
    pub(crate) fn give_way(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        if !self.fin.as_ref().and_then(|c| c.brk).is_some_and(|b| b.2) {
            return Ok(());
        }
        let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
        if args.len() < 9 || a(5) == ValueId::UNIT {
            return Ok(());
        }
        let gdefs: Vec<ValueId> = self
            .st
            .published(v, self.rec_load)
            .into_iter()
            .filter(|d| d.2 == Locality::Global)
            .map(|d| self.loc_field(d.0).0)
            .collect();
        let mut restore = Vec::new();
        for e in list_of(v, a(8)) {
            let xs = list_of(v, e);
            let Some(&f) = xs.first() else { continue };
            let stands = xs.get(1).is_some_and(|&g| int_of(v, g) != 0);
            if stands && gdefs.contains(&f) {
                restore.push(Field(f));
            }
        }
        for f in restore {
            let x = self.eng_read(f)?;
            let loc = self.loc_of(f);
            self.set_loc(loc, crate::runstate::Entry::raw(x), true);
        }
        Ok(())
    }
}

// ------------------------------------------------------ continuations

/// A line of the input left, as a text segment of a node's arguments
/// (`decode_seg`): the characters from its position on, its state, and
/// whether its end is still due.
pub(crate) fn line_frame(v: &Values, s: &Line) -> Option<ValueId> {
    let end_due = s.ends && s.endch < -1;
    if s.loc >= s.buf.len() && !end_due {
        return None;
    }
    let text: String = s.buf[s.loc.min(s.buf.len())..]
        .iter()
        .map(|&c| char::from_u32(c).unwrap_or('?'))
        .collect();
    let starts = s.starts && s.endch == -2;
    let mut xs = vec![
        v.str("s"),
        v.str(&text),
        v.int(i64::from(s.state)),
        v.int(-2),
        v.int(i64::from(starts) | (i64::from(end_due) << 1) | (i64::from(s.rescanned) << 2)),
    ];
    if let Some((name, at, unit)) = s.origin {
        let at = at + s.loc.min(s.buf.len());
        xs.push(name);
        xs.push(v.int(i64::try_from(at).unwrap_or(0)));
        if s.lineno > 0 || unit != ValueId::UNIT {
            xs.push(v.int(i64::from(s.lineno)));
        }
        // (a unit's line: its place from the unit's start, the unit named)
        if unit != ValueId::UNIT {
            xs.push(unit);
        }
    }
    Some(v.list(xs))
}

pub(crate) fn file_frames(v: &Values, f: &crate::run::FileRest) -> Vec<ValueId> {
    // (a file read as units, not read yet: the file as its units, a lazy
    // file again where the frames are decoded, `crate::run::units_frame`)
    if let Some(u) = crate::run::units_frame(v, f) {
        return vec![u];
    }
    vec![v.list(vec![
        v.str("f"),
        f.id,
        v.int(i64::try_from(f.off).unwrap_or(0)),
        f.name,
    ])]
}

pub(crate) fn opt_val(x: Option<ValueId>) -> ValueId {
    x.unwrap_or(ValueId::UNIT)
}

pub(crate) fn gid_int(v: &Values, g: Option<u32>) -> ValueId {
    v.int(g.map_or(-1, i64::from))
}

pub(crate) fn int_gid(k: i64) -> Option<u32> {
    (k >= 0).then(|| u32::try_from(k).unwrap_or(0))
}

impl Run<'_, '_> {
    /// The run stopped at a candidate, as a value for the resume piece:
    /// the input left (the token levels' rests, top first, then the text
    /// not reached), the levels of the nest with their lists, the
    /// conditionals open, the state table's groups (their local bindings
    /// and saves), the base-level values of the locations those bindings
    /// hide (the resume checks them against the engine's: a global
    /// definition in between stands, §283), and the rest of the run's
    /// counters. Also the group-local bindings themselves, for the
    /// finisher.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn cont_encode(&mut self) -> R<(ValueId, Vec<ValueId>)> {
        let v = self.v;
        // (each token level's file line: of the innermost file read
        // below it, §304, a file it is inside staying open until the
        // level is read to its end; below them all, the text's)
        let here_of = |c: Option<crate::run::FileLine>| -> Vec<ValueId> {
            match c {
                Some(crate::run::FileLine::Known(n)) => vec![ValueId::UNIT, v.int(n)],
                Some(crate::run::FileLine::Piece(name)) => vec![name, v.int(-1)],
                None => Vec::new(),
            }
        };
        let base = self.text_line();
        let mut cur = base;
        let mut level_here = Vec::with_capacity(self.input.levels.len());
        for l in &self.input.levels {
            match l.kind {
                crate::input::kind::TOKS => {}
                crate::input::kind::LINE => {
                    if let Some(c) = crate::run::line_of(self.input.line(*l)) {
                        cur = Some(c);
                    }
                }
                _ => {
                    cur = Some(crate::run::FileLine::Known(i64::from(
                        self.input.file(*l).line,
                    )));
                }
            }
            level_here.push(here_of(cur));
        }
        let toks_frame = |toks: &[crate::val::Tok], here: &[ValueId]| {
            let mut t = vec![v.str("t"), toks_value(v, toks)];
            t.extend(here.iter().copied());
            v.list(t)
        };
        let base_here = here_of(base);
        let mut frames = Vec::new();
        for (i, l) in self.input.levels.iter().enumerate().rev() {
            match l.kind {
                crate::input::kind::TOKS => {
                    let r = self.input.arena.get(l.rest());
                    if !r.is_empty() {
                        frames.push(toks_frame(r, &level_here[i]));
                    }
                }
                crate::input::kind::LINE => {
                    if let Some(f) = line_frame(v, self.input.line(*l)) {
                        frames.push(f);
                    }
                }
                _ => frames.extend(file_frames(v, self.input.file(*l))),
            }
        }
        for s in self.input.text_rest() {
            match s {
                Seg::Line(l) => {
                    if let Some(f) = line_frame(v, l) {
                        frames.push(f);
                    }
                }
                Seg::Toks(t) => {
                    if !t.is_empty() {
                        frames.push(toks_frame(t, &base_here));
                    }
                }
                Seg::File(f) => frames.extend(file_frames(v, f)),
            }
        }
        // the state table's groups
        let open: Vec<u32> = self
            .nest
            .iter()
            .filter(|l| l.group != 0)
            .map(|l| l.gid)
            .collect();
        if let Some(name) = crate::exec::obs_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(name));
            eprintln!(
                "cont {} at a cut: {name}: {} (open groups {open:?})",
                self.cx.node().raw(),
                self.st.debug_marks(l)
            );
        }
        let c = self.st.cont(v, &open);
        // (the stamps and the group numbers the continuation carries, by
        // their order only: a resume compares them with each other and
        // with its own, all later; what a run made before them, or how
        // many groups it opened and closed, does not change it)
        let mut stamps: Vec<u64> = vec![c.stamp];
        let mut gids: Vec<u32> = vec![c.next_gid];
        for l in &self.nest {
            stamps.push(l.open_stamp);
            gids.push(l.gid);
        }
        for x in &c.slots {
            stamps.extend([x.save_stamp, x.ginert]);
            gids.push(x.save_gid);
            if let Some((st, _, g)) = x.ldef {
                stamps.push(st);
                gids.extend(g);
            }
        }
        for x in &c.save {
            stamps.extend([x.stamp, x.at, x.ldef.0]);
            gids.push(x.gid);
            gids.extend(x.ldef.2);
        }
        stamps.sort_unstable();
        stamps.dedup();
        gids.sort_unstable();
        gids.dedup();
        // (0 stays 0: no stamp, no group)
        let st_of = |x: u64| -> u64 {
            if x == 0 {
                0
            } else {
                u64::try_from(stamps.partition_point(|&y| y < x)).unwrap_or(0) + u64::from(stamps[0] != 0)
            }
        };
        let gid_of = |x: u32| -> u32 {
            if x == 0 {
                0
            } else {
                u32::try_from(gids.partition_point(|&y| y < x)).unwrap_or(0) + u32::from(gids[0] != 0)
            }
        };
        let nest: Vec<ValueId> = self
            .nest
            .iter()
            .map(|l| {
                let (lang, packed) = l.par.map_or((ValueId::UNIT, ValueId::UNIT), |(a, b)| {
                    (v.int(a), v.int(b))
                });
                v.list(vec![
                    v.int(i64::from(l.mode)),
                    v.int(l.group),
                    v.int(l.lid),
                    v.int(i64::from(l.owns)),
                    v.list(l.list.iter().map(|x| x.to_value(v)).collect()),
                    v.int(i64::from(l.fold.sf)),
                    v.int(i64::from(l.fold.pd)),
                    v.int(i64::try_from(l.fold.since_bp).unwrap_or(0)),
                    lang,
                    packed,
                    v.int(l.prev_graf),
                    v.list(l.extra.clone()),
                    v.int(i64::from(gid_of(l.gid))),
                    v.int(i64::try_from(l.save_base).unwrap_or(0)),
                    v.int(i64::try_from(l.def_base).unwrap_or(0)),
                    v.int(i64::from_ne_bytes(st_of(l.open_stamp).to_ne_bytes())),
                    toks_value(v, &l.ag),
                ])
            })
            .collect();
        let conds = v.list(self.conds.iter().map(|&c| v.int(i64::from(c))).collect());
        let codes = v.list(self.if_codes.iter().map(|&c| v.int(i64::from(c))).collect());
        if let Some(name) = crate::exec::obs_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(name));
            let hit = c.slots.iter().find(|s| s.loc == l);
            eprintln!(
                "cont {} encodes {name}: {:?}",
                self.cx.node().raw(),
                hit.map(|s| (
                    s.cur.map(|x| v.render(x)),
                    s.loaded,
                    s.save_gid,
                    s.ldef,
                    s.group_local
                ))
            );
        }
        let mut slots = Vec::new();
        let mut overrides = Vec::new();
        let mut locals = Vec::new();
        for s in &c.slots {
            let f = self.loc_field(s.loc);
            if s.group_local {
                let base = match s.base {
                    Some(x) => x,
                    None => self.eng_read(f)?,
                };
                let after = v.int(self.made_after(s.ldef.map(|d| d.0)));
                overrides.push(v.list(vec![f.0, opt_val(base), after]));
                locals.push(v.list(vec![f.0, opt_val(s.cur), after]));
            }
            let (stamp, local, gid) = s.ldef.map_or((-1, 0, None), |(st, l, g)| {
                (i64::from_ne_bytes(st_of(st).to_ne_bytes()), i64::from(l), g.map(gid_of))
            });
            slots.push(v.list(vec![
                f.0,
                opt_val(s.cur),
                v.int(i64::from(s.loaded)),
                v.int(i64::from(gid_of(s.save_gid))),
                v.int(i64::from_ne_bytes(st_of(s.save_stamp).to_ne_bytes())),
                v.int(i64::from_ne_bytes(st_of(s.ginert).to_ne_bytes())),
                v.int(stamp),
                v.int(local),
                gid_int(v, gid),
            ]));
        }
        let mut saves = Vec::new();
        for s in &c.save {
            let f = self.loc_field(s.loc);
            saves.push(v.list(vec![
                f.0,
                opt_val(s.prev),
                v.int(i64::from(s.prev_loaded)),
                v.int(i64::from(gid_of(s.gid))),
                v.int(i64::from_ne_bytes(st_of(s.stamp).to_ne_bytes())),
                v.int(i64::from_ne_bytes(st_of(s.at).to_ne_bytes())),
                v.int(i64::from_ne_bytes(st_of(s.ldef.0).to_ne_bytes())),
                v.int(i64::from(s.ldef.1)),
                gid_int(v, s.ldef.2.map(gid_of)),
            ]));
        }
        let mut defined = Vec::new();
        for &l in &c.defined {
            defined.push(self.loc_field(l).0);
        }
        let state = v.list(vec![
            v.list(slots),
            v.list(saves),
            v.list(defined),
            v.int(i64::from_ne_bytes(st_of(c.stamp).to_ne_bytes())),
            v.int(i64::from(gid_of(c.next_gid))),
        ]);
        let misc = v.list(vec![
            v.int(self.next_lid),
            v.int(self.last_par.0),
            v.int(self.last_par.1),
            v.int(self.lines_entered),
            v.int(i64::from_ne_bytes(self.lines_read.to_ne_bytes())),
            v.int(i64::from(self.eof)),
            v.int(i64::from(self.radix)),
            v.int(self.cur_order as i64),
            // (the read-time commands' count: a resume keys its own by its
            // node, from its start, as the read-time objects' counts)
            v.int(0),
            // (the run's mutation count is compared only with itself
            // within one command, `Run::rewind`: not carried, an edit
            // before the cut would change the continuation for nothing)
            v.int(0),
            v.int(i64::from(self.read_back_done)),
            v.int(i64::from(self.astate.is_some())),
            v.int(self.astate.unwrap_or(0)),
            v.int(self.aopen),
            v.int(i64::from(self.claim_rest)),
            v.int(i64::from(self.hint_void)),
            v.int(i64::from(self.no_pull)),
            v.int(i64::from(self.rec_load)),
            v.int(i64::from(self.cond_loaded || self.cond_stop)),
        ]);
        let cont = v.list(vec![
            v.list(frames),
            v.list(nest),
            conds,
            codes,
            state,
            misc,
            v.list(overrides),
        ]);
        if crate::exec::obs_flag!("PHITEX_CONT_PARTS") {
            let xs = list_of(v, cont);
            let st = list_of(v, xs[4]);
            eprintln!(
                "cont-parts {} k {}: {:?} state {:?} misc {:?}",
                self.cx.node().raw(),
                self.cand,
                xs.iter().map(|x| x.raw() % 100_000).collect::<Vec<_>>(),
                st.iter().map(|x| x.raw() % 100_000).collect::<Vec<_>>(),
                list_of(v, xs[5])
                    .iter()
                    .map(|&x| int_of(v, x))
                    .collect::<Vec<_>>()
            );
        }
        Ok((cont, locals))
    }

    /// A resume piece's start: the continuation `x` ([`Run::cont_encode`])
    /// loaded; the base-level values the open groups' bindings hide
    /// checked against the engine's reaching here (a global definition
    /// between the cut and here, the finisher's, replaces the binding and
    /// makes the groups' saves inert, §283).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn cont_load(&mut self, x: ValueId, pid: ValueId) -> R<()> {
        let v = self.v;
        let xs = list_of(v, x);
        let a = |i: usize| xs.get(i).copied().unwrap_or(ValueId::UNIT);
        let frames = self.resolve_units(&[a(0)])?;
        self.input.text = crate::run::decode_segs(v, &frames);
        // (where each segment was read: a token frame's file line)
        self.cont_lines = frames
            .first()
            .map(|&l| list_of(v, l))
            .unwrap_or_default()
            .iter()
            .filter(|&&f| crate::run::first_seg(v, &[v.list(vec![f])]).is_some())
            .map(|&f| crate::run::frame_line(v, f))
            .collect();
        self.input.text_at = 0;
        // the state table first: the levels index its saves
        let st = list_of(v, a(4));
        let sa = |i: usize| st.get(i).copied().unwrap_or(ValueId::UNIT);
        let u = |x: ValueId| u64::from_ne_bytes(int_of(v, x).to_ne_bytes());
        let mut slots = Vec::new();
        for e in list_of(v, sa(0)) {
            let ys = list_of(v, e);
            if ys.len() < 9 {
                continue;
            }
            let loc = self.loc_of(Field(ys[0]));
            let stamp = int_of(v, ys[6]);
            slots.push(ContSlot {
                loc,
                cur: some(ys[1]),
                loaded: int_of(v, ys[2]) != 0,
                save_gid: u32::try_from(int_of(v, ys[3])).unwrap_or(0),
                save_stamp: u(ys[4]),
                ginert: u(ys[5]),
                ldef: (stamp >= 0).then(|| {
                    (
                        u64::from_ne_bytes(stamp.to_ne_bytes()),
                        int_of(v, ys[7]) != 0,
                        int_gid(int_of(v, ys[8])),
                    )
                }),
                base: None,
                group_local: false,
            });
        }
        let mut save = Vec::new();
        for e in list_of(v, sa(1)) {
            let ys = list_of(v, e);
            if ys.len() < 6 {
                continue;
            }
            let loc = self.loc_of(Field(ys[0]));
            save.push(ContSaved {
                loc,
                prev: some(ys[1]),
                prev_loaded: int_of(v, ys[2]) != 0,
                gid: u32::try_from(int_of(v, ys[3])).unwrap_or(0),
                stamp: u(ys[4]),
                at: u(ys[5]),
                ldef: if ys.len() >= 9 {
                    (u(ys[6]), int_of(v, ys[7]) != 0, int_gid(int_of(v, ys[8])))
                } else {
                    (0, false, None)
                },
            });
        }
        let mut defined = Vec::new();
        for f in list_of(v, sa(2)) {
            defined.push(self.loc_of(Field(f)));
        }
        // (the locations above were computed in the run's first state:
        // a field past the static cells has its location there, kept)
        let mut ns = State::from_cont(ContState {
            slots,
            save,
            defined,
            stamp: u(sa(3)),
            next_gid: u32::try_from(int_of(v, sa(4))).unwrap_or(0),
        });
        ns.adopt_spill(&self.st);
        self.st = ns;
        if let Some(name) = crate::exec::obs_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(name));
            eprintln!(
                "cont {} loaded: {name}: {}",
                self.cx.node().raw(),
                self.st.debug_marks(l)
            );
        }
        // the fields the finishers between the cut and this resume
        // defined globally (`gdefs`, the window after the cutting node),
        // read only where a group's binding is open at the cut: they
        // decide nothing else here, and a finisher placed between would
        // run this resume again for nothing
        let hidden = list_of(v, a(6));
        let gdefs: Vec<GDef> = match Some(pid).filter(|&p| p != ValueId::UNIT) {
            Some(p) if self.cx.op() == self.ops.resume && !hidden.is_empty() => {
                self.gdefs_after(int_of(v, p))?
            }
            _ => Vec::new(),
        };
        let piece = int_of(v, pid);
        for e in hidden {
            let ys = list_of(v, e);
            if ys.len() < 2 {
                continue;
            }
            let f = Field(ys[0]);
            let x0 = some(ys[1]);
            let x1 = self.eng_read(f)?;
            let after = ys.get(2).map_or(-1, |&c| int_of(v, c));
            // (§279: a global definition replaces the group's local value
            // whatever it defines, the value outside the group too: by
            // the definition, not the value; a routine's at a candidate
            // the binding was made after left it standing)
            if replaced(&gdefs, f, piece, after) {
                let loc = self.loc_of(f);
                self.st.override_global(loc, x1);
                continue;
            }
            if crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
                eprintln!(
                    "page-trace: resume {} override check {}: at the cut {:?}, now {:?}",
                    self.cx.node().raw(),
                    str_of(v, f.0),
                    x0.map(|x| v.render(x)),
                    x1.map(|x| v.render(x))
                );
            }
            if x1 != x0 {
                let loc = self.loc_of(f);
                self.st.override_global(loc, x1);
            }
        }
        for e in list_of(v, a(1)) {
            let ys = list_of(v, e);
            if ys.len() < 17 {
                continue;
            }
            let n = |i: usize| int_of(v, ys[i]);
            let mut l = crate::nest::Level::new(
                i32::try_from(n(0)).unwrap_or(1),
                n(1),
                n(2),
                n(3) != 0,
                list_of(v, ys[11]),
            );
            l.list = crate::nest::of_values(list_of(v, ys[4]));
            l.fold = FoldState {
                sf: i32::try_from(n(5)).unwrap_or(1000),
                pd: i32::try_from(n(6)).unwrap_or(IGNORE_DEPTH),
                since_bp: usize::try_from(n(7)).unwrap_or(0),
            };
            l.par = (ys[8] != ValueId::UNIT).then(|| (n(8), n(9)));
            l.prev_graf = n(10);
            l.gid = u32::try_from(n(12)).unwrap_or(0);
            l.save_base = usize::try_from(n(13)).unwrap_or(0);
            l.def_base = usize::try_from(n(14)).unwrap_or(0);
            l.open_stamp = u(ys[15]);
            l.ag = toks_of(v, Some(ys[16]));
            self.nest.push(l);
        }
        self.conds = list_of(v, a(2))
            .iter()
            .map(|&c| u8::try_from(int_of(v, c)).unwrap_or(0))
            .collect();
        self.if_codes = list_of(v, a(3))
            .iter()
            .map(|&c| i32::try_from(int_of(v, c)).unwrap_or(0))
            .collect();
        let m = list_of(v, a(5));
        let n = |i: usize| m.get(i).map_or(0, |&y| int_of(v, y));
        self.next_lid = n(0);
        self.last_par = (n(1), n(2));
        self.lines_entered = n(3);
        self.lines_read = u64::from_ne_bytes(n(4).to_ne_bytes());
        self.eof = n(5) != 0;
        self.radix = i32::try_from(n(6)).unwrap_or(0);
        self.cur_order = crate::val::order(n(7));
        self.pdf_occ = usize::try_from(n(8)).unwrap_or(0);
        self.muts = 0;
        self.read_back_done = n(10) != 0;
        self.astate = (n(11) != 0).then(|| n(12));
        self.aopen = n(13);
        self.claim_rest = n(14) != 0;
        self.hint_void = n(15) != 0;
        self.no_pull = n(16) != 0;
        self.rec_load = n(17) != 0;
        self.cond_loaded = n(18) != 0;
        Ok(())
    }
}

/// The rest of a piece after a cut: arguments `[cont, flags, piece,
/// claim guess, candidate]`, the continuation ([`Run::cont_encode`]),
/// the piece's text kind, the cutting piece's node, the siblings it
/// takes and the candidate cut at. It runs the input left in the groups
/// left open, as the piece would have gone on, reading what the
/// finishers before it defined.
pub(crate) struct Resume(pub Shared);

impl Operation for Resume {
    /// Layer 2 (docs/spec-layers.md §2): the rest of a cut piece, expansion.
    fn layer(&self) -> u8 {
        2
    }
    fn name(&self) -> &'static str {
        "resume"
    }
    /// Each runs its own text: a counter one steps is not another's.
    fn alike(&self) -> bool {
        false
    }
    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        MayDefine::Any
    }
    /// The brackets its rest's text shows inside the groups it carries
    /// ([`crate::ops::predicted_effect`]).
    fn effect(&self, args: &[ValueId], v: &Values) -> Effect {
        crate::ops::predicted_effect(ops(&self.0), v, args, true)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let _busy = crate::exec::Busy::new(&ops(&self.0).watch, 1, cx.node().raw());
        let args = cx.args().to_vec();
        let o = ops(&self.0);
        let t0 = std::time::Instant::now();
        let out = run_node(cx, o, |r| {
            let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
            r.cont_load(a(0), a(2))?;
            r.run_all()
        });
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") && t0.elapsed().as_millis() > 200 {
            let v = cx.values();
            let text = v.render(args.first().copied().unwrap_or(ValueId::UNIT));
            let cut: String = text.chars().take(600).collect();
            crate::exec::later(format!(
                "longrun: resume {} {:.0} ms: {cut}",
                cx.node().raw(),
                t0.elapsed().as_secs_f64() * 1e3
            ));
        }
        out
    }
}

/// Two lists each sorted by field then standing first, merged in that
/// order, `a`'s before `b`'s at a tie: a stable sort of `a` then `b`.
fn merge_stable(
    a: impl Iterator<Item = (ValueId, bool, ValueId)>,
    b: Vec<(ValueId, bool, ValueId)>,
) -> Vec<(ValueId, bool, ValueId)> {
    let key = |x: &(ValueId, bool, ValueId)| (x.0.raw(), !x.1);
    let mut out = Vec::with_capacity(b.len());
    let mut b = b.into_iter().peekable();
    for x in a {
        while let Some(y) = b.next_if(|y| key(y) < key(&x)) {
            out.push(y);
        }
        out.push(x);
    }
    out.extend(b);
    out
}

/// The kind of a name's value in a piece's check data: 0 none (`UNIT`), 1
/// what reached the piece ([`BEFORE_PIECE`]), 2 a value (`[value]` or
/// `[]`).
fn kv_kind(v: &Values, kv: ValueId) -> i64 {
    if kv == ValueId::UNIT {
        0
    } else if kv == v.str(BEFORE_PIECE) {
        1
    } else {
        2
    }
}

/// The names standing at candidate `k` from a piece's check data's
/// index (the third part, without values): each `(field, standing,
/// kind, stretch)`, the stretch's number in the check data; and the
/// candidates' cut flags' list.
pub(crate) fn names_index_at(v: &Values, index: ValueId, k: u32) -> Vec<(ValueId, ValueId, i64, usize)> {
    let k = i64::from(k);
    list_of(v, index)
        .as_chunks::<5>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, s)| int_of(v, s[3]) <= k && k < int_of(v, s[4]))
        .map(|(i, s)| (s[0], s[1], int_of(v, s[2]), i))
        .collect()
}

/// A definition the run made: its field, whether it stands, its
/// location, its stamp, whether local and the group it was made in
/// ([`State::defined_stamped`]).
type RunDef = (Field, bool, Loc, u64, bool, Option<u32>);

/// A field's name over a stretch of candidates `[lo, hi)`: the field,
/// whether its definition stands, its value there.
type Stretch = (ValueId, bool, ValueId, usize, usize);

/// What a piece's names at its candidates are made of
/// ([`Run::check_data`]): a candidate's names are its locals, the
/// definitions after it, its restored bindings and the files written
/// after it, by field, a standing definition first, the first of a
/// field kept (at a tie, in that order).
struct NameSources<'a> {
    n: usize,
    marks: &'a [CandMark],
    is_last: &'a [bool],
    locals: &'a [(Loc, Field)],
    defs: &'a [RunDef],
    held: &'a [(Field, Loc, Option<ValueId>)],
    streams: &'a [Vec<Field>],
    at: &'a crate::runstate::AtCandidates,
}

/// One source of a field's name ([`NameSources::by_field`]): its rank
/// at a tie (not standing, source, place in it), whether it stands, the
/// candidates it covers and the location its value is read at (none: no
/// value).
struct NameSource {
    rank: (bool, u8, usize),
    stands: bool,
    covers: Vec<(usize, usize)>,
    loc: Option<Loc>,
}

impl NameSources<'_> {
    /// Every candidate's names, each made in full, folded into stretches.
    fn by_candidate(&self, kv_at: &dyn Fn(usize, Loc) -> ValueId) -> Vec<Stretch> {
        let mut open: phitex_defuse::FxHashMap<ValueId, (bool, ValueId, usize, usize)> =
            phitex_defuse::FxHashMap::default();
        let mut stretches: Vec<Stretch> = Vec::new();
        for (i, m) in self.marks.iter().enumerate() {
            if !self.is_last[i] {
                let kv_of = |l: Loc| kv_at(i, l);
                // (a binding local to a group of the run open at the
                // candidate, restored at the group's close after it: the
                // routine runs inside that group and sees the binding,
                // which the run's end no longer holds, §1012)
                let restored =
                    self.held
                        .iter()
                        .filter_map(|&(f, l, end)| match self.at.get(i, l) {
                            Some(Some(x)) if x != end => Some((f.0, false, kv_of(l))),
                            _ => None,
                        });
                let mut streams: Vec<(ValueId, bool, ValueId)> = self.streams[i]
                    .iter()
                    .map(|f| (f.0, false, ValueId::UNIT))
                    .collect();
                streams.sort_by_key(|x| x.0.raw());
                let names = merge_stable(restored, streams);
                let after = self
                    .defs
                    .iter()
                    .filter(|d| d.3 > m.stamp && !(d.4 && d.5.is_some_and(|g| g > m.gids)))
                    .map(|d| (d.0.0, d.1, kv_of(d.2)));
                let own = merge_stable(after, names);
                let locals = self.locals.iter().map(|&(l, f)| (f.0, false, kv_of(l)));
                let mut names = merge_stable(locals, own);
                names.dedup_by_key(|x| x.0);
                for (f, g, kv) in names {
                    match open.get_mut(&f) {
                        Some(o) if o.0 == g && o.1 == kv => o.3 = i,
                        _ => {
                            if let Some(o) = open.insert(f, (g, kv, i, i)) {
                                stretches.push((f, o.0, o.1, o.2, o.3 + 1));
                            }
                        }
                    }
                }
            }
            // (a name this candidate's names lack ends its stretch)
            open.retain(|&f, o| {
                if o.3 == i {
                    return true;
                }
                stretches.push((f, o.0, o.1, o.2, o.3 + 1));
                false
            });
        }
        stretches.extend(open.into_iter().map(|(f, o)| (f, o.0, o.1, o.2, o.3 + 1)));
        stretches
    }

    /// The stretches field by field, the candidates in the run's order
    /// (their stamps, groups and contributions growing): a definition
    /// covers the candidates before it that its group is open at, a
    /// value changes only where a candidate holds the location, so a
    /// field's name is looked at only where one of its sources changes.
    fn by_field(&self, kv_at: &dyn Fn(usize, Loc) -> ValueId) -> Vec<Stretch> {
        let n = self.n;
        let lasts: Vec<usize> = (0..n).filter(|&i| self.is_last[i]).collect();
        let mut out: Vec<Stretch> = Vec::new();
        let mut cuts: Vec<usize> = Vec::new();
        for (f, mut srcs) in self.sources() {
            srcs.sort_by_key(|s| s.rank);
            cuts.clear();
            cuts.extend([0, n]);
            for &i in &lasts {
                cuts.extend([i, i + 1]);
            }
            for s in &srcs {
                for &(a, b) in &s.covers {
                    cuts.extend([a, b]);
                }
                if let Some(l) = s.loc {
                    cuts.extend(self.at.history(l).iter().map(|x| x.0 + 1));
                }
            }
            cuts.retain(|&c| c <= n);
            cuts.sort_unstable();
            cuts.dedup();
            let mut cur: Option<Stretch> = None;
            for w in cuts.windows(2) {
                let (a, b) = (w[0], w[1]);
                let name = if self.is_last[a] {
                    None
                } else {
                    srcs.iter()
                        .find(|s| s.covers.iter().any(|&(lo, hi)| lo <= a && a < hi))
                        .map(|s| (s.stands, s.loc.map_or(ValueId::UNIT, |l| kv_at(a, l))))
                };
                match (&mut cur, name) {
                    (Some(c), Some((g, kv))) if c.1 == g && c.2 == kv && c.4 == a => c.4 = b,
                    (_, name) => {
                        if let Some(c) = cur.take() {
                            out.push(c);
                        }
                        cur = name.map(|(g, kv)| (f, g, kv, a, b));
                    }
                }
            }
            if let Some(c) = cur {
                out.push(c);
            }
        }
        out
    }

    /// Each field's sources, with the candidates each covers.
    fn sources(&self) -> phitex_defuse::FxHashMap<ValueId, Vec<NameSource>> {
        let n = self.n;
        let mut by: phitex_defuse::FxHashMap<ValueId, Vec<NameSource>> =
            phitex_defuse::FxHashMap::default();
        for (k, &(l, f)) in self.locals.iter().enumerate() {
            by.entry(f.0).or_default().push(NameSource {
                rank: (true, 0, k),
                stands: false,
                covers: vec![(0, n)],
                loc: Some(l),
            });
        }
        for (k, d) in self.defs.iter().enumerate() {
            // (made after the candidate: a stamp past the candidate's)
            let hi = self.marks.partition_point(|m| m.stamp < d.3);
            // (a local one of a group opened after the candidate: left
            // out there)
            let lo = match d.5 {
                Some(g) if d.4 => self.marks.partition_point(|m| m.gids < g),
                _ => 0,
            };
            by.entry(d.0.0).or_default().push(NameSource {
                rank: (!d.1, 1, k),
                stands: d.1,
                covers: if lo < hi { vec![(lo, hi)] } else { Vec::new() },
                loc: Some(d.2),
            });
        }
        for (k, &(f, l, end)) in self.held.iter().enumerate() {
            let mut covers = Vec::new();
            let mut from = 0;
            for &(j, x) in self.at.history(l) {
                if let Some(x) = x
                    && x != end
                {
                    covers.push((from, j + 1));
                }
                from = j + 1;
            }
            by.entry(f.0).or_default().push(NameSource {
                rank: (true, 2, k),
                stands: false,
                covers,
                loc: Some(l),
            });
        }
        let mut written: phitex_defuse::FxHashMap<ValueId, Vec<(usize, usize)>> =
            phitex_defuse::FxHashMap::default();
        for (i, fs) in self.streams.iter().enumerate() {
            for f in fs {
                let c = written.entry(f.0).or_default();
                match c.last_mut() {
                    Some(r) if r.1 == i => r.1 = i + 1,
                    _ => c.push((i, i + 1)),
                }
            }
        }
        for (f, covers) in written {
            by.entry(f).or_default().push(NameSource {
                rank: (true, 3, 0),
                stands: false,
                covers,
                loc: None,
            });
        }
        by
    }
}
