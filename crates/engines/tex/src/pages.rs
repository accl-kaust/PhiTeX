//! Pages: where the document's pages finish and their output routines,
//! as nodes of the graph, values by def-use only.
//!
//! - Text pieces (`exec`) typeset in parallel. Each item a piece puts on
//!   the outer vertical list carries its finish candidate ([`Cand`]: the
//!   piece and the index `k` of the moment TeX looks for a full page,
//!   §1091 §1096 §1100 §1103 §1200); at its end the piece defines its
//!   items ([`items_field`]).
//! - A page-state node ([`PageStateOp`]) stands after each piece: the
//!   page builder in SSA form. It reads the version of the page state
//!   reaching it ([`PS`]) and its piece's items, applies Knuth's rules
//!   (§980–§1010) item by item, defines the next version, and outputs
//!   where a page finishes ([`Fin`]) if one does: it stops there.
//! - A finisher ([`Finish`]) stands where a page finishes. Its page is
//!   its argument; it reads what reaches it, runs the output routine,
//!   defines its `\global`s, the marks, [`SHIPPED`], and a fresh version
//!   of the page state (the next page's start, the material it put back
//!   pending). A drain node (a page-state node with no piece) follows
//!   each finisher: a page the put-back material fills finishes at the
//!   same moment.
//! - A page finishing inside a piece cuts it: the piece stops after
//!   candidate `k` ([`stop_arg`]) and defines its continuation
//!   ([`cont_field`]); after the finishers a [`Resume`] piece runs the
//!   rest, with its own page-state node.
//! - The driver ([`Driver`]) changes structure only, during the
//!   evaluation: the engine reports a page-state node's output when it is
//!   final (`Engine::mark`, `EditSource::settled`); between steps
//!   (`EditSource::absorb`) the driver places, moves or removes
//!   finishers and cuts, diffed against what stands, so node identities
//!   stay and unchanged readers never run again.
//!
//! Nothing reads the state after a whole piece, nothing is fed back to a
//! later round, nothing is replayed; the page chain is the document's own
//! (one addition per piece, one finisher per page), overlapped with the
//! typesetting.

use std::sync::{Arc, Mutex, PoisonError};

use phitex_defuse::{
    Ctx, EditSource, Engine, Field, FxHashMap, Locality, MayDefine, NodeId, OpId, Operation, Settled, Stop,
    ValueId, Values,
};

use crate::run::{R, Run};
use crate::val::list_of;

/// The page state's versions: defined by every page-state node and
/// finisher, read where it reaches.
pub(crate) const PS: &str = "ps";

/// Defined by a finisher that shipped its page: text after it reads it
/// (a read-time PDF write after a `\\shipout`).
pub(crate) const SHIPPED: &str = "pages:shipped";

/// The items a piece put on the outer vertical list, tagged with their
/// candidates, which its page-state node reads.
pub(crate) fn items_field(v: &Values, piece: NodeId) -> Field {
    v.field(&format!("pitems:{:x}", piece.raw()))
}

/// What a piece stopped at a candidate defines: its continuation (the
/// text still to run, relative to its own region, and the groups still
/// open: a stop can fall inside a macro), which the resume piece after it
/// reads.
pub(crate) fn cont_field(v: &Values, piece: NodeId) -> Field {
    v.field(&format!("cont:{:x}", piece.raw()))
}

/// The last argument of a piece that stops after its `k`-th candidate.
pub(crate) fn stop_arg(v: &Values, k: u32) -> ValueId {
    v.list(vec![v.str("stop"), v.int(i64::from(k))])
}

/// The candidate a piece stops after (its last argument, [`stop_arg`]).
pub(crate) fn stop_of(v: &Values, args: &[ValueId]) -> Option<u32> {
    let xs = list_of(v, *args.last()?);
    match xs[..] {
        [t, k] if v.as_str(t).as_deref() == Some("stop") => v.as_int(k).and_then(|k| u32::try_from(k).ok()),
        _ => None,
    }
}

fn node_int(v: &Values, n: NodeId) -> ValueId {
    v.int(i64::from_ne_bytes(n.raw().to_ne_bytes()))
}

