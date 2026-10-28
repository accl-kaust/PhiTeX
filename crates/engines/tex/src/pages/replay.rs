//! The page builder on real nodes and on profiles: TeX's §980–§1026 as
//! pure functions over the items, shared by the loop (over profiles) and
//! the finisher (over the real items, to the same fire). Nothing here
//! reads the engine but through the arguments it is given, or at a piece
//! through the reads named for it (the marker resolution, the output
//! routine's hint, an insertion class's registers).
//!
//! Moved here verbatim from the old glue (`builder.rs` at `bc394b811`
//! on branch `latex-pages`), with their docs: `Src`, `Span`, `SrcRef`,
//! `Resolved`, `fold_item_t`, `is_marker_p`, `build_t`, `held_srcs`,
//! `spans_of`, `span_srcs`, `loop_item`, `loop_item_raw`, `PageState`
//! and its `impl` (the replay: `build`, `fold_items`, `fired`),
//! `class_in`, `page_vers`, the marker resolution (`page_params_at`,
//! `page_params_struct_at`, `page_cells_at`, `insert_class_at`,
//! `resolve_params_of`, `resolve_prof_of`, `resolve_prof_of_with`,
//! `OwnerReads`, `RunReaching` and its `Reaching` impl), `unit_classes`,
//! `rtn_items`/`unit_items`, `output_hint_at`, `remap_marks`,
//! `bot_marks_after`, `step_rtn`, `drop_units`. Nothing of the step
//! chain: no `PageLoop`, `Pending`, `encode_state`, `decode_state`,
//! `cut`, `step_log`, `kick`-chaining, `pages:loop`.
//!
//! The parts of the loop's state that are not the builder's own
//! ([`crate::vprof::ScanState`]) live in [`LoopCarry`]: `SPEC.md` §2.1
//! "every fire is noted with its sources", §2.2 (the routines' material
//! by page, marks in force, `\deadcycles`, the best-break penalties, the
//! last give-backs by routine and page class).

use super::*;

/// The loop's state between fires that is not the builder's own: the old
/// `PageLoop` without its builder (`st`) and without what only the chain
/// of steps needed (the placer, the finisher before, the placement made
/// at the fire).
#[derive(Clone)]
pub(crate) struct LoopCarry {
    /// The last contributor folded (the cursor of `next_contribution`).
    pub cur: Option<phitex_defuse::NodeId>,
    /// Where each item the current page's input holds came from, in the
    /// order its finisher feeds them ([`Page::spans`]): the routine's
    /// material and the held-over insertions, the contributions carried
    /// past the page before, then the pieces' items.
    pub srcs: Vec<Src>,
    /// How many of `srcs` the post-output build takes (§1026: the page
    /// before's material and carried contributions, built at once after
    /// its routine): its markers and its candidate.
    pub post: usize,
    pub post_markers: Vec<ValueId>,
    pub post_k: u32,
    /// The builder's state where the current page began: the main list's
    /// fold and the page parameters in force ([`Page::fold`]).
    pub began: (FoldState, Option<ValueId>),
    /// The marks in force where the current page began (§1012: the
    /// `\botmark` of every class before it), `[[class, mark]…]`
    /// ([`Page::top_marks`]); unit for none.
    pub top_marks: ValueId,
    /// `\deadcycles` as the routine of the page fired last left it
    /// ([`RTN`]'s third element; 0 where it has none).
    pub rtn_dead: i64,
    /// `\deadcycles` where the loop is (§1054's `\end` reads it): as the
    /// routine of the page fired last left it, else as the text set it
    /// after ([`crate::pages::DEAD_SLOT`] markers).
    pub dead: i64,
    /// The best breaks of the pages fired, penalties: 10000 from their
    /// fire on (§1013), wherever they are folded again.
    pub inf: Vec<Src>,
    /// What each routine (by its hint) gave back on the last page fired
    /// on the text at each `\outputpenalty` (every ordinary one alike,
    /// `i32::MAX`), after a page that left dead cycles or not
    /// ([`RtnKey`]), `\deadcycles` left out only on a page `\end` fired:
    /// the guess of a later page fired there ([`Run::guess_give_back`]).
    pub last_rtn: Vec<(RtnKey, ValueId)>,
    /// The page fired last: its key and what its routine gave back as
    /// the loop folded it (read or guessed), `\deadcycles` included: a
    /// routine stepping through dead cycles gives back one more item each
    /// cycle ([`step_rtn`]).
    pub prev_rtn: Option<(RtnKey, ValueId)>,
    /// The markers each routine gave back last (the page parameters it
    /// sets): an ordinary page's guess where none fired before it.
    pub last_markers: Vec<(ValueId, ValueId)>,
    /// The piece the page fired last fired in and `rtn_dead`
    /// ([`Page::prev_fire`]).
    pub prev_fire: (i64, i64),
    /// The marks the profiles' mark items name (a unit's marks given ids
    /// here as it is decoded, [`remap_marks`]): the loop's own.
    pub marks: Vec<ValueId>,
    /// The contributions not yet looked at, with their sources.
    pub pend: Vec<(crate::vprof::PItem, SrcRef)>,
    /// The sources of the builder's page, item for item.
    pub page_src: Vec<SrcRef>,
    /// The units folded whose items a page's carried contributions may
    /// fold again (the oldest dropped once no page needs them).
    pub units: Vec<UnitRec>,
    /// The contributor with units before each piece folded: a
    /// finisher's window of [`PITEMS`] starts after it.
    pub prev: phitex_defuse::FxHashMap<i64, Option<phitex_defuse::NodeId>>,
    /// The last contributor folded that gave units ([`LoopCarry::prev`]).
    pub cur_units: Option<phitex_defuse::NodeId>,
    /// The routines' material the loop folded, by page: `(page, value,
    /// its markers then its list as profile items)`.
    pub rtns: Vec<(i64, ValueId, std::sync::Arc<Vec<crate::vprof::PItem>>)>,
    /// Where the pieces' items are folded to: `(unit index in `units`,
    /// the next item)`.
    pub frontier: (usize, u32),
    /// The insertion class of each item of the builder's page (none for
    /// any other item).
    pub page_ins: Vec<Option<u16>>,
    /// The insertions the last fire held over, measured, by source.
    pub held_nodes: Vec<(Src, Node)>,
    /// Each piece's place among those folded (the earliest a page names
    /// starts its finisher's window).
    pub order: phitex_defuse::FxHashMap<i64, u64>,
    /// The next piece's place in [`LoopCarry::order`].
    pub seq: u64,
    /// What the loop read where the owner of the units it folds last is
    /// ([`OwnerReads`]): a piece's unit and its paragraphs' lines' units
    /// share it.
    pub own: OwnerReads,
    /// The contributor the last page fired in and the pages fired in it
    /// ([`Page::nth`]).
    pub nth: (Option<phitex_defuse::NodeId>, u32),
    /// Whether `\end` has ended the job (§1054: its page empty, nothing
    /// left, no dead cycle), or the job was aborted: nothing after it is
    /// read.
    pub ended: bool,
}

impl LoopCarry {
    /// The loop at the job's start: no page fired, the main list empty.
    pub(crate) fn new() -> LoopCarry {
        LoopCarry {
            cur: None,
            srcs: Vec::new(),
            post: 0,
            post_markers: Vec::new(),
            post_k: 0,
            began: (crate::build::IGNORE_DEPTH_STATE, None),
            top_marks: ValueId::UNIT,
            rtn_dead: 0,
            dead: 0,
            inf: Vec::new(),
            last_rtn: Vec::new(),
            prev_rtn: None,
            last_markers: Vec::new(),
            prev_fire: (-1, 0),
            marks: Vec::new(),
            pend: Vec::new(),
            page_src: Vec::new(),
            units: Vec::new(),
            prev: phitex_defuse::FxHashMap::default(),
            cur_units: None,
            rtns: Vec::new(),
            frontier: (0, 0),
            page_ins: Vec::new(),
            held_nodes: Vec::new(),
            order: phitex_defuse::FxHashMap::default(),
            seq: 0,
            own: OwnerReads::default(),
            nth: (None, 0),
            ended: false,
        }
    }
}

