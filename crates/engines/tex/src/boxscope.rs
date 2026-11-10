//! Lists left open across nodes (docs/spec-slices.md §6, §6.1; on by
//! default, `PHITEX_BOXSCOPE=0` turns it off).
//!
//! A run at rest at its text's end with lists of its own open (a box
//! being built, the paragraph the box is in) ends there instead of
//! reading on: what is inside the box is cut into slices like any text,
//! and each is a node of its own.
//!
//! Lists and groups are two stacks in TeX (the nest and the save stack,
//! §211, §268): a paragraph begun inside a group goes on after the group
//! closes, and `\par` inside a group ends the paragraph with the group
//! still open. So they are left apart:
//!
//! - every group open (a box's own group among them) is an engine scope,
//!   as [`Run::leave_scope`] leaves one; its kind carries the mode and
//!   the id of the list it is in;
//! - every list open gets an id no list open at the same time has (from
//!   the scope depth it is left at, [`global_lid`]: not from the opener's
//!   node, so a re-cut of the opener changes no kind); its items are
//!   contributions to its own accumulator `bitems:<id>`, the opener's a
//!   start entry holding what it had; its reader folds the field and
//!   takes the entries from the last start on ([`Run::left_list_fold`]):
//!   nothing it reads names the opener, so an opener run again with the
//!   same items wakes no reader of the list;
//! - a paragraph, which no group brackets, is left only under a group,
//!   its own parameters the field `par:<list it is in>`: open while that
//!   field names it. A group's kind in it reads the field
//!   ([`Run::par_kind`]), and so does a kind of the list it is in
//!   ([`Run::vert_kind`]): after the close of the group above it (the
//!   picture's environment) the following nodes are in it, with no
//!   continuation; the node at its `\par` breaks it
//!   ([`Run::end_left_paragraph`]).

use crate::build::{FoldState, Mode, fold_item, sf_value};
use crate::run::{R, Run};
use crate::val::{group, int_of, list_of};
use phitex_defuse::{NodeId, ValueId};

/// The ids of lists left open are at or below this.
const BASE: i64 = -(1i64 << 55);

/// Whether open lists may be left to the following nodes: on, unless
/// `PHITEX_BOXSCOPE=0` (or empty).
pub(crate) fn on() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var_os("PHITEX_BOXSCOPE").is_none_or(|v| !v.is_empty() && v != "0"))
}

/// Whether `lid` is a list left open by some node.
pub(crate) fn is_global(lid: i64) -> bool {
    lid <= BASE
}

/// The id of the `k`-th list a run at scope depth `depth` leaves open.
/// Lists open at once are left at different depths (a run inside one
/// runs inside its groups' scopes), but for a paragraph left open, which
/// outlives them: [`first_free`].
fn global_lid(depth: i64, k: usize) -> i64 {
    BASE - depth.clamp(0, 1 << 40) * 64 - i64::try_from(k).unwrap_or(63).min(63)
}

/// The first `k` free for lists left at `depth` by a run whose base is
/// `base`: past a paragraph left open at that depth that the run is in
/// ([`Run::vert_kind`]; it outlives the scopes of the run that left
/// it, so a later run at its depth leaves lists while it is open:
/// tikz2d-fr's second picture in a `tcolorbox`, its box given the
/// paragraph's id, its start resetting the paragraph's items).
fn first_free(depth: i64, base: &Mode) -> usize {
    if is_global(base.lid) {
        let off = BASE - base.lid;
        if off / 64 == depth.clamp(0, 1 << 40) {
            return usize::try_from(off % 64 + 1).unwrap_or(63);
        }
    }
    0
}

/// The name of the token that appends a box closed with the engine
/// scope it was in, the closed list's id after it
/// ([`Run::box_end_outside`]).
pub(crate) const BOX_END: &str = "\u{10FFFE}boxend:";

/// The token's `chr` (`cmd::IGNORE`; an alignment's `endv` has its
/// column there, never negative).
pub(crate) const BOX_END_CHR: i32 = -0xb0e;

fn box_end_field(lid: i64) -> String {
    format!("boxend:{lid}")
}

fn par_field(vlid: i64) -> String {
    format!("par:{vlid}")
}

/// The prefix of the accumulators of lists left open.
pub(crate) const ITEMS_PREFIX: &str = "bitems:";

