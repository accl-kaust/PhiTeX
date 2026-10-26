//! Layer 4: the page builder (§980–§1010) as a chain of page steps,
//! [`Pages`] nodes.
//!
//! The first step is pushed where the job's text ends. A step reads the
//! profiles of the pieces and of the paragraphs' lines in text order, one
//! contributor at a time ([`phitex_defuse::Ctx::next_contribution`] on
//! [`crate::vprof::VPROF`]), from where the page before fired, folding
//! each with TeX's page builder over profiles
//! ([`crate::vprof::ScanState`]), to the next page's fire. A contributor
//! still in work ends the read there; the step runs again when it comes
//! in.
//!
//! Where a page fires the step notes it, with everything its finisher
//! needs to assemble it alone ([`FinishIn`]): where each item on it came
//! from ([`Src`], [`Span`]: a piece's items, a routine's material, a
//! held-over insertion), the builder's state where the page began and its
//! decisions (the best break, `\outputpenalty`, the insertions held
//! over), the marks in force where it began. It places the finisher as
//! its child and the next step after it, with the loop as it stands
//! ([`PageLoop`], [`Pending`]) as the next step's arguments: a step whose
//! loop comes out as it was is the same node and does not run again.
//! The step after the last page defines [`PAGES_END`].
//!
//! The next page begins with what the routine gives back (§1026): the
//! finisher's output ([`PAGE_OUT`]), read by the step after it,
//! and while the finisher has none, a guess ([`Run::rtn_guess`],
//! [`Feedback::RoutineGivesBack`]): the routine's last value on another
//! page, or nothing. A finisher's output coming in other than the guess
//! runs that step again. The guess is no finisher's argument: another
//! page's give-back, its content, would make a later page's finisher
//! another node whenever that page changed.
//!
//! What the loop reads where a piece is (the page parameters, an
//! insertion class's registers) it reads at that piece, a read past its
//! own node ([`Run::page_params_at`], [`Run::insert_class_at`],
//! [`Feedback::BuilderRegisters`]). `\deadcycles` (§1054's `\end`, §1012)
//! it has as the routine before left it ([`RTN`]) and as the text set it
//! after, a marker on the main list in order with the pieces' items
//! ([`crate::pages::DEAD_SLOT`]).
//!
//! The finisher's own fold of its page's items is the same builder on
//! real nodes ([`PageState`]), at the decisions the loop made.

use super::*;

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

/// The page builder's loop as a step has it.
#[derive(Clone)]
pub(crate) struct PageLoop {
    /// The builder over profiles (its own pending list unused: `pend`).
    pub st: crate::vprof::ScanState,
    /// The last contributor folded (the cursor of `next_contribution`).
    pub cur: Option<phitex_defuse::NodeId>,
    /// Where each item the current page's input holds came from, in the
    /// order its finisher feeds them ([`FinishIn::spans`]): the routine's
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
    /// fold and the page parameters in force ([`FinishIn::fold`]).
    pub began: (FoldState, Option<ValueId>),
    /// The marks in force where the current page began (§1012: the
    /// `\botmark` of every class before it), `[[class, mark]…]`
    /// ([`FinishIn::top_marks`]); unit for none.
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
    /// the guess of a later page fired there ([`Run::rtn_guess`]).
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
    /// ([`FinishIn::prev_fire`]).
    pub prev_fire: (i64, i64),
    /// The marks the profiles' mark items name (a unit's marks given ids
    /// here as it is decoded, [`remap_marks`]): the step's own.
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
    /// The last contributor folded that gave units ([`PageLoop::prev`]).
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
    /// The next piece's place in [`PageLoop::order`].
    pub seq: u64,
    /// What the loop read where the owner of the units it folds last is
    /// ([`OwnerReads`]): a piece's unit and its paragraphs' lines' units
    /// share it.
    pub own: OwnerReads,
    /// The contributor the last page fired in and the pages fired in it
    /// ([`FinishIn::nth`]).
    pub nth: (Option<phitex_defuse::NodeId>, u32),
    /// Whether `\end` has ended the job (§1054: its page empty, nothing
    /// left, no dead cycle), or the job was aborted: nothing after it is
    /// read.
    pub ended: bool,
}

/// A page fired at the end of a step: what the step after it needs to
/// fold on from there (§1017–§1026), with the loop as it stood.
#[derive(Clone, Debug)]
pub(crate) struct Pending {
    pub j: i64,
    /// The contributor the page fired in, the unit of its contribution
    /// the fire was in, and that unit's piece.
    pub node: phitex_defuse::NodeId,
    pub xs_i: usize,
    pub pid: i64,
    pub in_material: bool,
    pub hint: ValueId,
    pub key: RtnKey,
    /// What the routine is guessed to give back ([`Run::rtn_guess`]).
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

/// The reads at one owner piece ([`crate::vprof::unit_owner`]): its page
/// parameters and its insertion classes' registers, read once for all
/// the units it owns ([`Run::resolve_prof_of`], [`Run::insert_class_at`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct OwnerReads {
    pub owner: Option<i64>,
    pub params: Option<PageParams>,
    pub classes: Vec<(u16, InsertClass)>,
}

