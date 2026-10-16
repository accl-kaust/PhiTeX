//! Pages: where the document's pages finish, and their output routines,
//! as nodes of the graph; every page value moves by def-use
//! (docs/spec-predicted-breaks.md, S2).
//!
//! - A text piece (an `exec` node, or a `resume` piece) typesets in
//!   parallel with every other, and never reads the page. The items it
//!   puts on the main vertical list are its own contributions, delimited
//!   by candidate markers at the points where tex.web calls `build_page`
//!   (§994; [`Run::finish_candidate`]). At its end it defines its main
//!   list's tail ([`crate::build::VTAIL`], read from the piece before
//!   only when its items leave that tail standing), gives its vertical
//!   profile and its items as units of two accumulators
//!   ([`crate::vprof::VPROF`], [`PITEMS`]) and places its scan step
//!   ([`Scan`]) after it.
//! - The scan is TeX's page builder (§980–§1010) over the profiles. A
//!   step reads the scan state ([`SCAN`]), defined once per page by the
//!   continuation after the page's finisher, and the profiles of the
//!   pieces after it up to its own; a step that fires no page defines
//!   nothing, so the steps of a page depend on the page's start, not on
//!   one another. Where a page fires in its own piece, the step defines
//!   the page's break ([`BREAKS`]: its range and candidate, no items)
//!   and places the finisher ([`Finish`]) and the continuation; each of
//!   them assembles the page from the pieces' items by the break's range
//!   with the same builder ([`Run::assemble_page`],
//!   [`PageState::fold_items`]: `\box255`, the insertions, the marks), so
//!   an edit inside a page that moves no break runs no scan step.
//! - A finisher runs the output routine (§1012–§1026) as one run of its
//!   own on its page, defines what the routine defines for everything
//!   after it, and what it gives back to the main list ([`RTN`]); the
//!   continuation after it reads that on the finisher's prediction, so
//!   the scan does not wait for the routine.
//! - A page finishing at a candidate inside a piece, with the piece
//!   going on after it, gets its finisher after the piece all the same.
//!   That is the same as tex.web unless the routine changes what the
//!   piece's commands after the candidate read, or reads what they
//!   defined: the finisher checks ([`Run::check_cut`]) and asks for a cut
//!   (`cutreq`). The piece runs again stopped at that candidate, its
//!   continuation ([`Run::cont_encode`]) the material of a `resume`
//!   piece ([`Resume`], [`RESUME`]) placed after the page's nodes. A piece
//!   whose finishers so far touch what it reads or defines finds a page
//!   finishing inside it in its own run ([`Run::fold_here`]) and cuts at
//!   once; its step places the page as any step does.
//! - Nothing reads the whole list, nothing is replayed: one scan state
//!   per page, one finisher per page, a step per piece reading only the
//!   profiles since its page began.

use std::collections::VecDeque;

use phitex_defuse::{
    Claim, Ctx, Effect, Field, Locality, MayDefine, OpId, Operation, Stop, ValueId, Values,
};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{List, Node};
use phitex_typeset::pack::{PackParams, Spec};
use phitex_typeset::page::{Contents, Fired, InsertClass, Outcome, PageBuilder, PageParams};

use crate::build::{FoldState, IGNORE_DEPTH, Mode, fold_item};
use crate::fields::Par;
use crate::input::{Line, Seg};
use crate::nodes::{self, Side};
use crate::ops::{Shared, ops, run_node_parking};
use crate::run::{Halt, R, Run};
use crate::runstate::{ContSaved, ContSlot, ContState, Loc, State};
use crate::val::{
    glue_of, glue_value, group, int_at, int_of, list_of, str_of, toks_of, toks_value,
};

/// What a finisher's output routine gives back to the main list
/// (§1026): `[markers, list]`, the run's own main-list items (page
/// parameter markers) and the routine's list, without the held-over
/// insertions (the fold continuation after the finisher has them).
/// Read by that continuation; predicted (a routine that gives nothing
/// back gives the same value every page).
pub(crate) const RTN: &str = "rtn";

/// The pieces' main-list items, one unit per piece beside its profile:
/// `[piece, last, items, reads, defs, locals]` (its raw node id, its
/// last candidate when nothing came after it, its items with their
/// markers resolved, what a finisher's check needs, [`Run::check_cut`],
/// and for a piece cut at its last candidate the bindings local to the
/// groups open there, which the routine sees). Read by range where a
/// page fires: the page's real items.
pub(crate) const PITEMS: &str = "pitems";

/// A cut's continuation, `[cont, cutting node]` ([`Run::cont_encode`]):
/// defined by the piece that cuts, read by the resume placed after its
/// finisher. A field, not the resume's arguments: the input left
/// changes with any edit before the document's end, and a resume made
/// anew re-makes everything after it (spec-predicted-breaks §7).
pub(crate) const RESUME: &str = "pages:resume";

/// The fields a finisher defined globally, one unit per finisher: read
/// by range by the resume after a cut (a global definition replaces
/// the group's local value, §279). A finisher declares its unit as the
/// last run of its routine gave it, a guess ([`Finish::static_contribs`]).
pub(crate) const GDEFS: &str = "gdefs";

/// The scan's state (spec-predicted-breaks §2, S2): defined where the
/// scan continues after a page fired ([`Scan`] after the finisher), read
/// by every scan step after it: `[anchor, j, normal, sf, pd, since_bp,
/// params, profile, material]`, the defining node (the steps read the
/// pieces' profiles after it), the pages fired, the ordinary ones, the
/// main list's fold where the page fired, the page parameters in force
/// there, and what the next page begins with (the routine's markers,
/// the held-over insertions, the routine's list, the material carried
/// over, the rest of the piece) as a profile and as items. No version
/// per piece: a step that fires no page defines nothing.
pub(crate) const SCAN: &str = "scan:state";

/// A page's break (spec-redesign §1, `breaks:p`): `[j, at, src, pid, k,
/// rest]`, defined by the node that finds the page firing (a piece's
/// scan step over the profiles, or the scan's continuation over its own
/// material) and read by the page's finisher and the continuation after
/// it, which each assemble the page from the items by the break's range
/// ([`Run::assemble_page`]): the material the page began with and the
/// state there (`src`, a [`SCAN`] value), the pieces' items after its
/// anchor to piece `pid` (-2: the page fires in `src`'s material), the
/// candidate `k`, the profile items after it, and `at`, the node whose
/// position the page's eqtb reads are made at (`\deadcycles`, an
/// insertion class's registers: what reaches the step, not what the
/// routine left). No items in it: an edit inside a page that moves no
/// break changes no break, and runs the page's finisher and
/// continuation, not the scan.
pub(crate) const BREAKS: &str = "breaks";

/// The page nodes' declared contributions: none to the pieces' units
/// ([`crate::vprof::VPROF`], [`PITEMS`]), so a scan step's read of a
/// window, and its read hint's gate, do not wait for a scan step, a
/// finisher or a page's placement still to run, only for the pieces.
pub(crate) fn no_units(v: &Values) -> Vec<(Field, ValueId)> {
    vec![
        (v.field(crate::vprof::VPROF), ValueId::NIL),
        (v.field(PITEMS), ValueId::NIL),
    ]
}

/// [`no_units`] as a run gives them (a run contributing otherwise than
/// its text declared runs the windows' readers again).
pub(crate) fn give_no_units(r: &mut Run<'_, '_>) {
    for (f, x) in no_units(r.v) {
        r.contribs.push((f, x));
    }
}

/// Defined by a `\shipout`: the mark that a page shipped before.
pub(crate) const SHIPPED: &str = "pages:shipped";

/// `max_dimen` (§421).
pub(crate) const MAX_DIMEN: i32 = 0o7777777777;

/// The fields the pages define through a text piece (beyond what its
/// hint can name).
pub(crate) fn is_page_field(name: &str) -> bool {
    PAGE_FIELDS.contains(&name)
}

/// The fields a piece with items on the main list may define for the
/// pages (its hint names them, `crate::hints`): its main list's tail,
/// the shipped mark, and where its own run finds a page finishing inside
/// it, the cut's continuation.
pub(crate) const PAGE_FIELDS: [&str; 3] = [crate::build::VTAIL, SHIPPED, RESUME];

/// What a run had done when it passed a candidate: at its end, a page
/// finishing at its last candidate finishes at the end of the piece iff
/// nothing of this changed after it; a finisher's check of a page
/// finishing at an earlier one takes what the run defined after it
/// (`stamp`, `gids`: [`State::defined_since`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

/// A finisher's page: its index and what the routine leaves for the
/// next page (§1026: the held-over insertions, then its list; the
/// scan's continuation after the finisher folds them, with the material
/// carried over and the rest of the piece).
#[derive(Clone, Debug, Default)]
pub(crate) struct FinCtx {
    pub j: i64,
    pub pending: Vec<ValueId>,
    /// How many of `pending` are the held-over insertions (§1022).
    pub held_n: usize,
    /// The `rtn` the finisher defined.
    pub rtn: Option<ValueId>,
    /// Its arguments in full ([`Run::finisher_args`]).
    pub args: Vec<ValueId>,
}

// -------------------------------------------------------------- state

/// A version of the page builder's state (§980–§991), with the main
/// list's fold and the contributions not yet looked at.
pub(crate) struct PageState {
    /// Pages finished before.
    pub j: i64,
    pub pb: PageBuilder,
    /// The marks and whatsits of `pb.page` (decoded with it).
    pub side: Side,
    /// The main list's fold where the state is (the previous depth, the
    /// items since the last candidate).
    pub fold: FoldState,
    /// The page parameters in force (a `T_PAGE_PARAMS` marker).
    pub params: Option<ValueId>,
    /// Contributions after the last candidate: folded items and markers,
    /// in order; the next candidate takes them first (§994).
    pub pending: Vec<ValueId>,
    /// The pages fired so far whose `\outputpenalty` is above -10001
    /// (the ordinary ones: the finishers' predictions step page
    /// counters by these, [`FinDefs::predict`]).
    pub normal: i64,
}

/// A page finishing: at candidate `k`, the finisher's page data, the
/// material to carry over past it, and the items after the candidate
/// (not folded: the finisher's fold takes them after the routine's).
pub(crate) struct Fire {
    pub k: u32,
    pub data: ValueId,
    pub carried: Vec<ValueId>,
    pub rest: Vec<ValueId>,
}

fn some(x: ValueId) -> Option<ValueId> {
    (x != ValueId::UNIT).then_some(x)
}

impl PageState {
    pub(crate) fn fresh(j: i64) -> PageState {
        PageState {
            j,
            pb: PageBuilder::new(),
            side: Side::default(),
            fold: FoldState {
                sf: 1000,
                pd: IGNORE_DEPTH,
                since_bp: 0,
            },
            params: None,
            pending: Vec::new(),
            normal: 0,
        }
    }

    /// The items of a run folded in: candidate by candidate
    /// ([`PageState::build`]), the rest left pending. `dead` is
    /// `\deadcycles` where the run is; `live` answers for an insertion
    /// class no marker of the run gives.
    pub(crate) fn fold_items(
        &mut self,
        v: &Values,
        raw: &[ValueId],
        dead: i64,
        live: &mut dyn FnMut(u16) -> R<InsertClass>,
    ) -> R<Option<Fire>> {
        let mut buf = std::mem::take(&mut self.pending);
        for (i, &x) in raw.iter().enumerate() {
            match nodes::tag(v, x) {
                nodes::T_BUILD_PAGE => {
                    let k = u32::try_from(int_at(v, x, 1)).unwrap_or(0);
                    self.fold.since_bp = 0;
                    if let Some(mut f) = self.build(v, &mut buf, k, dead, live)? {
                        f.rest = raw[i + 1..].to_vec();
                        return Ok(Some(f));
                    }
                }
                nodes::T_SET_PAGE | nodes::T_INS_PARAMS | nodes::T_END | nodes::T_ABORT => {
                    buf.push(x);
                }
                _ => fold_item(v, 0, x, &mut self.fold, &mut buf),
            }
        }
        self.pending = buf;
        Ok(None)
    }