fn items_field(lid: i64) -> String {
    format!("{ITEMS_PREFIX}{lid}")
}

/// The head of a list's start entry.
const START: &str = "\u{0}bstart";

/// The fold of a left list's accumulator: values `[reset, entries]`
/// (`reset` 1: the entries begin with a start); a start discards what
/// came before it, so the fold at a reader is its list's entries from its
/// opener's start on, whatever lists of the same id ended before it, and
/// a change in those leaves it equal (no reader of this list asked
/// again). Associative: `[r, s]` then `[1, t]` is `[1, t]`, then `[0, t]`
/// is `[r, s ++ t]`.
pub(crate) struct ResetMonoid;

impl phitex_defuse::Monoid for ResetMonoid {
    fn empty(&self, v: &phitex_defuse::Values) -> ValueId {
        v.list(vec![v.int(0), ValueId::NIL])
    }

    fn combine(&self, v: &phitex_defuse::Values, a: ValueId, b: ValueId) -> ValueId {
        let (_, sb) = parts(v, b);
        if reset_of(v, b) {
            return b;
        }
        if sb == ValueId::NIL {
            return a;
        }
        let (ra, sa) = parts(v, a);
        v.list(vec![v.int(i64::from(ra)), v.seq_concat(sa, sb)])
    }
}

/// Where a scope kind was defined, for the walk to its parents
/// ([`Run::par_kind`]).
#[derive(Clone, Copy)]
pub(crate) enum KindFrom {
    /// The scope this node runs in (its opener read before this node).
    Here,
    /// By this node.
    Node(NodeId),
    /// Not known: its parents are not walked.
    Unknown,
}

/// A list left open folded so far by this run ([`Run::left_list_fold`]).
pub(crate) struct LeftMemo {
    lid: i64,
    base: ValueId,
    n: usize,
    last: ValueId,
    st: FoldState,
    out: Vec<ValueId>,
}

/// The entries `seq` of a list left open folded into `st` and `out`: a
/// start record's items, any other item itself.
fn fold_seq(
    v: &phitex_defuse::Values,
    lid: i64,
    start: ValueId,
    seq: ValueId,
    st: &mut FoldState,
    out: &mut Vec<ValueId>,
) {
    for x in v.seq_to_vec(seq) {
        let xs = list_of(v, x);
        if xs.first() == Some(&start) {
            for &y in &xs[1..] {
                fold_item(v, lid, y, st, out);
            }
        } else {
            fold_item(v, lid, x, st, out);
        }
    }
}

fn reset_of(v: &phitex_defuse::Values, x: ValueId) -> bool {
    parts(v, x).0
}

fn parts(v: &phitex_defuse::Values, x: ValueId) -> (bool, ValueId) {
    let xs = list_of(v, x);
    (
        xs.first().is_some_and(|&r| int_of(v, r) == 1),
        xs.get(1).copied().unwrap_or(ValueId::NIL),
    )
}

fn is_box_group(g: i64) -> bool {
    matches!(
        g,
        group::HBOX | group::ADJUSTED_HBOX | group::VBOX | group::VTOP
    )
}

impl Run<'_, '_> {
    /// Whether the run's open levels can all be left (`leave_lists`): a
    /// text piece or a resume with a box being built among its lists and
    /// no conditional, scope effect or append
    /// pending, nothing half done ([`Run::open_state_refusal`]: a word whose
    /// lookahead met the text's end, a scan, an alignment, a picture
    /// child, …); the innermost level a
    /// group; each level a group sharing the list below, a box's group, or
    /// a paragraph with a group above it (the paragraph is `par:<list>`,
    /// which a kind in that list reads, [`Run::vert_kind`]: the close of a
    /// box's group right above it comes back to it, amsldoc's minipage in
    /// a `center`).
    pub(crate) fn can_leave_lists(&self) -> bool {
        self.leave_refusal().is_none()
    }