impl PageLoop {
    /// The loop at the job's start: no page fired, the main list empty.
    pub(crate) fn new() -> PageLoop {
        PageLoop {
            st: crate::vprof::ScanState::fresh(0, 0, crate::build::IGNORE_DEPTH_STATE, None),
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

/// A node's raw id as an integer value (-1 for none).
fn raw_value(v: &Values, n: Option<phitex_defuse::NodeId>) -> ValueId {
    v.int(n.map_or(-1, |n| i64::from_ne_bytes(n.raw().to_ne_bytes())))
}

/// [`raw_value`]'s node.
fn raw_node(v: &Values, x: ValueId) -> Option<phitex_defuse::NodeId> {
    let i = int_of(v, x);
    (i != -1).then(|| node_of(i))
}

/// The loop at a page's fire and the fire, as the next step's state: what
/// the fold on from there reads, nothing else (the units no source after
/// the fire names, the routines' material none names, the places of the
/// pieces neither names, left out), so a step whose page comes out as it
/// was gives the step after it the same arguments.
fn encode_state(v: &Values, lp: &PageLoop, p: &Pending) -> ValueId {
    let piece = |s: &Src| match *s {
        Src::Piece { piece, .. } | Src::Held { piece, .. } => Some(piece),
        Src::Rtn { .. } => None,
    };
    let named: Vec<i64> = p
        .carried
        .iter()
        .filter_map(|c| c.src.as_ref().and_then(piece))
        .chain(p.held.iter().filter_map(|h| piece(&h.0)))
        .collect();
    let keep = lp
        .units
        .iter()
        .position(|u| named.contains(&u.pid))
        .map_or(lp.frontier.0, |i| i.min(lp.frontier.0));
    let units = &lp.units[keep.min(lp.units.len())..];
    let mut pieces: Vec<i64> = units.iter().map(|u| u.pid).chain(named.iter().copied()).collect();
    pieces.sort_unstable();
    pieces.dedup();
    let mut pages: Vec<i64> = p
        .carried
        .iter()
        .filter_map(|c| match c.src {
            Some(Src::Rtn { page, .. }) => Some(page),
            _ => None,
        })
        .chain(lp.inf.iter().filter_map(|s| match *s {
            Src::Rtn { page, .. } => Some(page),
            _ => None,
        }))
        .collect();
    pages.sort_unstable();
    pages.dedup();
    let mut e = Enc { v, xs: Vec::with_capacity(64 + 4 * lp.inf.len() + 2 * units.len()) };
    let st = &lp.st;
    e.i(st.j);
    e.i(st.normal);
    e.fold(st.fold);
    match st.params {
        Some(q) => {
            e.i(1);
            e.i(i64::from(q.vsize));
            e.i(i64::from(q.max_depth));
            e.glue(&q.top_skip);
            e.i(i64::from(q.holding_inserts));
        }
        None => e.i(0),
    }
    e.node(lp.cur);
    e.node(lp.cur_units);
    e.x(lp.top_marks);
    let inf: Vec<Src> = lp
        .inf
        .iter()
        .copied()
        .filter(|s| match *s {
            Src::Rtn { page, .. } => pages.binary_search(&page).is_ok(),
            Src::Piece { piece, .. } | Src::Held { piece, .. } => pieces.binary_search(&piece).is_ok(),
        })
        .collect();
    e.len(inf.len());
    for s in inf {
        e.src(s);
    }
    e.len(lp.last_rtn.len());
    for (k, x) in &lp.last_rtn {
        e.key(*k);
        e.x(*x);
    }
    e.len(lp.last_markers.len());
    for &(h, m) in &lp.last_markers {
        e.x(h);
        e.x(m);
    }
    e.len(units.len());
    for u in units {
        e.x(u.src);
    }
    e.len(lp.frontier.0.saturating_sub(keep));
    e.i(i64::from(lp.frontier.1));
    e.len(pieces.len());
    for q in &pieces {
        e.i(*q);
        e.node(lp.prev.get(q).copied().flatten());
        e.i(lp.order.get(q).map_or(-1, |&o| i64::try_from(o).unwrap_or(0)));
    }
    e.i(i64::try_from(lp.seq).unwrap_or(0));
    let rtns: Vec<&(i64, ValueId, std::sync::Arc<Vec<crate::vprof::PItem>>)> =
        lp.rtns.iter().filter(|r| pages.binary_search(&r.0).is_ok()).collect();
    e.len(rtns.len());
    for r in rtns {
        e.i(r.0);
        e.x(r.1);
    }
    e.node(lp.nth.0);
    e.i(i64::from(lp.nth.1));
    // the fire
    e.i(p.j);
    e.node(Some(p.node));
    e.len(p.xs_i);
    e.i(p.pid);
    e.i(i64::from(p.in_material));
    e.x(p.hint);
    e.key(p.key);
    e.x(p.rtn_guess);
    e.i(i64::from(p.k));
    e.i(i64::from(p.output_penalty));
    e.len(p.held.len());
    for (s, h) in &p.held {
        e.src(*s);
        let mut out = Vec::new();
        crate::vprof::push_node(h, &mut out);
        e.len(out.len());
        for x in out {
            e.i(x);
        }
    }
    e.len(p.carried.len());
    for c in &p.carried {
        match c.src {
            Some(s) => e.src(s),
            None => e.i(-1),
        }
        e.fold(c.fold);
    }
    e.i(i64::from(p.nth));
    e.len(p.classes.len());
    for (k, c) in &p.classes {
        e.i(i64::from(*k));
        e.i(c.box_height_plus_depth.map_or(-1, i64::from));
        e.i(i64::from(c.box_is_hbox));
        e.i(i64::from(c.count));
        e.i(i64::from(c.dimen));
        e.glue(&c.skip);
    }
    v.list(e.xs)
}

/// The step's state as one flat list ([`encode_state`]): integers inline,
/// values by their ids, a list as its length then its elements.
struct Enc<'v> {
    v: &'v Values,
    xs: Vec<ValueId>,
}

impl Enc<'_> {
    fn i(&mut self, x: i64) {
        self.xs.push(self.v.int(x));
    }
    fn len(&mut self, n: usize) {
        self.i(i64::try_from(n).unwrap_or(0));
    }
    fn x(&mut self, x: ValueId) {
        self.xs.push(x);
    }
    fn node(&mut self, n: Option<phitex_defuse::NodeId>) {
        self.i(n.map_or(-1, |n| i64::from_ne_bytes(n.raw().to_ne_bytes())));
    }
    fn src(&mut self, s: Src) {
        let (t, a, b, c) = match s {
            Src::Piece { piece, item } => (0, piece, i64::from(item), 0),
            Src::Rtn { page, item } => (1, page, i64::from(item), 0),
            Src::Held { piece, item, from } => (2, piece, i64::from(item), i64::from(from)),
        };
        self.i(t);
        self.i(a);
        self.i(b);
        self.i(c);
    }
    fn fold(&mut self, f: FoldState) {
        self.i(i64::from(f.sf));
        self.i(i64::from(f.pd));
        self.len(f.since_bp);
    }
    fn key(&mut self, k: RtnKey) {
        self.x(k.0);
        self.i(i64::from(k.1));
        self.i(i64::from(k.2));
    }
    fn glue(&mut self, g: &Glue) {
        self.i(i64::from(g.width));
        self.i(i64::from(g.stretch));
        self.i(i64::from(g.shrink));
        self.i((g.stretch_order as i64) * 4 + g.shrink_order as i64);
    }
}

/// [`Enc`]'s reader.
struct Dec<'v> {
    v: &'v Values,
    xs: Vec<ValueId>,
    at: usize,
}

impl Dec<'_> {
    fn x(&mut self) -> ValueId {
        let x = self.xs.get(self.at).copied().unwrap_or(ValueId::UNIT);
        self.at += 1;
        x
    }
    fn i(&mut self) -> i64 {
        let x = self.x();
        int_of(self.v, x)
    }
    fn i32(&mut self) -> i32 {
        i32::try_from(self.i()).unwrap_or(0)
    }
    fn u32(&mut self) -> u32 {
        u32::try_from(self.i()).unwrap_or(0)
    }
    fn len(&mut self) -> usize {
        usize::try_from(self.i()).unwrap_or(0)
    }
    fn node(&mut self) -> Option<phitex_defuse::NodeId> {
        let i = self.i();
        (i != -1).then(|| node_of(i))
    }
    fn src_tagged(&mut self, t: i64) -> Option<Src> {
        let (a, b, c) = (self.i(), self.u32(), self.u32());
        match t {
            0 => Some(Src::Piece { piece: a, item: b }),
            1 => Some(Src::Rtn { page: a, item: b }),
            2 => Some(Src::Held { piece: a, item: b, from: c }),
            _ => None,
        }
    }
    fn src(&mut self) -> Option<Src> {
        let t = self.i();
        self.src_tagged(t)
    }
    fn fold(&mut self) -> FoldState {
        FoldState {
            sf: self.i32(),
            pd: self.i32(),
            since_bp: self.len(),
        }
    }
    fn key(&mut self) -> RtnKey {
        (self.x(), self.i32(), self.i() != 0)
    }
    fn glue(&mut self) -> Glue {
        let (w, st, sh, o) = (self.i32(), self.i32(), self.i32(), self.i());
        Glue {
            width: w,
            stretch: st,
            stretch_order: crate::val::order(o / 4),
            shrink: sh,
            shrink_order: crate::val::order(o % 4),
        }
    }
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
    /// The marks on the page, in order (ids in [`PageLoop::marks`]).
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
fn loop_item(lp: &PageLoop, src: Src) -> Option<crate::vprof::PItem> {
    use crate::vprof::PItem;
    // (§1013: a page's best break, a penalty, set to 10000 where the page
    // fired: it goes back to the contributions so)
    if lp.inf.contains(&src) {
        return match loop_item_raw(lp, src)? {
            PItem::Node(Node::Penalty(_)) => {
                Some(PItem::Node(Node::Penalty(phitex_typeset::linebreak::INF_PENALTY)))
            }
            x => Some(x),
        };
    }
    loop_item_raw(lp, src)
}

/// [`loop_item`] as the source holds it.
fn loop_item_raw(lp: &PageLoop, src: Src) -> Option<crate::vprof::PItem> {
    use crate::vprof::PItem;
    match src {
        Src::Piece { piece, item } => {
            let u = lp.units.iter().rev().find(|u| u.pid == piece)?;
            match end_sub_of(item) {
                None => u.items.get(item as usize).map(|x| u.res.of(x).unwrap_or_else(|| x.clone())),
                Some((e, s)) => match u.items.get(e as usize)? {
                    PItem::End(ys) => ys.get(s as usize).cloned(),
                    _ => None,
                },
            }
        }
        Src::Rtn { page, item } => {
            let (_, _, items) = lp.rtns.iter().rev().find(|r| r.0 == page)?;
            items.get(item as usize).cloned()
        }
        Src::Held { .. } => None,
    }
}