    /// One `build_page` call (§994) over the contributions `buf` at
    /// candidate `k`: what the page cannot take stays in `buf`.
    #[allow(clippy::too_many_lines)]
    fn build(
        &mut self,
        v: &Values,
        buf: &mut Vec<ValueId>,
        k: u32,
        dead: i64,
        live: &mut dyn FnMut(u16) -> R<InsertClass>,
    ) -> R<Option<Fire>> {
        // §1245: `\pagegoal=` and its kin set the page so far at once;
        // §1008: an insertion class's registers as the call reads them;
        // §987: the page parameters as the call reads them (the last
        // marker: the one the run put at this candidate), for every
        // contribution it takes
        let mut classes: Vec<(u16, InsertClass)> = Vec::new();
        let mut page_params = None;
        let so_far = &mut self.pb.so_far;
        buf.retain(|&x| match nodes::tag(v, x) {
            nodes::T_PAGE_PARAMS => {
                page_params = Some(x);
                false
            }
            nodes::T_SET_PAGE => {
                let i = usize::try_from(int_at(v, x, 1)).unwrap_or(8);
                if i < 8 {
                    so_far[i] = i32::try_from(int_at(v, x, 2)).unwrap_or(0);
                }
                false
            }
            nodes::T_INS_PARAMS => {
                classes.push(class_of(v, x));
                false
            }
            _ => true,
        });
        if page_params.is_some() {
            self.params = page_params;
        }
        let params = self.params.map_or(
            PageParams {
                vsize: 0,
                max_depth: 0,
                top_skip: Glue::ZERO,
                holding_inserts: 0,
                saving_vdiscards: false,
            },
            |m| params_of(v, m),
        );
        let mut queue: VecDeque<ValueId> = std::mem::take(buf).into();
        let mut end_seen = None;
        while let Some(x) = queue.pop_front() {
            match nodes::tag(v, x) {
                // §1054: `\end` ends the job once the page and the
                // contributions are empty and no dead cycle pends; else
                // it fills the page out and comes round again (the
                // marker carried past the finisher)
                nodes::T_END => {
                    end_seen = Some(x);
                    if self.pb.page.is_empty() && queue.is_empty() && dead == 0 {
                        continue;
                    }
                    let xs = list_of(v, x);
                    for &y in xs.iter().skip(1).rev() {
                        queue.push_front(y);
                    }
                    continue;
                }
                // §1335: the main list ends there; the page is not shipped
                nodes::T_ABORT => {
                    buf.clear();
                    return Ok(None);
                }
                nodes::T_WRITE_AT | nodes::T_FONT_AT | nodes::T_PDFW_AT => continue,
                _ => {}
            }
            let Some(p) = nodes::decode_shallow(v, x, &mut self.side) else {
                continue;
            };
            // §1000: only a kern looks at what follows it
            let next = if matches!(p, Node::Kern(..)) {
                queue
                    .front()
                    .and_then(|&y| nodes::decode(v, y, &mut Side::default()))
            } else {
                None
            };
            let mut halt = None;
            let out = {
                let mut class = |n: u16| {
                    classes.iter().find(|c| c.0 == n).map_or_else(
                        || match live(n) {
                            Ok(c) => c,
                            Err(h) => {
                                halt.get_or_insert(h);
                                InsertClass {
                                    box_height_plus_depth: None,
                                    box_is_hbox: false,
                                    count: 0,
                                    dimen: 0,
                                    skip: Glue::ZERO,
                                }
                            }
                        },
                        |c| c.1,
                    )
                };
                self.pb.contribute(p, next.as_ref(), &params, &mut class)
            };
            if let Some(h) = halt {
                return Err(h);
            }
            match out {
                Outcome::Done => {}
                // the kern waits for its successor, with everything after
                Outcome::Wait(_) => {
                    queue.push_front(x);
                    break;
                }
                Outcome::Fire(f) => {
                    if crate::exec::env_flag!("PHITEX_PAGE_TRACE") {
                        eprintln!(
                            "page-trace: page {} fires at candidate {k}: vsize {} maxdepth {} (marker {})",
                            self.j,
                            params.vsize,
                            params.max_depth,
                            self.params
                                .map_or_else(|| "none".to_owned(), |m| v.render(m))
                        );
                    }
                    // §1017: what the page could not take goes first
                    let mut carried: Vec<ValueId> = f
                        .contributions
                        .iter()
                        .map(|n| nodes::encode(v, n, &self.side))
                        .collect();
                    carried.extend(queue.drain(..));
                    if let Some(e) = end_seen {
                        carried.push(e);
                    }
                    let data = page_data(v, &f, &self.side);
                    self.pb = PageBuilder::new();
                    self.side = Side::default();
                    self.j += 1;
                    if f.output_penalty > -10001 {
                        self.normal += 1;
                    }
                    self.fold.since_bp = 0;
                    buf.clear();
                    return Ok(Some(Fire {
                        k,
                        data,
                        carried,
                        rest: Vec::new(),
                    }));
                }
            }
        }
        *buf = queue.into();
        Ok(None)
    }
}

/// A `T_PAGE_PARAMS` marker's parameters (§980).
pub(crate) fn params_of(v: &Values, m: ValueId) -> PageParams {
    let xs = list_of(v, m);
    let g = |i: usize| xs.get(i).map_or(0, |&y| int_of(v, y));
    PageParams {
        vsize: i32::try_from(g(1)).unwrap_or(0),
        max_depth: i32::try_from(g(2)).unwrap_or(0),
        top_skip: glue_of(v, xs.get(3).copied()),
        holding_inserts: i32::try_from(g(4)).unwrap_or(0),
        saving_vdiscards: false,
    }
}

/// A `T_INS_PARAMS` marker: `[tag, n, box h+d or -1, box is hbox, count,
/// dimen, skip]` (§1008's reads of eqtb for class `n`).
pub(crate) fn class_of(v: &Values, m: ValueId) -> (u16, InsertClass) {
    let xs = list_of(v, m);
    let g = |i: usize| xs.get(i).map_or(0, |&y| int_of(v, y));
    let h = g(2);
    (
        u16::try_from(g(1)).unwrap_or(0),
        InsertClass {
            box_height_plus_depth: (h >= 0).then(|| i32::try_from(h).unwrap_or(0)),
            box_is_hbox: g(3) != 0,
            count: i32::try_from(g(4)).unwrap_or(0),
            dimen: i32::try_from(g(5)).unwrap_or(0),
            skip: glue_of(v, xs.get(6).copied()),
        },
    )
}

/// Whether a fired page's `\outputpenalty` is -10001 or less.
fn fire_special(v: &Values, data: ValueId) -> bool {
    list_of(v, data)
        .get(4)
        .is_some_and(|&x| int_of(v, x) <= -10001)
}

/// A fired page as a finisher gets it: `[\box255, inserts, marks, held,
/// \outputpenalty]` (§1012–§1022).
fn page_data(v: &Values, f: &Fired, side: &Side) -> ValueId {
    let b255 = nodes::encode_box(v, &f.box255.node, side);
    let inserts: Vec<ValueId> = f
        .inserts
        .iter()
        .map(|(n, list)| {
            let l: Vec<ValueId> = list.iter().map(|x| nodes::encode(v, x, side)).collect();
            v.list(vec![v.int(i64::from(*n)), v.list(l)])
        })
        .collect();
    let marks: Vec<ValueId> = f
        .marks
        .iter()
        .map(|&k| side.marks.get(k as usize).copied().unwrap_or(ValueId::UNIT))
        .collect();
    let held: Vec<ValueId> = f
        .held_inserts
        .iter()
        .map(|n| nodes::encode(v, n, side))
        .collect();
    v.list(vec![
        b255,
        v.list(inserts),
        v.list(marks),
        v.list(held),
        v.int(i64::from(f.output_penalty)),
    ])
}

// ---------------------------------------------------------- the runs