    /// Why the run's open levels cannot be left ([`Self::can_leave_lists`]).
    pub(crate) fn leave_refusal(&self) -> Option<&'static str> {
        let op = self.cx.op();
        let why = if !(op == self.ops.exec || op == self.ops.resume) {
            "not a text's run"
        } else if self.nest.is_empty() || !self.nest.iter().any(|l| l.owns) {
            "no list open"
        } else if !self.nest.iter().any(|l| l.owns && is_box_group(l.group)) {
            // (what leaving pays for is a box's contents run as nodes of
            // their own, a picture's statements; a paragraph under groups
            // alone is broken whole at its end whatever its nodes, and
            // each slice of it left would be one more node: huaz's
            // `\lstinline` groups, 2771 nodes against 725)
            "no box open"
        } else if self.nest.len() >= usize::from(u8::MAX - 2) {
            "too deep"
        } else if !self.conds.is_empty() {
            "a conditional open"
        } else if self.effect != phitex_defuse::Effect::None {
            "a scope effect"
        } else if let Some(why) = self.open_state_refusal() {
            why
        } else if self.cut.is_some() || self.close_cont.is_some() || self.tool_cut.is_some() {
            "cut already"
        } else if !self.ins_pending.is_empty() {
            // (insertions waiting for the page builder: only this run
            // holds them, as a continuation's cut would carry them)
            "insertions pending"
        } else if !self.appending.is_empty() || !self.appending_vals.is_empty() {
            "appends pending"
        } else if self.nest.last().is_some_and(|l| l.group == 0) {
            "the innermost level a paragraph"
        } else {
            let k = self.nest.iter().enumerate().position(|(k, l)| {
                !if !l.owns {
                    l.group != 0
                } else if l.group == 0 {
                    l.mode == 102
                        && l.par.is_some()
                        && self
                            .nest
                            .get(k + 1)
                            .is_some_and(|u| u.group != 0 && !u.owns && u.lid == l.lid)
                } else {
                    is_box_group(l.group) && l.par.is_none() && l.prev_graf == 0
                }
            })?;
            let l = &self.nest[k];
            if l.owns && l.group != 0 && !is_box_group(l.group) {
                "a list of a group not a box's (math, insert, …)"
            } else {
                "a level that cannot be left"
            }
        };
        Some(why)
    }

    /// Leaves the run's open levels to the following nodes: each list's
    /// items so far and its transitions' state as a start entry of `bitems:<id>`, each
    /// paragraph's parameters as `par:<list>`, the groups as scopes
    /// ([`Run::leave_scope`]).
    pub(crate) fn leave_lists(&mut self) -> R<()> {
        let v = self.v;
        let base = self.base_mode()?;
        self.muts += 1;
        let mut levels = std::mem::take(&mut self.nest);
        // the run's own ids made global
        let mut k = first_free(base.depth, &base);
        for i in 0..levels.len() {
            if !levels[i].owns {
                continue;
            }
            let old = levels[i].lid;
            let new = global_lid(base.depth, k);
            k += 1;
            for l in &mut levels[i..] {
                if l.lid == old {
                    l.lid = new;
                }
            }
        }
        for i in 0..levels.len() {
            if !levels[i].owns {
                continue;
            }
            let vlid = match i.checked_sub(1) {
                Some(b) => levels[b].lid,
                None => base.lid,
            };
            let l = &mut levels[i];
            let lid = l.lid;
            let mut xs = vec![v.str(START)];
            xs.extend(crate::nest::into_values(v, std::mem::take(&mut l.list)));
            xs.push(if l.mode.abs() == 1 {
                crate::build::pd_value(v, l.fold.pd)
            } else {
                sf_value(v, l.fold.sf)
            });
            let rec = v.list(xs);
            let (group0, par, clang) = (l.group == 0, l.par, l.clang);
            let e = v.list(vec![v.int(1), v.seq_unit(rec)]);
            self.contribute(&items_field(lid), e);
            if group0 {
                let (lang, packed) = par.unwrap_or((0, 0));
                let rec = v.list(vec![v.int(lid), v.int(lang), v.int(packed), v.int(clang)]);
                self.set(&par_field(vlid), rec, true);
            }
        }
        self.par_memo = None;
        self.vpar_memo = None;
        // the groups as scopes
        self.nest = levels.into_iter().filter(|l| l.group != 0).collect();
        self.leave_scope()
    }

    /// An item appended to list `lid` left open by another node.
    pub(crate) fn append_left(&mut self, lid: i64, x: ValueId) {
        let v = self.v;
        let e = v.list(vec![v.int(0), v.seq_unit(x)]);
        self.contribute(&items_field(lid), e);
    }

    /// [`Run::list_fold`] of list `lid` left open: the entries of its
    /// field from the last start on, then this run's own, folded into
    /// `st` and `out`. (Folded once per run: a later read folds only the
    /// run's contributions since, [`LeftMemo`].)
    pub(crate) fn left_list_fold(
        &mut self,
        lid: i64,
        st: &mut FoldState,
        out: &mut Vec<ValueId>,
    ) -> R<()> {
        let v = self.v;
        let f = v.field(&items_field(lid));
        let base = self.cx.fold(f, None, None)?;
        let own: Vec<ValueId> = self
            .contribs
            .iter()
            .filter(|(g, _)| *g == f)
            .map(|(_, x)| *x)
            .collect();
        let start = v.str(START);
        let init = *st;
        let (mut s, mut o, from) = match self.left_memo.take() {
            Some(m)
                if m.lid == lid
                    && m.base == base
                    && m.n <= own.len()
                    && (m.n == 0 || m.last == own[m.n - 1]) =>
            {
                (m.st, m.out, m.n)
            }
            _ => {
                // (the fold from the unit's start: a start discards what
                // came before it, [`ResetMonoid`])
                let (mut s, mut o) = (init, out.clone());
                fold_seq(v, lid, start, parts(v, base).1, &mut s, &mut o);
                (s, o, 0)
            }
        };
        for &x in &own[from..] {
            let (reset, seq) = parts(v, x);
            if reset {
                s = init;
                o.clear();
            }
            fold_seq(v, lid, start, seq, &mut s, &mut o);
        }
        *st = s;
        out.clone_from(&o);
        self.left_memo = Some(LeftMemo {
            lid,
            base,
            n: own.len(),
            last: own.last().copied().unwrap_or(ValueId::UNIT),
            st: s,
            out: o,
        });
        Ok(())
    }

    /// The paragraph a group's kind `k` (mode 102, a list left open) is
    /// in: `md`'s list stays it while `par:<list below>` names it, with
    /// the list below as its vertical list; else the paragraph ended
    /// (a `\par` inside the group) and `md` is that vertical list's.
    ///
    /// `from`: where `k` was defined ([`KindFrom`]). A frame 0's parent is not carried
    /// ([`crate::build::outer_before_opener`]): it is the kind that
    /// reached the frame's opener ([`Run::kind_before`]), and so outward.
    pub(crate) fn par_kind(&mut self, k: ValueId, from: KindFrom, md: &mut Mode) -> R<()> {
        if let Some((kk, open, vlid, vm)) = self.par_memo
            && kk == k
        {
            apply(md, open, vlid, vm);
            return Ok(());
        }
        let v = self.v;
        let marker = crate::build::outer_before_opener(v);
        // the list below: the first kind outward in another list
        let mut cur = k;
        let mut from = from;
        let (vlid, vm) = loop {
            let xs = list_of(v, cur);
            let Some(&p) = xs.get(5) else {
                break (0, 1);
            };
            let p = if p == marker {
                let opener = match from {
                    KindFrom::Here => None,
                    KindFrom::Node(n) => Some(n),
                    KindFrom::Unknown => break (0, 1),
                };
                let (x, d) = self.kind_before(opener)?;
                from = d.map_or(KindFrom::Unknown, KindFrom::Node);
                x
            } else {
                p
            };
            if p == ValueId::UNIT {
                break (0, 1);
            }
            let ys = list_of(v, p);
            let n = |i: usize| ys.get(i).map_or(0, |&y| int_of(v, y));
            if n(2) != md.lid {
                break (n(2), i32::try_from(n(3)).unwrap_or(1));
            }
            cur = p;
        };
        let open = self.get(&par_field(vlid))?.is_some_and(|r| {
            list_of(v, r)
                .first()
                .is_some_and(|&x| int_of(v, x) == md.lid)
        });
        self.par_memo = Some((k, open, vlid, vm));
        apply(md, open, vlid, vm);
        Ok(())
    }

    /// A vertical kind `k` (its list `md.lid`): the paragraph left open
    /// in that list, if `par:<list>` names one, is the mode and the list
    /// here ([`Run::base_mode`]): the close of a group in it (the
    /// picture's environment) comes back to it, the following nodes add
    /// to it, and the node at its `\par` breaks it
    /// ([`Run::end_left_paragraph`]). One read a run.
    pub(crate) fn vert_kind(&mut self, k: ValueId, md: &mut Mode) -> R<()> {
        let open = match self.vpar_memo {
            Some((kk, p)) if kk == k => p,
            _ => {
                let v = self.v;
                let p = self
                    .get(&par_field(md.lid))?
                    .and_then(|r| list_of(v, r).first().map(|&x| int_of(v, x)))
                    .filter(|&p| is_global(p));
                self.vpar_memo = Some((k, p));
                p
            }
        };
        if let Some(p) = open {
            md.vlid = md.lid;
            md.vm = md.m;
            md.m = 102;
            md.lid = p;
        }
        Ok(())
    }

    /// `\par` (§1096) in a paragraph left open by another node, under
    /// groups of the engine's: its items, the field cleared (the groups
    /// go on in the vertical list), its parameters the run's last
    /// paragraph's. None: no such paragraph.
    pub(crate) fn end_left_paragraph(&mut self, md: &Mode) -> R<Option<Vec<crate::nest::Item>>> {
        if !on() || !is_global(md.lid) || self.nest.iter().any(|l| l.owns) {
            return Ok(None);
        }
        let v = self.v;
        let Some(rec) = self.get(&par_field(md.vlid))? else {
            return Ok(None);
        };
        let xs = list_of(v, rec);
        let n = |i: usize| xs.get(i).map_or(0, |&x| int_of(v, x));
        if xs.len() < 4 || n(0) != md.lid {
            return Ok(None);
        }
        let f = self.list_fold(md.lid)?;
        if crate::exec::prof_flag!("PHITEX_BOXSCOPE_TRACE") {
            eprintln!(
                "boxscope: node {} ends the paragraph left open, list {} ({} items)",
                self.cx.node().raw(),
                md.lid,
                f.items.len()
            );
        }
        self.set(&par_field(md.vlid), ValueId::UNIT, true);
        self.par_memo = None;
        self.vpar_memo = None;
        self.last_par = (n(1), n(2));
        self.last_par_line = None;
        self.last_par_place = None;
        // (the run's groups above it now in the vertical list)
        let (vm, vlid) = (md.vm, md.vlid);
        for l in &mut self.nest {
            if l.lid == md.lid {
                l.lid = vlid;
                l.mode = vm;
            }
        }
        Ok(Some(crate::nest::of_values(f.items)))
    }
}