impl Default for LoopCarry {
    fn default() -> LoopCarry {
        LoopCarry::new()
    }
}

/// The builder over profiles at the job's start.
pub(crate) fn fresh_scan() -> crate::vprof::ScanState {
    crate::vprof::ScanState::fresh(0, 0, crate::build::IGNORE_DEPTH_STATE, None)
}

/// A page fired: what the fold after it needs (§1017–§1026), kept with
/// its checkpoint until its give-back is folded.
#[derive(Clone, Debug)]
pub(crate) struct FireNote {
    pub j: i64,
    /// The contributor the page fired in, the unit of its contribution
    /// the fire was in, and that unit's piece.
    pub node: phitex_defuse::NodeId,
    pub xs_i: usize,
    /// The contribution of `node` the fire was folded from (its units as
    /// one sequence value): a run resuming at this fire folds the rest of
    /// `node` from these, not from a read of `node` again, so the read
    /// the fold rested on stays the one checked at the loop's commit (a
    /// read again would answer the new units and hide that the fold up
    /// to here was made from the old). `UNIT`: not known.
    pub units: ValueId,
    pub pid: i64,
    pub in_material: bool,
    pub hint: ValueId,
    pub key: RtnKey,
    /// What the routine is guessed to give back ([`Run::guess_give_back`]).
    pub rtn_guess: ValueId,
    pub k: u32,
    pub output_penalty: i32,
    pub held: Vec<(Src, Node)>,
    pub carried: Vec<SrcRef>,
    /// The pages fired in `node` before this one.
    pub nth: u32,
    /// The insertion classes' registers of the unit being folded.
    pub classes: Vec<(u16, InsertClass)>,
}

/// What the finisher's assembly gives the replay: each unit the page
/// names (`(piece, shape, items)`, the own piece's shape `[piece, last
/// candidate or -2]`), and the own piece's view for the parts read as
/// they are used.
pub(crate) struct Assembled {
    pub units: Vec<(i64, ValueId, Vec<ValueId>)>,
    pub own: Option<(phitex_defuse::View, u32)>,
}

/// A page assembled and fired on the real nodes: the routine's input
/// ([`super::FinCtx::args`]'s layout: `[nth, data, carried, locals,
/// rest, piece, k, reads, defs, hint, normal, special, cuttable,
/// start]`) and the break in its piece (`[piece, candidate, inside]`).
pub(crate) struct Shipped {
    pub args: Vec<ValueId>,
    pub brk: Option<(i64, i64, bool)>,
}
/// Where an item the page builder took came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Src {
    /// Item `item` of piece `piece`'s items (its [`PITEMS`] unit: one
    /// profile item per raw item, so the loop's position in a profile is
    /// the position in the items).
    Piece { piece: i64, item: u32 },
    /// Item `item` of what page `page`'s routine gave back (the list of
    /// its [`RTN`] value, §1026), as the loop folded it: the guess, or the
    /// real one where it missed.
    Rtn { page: i64, item: u32 },
    /// The insertion at item `item` of piece `piece`, its list from
    /// `from` on: held over to the next page (§1021: what a split left,
    /// or the whole insertion with `from` 0).
    Held { piece: i64, item: u32, from: u32 },
}

/// `len` items from consecutive positions of one source ([`Src::Held`]:
/// one insertion, `len` 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub src: Src,
    pub len: u32,
}

/// Where an item the loop took came from, and the main list's fold just
/// before it: a piece's raw item folds again from there where it goes
/// back to the contributions (§1017).
#[derive(Clone, Copy, Debug)]
pub(crate) struct SrcRef {
    /// `None`: made by the builder itself (`\topskip`'s glue, §1001).
    pub src: Option<Src>,
    pub fold: FoldState,
}

/// An `\end` marker's own items (§1054: the box, the glue, the penalty it
/// puts on the list) as sources: item `sub` of the marker at `item`.
pub(crate) const fn end_sub(item: u32, sub: u32) -> u32 {
    item | ((sub + 1) << 24)
}

/// The marker's item and its own item's index, where `item` is one.
pub(crate) const fn end_sub_of(item: u32) -> Option<(u32, u32)> {
    if item >> 24 == 0 {
        None
    } else {
        Some((item & 0xff_ffff, (item >> 24) - 1))
    }
}

/// A unit the loop folded: the piece it names, its items.
#[derive(Clone, Debug)]
pub(crate) struct UnitRec {
    pub pid: i64,
    /// As decoded, shared ([`crate::vprof::decoded`]): its markers read
    /// where the piece is are `res`'s.
    pub items: std::sync::Arc<Vec<crate::vprof::PItem>>,
    pub res: Resolved,
    /// The profile unit it was decoded from (its version, [`loop_state`]).
    pub src: ValueId,
}

/// A unit's markers as read where its piece is ([`Run::unit_items`]):
/// the page parameters and the insertion classes' registers, given to
/// its items unread in their place ([`Resolved::of`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct Resolved {
    pub params: Option<PageParams>,
    pub classes: Vec<(u16, InsertClass)>,
}

impl Resolved {
    /// Item `x` with what it names read, where it names any.
    pub(crate) fn of(&self, x: &crate::vprof::PItem) -> Option<crate::vprof::PItem> {
        use crate::vprof::PItem;
        match x {
            PItem::Params(None) => self.params.map(|p| PItem::Params(Some(p))),
            PItem::InsParams(n, None) => self
                .classes
                .iter()
                .find(|c| c.0 == *n)
                .map(|c| PItem::InsParams(*n, Some(c.1))),
            _ => None,
        }
    }
}

/// The reads at one owner piece ([`crate::vprof::unit_owner`]): its
/// insertion classes' registers, read once for all the units it owns
/// ([`Run::resolve_prof_of`], [`Run::insert_class_at`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct OwnerReads {
    pub owner: Option<i64>,
    pub classes: Vec<(u16, InsertClass)>,
}

/// A page the loop's builder fired, with the sources of what it took.
pub(crate) struct LoopFire {
    pub k: u32,
    pub output_penalty: i32,
    pub best: usize,
    pub holding: bool,
    pub classes: Vec<u16>,
    /// The insertions held over (§1022), measured, with their sources;
    /// `held_named` false where a source could not be named (a defect,
    /// said where the page is noted).
    pub held: Vec<(Src, Node)>,
    pub held_named: bool,
    /// The marks on the page, in order (ids in [`LoopCarry::marks`]).
    pub marks: Vec<u32>,
    /// The best break where it is a penalty (§1013: 10000 after this
    /// fire, not before).
    pub brk: Option<Src>,
    /// What goes back to the contributions (§1017), with their sources:
    /// the page from its best break on, the item that fired it, the
    /// contributions after it.
    pub carried: Vec<SrcRef>,
}

/// The profile item a source names, as the loop has it.
pub(crate) fn loop_item(lc: &LoopCarry, src: Src) -> Option<crate::vprof::PItem> {
    use crate::vprof::PItem;
    // (§1013: a page's best break, a penalty, set to 10000 where the page
    // fired: it goes back to the contributions so)
    if lc.inf.contains(&src) {
        return match loop_item_raw(lc, src)? {
            PItem::Node(Node::Penalty(_)) => {
                Some(PItem::Node(Node::Penalty(phitex_typeset::linebreak::INF_PENALTY)))
            }
            x => Some(x),
        };
    }
    loop_item_raw(lc, src)
}