fn int_node(v: &Values, x: ValueId) -> Option<NodeId> {
    v.as_int(x).map(|i| NodeId::from_raw(u64::from_ne_bytes(i.to_ne_bytes())))
}

/// A finish candidate: the piece that made it and its index among that
/// piece's candidates (in the piece's own text: a resume piece goes on
/// counting where the one before it stopped).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct Cand {
    pub piece: NodeId,
    pub k: u32,
}

/// One item on the outer vertical list.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Item {
    pub at: Cand,
    pub kind: ItemKind,
    /// The item itself, as the finisher's `\box255` gets it.
    pub node: ValueId,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum ItemKind {
    Box { height: i32, depth: i32 },
    Glue { width: i32, stretch: [i32; 4], shrink: i32 },
    Kern(i32),
    Penalty(i32),
    Insert { class: u16, height: i32, depth: i32 },
    Mark { class: u16, text: ValueId },
    /// `\pagegoal=` and its kin (§1245): `page_so_far[i] := x`.
    SetPage { i: u8, x: i32 },
    /// The page parameters as they stand when the item after arrives (§987).
    Params { vsize: i32, max_depth: i32 },
    Other,
}

fn decode_items(v: &Values, x: Option<ValueId>) -> Vec<Item> {
    let _ = (v, x);
    todo!("a piece's `pitems:` value: [[piece, k, item], ...] decoded to Items (crate::nodes)")
}

/// One insertion class on the current page (§981).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct InsState {
    pub class: u16,
    pub height: i32,
    /// Split on this page (§1010): later ones of the class wait.
    pub split: bool,
}

/// A version of the page builder's state (§980–§982).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct PageState {
    /// Pages finished before.
    pub page: i64,
    /// The current page's items so far.
    pub contents: Vec<Item>,
    /// Material a finisher put back, not on the page yet (§1017, §1023).
    pub pending: Vec<Item>,
    pub goal: i32,
    /// Height, the four stretch orders, shrink (§982).
    pub total: [i32; 6],
    pub depth: i32,
    pub max_depth: i32,
    pub inserts: Vec<InsState>,
    pub insert_penalties: i32,
    /// The best break so far: its index in `contents` and its cost.
    pub best: Option<(usize, i32)>,
}

impl PageState {
    fn decode(v: &Values, x: Option<ValueId>) -> PageState {
        let _ = (v, x);
        todo!("a `ps` version as a value: the fields above; None: the empty page of the preamble's parameters")
    }

    fn encode(&self, v: &Values) -> ValueId {
        let _ = v;
        todo!("the fields above as a value")
    }

    /// TeX's page builder over the pending material, then `items`, item
    /// by item (§980–§1010: the page's top discards, insertions charged
    /// or split by the room left, the cost of each break, the best kept),
    /// stopping where the page finishes: at the candidate whose item made
    /// the cost `awful_bad` or brought a penalty of -10000 or less.
    fn add(&mut self, items: &[Item]) -> Option<(Cand, Vec<Item>, Vec<Item>)> {
        let _ = items;
        todo!("§980–§1010; at a finish: the page's items up to the best break, and the rest put back")
    }
}

/// A page finishing, as a page-state node outputs it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Fin {
    pub at: Cand,
    /// At the last candidate of the piece the node follows (no cut).
    pub at_end: bool,
    /// The page's index.
    pub page: i64,
    /// The page's items, as the finisher's argument.
    pub content: ValueId,
    /// What goes back to the next page.
    pub carried: ValueId,
}

impl Fin {
    fn encode(&self, v: &Values) -> ValueId {
        v.list(vec![
            node_int(v, self.at.piece),
            v.int(i64::from(self.at.k)),
            v.int(i64::from(self.at_end)),
            v.int(self.page),
            self.content,
            self.carried,
        ])
    }

    fn decode(v: &Values, x: ValueId) -> Option<Fin> {
        let xs = list_of(v, x);
        let [p, k, e, j, content, carried] = xs[..] else {
            return None;
        };
        Some(Fin {
            at: Cand {
                piece: int_node(v, p)?,
                k: u32::try_from(v.as_int(k)?).ok()?,
            },
            at_end: v.as_int(e)? != 0,
            page: v.as_int(j)?,
            content,
            carried,
        })
    }