impl Run<'_, '_> {
    /// Whether the run is on the main vertical list, at most a paragraph
    /// just begun above it (`nest_ptr <= 1` of §1091, §1200; `mode =
    /// vmode` of §1076, §1094, §1100, §1103, §812).
    fn at_outer(&mut self) -> R<bool> {
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
        if !self.at_outer()? {
            return Ok(());
        }
        let v = self.v;
        let k = self.cand;
        if self.st.take_page_params() {
            if crate::exec::env_flag!("PHITEX_PAGE_TRACE") {
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
        self.cand += 1;
        self.cand_marks.push(CandMark {
            k,
            stamp: self.st.stamp(),
            gids: self.st.gid_mark(),
            contribs: self.contribs.len(),
            kids: self.kids.len(),
            lines: self.lines_entered,
            sig: self.st.sig(),
            nest: self.nest.len(),
        });
        if self.stop_at == Some(k) {
            return Err(Halt::Cut);
        }
        Ok(())
    }

    /// Whether this run put a page parameter marker on the main list.
    fn own_page_params_put(&self) -> bool {
        let v = self.v;
        self.own_outer_items()
            .iter()
            .any(|&x| nodes::tag(v, x) == nodes::T_PAGE_PARAMS)
    }

    /// The items this run put on the main list, in order (its `items`
    /// contributions tagged with list 0).
    fn own_outer_items(&self) -> Vec<ValueId> {
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
    /// ships its `\write`s before them in TeX's order ([`Run::check_cut`]).
    fn streams_since(&self, from: usize) -> Vec<Field> {
        let v = self.v;
        let f = v.field("streams");
        let mut out: Vec<Field> = Vec::new();
        for &(g, x) in self.contribs.get(from..).unwrap_or_default() {
            if g != f {
                continue;
            }
            for e in v.seq_to_vec(x) {
                let xs = list_of(v, e);
                // (`[j, entry]`: an entry tagged with its page)
                let entry = match xs.first() {
                    Some(&j) if v.as_int(j).is_some() => {
                        xs.get(1).map(|&y| list_of(v, y)).unwrap_or_default()
                    }
                    _ => xs,
                };
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
        let v = self.v;
        let c = self.insert_class(n)?;
        Ok(v.list(vec![
            v.int(nodes::T_INS_PARAMS),
            v.int(i64::from(n)),
            v.int(c.box_height_plus_depth.map_or(-1, i64::from)),
            v.int(i64::from(c.box_is_hbox)),
            v.int(i64::from(c.count)),
            v.int(i64::from(c.dimen)),
            glue_value(v, &c.skip),
        ]))
    }

    /// Whether this run defined one of class `n`'s registers (its box,
    /// `\count`, `\dimen` or `\skip`).
    fn ins_registers_defined(&mut self, n: u16) -> bool {
        let b = self.box_field(i64::from(n));
        let bl = self.loc_of(b);
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
    fn pin_reaching_page_params(&mut self, raw: &mut [ValueId]) -> R<()> {
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
    /// defines its main list's tail ([`crate::build::VTAIL`]), gives its
    /// profile and its items as units ([`Run::contribute_units`]) and
    /// places its scan step ([`Scan`]) after it; its own run never reads
    /// the page. A piece that must find a page finishing in it at once
    /// (a cut asked by a finisher, `cut:<node>`, [`Run::read_cut_request`];
    /// a second pass stopped at its candidate; the finishers so far
    /// touching what it reads or defines, [`Run::needs_own_fold`]) runs
    /// the page builder in its own run instead ([`Run::fold_here`]). A
    /// piece with nothing for the main list touches nothing. A
    /// finisher's end defines what its routine gives back ([`RTN`]).
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
        if crate::exec::env_flag!("PHITEX_TEX_TRACE") && self.stop_at.is_some() {
            eprintln!(
                "  page fold {}: pass stopped at {:?}: {} items, {} candidates, piece {piece}, top {} doc {}, contribs {}",
                self.cx.node().raw(),
                self.stop_at,
                raw.len(),
                self.cand_marks.len(),
                self.top,
                self.doc,
                self.contribs.len()
            );
        }
        if fin {
            return self.define_rtn(raw);
        }
        if raw.is_empty() {
            return Ok(());
        }
        // (a pass stopped at its candidate folds here whatever it did
        // before it: the finisher and the resume are its to place)
        if self.stop_at.is_none() && self.cut_req.is_none() && !self.needs_own_fold()? {
            self.define_vtail_piece(&raw)?;
            return self.fold_apart(raw);
        }
        self.fold_here(raw)
    }

    /// A finisher's end: what its routine gives back to the main list
    /// ([`RTN`]); the fold continuation placed after it folds that.
    #[allow(clippy::unnecessary_wraps)]
    fn define_rtn(&mut self, raw: Vec<ValueId>) -> R<()> {
        let v = self.v;
        let c = self.fin.clone().unwrap_or_default();
        let list = c.pending.get(c.held_n..).unwrap_or_default().to_vec();
        let x = v.list(vec![v.list(raw), v.list(list)]);
        self.cx.define_with(v.field(RTN), x, Locality::Global);
        if let Some(c) = self.fin.as_mut() {
            c.rtn = Some(x);
        }
        Ok(())
    }

    /// A page found firing (spec-redesign §1): its break defined
    /// ([`BREAKS`]), its finisher placed after this node with its
    /// position as its arguments (`[j, …, routine hint, ordinary pages
    /// fired, special]`), and the scan's continuation after it. Nothing
    /// of the page's items: the finisher and the continuation assemble it
    /// from the break ([`Run::assemble_page`]).
    #[allow(clippy::too_many_arguments)]
    fn place_break(
        &mut self,
        j: i64,
        src: ValueId,
        pid: i64,
        k: u32,
        rest: usize,
        normal: i64,
        special: bool,
    ) -> R<()> {
        let v = self.v;
        let text = self.output_hint()?;
        let at = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
        let b = v.list(vec![
            v.int(j),
            v.int(at),
            src,
            v.int(pid),
            v.int(i64::from(k)),
            v.int(i64::try_from(rest).unwrap_or(-1)),
        ]);
        self.cx.define_with(v.field(BREAKS), b, Locality::Global);
        let mut pos = vec![v.int(j)];
        pos.extend([ValueId::UNIT; 8]);
        pos.extend([text, v.int(normal), v.int(i64::from(special))]);
        self.cx.follow(self.ops.finish, pos);
        self.cx.follow(self.ops.scan, vec![v.int(-2)]);
        Ok(())
    }

    /// A page assembled from its break (`b`, a [`BREAKS`] value): the
    /// state where it began and its material (`src`), the items of the
    /// pieces after the anchor up to piece `pid` (the break's range: the
    /// window ends at its piece, whatever comes after it), folded with
    /// the page builder; what a class's registers and `\deadcycles` give
    /// read where the break was found. `(state after, the fire, the piece's
    /// unit)`, the unit empty for a page firing in the material; `None`
    /// where the items fire no page at the break (a scan defect, said).
    fn assemble_page(&mut self, b: &[ValueId]) -> R<Option<(PageState, Fire, Vec<ValueId>)>> {
        let v = self.v;
        let n = |i: usize| b.get(i).map_or(0, |&x| int_of(v, x));
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(n(1).to_ne_bytes()));
        let (pid, k, rest) = (n(3), n(4), n(5));
        let s = ScanIn::decode(v, b.get(2).copied().and_then(some));
        let mut raws = Vec::new();
        let mut own = Vec::new();
        if pid != -2 {
            let mut items = v.seq_to_vec(self.cx.acc_range(v.field(PITEMS), s.anchor)?);
            let unit_pid = |u: ValueId| list_of(v, u).first().map_or(-1, |&x| int_of(v, x));
            let Some(last) = items.iter().position(|&u| unit_pid(u) == pid) else {
                self.scan_defect(&format!("piece {pid} is not in its range"));
                return Ok(None);
            };
            items.truncate(last + 1);
            for &u in &items {
                let mut raw = unit_items(v, u);
                self.resolve_params_of(unit_pid(u), &mut raw)?;
                raws.push(raw);
            }
            own = list_of(v, items[last]);
        }
        let dead = if has_end(v, &s.material) || raws.iter().any(|r| has_end(v, r)) {
            let f = self.loc_field(crate::fields::cell::DEADCYCLES as Loc);
            match self.cx.use_field_at(f, Some(at))? {
                phitex_defuse::FieldAt::Def(x, _) => int_of(v, x),
                phitex_defuse::FieldAt::Unset => 0,
                phitex_defuse::FieldAt::Gone => self.cint(crate::fields::cell::DEADCYCLES)?,
            }
        } else {
            0
        };
        let mut ps = s.page_state();
        let fire = {
            let mut live = |c: u16| self.insert_class_at(c, at);
            let last = raws.len();
            let mut out = ps.fold_items(v, &s.material, dead, &mut live)?.map(|f| (0, f));
            for (i, raw) in raws.iter().enumerate() {
                if out.is_some() {
                    break;
                }
                out = ps.fold_items(v, raw, dead, &mut live)?.map(|f| (i + 1, f));
            }
            match out {
                Some((i, f)) if i == last => Some(f),
                _ => None,
            }
        };
        let Some(fire) = fire else {
            self.scan_defect(&format!(
                "page {} fires in piece {pid} at candidate {k} by the profiles, not by the items",
                n(0) + 1
            ));
            return Ok(None);
        };
        if i64::from(fire.k) != k || i64::try_from(fire.rest.len()).ok() != Some(rest) {
            self.scan_defect(&format!(
                "page {} fires in piece {pid} at candidate {k} ({rest} items after) by the profiles, at candidate {} ({} after) by the items",
                n(0) + 1,
                fire.k,
                fire.rest.len()
            ));
        }
        Ok(Some((ps, fire, own)))
    }

    /// A page where the scan's decisions and the builder's on the items
    /// differ: said loudly, counted ([`SCAN_REAL_DIFFS`]); the page is
    /// the items'.
    fn scan_defect(&self, what: &str) {
        let n = SCAN_REAL_DIFFS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        eprintln!(
            "phitex: page break at node {}: {what} ({n} so far)",
            self.cx.node().raw()
        );
    }

    /// Insertion class `n`'s registers (§1008) as they reach the node
    /// `at` ([`phitex_defuse::Ctx::use_field_at`]), what the page
    /// builder read where the break was found; as they reach this run
    /// where `at` is gone.
    fn insert_class_at(&mut self, n: u16, at: phitex_defuse::NodeId) -> R<InsertClass> {
        let v = self.v;
        let mut xs = Vec::with_capacity(4);
        let bf = self.box_field(i64::from(n));
        let mut fields = vec![bf];
        for r in 0..3 {
            let l = self.reg_loc(r, i32::from(n));
            fields.push(self.loc_field(l));
        }
        for f in fields {
            match self.cx.use_field_at(f, Some(at))? {
                phitex_defuse::FieldAt::Def(x, _) => xs.push(Some(x)),
                phitex_defuse::FieldAt::Unset => xs.push(None),
                phitex_defuse::FieldAt::Gone => return self.insert_class(n),
            }
        }
        // (a box register's value: `[shift, box]`, the box unit when void)
        let b = xs[0]
            .and_then(|x| list_of(v, x).get(1).copied())
            .filter(|&b| b != ValueId::UNIT);
        Ok(InsertClass {
            box_height_plus_depth: b.map(|b| nodes::box_dim(v, b, 3) + nodes::box_dim(v, b, 2)),
            box_is_hbox: b.is_some_and(|b| nodes::box_is_h(v, b)),
            count: crate::val::i32_of(v, xs[1]),
            dimen: crate::val::i32_of(v, xs[2]),
            skip: glue_of(v, xs[3]),
        })
    }

    /// A finisher's arguments in full from its page's break (`b`) and its
    /// position (`pos`): `[j, data, carried, locals, rest, piece, k, reads,
    /// defs, hint, normal, special]`, the page assembled
    /// ([`Run::assemble_page`]); the check of a page finishing inside the
    /// piece with commands after it ([`Run::check_cut`]) from the piece's
    /// unit. Its position where the page cannot be assembled.
    fn finisher_args(&mut self, b: &[ValueId], pos: &[ValueId]) -> R<Vec<ValueId>> {
        let v = self.v;
        let Some((_, fire, own)) = self.assemble_page(b)? else {
            return Ok(pos.to_vec());
        };
        let pid = b.get(3).map_or(-2, |&x| int_of(v, x));
        let a = |i: usize| own.get(i).copied().unwrap_or(ValueId::UNIT);
        let at_end = pid == -2 || i64::from(fire.k) == int_of(v, a(1));
        let locals = if pid == -2 { Vec::new() } else { list_of(v, a(5)) };
        let mut args = vec![
            pos.first().copied().unwrap_or(ValueId::UNIT),
            fire.data,
            v.list(Vec::new()),
            v.list(locals),
        ];
        if at_end {
            args.extend([ValueId::UNIT; 5]);
        } else {
            let defs_k = list_of(v, a(4))
                .get(usize::try_from(fire.k).unwrap_or(usize::MAX))
                .copied()
                .unwrap_or(ValueId::UNIT);
            args.extend([
                v.list(Vec::new()),
                v.int(pid),
                v.int(i64::from(fire.k)),
                a(3),
                defs_k,
            ]);
        }
        args.extend(pos.get(9..12).unwrap_or_default().iter().copied());
        Ok(args)
    }

    /// The piece's units: its vertical profile ([`crate::vprof::VPROF`])
    /// and its items ([`PITEMS`]), at its position.
    fn contribute_units(
        &mut self,
        raw: &[ValueId],
        last: Option<u32>,
        reads: ValueId,
        defs: ValueId,
        locals: Vec<ValueId>,
    ) {
        let v = self.v;
        let ints = crate::vprof::encode(v, raw);
        crate::vprof::count_unit(&ints);
        let pid = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
        self.contribute(
            crate::vprof::VPROF,
            v.seq_unit(crate::vprof::unit(v, pid, &ints)),
        );
        let it = v.list(vec![
            v.int(pid),
            v.int(last.map_or(-1, i64::from)),
            v.list(raw.to_vec()),
            reads,
            defs,
            v.list(locals),
        ]);
        self.contribute(PITEMS, v.seq_unit(it));
    }

    /// The piece's main list's tail ([`crate::build::VTAIL`]): its
    /// items folded onto the tail before it ([`crate::build::vtail_value`]),
    /// read only when its items leave something of it (no candidate
    /// after them, a deletion reaching before them, the previous depth
    /// their first box's interline glue reads standing in the tail): a
    /// paragraph's tail is its own, and the pieces do not read one
    /// another's (spec-predicted-breaks §8).
    fn define_vtail_piece(&mut self, raw: &[ValueId]) -> R<()> {
        let v = self.v;
        let vt = v.field(crate::build::VTAIL);
        let own = crate::build::vtail_value(v, None, raw);
        let x = if crate::build::vtail_value(v, Some(crate::build::vtail_probe(v, raw.len())), raw)
            == own
        {
            own
        } else {
            let prev = self.eng_read(vt)?;
            crate::build::vtail_value(v, prev, raw)
        };
        self.cx.define_with(vt, x, Locality::Global);
        Ok(())
    }

    /// Whether a page finishing inside this piece would need a cut, as
    /// far as the finishers so far tell (`Ops::fin_touch`): the piece
    /// read a field a routine defines (the page number). Then the piece
    /// folds in its own run. What it defined after a candidate that a
    /// routine reads is the finisher's check's (a cut where a page
    /// finishes inside it, `Run::check_cut`), never a fold here.
    #[allow(clippy::unnecessary_wraps)]
    fn needs_own_fold(&mut self) -> R<bool> {
        let n = self.cand_marks.len();
        if n == 0 || (n == 1 && self.last_candidate() == Some(0)) {
            return Ok(false);
        }
        let touch = self.ops.fin_touch.clone();
        let t = touch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if t.reads.is_empty() && t.defs.is_empty() {
            return Ok(false);
        }
        let v = self.v;
        let mut read: Vec<Field> = self
            .st
            .read_locs()
            .into_iter()
            .map(|l| self.loc_field(l))
            .collect();
        read.extend(self.class_read_fields());
        for f in read {
            if t.defs.contains(&f) {
                if crate::exec::env_flag!("PHITEX_TEX_TRACE") {
                    eprintln!(
                        "  own fold {}: reads {}",
                        self.cx.node().raw(),
                        str_of(v, f.0)
                    );
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The candidate the run passed last, when nothing happened after
    /// it: a page finishing there finishes at the piece's end. A group
    /// that closed after it is something (its local definitions, in force
    /// at the candidate, are gone at the end: amsbook's
    /// `\cleardoublepage` is `\clearpage{\pagestyle{empty}...\newpage}`).
    fn last_candidate(&self) -> Option<u32> {
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

    /// The `[tag, n]` insertion class markers of `raw` given class `n`'s
    /// registers as this run leaves them ([`Run::ins_params_marker`]).
    fn resolve_ins_markers(&mut self, raw: &mut [ValueId]) -> R<()> {
        let v = self.v;
        let mut classes: Vec<(u16, ValueId)> = Vec::new();
        for x in raw.iter_mut() {
            if nodes::tag(v, *x) != nodes::T_INS_PARAMS {
                continue;
            }
            let xs = list_of(v, *x);
            if xs.len() != 2 {
                continue;
            }
            let n = u16::try_from(int_of(v, xs[1])).unwrap_or(0);
            let m = if let Some(&(_, m)) = classes.iter().find(|c| c.0 == n) {
                m
            } else {
                let m = self.ins_params_marker(n)?;
                classes.push((n, m));
                m
            };
            *x = m;
        }
        Ok(())
    }

    /// The page parameters (§980) reaching the position just after node
    /// `at`, as a `T_PAGE_PARAMS` marker: what a piece's `[tag]` marker
    /// means, read where the piece is (the record `pagepar`, or its cells
    /// where it is not defined there, as [`Run::eng_read`] reads them).
    fn page_params_at(&mut self, at: i64) -> R<ValueId> {
        let v = self.v;
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(at.to_ne_bytes()));
        let b = crate::fields::B_PAGE;
        let rf = self.ops.fields.slot_field(crate::fields::record_loc(b));
        let n = crate::fields::record_len(b);
        let cells = match self.cx.use_field_at(rf, Some(at))? {
            phitex_defuse::FieldAt::Def(x, _) => Some(list_of(v, x)).filter(|xs| xs.len() == n),
            phitex_defuse::FieldAt::Gone => return self.page_params_marker(),
            phitex_defuse::FieldAt::Unset => None,
        };
        let absent = crate::fields::record_absent(v);
        let mut xs = Vec::with_capacity(n);
        for i in 0..n {
            let x = if let Some(c) = &cells {
                (c[i] != absent).then_some(c[i])
            } else {
                let loc =
                    crate::runstate::Loc::try_from(crate::fields::record_cell(b, i)).unwrap_or(0);
                let f = self.loc_field(loc);
                match self.cx.use_field_at(f, Some(at))? {
                    phitex_defuse::FieldAt::Def(x, _) => Some(x),
                    _ => None,
                }
            };
            xs.push(x);
        }
        let int = |x: Option<ValueId>| v.int(i64::from(crate::val::i32_of(v, x)));
        Ok(v.list(vec![
            v.int(nodes::T_PAGE_PARAMS),
            int(xs[0]),
            int(xs[1]),
            glue_value(v, &glue_of(v, xs[2])),
            int(xs[3]),
        ]))
    }

    /// `raw`'s `[tag]` page parameter markers, of piece `pid`, given the
    /// parameters reaching it ([`Run::page_params_at`]).
    fn resolve_params_of(&mut self, pid: i64, raw: &mut [ValueId]) -> R<()> {
        let v = self.v;
        let mut m = None;
        for x in raw.iter_mut() {
            if nodes::tag(v, *x) == nodes::T_PAGE_PARAMS && list_of(v, *x).len() == 1 {
                let mm = if let Some(mm) = m {
                    mm
                } else {
                    let mm = self.page_params_at(pid)?;
                    m = Some(mm);
                    mm
                };
                *x = mm;
            }
        }
        Ok(())
    }

    /// A profile's `[tag]` page parameter items, of piece `pid`, given the
    /// parameters reaching it.
    fn resolve_prof_of(&mut self, pid: i64, items: &mut [crate::vprof::PItem]) -> R<()> {
        let mut m = None;
        for x in items.iter_mut() {
            if let crate::vprof::PItem::Params(p @ None) = x {
                let pp = if let Some(pp) = m {
                    pp
                } else {
                    let mm = self.page_params_at(pid)?;
                    let pp = params_of(self.v, mm);
                    m = Some(pp);
                    pp
                };
                *p = Some(pp);
            }
        }
        Ok(())
    }

    /// What a finisher's check needs when a page can finish inside the
    /// piece with commands after it ([`Run::check_cut`]): the locations
    /// the run read from the engine with the values it read, and per
    /// candidate the names the run defined after it (and its local
    /// definitions at any time: a scope closing after the candidate hides
    /// them from a finisher placed after the piece). Units otherwise.
    fn check_data(&mut self, last: Option<u32>) -> (ValueId, ValueId) {
        let v = self.v;
        let n = self.cand_marks.len();
        let inside_possible = !(n == 1 && last == Some(0)) && n > 0;
        if !inside_possible {
            return (ValueId::UNIT, ValueId::UNIT);
        }
        let mut rs: Vec<ValueId> = self
            .st
            .read_values()
            .into_iter()
            .map(|(l, x)| {
                let f = self.loc_field(l);
                v.list(vec![f.0, opt_val(x)])
            })
            .collect();
        // (a meaning read by its class only: no value a routine's
        // definition can equal, so any global definition of it by the
        // routine counts as a change, [`Run::check_cut`])
        let class = v.str("\u{1}class");
        rs.extend(
            self.class_read_fields()
                .into_iter()
                .map(|f| v.list(vec![f.0, class])),
        );
        let gids = self.st.gid_mark();
        let locals: Vec<Field> = self
            .st
            .defined_since(v, 0, gids)
            .into_iter()
            .filter(|(_, _, l)| *l == Locality::Local)
            .map(|(l, _, _)| self.loc_field(l))
            .collect();
        let marks = self.cand_marks.clone();
        let mut per = Vec::with_capacity(n);
        for m in &marks {
            if Some(m.k) == last {
                per.push(v.list(Vec::new()));
                continue;
            }
            let mut names: Vec<(ValueId, bool)> = locals.iter().map(|f| (f.0, false)).collect();
            for (l, _, _) in self.st.defined_since(v, m.stamp, m.gids) {
                let stands = self.st.def_stands(l);
                names.push((self.loc_field(l).0, stands));
            }
            names.extend(
                self.streams_since(m.contribs)
                    .into_iter()
                    .map(|f| (f.0, false)),
            );
            names.sort_unstable_by_key(|x| (x.0.raw(), !x.1));
            names.dedup_by_key(|x| x.0);
            per.push(
                v.list(
                    names
                        .into_iter()
                        .map(|(f, g)| v.list(vec![f, v.int(i64::from(g))]))
                        .collect(),
                ),
            );
        }
        (v.list(rs), v.list(per))
    }

    /// A piece's end when no page needs finding in its own run: its
    /// insertion class markers given the registers where it ends, its
    /// units, its scan step placed after it. Its page parameter markers
    /// stay `[tag]`: the steps read the parameters reaching it where it
    /// is ([`Run::page_params_at`]), so a piece reads none (an output
    /// routine setting `\vsize` would refute every piece after it).
    fn fold_apart(&mut self, mut raw: Vec<ValueId>) -> R<()> {
        let v = self.v;
        let last = self.last_candidate();
        // (what its commands read, before the reads that resolve its
        // markers: those are no command's)
        let (reads, defs) = self.check_data(last);
        self.pin_reaching_page_params(&mut raw)?;
        self.resolve_ins_markers(&mut raw)?;
        self.contribute_units(&raw, last, reads, defs, Vec::new());
        self.cx.follow(self.ops.scan, vec![v.int(-3)]);
        Ok(())
    }

    /// The page builder in this run: the scan state reaching it, the
    /// pieces' items after its anchor and this run's own folded, the
    /// page finishing in its own items placed (its finisher, the scan's
    /// continuation, and after a cut the resume piece). A stop here
    /// parks the run: the fold is a command of its own
    /// ([`Run::begin_command`]).
    fn fold_here(&mut self, raw: Vec<ValueId>) -> R<()> {
        let t0 = std::time::Instant::now();
        let r = self.fold_here_(raw);
        phitex_pdf::phase::add(18, t0);
        r
    }

    #[allow(clippy::too_many_lines)]
    fn fold_here_(&mut self, mut raw: Vec<ValueId>) -> R<()> {
        let v = self.v;
        // (its input read out: the siblings it takes are known, and so
        // are the fields it defines: what its state publishes and the
        // page's; a park at the reads below holds no other sibling and no
        // reader of another field, a piece's hint bound narrowed to them)
        let n = self.claim_count();
        if crate::exec::env_flag!("PHITEX_DEBUG_CLAIMS") {
            eprintln!(
                "claim: node {} claims {n} at its fold (pulled {})",
                self.cx.node().raw(),
                self.pulled
            );
        }
        self.cx.claim(n);
        let mut defs = self.published_fields();
        defs.extend([crate::build::VTAIL, SHIPPED, RESUME].map(|f| v.field(f)));
        if let Some(list) = crate::exec::env_str!("PHITEX_DEBUG_NODES")
            && list
                .split(',')
                .any(|x| x.trim().parse::<u64>().ok() == Some(self.cx.node().raw()))
        {
            let names: Vec<String> = defs.iter().map(|f| crate::val::field_name(v, *f)).collect();
            eprintln!(
                "narrow {} (resumed {}): {names:?}",
                self.cx.node().raw(),
                self.resumed
            );
        }
        self.cx.narrow(MayDefine::Names(defs));
        self.begin_command();
        self.define_vtail_piece(&raw)?;
        let last = self.last_candidate();
        let (reads, defs) = self.check_data(last);
        self.pin_reaching_page_params(&mut raw)?;
        self.resolve_page_params(&mut raw)?;
        let s = ScanIn::decode(v, self.eng_read(v.field(SCAN))?);
        let units = v.seq_to_vec(self.cx.acc_range(v.field(PITEMS), s.anchor)?);
        let dead = if [&s.material, &raw].iter().any(|r| has_end(v, r))
            || units.iter().any(|&u| has_end(v, &unit_items(v, u)))
        {
            self.cint(crate::fields::cell::DEADCYCLES)?
        } else {
            0
        };
        let mut raws = Vec::with_capacity(units.len());
        for &u in &units {
            let mut r = unit_items(v, u);
            let pid = list_of(v, u).first().map_or(-1, |&x| int_of(v, x));
            self.resolve_params_of(pid, &mut r)?;
            raws.push(r);
        }
        let mut ps = s.page_state();
        let fire = {
            let mut live = |n: u16| self.insert_class(n);
            let mut fire = ps.fold_items(v, &s.material, dead, &mut live)?;
            for r in &raws {
                if fire.is_some() {
                    break;
                }
                fire = ps.fold_items(v, r, dead, &mut live)?;
            }
            if fire.is_some() {
                // (a page finishing before this piece: a state read before
                // the node that finishes it ran; the run goes again when
                // it has)
                None
            } else {
                ps.fold_items(v, &raw, dead, &mut live)?
            }
        };
        self.mark.active = false;
        let Some(fire) = fire else {
            // a second pass whose state no longer finishes the page at its
            // candidate (the state reaching it changed since the first pass
            // read it): nothing placed, the run goes again from the top as a
            // first pass
            if self.stop_at.is_some() {
                return Ok(());
            }
            self.contribute_units(&raw, last, reads, defs, Vec::new());
            self.cx.follow(self.ops.scan, vec![v.int(-3)]);
            return Ok(());
        };
        // (a second pass stopped at the candidate: the piece goes on after
        // it, whatever the pass did not run)
        let at_end = self.stop_at != Some(fire.k) && last == Some(fire.k);
        if crate::exec::env_flag!("PHITEX_TEX_TRACE") {
            eprintln!(
                "  fold {}: page {} fires at candidate {} (at end {at_end}, stop_at {:?}, cut asked {:?})",
                self.cx.node().raw(),
                ps.j,
                fire.k,
                self.stop_at,
                self.cut_req
            );
        }
        if !at_end {
            self.cut = Some(fire.k);
            if self.stop_at != Some(fire.k) {
                // the second pass, stopped at the candidate, places it
                return Ok(());
            }
        }
        // the page is its scan step's to place, as for any piece (the same
        // nodes whichever run finds it); a cut's continuation is the resume
        // piece's, placed after them
        let (cont, locals) = if at_end {
            (None, Vec::new())
        } else {
            let (c, l) = self.cont_encode()?;
            (Some(c), l)
        };
        self.contribute_units(&raw, Some(fire.k), ValueId::UNIT, ValueId::UNIT, locals);
        self.cx.follow(self.ops.scan, vec![v.int(-3)]);
        if let Some(cont) = cont {
            let flags = i64::from(self.top) | (i64::from(self.doc) << 1);
            // (the cutting node: the finishers between it and the resume
            // tell which fields they defined globally, `gdefs`)
            let pid = v.int(i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes()));
            self.cx
                .define_with(v.field(RESUME), v.list(vec![cont, pid]), Locality::Global);
            self.cx.follow(
                self.ops.resume,
                vec![ValueId::UNIT, v.int(flags), ValueId::UNIT],
            );
        }
        Ok(())
    }

    /// A piece's start: whether a finisher asked for a cut of this node
    /// (`cut:<node>`, an environment field the client sets from the
    /// finishers' `cutreq` contributions, [`Run::check_cut`]), at which
    /// candidate.
    pub(crate) fn read_cut_request(&mut self) -> R<()> {
        let args = self.cx.args();
        let k = args.get(1).map_or(0, |&t| int_of(self.v, t));
        if k.trailing_zeros() >= 2 {
            return Ok(());
        }
        let f = self.v.field(&format!("cut:{}", self.cx.node().raw()));
        self.cut_req = self
            .cx
            .use_field(f)?
            .and_then(|x| u32::try_from(int_of(self.v, x)).ok());
        Ok(())
    }

    /// A finisher placed after the piece its page finished inside of
    /// (arguments past the fourth: the rest of the piece's items, the
    /// piece's node, the candidate, what the piece read, what it defined
    /// after the candidate): whether the routine, run here instead of at
    /// the candidate, changed what the piece's commands after it saw or
    /// saw what they defined. If so, a cut is asked for: the piece runs
    /// again next round with the routine between its two parts.
    fn check_cut(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
        if args.len() < 9 {
            return Ok(());
        }
        let (pid, k, reads, defs) = (a(5), a(6), a(7), a(8));
        let mut gdefs: Vec<(ValueId, ValueId)> = Vec::new();
        for (loc, x, l) in self.st.published(v, self.rec_load) {
            if l == Locality::Global {
                gdefs.push((self.loc_field(loc).0, x));
            }
        }
        let mut read: Vec<ValueId> = self
            .st
            .read_locs()
            .into_iter()
            .map(|l| self.loc_field(l).0)
            .collect();
        read.extend(self.class_read_fields().into_iter().map(|f| f.0));
        // (the page placed by this finisher's `place` node reads the
        // writer's state where the finisher stands: a writer command the
        // piece ran after the candidate came before the page's objects)
        if self.placed_page {
            read.push(v.field("pdf:state").0);
        }
        // (the files the page's `\write`s went to: a write, open or
        // close the piece made after the candidate came after them)
        read.extend(self.streams_since(0).into_iter().map(|f| f.0));
        let mut why = None;
        for e in list_of(v, reads) {
            let xs = list_of(v, e);
            if let [f, x] = xs[..]
                && let Some(&(_, y)) = gdefs.iter().find(|(g, _)| *g == f)
                && y != x
            {
                why = Some(format!("changed {}", str_of(v, f)));
                break;
            }
        }
        // (a location both defined, the piece's definition standing at
        // its end: the piece's is the later one in tex.web, so the
        // routine's gives way to it: the value reaching here put back)
        let mut restore = Vec::new();
        if why.is_none() {
            for e in list_of(v, defs) {
                let xs = list_of(v, e);
                let Some(&f) = xs.first() else { continue };
                let stands = xs.get(1).is_some_and(|&g| int_of(v, g) != 0);
                if read.contains(&f) {
                    why = Some(format!("after {}", str_of(v, f)));
                    break;
                }
                if gdefs.iter().any(|(g, _)| *g == f) && stands {
                    restore.push(Field(f));
                }
            }
        }
        if why.is_none() {
            for f in restore {
                let x = self.eng_read(f)?;
                let loc = self.loc_of(f);
                self.set_loc(
                    loc,
                    x.map_or(
                        crate::runstate::Native::Absent,
                        crate::runstate::Native::Raw,
                    ),
                    true,
                );
            }
            return Ok(());
        }
        let Some(why) = why else { return Ok(()) };
        if crate::exec::env_flag!("PHITEX_TEX_TRACE") || crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "  cut asked: piece {} at candidate {} ({why})",
                int_of(v, pid),
                int_of(v, k)
            );
        }
        self.contribute("cutreq", v.seq_unit(v.list(vec![pid, k])));
        Ok(())
    }

    /// `\pagegoal`, `\pagetotal`, … (§421): the page state reaching this
    /// piece with its own items up to its last candidate folded in (the
    /// contributions after it are not on the page), read, not defined.
    pub(crate) fn page_dimen(&mut self, m: i32) -> R<i32> {
        let v = self.v;
        let raw = self.own_outer_items();
        let last = raw
            .iter()
            .rposition(|&x| nodes::tag(v, x) == nodes::T_BUILD_PAGE)
            .map_or(0, |i| i + 1);
        let mut head = raw[..last].to_vec();
        self.resolve_page_params(&mut head)?;
        let s = ScanIn::decode(v, self.eng_read(v.field(SCAN))?);
        let units = v.seq_to_vec(self.cx.acc_range(v.field(crate::vprof::VPROF), s.anchor)?);
        let dead = self.cint(crate::fields::cell::DEADCYCLES)?;
        let mut st = s.state(v);
        let mut items = vec![crate::vprof::decode(&s.prof)];
        for &u in &units {
            let (pid, ints) = crate::vprof::unit_parts(v, u);
            let mut x = crate::vprof::decode(&ints);
            self.resolve_prof_of(pid, &mut x)?;
            items.push(x);
        }
        items.push(crate::vprof::decode(&crate::vprof::encode(v, &head)));
        let mut rd = RunReaching(self);
        for x in &items {
            if st.fold_items(x, dead, &mut rd).is_some() {
                break;
            }
        }
        if st.pb.contents == Contents::Empty {
            return Ok(if m == 0 { MAX_DIMEN } else { 0 });
        }
        Ok(st.pb.so_far[usize::try_from(m).unwrap_or(0).min(7)])
    }

    /// The page a finisher finishes (its first argument); `usize::MAX`
    /// in any other run.
    pub(crate) fn page_index(&self) -> usize {
        self.fin
            .as_ref()
            .and_then(|c| usize::try_from(c.j).ok())
            .unwrap_or(usize::MAX)
    }

    /// `fire_up` (§1012–§1025) for a finisher's page: the marks and the
    /// insertion boxes defined, `\deadcycles` counted, then the output
    /// routine pushed to run on this run's nest in its group (`true`), or
    /// the page shipped by the default routine (§1023; also after too
    /// many dead cycles, §1024), the held-over insertions pending.
    #[allow(clippy::too_many_lines)]
    fn fire_page(&mut self, d: ValueId) -> R<bool> {
        let v = self.v;
        let data = list_of(v, d);
        let a = |i: usize| data.get(i).copied().unwrap_or(ValueId::UNIT);
        let (b255, inserts, marks, held) =
            (a(0), list_of(v, a(1)), list_of(v, a(2)), list_of(v, a(3)));
        let output_penalty = data.get(4).map_or(0, |&x| int_of(v, x));
        if let Some(c) = self.fin.as_mut() {
            c.held_n = held.len();
        }
        // §1012: the marks
        let mut top = self
            .get_loc(crate::fields::cell::MARK_TOP)?
            .filter(|&x| x != ValueId::UNIT);
        let bot = self
            .get_loc(crate::fields::cell::MARK_BOT)?
            .filter(|&x| x != ValueId::UNIT);
        let mut first = None;
        let mut new_bot = bot;
        if bot.is_some() {
            top = bot;
        }
        crate::etex::fire_marks_values(self, &marks, None)?;
        for &m in &marks {
            if crate::etex::is_class_mark(v, Some(m)) {
                continue;
            }
            if first.is_none() {
                first = Some(m);
            }
            new_bot = Some(m);
        }
        if top.is_some() && first.is_none() {
            first = top;
        }
        for (name, m) in [
            ("mark:top", top),
            ("mark:first", first),
            ("mark:bot", new_bot),
        ] {
            self.set(name, m.unwrap_or(ValueId::UNIT), true);
        }
        // §1018–§1021: the insertion boxes
        for e in inserts {
            let xs = list_of(v, e);
            let n = xs.first().map_or(0, |&x| int_of(v, x));
            let old = self.box_reg(i32::try_from(n).unwrap_or(0))?;
            let mut s2 = Side::default();
            let mut l: List = match old.and_then(|b| nodes::decode(v, b, &mut s2)) {
                Some(Node::Box(b)) => b.list,
                _ => Vec::new(),
            };
            for &x in &xs.get(1).map(|&l| list_of(v, l)).unwrap_or_default() {
                if let Some(nd) = nodes::decode(v, x, &mut s2) {
                    l.push(nd);
                }
            }
            let p = phitex_typeset::pack::vpack(l, Spec::Additional(0), &PackParams::default());
            let b = nodes::encode_box(v, &p.node, &s2);
            self.set(&format!("box:{n}"), v.list(vec![v.int(0), b]), true);
        }
        self.set("param:outputpenalty", v.int(output_penalty), true);
        let output = self.par(Par::output)?;
        let mut toks = toks_of(v, output);
        let dead = self.cint(crate::fields::cell::DEADCYCLES)?;
        let max_dead = i64::from(self.param("maxdeadcycles")?);
        if !toks.is_empty() && dead >= max_dead {
            // §1024
            self.log(
                "error",
                &format!("Output loop---{dead} consecutive dead cycles"),
            )?;
            toks.clear();
        }
        if toks.is_empty() {
            // §1023: the default output routine: the held-over insertions
            // go back to the contributions, the page is shipped
            if let Some(c) = self.fin.as_mut() {
                c.pending = held;
            }
            self.ship_out(b255)?;
            return Ok(false);
        }
        // §1025
        self.set_cell(crate::fields::cell::DEADCYCLES, v.int(dead + 1), true);
        self.set_cell(
            crate::fields::cell::BOX255,
            v.list(vec![v.int(0), b255]),
            true,
        );
        self.push_list(-1, group::OUTPUT, vec![v.list(held)]);
        self.normal_paragraph()?;
        // (the routine's token list carries its braces, §1226: the group
        // is open already, so its `{` is not read; its `}` ends it)
        toks.remove(0);
        self.push_toks(toks);
        Ok(true)
    }

    /// The output group's `}` (§1026): the held-over insertions and the
    /// routine's list are what the next page sees first (folded at the
    /// run's end); `\box255` must be void; a page parameter the routine
    /// set governs the next page's breaks.
    pub(crate) fn output_end(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let (items, _) = self.list_items(md.lid)?;
        let mut left = md.extra.first().map(|&x| list_of(v, x)).unwrap_or_default();
        self.close_group(md)?;
        if self.st.take_page_params() {
            self.put_page_params();
        }
        if self.box_reg(255)?.is_some() {
            // §1028
            self.log("error", "Output routine didn't use all of \\box255")?;
            self.set_cell(
                crate::fields::cell::BOX255,
                v.list(vec![v.int(0), ValueId::UNIT]),
                true,
            );
        }
        self.set_cell(crate::fields::cell::INSPEN, v.int(0), true);
        left.extend(items);
        if let Some(c) = self.fin.as_mut() {
            c.pending = left;
        }
        Ok(())
    }

    /// `ship_out` (§638–§640): the page goes to the `pages` accumulator
    /// with its `\count0..9` and fonts; its `\write`s and `\openout`s
    /// happen now (§1366–§1374); [`SHIPPED`] is defined for the text
    /// after.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn ship_out(&mut self, b: ValueId) -> R<()> {
        let v = self.v;
        let mut counts = [0i32; 10];
        for (k, c) in counts.iter_mut().enumerate() {
            *c = self.reg_int(0, k)?;
        }
        let mut j = 9;
        while j > 0 && counts[j] == 0 {
            j -= 1;
        }
        let mut s = String::from("[");
        for (k, c) in counts.iter().enumerate().take(j + 1) {
            s.push_str(&c.to_string());
            if k < j {
                s.push('.');
            }
        }
        self.log("ship", &s)?;
        let timed = crate::exec::env_flag!("PHITEX_TEX_TIME");
        let t0 = std::time::Instant::now();
        let mut side = Side::default();
        // (the page's boxes are decoded once, here, for the writer: their
        // decodings are not kept for a later decode)
        let prev = crate::val::NO_KEEP.with(|c| c.replace(true));
        let decoded = nodes::decode(v, b, &mut side);
        crate::val::NO_KEEP.with(|c| c.set(prev));
        let Some(Node::Box(bx)) = decoded else {
            return Ok(());
        };
        let t_decode = t0.elapsed();
        phitex_pdf::phase::add(1, t0);
        let t_p2 = std::time::Instant::now();
        // (the positions walk only for a whatsit that needs one: a
        // `\write` expands without positions; a plain page skips the
        // walk, the second copy of the box it makes and the font view)
        let needs_pos = side.whats.iter().any(|&w| {
            let xs = list_of(v, w);
            let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
            !matches!(kind.as_str(), "write" | "open" | "close")
        });
        let pos = if needs_pos {
            crate::pdfprim::positions(self, &bx)?
        } else {
            crate::pdfprim::Positions {
                at: phitex_defuse::FxHashMap::default(),
                page_height: 0,
                pdf: self.param("pdfoutput")? > 0,
            }
        };
        let t_pos = t0.elapsed();
        // what the PDF writer makes of the page's pdfTeX whatsits
        let mut pdf_data = Vec::new();
        // §1366: the whatsits in the order `hlist_out`/`vlist_out` meet them
        for (wi, w) in side.whats.clone().into_iter().enumerate() {
            let xs = list_of(v, w);
            let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
            match kind.as_str() {
                "write" => {
                    let k = i32::try_from(xs.get(1).map_or(0, |&y| int_of(v, y))).unwrap_or(0);
                    let toks = toks_of(v, xs.get(2).copied());
                    self.write_out(k, &toks)?;
                }
                "open" => {
                    let n = xs.get(1).map_or(0, |&y| int_of(v, y));
                    let name = xs.get(2).map(|&y| str_of(v, y)).unwrap_or_default();
                    self.open_out(n, &name)?;
                }
                "close" => {
                    let n = xs.get(1).map_or(0, |&y| int_of(v, y));
                    self.set(&format!("write:{n}"), ValueId::UNIT, true);
                }
                k => {
                    let at = u32::try_from(wi).ok().and_then(|i| pos.at.get(&i).copied());
                    crate::pdfprim::at_shipout(self, k, at, &pos)?;
                    if pos.pdf
                        && let Some(d) = crate::pdfprim::writer_data(self, &xs)?
                    {
                        let h = i64::try_from(wi).unwrap_or(0);
                        pdf_data.push(v.list(vec![v.int(h), d]));
                    }
                }
            }
        }
        let t_whats = t0.elapsed();
        phitex_pdf::phase::add(2, t_p2);
        let t_p3 = std::time::Instant::now();
        let page_box = Node::Box(bx);
        let view = self.font_view(std::slice::from_ref(&page_box))?;
        let mut fonts = Vec::new();
        let mut ids: Vec<u32> = view.fonts.keys().copied().collect();
        ids.sort_unstable();
        for &f in &ids {
            let info = self.get(&format!("fontinfo:{f}"))?;
            let Some(i) = crate::fonts::info(v, info) else {
                continue;
            };
            let Some(file) = i.file else { continue };
            fonts.push(v.list(vec![
                v.int(i64::from(f)),
                v.str(&i.name),
                file,
                v.int(i64::from(i.s)),
            ]));
        }
        let cs = v.list(counts.iter().map(|&c| v.int(i64::from(c))).collect());
        let pdf = self.pdf_page_params(pos.pdf)?;
        let page = v.list(vec![b, cs, v.list(fonts), pdf, v.list(pdf_data)]);
        let t_fonts = t0.elapsed();
        phitex_pdf::phase::add(3, t_p3);
        let t_p4 = std::time::Instant::now();
        let shown = match &page_box {
            Node::Box(bx) if pos.pdf => crate::pdfout::ship(self, page, bx, &side, &ids)?,
            _ => page.raw(),
        };
        phitex_pdf::phase::add(4, t_p4);
        if timed {
            eprintln!(
                "shipout: decode_us {} positions_us {} whatsits_us {} fonts_us {} pdf_us {}",
                t_decode.as_micros(),
                t_pos.checked_sub(t_decode).unwrap().as_micros(),
                t_whats.checked_sub(t_pos).unwrap().as_micros(),
                t_fonts.checked_sub(t_whats).unwrap().as_micros(),
                t0.elapsed().checked_sub(t_fonts).unwrap().as_micros()
            );
        }
        let su = v.seq_unit(page);
        self.contribute("pages", su);
        let h = v.seq_unit(v.int(i64::from_ne_bytes(shown.to_ne_bytes())));
        self.contribute("pagehash", h);
        self.log("shipend", "]")?;
        self.set_cell(crate::fields::cell::DEADCYCLES, v.int(0), true);
        let j = i64::try_from(self.page_index()).unwrap_or(-1);
        self.set(SHIPPED, v.int(j), true);
        phitex_pdf::phase::add(15, t0);
        Ok(())
    }

    /// The parameters pdfTeX reads at each page (pdfTeX §750–§757), with
    /// whether the file is PDF and the colour stacks made so far.
    fn pdf_page_params(&mut self, pdf: bool) -> R<ValueId> {
        let v = self.v;
        let mut xs = vec![v.int(i64::from(pdf))];
        for p in [
            "pdfhorigin",
            "pdfvorigin",
            "pdfpagewidth",
            "pdfpageheight",
            "hoffset",
            "voffset",
            "pdfcompresslevel",
            "pdfdecimaldigits",
            "pdfminorversion",
            "pdfmajorversion",
            "mag",
        ] {
            xs.push(v.int(i64::from(self.param(p)?)));
        }
        xs.push(
            self.get_loc(crate::fields::cell::PDF_COLORSTACKS)?
                .unwrap_or(ValueId::UNIT),
        );
        for p in ["pdfobjcompresslevel", "pdfgentounicode"] {
            xs.push(v.int(i64::from(self.param(p)?)));
        }
        for p in ["pdfpageattr", "pdfpagesattr", "pdfpageresources"] {
            let toks = toks_of(v, self.get(&format!("param:{p}"))?);
            let mut s = String::new();
            self.show_toks_into(&mut s, &toks)?;
            xs.push(v.str(&s));
        }
        Ok(v.list(xs))
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
    if let Some((name, at)) = s.origin {
        let at = at + s.loc.min(s.buf.len());
        xs.push(name);
        xs.push(v.int(i64::try_from(at).unwrap_or(0)));
        if s.lineno > 0 {
            xs.push(v.int(i64::from(s.lineno)));
        }
    }
    Some(v.list(xs))
}

fn file_frame(v: &Values, f: &crate::run::FileRest) -> ValueId {
    v.list(vec![
        v.str("f"),
        f.id,
        v.int(i64::try_from(f.off).unwrap_or(0)),
        f.name,
    ])
}

fn opt_val(x: Option<ValueId>) -> ValueId {
    x.unwrap_or(ValueId::UNIT)
}

fn gid_int(v: &Values, g: Option<u32>) -> ValueId {
    v.int(g.map_or(-1, i64::from))
}

fn int_gid(k: i64) -> Option<u32> {
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
        let here: Vec<ValueId> = match self.file_line_here() {
            Some(crate::run::FileLine::Known(n)) => vec![ValueId::UNIT, v.int(n)],
            Some(crate::run::FileLine::Piece(name)) => vec![name, v.int(-1)],
            None => Vec::new(),
        };
        let toks_frame = |toks: &[crate::val::Tok]| {
            let mut t = vec![v.str("t"), toks_value(v, toks)];
            t.extend(here.iter().copied());
            v.list(t)
        };
        let mut frames = Vec::new();
        for l in self.input.levels.iter().rev() {
            match l.kind {
                crate::input::kind::TOKS => {
                    let r = self.input.arena.get(l.rest());
                    if !r.is_empty() {
                        frames.push(toks_frame(r));
                    }
                }
                crate::input::kind::LINE => {
                    if let Some(f) = line_frame(v, self.input.line(*l)) {
                        frames.push(f);
                    }
                }
                _ => frames.push(file_frame(v, self.input.file(*l))),
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
                        frames.push(toks_frame(t));
                    }
                }
                Seg::File(f) => frames.push(file_frame(v, f)),
            }
        }
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
                    v.int(i64::from(l.gid)),
                    v.int(i64::try_from(l.save_base).unwrap_or(0)),
                    v.int(i64::try_from(l.def_base).unwrap_or(0)),
                    v.int(i64::from_ne_bytes(l.open_stamp.to_ne_bytes())),
                    toks_value(v, &l.ag),
                ])
            })
            .collect();
        let conds = v.list(self.conds.iter().map(|&c| v.int(i64::from(c))).collect());
        let codes = v.list(self.if_codes.iter().map(|&c| v.int(i64::from(c))).collect());
        // the state table's groups
        let open: Vec<u32> = self
            .nest
            .iter()
            .filter(|l| l.group != 0)
            .map(|l| l.gid)
            .collect();
        if let Some(name) = crate::exec::env_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(name));
            eprintln!(
                "cont {} at a cut: {name}: {} (open groups {open:?})",
                self.cx.node().raw(),
                self.st.debug_marks(l)
            );
        }
        let c = self.st.cont(v, &open);
        if let Some(name) = crate::exec::env_str!("PHITEX_CONT_SHOW") {
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
                overrides.push(v.list(vec![f.0, opt_val(base)]));
                locals.push(v.list(vec![f.0, opt_val(s.cur)]));
            }
            let (stamp, local, gid) = s.ldef.map_or((-1, 0, None), |(st, l, g)| {
                (i64::from_ne_bytes(st.to_ne_bytes()), i64::from(l), g)
            });
            slots.push(v.list(vec![
                f.0,
                opt_val(s.cur),
                v.int(i64::from(s.loaded)),
                v.int(i64::from(s.save_gid)),
                v.int(i64::from_ne_bytes(s.save_stamp.to_ne_bytes())),
                v.int(i64::from_ne_bytes(s.ginert.to_ne_bytes())),
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
                v.int(i64::from(s.gid)),
                v.int(i64::from_ne_bytes(s.stamp.to_ne_bytes())),
                v.int(i64::from_ne_bytes(s.at.to_ne_bytes())),
                v.int(i64::from_ne_bytes(s.ldef.0.to_ne_bytes())),
                v.int(i64::from(s.ldef.1)),
                gid_int(v, s.ldef.2),
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
            v.int(i64::from_ne_bytes(c.stamp.to_ne_bytes())),
            v.int(i64::from(c.next_gid)),
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
            v.int(i64::try_from(self.pdf_occ).unwrap_or(0)),
            v.int(i64::from_ne_bytes(self.irreversible.to_ne_bytes())),
            v.int(i64::from(self.read_back_done)),
            v.int(i64::from(self.astate.is_some())),
            v.int(self.astate.unwrap_or(0)),
            v.int(self.aopen),
            v.int(i64::from(self.claim_rest)),
            v.int(i64::from(self.hint_void)),
            v.int(i64::from(self.no_pull)),
            v.int(i64::from(self.rec_load)),
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
        self.input.text = crate::run::decode_segs(v, &[a(0)]);
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
        if let Some(name) = crate::exec::env_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(name));
            eprintln!(
                "cont {} loaded: {name}: {}",
                self.cx.node().raw(),
                self.st.debug_marks(l)
            );
        }
        // the fields the finishers between the cut and this resume
        // defined globally (`gdefs`, the window after the cutting node)
        let gdefs: Vec<ValueId> = match Some(pid).filter(|&p| p != ValueId::UNIT) {
            Some(p) if self.cx.op() == self.ops.resume => {
                let pid =
                    phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(int_of(v, p).to_ne_bytes()));
                let acc = self.cx.acc_range(v.field(GDEFS), Some(pid))?;
                v.seq_to_vec(acc)
                    .into_iter()
                    .flat_map(|e| list_of(v, e))
                    .collect()
            }
            _ => Vec::new(),
        };
        for e in list_of(v, a(6)) {
            let ys = list_of(v, e);
            if ys.len() < 2 {
                continue;
            }
            let f = Field(ys[0]);
            let x0 = some(ys[1]);
            let x1 = self.eng_read(f)?;
            // (§279: a global definition replaces the group's local value
            // whatever it defines, the value outside the group too: by
            // the definition, not the value)
            if gdefs.contains(&f.0) {
                let loc = self.loc_of(f);
                self.st.override_global(loc, x1);
                continue;
            }
            if crate::exec::env_flag!("PHITEX_PAGE_TRACE") {
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
        self.irreversible = u64::from_ne_bytes(n(9).to_ne_bytes());
        self.read_back_done = n(10) != 0;
        self.astate = (n(11) != 0).then(|| n(12));
        self.aopen = n(13);
        self.claim_rest = n(14) != 0;
        self.hint_void = n(15) != 0;
        self.no_pull = n(16) != 0;
        self.rec_load = n(17) != 0;
        Ok(())
    }
}