/// [`loop_item`] as the source holds it.
pub(crate) fn loop_item_raw(lc: &LoopCarry, src: Src) -> Option<crate::vprof::PItem> {
    use crate::vprof::PItem;
    match src {
        Src::Piece { piece, item } => {
            let u = lc.units.iter().rev().find(|u| u.pid == piece)?;
            match end_sub_of(item) {
                None => u.items.get(item as usize).map(|x| u.res.of(x).unwrap_or_else(|| x.clone())),
                Some((e, s)) => match u.items.get(e as usize)? {
                    PItem::End(ys) => ys.get(s as usize).cloned(),
                    _ => None,
                },
            }
        }
        Src::Rtn { page, item } => {
            let (_, _, items) = lc.rtns.iter().rev().find(|r| r.0 == page)?;
            items.get(item as usize).cloned()
        }
        Src::Held { .. } => None,
    }
}

/// [`crate::vprof::ScanState`]'s fold of one item with its source
/// (§679's interline glue, deletions, markers), into `lc.pend`.
pub(crate) fn fold_item_t(st: &mut crate::vprof::ScanState, lc: &mut LoopCarry, x: &crate::vprof::PItem, s: SrcRef) {
    use crate::vprof::PItem;
    let st = &mut st.fold;
    let out = &mut lc.pend;
    match x {
        PItem::Delete => {
            if st.since_bp > 0 {
                if let Some(i) = out.iter().rposition(|y| !is_marker_p(&y.0)) {
                    out.remove(i);
                }
                st.since_bp -= 1;
            }
        }
        PItem::Params(_) | PItem::SetPage(..) | PItem::InsParams(..) => out.push((x.clone(), s)),
        PItem::Opaque(t)
            if matches!(*t, nodes::T_WRITE_AT | nodes::T_FONT_AT | nodes::T_PDFW_AT) =>
        {
            out.push((x.clone(), s));
        }
        PItem::Build(_) => st.since_bp = 0,
        PItem::Pd(d) => st.pd = *d,
        PItem::VBox { node, bs, ls, lsl } => {
            let ht = node.box_dims().map_or(0, |d| d.1);
            if let Some(g) = phitex_typeset::linebreak::interline_glue(st.pd, ht, *bs, *ls, *lsl) {
                out.push((PItem::Node(g), s));
                st.since_bp += 1;
            }
            out.push((PItem::Node(node.clone()), s));
            st.since_bp += 1;
            st.pd = node.box_dims().map_or(0, |d| d.2);
        }
        PItem::Sf(k, sf) => {
            st.sf = if *k == 0 || st.sf >= 1000 { *sf } else { 1000 };
        }
        _ => {
            out.push((x.clone(), s));
            st.since_bp += 1;
        }
    }
}

/// The page builder's markers in a profile.
pub(crate) fn is_marker_p(x: &crate::vprof::PItem) -> bool {
    use crate::vprof::PItem;
    match x {
        PItem::Params(_) | PItem::SetPage(..) | PItem::InsParams(..) => true,
        PItem::Opaque(t) => matches!(*t, nodes::T_WRITE_AT | nodes::T_FONT_AT | nodes::T_PDFW_AT),
        _ => false,
    }
}

/// One `build_page` call (§994) over `lc.pend` at candidate `k`, with the
/// sources of what the page takes ([`crate::vprof::ScanState`]'s build).
/// `class` answers for an insertion class no marker names.
#[allow(clippy::too_many_lines)]
pub(crate) fn build_t(
    st: &mut crate::vprof::ScanState,
    lc: &mut LoopCarry,
    k: u32,
    class_of_live: &mut dyn FnMut(u16) -> InsertClass,
) -> Option<LoopFire> {
    use crate::vprof::PItem;
    let mut classes: Vec<(u16, InsertClass)> = Vec::new();
    let mut page_params = None;
    let mut buf = std::mem::take(&mut lc.pend);
    let mut dead_set = None;
    {
        let so_far = &mut st.pb.so_far;
        buf.retain(|(x, _)| match x {
            PItem::Params(p) => {
                // (`None`, the parameters reaching a piece: those the fold
                // has in force, docs/spec-settle.md §2.1)
                if p.is_some() {
                    page_params = *p;
                }
                false
            }
            PItem::SetPage(i, x) => {
                if *i == crate::pages::DEAD_SLOT {
                    dead_set = Some(i64::from(*x));
                } else if let Ok(i) = usize::try_from(*i)
                    && i < 8
                {
                    so_far[i] = *x;
                }
                false
            }
            PItem::InsParams(n, c) => {
                classes.push((*n, c.unwrap_or_else(|| class_of_live(*n))));
                false
            }
            _ => true,
        });
    }
    if let Some(d) = dead_set {
        lc.dead = d;
    }
    let dead = lc.dead;
    if page_params.is_some() {
        st.params = page_params;
    }
    if crate::exec::obs_str!("PHITEX_PAGE_DUMP").is_some_and(|d| d == st.j.to_string()) {
        eprintln!(
            "dump loop page {}: build {k} topskip {:?}",
            st.j,
            st.params.map(|p| p.top_skip.width)
        );
    }
    let params = st.params.unwrap_or(PageParams {
        vsize: 0,
        max_depth: 0,
        top_skip: Glue::ZERO,
        holding_inserts: 0,
        saving_vdiscards: false,
    });
    // (the contributions in order, the next at the front)
    let mut queue: std::collections::VecDeque<(PItem, SrcRef)> = buf.into();
    let mut end_seen: Option<(PItem, SrcRef)> = None;
    while let Some((x, s)) = queue.pop_front() {
        let p = match x {
            PItem::End(ys) => {
                if st.pb.page.is_empty() && queue.is_empty() && dead == 0 {
                    lc.ended = true;
                    continue;
                }
                let base = match s.src {
                    Some(Src::Piece { piece, item }) => Some((piece, item)),
                    _ => None,
                };
                for (i, y) in ys.iter().enumerate().rev() {
                    let src = base.map(|(piece, item)| Src::Piece {
                        piece,
                        item: end_sub(item, u32::try_from(i).unwrap_or(0)),
                    });
                    queue.push_front((y.clone(), SrcRef { src, fold: s.fold }));
                }
                end_seen = Some((PItem::End(ys), s));
                continue;
            }
            PItem::Abort => {
                lc.ended = true;
                return None;
            }
            PItem::Node(n) => n,
            _ => continue,
        };
        let next = if matches!(p, Node::Kern(..)) {
            match queue.front() {
                Some((PItem::Node(n), _)) => Some(n.clone()),
                _ => None,
            }
        } else {
            None
        };
        let before = st.pb.page.len();
        if crate::exec::obs_str!("PHITEX_PAGE_DUMP").is_some_and(|d| d == st.j.to_string()) {
            eprintln!("dump loop page {}: {} from {:?}", st.j, node_short(&p), s.src);
        }
        let ins_n = if let Node::Ins(i) = &p { Some(i.number) } else { None };
        let out = {
            let mut class = |n: u16| {
                classes
                    .iter()
                    .find(|c| c.0 == n)
                    .map_or_else(|| class_of_live(n), |c| c.1)
            };
            st.pb.contribute(p, next.as_ref(), &params, &mut class)
        };
        match out {
            Outcome::Done => {
                let after = st.pb.page.len();
                if after == before + 2 {
                    // (`\topskip`'s glue before the first box, §1001)
                    lc.page_src.push(SrcRef { src: None, fold: s.fold });
                    lc.page_ins.push(None);
                }
                if after > before {
                    lc.page_src.push(s);
                    lc.page_ins.push(ins_n);
                }
            }
            Outcome::Wait(n) => {
                queue.push_front((PItem::Node(n), s));
                break;
            }
            Outcome::Fire(f) => {
                let srcs = std::mem::take(&mut lc.page_src);
                let ins = std::mem::take(&mut lc.page_ins);
                // (§1001: `\topskip`'s glue fired it, the box after it
                // goes back too)
                let topskip = f.contributions.len() == before.saturating_sub(f.best) + 2;
                let holding = params.holding_inserts > 0;
                let held = held_srcs(&f, &srcs, &ins, holding);
                let held_named = held.is_some();
                // (§1013: the best break, a penalty, is 10000 from here on)
                let mut brk = None;
                if f.output_penalty < phitex_typeset::linebreak::INF_PENALTY {
                    brk = srcs.get(f.best).map_or(s.src, |b| b.src);
                    if let Some(b) = brk
                        && !lc.inf.contains(&b)
                    {
                        lc.inf.push(b);
                    }
                }
                let mut carried: Vec<SrcRef> = srcs.get(f.best..).unwrap_or_default().to_vec();
                if topskip {
                    carried.push(SrcRef { src: None, fold: s.fold });
                }
                carried.push(s);
                carried.extend(queue.drain(..).map(|(_, s)| s));
                if let Some((_, s)) = end_seen {
                    carried.push(s);
                }
                st.pb = PageBuilder::new();
                st.pb.measure_only = true;
                st.j += 1;
                if f.output_penalty > -10001 {
                    st.normal += 1;
                }
                st.fold.since_bp = 0;
                let held = held
                    .unwrap_or_default()
                    .into_iter()
                    .zip(f.held_inserts.iter().cloned())
                    .collect();
                return Some(LoopFire {
                    k,
                    output_penalty: f.output_penalty,
                    best: f.best,
                    holding,
                    classes: f.ins_records.iter().map(|r| r.n).collect(),
                    held,
                    held_named,
                    marks: f.marks.clone(),
                    brk,
                    carried,
                });
            }
        }
    }
    lc.pend = queue.into();
    None
}