    /// The finisher's arguments.
    fn finisher_args(&self, v: &Values) -> Vec<ValueId> {
        vec![v.int(self.page), self.content, self.carried]
    }
}

/// What a page-state node follows: a piece (its items), or a finisher
/// at candidate `Cand` (no items: the material put back drains).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    Piece(NodeId),
    Drain(Cand),
}

fn source_args(v: &Values, s: Source) -> Vec<ValueId> {
    match s {
        Source::Piece(n) => vec![v.str("piece"), node_int(v, n)],
        Source::Drain(c) => vec![v.str("drain"), node_int(v, c.piece), v.int(i64::from(c.k))],
    }
}

fn source_of(v: &Values, args: &[ValueId]) -> Option<Source> {
    let kind = v.as_str(*args.first()?)?;
    let n = int_node(v, *args.get(1)?)?;
    match &*kind {
        "piece" => Some(Source::Piece(n)),
        "drain" => Some(Source::Drain(Cand {
            piece: n,
            k: u32::try_from(v.as_int(*args.get(2)?)?).ok()?,
        })),
        _ => None,
    }
}

/// The page builder in SSA form: the next version of [`PS`] from the one
/// reaching it and its piece's items; its output the [`Fin`] where a page
/// finishes (UNIT: none).
pub(crate) struct PageStateOp;

impl Operation for PageStateOp {
    fn name(&self) -> &'static str {
        "pagestate"
    }
    fn defs(&self, _: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(vec![(v.field(PS), Locality::Global)])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let args = cx.args();
        let Some(src) = source_of(v, &args) else {
            return Ok(ValueId::UNIT);
        };
        let ps = v.field(PS);
        let mut st = PageState::decode(v, cx.use_field(ps)?);
        let (items, last) = match src {
            Source::Piece(n) => {
                let items = decode_items(v, cx.use_field(items_field(v, n))?);
                let last = items.iter().map(|i| i.at.k).max();
                (items, last)
            }
            Source::Drain(_) => (Vec::new(), None),
        };
        let fin = st.add(&items).map(|(at, page, rest)| {
            let (at, at_end) = match src {
                Source::Drain(c) => (c, true),
                Source::Piece(_) => (at, Some(at.k) == last),
            };
            let enc = |xs: Vec<Item>| v.list(xs.into_iter().map(|i| i.node).collect());
            Fin {
                at,
                at_end,
                page: st.page,
                content: enc(page),
                carried: enc(rest),
            }
        });
        cx.define(ps, st.encode(v));
        Ok(fin.map_or(ValueId::UNIT, |f| f.encode(v)))
    }
}

/// A page's finisher: TeX's output routine for one page (§1012–§1025)
/// where the page finishes. Arguments `[j, content, carried]`. It reads
/// what reaches its place as any node does; it defines its `\global`
/// assignments (its local ones end with the routine's group), the marks
/// of its page for the text after it (`mark:top`, `mark:first`,
/// `mark:bot` and the classes'), `\outputpenalty`, `\deadcycles`,
/// [`SHIPPED`] if it shipped, and a fresh [`PS`]: page `j+1`'s start,
/// `carried` and what the routine put back pending. Its output is its
/// page.
pub(crate) struct Finish;

impl Operation for Finish {
    fn name(&self) -> &'static str {
        "finish"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        MayDefine::Any
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _ = cx;
        todo!(
            "\\box255, marks and insertion boxes from the content (§1012–§1022); the output \
             routine run in a group (§1025); its \\globals, the marks, \\outputpenalty, \
             \\deadcycles, pages:shipped and the fresh ps defined; the page value output"
        )
    }
}

/// The rest of a piece after it stopped: arguments `[from, stop?]`, the
/// node whose continuation ([`cont_field`]) it reads; it runs it, counting
/// candidates on from there, and stops in turn if it has a stop.
pub(crate) struct Resume;

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
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _ = cx;
        todo!("read cont:<from>, run its pending text in its open groups; define pitems:<self>; at a stop define cont:<self>")
    }
}

