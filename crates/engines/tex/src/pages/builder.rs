//! Layer 4: the page builder (§980–§1010) as **one** node, [`Pages`].
//!
//! It is pushed where the job begins and reads the profiles of the
//! pieces and of the paragraphs' lines in text order, one contributor
//! at a time ([`phitex_defuse::Ctx::acc_next`] on
//! [`crate::vprof::VPROF`]), folding each with TeX's page builder over
//! profiles ([`crate::vprof::ScanState`]). A contributor still in work
//! stops the read: the node parks its loop ([`PageLoop`],
//! [`phitex_defuse::Ctx::park`]) and goes on from there when the engine
//! runs it again, so the loop costs microseconds a paragraph however the
//! pieces finish.
//!
//! Where a page fires the loop notes it, with everything its finisher
//! needs to assemble it alone ([`FinishIn`]): where each item on it came
//! from ([`Src`], [`Span`]: a piece's items, a routine's material, a
//! held-over insertion), the builder's state where the page began and its
//! decisions (the best break, `\outputpenalty`, the insertions held
//! over), the marks in force where it began. No finisher reads another's
//! page, and none reads the profiles.
//!
//! The next page begins with what the routine gives back (§1026), which
//! the loop folds on a guess ([`FinishIn::rtn_guess`],
//! [`Feedback::RoutineGivesBack`]): the routine's last value on another
//! page, or nothing. A finisher whose routine gave otherwise contributes
//! the real one to [`PAGE_MISS`], which the loop reads whole at its
//! start; the node runs again on it and folds that page's next page on
//! the real material. The guess in a finisher's arguments stays the
//! guess, never the real value, so a miss read back does not change the
//! finisher it came from.
//!
//! It places each page's finisher right after the piece its page fired
//! in as the page fires ([`Run::place_finisher`], `follow_after_any`):
//! the engine publishes a run's placements at every stop, so finishers
//! run while the loop still folds. A parked loop saves work, not output:
//! every attempt places again the pages it had. At its end it defines
//! [`PAGES_END`].
//!
//! What the loop reads where a piece is (the page parameters, an
//! insertion class's registers, `\deadcycles` at `\end`) it reads at that
//! piece, a read past its own node ([`Run::page_params_at`],
//! [`Run::insert_class_at`], [`Feedback::BuilderRegisters`]).
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

impl Span {
    /// As a value: `[tag, a, b, c, len]` (tags 0 piece, 1 routine, 2
    /// held).
    pub(crate) fn encode(&self, v: &Values) -> ValueId {
        let (t, a, b, c) = match self.src {
            Src::Piece { piece, item } => (0, piece, i64::from(item), 0),
            Src::Rtn { page, item } => (1, page, i64::from(item), 0),
            Src::Held { piece, item, from } => (2, piece, i64::from(item), i64::from(from)),
        };
        v.list(vec![v.int(t), v.int(a), v.int(b), v.int(c), v.int(i64::from(self.len))])
    }