/// The sources of the insertions a fire holds over (§1020–§1022), in
/// order: each insertion before the best break whose class has no best
/// insertion or comes after it, all of them where `\holdinginserts` is
/// positive, and the rest of the one split where a rest is left. `None`
/// where one cannot be named by a piece's item ([`Src::Held`]): a defect,
/// said by the caller.
pub(crate) fn held_srcs(f: &Fired, page_src: &[SrcRef], page_ins: &[Option<u16>], holding: bool) -> Option<Vec<Src>> {
    if f.held_inserts.is_empty() {
        return Some(Vec::new());
    }
    let mut recs = f.ins_records.clone();
    // (sure ones, and the rest of a split, which pruning may leave empty)
    let mut out: Vec<(Src, bool)> = Vec::new();
    for (idx, ins) in page_ins.iter().enumerate().take(f.best) {
        let Some(n) = *ins else { continue };
        let src = page_src.get(idx).and_then(|s| s.src)?;
        let (piece, item, from) = match src {
            Src::Piece { piece, item } if end_sub_of(item).is_none() => (piece, item, 0),
            Src::Held { piece, item, from } => (piece, item, from),
            _ => return None,
        };
        if holding {
            out.push((Src::Held { piece, item, from }, true));
            continue;
        }
        let r = recs.iter_mut().find(|r| r.n == n)?;
        let mut wait = true;
        let mut maybe = false;
        let mut at = from;
        if r.best_ins.is_some() {
            wait = false;
            if r.best_ins == Some(idx) {
                if r.split_up
                    && r.broken_ins == Some(idx)
                    && let Some(q) = r.broken_ptr
                {
                    // (a rest held again after a split of a rest: its
                    // items are not the piece's from an index on)
                    if from != 0 {
                        return None;
                    }
                    wait = true;
                    maybe = true;
                    at = u32::try_from(q).ok()?;
                }
                r.best_ins = None;
            }
        }
        if wait {
            out.push((Src::Held { piece, item, from: at }, !maybe));
        }
    }
    let sure = out.iter().filter(|x| x.1).count();
    let keep_maybe = out.len() == f.held_inserts.len();
    if !keep_maybe && sure != f.held_inserts.len() {
        return None;
    }
    Some(
        out.into_iter()
            .filter(|x| x.1 || keep_maybe)
            .map(|x| x.0)
            .collect(),
    )
}

/// Consecutive sources as spans.
pub(crate) fn spans_of(srcs: &[Src]) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for &s in srcs {
        if let Some(last) = out.last_mut() {
            let next = match last.src {
                Src::Piece { piece, item } => match s {
                    Src::Piece { piece: p, item: i } => {
                        p == piece
                            && end_sub_of(item).is_none()
                            && end_sub_of(i).is_none()
                            && i == item + last.len
                    }
                    _ => false,
                },
                Src::Rtn { page, item } => {
                    matches!(s, Src::Rtn { page: p, item: i } if p == page && i == item + last.len)
                }
                Src::Held { .. } => false,
            };
            if next {
                last.len += 1;
                continue;
            }
        }
        out.push(Span { src: s, len: 1 });
    }
    out
}

/// The sources a span names, in order.
pub(crate) fn span_srcs(s: &Span) -> impl Iterator<Item = Src> + '_ {
    (0..s.len).map(move |d| match s.src {
        Src::Piece { piece, item } => Src::Piece { piece, item: item + d },
        Src::Rtn { page, item } => Src::Rtn { page, item: item + d },
        h @ Src::Held { .. } => h,
    })
}

/// A page parameter marker (`T_PAGE_PARAMS`) of `p`.
pub(crate) fn params_marker(v: &Values, p: &PageParams) -> ValueId {
    v.list(vec![
        v.int(nodes::T_PAGE_PARAMS),
        v.int(i64::from(p.vsize)),
        v.int(i64::from(p.max_depth)),
        glue_value(v, &p.top_skip),
        v.int(i64::from(p.holding_inserts)),
    ])
}

/// A decoded unit's mark items given ids in the loop's table: `ms` are
/// the unit's marks ([`crate::vprof::unit_marks`]), an item's index into
/// them becomes the index of the mark pushed on `table` (a mark the
/// profile names none of, `u32::MAX`, stays).
pub(crate) fn remap_marks(items: &mut [crate::vprof::PItem], ms: &[ValueId], table: &mut Vec<ValueId>) {
    use crate::vprof::PItem;
    for x in items {
        match x {
            PItem::Node(Node::Mark(i)) => {
                *i = match ms.get(*i as usize) {
                    Some(&m) => {
                        table.push(m);
                        u32::try_from(table.len() - 1).unwrap_or(u32::MAX)
                    }
                    None => u32::MAX,
                };
            }
            PItem::End(ys) => remap_marks(ys, ms, table),
            _ => {}
        }
    }
}

/// The marks in force after a page (§1012–§1014): `before`'s
/// (`[[class, mark]…]`, unit for none), each class's last mark on the
/// page (`page`, ids in `table`) in its place.
pub(crate) fn bot_marks_after(v: &Values, before: ValueId, page: &[u32], table: &[ValueId]) -> ValueId {
    if page.is_empty() {
        return before;
    }
    let mut cur: Vec<(i64, ValueId)> = if before == ValueId::UNIT {
        Vec::new()
    } else {
        list_of(v, before)
            .into_iter()
            .filter_map(|e| match list_of(v, e)[..] {
                [c, m] => Some((int_of(v, c), m)),
                _ => None,
            })
            .collect()
    };
    for &k in page {
        let Some(&m) = table.get(k as usize) else { continue };
        let (c, text) = crate::etex::mark_class(v, m);
        let c = i64::from(c);
        match cur.iter_mut().find(|e| e.0 == c) {
            Some(e) => e.1 = text,
            None => cur.push((c, text)),
        }
    }
    cur.sort_by_key(|e| e.0);
    v.list(cur.into_iter().map(|(c, m)| v.list(vec![v.int(c), m])).collect())
}