impl Run<'_, '_> {
    /// A finish candidate: TeX looks for a full page here (`build_page`
    /// at the outer level). The run counts it, tags the items it puts on
    /// the outer list from here with `[piece, k]`; at its stop it defines
    /// its continuation ([`cont_field`]) and its items, and ends.
    pub(crate) fn finish_candidate(&mut self) -> R<()> {
        todo!("k += 1; tag the next items [piece, k]; at the stop: define cont:<node> and pitems:<node>, end the run")
    }

    /// The items this piece put on the outer list, defined at its end
    /// ([`items_field`]) for its page-state node.
    pub(crate) fn define_piece_items(&mut self) {
        todo!("pitems:<node> = the tagged items, in order")
    }

    /// `\\shipout` (§1023, §638): in a finisher, its page value.
    pub(crate) fn ship_out(&mut self, b: ValueId) -> R<()> {
        let _ = b;
        todo!("the shipped box as the finisher's page value (no PDF in the skeleton)")
    }

    /// `\\pagegoal`, `\\pagetotal`, … (§421, §982): the version of the
    /// page state reaching this piece ([`PS`], def-use), with this piece's
    /// items up to here added.
    pub(crate) fn page_dimen<M>(&mut self, m: M) -> R<i32> {
        let reaching = self.get(PS)?;
        let _ = (m, reaching);
        todo!("PageState::decode(reaching), add this piece's items so far, the field m")
    }

    /// The output routine's group ends (§1100): in a finisher, its
    /// `\\global`s stand, its locals end, what it put back goes to its
    /// fresh page state.
    pub(crate) fn output_end<M>(&mut self, md: M) -> R<()> {
        let _ = md;
        todo!("§1100: \\box255 void checked, the put-back material pending in the fresh ps, the run ends")
    }

    /// The page a finisher finishes (its first argument).
    pub(crate) fn page_index(&self) -> usize {
        todo!("a finisher's argument j")
    }
}

/// The operations the driver places.
#[derive(Clone, Copy)]
pub(crate) struct PageOps {
    pub exec: OpId,
    pub finish: OpId,
    pub resume: OpId,
    pub pagestate: OpId,
}

/// The text pieces after an edit applied to the line, for the driver
/// ([`Driver::place`]); set by the text's edit source.
pub(crate) type PieceInbox = Arc<Mutex<Option<Vec<NodeId>>>>;

/// One finisher placed, and the drain node after it.
struct Placed {
    finisher: NodeId,
    drain: NodeId,
    args: Vec<ValueId>,
}

/// A run of a piece's text: the piece itself (`segs[0]`) or a resume
/// piece, its stop, its page-state node, the finishers after it.
struct Seg {
    node: NodeId,
    stop: Option<u32>,
    ps: NodeId,
    pages: Vec<Placed>,
}

/// A text piece and what stands after it.
struct Group {
    /// The piece's arguments, no stop.
    text: Vec<ValueId>,
    segs: Vec<Seg>,
}

#[derive(Clone, Copy)]
enum Slot {
    Ps,
    Drain(usize),
}

/// The structure the pages need, kept in step with the page-state nodes'
/// outputs as they become final.
pub(crate) struct Driver {
    ops: PageOps,
    groups: FxHashMap<NodeId, Group>,
    /// A page-state or drain node: its piece, segment and place.
    by_node: FxHashMap<NodeId, (NodeId, usize, Slot)>,
    /// Final outputs reported, not acted on yet.
    reports: Vec<(NodeId, ValueId)>,
    inbox: PieceInbox,
}

impl Driver {
    pub(crate) fn new(ops: PageOps, inbox: PieceInbox) -> Driver {
        Driver {
            ops,
            groups: FxHashMap::default(),
            by_node: FxHashMap::default(),
            reports: Vec::new(),
            inbox,
        }
    }