    pub(crate) fn decode(v: &Values, x: ValueId) -> Option<Span> {
        let xs = list_of(v, x);
        let n = |i: usize| xs.get(i).map_or(0, |&y| int_of(v, y));
        let u = |i: usize| u32::try_from(n(i)).unwrap_or(0);
        let src = match n(0) {
            0 => Src::Piece {
                piece: n(1),
                item: u(2),
            },
            1 => Src::Rtn {
                page: n(1),
                item: u(2),
            },
            2 => Src::Held {
                piece: n(1),
                item: u(2),
                from: u(3),
            },
            _ => return None,
        };
        Some(Span { src, len: u(4) })
    }
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
#[derive(Clone)]
pub(crate) struct UnitRec {
    pub pid: i64,
    pub items: std::sync::Arc<Vec<crate::vprof::PItem>>,
}

/// The page builder's node's loop, kept over a park.
#[derive(Clone)]
pub(crate) struct PageLoop {
    /// The builder over profiles (its own pending list unused: `pend`).
    pub st: crate::vprof::ScanState,
    /// The last contributor folded (the cursor of `acc_next`).
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
    /// The best breaks of the pages fired, penalties: 10000 from their
    /// fire on (§1013), wherever they are folded again.
    pub inf: Vec<Src>,
    /// What each routine (by its hint) gave back on the last page fired
    /// on the text, `\deadcycles` left out: the next page's guess
    /// ([`FinishIn::rtn_guess`]).
    pub last_rtn: Vec<(ValueId, ValueId)>,
    /// The piece the page fired last fired in and `rtn_dead`
    /// ([`FinishIn::prev_fire`]).
    pub prev_fire: (i64, i64),
    /// The marks the profiles' mark items name (a unit's marks given ids
    /// here as it is decoded, [`remap_marks`]).
    pub marks: Vec<ValueId>,
    /// The routines' real material where it missed the guess
    /// ([`PAGE_MISS`]), read once at the loop's start: `(page, rtn)`.
    pub misses: Vec<(i64, ValueId)>,
    /// The pages fired so far: the piece each fired in, its finisher's
    /// input.
    pub pages: Vec<(phitex_defuse::NodeId, FinishIn)>,
    /// Whether the loop has read [`PAGE_MISS`] (once, at its start).
    pub misses_read: bool,
    /// The contributions not yet looked at, with their sources.
    pub pend: Vec<(crate::vprof::PItem, SrcRef)>,
    /// The sources of the builder's page, item for item.
    pub page_src: Vec<SrcRef>,
    /// The units folded whose items a page's carried contributions may
    /// fold again (the oldest dropped once no page needs them).
    pub units: Vec<UnitRec>,
    /// The contributor before each piece folded: a finisher's window of
    /// [`PITEMS`] starts after it.
    pub prev: phitex_defuse::FxHashMap<i64, Option<phitex_defuse::NodeId>>,
    /// The routines' material the loop folded, by page: `(page, value,
    /// its markers then its list as profile items)`.
    pub rtns: Vec<(i64, ValueId, std::sync::Arc<Vec<crate::vprof::PItem>>)>,
    /// Where the pieces' items are folded to: `(unit index in `units`,
    /// the next item)`.
    pub frontier: (usize, u32),
    /// The piece and candidate the last page fired at (a page firing at
    /// the post-output build fires there too).
    pub last_fire: Option<(phitex_defuse::NodeId, i64, u32)>,
    /// The insertion class of each item of the builder's page (none for
    /// any other item).
    pub page_ins: Vec<Option<u16>>,
    /// The insertions the last fire held over, measured, by source.
    pub held_nodes: Vec<(Src, Node)>,
    /// Each piece's place among those folded (the earliest a page names
    /// starts its finisher's window).
    pub order: phitex_defuse::FxHashMap<i64, u64>,
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
            inf: Vec::new(),
            last_rtn: Vec::new(),
            prev_fire: (-1, 0),
            marks: Vec::new(),
            misses: Vec::new(),
            pages: Vec::new(),
            misses_read: false,
            pend: Vec::new(),
            page_src: Vec::new(),
            units: Vec::new(),
            prev: phitex_defuse::FxHashMap::default(),
            rtns: Vec::new(),
            frontier: (0, 0),
            last_fire: None,
            page_ins: Vec::new(),
            held_nodes: Vec::new(),
            order: phitex_defuse::FxHashMap::default(),
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
                None => u.items.get(item as usize).cloned(),
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
    dead: i64,
    class_of_live: &mut dyn FnMut(u16) -> InsertClass,
) -> Option<LoopFire> {
    use crate::vprof::PItem;
    let mut classes: Vec<(u16, InsertClass)> = Vec::new();
    let mut page_params = None;
    let mut buf = std::mem::take(&mut lp.pend);
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
                let i = usize::try_from(*i).unwrap_or(8);
                if i < 8 {
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
    if page_params.is_some() {
        lp.st.params = page_params;
    }
    let params = lp.st.params.unwrap_or(PageParams {
        vsize: 0,
        max_depth: 0,
        top_skip: Glue::ZERO,
        holding_inserts: 0,
        saving_vdiscards: false,
    });
    let mut queue = buf;
    queue.reverse();
    let mut end_seen: Option<(PItem, SrcRef)> = None;
    while let Some((x, s)) = queue.pop() {
        let p = match x {
            PItem::End(ref ys) => {
                if lp.st.pb.page.is_empty() && queue.is_empty() && dead == 0 {
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
                    queue.push((y.clone(), SrcRef { src, fold: s.fold }));
                }
                end_seen = Some((x.clone(), s));
                continue;
            }
            PItem::Abort => {
                return None;
            }
            PItem::Node(ref n) => n.clone(),
            _ => continue,
        };
        let next = if matches!(p, Node::Kern(..)) {
            match queue.last() {
                Some((PItem::Node(n), _)) => Some(n.clone()),
                _ => None,
            }
        } else {
            None
        };
        let before = lp.st.pb.page.len();
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
            Outcome::Wait(_) => {
                queue.push((x, s));
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
                if f.output_penalty < phitex_typeset::linebreak::INF_PENALTY {
                    let brk = srcs.get(f.best).map_or(s.src, |b| b.src);
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
                carried.extend(queue.drain(..).rev().map(|(_, s)| s));
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
                    carried,
                });
            }
        }
    }
    queue.reverse();
    lp.pend = queue;
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

/// The page builder: one node, pushed where the job begins ([`PageLoop`];
/// the module's doc).
pub(crate) struct Pages(pub Shared);

impl Operation for Pages {
    fn name(&self) -> &'static str {
        "pages"
    }
    /// Where the pages end, once every finisher is placed.
    fn defs(&self, _: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(vec![(v.field(PAGES_END), Locality::Global)])
    }
    fn may_define(&self, _: &[ValueId], v: &Values) -> MayDefine {
        MayDefine::Names(vec![v.field(PAGES_END)])
    }
    /// What it folds ([`PageNode::folds`]).
    fn reads(&self, _: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(PageNode::Pages.reads(v, &[]))
    }
    /// No units of the page accumulators.
    fn static_contribs(&self, _: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        Some(no_units(v))
    }
    /// All it contributes: a window of any other accumulator does not
    /// wait for it.
    fn contribs_complete(&self, _: &[ValueId], _: &Values) -> bool {
        true
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let o = ops(&self.0);
        let parked = cx
            .parked()
            .and_then(|t| t.downcast::<PageLoop>().ok())
            .map(|b| *b);
        // (a run, for its reads where a piece is; no text runs in it)
        let mut r = Run::new(cx, o);
        let mut lp = parked.unwrap_or_else(PageLoop::new);
        match r.page_loop(&mut lp) {
            Ok(()) => {}
            Err(Halt::Stop(s)) => {
                r.cx.park(Box::new(lp));
                return Err(s);
            }
            Err(_) => unreachable!("the page builder's loop runs no command"),
        }
        r.place_pages(&lp);
        Ok(ValueId::UNIT)
    }
}