/// (`observe`) Page `j`'s guess depends on the pages before it alone
/// ([`Run::guess_give_back`]): the same give-backs of the pages before `j`
/// (what the loop took for them, outputs read or guesses), the same
/// routine and `\outputpenalty`, give the same guess in every attempt,
/// whatever page `j`'s own output; one changed by it is a defect (its
/// finisher placed anew on its own output).
type GuessSeen = Option<phitex_defuse::FxHashMap<(u64, i64, u64), ValueId>>;
static GUESS_SEEN: std::sync::Mutex<GuessSeen> = std::sync::Mutex::new(None);

/// At a collection ([`crate::collect`]): the guesses [`guess_check`] saw
/// dropped (a guess made again after it may have another id).
pub(crate) fn clear_guess_seen() {
    *GUESS_SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

pub(crate) fn guess_check(node: u64, j: i64, how: impl std::hash::Hash, guess: ValueId) {
    use std::hash::Hasher as _;
    static SEEN: &std::sync::Mutex<GuessSeen> = &GUESS_SEEN;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&how, &mut h);
    let key = (node, j, h.finish());
    let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let prev = *seen.get_or_insert_with(Default::default).entry(key).or_insert(guess);
    assert!(
        prev == guess,
        "page {}: its routine's guess changed with nothing before it changed (its own output read back?)",
        j + 1
    );
}

/// (`observe`, `PHITEX_PAGE_DUMP=<page>`) A node in a line: its kind and
/// what the page builder measures of it.
pub(crate) fn node_short(n: &Node) -> String {
    match n {
        Node::Box(b) => format!("box {:?} {}+{}", b.kind, b.height, b.depth),
        Node::Rule(r) => format!("rule {}+{}", r.height, r.depth),
        Node::Glue(g) => format!("glue {}", g.spec.width),
        Node::Kern(w, _) => format!("kern {w}"),
        Node::Penalty(p) => format!("penalty {p}"),
        Node::Mark(_) => "mark".to_owned(),
        Node::Ins(i) => format!("ins {} {}+{}", i.number, i.height, i.depth),
        Node::Whatsit(..) => "whatsit".to_owned(),
        _ => "other".to_owned(),
    }
}

/// The key of a routine's give-back the loop guesses from
/// ([`LoopCarry::last_rtn`]): the routine's hint, the page's class
/// ([`pen_class`]) and whether the page before
/// left dead cycles.
pub(crate) type RtnKey = (ValueId, i32, bool);

/// The penalty `\end` fires a page at (§1054).
pub(crate) const END_PENALTY: i32 = -0o10000000000;

/// A dead cycle's give-back guessed from the one before it (`x`, which
/// left `\deadcycles` above 0): its list one item longer (the last again:
/// a routine holding back the `\write`s of the pages it did not ship,
/// one more each cycle) and `\deadcycles` one more; `None` where `x`
/// left none or gave back no list.
pub(crate) fn step_rtn(v: &Values, x: ValueId) -> Option<ValueId> {
    let xs = list_of(v, x);
    let dead = xs.get(2).map_or(0, |&d| int_of(v, d));
    if dead <= 0 {
        return None;
    }
    let mut list = list_of(v, *xs.get(1)?);
    list.push(*list.last()?);
    Some(rtn_value(
        v,
        xs[0],
        v.list(list),
        dead + 1,
    ))
}

impl Run<'_, '_> {
    /// A [`crate::vprof::VPROF`] unit's piece and items: decoded, its
    /// marks given ids in the loop's table, its markers resolved where
    /// its piece is.
    pub(crate) fn unit_items(
        &mut self,
        lc: &mut LoopCarry,
        x: ValueId,
    ) -> R<(i64, std::sync::Arc<Vec<crate::vprof::PItem>>, Resolved)> {
        use crate::prof::pages as pp;
        use crate::vprof::PItem;
        let v = self.v;
        let t = pp::start();
        let d = crate::vprof::decoded(v, x);
        let pid = d.pid;
        pp::add(pp::DECODE, t);
        // (the piece whose run made its items: the contributor)
        if lc.own.owner != Some(pid) {
            lc.own = OwnerReads {
                owner: Some(pid),
                ..OwnerReads::default()
            };
        }
        // (its markers read where the piece is, kept beside the items)
        let mut res = Resolved::default();
        if d.unresolved {
            let t = pp::start();
            let mut own = std::mem::take(&mut lc.own);
            let mut probe: Vec<PItem> = d
                .items
                .iter()
                .filter(|y| matches!(y, PItem::Params(None) | PItem::InsParams(_, None)))
                .cloned()
                .collect();
            let r = self.resolve_prof_of_with(pid, &mut probe, &mut own);
            lc.own = own;
            pp::add(pp::RESOLVE, t);
            r?;
            for y in probe {
                match y {
                    PItem::Params(p) => res.params = p,
                    PItem::InsParams(n, Some(c)) => res.classes.push((n, c)),
                    _ => {}
                }
            }
        }
        // (a mark's id is the loop's own: those items copied)
        if !d.marks {
            return Ok((pid, std::sync::Arc::clone(&d.items), res));
        }
        let mut items = (*d.items).clone();
        remap_marks(&mut items, &crate::vprof::unit_marks(v, x), &mut lc.marks);
        Ok((pid, std::sync::Arc::new(items), res))
    }

    /// A routine's material as profile items (§1026: its markers, then
    /// its list), its marks given ids in the loop's table.
    pub(crate) fn rtn_items(&mut self, lc: &mut LoopCarry, rtn_value: ValueId) -> Vec<crate::vprof::PItem> {
        let v = self.v;
        let rs = list_of(v, rtn_value);
        let mut raw = rs.first().map(|&x| list_of(v, x)).unwrap_or_default();
        raw.extend(rs.get(1).map(|&x| list_of(v, x)).unwrap_or_default());
        let (ints, ms) = crate::vprof::encode_marks(v, &raw);
        let mut items = crate::vprof::decode(&ints);
        remap_marks(&mut items, &ms, &mut lc.marks);
        items
    }
}

impl Run<'_, '_> {
    /// The registers of the insertion classes a unit's `items` and the
    /// contributions pending put on the page, as they reach it at `at`,
    /// where no marker names them (§1008 reads them at the `build_page`
    /// call); memoised for the unit's owner in `lc.own`.
    pub(crate) fn unit_classes(&mut self, lc: &mut LoopCarry, items: &[crate::vprof::PItem], at: phitex_defuse::NodeId) -> R<Vec<(u16, InsertClass)>> {
        use crate::prof::pages as pp;
        let t = pp::start();
        let mut ns: Vec<u16> = Vec::new();
        for y in items.iter().chain(lc.pend.iter().map(|p| &p.0)) {
            if let crate::vprof::PItem::Node(Node::Ins(i)) = y
                && !ns.contains(&i.number)
            {
                ns.push(i.number);
            }
        }
        let mut classes = Vec::with_capacity(ns.len());
        for n in ns {
            if let Some(&c) = lc.own.classes.iter().find(|c| c.0 == n) {
                classes.push(c);
                continue;
            }
            let c = self.insert_class_at(n, at);
            if c.is_err() {
                pp::add(pp::CLASSES, t);
            }
            let c = (n, c?);
            lc.own.classes.push(c);
            classes.push(c);
        }
        pp::add(pp::CLASSES, t);
        Ok(classes)
    }