    /// Every text piece followed by its page-state node; the groups of
    /// pieces gone removed.
    pub(crate) fn sync_pieces(&mut self, eng: &mut Engine, pieces: &[NodeId]) {
        let live: phitex_defuse::FxHashSet<NodeId> = pieces.iter().copied().collect();
        let gone: Vec<NodeId> = self.groups.keys().filter(|n| !live.contains(n)).copied().collect();
        for p in gone {
            if let Some(g) = self.groups.remove(&p) {
                for (i, s) in g.segs.into_iter().enumerate() {
                    self.drop_seg(eng, s, i > 0);
                }
            }
        }
        for &p in pieces {
            if self.groups.contains_key(&p) {
                continue;
            }
            let text = eng.node_info(p).map(|i| i.args).unwrap_or_default();
            let ps = self.page_state_after(eng, p, Source::Piece(p));
            self.by_node.insert(ps, (p, 0, Slot::Ps));
            self.groups.insert(
                p,
                Group {
                    text,
                    segs: vec![Seg {
                        node: p,
                        stop: None,
                        ps,
                        pages: Vec::new(),
                    }],
                },
            );
        }
    }

    fn page_state_after(&self, eng: &mut Engine, after: NodeId, s: Source) -> NodeId {
        let args = source_args(eng.values(), s);
        let n = eng.insert_after(after, self.ops.pagestate, args);
        eng.mark(n);
        n
    }

    /// A segment's nodes removed (its own node too when `own`).
    fn drop_seg(&mut self, eng: &mut Engine, s: Seg, own: bool) {
        for p in s.pages {
            self.by_node.remove(&p.drain);
            eng.remove(p.drain);
            eng.remove(p.finisher);
        }
        self.by_node.remove(&s.ps);
        eng.remove(s.ps);
        if own {
            eng.remove(s.node);
        }
    }

    /// The final outputs of the page-state nodes among `items`, kept for
    /// [`Driver::place`] (the reports stay the engine's too).
    pub(crate) fn record(&mut self, items: &[Settled]) {
        for s in items {
            if self.by_node.contains_key(&s.node) {
                self.reports.push((s.node, s.output));
            }
        }
    }

    /// Between steps: the pieces an edit changed synced, then each report
    /// acted on.
    pub(crate) fn place(&mut self, eng: &mut Engine) {
        let pieces = self.inbox.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(pieces) = pieces {
            self.sync_pieces(eng, &pieces);
        }
        for (n, out) in std::mem::take(&mut self.reports) {
            let Some(&(piece, s, slot)) = self.by_node.get(&n) else {
                continue;
            };
            let fin = Fin::decode(eng.values(), out);
            match (slot, fin) {
                (Slot::Ps, None) => self.no_finish(eng, piece, s),
                (Slot::Ps, Some(f)) if !f.at_end => self.cut(eng, piece, s, f.at.k),
                (Slot::Ps, Some(f)) => self.set_page(eng, piece, s, 0, &f),
                (Slot::Drain(i), None) => self.truncate(eng, piece, s, i + 1),
                (Slot::Drain(i), Some(f)) => self.set_page(eng, piece, s, i + 1, &f),
            }
        }
    }

    /// The arguments of segment `s` of `g` with stop `stop`.
    fn seg_args(v: &Values, g: &Group, s: usize, stop: Option<u32>) -> Vec<ValueId> {
        let mut args = if s == 0 {
            g.text.clone()
        } else {
            vec![node_int(v, g.segs[s - 1].node)]
        };
        if let Some(k) = stop {
            args.push(stop_arg(v, k));
        }
        args
    }

    /// No page finishes in segment `s`: its finishers and the segments
    /// after it go, and it runs to the end of the text.
    fn no_finish(&mut self, eng: &mut Engine, piece: NodeId, s: usize) {
        self.truncate(eng, piece, s, 0);
        let Some(mut g) = self.groups.remove(&piece) else { return };
        for later in g.segs.drain(s + 1..).collect::<Vec<_>>() {
            self.drop_seg(eng, later, true);
        }
        if g.segs[s].stop.is_some() {
            g.segs[s].stop = None;
            let op = if s == 0 { self.ops.exec } else { self.ops.resume };
            let args = Self::seg_args(eng.values(), &g, s, None);
            eng.replace(g.segs[s].node, op, args);
        }
        self.groups.insert(piece, g);
    }