impl Run<'_, '_> {
    /// A routine's give-back as the evaluation began (§1026): its last
    /// value on another page (`crate::run::Ops::fin_defs` then, so every
    /// attempt of the loop guesses the same), `\deadcycles` left out,
    /// else nothing given back. The guess of page `j`
    /// ([`FinishIn::rtn_guess`], made in [`Run::page_loop`]) is what the
    /// same routine gave back on the last page the loop fired on the text
    /// before `j` (the miss it read, or that page's own guess where it
    /// read none), else this; nothing for a page fired in a routine's
    /// material (no chain of pages grows on guesses). It depends on the
    /// pages before `j` alone: a page's own miss never changes it.
    pub(crate) fn rtn_guess(&self, hint: ValueId) -> ValueId {
        let v = self.v;
        let empty = || v.list(vec![v.list(Vec::new()), v.list(Vec::new())]);
        if hint == ValueId::UNIT {
            return empty();
        }
        let rtn = v.field(RTN);
        self.ops
            .fin_defs_start
            .locked()
            .get(&fin_key(v, hint, &[]))
            .and_then(|d| d.last.iter().find(|e| e.0 == rtn).map(|e| e.1))
            // (`\deadcycles` guessed 0, a page shipped: a dead cycle
            // guessed from the last run would grow a chain of pages on
            // guesses)
            .map_or_else(empty, |x| {
                let xs = list_of(v, x);
                if xs.len() > 2 {
                    v.list(xs[..2].to_vec())
                } else {
                    x
                }
            })
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
/// ([`Run::rtn_guess`]): the same misses of the pages before `j`, the
/// same routine, give the same guess in every attempt; a guess changed
/// by page `j`'s own miss read back is a defect (its finisher placed
/// anew on its own miss).
fn guess_check(node: u64, j: i64, misses: &[(i64, ValueId)], how: impl std::hash::Hash, guess: ValueId) {
    use std::hash::{Hash, Hasher};
    type Seen = Option<phitex_defuse::FxHashMap<(u64, i64, u64), ValueId>>;
    static SEEN: std::sync::Mutex<Seen> = std::sync::Mutex::new(None);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for m in misses.iter().filter(|m| m.0 < j) {
        m.hash(&mut h);
    }
    how.hash(&mut h);
    let key = (node, j, h.finish());
    let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let prev = *seen.get_or_insert_with(Default::default).entry(key).or_insert(guess);
    assert!(
        prev == guess,
        "page {}: its routine's guess changed with nothing before it changed (its own miss read back?)",
        j + 1
    );
}

/// What the loop saves before folding a unit, put back where the fold
/// stops on a read ([`Halt::Stop`]): the unit is folded again whole.
struct LoopSnap {
    st: crate::vprof::ScanState,
    srcs: Vec<Src>,
    post: usize,
    post_markers: Vec<ValueId>,
    post_k: u32,
    began: (FoldState, Option<ValueId>),
    top_marks: ValueId,
    rtn_dead: i64,
    inf: Vec<Src>,
    prev_fire: (i64, i64),
    last_rtn: Vec<(ValueId, ValueId)>,
    pend: Vec<(crate::vprof::PItem, SrcRef)>,
    page_src: Vec<SrcRef>,
    page_ins: Vec<Option<u16>>,
    held_nodes: Vec<(Src, Node)>,
    units: usize,
    pages: usize,
    rtns: usize,
    frontier: (usize, u32),
    last_fire: Option<(phitex_defuse::NodeId, i64, u32)>,
}

impl PageLoop {
    fn snap(&self) -> LoopSnap {
        LoopSnap {
            st: self.st.clone(),
            srcs: self.srcs.clone(),
            post: self.post,
            post_markers: self.post_markers.clone(),
            post_k: self.post_k,
            began: self.began,
            top_marks: self.top_marks,
            rtn_dead: self.rtn_dead,
            inf: self.inf.clone(),
            prev_fire: self.prev_fire,
            last_rtn: self.last_rtn.clone(),
            pend: self.pend.clone(),
            page_src: self.page_src.clone(),
            page_ins: self.page_ins.clone(),
            held_nodes: self.held_nodes.clone(),
            units: self.units.len(),
            pages: self.pages.len(),
            rtns: self.rtns.len(),
            frontier: self.frontier,
            last_fire: self.last_fire,
        }
    }

    fn restore(&mut self, s: LoopSnap) {
        self.st = s.st;
        self.srcs = s.srcs;
        self.post = s.post;
        self.post_markers = s.post_markers;
        self.post_k = s.post_k;
        self.began = s.began;
        self.top_marks = s.top_marks;
        self.rtn_dead = s.rtn_dead;
        self.inf = s.inf;
        self.prev_fire = s.prev_fire;
        self.last_rtn = s.last_rtn;
        self.pend = s.pend;
        self.page_src = s.page_src;
        self.page_ins = s.page_ins;
        self.held_nodes = s.held_nodes;
        // (a unit folded before the stop and dropped since is not
        // brought back: the stop comes before any page fired in it
        // is noted, or the pages are truncated with it)
        self.units.truncate(s.units);
        self.pages.truncate(s.pages);
        self.rtns.truncate(s.rtns);
        self.frontier = s.frontier;
        self.last_fire = s.last_fire;
    }
}

impl Run<'_, '_> {
    /// The page builder's loop over the profiles (the module's doc), from
    /// where `lp` stands to the unit's end:
    ///
    /// 1. once, the misses ([`PAGE_MISS`], `acc_fold` from this node to
    ///    the unit's end, which reads past the text in work,
    ///    [`MissMonoid`]) into `lp.misses`; then every page already in
    ///    `lp.pages` placed again, in order ([`Run::place_finisher`]): a
    ///    parked token saves work, not output, and every attempt makes
    ///    its whole result;
    /// 2. each contributor after `lp.cur` ([`phitex_defuse::Ctx::acc_next`]
    ///    on [`crate::vprof::VPROF`]): its units decoded, their markers
    ///    resolved where the piece is ([`Run::resolve_prof_of`]),
    ///    `\deadcycles` read at the piece where it holds `\end` (§1054),
    ///    each item folded with `lp.st` and its source pushed on
    ///    `lp.srcs` as the builder takes it (§994–§1008: what the page
    ///    takes, what waits, what is discarded at the page's top);
    /// 3. where a page fires (§1005, §1014–§1017): its [`FinishIn`] from
    ///    `lp.srcs` up to the fire and the builder's decisions, pushed on
    ///    `lp.pages` with the piece it fired in and its finisher placed
    ///    once the unit it fired in is folded ([`Run::place_finisher`]:
    ///    the engine publishes a run's placements at every stop, so the
    ///    finishers run while the loop still folds; a fold stopped inside
    ///    a unit places none of its pages, so no attempt places fewer
    ///    than the stop before it published); `lp.srcs` keeps what goes
    ///    back to the contributions (the page's items from the best break
    ///    on, §1017), after the held-over insertions ([`Src::Held`]) and
    ///    the routine's material ([`Src::Rtn`]: the miss read back where
    ///    the page has one, else the guess, [`Run::rtn_guess`]), which
    ///    are folded first; `lp.began` and `lp.top_marks` the new page's.
    ///
    /// A read of a piece still in work stops ([`Halt::Stop`]): the node
    /// parks `lp` and goes on from it.
    pub(crate) fn page_loop(&mut self, lp: &mut PageLoop) -> R<()> {
        let v = self.v;
        if !lp.misses_read {
            let me = self.cx.node();
            let x = self.cx.acc_fold(v.field(PAGE_MISS), Some(me), None)?;
            lp.misses = v
                .seq_to_vec(x)
                .into_iter()
                .filter_map(|e| match list_of(v, e)[..] {
                    [j, rtn] => Some((int_of(v, j), rtn)),
                    _ => None,
                })
                .collect();
            lp.misses_read = true;
        }
        // (the pages an attempt before fired: placed again, in page
        // order, before folding further: a park keeps work, not output,
        // so every attempt makes its whole result, the same calls giving
        // the same nodes; a page not placed again is freed)
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            eprintln!(
                "pages {}: attempt, {} pages placed again, from {:?}, misses {:?}",
                self.cx.node().raw(),
                lp.pages.len(),
                lp.cur.map(phitex_defuse::NodeId::raw),
                lp.misses.iter().map(|m| m.0).collect::<Vec<_>>()
            );
        }
        for (at, fin) in &lp.pages {
            self.place_finisher(*at, fin);
        }
        let vprof = v.field(crate::vprof::VPROF);
        while let Some((node, u)) = self.cx.acc_next(vprof, lp.cur)? {
            for x in v.seq_to_vec(u) {
                let (pid, ints) = crate::vprof::unit_parts(v, x);
                let mut items = crate::vprof::decode(&ints);
                remap_marks(&mut items, &crate::vprof::unit_marks(v, x), &mut lp.marks);
                self.resolve_prof_of(pid, &mut items)?;
                let at = node_of(pid);
                let dead = if crate::vprof::has_end(&items) {
                    self.dead_at(at)?
                } else {
                    0
                };
                // (the registers of the insertion classes this unit and
                // the contributions pending put on the page, as they
                // reach it, where no marker names them: §1008 reads them
                // at the `build_page` call)
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
                    classes.push((n, self.insert_class_at(n, at)?));
                }
                // (units no page needs any more dropped, before the
                // snapshot a stop puts back)
                Self::drop_units(lp);
                let snap = lp.snap();
                let placed = lp.pages.len();
                if let Err(h) = self.fold_unit(lp, node, pid, items, dead, &classes) {
                    lp.restore(snap);
                    return Err(h);
                }
                // (the pages the unit fired placed once its fold is
                // through: a fold stopped inside it places none, so the
                // next attempt, which folds the unit again whole, makes
                // the same placements, never one fewer than the stop
                // published)
                for (at, fin) in &lp.pages[placed..] {
                    self.place_finisher(*at, fin);
                }
            }
            lp.cur = Some(node);
        }
        Ok(())
    }