// -------------------------------------------------------- operations

/// A page's finisher: the output routine for one page (§1012–§1026)
/// where the page finishes. Arguments `[j, data, carried, locals]`: the
/// page's index, its material (`\box255`, the insertion boxes, the
/// marks, the held-over insertions, `\outputpenalty`), what the fold
/// carries past it, and the bindings local to the groups open at the
/// cut (the routine runs inside them). A finisher placed after the
/// piece its page finished inside of has five more: `[rest, piece,
/// k, reads, defs]`, the piece's items after the candidate and what
/// its check needs ([`Run::check_cut`]). It reads what reaches its
/// place as any node does and defines what the routine defines; its
/// page is shipped from here.
pub(crate) struct Finish(pub Shared);

/// What the finishers of a build read and defined (`Ops::fin_touch`).
#[derive(Default)]
pub(crate) struct FinTouch {
    pub reads: phitex_defuse::FxHashSet<Field>,
    pub defs: phitex_defuse::FxHashSet<Field>,
}

/// What the last two finishers running one routine defined
/// (`Ops::fin_defs`): the earlier run's definitions, then the last's.
#[derive(Clone, Debug, Default)]
pub(crate) struct FinDefs {
    pub prev: Vec<(Field, ValueId)>,
    pub last: Vec<(Field, ValueId)>,
    /// The pages of the runs `prev` and `last` were.
    pub prev_j: i64,
    pub last_j: i64,
    /// Every field a run of the routine defined so far: a routine
    /// defines some on some pages only (a float page, a page with a
    /// list continued), not on the last two.
    pub seen: Vec<Field>,
    /// What the last run contributed to `gdefs` (the fields it defined
    /// globally, [`Run::contribute_global_defs`]): the next finisher's
    /// declared contribution, a guess ([`Finish::static_contribs`]).
    pub gdefs: Option<ValueId>,
}