    /// Segment `s` stops after candidate `k`: a resume segment after it
    /// takes the rest (and the stop it had).
    fn cut(&mut self, eng: &mut Engine, piece: NodeId, s: usize, k: u32) {
        self.truncate(eng, piece, s, 0);
        let Some(mut g) = self.groups.remove(&piece) else { return };
        let old_stop = g.segs[s].stop;
        g.segs[s].stop = Some(k);
        let op = if s == 0 { self.ops.exec } else { self.ops.resume };
        let args = Self::seg_args(eng.values(), &g, s, Some(k));
        eng.replace(g.segs[s].node, op, args);
        let v = eng.values();
        let mut rargs = vec![node_int(v, g.segs[s].node)];
        if let Some(k2) = old_stop {
            rargs.push(stop_arg(v, k2));
        }
        let r = eng.insert_after(g.segs[s].ps, self.ops.resume, rargs);
        let ps = self.page_state_after(eng, r, Source::Piece(r));
        g.segs.insert(
            s + 1,
            Seg {
                node: r,
                stop: old_stop,
                ps,
                pages: Vec::new(),
            },
        );
        // (the segment after it reads the new one's continuation now)
        if let Some(next) = g.segs.get(s + 2) {
            let args = Self::seg_args(eng.values(), &g, s + 2, next.stop);
            eng.replace(next.node, self.ops.resume, args);
        }
        self.reindex(&g, piece);
        self.groups.insert(piece, g);
    }

    /// Page `i` after segment `s` is `f`: its finisher's arguments
    /// replaced if they differ, or the finisher and its drain placed.
    fn set_page(&mut self, eng: &mut Engine, piece: NodeId, s: usize, i: usize, f: &Fin) {
        let args = f.finisher_args(eng.values());
        let Some(g) = self.groups.get_mut(&piece) else { return };
        let seg = &mut g.segs[s];
        if let Some(p) = seg.pages.get_mut(i) {
            if p.args != args {
                eng.replace(p.finisher, self.ops.finish, args.clone());
                p.args = args;
            }
            return;
        }
        let after = seg.pages.last().map_or(seg.ps, |p| p.drain);
        let fin = eng.insert_after(after, self.ops.finish, args.clone());
        let src = source_args(eng.values(), Source::Drain(f.at));
        let drain = eng.insert_after(fin, self.ops.pagestate, src);
        eng.mark(drain);
        seg.pages.push(Placed {
            finisher: fin,
            drain,
            args,
        });
        self.by_node.insert(drain, (piece, s, Slot::Drain(i)));
    }

    /// Segment `s` keeps its first `n` pages.
    fn truncate(&mut self, eng: &mut Engine, piece: NodeId, s: usize, n: usize) {
        let Some(g) = self.groups.get_mut(&piece) else { return };
        let seg = &mut g.segs[s];
        if seg.pages.len() <= n {
            return;
        }
        for p in seg.pages.drain(n..) {
            self.by_node.remove(&p.drain);
            eng.remove(p.drain);
            eng.remove(p.finisher);
        }
    }

    /// The page-state and drain nodes of `g` indexed by their segment.
    fn reindex(&mut self, g: &Group, piece: NodeId) {
        for (s, seg) in g.segs.iter().enumerate() {
            self.by_node.insert(seg.ps, (piece, s, Slot::Ps));
            for (i, p) in seg.pages.iter().enumerate() {
                self.by_node.insert(p.drain, (piece, s, Slot::Drain(i)));
            }
        }
    }
}

/// The evaluation's edit source with the pages' driver: the text's edits
/// first, then the structure the pages need; the final outputs of the
/// page-state nodes noted on the way.
pub(crate) struct WithPages<'a> {
    pub inner: &'a mut dyn EditSource,
    pub pages: &'a mut Driver,
}