    /// The output routine's hint (as [`Run::output_hint`]) where piece
    /// `at` ends: the routine of a page firing there.
    pub(crate) fn output_hint_at(&mut self, at: phitex_defuse::NodeId) -> R<ValueId> {
        // (the last routine's hint, by its value and the definitions: a
        // page's routine is mostly the page before's)
        type Last = Option<(u64, Option<ValueId>, usize, ValueId)>;
        thread_local! {
            static LAST: std::cell::Cell<Last> = const { std::cell::Cell::new(None) };
        }
        let v = self.v;
        let f = self.loc_field(Par::output as Loc);
        let out = match self.field_at(f, at, false)? {
            phitex_defuse::FieldAt::Def(x, _) => Some(x),
            phitex_defuse::FieldAt::Unset => None,
            phitex_defuse::FieldAt::Gone => self.par(Par::output)?,
        };
        let defs = self.ops.defs.locked().clone();
        let d = std::sync::Arc::as_ptr(&defs) as usize;
        if let Some((i, o, dp, h)) = LAST.get()
            && i == v.instance()
            && o == out
            && dp == d
        {
            return Ok(h);
        }
        let toks = toks_of(v, out);
        let h = if toks.is_empty() {
            ValueId::UNIT
        } else {
            let mut text = String::new();
            self.show_toks_into(&mut text, &toks)?;
            v.str(&crate::hints::routine_hint(&text, &defs).encode())
        };
        LAST.set(Some((v.instance(), out, d, h)));
        Ok(h)
    }
}

impl Run<'_, '_> {
    /// A held-over insertion as the loop measured it (the source names it).
    pub(crate) fn loop_node(lc: &LoopCarry, src: Src) -> Option<crate::vprof::PItem> {
        match src {
            Src::Held { .. } => lc
                .held_nodes
                .iter()
                .find(|h| h.0 == src)
                .map(|h| crate::vprof::PItem::Node(h.1.clone())),
            _ => None,
        }
    }

    /// Drops the units older than every source the current page's input
    /// names (their items are folded again only from there on).
    /// The first piece the input names is the oldest: `lc.srcs` names
    /// the pieces' items in the order they were folded (a fire keeps a
    /// suffix of them, after the routine's material and the held-over
    /// insertions, which name no unit), so no other is looked up.
    pub(crate) fn drop_units(lc: &mut LoopCarry) {
        let first = lc.srcs.iter().find_map(|s| match s {
            Src::Piece { piece, .. } => Some(*piece),
            _ => None,
        });
        let keep = first
            .and_then(|p| lc.units.iter().position(|u| u.pid == p))
            .map_or(lc.frontier.0, |i| i.min(lc.frontier.0));
        if cfg!(feature = "observe") {
            let mut all = lc.frontier.0;
            for s in &lc.srcs {
                if let Src::Piece { piece, .. } = s
                    && let Some(i) = lc.units.iter().position(|u| u.pid == *piece)
                {
                    all = all.min(i);
                }
            }
            assert_eq!(keep, all, "the page's input names an older piece after a newer one");
        }
        if keep > 0 {
            lc.units.drain(..keep);
            lc.frontier.0 -= keep;
        }
        // (the places of the pieces the page's input and the units carried
        // name: the others' go, the loop is copied at every mark)
        let named = lc.units.len() + lc.srcs.len() + lc.pend.len() + lc.held_nodes.len();
        if lc.order.len() > 2 * named + 64 {
            let mut keep: phitex_defuse::FxHashSet<i64> = lc.units.iter().map(|u| u.pid).collect();
            let piece = |s: &Src| match *s {
                Src::Piece { piece, .. } | Src::Held { piece, .. } => Some(piece),
                Src::Rtn { .. } => None,
            };
            keep.extend(lc.srcs.iter().filter_map(piece));
            keep.extend(lc.pend.iter().filter_map(|p| p.1.src.as_ref().and_then(piece)));
            keep.extend(lc.page_src.iter().filter_map(|p| p.src.as_ref().and_then(piece)));
            keep.extend(lc.held_nodes.iter().filter_map(|h| piece(&h.0)));
            lc.order.retain(|p, _| keep.contains(p));
            lc.prev.retain(|p, _| keep.contains(p));
        }
    }
}

/// A class's registers as read for a unit, or nothing where none was.
pub(crate) fn class_in(classes: &[(u16, InsertClass)], n: u16) -> InsertClass {
    classes.iter().find(|c| c.0 == n).map_or(
        InsertClass {
            box_height_plus_depth: None,
            box_is_hbox: false,
            count: 0,
            dimen: 0,
            skip: Glue::ZERO,
        },
        |c| c.1,
    )
}

/// A piece's node from its raw id.
pub(crate) fn node_of(pid: i64) -> phitex_defuse::NodeId {
    phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()))
}

// ------------------------------------------- the builder on real nodes

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
    /// The page parameters in force as values, until a marker names
    /// them (a state taken over from the builder over profiles, where
    /// the marker in force is not read).
    pub params_pre: Option<PageParams>,
    /// Contributions after the last candidate: folded items and markers,
    /// in order; the next candidate takes them first (§994).
    pub pending: Vec<ValueId>,
    /// The pages fired so far whose `\outputpenalty` is above -10001
    /// (the ordinary ones: the finishers' predictions step page
    /// counters by these, [`FinDefs::predict`]).
    pub normal: i64,
    /// `\deadcycles` where the state is (§1054's `\end` reads it, §1012's
    /// fire counts from it): as the routine before left it, else as the
    /// text set it after ([`crate::pages::DEAD_SLOT`] markers).
    pub dead: i64,
}

/// A page finishing: at candidate `k`, the finisher's page data, the
/// material to carry over past it, and the items after the candidate
/// (not folded: the finisher's fold takes them after the routine's).
pub(crate) struct Fire {
    pub k: u32,
    pub data: ValueId,
    /// `\box255` as the builder packed it, native (spec-final A4): its
    /// line boxes are the pieces' values, never encoded again; `data`
    /// holds `()` in its place.
    pub b255: Option<(phitex_typeset::node::BoxNode, Side)>,
    pub carried: Vec<ValueId>,
    pub rest: Vec<ValueId>,
    /// The best break's index on the page.
    pub best: usize,
}