    /// `\deadcycles` where piece `at` ends (§1054's read at `\end`).
    pub(crate) fn dead_at(&mut self, at: phitex_defuse::NodeId) -> R<i64> {
        if at == self.cx.node() {
            return self.cint(crate::fields::cell::DEADCYCLES);
        }
        let v = self.v;
        let f = self.loc_field(crate::fields::cell::DEADCYCLES as Loc);
        Ok(match self.field_at(f, at, false)? {
            phitex_defuse::FieldAt::Def(x, _) => int_of(v, x),
            phitex_defuse::FieldAt::Unset => 0,
            phitex_defuse::FieldAt::Gone => self.cint(crate::fields::cell::DEADCYCLES)?,
        })
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
        let toks = toks_of(v, out);
        if toks.is_empty() {
            return Ok(ValueId::UNIT);
        }
        let mut text = String::new();
        self.show_toks_into(&mut text, &toks)?;
        let h = {
            let defs = self.ops.defs.locked();
            crate::hints::routine_hint(&text, &defs)
        };
        Ok(v.str(&h.encode()))
    }

    /// One unit's items folded (its units' doc in [`Run::page_loop`]).
    #[allow(clippy::too_many_arguments)]
    fn fold_unit(
        &mut self,
        lp: &mut PageLoop,
        node: phitex_defuse::NodeId,
        pid: i64,
        items: Vec<crate::vprof::PItem>,
        mut dead: i64,
        classes: &[(u16, InsertClass)],
    ) -> R<()> {
        use crate::vprof::PItem;
        let seq = u64::try_from(lp.order.len()).unwrap_or(u64::MAX);
        lp.order.entry(pid).or_insert(seq);
        lp.prev.entry(pid).or_insert(lp.cur);
        lp.units.push(UnitRec {
            pid,
            items: std::sync::Arc::new(items),
        });
        let ui = lp.units.len() - 1;
        let items = std::sync::Arc::clone(&lp.units[ui].items);
        for (i, x) in items.iter().enumerate() {
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
                    let mut fire = build_t(lp, *k, dead, &mut live);
                    let mut in_material = false;
                    while let Some(f) = fire {
                        fire = self.on_fire(lp, node, pid, &f, in_material, classes)?;
                        // (§1054 after a page fired: `\deadcycles` as its
                        // routine left it, given back with its material)
                        dead = lp.rtn_dead;
                        in_material = true;
                    }
                }
                PItem::SetPage(..) | PItem::InsParams(..) | PItem::End(_) | PItem::Abort => {
                    lp.pend.push((x.clone(), s));
                }
                _ => fold_item_t(lp, x, s),
            }
        }
        Ok(())
    }

    /// A page fired at candidate `f.k` of piece `node` (or at the
    /// post-output build of the page before, `in_material`): its
    /// finisher's input made and the finisher placed, then the next
    /// page's input folded and built at once; the fire that build makes,
    /// if any.
    #[allow(
        clippy::too_many_lines,
        clippy::too_many_arguments,
        clippy::assertions_on_constants,
        reason = "one fire's whole step; a debug or observe build stops on the defect"
    )]
    fn on_fire(
        &mut self,
        lp: &mut PageLoop,
        node: phitex_defuse::NodeId,
        pid: i64,
        f: &LoopFire,
        in_material: bool,
        classes: &[(u16, InsertClass)],
    ) -> R<Option<LoopFire>> {
        use crate::vprof::PItem;
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
        // (the guess: what the routine gave back on the page before that
        // fired on the text, where one did in this loop, else its last
        // value as the evaluation began; nothing for a page fired in a
        // routine's material, so no chain of pages grows on guesses)
        let rtn_guess = if in_material {
            v.list(vec![v.list(Vec::new()), v.list(Vec::new())])
        } else {
            lp.last_rtn
                .iter()
                .find(|e| e.0 == hint)
                .map_or_else(|| self.rtn_guess(hint), |e| e.1)
        };
        if cfg!(feature = "observe") {
            // (the evaluation: its values and their epoch, the guesses
            // it began with)
            let eval = (
                std::ptr::from_ref(v) as usize,
                self.cx.values_epoch(),
                self.rtn_guess(hint),
            );
            guess_check(self.cx.node().raw(), j, &lp.misses, (eval, hint, in_material), rtn_guess);
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
        let held: Vec<Src> = f.held.iter().map(|h| h.0).collect();
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
            .filter(|&b| spans.iter().any(|sp| span_srcs(sp).any(|x| x == b)))
            .collect();
        let fin = FinishIn {
            j,
            normal: lp.st.normal,
            special: f.output_penalty <= -10001,
            hint,
            classes: f.classes.clone(),
            piece: node,
            k: f.k,
            in_material,
            after,
            spans,
            rtns,
            fold: lp.began.0,
            params: lp.began.1,
            best: u32::try_from(f.best).unwrap_or(u32::MAX),
            output_penalty: f.output_penalty,
            holding: f.holding,
            held: u32::try_from(held.len()).unwrap_or(0),
            top_marks: lp.top_marks,
            rtn_guess,
            post: u32::try_from(post).unwrap_or(0),
            post_markers: lp.post_markers.clone(),
            post_k: lp.post_k,
            prev_fire: lp.prev_fire,
            inf,
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
        lp.pages.push((node, fin));
        // (§1012: the marks in force where the next page begins, the
        // `\botmark` of every class after this page's marks)
        lp.top_marks = bot_marks_after(v, lp.top_marks, &f.marks, &lp.marks);
        lp.last_fire = Some((node, pid, f.k));
        // (the routine's material, §1026: its real one where it missed
        // the guess; its page parameter markers resolved where the page
        // fired)
        let rtn = lp
            .misses
            .iter()
            .rev()
            .find(|m| m.0 == j)
            .map_or(rtn_guess, |m| m.1);
        let rs = list_of(v, rtn);
        if !in_material {
            let given = if rs.len() > 2 {
                v.list(rs[..2].to_vec())
            } else {
                rtn
            };
            match lp.last_rtn.iter_mut().find(|e| e.0 == hint) {
                Some(e) => e.1 = given,
                None => lp.last_rtn.push((hint, given)),
            }
        }
        let mut markers = rs.first().map(|&x| list_of(v, x)).unwrap_or_default();
        let list = rs.get(1).map(|&x| list_of(v, x)).unwrap_or_default();
        // (`\deadcycles` as the routine left it, where not 0: what an
        // `\end` in the material after it reads, §1054)
        lp.rtn_dead = rs.get(2).map_or(0, |&x| int_of(v, x));
        lp.prev_fire = (pid, lp.rtn_dead);
        self.resolve_params_of(pid, &mut markers)?;
        let m_len = markers.len();
        let rtn_value = v.list(vec![v.list(markers.clone()), v.list(list.clone())]);
        let mut raw = markers;
        raw.extend(list);
        let (ints, ms) = crate::vprof::encode_marks(v, &raw);
        let mut rtn_items = crate::vprof::decode(&ints);
        remap_marks(&mut rtn_items, &ms, &mut lp.marks);
        let rtn_items = std::sync::Arc::new(rtn_items);
        lp.rtns.push((j, rtn_value, std::sync::Arc::clone(&rtn_items)));
        // (the next page's input, in the order §1026 puts it back: the
        // routine's markers, the held-over insertions, its list, then
        // the contributions carried)
        let mut next: Vec<(Src, bool)> = Vec::new();
        for i in 0..m_len {
            next.push((Src::Rtn { page: j, item: u32::try_from(i).unwrap_or(0) }, false));
        }
        for h in &held {
            next.push((*h, false));
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
        for c in &f.carried {
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
        lp.began = (fold0, lp.st.params.map(|p| params_marker(v, &p)));
        lp.held_nodes.clone_from(&f.held);
        lp.srcs = next.iter().map(|x| x.0).collect();
        lp.post = lp.srcs.len();
        lp.post_k = f.k;
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
        let dead = lp.rtn_dead;
        Ok(build_t(lp, f.k, dead, &mut live))
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
    fn drop_units(lp: &mut PageLoop) {
        let mut keep = lp.frontier.0;
        for s in &lp.srcs {
            if let Src::Piece { piece, .. } = s
                && let Some(i) = lp.units.iter().position(|u| u.pid == *piece)
            {
                keep = keep.min(i);
            }
        }
        if keep > 0 {
            lp.units.drain(..keep);
            lp.frontier.0 -= keep;
        }
    }


    /// A page's finisher placed right after the piece its page fired in
    /// (`follow_after_any`, its position the composite of the piece's, the
    /// placer and the order placed; docs/spec-layers.md §6).
    pub(crate) fn place_finisher(&mut self, at: phitex_defuse::NodeId, fin: &FinishIn) {
        let v = self.v;
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            eprintln!(
                "place finisher of page {} (piece {} candidate {})",
                fin.j,
                at.raw(),
                fin.k
            );
        }
        self.cx.follow_after_any(at, self.ops.finish, fin.args(v));
    }

    /// The loop's end: where the pages end, once every finisher is
    /// placed ([`Run::page_loop`] placed them).
    pub(crate) fn place_pages(&mut self, lp: &PageLoop) {
        let _ = lp;
        let v = self.v;
        self.cx
            .define_with(v.field(PAGES_END), ValueId::UNIT, Locality::Global);
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
        dead: i64,
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
                if list_of(v, x).len() == 2 {
                    unresolved.push(u16::try_from(int_at(v, x, 1)).unwrap_or(0));
                } else {
                    classes.push(class_of(v, x));
                }
                false
            }
            _ => true,
        });
        // (a marker no step resolved: the registers as the fold has them)
        for n in unresolved {
            classes.push((n, live(n)?));
        }
        if page_params.is_some() {
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
                    if self.pb.page.is_empty() && queue.is_empty() && dead == 0 {
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
            match self.field_at(f, at, false)? {
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
        let mut m = None;
        let mut classes: Vec<(u16, InsertClass)> = Vec::new();
        for x in items.iter_mut() {
            if let crate::vprof::PItem::InsParams(n, c @ None) = x {
                let n = *n;
                *c = Some(if let Some(&(_, k)) = classes.iter().find(|k| k.0 == n) {
                    k
                } else {
                    let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
                    let k = self.insert_class_at(n, at)?;
                    classes.push((n, k));
                    k
                });
                continue;
            }
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