impl EditSource for WithPages<'_> {
    fn absorb(&mut self, eng: &mut Engine) -> Option<u64> {
        let version = self.inner.absorb(eng);
        self.pages.place(eng);
        version
    }

    fn settled(&mut self, eng: &Engine, items: &[Settled]) -> bool {
        self.pages.record(items);
        self.inner.settled(eng, items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An operation that runs nothing (the test drives the structure
    /// only).
    struct Nop(&'static str);

    impl Operation for Nop {
        fn name(&self) -> &'static str {
            self.0
        }
        fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
            Some(Vec::new())
        }
        fn run(&self, _: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            Ok(ValueId::UNIT)
        }
    }

    /// Three pieces:
    ///
    /// ```tex
    /// Paragraph A ... (fills page 1 nearly to the bottom)     % piece a
    ///
    /// \newpage Paragraph B: we are on page \thepage.          % piece b
    ///
    /// Paragraph C: this is page \thepage.                     % piece c
    /// \end{document}
    /// ```
    ///
    /// Page 0 finishes at b's `\newpage`, candidate 0 of 2: b is cut
    /// there (b stops at 0, a resume piece takes the rest), the finisher
    /// stands between, so B's `\thepage` runs after it. Page 1 finishes
    /// at the end of the job, c's last candidate: its finisher follows c.
    #[test]
    fn three_paragraphs_structure() {
        let mut eng = Engine::new();
        let exec = eng.register(Arc::new(Nop("exec")));
        let ops = PageOps {
            exec,
            finish: eng.register(Arc::new(Nop("finish"))),
            resume: eng.register(Arc::new(Nop("resume"))),
            pagestate: eng.register(Arc::new(Nop("pagestate"))),
        };
        let unit = eng.new_unit();
        let v = eng.values();
        let (ta, tb, tc) = (v.str("A"), v.str("B"), v.str("C"));
        let a = eng.push(unit, exec, vec![ta]);
        let b = eng.push(unit, exec, vec![tb]);
        let c = eng.push(unit, exec, vec![tc]);
        let mut d = Driver::new(ops, Arc::default());
        d.sync_pieces(&mut eng, &[a, b, c]);
        let ps = |d: &Driver, p: NodeId, s: usize| d.groups[&p].segs[s].ps;
        let none = ValueId::UNIT;
        let fin = |eng: &Engine, piece: NodeId, k: u32, at_end: bool, page: i64| {
            let v = eng.values();
            Fin {
                at: Cand { piece, k },
                at_end,
                page,
                content: v.list(Vec::new()),
                carried: v.list(Vec::new()),
            }
            .encode(v)
        };
        // a: no page finishes; b: page 0 finishes at candidate 0, not its last
        let (pa, pb) = (ps(&d, a, 0), ps(&d, b, 0));
        let fb = fin(&eng, b, 0, false, 0);
        d.reports = vec![(pa, none), (pb, fb)];
        d.place(&mut eng);
        let g = &d.groups[&b];
        assert_eq!(g.segs.len(), 2, "b cut in two");
        assert_eq!(g.segs[0].stop, Some(0));
        assert_eq!(stop_of(eng.values(), &eng.node_info(b).expect("b").args), Some(0));
        // b, stopped, now ends at candidate 0: the finisher follows it
        let fb = fin(&eng, b, 0, true, 0);
        d.reports = vec![(pb, fb)];
        d.place(&mut eng);
        let g = &d.groups[&b];
        assert_eq!(g.segs[0].pages.len(), 1, "page 0's finisher after b's first half");
        // its drain: nothing more finishes there; the rest of b, none
        let (drain, rest) = (g.segs[0].pages[0].drain, g.segs[1].ps);
        d.reports = vec![(drain, none), (rest, none)];
        d.place(&mut eng);
        // c: page 1 at its last candidate, no cut
        let pc = ps(&d, c, 0);
        let fc = fin(&eng, c, 2, true, 1);
        d.reports = vec![(pc, fc)];
        d.place(&mut eng);
        let g = &d.groups[&c];
        assert_eq!(g.segs.len(), 1, "c not cut");
        assert_eq!(g.segs[0].pages.len(), 1, "page 1's finisher after c");
        // an edit moves page 1's content: the same finisher, new arguments
        let f1 = g.segs[0].pages[0].finisher;
        let fc2 = {
            let v = eng.values();
            Fin {
                at: Cand { piece: c, k: 2 },
                at_end: true,
                page: 1,
                content: v.list(vec![v.int(1)]),
                carried: v.list(Vec::new()),
            }
            .encode(v)
        };
        d.reports = vec![(pc, fc2)];
        d.place(&mut eng);
        assert_eq!(d.groups[&c].segs[0].pages[0].finisher, f1, "the finisher kept its identity");
    }
}