/// [`crate::vprof::ScanState`]'s fold of one item with its source
/// (§679's interline glue, deletions, markers), into `lp.pend`.
fn fold_item_t(lp: &mut PageLoop, x: &crate::vprof::PItem, s: SrcRef) {
    use crate::vprof::PItem;
    let st = &mut lp.st.fold;
    let out = &mut lp.pend;
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
fn is_marker_p(x: &crate::vprof::PItem) -> bool {
    use crate::vprof::PItem;
    match x {
        PItem::Params(_) | PItem::SetPage(..) | PItem::InsParams(..) => true,
        PItem::Opaque(t) => matches!(*t, nodes::T_WRITE_AT | nodes::T_FONT_AT | nodes::T_PDFW_AT),
        _ => false,
    }
}

/// One `build_page` call (§994) over `lp.pend` at candidate `k`, with the
/// sources of what the page takes ([`crate::vprof::ScanState`]'s build).
/// `class` answers for an insertion class no marker names.
#[allow(clippy::too_many_lines)]
fn build_t(
    lp: &mut PageLoop,
    k: u32,
    class_of_live: &mut dyn FnMut(u16) -> InsertClass,
) -> Option<LoopFire> {
    use crate::vprof::PItem;
    let mut classes: Vec<(u16, InsertClass)> = Vec::new();
    let mut page_params = None;
    let mut buf = std::mem::take(&mut lp.pend);
    let mut dead_set = None;
    {
        let so_far = &mut lp.st.pb.so_far;
        buf.retain(|(x, _)| match x {
            PItem::Params(p) => {
                page_params = Some(p.unwrap_or(PageParams {
                    vsize: 0,
                    max_depth: 0,
                    top_skip: Glue::ZERO,
                    holding_inserts: 0,
                    saving_vdiscards: false,
                }));
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
        lp.dead = d;
    }
    let dead = lp.dead;
    if page_params.is_some() {
        lp.st.params = page_params;
    }
    if crate::exec::obs_str!("PHITEX_PAGE_DUMP").is_some_and(|d| d == lp.st.j.to_string()) {
        eprintln!(
            "dump loop page {}: build {k} topskip {:?}",
            lp.st.j,
            lp.st.params.map(|p| p.top_skip.width)
        );
    }
    let params = lp.st.params.unwrap_or(PageParams {
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
                if lp.st.pb.page.is_empty() && queue.is_empty() && dead == 0 {
                    lp.ended = true;
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
                lp.ended = true;
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
        let before = lp.st.pb.page.len();
        if crate::exec::obs_str!("PHITEX_PAGE_DUMP").is_some_and(|d| d == lp.st.j.to_string()) {
            eprintln!("dump loop page {}: {} from {:?}", lp.st.j, node_short(&p), s.src);
        }
        let ins_n = if let Node::Ins(i) = &p { Some(i.number) } else { None };
        let out = {
            let mut class = |n: u16| {
                classes
                    .iter()
                    .find(|c| c.0 == n)
                    .map_or_else(|| class_of_live(n), |c| c.1)
            };
            lp.st.pb.contribute(p, next.as_ref(), &params, &mut class)
        };
        match out {
            Outcome::Done => {
                let after = lp.st.pb.page.len();
                if after == before + 2 {
                    // (`\topskip`'s glue before the first box, §1001)
                    lp.page_src.push(SrcRef { src: None, fold: s.fold });
                    lp.page_ins.push(None);
                }
                if after > before {
                    lp.page_src.push(s);
                    lp.page_ins.push(ins_n);
                }
            }
            Outcome::Wait(n) => {
                queue.push_front((PItem::Node(n), s));
                break;
            }
            Outcome::Fire(f) => {
                let srcs = std::mem::take(&mut lp.page_src);
                let ins = std::mem::take(&mut lp.page_ins);
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
                        && !lp.inf.contains(&b)
                    {
                        lp.inf.push(b);
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
                lp.st.pb = PageBuilder::new();
                lp.st.pb.measure_only = true;
                lp.st.j += 1;
                if f.output_penalty > -10001 {
                    lp.st.normal += 1;
                }
                lp.st.fold.since_bp = 0;
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
    lp.pend = queue.into();
    None
}


/// The sources of the insertions a fire holds over (§1020–§1022), in
/// order: each insertion before the best break whose class has no best
/// insertion or comes after it, all of them where `\holdinginserts` is
/// positive, and the rest of the one split where a rest is left. `None`
/// where one cannot be named by a piece's item ([`Src::Held`]): a defect,
/// said by the caller.
fn held_srcs(f: &Fired, page_src: &[SrcRef], page_ins: &[Option<u16>], holding: bool) -> Option<Vec<Src>> {
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
fn spans_of(srcs: &[Src]) -> Vec<Span> {
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

/// The tag of a kick's arguments ([`kick_args`]).
const KICK: &str = "\u{1}kick";

/// The arguments of the node that places the first page step after
/// `mark` (where the text ends): `[tag, mark]`. Pushed where the text
/// begins, it runs with the first block, so the steps start beside the
/// text instead of after it.
pub(crate) fn kick_args(v: &Values, mark: phitex_defuse::NodeId) -> Vec<ValueId> {
    vec![v.str(KICK), raw_value(v, Some(mark))]
}

/// The mark a kick's arguments name.
fn kick_of(v: &Values, args: &[ValueId]) -> Option<phitex_defuse::NodeId> {
    match args {
        [t, m] if v.with_str(*t, |s| s == KICK).unwrap_or(false) => raw_node(v, *m),
        _ => None,
    }
}

/// A page step ([`PageLoop`]; the module's doc): `[]` for the first,
/// else `[node, state]`, the contributor the page before fired in and
/// the loop there ([`encode_state`]).
pub(crate) struct Pages(pub Shared);

impl Operation for Pages {
    fn name(&self) -> &'static str {
        "pages"
    }
    fn may_define(&self, _: &[ValueId], v: &Values) -> MayDefine {
        MayDefine::Names(vec![v.field(PAGES_END)])
    }
    /// (set right after the text's first piece where there are threads
    /// to run it beside the text: no piece's claim takes it)
    fn sibling(&self) -> bool {
        false
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn contributes(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        !crate::pages::from_pieces(v, f)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let o = ops(&self.0);
        let args = cx.args().to_vec();
        // (the kick: the first step placed after the mark where the text
        // ends, [`kick_args`])
        if let Some(at) = kick_of(cx.values(), &args) {
            cx.follow_after(at, o.pages, Vec::new());
            return Ok(ValueId::UNIT);
        }
        // (a run, for its reads where a piece is; no text runs in it)
        let mut r = Run::new(cx, o);
        crate::prof::pages::attempt();
        let t = crate::prof::pages::start();
        let res = match args.get(1) {
            Some(&x) => r.decode_state(x).and_then(|(lp, p)| r.page_step(lp, Some(p))),
            None => r.page_step(PageLoop::new(), None),
        };
        crate::prof::pages::add(crate::prof::pages::ATTEMPT, t);
        match res {
            Ok(()) => Ok(ValueId::UNIT),
            Err(Halt::Stop(s)) => Err(s),
            Err(_) => unreachable!("a page step runs no command"),
        }
    }
}

impl Run<'_, '_> {
    /// A routine's give-back as the evaluation began (§1026): its last
    /// value on another page of the class `pen`
    /// ([`crate::pages::finish::pen_class`]; `crate::run::Ops::fin_defs` then, so every
    /// attempt of the loop guesses the same), `\deadcycles` left out,
    /// else nothing given back. The guess of page `j`
    /// ([`Run::rtn_guess`], made in [`Run::page_loop`]) is what the
    /// same routine gave back on the last page the loop fired on the text
    /// before `j` at the same `\outputpenalty`, every one above -10001
    /// alike (the output it read, or that page's own guess where it read
    /// none); else for an ordinary page the markers the routine gave back
    /// last and no list (it ships), this for a special one;
    /// nothing for a page fired in a routine's material (no chain of
    /// pages grows on guesses). It depends on the pages before `j` alone:
    /// a page's own output never changes it.
    pub(crate) fn rtn_guess(&self, hint: ValueId, pen: i32) -> ValueId {
        let v = self.v;
        let empty = || v.list(vec![v.list(Vec::new()), v.list(Vec::new())]);
        if hint == ValueId::UNIT {
            return empty();
        }
        let rtn = v.field(RTN);
        self.ops
            .fin_defs_start
            .locked()
            .get(&crate::pages::finish::fin_key_class(hint, pen))
            .and_then(|d| d.last.iter().find(|e| e.0 == rtn).map(|e| e.1))
            // (`\deadcycles` guessed 0, its register maps kept)
            .map_or_else(empty, |x| crate::pages::finish::rtn_as_guess(v, x))
    }
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
/// ([`Run::rtn_guess`]): the same give-backs of the pages before `j`
/// (what the loop took for them, outputs read or guesses), the same
/// routine and `\outputpenalty`, give the same guess in every attempt,
/// whatever page `j`'s own output; one changed by it is a defect (its
/// finisher placed anew on its own output).
fn guess_check(node: u64, j: i64, how: impl std::hash::Hash, guess: ValueId) {
    use std::hash::Hasher as _;
    type Seen = Option<phitex_defuse::FxHashMap<(u64, i64, u64), ValueId>>;
    static SEEN: std::sync::Mutex<Seen> = std::sync::Mutex::new(None);
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
fn node_short(n: &Node) -> String {
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
/// ([`PageLoop::last_rtn`]): the routine's hint, the page's class
/// ([`crate::pages::finish::pen_class`]) and whether the page before
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
    Some(crate::pages::finish::rtn_value(
        v,
        xs[0],
        v.list(list),
        dead + 1,
    ))
}

impl Run<'_, '_> {
    /// A step's state ([`encode_state`]) as the loop and the fire it
    /// folds on from; its units decoded again, their markers resolved
    /// where their pieces are.
    pub(crate) fn decode_state(&mut self, x: ValueId) -> R<(PageLoop, Pending)> {
        let v = self.v;
        let mut d = Dec { v, xs: list_of(v, x), at: 0 };
        let mut lp = PageLoop::new();
        let (j, normal, fold) = (d.i(), d.i(), d.fold());
        let params = (d.i() != 0).then(|| PageParams {
            vsize: d.i32(),
            max_depth: d.i32(),
            top_skip: d.glue(),
            holding_inserts: d.i32(),
            saving_vdiscards: false,
        });
        lp.st = crate::vprof::ScanState::fresh(j, normal, fold, params);
        lp.cur = d.node();
        lp.cur_units = d.node();
        lp.top_marks = d.x();
        let n = d.len();
        lp.inf = (0..n).filter_map(|_| d.src()).collect();
        let n = d.len();
        lp.last_rtn = (0..n).map(|_| (d.key(), d.x())).collect();
        let n = d.len();
        lp.last_markers = (0..n).map(|_| (d.x(), d.x())).collect();
        let n = d.len();
        for _ in 0..n {
            let src = d.x();
            let (pid, items, res) = self.unit_items(&mut lp, src)?;
            lp.units.push(UnitRec { pid, items, res, src });
        }
        lp.frontier = (d.len(), d.u32());
        let n = d.len();
        for _ in 0..n {
            let p = d.i();
            lp.prev.insert(p, d.node());
            let o = d.i();
            if o >= 0 {
                lp.order.insert(p, u64::try_from(o).unwrap_or(0));
            }
        }
        lp.seq = u64::try_from(d.i()).unwrap_or(0);
        let n = d.len();
        for _ in 0..n {
            let (page, val) = (d.i(), d.x());
            let items = self.rtn_items(&mut lp, val);
            lp.rtns.push((page, val, std::sync::Arc::new(items)));
        }
        lp.nth = (d.node(), d.u32());
        let j = d.i();
        let node = d.node().unwrap_or_else(|| node_of(0));
        let xs_i = d.len();
        let pid = d.i();
        let in_material = d.i() != 0;
        let hint = d.x();
        let key = d.key();
        let rtn_guess = d.x();
        let k = d.u32();
        let output_penalty = d.i32();
        let n = d.len();
        let mut held = Vec::with_capacity(n);
        for _ in 0..n {
            let s = d.src();
            let m = d.len();
            let ints: Vec<i64> = (0..m).map(|_| d.i()).collect();
            if let (Some(s), Some(crate::vprof::PItem::Node(h))) = (s, crate::vprof::decode(&ints).into_iter().next()) {
                held.push((s, h));
            }
        }
        let n = d.len();
        let carried = (0..n)
            .map(|_| {
                let t = d.i();
                let src = if t == -1 { None } else { d.src_tagged(t) };
                SrcRef { src, fold: d.fold() }
            })
            .collect();
        let nth = d.u32();
        let n = d.len();
        let classes = (0..n)
            .map(|_| {
                let k = u16::try_from(d.i()).unwrap_or(0);
                let h = d.i();
                (
                    k,
                    InsertClass {
                        box_height_plus_depth: (h >= 0).then(|| i32::try_from(h).unwrap_or(0)),
                        box_is_hbox: d.i() != 0,
                        count: d.i32(),
                        dimen: d.i32(),
                        skip: d.glue(),
                    },
                )
            })
            .collect();
        let pending = Pending {
            j,
            node,
            xs_i,
            pid,
            in_material,
            hint,
            key,
            rtn_guess,
            k,
            output_penalty,
            held,
            carried,
            nth,
            classes,
        };
        Ok((lp, pending))
    }

    /// A [`crate::vprof::VPROF`] unit's piece and items: decoded, its
    /// marks given ids in the loop's table, its markers resolved where
    /// its piece is.
    fn unit_items(
        &mut self,
        lp: &mut PageLoop,
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
        if lp.own.owner != Some(pid) {
            lp.own = OwnerReads {
                owner: Some(pid),
                ..OwnerReads::default()
            };
        }
        // (its markers read where the piece is, kept beside the items)
        let mut res = Resolved::default();
        if d.unresolved {
            let t = pp::start();
            let mut own = std::mem::take(&mut lp.own);
            let mut probe: Vec<PItem> = d
                .items
                .iter()
                .filter(|y| matches!(y, PItem::Params(None) | PItem::InsParams(_, None)))
                .cloned()
                .collect();
            let r = self.resolve_prof_of_with(pid, &mut probe, &mut own);
            lp.own = own;
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
        remap_marks(&mut items, &crate::vprof::unit_marks(v, x), &mut lp.marks);
        Ok((pid, std::sync::Arc::new(items), res))
    }

    /// A routine's material as profile items (§1026: its markers, then
    /// its list), its marks given ids in the loop's table.
    fn rtn_items(&mut self, lp: &mut PageLoop, rtn_value: ValueId) -> Vec<crate::vprof::PItem> {
        let v = self.v;
        let rs = list_of(v, rtn_value);
        let mut raw = rs.first().map(|&x| list_of(v, x)).unwrap_or_default();
        raw.extend(rs.get(1).map(|&x| list_of(v, x)).unwrap_or_default());
        let (ints, ms) = crate::vprof::encode_marks(v, &raw);
        let mut items = crate::vprof::decode(&ints);
        remap_marks(&mut items, &ms, &mut lp.marks);
        items
    }

    /// A page step's fold (the module's doc), from the job's start or
    /// from the page before's fire `pending`: the routine's material
    /// folded on (§1026), then the rest of the contributor it fired in,
    /// then each contributor after it ([`phitex_defuse::Ctx::next_contribution`]
    /// on [`crate::vprof::VPROF`]): its units decoded, their markers
    /// resolved where the piece is ([`Run::resolve_prof_of`]), each item
    /// folded with `lp.st` and its source pushed on `lp.srcs` as the
    /// builder takes it (§994–§1008), to the next fire, where the page's
    /// finisher and the next step are placed ([`Run::fire_page`],
    /// [`Run::cut`]). With none before the contributions in work or the
    /// text's end, the step places no page; after `\end`'s last page it
    /// defines [`PAGES_END`].
    pub(crate) fn page_step(&mut self, mut lp: PageLoop, pending: Option<Pending>) -> R<()> {
        let v = self.v;
        let vprof = v.field(crate::vprof::VPROF);
        if let Some(p) = pending {
            // (the contributor the page fired in, read again: its unit's
            // items after the fire are folded from it as it is)
            let t = crate::prof::pages::start();
            let next = self.cx.next_contribution(vprof, lp.cur)?;
            crate::prof::pages::add(crate::prof::pages::NEXT, t);
            let Some((node, u)) = next.filter(|n| n.0 == p.node) else {
                // (another before it now: the step before runs again)
                return Ok(());
            };
            let xs = v.seq_to_vec(u);
            let classes = p.classes.clone();
            if let Some(f) = self.after_fire(&mut lp, &p, &classes)? {
                let q = self.fire_at(&mut lp, node, p.pid, &f, true)?;
                return self.cut(&lp, q, classes, p.xs_i);
            }
            let (ui, from) = lp.frontier;
            if let Some(q) = self.fold_items(&mut lp, node, p.pid, ui, from as usize, &classes)? {
                return self.cut(&lp, q, classes, p.xs_i);
            }
            for (i, &x) in xs.iter().enumerate().skip(p.xs_i + 1) {
                if let Some((q, classes)) = self.fold_unit(&mut lp, node, x)? {
                    return self.cut(&lp, q, classes, i);
                }
            }
            lp.cur = Some(node);
            if !xs.is_empty() {
                lp.cur_units = Some(node);
            }
        }
        while !lp.ended {
            let t = crate::prof::pages::start();
            let next = self.cx.next_contribution(vprof, lp.cur)?;
            crate::prof::pages::add(crate::prof::pages::NEXT, t);
            let Some((node, u)) = next else { break };
            let xs = v.seq_to_vec(u);
            for (i, &x) in xs.iter().enumerate() {
                if let Some((q, classes)) = self.fold_unit(&mut lp, node, x)? {
                    return self.cut(&lp, q, classes, i);
                }
            }
            lp.cur = Some(node);
            if !xs.is_empty() {
                lp.cur_units = Some(node);
            }
        }
        if lp.ended {
            self.cx.define_with(v.field(PAGES_END), v.int(lp.st.j), Locality::Global);
        }
        Ok(())
    }

    /// The step's end at a page's fire: the next step placed after the
    /// page's finisher, on the loop as it stands.
    fn cut(&mut self, lp: &PageLoop, mut q: Pending, classes: Vec<(u16, InsertClass)>, xs_i: usize) -> R<()> {
        let v = self.v;
        q.classes = classes;
        q.xs_i = xs_i;
        let state = encode_state(v, lp, &q);
        self.cx.follow(self.ops.pages, vec![raw_value(v, Some(q.node)), state]);
        Ok(())
    }

    /// A contributor's unit `x` decoded and folded from its first item
    /// (its units' doc in [`Run::page_step`]); the fire it made, with
    /// its insertion classes, if any.
    fn fold_unit(
        &mut self,
        lp: &mut PageLoop,
        node: phitex_defuse::NodeId,
        x: ValueId,
    ) -> R<Option<(Pending, Vec<(u16, InsertClass)>)>> {
        use crate::prof::pages as pp;
        pp::unit();
        let (pid, items, res) = self.unit_items(lp, x)?;
        let classes = self.unit_classes(lp, &items, node_of(pid))?;
        // (units no page needs any more dropped)
        let t = pp::start();
        Self::drop_units(lp);
        pp::add(pp::DROP, t);
        if let std::collections::hash_map::Entry::Vacant(e) = lp.order.entry(pid) {
            e.insert(lp.seq);
            lp.seq += 1;
        }
        lp.prev.entry(pid).or_insert(lp.cur_units);
        lp.units.push(UnitRec { pid, src: x, items, res });
        let ui = lp.units.len() - 1;
        let t = pp::start();
        let r = self.fold_items(lp, node, pid, ui, 0, &classes);
        pp::add(pp::FOLD, t);
        Ok(r?.map(|q| (q, classes)))
    }

    /// Unit `ui`'s items from `from` folded; the page fired, if one.
    fn fold_items(
        &mut self,
        lp: &mut PageLoop,
        node: phitex_defuse::NodeId,
        pid: i64,
        ui: usize,
        from: usize,
        classes: &[(u16, InsertClass)],
    ) -> R<Option<Pending>> {
        use crate::vprof::PItem;
        let Some((items, res)) = lp.units.get(ui).map(|u| (std::sync::Arc::clone(&u.items), u.res.clone())) else {
            return Ok(None);
        };
        for (i, x) in items.iter().enumerate().skip(from) {
            let r;
            let x = match res.of(x) {
                Some(y) => {
                    r = y;
                    &r
                }
                None => x,
            };
            let item = u32::try_from(i).unwrap_or(u32::MAX);
            let src = Src::Piece { piece: pid, item };
            let s = SrcRef {
                src: Some(src),
                fold: lp.st.fold,
            };
            lp.srcs.push(src);
            lp.frontier = (ui, item + 1);
            match x {
                PItem::Build(k) => {
                    lp.st.fold.since_bp = 0;
                    let mut live = |n: u16| class_in(classes, n);
                    if let Some(f) = build_t(lp, *k, &mut live) {
                        let t = crate::prof::pages::start();
                        let q = self.fire_at(lp, node, pid, &f, false);
                        crate::prof::pages::add(crate::prof::pages::FIRE, t);
                        return Ok(Some(q?));
                    }
                }
                // (§1335: the main list ends there, no build after it; the
                // page is not shipped)
                PItem::Abort => {
                    lp.pend.clear();
                    lp.ended = true;
                    return Ok(None);
                }
                PItem::SetPage(..) | PItem::InsParams(..) | PItem::End(_) => {
                    lp.pend.push((x.clone(), s));
                }
                _ => fold_item_t(lp, x, s),
            }
        }
        Ok(None)
    }

    /// The registers of the insertion classes a unit's `items` and the
    /// contributions pending put on the page, as they reach it at `at`,
    /// where no marker names them (§1008 reads them at the `build_page`
    /// call); memoised for the unit's owner in `lp.own`.
    fn unit_classes(&mut self, lp: &mut PageLoop, items: &[crate::vprof::PItem], at: phitex_defuse::NodeId) -> R<Vec<(u16, InsertClass)>> {
        use crate::prof::pages as pp;
        let t = pp::start();
        let mut ns: Vec<u16> = Vec::new();
        for y in items.iter().chain(lp.pend.iter().map(|p| &p.0)) {
            if let crate::vprof::PItem::Node(Node::Ins(i)) = y
                && !ns.contains(&i.number)
            {
                ns.push(i.number);
            }
        }
        let mut classes = Vec::with_capacity(ns.len());
        for n in ns {
            if let Some(&c) = lp.own.classes.iter().find(|c| c.0 == n) {
                classes.push(c);
                continue;
            }
            let c = self.insert_class_at(n, at);
            if c.is_err() {
                pp::add(pp::CLASSES, t);
            }
            let c = (n, c?);
            lp.own.classes.push(c);
            classes.push(c);
        }
        pp::add(pp::CLASSES, t);
        Ok(classes)
    }


    /// The output routine's hint (as [`Run::output_hint`]) where piece
    /// `at` ends: the routine of a page firing there.
    pub(crate) fn output_hint_at(&mut self, at: phitex_defuse::NodeId) -> R<ValueId> {
        let v = self.v;
        let f = self.loc_field(Par::output as Loc);
        let out = match self.field_at(f, at, false)? {
            phitex_defuse::FieldAt::Def(x, _) => Some(x),
            phitex_defuse::FieldAt::Unset => None,
            phitex_defuse::FieldAt::Gone => self.par(Par::output)?,
        };
        // (the last routine's hint, by its value and the definitions: a
        // page's routine is mostly the page before's)
        type Last = Option<(u64, Option<ValueId>, usize, ValueId)>;
        thread_local! {
            static LAST: std::cell::Cell<Last> = const { std::cell::Cell::new(None) };
        }
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


    /// A page fired at candidate `f.k` of contributor `node` (or at the
    /// post-output build of the page before, `in_material`): its
    /// finisher's input made and the finisher placed; what the step after
    /// it folds on from ([`Pending`]).
    #[allow(
        clippy::too_many_lines,
        clippy::assertions_on_constants,
        reason = "one fire's whole step; a debug or observe build stops on the defect"
    )]
    fn fire_at(
        &mut self,
        lp: &mut PageLoop,
        node: phitex_defuse::NodeId,
        pid: i64,
        f: &LoopFire,
        in_material: bool,
    ) -> R<Pending> {
        let v = self.v;
        let j = lp.st.j - 1;
        // (the page's input as spans: the post-output build's, then the
        // pieces' items)
        let mut spans = spans_of(&lp.srcs[..lp.post.min(lp.srcs.len())]);
        let post = spans.len();
        spans.extend(spans_of(lp.srcs.get(lp.post..).unwrap_or_default()));
        // (the window of the pieces' items its spans name: after the
        // contributor before the first of them)
        let mut first: Option<(u64, i64)> = None;
        for s in &spans {
            let p = match s.src {
                Src::Piece { piece, .. } | Src::Held { piece, .. } => piece,
                Src::Rtn { .. } => continue,
            };
            let o = lp.order.get(&p).copied().unwrap_or(u64::MAX);
            if first.is_none_or(|f| o < f.0) {
                first = Some((o, p));
            }
        }
        let after = first.and_then(|(_, p)| lp.prev.get(&p).copied().flatten());
        let hint = self.output_hint_at(node)?;
        // (the guess: what the routine gave back on the last page before
        // fired on the text at the same `\outputpenalty`, which the
        // routine branches on, every ordinary one (above -10001, a page
        // shipped) alike; else for an ordinary page the markers it gave
        // back last (the page parameters it sets) and no list, for a
        // special one its last value as the evaluation began; nothing
        // for a page fired in a routine's material, so no chain of pages
        // grows on guesses)
        let ordinary = f.output_penalty > -10001;
        // (and whether the page before left dead cycles, §1012: a
        // routine's run after a page it did not ship goes otherwise)
        let key = (
            hint,
            crate::pages::finish::pen_class(f.output_penalty),
            lp.prev_fire.1 > 0,
        );
        let empty = || v.list(vec![v.list(Vec::new()), v.list(Vec::new())]);
        // (a dead cycle after one of the same routine and class, not one
        // `\end` fired, §1054: the page before's give-back one item longer
        // and `\deadcycles` one more, [`step_rtn`])
        let stepped = (!in_material && key.2 && f.output_penalty != END_PENALTY)
            .then_some(lp.prev_rtn)
            .flatten()
            .filter(|p| p.0.0 == key.0 && p.0.1 == key.1)
            .and_then(|p| step_rtn(v, p.1));
        let rtn_guess = if in_material {
            empty()
        } else if let Some(x) = stepped {
            x
        } else {
            match lp.last_rtn.iter().find(|e| e.0 == key) {
                Some(e) => e.1,
                // (and on a page `\end` fired, §1054: the routine runs as on
                // an ordinary one, its markers kept)
                None if ordinary || f.output_penalty == END_PENALTY => lp
                    .last_markers
                    .iter()
                    .find(|e| e.0 == hint)
                    .map_or_else(empty, |e| v.list(vec![e.1, v.list(Vec::new())])),
                None => self.rtn_guess(hint, key.1),
            }
        };
        if cfg!(feature = "observe") {
            // (the evaluation: its values and their epoch, the guesses
            // it began with)
            let eval = (
                std::ptr::from_ref(v) as usize,
                self.cx.values_epoch(),
                self.rtn_guess(hint, key.1),
            );
            guess_check(
                u64::try_from(node.raw()).unwrap_or(0),
                j,
                (eval, key, in_material, &lp.last_rtn, &lp.prev_rtn, &lp.last_markers),
                rtn_guess,
            );
        }
        let rtns: Vec<(i64, ValueId)> = {
            let mut pages: Vec<i64> = spans
                .iter()
                .filter_map(|s| match s.src {
                    Src::Rtn { page, .. } => Some(page),
                    _ => None,
                })
                .collect();
            pages.dedup();
            pages
                .into_iter()
                .filter_map(|p| lp.rtns.iter().rev().find(|r| r.0 == p).map(|r| (p, r.1)))
                .collect()
        };
        if !f.held_named {
            let msg = format!(
                "page {}: an insertion held over is no piece's item from an index on (split twice, or a routine's): its finisher's page may part from the builder's",
                j + 1
            );
            assert!(!cfg!(any(debug_assertions, feature = "observe")), "{msg}");
            self.engine_diag(&msg);
        }
        let inf: Vec<Src> = lp
            .inf
            .iter()
            .copied()
            .filter(|&b| Some(b) != f.brk && spans.iter().any(|sp| span_srcs(sp).any(|x| x == b)))
            .collect();
        let mut spans_pieces: Vec<i64> = vec![i64::from_ne_bytes(node.raw().to_ne_bytes())];
        for sp in &spans {
            if let Src::Piece { piece, .. } | Src::Held { piece, .. } = sp.src
                && !spans_pieces.contains(&piece)
            {
                spans_pieces.push(piece);
            }
        }
        // (the pages fired in this contributor before this one)
        let nth = if lp.nth.0 == Some(node) { lp.nth.1 } else { 0 };
        lp.nth = (Some(node), nth + 1);
        // (a routine's material by the pages before this one counted back
        // from it, not by their index among all the pages)
        let rel = |src: Src| match src {
            Src::Rtn { page, item } => Src::Rtn { page: j - page, item },
            s => s,
        };
        let fin = FinishIn {
            j,
            nth,
            normal: lp.st.normal,
            special: f.output_penalty <= -10001,
            hint,
            classes: f.classes.clone(),
            piece: node,
            k: f.k,
            in_material,
            after,
            spans: spans.iter().map(|s| Span { src: rel(s.src), len: s.len }).collect(),
            rtns: rtns.iter().map(|&(p, x)| (j - p, x)).collect(),
            fold: lp.began.0,
            params: lp.began.1,
            best: u32::try_from(f.best).unwrap_or(u32::MAX),
            output_penalty: f.output_penalty,
            holding: f.holding,
            held: u32::try_from(f.held.len()).unwrap_or(0),
            top_marks: lp.top_marks,
            post: u32::try_from(post).unwrap_or(0),
            post_markers: lp.post_markers.clone(),
            post_k: lp.post_k,
            prev_fire: lp.prev_fire,
            inf: inf.into_iter().map(rel).collect(),
            vers: page_vers(lp, &spans_pieces),
        };
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            eprintln!(
                "pages {}: page {} fires in piece {} at candidate {} (in material {in_material}, {} spans, best {}, penalty {})",
                self.cx.node().raw(),
                j + 1,
                node.raw(),
                f.k,
                fin.spans.len(),
                f.best,
                f.output_penalty
            );
        }
        let args = fin.args(v);
        self.place_finisher(&fin, args);
        // (§1012: the marks in force where the next page begins, the
        // `\botmark` of every class after this page's marks)
        lp.top_marks = bot_marks_after(v, lp.top_marks, &f.marks, &lp.marks);
        Ok(Pending {
            j,
            node,
            xs_i: 0,
            pid,
            in_material,
            hint,
            key,
            rtn_guess,
            k: f.k,
            output_penalty: f.output_penalty,
            held: f.held.clone(),
            carried: f.carried.clone(),
            nth,
            classes: Vec::new(),
        })
    }

    /// The page `p` fired: what its routine gave back (§1026: the
    /// finisher's output, the guess while it has none) folded as the next
    /// page's start with the held-over insertions and the contributions
    /// carried, then built at once; the fire that build makes, if any.
    #[allow(clippy::too_many_lines, reason = "one fire's whole give-back")]
    fn after_fire(&mut self, lp: &mut PageLoop, p: &Pending, classes: &[(u16, InsertClass)]) -> R<Option<LoopFire>> {
        use crate::vprof::PItem;
        let v = self.v;
        let j = p.j;
        let node = p.node;
        let guess = p.rtn_guess;
        // (its finisher's output, the definition reaching here: the
        // finisher is placed with it predicted, so read past the page's
        // name; none yet, the guess)
        let at = self.cx.path(&[phitex_defuse::Step::Index(2)]);
        let rtn = match self.cx.read_path(v.field(PAGE_OUT), at)? {
            None | Some(ValueId::UNIT) => guess,
            Some(x) => x,
        };
        if rtn != guess {
            Feedback::RoutineGivesBack.missed();
        }
        // (`PHITEX_RTN_TRACE=1`, profile: each page's routine material
        // as read, against the guess it was placed with)
        if crate::exec::prof_flag!("PHITEX_RTN_TRACE") {
            eprintln!(
                "rtn-trace: page {} node {} {} guess {} read {}",
                j + 1,
                self.cx.node().raw(),
                if rtn == guess { "same" } else { "DIFF" },
                v.render(guess).chars().take(400).collect::<String>(),
                v.render(rtn).chars().take(400).collect::<String>()
            );
        }
        let rs = list_of(v, rtn);
        lp.prev_rtn = Some((p.key, rtn));
        if !p.in_material {
            // (`\deadcycles` kept but on a page `\end` fired: a dead cycle
            // guessed there would fire another, §1054, and grow a chain of
            // pages on guesses)
            let given = if p.output_penalty == END_PENALTY {
                crate::pages::finish::rtn_as_guess(v, rtn)
            } else {
                rtn
            };
            match lp.last_rtn.iter_mut().find(|e| e.0 == p.key) {
                Some(e) => e.1 = given,
                None => lp.last_rtn.push((p.key, given)),
            }
            let m = rs.first().copied().unwrap_or_else(|| v.list(Vec::new()));
            match lp.last_markers.iter_mut().find(|e| e.0 == p.hint) {
                Some(e) => e.1 = m,
                None => lp.last_markers.push((p.hint, m)),
            }
        }
        let mut markers = rs.first().map(|&x| list_of(v, x)).unwrap_or_default();
        let list = rs.get(1).map(|&x| list_of(v, x)).unwrap_or_default();
        // (`\deadcycles` as the routine left it, where not 0: what an
        // `\end` in the material after it reads, §1054)
        lp.rtn_dead = rs.get(2).map_or(0, |&x| int_of(v, x));
        // (§1054, §1012 after the page: `\deadcycles` as its routine left
        // it, given back with its material)
        lp.dead = lp.rtn_dead;
        lp.prev_fire = (p.pid, lp.rtn_dead);
        self.resolve_params_of(p.pid, &mut markers)?;
        let m_len = markers.len();
        let rtn_value = v.list(vec![v.list(markers), v.list(list)]);
        let rtn_items = std::sync::Arc::new(self.rtn_items(lp, rtn_value));
        lp.rtns.push((j, rtn_value, std::sync::Arc::clone(&rtn_items)));
        // (the next page's input, in the order §1026 puts it back: the
        // routine's markers, the held-over insertions, its list, then
        // the contributions carried)
        let mut next: Vec<(Src, bool)> = Vec::new();
        for i in 0..m_len {
            next.push((Src::Rtn { page: j, item: u32::try_from(i).unwrap_or(0) }, false));
        }
        for h in &p.held {
            next.push((h.0, false));
        }
        for i in m_len..rtn_items.len() {
            next.push((Src::Rtn { page: j, item: u32::try_from(i).unwrap_or(0) }, false));
        }
        // (the carried: items made already (a routine's, a held one, an
        // `\end`'s own) as they are; a piece's items from the first one
        // carried to where the pieces are folded, folded again from the
        // main list's fold before it)
        let mut fold0 = lp.st.fold;
        let mut raw_from: Option<(usize, u32)> = None;
        let mut tail: Vec<Src> = Vec::new();
        for c in &p.carried {
            let Some(src) = c.src else { continue };
            match src {
                Src::Piece { piece, item } if end_sub_of(item).is_none() => {
                    let end_item = lp
                        .units
                        .iter()
                        .find(|u| u.pid == piece)
                        .and_then(|u| u.items.get(item as usize))
                        .is_some_and(|x| matches!(x, PItem::End(_)));
                    if end_item {
                        tail.push(src);
                    } else if raw_from.is_none() {
                        let ui = lp.units.iter().position(|u| u.pid == piece);
                        if let Some(ui) = ui {
                            raw_from = Some((ui, item));
                            fold0 = c.fold;
                        }
                    }
                }
                Src::Piece { .. } => tail.push(src),
                _ => {
                    if raw_from.is_some() {
                        tail.push(src);
                    } else {
                        next.push((src, false));
                    }
                }
            }
        }
        if let Some((ui, item)) = raw_from {
            // (to where the pieces are folded, or to the `\end` the
            // carried ends with)
            let end_at = tail.iter().find_map(|s| match s {
                Src::Piece { piece, item } => Some((*piece, end_sub_of(*item).map_or(*item, |e| e.0))),
                _ => None,
            });
            let (fu, fi) = lp.frontier;
            let mut u = ui;
            let mut i = item;
            'outer: while u < lp.units.len() {
                let rec = &lp.units[u];
                let n = u32::try_from(rec.items.len()).unwrap_or(0);
                while i < n {
                    if (u, i) >= (fu, fi) {
                        break 'outer;
                    }
                    if end_at == Some((rec.pid, i)) {
                        break 'outer;
                    }
                    next.push((Src::Piece { piece: rec.pid, item: i }, true));
                    i += 1;
                }
                u += 1;
                i = 0;
            }
        }
        for s in tail {
            next.push((s, false));
        }
        // (the next page begins: the main list's fold where its carried
        // pieces' items fold from, the page parameters in force)
        lp.st.fold = fold0;
        lp.began = (fold0, lp.st.params.map(|q| params_marker(v, &q)));
        lp.held_nodes.clone_from(&p.held);
        lp.srcs = next.iter().map(|x| x.0).collect();
        lp.post = lp.srcs.len();
        lp.post_k = p.k;
        lp.pend.clear();
        lp.page_src.clear();
        lp.page_ins.clear();
        // (what the post-output build reads of the insertion classes on
        // its material: their registers where the page fired)
        let mut ns: Vec<u16> = Vec::new();
        for (src, raw) in &next {
            let y = if *raw {
                loop_item(lp, *src)
            } else {
                Self::loop_node(lp, *src).or_else(|| loop_item(lp, *src))
            };
            if let Some(PItem::Node(Node::Ins(i))) = &y
                && !ns.contains(&i.number)
            {
                ns.push(i.number);
            }
        }
        let mut post_markers = Vec::new();
        let mut post_classes: Vec<(u16, InsertClass)> = Vec::new();
        for n in ns {
            let c = self.insert_class_at(n, node)?;
            post_markers.push(self.ins_marker(n, &c));
            post_classes.push((n, c));
        }
        lp.post_markers = post_markers;
        // (folded: as they are, or folded again with their builds and
        // markers spent)
        for (src, raw) in next {
            let s = SrcRef {
                src: Some(src),
                fold: lp.st.fold,
            };
            if raw {
                let Some(x) = loop_item(lp, src) else { continue };
                match x {
                    PItem::Build(_) => lp.st.fold.since_bp = 0,
                    PItem::Params(_) | PItem::SetPage(..) | PItem::InsParams(..) => {}
                    PItem::End(_) | PItem::Abort => lp.pend.push((x, s)),
                    _ => fold_item_t(lp, &x, s),
                }
            } else {
                let y = Self::loop_node(lp, src).or_else(|| loop_item(lp, src));
                if let Some(y) = y {
                    lp.pend.push((y, s));
                }
            }
        }
        for (n, c) in &post_classes {
            lp.pend.push((
                PItem::InsParams(*n, Some(*c)),
                SrcRef {
                    src: None,
                    fold: lp.st.fold,
                },
            ));
        }
        lp.st.fold.since_bp = 0;
        let mut live = |n: u16| class_in(classes, n);
        Ok(build_t(lp, p.k, &mut live))
    }

    /// A held-over insertion as the loop measured it (the source names it).
    fn loop_node(lp: &PageLoop, src: Src) -> Option<crate::vprof::PItem> {
        match src {
            Src::Held { .. } => lp
                .held_nodes
                .iter()
                .find(|h| h.0 == src)
                .map(|h| crate::vprof::PItem::Node(h.1.clone())),
            _ => None,
        }
    }

    /// Drops the units older than every source the current page's input
    /// names (their items are folded again only from there on).
    /// The first piece the input names is the oldest: `lp.srcs` names
    /// the pieces' items in the order they were folded (a fire keeps a
    /// suffix of them, after the routine's material and the held-over
    /// insertions, which name no unit), so no other is looked up.
    fn drop_units(lp: &mut PageLoop) {
        let first = lp.srcs.iter().find_map(|s| match s {
            Src::Piece { piece, .. } => Some(*piece),
            _ => None,
        });
        let keep = first
            .and_then(|p| lp.units.iter().position(|u| u.pid == p))
            .map_or(lp.frontier.0, |i| i.min(lp.frontier.0));
        if cfg!(feature = "observe") {
            let mut all = lp.frontier.0;
            for s in &lp.srcs {
                if let Src::Piece { piece, .. } = s
                    && let Some(i) = lp.units.iter().position(|u| u.pid == *piece)
                {
                    all = all.min(i);
                }
            }
            assert_eq!(keep, all, "the page's input names an older piece after a newer one");
        }
        if keep > 0 {
            lp.units.drain(..keep);
            lp.frontier.0 -= keep;
        }
        // (the places of the pieces the page's input and the units carried
        // name: the others' go, the loop is copied at every mark)
        let named = lp.units.len() + lp.srcs.len() + lp.pend.len() + lp.held_nodes.len();
        if lp.order.len() > 2 * named + 64 {
            let mut keep: phitex_defuse::FxHashSet<i64> = lp.units.iter().map(|u| u.pid).collect();
            let piece = |s: &Src| match *s {
                Src::Piece { piece, .. } | Src::Held { piece, .. } => Some(piece),
                Src::Rtn { .. } => None,
            };
            keep.extend(lp.srcs.iter().filter_map(piece));
            keep.extend(lp.pend.iter().filter_map(|p| p.1.src.as_ref().and_then(piece)));
            keep.extend(lp.page_src.iter().filter_map(|p| p.src.as_ref().and_then(piece)));
            keep.extend(lp.held_nodes.iter().filter_map(|h| piece(&h.0)));
            lp.order.retain(|p, _| keep.contains(p));
            lp.prev.retain(|p, _| keep.contains(p));
        }
    }


    /// A page's finisher placed after the piece the page fired in, where
    /// its routine runs in the text's order (its definitions reach the
    /// pieces after it), its arguments `fin`'s ([`FinishIn::args`]); the
    /// next step follows it ([`Run::cut`]). A page firing in the piece the
    /// step began in: the step already stands after that piece and the
    /// finisher before it, and the finisher is the step's child.
    pub(crate) fn place_finisher(&mut self, fin: &FinishIn, args: Vec<ValueId>) -> usize {
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            eprintln!(
                "place finisher of page {} (piece {} candidate {})",
                fin.j,
                fin.piece.raw(),
                fin.k
            );
        }
        // (`PHITEX_FIN_ARGS=1`: each page's arguments' parts, by id, to
        // compare the pages two runs place)
        if crate::exec::obs_flag!("PHITEX_FIN_ARGS") {
            let v = self.v;
            let parts: Vec<u64> = args.get(12).map(|&p| list_of(v, p).iter().map(|x| x.raw()).collect()).unwrap_or_default();
            let top: Vec<u64> = args.iter().map(|x| x.raw()).collect();
            eprintln!("fin-args: node {} page {} at {} parts {parts:?} top {top:?}", self.cx.node().raw(), fin.j, fin.piece.raw());
        }
        let began = match self.cx.args() {
            [x, _] => raw_node(self.v, *x),
            _ => None,
        };
        if began == Some(fin.piece) {
            self.cx.child(self.ops.finish, args)
        } else {
            self.cx.follow_after(fin.piece, self.ops.finish, args)
        }
    }
}

/// A class's registers as read for a unit, or nothing where none was.
fn class_in(classes: &[(u16, InsertClass)], n: u16) -> InsertClass {
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
                page_params = Some(x);
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
    /// means, read where the piece is (the record `pagepar`, or its cells
    /// where it is not defined there, as [`Run::eng_read`] reads them).
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
    fn page_cells_at(&mut self, at: i64) -> R<Option<Vec<Option<ValueId>>>> {
        let v = self.v;
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(at.to_ne_bytes()));
        let b = crate::fields::B_PAGE;
        let rf = self.ops.fields.slot_field(crate::fields::record_loc(b));
        let n = crate::fields::record_len(b);
        let cells = self.cx.read_at(rf, phitex_defuse::At::AfterOwn(at));
        if cells.is_err() && crate::exec::prof_flag!("PHITEX_PAGES_STOPS") {
            eprintln!("pages-stop: the page parameters after piece {}", at.raw());
        }
        let cells = match cells? {
            phitex_defuse::FieldAt::Def(x, _) => Some(list_of(v, x)).filter(|xs| xs.len() == n),
            phitex_defuse::FieldAt::Gone => return Ok(None),
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
                match self.cx.read_at(f, phitex_defuse::At::AfterOwn(at))? {
                    phitex_defuse::FieldAt::Def(x, _) => Some(x),
                    _ => None,
                }
            };
            xs.push(x);
        }
        Ok(Some(xs))
    }

    /// The page parameters reaching this node, as a `T_PAGE_PARAMS`
    /// marker: what a `[tag]` marker of its own items stands for, the
    /// builds before its later settings (a setting at a candidate
    /// puts a marker of its values there).
    pub(crate) fn page_params_reaching(&mut self) -> R<ValueId> {
        let v = self.v;
        let b = crate::fields::B_PAGE;
        let n = crate::fields::record_len(b);
        let cells = self.record(b)?;
        let absent = crate::fields::record_absent(v);
        let mut xs = Vec::with_capacity(n);
        for i in 0..n {
            let x = if let Some(c) = &cells {
                (c[i] != absent).then_some(c[i])
            } else {
                let loc =
                    crate::runstate::Loc::try_from(crate::fields::record_cell(b, i)).unwrap_or(0);
                let f = self.loc_field(loc);
                self.cx.read(f)?
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
    /// parameters reaching it ([`Run::page_params_at`]), and its `[tag,
    /// n]` insertion class markers the class's registers reaching it
    /// ([`Run::insert_class_at`]).
    pub(crate) fn resolve_params_of(&mut self, pid: i64, raw: &mut [ValueId]) -> R<()> {
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
                continue;
            }
            if let crate::vprof::PItem::Params(p @ None) = x {
                let pp = if let Some(pp) = own.params {
                    pp
                } else {
                    let pp = self.page_params_struct_at(pid)?;
                    own.params = Some(pp);
                    pp
                };
                *p = Some(pp);
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
/// piece ([`FinishIn::vers`]).
fn page_vers(lp: &PageLoop, pieces: &[i64]) -> Vec<(i64, ValueId)> {
    let mut out: Vec<(i64, ValueId)> = pieces
        .iter()
        .filter_map(|&p| lp.units.iter().rev().find(|u| u.pid == p).map(|u| (p, u.src)))
        .collect();
    out.sort_unstable_by_key(|e| e.0);
    out
}