impl FinDefs {
    /// The fields any run of the routine defined.
    fn names(&self) -> Vec<Field> {
        self.seen.clone()
    }

    /// The definitions both runs made with the same value: stable
    /// across pages, so a reader may run on them; a page number or a
    /// mark, changing every page, is not predicted (a wrong prediction
    /// runs its readers again).
    fn stable(&self) -> Vec<(Field, ValueId)> {
        self.last
            .iter()
            .filter(|d| self.prev.contains(d))
            .copied()
            .collect()
    }

    /// [`FinDefs::stable`] for the finisher of page `j`, with the
    /// integers that step uniformly by page (`\count0`, a page
    /// counter) carried on to page `j` (spec-predicted-breaks §3).
    fn predict(&self, v: &Values, j: i64) -> Vec<(Field, ValueId)> {
        let dj = self.last_j - self.prev_j;
        self.last
            .iter()
            .filter_map(|&(f, x)| {
                // (the marks are the page's text: equal on two pages by
                // chance; the scan knows them, spec §3)
                let name = crate::val::field_name(v, f);
                if name.starts_with("mark")
                    || name.starts_with("cs:g__mark_")
                    || name == "param:outputpenalty"
                {
                    return None;
                }
                let &(_, y) = self.prev.iter().find(|d| d.0 == f)?;
                if x == y {
                    return Some((f, x));
                }
                // (registers only: a page counter; not a hash, a penalty)
                if !name.starts_with("count:") {
                    return None;
                }
                let (a, b) = (v.as_int(y)?, v.as_int(x)?);
                let d = b.checked_sub(a)?;
                if dj <= 0 || d % dj != 0 {
                    return None;
                }
                let x = (d / dj).checked_mul(j - self.last_j)?.checked_add(b)?;
                Some((f, v.int(x)))
            })
            .collect()
    }
}