pub(crate) fn some(x: ValueId) -> Option<ValueId> {
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
            params_pre: None,
            pending: Vec::new(),
            normal: 0,
            dead: 0,
        }
    }

    /// The items of a run folded in: candidate by candidate
    /// ([`PageState::build`]), the rest left pending. `live` answers for
    /// an insertion class no marker of the run gives.
    pub(crate) fn fold_items(
        &mut self,
        v: &Values,
        raw: &[ValueId],
        live: &mut dyn FnMut(u16) -> R<InsertClass>,
    ) -> R<Option<Fire>> {
        let mut buf = std::mem::take(&mut self.pending);
        for (i, &x) in raw.iter().enumerate() {
            match nodes::tag(v, x) {
                nodes::T_BUILD_PAGE => {
                    let k = u32::try_from(int_at(v, x, 1)).unwrap_or(0);
                    self.fold.since_bp = 0;
                    if let Some(mut f) = self.build(v, &mut buf, k, live)? {
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

    /// The page fired at candidate `k` (§1017: what the page could not
    /// take goes first in what is carried, then `more`), the builder
    /// set for the next page.
    pub(crate) fn fired(&mut self, v: &Values, f: &Fired, k: u32, more: Vec<ValueId>) -> Fire {
        let mut carried: Vec<ValueId> = f
            .contributions
            .iter()
            .map(|n| nodes::encode(v, n, &self.side))
            .collect();
        carried.extend(more);
        let best = f.best;
        let data = page_data(v, f, &self.side);
        let b255 = Some((f.box255.node.clone(), self.side.clone()));
        self.pb = PageBuilder::new();
        self.side = Side::default();
        self.j += 1;
        if f.output_penalty > -10001 {
            self.normal += 1;
        }
        self.fold.since_bp = 0;
        Fire {
            k,
            data,
            b255,
            carried,
            rest: Vec::new(),
            best,
        }
    }

    /// One `build_page` call (§994) over the contributions `buf` at
    /// candidate `k`: what the page cannot take stays in `buf`.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn build(
        &mut self,
        v: &Values,
        buf: &mut Vec<ValueId>,
        k: u32,
        live: &mut dyn FnMut(u16) -> R<InsertClass>,
    ) -> R<Option<Fire>> {
        // §1245: `\pagegoal=` and its kin set the page so far at once;
        // §1008: an insertion class's registers as the call reads them;
        // §987: the page parameters as the call reads them (the last
        // marker: the one the run put at this candidate), for every
        // contribution it takes
        let mut classes: Vec<(u16, InsertClass)> = Vec::new();
        let mut unresolved: Vec<u16> = Vec::new();
        let mut page_params = None;
        let so_far = &mut self.pb.so_far;
        let mut dead_set = None;
        buf.retain(|&x| match nodes::tag(v, x) {
            nodes::T_PAGE_PARAMS => {
                // (a `[tag]` marker, the parameters reaching a piece: those
                // in force, the page's own (docs/spec-settle.md §2.1))
                if list_of(v, x).len() > 1 {
                    page_params = Some(x);
                }
                false
            }
            nodes::T_SET_PAGE => {
                if int_at(v, x, 1) == crate::pages::DEAD_SLOT {
                    dead_set = Some(int_at(v, x, 2));
                } else if let Ok(i) = usize::try_from(int_at(v, x, 1))
                    && i < 8
                {
                    so_far[i] = i32::try_from(int_at(v, x, 2)).unwrap_or(0);
                }
                false
            }
            nodes::T_INS_PARAMS => {
                if list_of(v, x).len() == 2 {
                    unresolved.push(u16::try_from(int_at(v, x, 1)).unwrap_or(0));
                } else {
                    classes.push(class_of(v, x));
                }
                false
            }
            _ => true,
        });
        if let Some(d) = dead_set {
            self.dead = d;
        }
        // (a marker no step resolved: the registers as the fold has them)
        for n in unresolved {
            classes.push((n, live(n)?));
        }
        if page_params.is_some() {
            if crate::exec::obs_str!("PHITEX_PAGE_DUMP").is_some_and(|d| d == self.j.to_string()) {
                eprintln!(
                    "dump fin page {}: params {} at build {k}",
                    self.j,
                    page_params.map_or_else(String::new, |m| v.render(m))
                );
            }
            self.params = page_params;
            self.params_pre = None;
        }
        let params = self.params_pre.unwrap_or_else(|| {
            self.params.map_or(
                PageParams {
                    vsize: 0,
                    max_depth: 0,
                    top_skip: Glue::ZERO,
                    holding_inserts: 0,
                    saving_vdiscards: false,
                },
                |m| params_of(v, m),
            )
        });
        // (the contributions as a stack, the next on top: a `Vec`)
        let mut queue: Vec<ValueId> = std::mem::take(buf);
        queue.reverse();
        let mut end_seen = None;
        while let Some(x) = queue.pop() {
            match nodes::tag(v, x) {
                // §1054: `\end` ends the job once the page and the
                // contributions are empty and no dead cycle pends; else
                // it fills the page out and comes round again (the
                // marker carried past the finisher)
                nodes::T_END => {
                    end_seen = Some(x);
                    if self.pb.page.is_empty() && queue.is_empty() && self.dead == 0 {
                        continue;
                    }
                    let xs = list_of(v, x);
                    for &y in xs.iter().skip(1).rev() {
                        queue.push(y);
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
                    .last()
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
                if crate::exec::obs_str!("PHITEX_PAGE_DUMP").is_some_and(|d| d == self.j.to_string()) {
                    eprintln!("dump fin page {}: {}", self.j, node_short(&p));
                }
                self.pb.contribute(p, next.as_ref(), &params, &mut class)
            };
            if let Some(h) = halt {
                return Err(h);
            }
            match out {
                Outcome::Done => {}
                // the kern waits for its successor, with everything after
                Outcome::Wait(_) => {
                    queue.push(x);
                    break;
                }
                Outcome::Fire(f) => {
                    if crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
                        eprintln!(
                            "page-trace: page {} fires at candidate {k}: vsize {} maxdepth {} (marker {})",
                            self.j,
                            params.vsize,
                            params.max_depth,
                            self.params
                                .map_or_else(|| "none".to_owned(), |m| v.render(m))
                        );
                    }
                    let mut more: Vec<ValueId> = queue.drain(..).rev().collect();
                    more.extend(end_seen);
                    buf.clear();
                    return Ok(Some(self.fired(v, &f, k, more)));
                }
            }
        }
        queue.reverse();
        *buf = queue;
        Ok(None)
    }
}

/// `p` with the page parameters `ch` names (`(index in PAGE_PARAMS,
/// value)`, [`Run::text_params_since`]) set.
pub(crate) fn with_text(v: &Values, mut p: PageParams, ch: &[(usize, ValueId)]) -> PageParams {
    for &(i, x) in ch {
        match i {
            0 => p.vsize = crate::val::i32_of(v, Some(x)),
            1 => p.max_depth = crate::val::i32_of(v, Some(x)),
            2 => p.top_skip = glue_of(v, Some(x)),
            _ => p.holding_inserts = crate::val::i32_of(v, Some(x)),
        }
    }
    p
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

/// A fired page as a finisher gets it: `[\box255, inserts, marks, held,
/// \outputpenalty]` (§1012–§1022).
pub(crate) fn page_data(v: &Values, f: &Fired, side: &Side) -> ValueId {
    // (`\box255` stays native, [`Fire::b255`])
    let _ = side;
    let b255 = ValueId::UNIT;
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

// ---------------------------------------- reads where a piece is

impl Run<'_, '_> {
    /// Insertion class `n`'s registers (§1008) as they reach the node
    /// `at` ([`phitex_defuse::Ctx::use_field_at`]), what the page
    /// builder read where the break was found; as they reach this run
    /// where `at` is gone or is this run's own node.
    pub(crate) fn insert_class_at(&mut self, n: u16, at: phitex_defuse::NodeId) -> R<InsertClass> {
        if at == self.cx.node() {
            return self.insert_class(n);
        }
        let v = self.v;
        let mut xs = Vec::with_capacity(4);
        let bf = self.box_field(i64::from(n));
        let mut fields = vec![bf];
        for r in 0..3 {
            let l = self.reg_loc(r, i32::from(n));
            fields.push(self.loc_field(l));
        }
        for f in fields {
            let r = self.field_at(f, at, false);
            if r.is_err() && crate::exec::prof_flag!("PHITEX_PAGES_STOPS") {
                eprintln!(
                    "pages-stop: {} after piece {}",
                    crate::val::field_name(v, f),
                    at.raw()
                );
            }
            match r? {
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

    /// The page parameters (§980) reaching the position just after node
    /// `at`, as a `T_PAGE_PARAMS` marker: what a piece's `[tag]` marker
    /// means, read where the piece is, each its own item).
    pub(crate) fn page_params_at(&mut self, at: i64) -> R<ValueId> {
        let v = self.v;
        let Some(xs) = self.page_cells_at(at)? else {
            return self.page_params_marker();
        };
        let int = |x: Option<ValueId>| v.int(i64::from(crate::val::i32_of(v, x)));
        Ok(v.list(vec![
            v.int(nodes::T_PAGE_PARAMS),
            int(xs[0]),
            int(xs[1]),
            glue_value(v, &glue_of(v, xs[2])),
            int(xs[3]),
        ]))
    }

    /// [`Run::page_params_at`]'s parameters, not made a marker.
    pub(crate) fn page_params_struct_at(&mut self, at: i64) -> R<PageParams> {
        let v = self.v;
        let Some(xs) = self.page_cells_at(at)? else {
            let m = self.page_params_marker()?;
            return Ok(params_of(v, m));
        };
        let int = |x: Option<ValueId>| crate::val::i32_of(v, x);
        Ok(PageParams {
            vsize: int(xs[0]),
            max_depth: int(xs[1]),
            top_skip: glue_of(v, xs[2]),
            holding_inserts: int(xs[3]),
            saving_vdiscards: false,
        })
    }

    /// The page parameters' cells after piece `at` (None: the piece is
    /// gone, the marker where the run is stands for them).
    pub(crate) fn page_cells_at(&mut self, at: i64) -> R<Option<Vec<Option<ValueId>>>> {
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(at.to_ne_bytes()));
        let mut xs = Vec::with_capacity(4);
        for p in crate::pages::PAGE_PARAMS {
            let f = self.loc_field(p as crate::state::Loc);
            let r = self.cx.read_at(f, phitex_defuse::At::AfterOwn(at));
            if r.is_err() && crate::exec::prof_flag!("PHITEX_PAGES_STOPS") {
                eprintln!("pages-stop: the page parameters after piece {}", at.raw());
            }
            match r? {
                phitex_defuse::FieldAt::Def(x, _) => xs.push(Some(x)),
                phitex_defuse::FieldAt::Gone => return Ok(None),
                phitex_defuse::FieldAt::Unset => xs.push(None),
            }
        }
        Ok(Some(xs))
    }

    /// The page parameters the text set (`tpp:`, [`Run::set_loc`]) that
    /// reach piece `pid` from a definition after piece `since` (its raw
    /// id; -1: any): each `(index in PAGE_PARAMS, value)`. A routine's
    /// setting is not among them: it reaches the builder through its
    /// give-back (docs/spec-settle.md §2.1).
    pub(crate) fn text_params_since(&mut self, pid: i64, since: i64) -> R<Vec<(usize, ValueId)>> {
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
        let after = (since >= 0).then(|| phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(since.to_ne_bytes())));
        let mut out = Vec::new();
        for (i, p) in crate::pages::PAGE_PARAMS.into_iter().enumerate() {
            let t = self.tpp_loc(p as crate::state::Loc);
            let f = self.loc_field(t);
            if let phitex_defuse::FieldAt::Def(x, Some(d)) = self.cx.read_at(f, phitex_defuse::At::AfterOwn(at))?
                && after.is_none_or(|s| s != d && self.cx.alive(s) && self.cx.alive(d) && self.cx.before(s, d))
            {
                out.push((i, x));
            }
        }
        Ok(out)
    }

    /// `raw`'s `[tag]` page parameter markers, of piece `pid`, given the
    /// parameters reaching it ([`Run::page_params_at`]), and its `[tag,
    /// n]` insertion class markers the class's registers reaching it
    /// ([`Run::insert_class_at`]).
    /// `text`: on a page after the first, the piece the page before fired
    /// in and the parameters in force so far on this page: a `[tag]`
    /// marker is then those, with what the text set since that fire
    /// ([`Run::text_params_since`]), or left `[tag]` (no change) where
    /// it set nothing; none: read where the piece is (the first page).
    pub(crate) fn resolve_params_of(&mut self, pid: i64, raw: &mut [ValueId], text: Option<(i64, &mut Option<PageParams>)>) -> R<()> {
        let reaching = text.is_none();
        let mut text = text;
        let v = self.v;
        let mut m = None;
        let mut classes: Vec<(u16, ValueId)> = Vec::new();
        for x in raw.iter_mut() {
            if nodes::tag(v, *x) == nodes::T_INS_PARAMS && list_of(v, *x).len() == 2 {
                // (§1008: the class's registers as they reach the piece)
                let n = u16::try_from(int_at(v, *x, 1)).unwrap_or(0);
                *x = if let Some(&(_, c)) = classes.iter().find(|c| c.0 == n) {
                    c
                } else {
                    let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
                    let c = self.insert_class_at(n, at)?;
                    let c = self.ins_marker(n, &c);
                    classes.push((n, c));
                    c
                };
                continue;
            }
            if let Some((since, cur)) = text.as_mut()
                && nodes::tag(v, *x) == nodes::T_PAGE_PARAMS
            {
                if list_of(v, *x).len() > 1 {
                    **cur = Some(params_of(v, *x));
                } else if let Some(c) = **cur {
                    let ch = self.text_params_since(pid, *since)?;
                    if !ch.is_empty() {
                        let q = with_text(v, c, &ch);
                        **cur = Some(q);
                        *x = params_marker(v, &q);
                    }
                }
                continue;
            }
            if reaching && nodes::tag(v, *x) == nodes::T_PAGE_PARAMS && list_of(v, *x).len() == 1 {
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
    /// parameters reaching it, and its `[tag, n]` insertion class items
    /// the class's registers reaching it: what [`Run::resolve_params_of`]
    /// gives the piece's items (a marker resolved where the build reads
    /// it would read the registers at a later piece, past a finisher
    /// that changed them, and the scan's decision would part from the
    /// items')
    pub(crate) fn resolve_prof_of(&mut self, pid: i64, items: &mut [crate::vprof::PItem]) -> R<()> {
        self.resolve_prof_of_with(pid, items, &mut OwnerReads::default())
    }

    /// [`Run::resolve_prof_of`] with what was read at `pid` already
    /// (`own`, which gains what this reads).
    pub(crate) fn resolve_prof_of_with(
        &mut self,
        pid: i64,
        items: &mut [crate::vprof::PItem],
        own: &mut OwnerReads,
    ) -> R<()> {
        // (a `[tag]` page parameter marker stays one: the fold reads it
        // as the parameters it has in force, docs/spec-settle.md §2.1)
        for x in items.iter_mut() {
            if let crate::vprof::PItem::InsParams(n, c @ None) = x {
                let n = *n;
                *c = Some(if let Some(&(_, k)) = own.classes.iter().find(|k| k.0 == n) {
                    k
                } else {
                    let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
                    let k = self.insert_class_at(n, at)?;
                    own.classes.push((n, k));
                    k
                });
            }
        }
        Ok(())
    }
}

/// What the builder over profiles reads where a fold runs: the fold's
/// run, and the piece whose end the registers are read at (none: where
/// the run is).
pub(crate) struct RunReaching<'r, 'c, 'a>(pub &'r mut Run<'c, 'a>, pub Option<i64>);

impl crate::vprof::Reaching for RunReaching<'_, '_, '_> {
    fn params(&mut self) -> PageParams {
        self.0.reach_reads += 1;
        let v = self.0.v;
        let m = match self.1 {
            Some(at) => self.0.page_params_at(at),
            None => self.0.page_params_marker(),
        };
        m.map_or(
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
        self.0.reach_reads += 1;
        let c = match self.1 {
            Some(at) => self.0.insert_class_at(
                n,
                phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(at.to_ne_bytes())),
            ),
            None => self.0.insert_class(n),
        };
        c.unwrap_or(InsertClass {
            box_height_plus_depth: None,
            box_is_hbox: false,
            count: 0,
            dimen: 0,
            skip: Glue::ZERO,
        })
    }
}

/// The version of each piece `pieces` names as the loop folded it (its
/// [`crate::vprof::VPROF`] unit, the latest the loop holds), sorted by
/// piece ([`Page::vers`]).
pub(crate) fn page_vers(lc: &LoopCarry, pieces: &[i64]) -> Vec<(i64, ValueId)> {
    let mut out: Vec<(i64, ValueId)> = pieces
        .iter()
        .filter_map(|&p| lc.units.iter().rev().find(|u| u.pid == p).map(|u| (p, u.src)))
        .collect();
    out.sort_unstable_by_key(|e| e.0);
    out
}