impl Run<'_, '_> {
    /// A box packaged by a run that closed the engine scope it was in
    /// (§1086, [`crate::run::CLOSE`]): the run's reads are the scope's
    /// to its end, so the box, its context and its adjustments go to the
    /// field `boxend:<list>` and a token reads them in the node after
    /// it, outside the scope ([`Run::box_end_left`]).
    pub(crate) fn box_end_outside(&mut self, lid: i64, ctx: i64, b: ValueId, adj: Vec<ValueId>) {
        let v = self.v;
        let mut xs = vec![v.int(ctx), b];
        xs.extend(adj);
        self.set(&box_end_field(lid), v.list(xs), true);
        self.back_input(crate::val::Tok::cs(&format!("{BOX_END}{lid}")));
    }

    /// `box_end` (§1075) of a box [`Run::box_end_outside`] left, in the
    /// mode outside the scope it closed; its adjustments after it, then
    /// the page builder (§1076).
    pub(crate) fn box_end_left(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let lid = self
            .cur
            .tok
            .as_cs()
            .and_then(|n| n.strip_prefix(BOX_END).and_then(|k| k.parse::<i64>().ok()));
        let Some(lid) = lid else {
            return Ok(());
        };
        let Some(rec) = self.get(&box_end_field(lid))? else {
            return Ok(());
        };
        let xs = list_of(v, rec);
        let (Some(&c), Some(&b)) = (xs.first(), xs.get(1)) else {
            return Ok(());
        };
        let ctx = int_of(v, c);
        self.box_end_in(md, ctx, Some(b), false)?;
        for &a in &xs[2..] {
            self.append(md.vlid, a);
        }
        if ctx < crate::build::BOX_FLAG && md.m > 0 {
            self.finish_candidate()?;
        }
        Ok(())
    }
}

fn apply(md: &mut Mode, open: bool, vlid: i64, vm: i32) {
    md.vlid = vlid;
    md.vm = vm;
    if !open {
        md.m = vm;
        md.km = vm;
        md.lid = vlid;
    }
}