/// What a finisher defines besides its routine's definitions
/// (`fire_page`, `output_end`, `ship_out`): field prefixes.
const FINISHER_FIELDS: &[&str] = &[
    SHIPPED,
    "deadcycles",
    "mark:",
    "param:outputpenalty",
    "inspen",
    "box:",
    "badness",
    "write:",
    "pdf:",
];

/// A finisher's entry in `Ops::fin_defs`: its routine (the hint `t`)
/// and whether its page is an ordinary one or one fired at a penalty
/// of -10001 or less (`\outputpenalty` is in the page data): such a
/// routine run (LaTeX's float and `\clearpage` handling) defines other
/// things and ships nothing; predicted from runs of its own kind.
fn fin_key(v: &Values, t: ValueId, args: &[ValueId]) -> u64 {
    let special = crate::exec::env_flag!("PHITEX_FIN_KIND")
        && args.get(11).is_some_and(|&x| v.as_int(x) == Some(1));
    t.raw() ^ u64::from(special).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

/// Finishers created with a prediction of their definitions, and
/// without (reported with `PHITEX_TEX_TIME`).
pub(crate) static FIN_PREDICTED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub(crate) static FIN_UNPREDICTED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

impl Finish {
    /// What the last runs of the routine defined, carried to page `j`
    /// ([`FinDefs::predict`]), and always what the routine gives back to
    /// the main list ([`RTN`]): the continuation after the finisher
    /// reads it, and with it guessed the scan goes on to the next page
    /// without waiting for the routine (spec-redesign §1, finish row).
    /// The guess is the last run's value, or nothing given back (the
    /// routine that ships its page, §1026); a wrong guess runs the
    /// continuation again.
    fn predicted_defs_(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        if crate::exec::env_flag!("PHITEX_NO_FIN_PREDICT") {
            return None;
        }
        let t = args.get(9).copied().unwrap_or(ValueId::UNIT);
        let j = args.get(10).map_or(0, |&x| int_of(v, x));
        let o = ops(&self.0);
        let m = o
            .fin_defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = (t != ValueId::UNIT)
            .then(|| m.get(&fin_key(v, t, args)))
            .flatten();
        let mut out = d
            .map(|d| {
                if crate::exec::env_flag!("PHITEX_NO_STEP") {
                    d.stable()
                } else {
                    d.predict(v, j)
                }
            })
            .unwrap_or_default();
        let rtn = v.field(RTN);
        if !out.iter().any(|e| e.0 == rtn) {
            let last = d.and_then(|d| d.last.iter().find(|e| e.0 == rtn).map(|e| e.1));
            out.push((
                rtn,
                last.unwrap_or_else(|| v.list(vec![v.list(Vec::new()), v.list(Vec::new())])),
            ));
        }
        Some(out)
    }

    /// The finisher's declared contribution to `gdefs` (what it defines
    /// globally, read by the resume after it as a guess, spec-redesign
    /// §1 resume row): the last run's of the routine, else none; a run
    /// giving another runs the resume again.
    fn gdefs_guess(&self, args: &[ValueId], v: &Values) -> ValueId {
        let t = args.get(9).copied().unwrap_or(ValueId::UNIT);
        if t == ValueId::UNIT {
            return ValueId::NIL;
        }
        ops(&self.0)
            .fin_defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&fin_key(v, t, args))
            .and_then(|d| d.gdefs)
            .unwrap_or(ValueId::NIL)
    }
}

impl Operation for Finish {
    fn name(&self) -> &'static str {
        "finish"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    /// The output routine's hint (encoded, the tenth argument:
    /// [`crate::hints::routine_hint`]) and the fields a finisher defines
    /// itself; anything where the routine's text cannot be bounded, or
    /// the hint is not given.
    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        let Some(&t) = args.get(9) else {
            return MayDefine::Any;
        };
        if t == ValueId::UNIT || crate::exec::env_flag!("PHITEX_HINTS_OFF") {
            return MayDefine::Any;
        }
        // (a finisher ran this routine before: what it defined then,
        // by name, and the fields a finisher sets itself; a definition
        // outside them refutes the declaration, as any node's does, and
        // only its readers run again)
        let known = ops(&self.0)
            .fin_defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&fin_key(v, t, args))
            .map(FinDefs::names);
        if let Some(mut names) = known
            && !crate::exec::env_flag!("PHITEX_NO_FIN_PREDICT")
        {
            // (and the writer's state the page's placement defines, the
            // node this finisher places after itself: `pdfout::Place`)
            for n in [
                RTN,
                SHIPPED,
                "deadcycles",
                "param:outputpenalty",
                "inspen",
                "badness",
                "box:255",
                "pdf:state",
                "pdf:pre",
            ] {
                names.push(v.field(n));
            }
            ops(&self.0).fields.add_record_fields(&mut names);
            return MayDefine::Names(names);
        }
        let h = crate::hints::Hint::decode(&str_of(v, t));
        let mut ps: Vec<String> = match h.may_define() {
            MayDefine::Any => return MayDefine::Any,
            MayDefine::Prefixes(ps) => ps,
            MayDefine::Names(ns) => ns.iter().map(|&f| crate::val::field_name(v, f)).collect(),
            MayDefine::Nothing => Vec::new(),
        };
        ps.extend(FINISHER_FIELDS.iter().map(|&s| s.to_owned()));
        ps.push(RTN.to_owned());
        crate::fields::add_record_prefixes(&mut ps);
        MayDefine::Prefixes(ps)
    }
    /// No pieces' units ([`no_units`]), and its global definitions as
    /// the last run of the routine gave them ([`Finish::gdefs_guess`]).
    fn static_contribs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let mut out = no_units(v);
        out.push((v.field(GDEFS), self.gdefs_guess(args, v)));
        Some(out)
    }
    /// What the last finisher running the same routine defined, with
    /// its values: the text after this one runs on them, and runs again
    /// only where a value differs (the page number's readers). The page
    /// state and the shipped mark are left out: the chain reads them.
    fn predicted_defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let r = self.predicted_defs_(args, v);
        let c = if r.is_some() {
            &FIN_PREDICTED
        } else {
            &FIN_UNPREDICTED
        };
        c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        r
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    /// Its page's break ([`BREAKS`]): the node waits for it.
    fn reads(&self, _: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(vec![v.field(BREAKS)])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let t_fin = std::time::Instant::now();
        let _ = phitex_pdf::phase::take();
        let pos = cx.args();
        let o = ops(&self.0);
        let out = run_node_parking(cx, o, true, |r| {
            if crate::exec::env_flag!("PHITEX_FIN_TRACE") {
                let j = pos.first().map_or(0, |&x| int_of(r.v, x));
                eprintln!(
                    "fin-trace: node {} page {j} spec {} resumed {}",
                    r.cx.node().raw(),
                    r.cx.is_speculative(),
                    r.resumed
                );
            }
            if !r.resumed {
                let v = r.v;
                // (its page's break, defined by the node that found it,
                // the node before it; the page assembled from the items
                // by the break's range; its own arguments are its
                // position)
                let b = r
                    .eng_read(v.field(BREAKS))?
                    .map(|x| list_of(v, x))
                    .filter(|xs| xs.len() == 6 && xs.first() == pos.first());
                let args = match b {
                    Some(b) => r.finisher_args(&b, &pos)?,
                    None => pos.clone(),
                };
                let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
                r.fin = Some(FinCtx {
                    j: int_of(v, a(0)),
                    pending: Vec::new(),
                    held_n: 0,
                    rtn: None,
                    args: args.clone(),
                });
                for e in list_of(v, a(3)) {
                    let ys = list_of(v, e);
                    if let [f, x] = ys[..] {
                        let loc = r.loc_of(Field(f));
                        r.st.preload(loc, some(x));
                    }
                }
                if let Some(names) = crate::exec::env_str!("PHITEX_FIN_SHOW") {
                    let mut shown = Vec::new();
                    for n in names.split(',') {
                        let x = r.get(n)?;
                        shown.push(format!(
                            "{n}={}",
                            x.map_or_else(
                                || "undefined".to_owned(),
                                |x| crate::val::show_def(v, x)
                            )
                        ));
                    }
                    let locals: Vec<String> = list_of(v, a(3))
                        .iter()
                        .filter_map(|&e| list_of(v, e).first().map(|&f| str_of(v, f)))
                        .collect();
                    eprintln!(
                        "finisher {} page {} (piece {}, candidate {}, locals {:?}, args {}): {}",
                        r.cx.node().raw(),
                        int_of(v, a(0)),
                        v.render(a(5)),
                        v.render(a(6)),
                        locals,
                        args.len(),
                        shown.join(", ")
                    );
                }
                r.fire_page(a(1))?;
            }
            r.run_all()?;
            give_no_units(r);
            let args = r.fin.as_ref().map(|c| c.args.clone()).unwrap_or_default();
            let olds = r.fin_olds()?;
            r.check_cut(&args)?;
            let g = r.contribute_global_defs();
            r.record_finisher_defs(&args, &olds, g);
            Ok(())
        });
        if phitex_pdf::phase::on() {
            phitex_pdf::phase::add(0, t_fin);
            let a = phitex_pdf::phase::take();
            #[allow(clippy::cast_precision_loss)]
            let us: Vec<String> = a
                .iter()
                .map(|&n| format!("{:.1}", n as f64 / 1000.0))
                .collect();
            eprintln!("finphase: {}", us.join(" "));
        }
        out
    }
}

impl Run<'_, '_> {
    /// The fields this finisher defined globally, for the resume after
    /// it ([`Run::cont_load`]): a group-local value the resume carries
    /// is replaced by a global definition even of an equal value.
    fn contribute_global_defs(&mut self) -> ValueId {
        let v = self.v;
        let names: Vec<ValueId> = self
            .st
            .published(v, self.rec_load)
            .into_iter()
            .filter(|d| d.2 == Locality::Global)
            .map(|d| self.loc_field(d.0).0)
            .collect();
        // (none given as nothing, as the declared guess says it,
        // [`Finish::static_contribs`])
        let x = if names.is_empty() {
            ValueId::NIL
        } else {
            v.seq_unit(v.list(names))
        };
        self.contribute(GDEFS, x);
        x
    }

    /// The values reaching this finisher of the fields its routine
    /// defined globally without reading them first (`\global\vsize=`):
    /// read before the commit, for [`Run::record_finisher_defs`].
    fn fin_olds(&mut self) -> R<Vec<(Loc, Option<ValueId>)>> {
        let v = self.v;
        let mut out = Vec::new();
        for (loc, _, l) in self.st.published(v, self.rec_load) {
            if l != Locality::Global || self.st.engine_value(loc).is_some() {
                continue;
            }
            let f = self.loc_field(loc);
            let name = crate::val::field_name(v, f);
            // (what the finisher sets itself, `fire_page` and `ship_out`,
            // counts as changed unread: reading it would make this
            // finisher wait on the last one's value)
            if is_page_field(&name) || FINISHER_FIELDS.iter().any(|p| name.starts_with(p)) {
                continue;
            }
            out.push((loc, self.eng_read(f)?));
        }
        Ok(out)
    }

    /// The definitions this finisher publishes, kept for the next one
    /// running the same routine ([`Finish::predicted_defs`]), and the
    /// fields it read and whose value it changed, for the pieces'
    /// own-fold decision ([`Run::needs_own_fold`]): a global definition
    /// giving the value that reached the finisher changes nothing a piece
    /// sees (LaTeX's `\shipout` sets `\afterassignment` and its
    /// `\setbox` consumes it; the routine sets `\vsize` to the value it
    /// has).
    fn record_finisher_defs(
        &mut self,
        args: &[ValueId],
        olds: &[(Loc, Option<ValueId>)],
        gdefs: ValueId,
    ) {
        let v = self.v;
        {
            let mut reads: Vec<Field> = self
                .st
                .read_locs()
                .into_iter()
                .map(|l| self.loc_field(l))
                .collect();
            reads.extend(self.class_read_fields());
            reads.extend(self.streams_since(0));
            let mut gdefs: Vec<Field> = Vec::new();
            for (loc, x, l) in self.st.published(v, self.rec_load) {
                if l != Locality::Global {
                    continue;
                }
                let f = self.loc_field(loc);
                if is_page_field(&crate::val::field_name(v, f)) {
                    continue;
                }
                let old = match self.st.engine_value(loc) {
                    Some(e) => Some(e),
                    None => olds.iter().find(|o| o.0 == loc).map(|o| o.1),
                };
                if old != Some(Some(x)) {
                    gdefs.push(f);
                }
            }
            let mut t = self
                .ops
                .fin_touch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            t.reads.extend(reads);
            t.defs.extend(gdefs);
        }
        let Some(&t) = args.get(9) else { return };
        // (a speculative run's definitions may be refuted: they would
        // seed the predictions of the finishers after it)
        if t == ValueId::UNIT || self.cx.is_speculative() {
            return;
        }
        let mut out = Vec::new();
        for (loc, x, _) in self.st.published(v, self.rec_load) {
            let f = self.loc_field(loc);
            let name = crate::val::field_name(v, f);
            if is_page_field(&name) {
                continue;
            }
            out.push((f, x));
        }
        if let Some(x) = self.fin.as_ref().and_then(|c| c.rtn) {
            out.push((v.field(RTN), x));
        }
        // (the ordinary pages fired up to this one: page counters step
        // by them)
        let j = args.get(10).map_or(0, |&x| int_of(v, x));
        let mut m = self
            .ops
            .fin_defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let e = m.entry(fin_key(v, t, args)).or_default();
        e.gdefs = Some(gdefs);
        for &(f, _) in &out {
            if !e.seen.contains(&f) {
                e.seen.push(f);
            }
        }
        if crate::exec::env_flag!("PHITEX_PRED_SHOW") {
            for (f, x) in e.predict(v, j) {
                let got = out.iter().find(|d| d.0 == f).map(|d| d.1);
                if got != Some(x) {
                    eprintln!(
                        "pred page {j}: {} predicted {} got {}",
                        crate::val::field_name(v, f),
                        v.render(x),
                        got.map_or_else(|| "nothing".to_owned(), |g| v.render(g))
                    );
                }
            }
        }
        // (the two latest pages: finishers commit in any order; a page's
        // run again replaces its own entry, never makes the other: two
        // runs of one page agree on what changes from page to page, a
        // column kept, a mark, and would predict it for the next page)
        if j == e.last_j && !e.last.is_empty() {
            e.last = out;
        } else if j == e.prev_j && !e.prev.is_empty() {
            e.prev = out;
        } else if j > e.last_j || e.last.is_empty() {
            e.prev = std::mem::take(&mut e.last);
            e.prev_j = e.last_j;
            e.last = out;
            e.last_j = j;
        } else if j > e.prev_j || e.prev.is_empty() {
            e.prev = out;
            e.prev_j = j;
        }
    }
}

impl Run<'_, '_> {
    /// `\deadcycles` where the fold is, when the fold needs it: only
    /// `\end` reads it (§1054); any other fold reads nothing a finisher
    /// defines, so a finisher placed anew does not run the folds after
    /// it again.
    fn deadcycles_for(&mut self, raw: &[ValueId]) -> R<i64> {
        let v = self.v;
        if raw.iter().any(|&x| nodes::tag(v, x) == nodes::T_END) {
            return self.cint(crate::fields::cell::DEADCYCLES);
        }
        Ok(0)
    }

    /// The hint of the output routine in force (its tokens shown as
    /// source, read under the format's definitions), encoded: the
    /// finisher's bound ([`Finish`]). Unit for the default routine.
    fn output_hint(&mut self) -> R<ValueId> {
        let v = self.v;
        let out = self.par(Par::output)?;
        let toks = toks_of(v, out);
        if toks.is_empty() {
            return Ok(ValueId::UNIT);
        }
        let mut text = String::new();
        self.show_toks_into(&mut text, &toks)?;
        let h = {
            let defs = self
                .ops
                .defs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::hints::routine_hint(&text, &defs)
        };
        Ok(v.str(&h.encode()))
    }
}

/// The rest of a piece after a cut: arguments `[cont, flags]`, the
/// continuation ([`Run::cont_encode`]) and the piece's text kind. It
/// runs the input left in the groups left open, as the piece would have
/// gone on, reading what the finisher before it defined.
pub(crate) struct Resume(pub Shared);

impl Operation for Resume {
    fn name(&self) -> &'static str {
        "resume"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        MayDefine::Any
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Pending
    }
    fn claim(
        &self,
        _: &[ValueId],
        _: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        Claim::Pending
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args();
        let o = ops(&self.0);
        let t0 = std::time::Instant::now();
        let out = run_node_parking(cx, o, true, |r| {
            if !r.resumed {
                let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
                let (cont, pid) = if a(0) == ValueId::UNIT {
                    let x = r.eng_read(r.v.field(RESUME))?;
                    let xs = x.map(|x| list_of(r.v, x)).unwrap_or_default();
                    (
                        xs.first().copied().unwrap_or(ValueId::UNIT),
                        xs.get(1).copied().unwrap_or(ValueId::UNIT),
                    )
                } else {
                    (a(0), a(2))
                };
                r.cont_load(cont, pid)?;
                r.read_cut_request()?;
            }
            r.run_all()
        });
        if crate::exec::env_flag!("PHITEX_TEX_TIME") && t0.elapsed().as_millis() > 200 {
            let v = cx.values();
            let text = v.render(args.first().copied().unwrap_or(ValueId::UNIT));
            let cut: String = text.chars().take(600).collect();
            eprintln!(
                "longrun: resume {} {:.0} ms: {cut}",
                cx.node().raw(),
                t0.elapsed().as_secs_f64() * 1e3
            );
        }
        out
    }
}

/// What a scan reads where it runs: the node's run./// What a scan reads where a fold runs: the fold's run.
struct RunReaching<'r, 'c, 'a>(&'r mut Run<'c, 'a>);

impl crate::vprof::Reaching for RunReaching<'_, '_, '_> {
    fn params(&mut self) -> PageParams {
        let v = self.0.v;
        self.0.page_params_marker().map_or(
            PageParams {
                vsize: 0,
                max_depth: 0,
                top_skip: Glue::ZERO,
                holding_inserts: 0,
                saving_vdiscards: false,
            },
            |m| params_of(v, m),
        )
    }

    fn class(&mut self, n: u16) -> InsertClass {
        self.0.insert_class(n).unwrap_or(InsertClass {
            box_height_plus_depth: None,
            box_is_hbox: false,
            count: 0,
            dimen: 0,
            skip: Glue::ZERO,
        })
    }
}

// ------------------------------------------------------------- scan

/// A scan step (spec-predicted-breaks §2, S2): TeX's page builder
/// (§980–§1010) over the pieces' vertical profiles, from the last page
/// fired on. Arguments `[kind]`, its position only (§7): -3, after a
/// piece (the step of that piece), or -2, after a finisher (the scan's
/// continuation).
///
/// - A piece's step is held until the pieces before it have run (its
///   read hint names the profiles, [`crate::vprof::VPROF`]: the engine
///   holds it while a contributor before it is in work; the page nodes
///   declare they contribute none, [`no_units`]), reads the scan state
///   ([`SCAN`]) and the profiles of the pieces after the state's anchor
///   up to its own (the units of that window), and runs the builder
///   over the material the page began with and the units. A step that
///   fires no page defines nothing: the scan's state has one version per
///   page, not per piece. Where a page fires in its own piece, the step
///   defines its break and places its finisher and the continuation
///   ([`Run::place_break`]); it reads no items.
/// - A continuation assembles its page from the break
///   ([`Run::assemble_page`]: what the page leaves, the held-over
///   insertions, the material carried, the rest of the piece), reads
///   what the routine gave back ([`RTN`], on the finisher's prediction),
///   and defines the scan state the next page begins with, or places the
///   next page's break when it fires within that material.
pub(crate) struct Scan(pub Shared);

impl Operation for Scan {
    fn name(&self) -> &'static str {
        "scan"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    /// The page fields and the scan state: a step not run yet may fire
    /// a page and place a continuation that defines it, so a step's read
    /// waits for the steps before it still to run; a step's run narrows
    /// this to what it defined ([`Run::scan_node`]).
    fn may_define(&self, _: &[ValueId], v: &Values) -> MayDefine {
        MayDefine::Names(vec![v.field(SCAN), v.field(BREAKS)])
    }
    /// A step is held until the pieces before it have run (the profiles,
    /// an accumulator: the engine's gate waits for every contributor
    /// before it in work) and the scan state is there.
    fn reads(&self, args: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        if args.first().is_some_and(|&x| int_of(v, x) == -2) {
            return Some(vec![v.field(BREAKS)]);
        }
        Some(vec![v.field(SCAN), v.field(crate::vprof::VPROF)])
    }
    /// No pieces' units: a window holding a step does not wait for it.
    fn static_contribs(&self, _: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        Some(no_units(v))
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args();
        let o = ops(&self.0);
        run_node_parking(cx, o, false, |r| r.scan_node(&args))
    }
}

/// The scan state a step starts from ([`SCAN`]).
struct ScanIn {
    anchor: Option<phitex_defuse::NodeId>,
    j: i64,
    normal: i64,
    fold: FoldState,
    /// The page parameters in force (a `T_PAGE_PARAMS` marker).
    params: Option<ValueId>,
    /// What the page begins with, as a profile and as items.
    prof: Vec<i64>,
    material: Vec<ValueId>,
}

impl ScanIn {
    fn decode(v: &Values, x: Option<ValueId>) -> ScanIn {
        let xs = x.map(|x| list_of(v, x)).unwrap_or_default();
        if xs.len() < 9 {
            return ScanIn {
                anchor: None,
                j: 0,
                normal: 0,
                fold: crate::build::IGNORE_DEPTH_STATE,
                params: None,
                prof: Vec::new(),
                material: Vec::new(),
            };
        }
        let n = |i: usize| int_of(v, xs[i]);
        ScanIn {
            anchor: Some(phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(
                n(0).to_ne_bytes(),
            ))),
            j: n(1),
            normal: n(2),
            fold: FoldState {
                sf: i32::try_from(n(3)).unwrap_or(1000),
                pd: i32::try_from(n(4)).unwrap_or(IGNORE_DEPTH),
                since_bp: usize::try_from(n(5)).unwrap_or(0),
            },
            params: some(xs[6]),
            prof: list_of(v, xs[7])
                .into_iter()
                .map(|y| int_of(v, y))
                .collect(),
            material: list_of(v, xs[8]),
        }
    }

    /// The scan's builder where the page began.
    fn state(&self, v: &Values) -> crate::vprof::ScanState {
        crate::vprof::ScanState::fresh(
            self.j,
            self.normal,
            self.fold,
            self.params.map(|m| params_of(v, m)),
        )
    }

    /// The real builder where the page began.
    fn page_state(&self) -> PageState {
        let mut ps = PageState::fresh(self.j);
        ps.normal = self.normal;
        ps.fold = self.fold;
        ps.params = self.params;
        ps
    }
}

/// A [`PITEMS`] unit's items.
fn unit_items(v: &Values, u: ValueId) -> Vec<ValueId> {
    list_of(v, u)
        .get(2)
        .map(|&x| list_of(v, x))
        .unwrap_or_default()
}

/// Whether items hold an `\end` marker (the builder then reads
/// `\deadcycles`, §1054).
fn has_end(v: &Values, raw: &[ValueId]) -> bool {
    raw.iter().any(|&x| nodes::tag(v, x) == nodes::T_END)
}

/// What a scan node defined.
enum Placed {
    Nothing,
    /// The scan state ([`SCAN`]).
    State,
    /// A page's break ([`BREAKS`]).
    Page,
}

/// Pages where the scan's decisions and the builder's on the items
/// differed (a scan defect; the page is built from the items).
static SCAN_REAL_DIFFS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Run<'_, '_> {
    /// A [`Scan`] node's run.
    #[allow(clippy::too_many_lines)]
    fn scan_node(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        give_no_units(self);
        let placed = if args.first().is_some_and(|&x| int_of(v, x) == -2) {
            self.scan_cont()?
        } else {
            self.scan_step()?
        };
        // (what it defined: a reader after it no longer waits for it)
        let names: Vec<Field> = match placed {
            Placed::Nothing => Vec::new(),
            Placed::State => vec![v.field(SCAN)],
            // (and the scan state: until it commits, the continuation it
            // places, which defines it, is not there for a reader to
            // wait for)
            Placed::Page => vec![v.field(SCAN), v.field(BREAKS)],
        };
        self.cx.narrow(MayDefine::Names(names));
        Ok(())
    }

    /// A piece's scan step.
    #[allow(clippy::too_many_lines)]
    fn scan_step(&mut self) -> R<Placed> {
        let v = self.v;
        let src = self.eng_read(v.field(SCAN))?;
        let s = ScanIn::decode(v, src);
        let units = v.seq_to_vec(self.cx.acc_range(v.field(crate::vprof::VPROF), s.anchor)?);
        let trace = crate::exec::env_flag!("PHITEX_SCAN_TRACE");
        if units.is_empty() {
            return Ok(Placed::Nothing);
        }
        if trace {
            eprintln!(
                "scan {}: from page {} (anchor {:?}), {} units, speculative {}",
                self.cx.node().raw(),
                s.j,
                s.anchor.map(phitex_defuse::NodeId::raw),
                units.len(),
                self.cx.is_speculative()
            );
        }
        let mut st = s.state(v);
        let material = crate::vprof::decode(&s.prof);
        let mut decoded: Vec<(i64, Vec<crate::vprof::PItem>)> = units
            .iter()
            .map(|&u| {
                let (pid, ints) = crate::vprof::unit_parts(v, u);
                (pid, crate::vprof::decode(&ints))
            })
            .collect();
        for (pid, items) in &mut decoded {
            self.resolve_prof_of(*pid, items)?;
        }
        let dead = if crate::vprof::has_end(&material)
            || decoded.iter().any(|d| crate::vprof::has_end(&d.1))
        {
            self.cint(crate::fields::cell::DEADCYCLES)?
        } else {
            0
        };
        let scan_fire = {
            let mut rd = RunReaching(self);
            if st.fold_items(&material, dead, &mut rd).is_some() {
                // (the continuation fires on this material itself: a state
                // this step read before that continuation ran)
                None
            } else {
                let last = decoded.len() - 1;
                let mut out = None;
                for (i, (pid, items)) in decoded.iter().enumerate() {
                    if let Some(f) = st.fold_items(items, dead, &mut rd) {
                        // (a page firing in an earlier piece is that
                        // piece's step's; this run read a state before it)
                        if i == last {
                            out = Some((*pid, f));
                        }
                        break;
                    }
                }
                out
            }
        };
        let Some((pid, sf)) = scan_fire else {
            return Ok(Placed::Nothing);
        };
        if trace {
            eprintln!(
                "scan {}: page {} fires in piece {pid} at candidate {}",
                self.cx.node().raw(),
                st.j,
                sf.k
            );
        }
        // (the break only: the finisher and the continuation assemble
        // the page from the items by its range)
        self.place_break(
            st.j - 1,
            src.unwrap_or(ValueId::UNIT),
            pid,
            sf.k,
            sf.rest,
            st.normal,
            sf.output_penalty <= -10001,
        )?;
        Ok(Placed::Page)
    }

    /// A scan continuation's run: its page assembled from the break
    /// ([`Run::assemble_page`]); the material the next page begins with
    /// (§1026: the routine's markers, the held-over insertions, its list,
    /// the material carried over, the page built again, the rest of the
    /// piece), its page parameter and insertion class markers resolved
    /// here; the page builder over it from the state after the page
    /// fired; the scan state defined, or the next page's break placed.
    fn scan_cont(&mut self) -> R<Placed> {
        let v = self.v;
        let b = self
            .eng_read(v.field(BREAKS))?
            .map(|x| list_of(v, x))
            .unwrap_or_default();
        if b.len() < 6 {
            return Ok(Placed::Nothing);
        }
        let pid = int_of(v, b[3]);
        let Some((after, fire, own)) = self.assemble_page(&b)? else {
            return Ok(Placed::Nothing);
        };
        let at_end = pid != -2
            && own
                .get(1)
                .is_some_and(|&x| int_of(v, x) == i64::from(fire.k));
        let rest = if at_end { Vec::new() } else { fire.rest };
        let r = self.eng_read(v.field(RTN))?;
        let rs = r.map(|x| list_of(v, x)).unwrap_or_default();
        let mut material = rs.first().map(|&x| list_of(v, x)).unwrap_or_default();
        let held = list_of(v, fire.data)
            .get(3)
            .map(|&x| list_of(v, x))
            .unwrap_or_default();
        let carried = fire.carried;
        // (the insertion classes of the material carried and held over:
        // their registers as they are here, for the page built at once;
        // a step after this reads the material, not the registers where
        // it is)
        let mut classes: Vec<u16> = Vec::new();
        for &x in held.iter().chain(carried.iter()) {
            if let Some(Node::Ins(i)) = nodes::decode_shallow(v, x, &mut Side::default())
                && !classes.contains(&i.number)
            {
                classes.push(i.number);
            }
        }
        material.extend(held);
        material.extend(rs.get(1).map(|&x| list_of(v, x)).unwrap_or_default());
        material.extend(carried);
        for k in classes {
            let m = self.ins_params_marker(k)?;
            material.push(m);
        }
        material.push(v.list(vec![v.int(nodes::T_BUILD_PAGE), v.int(0)]));
        material.extend(rest);
        self.resolve_page_params(&mut material)?;
        let dead = self.deadcycles_for(&material)?;
        let s = ScanIn {
            anchor: None,
            j: after.j,
            normal: after.normal,
            fold: after.fold,
            params: after.params,
            prof: Vec::new(),
            material: Vec::new(),
        };
        let me = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
        let state = |prof: ValueId, material: Vec<ValueId>| {
            v.list(vec![
                v.int(me),
                v.int(s.j),
                v.int(s.normal),
                v.int(i64::from(s.fold.sf)),
                v.int(i64::from(s.fold.pd)),
                v.int(i64::try_from(s.fold.since_bp).unwrap_or(0)),
                s.params.unwrap_or(ValueId::UNIT),
                prof,
                v.list(material),
            ])
        };
        let mut ps = s.page_state();
        let fire = {
            let mut live = |n: u16| self.insert_class(n);
            ps.fold_items(v, &material, dead, &mut live)?
        };
        if let Some(fire) = fire {
            if crate::exec::env_flag!("PHITEX_SCAN_TRACE") {
                eprintln!(
                    "scan {}: page {} fires in the continuation at candidate {}",
                    self.cx.node().raw(),
                    ps.j,
                    fire.k
                );
            }
            // (its break: the material from the state before it, which
            // the page's finisher and continuation fold again)
            let src = state(crate::vprof::value(v, &[]), material);
            self.place_break(
                ps.j - 1,
                src,
                -2,
                fire.k,
                fire.rest.len(),
                ps.normal,
                fire_special(v, fire.data),
            )?;
            return Ok(Placed::Page);
        }
        let prof = crate::vprof::encode(v, &material);
        let x = state(crate::vprof::value(v, &prof), material);
        self.cx.define_with(v.field(SCAN), x, Locality::Global);
        Ok(Placed::State)
    }
}
