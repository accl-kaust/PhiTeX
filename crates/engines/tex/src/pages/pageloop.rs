//! Layer 4: the page builder as **one node** over the whole text
//! (docs/spec-layers.md §2, §4–§5; the rules in `SPEC.md` §2).
//!
//! [`Pages`] is pushed once, after the mark where the text ends. Its run
//! folds the pieces' profiles ([`crate::vprof::VPROF`]) in text order,
//! one contributor at a time, running TeX's page builder (§980–§1010)
//! over them, microseconds a paragraph. Where a page fires it notes the
//! page ([`Page`]: where every item on it came from and the builder's
//! decisions), defines the page as a field of its own
//! ([`PAGE_OF`]`:<piece>:<nth>`, read by the page's finisher), and places
//! the finisher right after the piece the page fired in. At the text's
//! end it defines [`super::PAGES_END`].
//!
//! **Checkpoints, not a chain.** The loop's state after every fire is a
//! [`Checkpoint`]; the run keeps them all in its [`LoopState`] and stores
//! that with its record ([`phitex_defuse::Ctx::park`]). A run that meets
//! a contributor not committed yet (a hole) parks there with the fold
//! past the last checkpoint (its tail, the page half built): what it
//! fired stands, and the run when the hole fills goes on from the tail.
//! A run asked for because a contribution moved
//! ([`phitex_defuse::Cause::Range`], its site) goes back to the last
//! checkpoint whose cursor stands before the site, and stops early once
//! a page it fires again agrees with the one it had (same [`Page`] value
//! and state at the same place, past the site): the pages after are
//! unchanged, their finishers re-asked by nothing. Every run re-emits
//! every page's definition and placement it has (the ids are reused from
//! the checkpoints), so the engine keeps the finishers whose page did
//! not move, and reads every give-back again: one that moved
//! ([`phitex_defuse::Ctx::placed_output_or`]) sends the fold back to its
//! page's checkpoint.
//!
//! **What this node never does.** It runs no TeX text, reads no page
//! parameter but through the markers the pieces put on their profiles
//! and the records at the pieces ([`Run::page_params_at`]), reads no
//! finisher's state but its give-back through the placement
//! ([`phitex_defuse::Ctx::placed_output_or`]), and waits for nothing.
//!
//! Identity: one node, arguments `[]`; a page's key is `(piece, nth)`,
//! the piece it fired in and its place among the pages fired in that
//! piece. State is in definitions and the parked [`LoopState`]; nothing
//! of it is an argument of any node.

use phitex_defuse::{Cause, Ctx, Effect, MayDefine, NodeId, Operation, Stop, ValueId, Values};

use super::*;
use crate::vprof::{PItem, ScanState};

/// The page of a fired page, as its finisher reads it:
/// `pages:page:<piece raw id>:<nth>`, defined by the [`Pages`] node,
/// read `At::End(unit)` (the loop node is the last of the unit that
/// defines it).
pub(crate) const PAGE_OF: &str = "pages:page";

/// The loop's state after a fire: the page it fired, where, and
/// everything the fold after it needs, so a run resumes here without
/// folding what came before.
#[derive(Clone, Debug)]
pub(crate) struct Checkpoint {
    /// The page this checkpoint ends: its key and its content (the value
    /// defined as [`PAGE_OF`]; reused unchanged when the page did not
    /// move).
    pub key: PageKey,
    pub page: ValueId,
    /// Its finisher's arguments ([`FinishArgs`]), as one list.
    pub args: ValueId,
    /// The placement index of the page's finisher in this run's order
    /// (the pages in fire order: checkpoint `i` is placement `i`).
    pub placement: usize,
    /// The contributor the page fired in (`next_contribution`'s cursor
    /// there): its units after the fire are the next page's first input.
    pub cursor: NodeId,
    /// The builder after the fire, before §1026's give-back is folded,
    /// with the loop's carry and the fire ([`encode_state`]): what the
    /// next page begins with once the finisher's output (or its guess)
    /// is folded on ([`Run::after_fire`]).
    pub state: ValueId,
    /// What the routine is guessed to give back, and what the fold took
    /// (the finisher's output where it has run, else the guess).
    pub guess: ValueId,
    pub gave: ValueId,
}

/// The parked state of the loop: its checkpoints in fire order and where
/// the fold stands after the last one. Encoded as one value
/// ([`LoopState::encode`]) for `Ctx::park`; the page values inside it are
/// the ids defined as [`PAGE_OF`].
#[derive(Clone, Default, Debug)]
pub(crate) struct LoopState {
    pub checkpoints: Vec<Checkpoint>,
    /// The fold after the last checkpoint, up to the hole
    /// ([`encode_tail`]; `None`: the text ended).
    pub tail: Option<ValueId>,
    /// `\end` ended the job (§1054): the pages are complete.
    pub ended: bool,
}

/// The tag of a parked [`LoopState`] of this version.
const STATE_TAG: &str = "\u{1}pages-loop-1";

impl LoopState {
    pub(crate) fn encode(&self, v: &Values) -> ValueId {
        let cps = self
            .checkpoints
            .iter()
            .map(|c| {
                v.list(vec![
                    raw_value(v, Some(c.key.piece)),
                    v.int(i64::from(c.key.nth)),
                    c.page,
                    c.args,
                    raw_value(v, Some(c.cursor)),
                    c.state,
                    c.guess,
                    c.gave,
                ])
            })
            .collect();
        v.list(vec![
            v.str(STATE_TAG),
            v.list(cps),
            self.tail.unwrap_or(ValueId::UNIT),
            v.int(i64::from(self.ended)),
        ])
    }

    /// The inverse; `None` on a shape this version does not know (then
    /// the run starts from nothing).
    pub(crate) fn decode(v: &Values, x: ValueId) -> Option<LoopState> {
        let xs = list_of(v, x);
        if xs.len() != 4 || !v.with_str(xs[0], |s| s == STATE_TAG).unwrap_or(false) {
            return None;
        }
        let mut checkpoints = Vec::new();
        for (i, c) in list_of(v, xs[1]).into_iter().enumerate() {
            let cs = list_of(v, c);
            if cs.len() != 8 {
                return None;
            }
            checkpoints.push(Checkpoint {
                key: PageKey {
                    piece: raw_node(v, cs[0])?,
                    nth: u32::try_from(int_of(v, cs[1])).ok()?,
                },
                page: cs[2],
                args: cs[3],
                placement: i,
                cursor: raw_node(v, cs[4])?,
                state: cs[5],
                guess: cs[6],
                gave: cs[7],
            });
        }
        Some(LoopState {
            checkpoints,
            tail: some(xs[2]),
            ended: int_of(v, xs[3]) != 0,
        })
    }

    /// The last checkpoint whose cursor stands before `site` (the node a
    /// moved contribution is at): the run goes back to it. `None`: before
    /// the first page. A checkpoint's own cursor contributor counts as
    /// before only if `site` is a later node (its units are in the state).
    pub(crate) fn before(&self, cx: &Ctx<'_>, site: NodeId) -> Option<usize> {
        self.checkpoints
            .iter()
            .take_while(|c| c.cursor != site && cx.before(c.cursor, site))
            .count()
            .checked_sub(1)
    }
}

/// A node's raw id as an integer value (-1 for none).
pub(crate) fn raw_value(v: &Values, n: Option<NodeId>) -> ValueId {
    v.int(n.map_or(-1, |n| i64::from_ne_bytes(n.raw().to_ne_bytes())))
}

/// [`raw_value`]'s node.
pub(crate) fn raw_node(v: &Values, x: ValueId) -> Option<NodeId> {
    let i = int_of(v, x);
    (i != -1).then(|| node_of(i))
}

/// The tag of a kick's arguments ([`kick_args`]).
const KICK: &str = "\u{1}kick";

/// The arguments of the node that places the page builder's node after
/// `mark` (where the text ends): `[tag, mark]`. Pushed where the text
/// begins, it runs with the first block, so the loop starts beside the
/// text instead of after it.
pub(crate) fn kick_args(v: &Values, mark: NodeId) -> Vec<ValueId> {
    vec![v.str(KICK), raw_value(v, Some(mark))]
}

/// The mark a kick's arguments name.
pub(crate) fn kick_of(v: &Values, args: &[ValueId]) -> Option<NodeId> {
    match args {
        [t, m] if v.with_str(*t, |s| s == KICK).unwrap_or(false) => raw_node(v, *m),
        _ => None,
    }
}

/// The node that places the page builder ([`Pages`]) after the text's
/// end mark, from where the text begins (arguments [`kick_args`]), and
/// ends: its own operation, so the builder is the only `pages` node.
pub(crate) struct Kick(pub Shared);

impl Operation for Kick {
    fn name(&self) -> &'static str {
        "kick"
    }

    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        MayDefine::Nothing
    }

    /// Set among the text's pieces, not one of them: a claim passes over it.
    fn sibling(&self) -> bool {
        false
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        if let Some(at) = kick_of(cx.values(), cx.args()) {
            cx.follow_after(at, ops(&self.0).pages, Vec::new());
        }
        Ok(ValueId::UNIT)
    }
}

/// The one page-builder node. Arguments: none. Pushed after the text's
/// end mark, by the [`Kick`] where there is more than one thread.
pub(crate) struct Pages(pub Shared);

impl Operation for Pages {
    /// Layer 4 (docs/spec-layers.md §2): the page builder, once every piece and its lines are quiet: it meets no hole.
    fn layer(&self) -> u8 {
        4
    }
    fn name(&self) -> &'static str {
        "pages"
    }

    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        // (the page fields it defines are named per page; the engine
        // takes the family by its prefix)
        MayDefine::Prefixes(vec![PAGES_END.to_owned(), PAGE_OF.to_owned()])
    }

    fn sibling(&self) -> bool {
        false
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }

    fn contributes(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        !from_pieces(v, f)
    }

    /// The loop (`SPEC.md` §2.1–§2.2), from the right checkpoint
    /// ([`Run::page_loop`]).
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let mut r = Run::new(cx, ops(&self.0));
        r.page_loop()
    }
}

/// A fire a unit made: the fire, its page, the unit's insertion classes.
type UnitFire = (FireNote, Page, Vec<(u16, InsertClass)>);

/// Where a run's fold starts.
enum Start {
    /// The job's start: no page fired.
    Scratch,
    /// The fold past the last checkpoint kept, at the hole it stopped at.
    Tail(ValueId),
    /// Checkpoint `i` kept: its page's give-back folded on, then on.
    At(usize),
    /// Every checkpoint kept and the job ended there.
    Done,
}

/// Where the run may stop early on a page it fires again as it was: past
/// the change that asked for it.
#[derive(Clone, Copy)]
enum Bound {
    /// Nowhere: what moved is not known.
    Never,
    /// At a page fired after this contributor.
    Site(NodeId),
    /// At a page after this one (its give-back moved).
    Page(usize),
}

impl Run<'_, '_> {
    /// [`Pages::run`]: the loop from the right checkpoint.
    ///
    /// 1. `state` = the parked [`LoopState`] (`Ctx::resumed`), else empty.
    /// 2. Where to start: on a moved contribution or definition
    ///    (`Cause::Range { by }`, `Cause::Changed { site }`) go back to
    ///    the checkpoint before it ([`LoopState::before`]), or on from the
    ///    tail where the site is past it (a hole that filled); every
    ///    kept page's give-back is read again (`Ctx::placed_output_or`),
    ///    and one that moved sends the fold back to its page's
    ///    checkpoint; else from the start.
    /// 3. Fold contributors (`next_contribution(VPROF, cursor)`), each
    ///    unit's items with the builder ([`Run::fold_unit`]); at a fire
    ///    make the [`Page`] ([`Run::fire_at`]), define it, place its
    ///    finisher, push the checkpoint, and fold §1026's give-back
    ///    ([`Run::after_fire`]). **Early stop:** a re-fired page whose
    ///    [`Page`] value and state equal the old checkpoint's ends the
    ///    re-fold: the old checkpoints after it stand as they were.
    /// 4. A hole: keep the tail, park.
    /// 5. `\end` (§1054) with page and contributions empty and
    ///    `\deadcycles` 0: `ended`; define [`PAGES_END`] = pages.
    /// 6. Every run defines every checkpoint's page as
    ///    `PAGE_OF:<piece>:<nth>` (global), places every finisher in fire
    ///    order (placement index = checkpoint index), parks the state.
    pub(crate) fn page_loop(&mut self) -> Result<ValueId, Stop> {
        crate::prof::pages::attempt();
        let t = crate::prof::pages::start();
        let res = self.page_loop_r();
        crate::prof::pages::add(crate::prof::pages::ATTEMPT, t);
        match res {
            Ok(x) => Ok(x),
            Err(Halt::Stop(s)) => Err(s),
            Err(_) => unreachable!("the page loop runs no command"),
        }
    }

    fn page_loop_r(&mut self) -> R<ValueId> {
        let v = self.v;
        // 1. the parked state
        let old = self.cx.resumed().and_then(|x| LoopState::decode(v, x)).unwrap_or_default();
        // 2. where to start
        let me = self.cx.node();
        let site = match self.cx.woken_by() {
            Some(Cause::Range { by, .. } | Cause::Changed { site: Some(by), .. }) => Some(by),
            Some(Cause::Rebind { from, to, .. }) => from.or(to),
            // (a give-back: every one is read again below)
            Some(Cause::Output { .. }) => Some(me),
            _ => None,
        };
        let n_old = old.checkpoints.len();
        let (limit, mut bound) = match site {
            None => (0, Bound::Never),
            Some(s) if s == me => (n_old, Bound::Never),
            Some(s) => (old.before(self.cx, s).map_or(0, |i| i + 1), Bound::Site(s)),
        };
        // (the tail stands where every checkpoint does and the site is
        // past the contributor it folded last)
        let tail_ok = match (site, old.tail) {
            (Some(s), Some(t)) if limit == n_old => {
                let c = tail_cursor(v, t);
                (s == me || c.is_none_or(|c| c != s && self.cx.before(c, s))).then_some(t)
            }
            _ => None,
        };
        let mut ls = LoopState::default();
        let mut start = Start::Scratch;
        // (the kept checkpoints emitted again, their give-backs read
        // again: one that moved is where the fold goes on from)
        for i in 0..limit {
            let gave = self.emit_kept(&mut ls, &old.checkpoints[i]);
            if gave != old.checkpoints[i].gave {
                start = Start::At(i);
                bound = Bound::Page(i);
                break;
            }
        }
        if matches!(start, Start::Scratch) {
            start = match tail_ok {
                Some(t) => Start::Tail(t),
                // (every page kept, their give-backs as they were, the
                // job ended: nothing to fold)
                None if limit == n_old && limit > 0 && old.tail.is_none() && old.ended => Start::Done,
                None if limit > 0 => Start::At(limit - 1),
                None => Start::Scratch,
            };
        }
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            let how = match start {
                Start::Scratch => "scratch".to_owned(),
                Start::Tail(_) => "tail".to_owned(),
                Start::At(i) => format!("checkpoint {i}"),
                Start::Done => "nowhere (kept whole)".to_owned(),
            };
            eprintln!(
                "pages {}: run from {how} ({} checkpoints kept of {n_old}, woken by {:?})",
                me.raw(),
                ls.checkpoints.len(),
                self.cx.woken_by()
            );
        }
        // 3. the fold
        let (mut st, mut lc, mut pending) = match start {
            Start::Scratch => (fresh_scan(), LoopCarry::new(), None),
            Start::Tail(t) => {
                let (st, lc) = self.decode_tail(t);
                (st, lc, None)
            }
            Start::At(i) => {
                let (st, lc, p) = self.decode_state(ls.checkpoints[i].state);
                (st, lc, Some((i, p)))
            }
            Start::Done => {
                let mut lc = LoopCarry::new();
                lc.ended = true;
                (fresh_scan(), lc, None)
            }
        };
        let hole = if lc.ended { false } else { self.fold_on(&mut st, &mut lc, &mut pending, &mut ls, &old, bound)? };
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            eprintln!(
                "pages {}: run ends {} ({} checkpoints, cursor {:?}, stopped at {:?})",
                me.raw(),
                if hole { "at a hole" } else if lc.ended { "with the job" } else { "neither ended nor at a hole" },
                ls.checkpoints.len(),
                lc.cur.map(NodeId::raw),
                self.cx.stopped_at().map(NodeId::raw)
            );
        }
        // 4. a hole: the tail
        if hole {
            ls.tail = Some(encode_tail(v, &st, &lc));
        }
        ls.ended = lc.ended;
        // 5. the pages' end
        if ls.ended {
            let n = i64::try_from(ls.checkpoints.len()).unwrap_or(0);
            self.cx.define_with(v.field(PAGES_END), v.int(n), Locality::Global);
        }
        if cfg!(feature = "observe") {
            let mut keys: Vec<(u64, u32)> = ls.checkpoints.iter().map(|c| (c.key.piece.raw(), c.key.nth)).collect();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), ls.checkpoints.len(), "page keys are unique");
            assert!(ls.checkpoints.iter().enumerate().all(|(i, c)| c.placement == i), "checkpoint i is placement i");
        }
        // 6. parked
        self.cx.park(ls.encode(v));
        Ok(ValueId::UNIT)
    }

    /// A checkpoint kept: its page defined and its finisher placed again,
    /// its give-back read again ([`Run::give_back`]); pushed. The
    /// give-back read.
    fn emit_kept(&mut self, ls: &mut LoopState, c: &Checkpoint) -> ValueId {
        let i = ls.checkpoints.len();
        let prev = ls.checkpoints.last().map(|p| p.key);
        self.emit_page(i, c.key, c.page, c.args, prev);
        let gave = self.give_back(i, c.guess);
        ls.checkpoints.push(Checkpoint {
            placement: i,
            gave,
            ..c.clone()
        });
        gave
    }

    /// Page `key` (checkpoint `i`) defined as [`PAGE_OF`]`:<piece>:<nth>`
    /// and its finisher placed: right after the piece the page fired in,
    /// or after the finisher of the page before where that one fired in
    /// the same piece (its routine runs before this one's, both before
    /// the piece's rest).
    fn emit_page(&mut self, i: usize, key: PageKey, x: ValueId, args: ValueId, prev: Option<PageKey>) {
        let v = self.v;
        self.cx.define_with(key.field(v, PAGE_OF), x, Locality::Global);
        let args = list_of(v, args);
        let at = match prev {
            Some(p) if p.piece == key.piece && i > 0 => self.cx.follow_after_placed(i - 1, self.ops.finish, args),
            _ => self.cx.follow_after(key.piece, self.ops.finish, args),
        };
        debug_assert_eq!(at, i, "checkpoint i is placement i");
    }

    /// §1026's give-back of page `i`'s routine as the loop folds it: the
    /// finisher's output where it has run, else the guess (`SPEC.md`
    /// §2.2 "what the routine gives back"); a stale finisher's nothing
    /// (unit) the guess too.
    pub(crate) fn give_back(&mut self, i: usize, guess: ValueId) -> ValueId {
        match self.cx.placed_output_or(i, guess) {
            ValueId::UNIT => guess,
            x => x,
        }
    }

    /// The fold from where `st`, `lc` and `pending` stand (step 3), each
    /// page fired pushed on `ls`, to a hole (`true`) or the end. A page
    /// fired again as `old` had it past `bound` takes the old checkpoints
    /// after it, and the fold goes on from the first whose give-back
    /// moved, or from the old fold's tail.
    #[allow(clippy::too_many_lines, reason = "the fold's one loop")]
    fn fold_on(
        &mut self,
        st: &mut ScanState,
        lc: &mut LoopCarry,
        pending: &mut Option<(usize, FireNote)>,
        ls: &mut LoopState,
        old: &LoopState,
        mut bound: Bound,
    ) -> R<bool> {
        let v = self.v;
        let vprof = v.field(crate::vprof::VPROF);
        // (the units of the contributor being folded)
        let mut cur_xs: Option<(NodeId, Vec<ValueId>)> = None;
        'fold: loop {
            // (a page fired at the last step: the old ones after it, if
            // it is the old one as it was)
            if let Some((i, _)) = pending.as_ref()
                && self.agrees(&ls.checkpoints[*i], old, bound)
            {
                let mut moved = None;
                for j in *i + 1..old.checkpoints.len() {
                    let gave = self.emit_kept(ls, &old.checkpoints[j]);
                    if gave != old.checkpoints[j].gave {
                        moved = Some(j);
                        break;
                    }
                }
                if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
                    eprintln!("pages {}: page {} fired again as it was; on from {moved:?}", self.cx.node().raw(), *i + 1);
                }
                cur_xs = None;
                if let Some(j) = moved {
                    let (s, c, p) = self.decode_state(ls.checkpoints[j].state);
                    (*st, *lc) = (s, c);
                    *pending = Some((j, p));
                    bound = Bound::Page(j);
                } else {
                    *pending = None;
                    bound = Bound::Never;
                    let Some(t) = old.tail else {
                        lc.ended = old.ended;
                        return Ok(false);
                    };
                    (*st, *lc) = self.decode_tail(t);
                }
            }
            if let Some((i, p)) = pending.take() {
                let gave = ls.checkpoints[i].gave;
                if let Some(f) = self.after_fire(st, lc, &p, gave)? {
                    // (a page fired in the material, §1026's build)
                    let (mut q, page) = self.fire_at(st, lc, p.node, p.pid, &f, true)?;
                    q.xs_i = p.xs_i;
                    q.classes.clone_from(&p.classes);
                    *pending = Some(self.checkpoint(st, lc, q, &page, ls));
                    continue;
                }
                // (the rest of the contributor the page fired in)
                let xs = if let Some(c) = cur_xs.take().filter(|c| c.0 == p.node) {
                    c.1
                } else {
                    let t = crate::prof::pages::start();
                    let next = self.cx.next_contribution(vprof, lc.cur)?;
                    crate::prof::pages::add(crate::prof::pages::NEXT, t);
                    // (not where it was: the change that moved it runs
                    // this node again, from before it)
                    let Some((_, u)) = next.filter(|n| n.0 == p.node) else {
                        return Ok(true);
                    };
                    v.seq_to_vec(u)
                };
                cur_xs = Some((p.node, xs.clone()));
                let (ui, from) = lc.frontier;
                if let Some((mut q, page)) = self.fold_items(st, lc, p.node, p.pid, ui, from as usize, &p.classes)? {
                    q.classes.clone_from(&p.classes);
                    q.xs_i = p.xs_i;
                    *pending = Some(self.checkpoint(st, lc, q, &page, ls));
                    continue;
                }
                for (i, &x) in xs.iter().enumerate().skip(p.xs_i + 1) {
                    if let Some((mut q, page, classes)) = self.fold_unit(st, lc, p.node, x)? {
                        q.classes = classes;
                        q.xs_i = i;
                        *pending = Some(self.checkpoint(st, lc, q, &page, ls));
                        continue 'fold;
                    }
                }
                lc.cur = Some(p.node);
                if !xs.is_empty() {
                    lc.cur_units = Some(p.node);
                }
            }
            if lc.ended {
                return Ok(false);
            }
            let t = crate::prof::pages::start();
            let next = self.cx.next_contribution(vprof, lc.cur)?;
            crate::prof::pages::add(crate::prof::pages::NEXT, t);
            let Some((node, u)) = next else {
                // (a contributor not in yet: parked at it, the pages fired
                // so far placed; the run when it comes in goes on from
                // here)
                return Ok(self.cx.stopped_at().is_some());
            };
            let xs = v.seq_to_vec(u);
            cur_xs = Some((node, xs.clone()));
            for (i, &x) in xs.iter().enumerate() {
                if let Some((mut q, page, classes)) = self.fold_unit(st, lc, node, x)? {
                    q.classes = classes;
                    q.xs_i = i;
                    *pending = Some(self.checkpoint(st, lc, q, &page, ls));
                    continue 'fold;
                }
            }
            lc.cur = Some(node);
            if !xs.is_empty() {
                lc.cur_units = Some(node);
            }
        }
    }

    /// Whether checkpoint `c`, just pushed, is the old fold's at its
    /// index as it was (its page, its state, its give-back), past what
    /// moved: the early stop.
    fn agrees(&self, c: &Checkpoint, old: &LoopState, bound: Bound) -> bool {
        let past = match bound {
            Bound::Never => false,
            // (strictly after the site: a page fired in the site's own
            // contributor rests on what moved there, or after it where
            // the next contribution after it moved, which the run reads
            // again only past that page)
            Bound::Site(s) => c.cursor != s && !self.cx.before(c.cursor, s),
            Bound::Page(j) => c.placement > j,
        };
        past && old.checkpoints.get(c.placement).is_some_and(|o| {
            o.key == c.key && o.page == c.page && o.state == c.state && o.gave == c.gave
        })
    }

    /// A page fired (`q`, its [`Page`] `page`): defined, its finisher
    /// placed, its checkpoint pushed and its give-back read; the fire to
    /// fold on from.
    fn checkpoint(&mut self, st: &ScanState, lc: &LoopCarry, q: FireNote, page: &Page, ls: &mut LoopState) -> (usize, FireNote) {
        let v = self.v;
        let x = page.encode(v);
        let i = ls.checkpoints.len();
        let prev = ls.checkpoints.last().map(|p| p.key);
        let args = v.list(FinishArgs { key: page.key }.encode(v));
        // (what its finisher predicts by: the routine and the page's
        // class, the page's, not its placement's)
        self.ops.fin_class.locked().insert(
            (i64::from_ne_bytes(page.key.piece.raw().to_ne_bytes()), i64::from(page.key.nth)),
            (page.hint, pen_class(page.output_penalty)),
        );
        self.emit_page(i, page.key, x, args, prev);
        let gave = self.give_back(i, q.rtn_guess);
        ls.checkpoints.push(Checkpoint {
            key: page.key,
            page: x,
            args,
            placement: i,
            cursor: q.node,
            state: encode_state(v, st, lc, &q),
            guess: q.rtn_guess,
            gave,
        });
        (i, q)
    }
}

/// The tail's cursor: the contributor its fold took last.
fn tail_cursor(v: &Values, t: ValueId) -> Option<NodeId> {
    list_of(v, t).first().and_then(|&x| raw_node(v, x))
}

impl Run<'_, '_> {
    /// A contributor's unit `x` decoded and folded from its first item
    /// (its units' doc in [`Run::fold_on`]); the fire it made, with
    /// its insertion classes, if any.
    pub(crate) fn fold_unit(
        &mut self,
        st: &mut ScanState,
        lc: &mut LoopCarry,
        node: phitex_defuse::NodeId,
        x: ValueId,
    ) -> R<Option<UnitFire>> {
        use crate::prof::pages as pp;
        pp::unit();
        self.ops.units_folded.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (pid, items, res) = self.unit_items(lc, x)?;
        let classes = self.unit_classes(lc, &items, node_of(pid))?;
        // (units no page needs any more dropped)
        let t = pp::start();
        Self::drop_units(lc);
        pp::add(pp::DROP, t);
        if let std::collections::hash_map::Entry::Vacant(e) = lc.order.entry(pid) {
            e.insert(lc.seq);
            lc.seq += 1;
        }
        lc.prev.entry(pid).or_insert(lc.cur_units);
        lc.units.push(UnitRec { pid, src: x, items, res });
        let ui = lc.units.len() - 1;
        let t = pp::start();
        let r = self.fold_items(st, lc, node, pid, ui, 0, &classes);
        pp::add(pp::FOLD, t);
        Ok(r?.map(|(q, page)| (q, page, classes)))
    }

    /// Unit `ui`'s items from `from` folded; the page fired, if one.
    #[allow(clippy::too_many_arguments)]
    fn fold_items(
        &mut self,
        st: &mut ScanState,
        lc: &mut LoopCarry,
        node: phitex_defuse::NodeId,
        pid: i64,
        ui: usize,
        from: usize,
        classes: &[(u16, InsertClass)],
    ) -> R<Option<(FireNote, Page)>> {
        use crate::vprof::PItem;
        let Some((items, res)) = lc.units.get(ui).map(|u| (std::sync::Arc::clone(&u.items), u.res.clone())) else {
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
                fold: st.fold,
            };
            lc.srcs.push(src);
            lc.frontier = (ui, item + 1);
            match x {
                PItem::Build(k) => {
                    st.fold.since_bp = 0;
                    let mut live = |n: u16| class_in(classes, n);
                    if let Some(f) = build_t(st, lc, *k, &mut live) {
                        let t = crate::prof::pages::start();
                        let q = self.fire_at(st, lc, node, pid, &f, false);
                        crate::prof::pages::add(crate::prof::pages::FIRE, t);
                        return Ok(Some(q?));
                    }
                }
                // (§1335: the main list ends there, no build after it; the
                // page is not shipped)
                PItem::Abort => {
                    lc.pend.clear();
                    lc.ended = true;
                    return Ok(None);
                }
                PItem::SetPage(..) | PItem::InsParams(..) | PItem::End(_) => {
                    lc.pend.push((x.clone(), s));
                }
                _ => fold_item_t(st, lc, x, s),
            }
        }
        Ok(None)
    }
    /// The guess of what page `j`'s routine gives back (`SPEC.md` §2.2;
    /// [`Feedback::RoutineGivesBack`]): what the same routine gave back
    /// on the last page before fired on the text at the same
    /// `\outputpenalty`, which the routine branches on, every ordinary
    /// one (above -10001, a page shipped) alike; else for an ordinary
    /// page the markers it gave back last (the page parameters it sets)
    /// and no list, for a special one its last value as the evaluation
    /// began; nothing for a page fired in a routine's material, so no
    /// chain of pages grows on guesses. A dead cycle after one of the
    /// same routine and class, not one `\end` fired (§1054): the page
    /// before's give-back one item longer and `\deadcycles` one more
    /// ([`step_rtn`]). It depends on the pages before `j` alone.
    pub(crate) fn guess_give_back(
        &self,
        lc: &LoopCarry,
        key: RtnKey,
        hint: ValueId,
        output_penalty: i32,
        in_material: bool,
    ) -> ValueId {
        let v = self.v;
        let ordinary = output_penalty > -10001;
        let empty = || v.list(vec![v.list(Vec::new()), v.list(Vec::new())]);
        let stepped = (!in_material && key.2 && output_penalty != END_PENALTY)
            .then_some(lc.prev_rtn)
            .flatten()
            .filter(|p| p.0.0 == key.0 && p.0.1 == key.1)
            .and_then(|p| step_rtn(v, p.1));
        if in_material {
            empty()
        } else if let Some(x) = stepped {
            x
        } else {
            match lc.last_rtn.iter().find(|e| e.0 == key) {
                Some(e) => e.1,
                // (and on a page `\end` fired, §1054: the routine runs as on
                // an ordinary one, its markers kept)
                None if ordinary || output_penalty == END_PENALTY => lc
                    .last_markers
                    .iter()
                    .find(|e| e.0 == hint)
                    .map_or_else(empty, |e| v.list(vec![e.1, v.list(Vec::new())])),
                None => self.rtn_guess(hint, key.1),
            }
        }
    }

    /// A routine's give-back as the evaluation began (§1026): its last
    /// value on another page of the class `pen` ([`pen_class`];
    /// `crate::run::Ops::fin_defs` then, so every run of the loop
    /// guesses the same), `\deadcycles` left out, else nothing given
    /// back.
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
            .get(&fin_key_class(hint, pen))
            .and_then(|d| d.last.iter().find(|e| e.0 == rtn).map(|e| e.1))
            // (`\deadcycles` guessed 0, its register maps kept)
            .map_or_else(empty, |x| rtn_as_guess(v, x))
    }

    /// The [`Page`] of a fire (`SPEC.md` §2.3 "its page"): page `f` fired
    /// at candidate `f.k` of contributor `node` (or at the post-output
    /// build of the page before, `in_material`), from the builder's
    /// state where the page began and the sources folded since, without
    /// any later page's content; and what the fold after it goes on from
    /// ([`FireNote`]).
    #[allow(
        clippy::too_many_lines,
        clippy::assertions_on_constants,
        reason = "one fire's whole page; a debug or observe build stops on the defect"
    )]
    pub(crate) fn fire_at(
        &mut self,
        st: &mut ScanState,
        lc: &mut LoopCarry,
        node: phitex_defuse::NodeId,
        pid: i64,
        f: &LoopFire,
        in_material: bool,
    ) -> R<(FireNote, Page)> {
        let v = self.v;
        let j = st.j - 1;
        // (the page's input as spans: the post-output build's, then the
        // pieces' items)
        let mut spans = spans_of(&lc.srcs[..lc.post.min(lc.srcs.len())]);
        let post = spans.len();
        spans.extend(spans_of(lc.srcs.get(lc.post..).unwrap_or_default()));
        // (the window of the pieces' items its spans name: after the
        // contributor before the first of them)
        let mut first: Option<(u64, i64)> = None;
        for s in &spans {
            let p = match s.src {
                Src::Piece { piece, .. } | Src::Held { piece, .. } => piece,
                Src::Rtn { .. } => continue,
            };
            let o = lc.order.get(&p).copied().unwrap_or(u64::MAX);
            if first.is_none_or(|f| o < f.0) {
                first = Some((o, p));
            }
        }
        let after = first.and_then(|(_, p)| lc.prev.get(&p).copied().flatten());
        let hint = self.output_hint_at(node)?;
        // (and whether the page before left dead cycles, §1012: a
        // routine's run after a page it did not ship goes otherwise)
        let key = (hint, pen_class(f.output_penalty), lc.prev_fire.1 > 0);
        let rtn_guess = self.guess_give_back(lc, key, hint, f.output_penalty, in_material);
        if cfg!(feature = "observe") {
            // (the evaluation: its values and their epoch, the guesses
            // it began with)
            let eval = (
                std::ptr::from_ref(v) as usize,
                self.cx.values_epoch(),
                self.rtn_guess(hint, key.1),
            );
            guess_check(
                node.raw(),
                j,
                (eval, key, in_material, &lc.last_rtn, &lc.prev_rtn, &lc.last_markers),
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
                .filter_map(|p| lc.rtns.iter().rev().find(|r| r.0 == p).map(|r| (p, r.1)))
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
        let inf: Vec<Src> = lc
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
        let nth = if lc.nth.0 == Some(node) { lc.nth.1 } else { 0 };
        lc.nth = (Some(node), nth + 1);
        // (a routine's material by the pages before this one counted back
        // from it, not by their index among all the pages)
        let rel = |src: Src| match src {
            Src::Rtn { page, item } => Src::Rtn { page: j - page, item },
            s => s,
        };
        let page = Page {
            key: PageKey { piece: node, nth },
            normal: st.normal,
            special: f.output_penalty <= -10001,
            hint,
            classes: f.classes.clone(),
            k: f.k,
            in_material,
            after,
            spans: spans.iter().map(|s| Span { src: rel(s.src), len: s.len }).collect(),
            rtns: rtns.iter().map(|&(p, x)| (j - p, x)).collect(),
            fold: lc.began.0,
            params: lc.began.1,
            best: u32::try_from(f.best).unwrap_or(u32::MAX),
            output_penalty: f.output_penalty,
            holding: f.holding,
            held: u32::try_from(f.held.len()).unwrap_or(0),
            top_marks: lc.top_marks,
            post: u32::try_from(post).unwrap_or(0),
            post_markers: lc.post_markers.clone(),
            post_k: lc.post_k,
            prev_fire: lc.prev_fire,
            inf: inf.into_iter().map(rel).collect(),
            vers: page_vers(lc, &spans_pieces),
        };
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            eprintln!(
                "pages {}: page {} fires in piece {} at candidate {} (in material {in_material}, {} spans, best {}, penalty {})",
                self.cx.node().raw(),
                j + 1,
                node.raw(),
                f.k,
                page.spans.len(),
                f.best,
                f.output_penalty
            );
        }
        // (§1012: the marks in force where the next page begins, the
        // `\botmark` of every class after this page's marks)
        lc.top_marks = bot_marks_after(v, lc.top_marks, &f.marks, &lc.marks);
        Ok((
            FireNote {
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
            },
            page,
        ))
    }

    /// The page `p` fired: what its routine gave back (§1026: the
    /// finisher's output, the guess while it has none) folded as the next
    /// page's start with the held-over insertions and the contributions
    /// carried, then built at once; the fire that build makes, if any.
    #[allow(clippy::too_many_lines, reason = "one fire's whole give-back")]
    pub(crate) fn after_fire(&mut self, st: &mut ScanState, lc: &mut LoopCarry, p: &FireNote, gave: ValueId) -> R<Option<LoopFire>> {
        let classes = &p.classes[..];
        let v = self.v;
        let j = p.j;
        let node = p.node;
        let guess = p.rtn_guess;
        // (its finisher's output where it has run, else the guess:
        // [`Run::give_back`])
        let rtn = gave;
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
        lc.prev_rtn = Some((p.key, rtn));
        if !p.in_material {
            // (`\deadcycles` kept but on a page `\end` fired: a dead cycle
            // guessed there would fire another, §1054, and grow a chain of
            // pages on guesses)
            let given = if p.output_penalty == END_PENALTY {
                rtn_as_guess(v, rtn)
            } else {
                rtn
            };
            match lc.last_rtn.iter_mut().find(|e| e.0 == p.key) {
                Some(e) => e.1 = given,
                None => lc.last_rtn.push((p.key, given)),
            }
            let m = rs.first().copied().unwrap_or_else(|| v.list(Vec::new()));
            match lc.last_markers.iter_mut().find(|e| e.0 == p.hint) {
                Some(e) => e.1 = m,
                None => lc.last_markers.push((p.hint, m)),
            }
        }
        let mut markers = rs.first().map(|&x| list_of(v, x)).unwrap_or_default();
        let list = rs.get(1).map(|&x| list_of(v, x)).unwrap_or_default();
        // (`\deadcycles` as the routine left it, where not 0: what an
        // `\end` in the material after it reads, §1054)
        lc.rtn_dead = rs.get(2).map_or(0, |&x| int_of(v, x));
        // (§1054, §1012 after the page: `\deadcycles` as its routine left
        // it, given back with its material)
        lc.dead = lc.rtn_dead;
        lc.prev_fire = (p.pid, lc.rtn_dead);
        self.resolve_params_of(p.pid, &mut markers)?;
        let m_len = markers.len();
        let rtn_value = v.list(vec![v.list(markers), v.list(list)]);
        let rtn_items = std::sync::Arc::new(self.rtn_items(lc, rtn_value));
        lc.rtns.push((j, rtn_value, std::sync::Arc::clone(&rtn_items)));
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
        let mut fold0 = st.fold;
        let mut raw_from: Option<(usize, u32)> = None;
        let mut tail: Vec<Src> = Vec::new();
        for c in &p.carried {
            let Some(src) = c.src else { continue };
            match src {
                Src::Piece { piece, item } if end_sub_of(item).is_none() => {
                    let end_item = lc
                        .units
                        .iter()
                        .find(|u| u.pid == piece)
                        .and_then(|u| u.items.get(item as usize))
                        .is_some_and(|x| matches!(x, PItem::End(_)));
                    if end_item {
                        tail.push(src);
                    } else if raw_from.is_none() {
                        let ui = lc.units.iter().position(|u| u.pid == piece);
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
            let (fu, fi) = lc.frontier;
            let mut u = ui;
            let mut i = item;
            'outer: while u < lc.units.len() {
                let rec = &lc.units[u];
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
        st.fold = fold0;
        lc.began = (fold0, st.params.map(|q| params_marker(v, &q)));
        lc.held_nodes.clone_from(&p.held);
        lc.srcs = next.iter().map(|x| x.0).collect();
        lc.post = lc.srcs.len();
        lc.post_k = p.k;
        lc.pend.clear();
        lc.page_src.clear();
        lc.page_ins.clear();
        // (what the post-output build reads of the insertion classes on
        // its material: their registers where the page fired)
        let mut ns: Vec<u16> = Vec::new();
        for (src, raw) in &next {
            let y = if *raw {
                loop_item(lc, *src)
            } else {
                Self::loop_node(lc, *src).or_else(|| loop_item(lc, *src))
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
        lc.post_markers = post_markers;
        // (folded: as they are, or folded again with their builds and
        // markers spent)
        for (src, raw) in next {
            let s = SrcRef {
                src: Some(src),
                fold: st.fold,
            };
            if raw {
                let Some(x) = loop_item(lc, src) else { continue };
                match x {
                    PItem::Build(_) => st.fold.since_bp = 0,
                    PItem::Params(_) | PItem::SetPage(..) | PItem::InsParams(..) => {}
                    PItem::End(_) | PItem::Abort => lc.pend.push((x, s)),
                    _ => fold_item_t(st, lc, &x, s),
                }
            } else {
                let y = Self::loop_node(lc, src).or_else(|| loop_item(lc, src));
                if let Some(y) = y {
                    lc.pend.push((y, s));
                }
            }
        }
        for (n, c) in &post_classes {
            lc.pend.push((
                PItem::InsParams(*n, Some(*c)),
                SrcRef {
                    src: None,
                    fold: st.fold,
                },
            ));
        }
        st.fold.since_bp = 0;
        let mut live = |n: u16| class_in(classes, n);
        Ok(build_t(st, lc, p.k, &mut live))
    }}

/// A loop state as one flat list ([`encode_state`], [`encode_tail`]):
/// integers inline, values by their ids, a list as its length then its
/// elements.
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
    fn opt(&mut self, x: Option<usize>) {
        self.i(x.map_or(-1, |x| i64::try_from(x).unwrap_or(-1)));
    }
    fn x(&mut self, x: ValueId) {
        self.xs.push(x);
    }
    fn node(&mut self, n: Option<NodeId>) {
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
    fn src_ref(&mut self, s: &SrcRef) {
        match s.src {
            Some(s) => self.src(s),
            None => self.i(-1),
        }
        self.fold(s.fold);
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
    fn params(&mut self, p: Option<PageParams>) {
        match p {
            Some(q) => {
                self.i(1);
                self.i(i64::from(q.vsize));
                self.i(i64::from(q.max_depth));
                self.glue(&q.top_skip);
                self.i(i64::from(q.holding_inserts));
            }
            None => self.i(0),
        }
    }
    fn class(&mut self, n: u16, c: &InsertClass) {
        self.i(i64::from(n));
        self.i(c.box_height_plus_depth.map_or(-1, i64::from));
        self.i(i64::from(c.box_is_hbox));
        self.i(i64::from(c.count));
        self.i(i64::from(c.dimen));
        self.glue(&c.skip);
    }
    fn res(&mut self, r: &Resolved) {
        self.params(r.params);
        self.len(r.classes.len());
        for (n, c) in &r.classes {
            self.class(*n, c);
        }
    }
    /// A node the builder over profiles holds (a box as its kind and
    /// dimensions, an insertion with its list), a mark by its value in
    /// the loop's table (`marks`).
    fn pnode(&mut self, n: &Node, marks: &[ValueId]) {
        use phitex_typeset::node::GlueKind;
        match n {
            Node::Box(b) => {
                self.i(0);
                self.i(i64::from(b.kind != phitex_typeset::node::BoxKind::H));
                self.i(i64::from(b.width));
                self.i(i64::from(b.height));
                self.i(i64::from(b.depth));
            }
            Node::Rule(r) => {
                self.i(1);
                self.i(i64::from(r.width));
                self.i(i64::from(r.height));
                self.i(i64::from(r.depth));
            }
            Node::Glue(g) => {
                self.i(2);
                self.glue(&g.spec);
                self.i(match g.kind {
                    GlueKind::Param(c) => i64::from(c),
                    _ => -1,
                });
            }
            Node::Kern(w, _) => {
                self.i(3);
                self.i(i64::from(*w));
            }
            Node::Penalty(p) => {
                self.i(4);
                self.i(i64::from(*p));
            }
            Node::Mark(k) => {
                self.i(5);
                match marks.get(*k as usize) {
                    Some(&m) => self.x(m),
                    None => self.x(ValueId::NIL),
                }
            }
            Node::Whatsit(_, s) => {
                self.i(6);
                match s {
                    Some(s) => {
                        self.i(1);
                        self.i(i64::from(s.width));
                        self.i(i64::from(s.height));
                        self.i(i64::from(s.depth));
                    }
                    None => self.i(0),
                }
            }
            Node::Ins(ins) => {
                self.i(7);
                self.i(i64::from(ins.number));
                self.i(i64::from(ins.height));
                self.i(i64::from(ins.depth));
                self.i(i64::from(ins.float_cost));
                self.glue(&ins.split_top_skip);
                self.len(ins.list.len());
                for y in &ins.list {
                    self.pnode(y, marks);
                }
            }
            // (none other on the main vertical list as the profiles have
            // it: a whatsit of no size, nothing to the builder)
            _ => {
                self.i(6);
                self.i(0);
            }
        }
    }
    fn pnodes(&mut self, l: &[Node], marks: &[ValueId]) {
        self.len(l.len());
        for n in l {
            self.pnode(n, marks);
        }
    }
    fn pitem(&mut self, x: &PItem, marks: &[ValueId]) {
        match x {
            PItem::Node(n) => {
                self.i(0);
                self.pnode(n, marks);
            }
            PItem::VBox { node, bs, ls, lsl } => {
                self.i(1);
                self.pnode(node, marks);
                self.glue(bs);
                self.glue(ls);
                self.i(i64::from(*lsl));
            }
            PItem::Pd(d) => {
                self.i(2);
                self.i(i64::from(*d));
            }
            PItem::Delete => self.i(3),
            PItem::Build(k) => {
                self.i(4);
                self.i(i64::from(*k));
            }
            PItem::Params(p) => {
                self.i(5);
                self.params(*p);
            }
            PItem::SetPage(i, x) => {
                self.i(6);
                self.i(*i);
                self.i(i64::from(*x));
            }
            PItem::InsParams(n, c) => {
                self.i(7);
                if let Some(c) = c {
                    self.i(1);
                    self.class(*n, c);
                } else {
                    self.i(0);
                    self.i(i64::from(*n));
                }
            }
            PItem::End(ys) => {
                self.i(8);
                self.len(ys.len());
                for y in ys {
                    self.pitem(y, marks);
                }
            }
            PItem::Abort => self.i(9),
            PItem::Sf(k, s) => {
                self.i(10);
                self.i(*k);
                self.i(i64::from(*s));
            }
            PItem::Opaque(t) => {
                self.i(11);
                self.i(*t);
            }
        }
    }
    /// The page builder over profiles in full (§980–§991's state).
    fn pb(&mut self, pb: &PageBuilder, marks: &[ValueId]) {
        use phitex_typeset::page::{Contents, PageDiag};
        self.pnodes(&pb.page, marks);
        self.i(match pb.contents {
            Contents::Empty => 0,
            Contents::InsertsOnly => 1,
            Contents::BoxThere => 2,
        });
        for &x in &pb.so_far {
            self.i(i64::from(x));
        }
        self.i(i64::from(pb.max_depth));
        self.opt(pb.best_break);
        self.i(i64::from(pb.least_cost));
        self.i(i64::from(pb.best_size));
        match &pb.last_glue {
            Some(g) => {
                self.i(1);
                self.glue(g);
            }
            None => self.i(0),
        }
        self.i(i64::from(pb.last_penalty));
        self.i(i64::from(pb.last_kern));
        self.i(i64::from(pb.last_node_type));
        self.i(i64::from(pb.insert_penalties));
        self.len(pb.ins.len());
        for r in &pb.ins {
            self.i(i64::from(r.n));
            self.i(i64::from(r.split_up));
            self.i(i64::from(r.height));
            self.opt(r.broken_ptr);
            self.opt(r.broken_ins);
            self.opt(r.last_ins);
            self.opt(r.best_ins);
        }
        self.pnodes(&pb.discards, marks);
        self.len(pb.diags.len());
        for d in &pb.diags {
            let (t, n) = match *d {
                PageDiag::InfiniteShrink => (0, 0),
                PageDiag::InfiniteShrinkInsert(n) => (1, n),
                PageDiag::InsertIntoHbox(n) => (2, n),
                PageDiag::InfiniteShrinkSplit => (3, 0),
            };
            self.i(t);
            self.i(i64::from(n));
        }
        self.i(i64::from(pb.measure_only));
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
    fn opt(&mut self) -> Option<usize> {
        usize::try_from(self.i()).ok()
    }
    fn node(&mut self) -> Option<NodeId> {
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
    fn src_ref(&mut self) -> SrcRef {
        let t = self.i();
        let src = if t == -1 { None } else { self.src_tagged(t) };
        SrcRef { src, fold: self.fold() }
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
    fn params(&mut self) -> Option<PageParams> {
        (self.i() != 0).then(|| PageParams {
            vsize: self.i32(),
            max_depth: self.i32(),
            top_skip: self.glue(),
            holding_inserts: self.i32(),
            saving_vdiscards: false,
        })
    }
    fn class(&mut self) -> (u16, InsertClass) {
        let k = u16::try_from(self.i()).unwrap_or(0);
        let h = self.i();
        (
            k,
            InsertClass {
                box_height_plus_depth: (h >= 0).then(|| i32::try_from(h).unwrap_or(0)),
                box_is_hbox: self.i() != 0,
                count: self.i32(),
                dimen: self.i32(),
                skip: self.glue(),
            },
        )
    }
    fn res(&mut self) -> Resolved {
        let params = self.params();
        let n = self.len();
        Resolved {
            params,
            classes: (0..n).map(|_| self.class()).collect(),
        }
    }
    fn pnode(&mut self, marks: &mut Vec<ValueId>) -> Node {
        use phitex_typeset::node::{BoxKind, BoxNode, Insert, KernKind, Rule, WhatsitSize};
        match self.i() {
            0 => {
                let k = self.i();
                Node::Box(Box::new(BoxNode {
                    kind: if k == 0 { BoxKind::H } else { BoxKind::V },
                    width: self.i32(),
                    height: self.i32(),
                    depth: self.i32(),
                    ..BoxNode::default()
                }))
            }
            1 => Node::Rule(Rule {
                width: self.i32(),
                height: self.i32(),
                depth: self.i32(),
            }),
            2 => {
                let g = self.glue();
                match u8::try_from(self.i()) {
                    Ok(c) => Node::param_glue(c, g),
                    Err(_) => Node::glue(g),
                }
            }
            3 => Node::Kern(self.i32(), KernKind::Explicit),
            4 => Node::Penalty(self.i32()),
            5 => {
                let m = self.x();
                if m == ValueId::NIL {
                    Node::Mark(u32::MAX)
                } else {
                    marks.push(m);
                    Node::Mark(u32::try_from(marks.len() - 1).unwrap_or(u32::MAX))
                }
            }
            7 => {
                let number = u16::try_from(self.i()).unwrap_or(0);
                let (height, depth, float_cost) = (self.i32(), self.i32(), self.i32());
                let split_top_skip = self.glue();
                let n = self.len();
                let list = (0..n).map(|_| self.pnode(marks)).collect();
                Node::Ins(Box::new(Insert {
                    number,
                    height,
                    depth,
                    split_top_skip,
                    float_cost,
                    list,
                }))
            }
            _ => {
                if self.i() == 1 {
                    Node::Whatsit(
                        0,
                        Some(WhatsitSize {
                            width: self.i32(),
                            height: self.i32(),
                            depth: self.i32(),
                        }),
                    )
                } else {
                    Node::Whatsit(0, None)
                }
            }
        }
    }
    fn pnodes(&mut self, marks: &mut Vec<ValueId>) -> List {
        let n = self.len();
        (0..n).map(|_| self.pnode(marks)).collect()
    }
    fn pitem(&mut self, marks: &mut Vec<ValueId>) -> PItem {
        match self.i() {
            0 => PItem::Node(self.pnode(marks)),
            1 => {
                let node = self.pnode(marks);
                let (bs, ls) = (self.glue(), self.glue());
                PItem::VBox { node, bs, ls, lsl: self.i32() }
            }
            2 => PItem::Pd(self.i32()),
            3 => PItem::Delete,
            4 => PItem::Build(self.u32()),
            5 => PItem::Params(self.params()),
            6 => PItem::SetPage(self.i(), self.i32()),
            7 => {
                if self.i() == 1 {
                    let (n, c) = self.class();
                    PItem::InsParams(n, Some(c))
                } else {
                    PItem::InsParams(u16::try_from(self.i()).unwrap_or(0), None)
                }
            }
            8 => {
                let n = self.len();
                PItem::End((0..n).map(|_| self.pitem(marks)).collect())
            }
            9 => PItem::Abort,
            10 => PItem::Sf(self.i(), self.i32()),
            _ => PItem::Opaque(self.i()),
        }
    }
    fn pb(&mut self, marks: &mut Vec<ValueId>) -> PageBuilder {
        use phitex_typeset::page::{Contents, PageDiag, PageIns};
        let mut pb = PageBuilder::new();
        pb.page = self.pnodes(marks);
        pb.contents = match self.i() {
            0 => Contents::Empty,
            1 => Contents::InsertsOnly,
            _ => Contents::BoxThere,
        };
        for k in 0..8 {
            pb.so_far[k] = self.i32();
        }
        pb.max_depth = self.i32();
        pb.best_break = self.opt();
        pb.least_cost = self.i32();
        pb.best_size = self.i32();
        pb.last_glue = (self.i() != 0).then(|| self.glue());
        pb.last_penalty = self.i32();
        pb.last_kern = self.i32();
        pb.last_node_type = self.i32();
        pb.insert_penalties = self.i32();
        let n = self.len();
        pb.ins = (0..n)
            .map(|_| PageIns {
                n: u16::try_from(self.i()).unwrap_or(0),
                split_up: self.i() != 0,
                height: self.i32(),
                broken_ptr: self.opt(),
                broken_ins: self.opt(),
                last_ins: self.opt(),
                best_ins: self.opt(),
            })
            .collect();
        pb.discards = self.pnodes(marks);
        let n = self.len();
        pb.diags = (0..n)
            .map(|_| {
                let (t, n) = (self.i(), u16::try_from(self.i()).unwrap_or(0));
                match t {
                    0 => PageDiag::InfiniteShrink,
                    1 => PageDiag::InfiniteShrinkInsert(n),
                    2 => PageDiag::InsertIntoHbox(n),
                    _ => PageDiag::InfiniteShrinkSplit,
                }
            })
            .collect();
        pb.measure_only = self.i() != 0;
        pb
    }
}

/// [`encode_state`]'s part for the fire `p`.
fn encode_fire(e: &mut Enc<'_>, p: &FireNote) {
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
        e.src_ref(c);
    }
    e.i(i64::from(p.nth));
    e.len(p.classes.len());
    for (k, c) in &p.classes {
        e.class(*k, c);
    }
}

/// [`encode_fire`]'s fire.
fn decode_fire(d: &mut Dec<'_>) -> FireNote {
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
        if let (Some(s), Some(PItem::Node(h))) = (s, crate::vprof::decode(&ints).into_iter().next()) {
            held.push((s, h));
        }
    }
    let n = d.len();
    let carried = (0..n).map(|_| d.src_ref()).collect();
    let nth = d.u32();
    let n = d.len();
    let classes = (0..n).map(|_| d.class()).collect();
    FireNote {
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
    }
}

/// What [`encode_state`] keeps of the loop: the units from `keep` on, the
/// pieces the units and the fire name, and the routines' pages it names.
fn state_names(lc: &LoopCarry, p: &FireNote) -> (usize, Vec<i64>, Vec<i64>) {
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
    let keep = lc
        .units
        .iter()
        .position(|u| named.contains(&u.pid))
        .map_or(lc.frontier.0, |i| i.min(lc.frontier.0));
    let units = &lc.units[keep.min(lc.units.len())..];
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
        .chain(lc.inf.iter().filter_map(|s| match *s {
            Src::Rtn { page, .. } => Some(page),
            _ => None,
        }))
        .collect();
    pages.sort_unstable();
    pages.dedup();
    (keep, pieces, pages)
}

/// The loop at a page's fire and the fire, as its checkpoint holds it:
/// what the fold on from there reads, nothing else (the units no source
/// after the fire names, the routines' material none names, the places
/// of the pieces neither names, left out), so a page fired again as it
/// was has the same state. The units with their markers as they were
/// read where their pieces are ([`Resolved`]), so a run resuming here
/// reads none of them again.
pub(crate) fn encode_state(v: &Values, st: &ScanState, lc: &LoopCarry, p: &FireNote) -> ValueId {
    let (keep, pieces, pages) = state_names(lc, p);
    let units = &lc.units[keep.min(lc.units.len())..];
    let mut e = Enc { v, xs: Vec::with_capacity(64 + 4 * lc.inf.len() + 2 * units.len()) };
    e.i(st.j);
    e.i(st.normal);
    e.fold(st.fold);
    e.params(st.params);
    e.node(lc.cur);
    e.node(lc.cur_units);
    e.x(lc.top_marks);
    let inf: Vec<Src> = lc
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
    // (the give-backs the guesses of later pages read, by routine and
    // class: what the pages before gave, read or guessed)
    e.len(lc.last_rtn.len());
    for &(k, x) in &lc.last_rtn {
        e.key(k);
        e.x(x);
    }
    e.len(lc.last_markers.len());
    for &(h, m) in &lc.last_markers {
        e.x(h);
        e.x(m);
    }
    e.len(units.len());
    for u in units {
        e.x(u.src);
        e.res(&u.res);
    }
    e.len(lc.frontier.0.saturating_sub(keep));
    e.i(i64::from(lc.frontier.1));
    // (the pieces' places counted from the first it names: only their
    // order is read, and a place counted from the job's start moved every
    // later state with each piece more or fewer before it)
    let base = pieces.iter().filter_map(|q| lc.order.get(q)).min().copied().unwrap_or(lc.seq);
    let rel = |o: u64| i64::try_from(o.saturating_sub(base)).unwrap_or(0);
    e.len(pieces.len());
    for q in &pieces {
        e.i(*q);
        e.node(lc.prev.get(q).copied().flatten());
        e.i(lc.order.get(q).map_or(-1, |&o| rel(o)));
    }
    e.i(rel(lc.seq));
    let rtns: Vec<&(i64, ValueId, std::sync::Arc<Vec<PItem>>)> =
        lc.rtns.iter().filter(|r| pages.binary_search(&r.0).is_ok()).collect();
    e.len(rtns.len());
    for r in rtns {
        e.i(r.0);
        e.x(r.1);
    }
    e.node(lc.nth.0);
    e.i(i64::from(lc.nth.1));
    encode_fire(&mut e, p);
    v.list(e.xs)
}

/// The loop past its last checkpoint, at a hole, in full: the builder
/// with its page half built, the contributions pending with their
/// sources, every part of the carry (a mark by its value), so the run
/// when the hole fills goes on as if it had not stopped.
pub(crate) fn encode_tail(v: &Values, st: &ScanState, lc: &LoopCarry) -> ValueId {
    let m = &lc.marks[..];
    let mut e = Enc { v, xs: Vec::with_capacity(256) };
    e.node(lc.cur);
    e.i(st.j);
    e.i(st.normal);
    e.fold(st.fold);
    e.params(st.params);
    e.pb(&st.pb, m);
    e.len(lc.srcs.len());
    for &s in &lc.srcs {
        e.src(s);
    }
    e.len(lc.post);
    e.len(lc.post_markers.len());
    for &x in &lc.post_markers {
        e.x(x);
    }
    e.i(i64::from(lc.post_k));
    e.fold(lc.began.0);
    e.x(lc.began.1.unwrap_or(ValueId::UNIT));
    e.x(lc.top_marks);
    e.i(lc.rtn_dead);
    e.i(lc.dead);
    e.len(lc.inf.len());
    for &s in &lc.inf {
        e.src(s);
    }
    e.len(lc.last_rtn.len());
    for &(k, x) in &lc.last_rtn {
        e.key(k);
        e.x(x);
    }
    match lc.prev_rtn {
        Some((k, x)) => {
            e.i(1);
            e.key(k);
            e.x(x);
        }
        None => e.i(0),
    }
    e.len(lc.last_markers.len());
    for &(h, x) in &lc.last_markers {
        e.x(h);
        e.x(x);
    }
    e.i(lc.prev_fire.0);
    e.i(lc.prev_fire.1);
    e.len(lc.pend.len());
    for (x, s) in &lc.pend {
        e.pitem(x, m);
        e.src_ref(s);
    }
    e.len(lc.page_src.len());
    for s in &lc.page_src {
        e.src_ref(s);
    }
    e.len(lc.units.len());
    for u in &lc.units {
        e.x(u.src);
        e.res(&u.res);
    }
    let mut prev: Vec<(&i64, &Option<NodeId>)> = lc.prev.iter().collect();
    prev.sort_unstable_by_key(|e| *e.0);
    e.len(prev.len());
    for (p, n) in prev {
        e.i(*p);
        e.node(*n);
    }
    e.node(lc.cur_units);
    e.len(lc.rtns.len());
    for r in &lc.rtns {
        e.i(r.0);
        e.x(r.1);
    }
    e.len(lc.frontier.0);
    e.i(i64::from(lc.frontier.1));
    e.len(lc.page_ins.len());
    for x in &lc.page_ins {
        e.i(x.map_or(-1, i64::from));
    }
    e.len(lc.held_nodes.len());
    for (s, n) in &lc.held_nodes {
        e.src(*s);
        e.pnode(n, m);
    }
    let mut order: Vec<(&i64, &u64)> = lc.order.iter().collect();
    order.sort_unstable_by_key(|e| *e.0);
    e.len(order.len());
    for (p, o) in order {
        e.i(*p);
        e.i(i64::try_from(*o).unwrap_or(0));
    }
    e.i(i64::try_from(lc.seq).unwrap_or(0));
    e.node(lc.nth.0);
    e.i(i64::from(lc.nth.1));
    e.i(i64::from(lc.ended));
    v.list(e.xs)
}

impl Run<'_, '_> {
    /// A unit's items as the loop holds them, decoded from its profile
    /// unit `x` with its markers as read before (`res`): reads nothing.
    fn unit_items_as(&self, lc: &mut LoopCarry, x: ValueId) -> (i64, std::sync::Arc<Vec<PItem>>) {
        let v = self.v;
        let d = crate::vprof::decoded(v, x);
        if !d.marks {
            return (d.pid, std::sync::Arc::clone(&d.items));
        }
        let mut items = (*d.items).clone();
        remap_marks(&mut items, &crate::vprof::unit_marks(v, x), &mut lc.marks);
        (d.pid, std::sync::Arc::new(items))
    }

    /// A checkpoint's state ([`encode_state`]) as the builder, the carry
    /// and the fire it folds on from: its units decoded again with their
    /// markers as they were read. Reads nothing.
    pub(crate) fn decode_state(&mut self, x: ValueId) -> (ScanState, LoopCarry, FireNote) {
        let v = self.v;
        let mut d = Dec { v, xs: list_of(v, x), at: 0 };
        let mut lc = LoopCarry::new();
        let (j, normal, fold) = (d.i(), d.i(), d.fold());
        let params = d.params();
        let st = ScanState::fresh(j, normal, fold, params);
        lc.cur = d.node();
        lc.cur_units = d.node();
        lc.top_marks = d.x();
        let n = d.len();
        lc.inf = (0..n).filter_map(|_| d.src()).collect();
        let n = d.len();
        lc.last_rtn = (0..n).map(|_| (d.key(), d.x())).collect();
        let n = d.len();
        lc.last_markers = (0..n).map(|_| (d.x(), d.x())).collect();
        let n = d.len();
        for _ in 0..n {
            let src = d.x();
            let res = d.res();
            let (pid, items) = self.unit_items_as(&mut lc, src);
            lc.units.push(UnitRec { pid, items, res, src });
        }
        lc.frontier = (d.len(), d.u32());
        let n = d.len();
        for _ in 0..n {
            let p = d.i();
            lc.prev.insert(p, d.node());
            let o = d.i();
            if o >= 0 {
                lc.order.insert(p, u64::try_from(o).unwrap_or(0));
            }
        }
        lc.seq = u64::try_from(d.i()).unwrap_or(0);
        let n = d.len();
        for _ in 0..n {
            let (page, val) = (d.i(), d.x());
            let items = self.rtn_items(&mut lc, val);
            lc.rtns.push((page, val, std::sync::Arc::new(items)));
        }
        lc.nth = (d.node(), d.u32());
        let p = decode_fire(&mut d);
        (st, lc, p)
    }

    /// [`encode_tail`]'s loop. Reads nothing.
    pub(crate) fn decode_tail(&mut self, x: ValueId) -> (ScanState, LoopCarry) {
        let v = self.v;
        let mut d = Dec { v, xs: list_of(v, x), at: 0 };
        let mut lc = LoopCarry::new();
        lc.cur = d.node();
        let (j, normal, fold) = (d.i(), d.i(), d.fold());
        let params = d.params();
        let mut st = ScanState::fresh(j, normal, fold, params);
        st.pb = d.pb(&mut lc.marks);
        let n = d.len();
        lc.srcs = (0..n).filter_map(|_| d.src()).collect();
        lc.post = d.len();
        let n = d.len();
        lc.post_markers = (0..n).map(|_| d.x()).collect();
        lc.post_k = d.u32();
        lc.began = (d.fold(), some(d.x()));
        lc.top_marks = d.x();
        lc.rtn_dead = d.i();
        lc.dead = d.i();
        let n = d.len();
        lc.inf = (0..n).filter_map(|_| d.src()).collect();
        let n = d.len();
        lc.last_rtn = (0..n).map(|_| (d.key(), d.x())).collect();
        lc.prev_rtn = (d.i() != 0).then(|| (d.key(), d.x()));
        let n = d.len();
        lc.last_markers = (0..n).map(|_| (d.x(), d.x())).collect();
        lc.prev_fire = (d.i(), d.i());
        let n = d.len();
        for _ in 0..n {
            let x = d.pitem(&mut lc.marks);
            let s = d.src_ref();
            lc.pend.push((x, s));
        }
        let n = d.len();
        lc.page_src = (0..n).map(|_| d.src_ref()).collect();
        let n = d.len();
        for _ in 0..n {
            let src = d.x();
            let res = d.res();
            let (pid, items) = self.unit_items_as(&mut lc, src);
            lc.units.push(UnitRec { pid, items, res, src });
        }
        let n = d.len();
        for _ in 0..n {
            let p = d.i();
            lc.prev.insert(p, d.node());
        }
        lc.cur_units = d.node();
        let n = d.len();
        for _ in 0..n {
            let (page, val) = (d.i(), d.x());
            let items = self.rtn_items(&mut lc, val);
            lc.rtns.push((page, val, std::sync::Arc::new(items)));
        }
        lc.frontier = (d.len(), d.u32());
        let n = d.len();
        lc.page_ins = (0..n).map(|_| u16::try_from(d.i()).ok()).collect();
        let n = d.len();
        for _ in 0..n {
            let s = d.src();
            let node = d.pnode(&mut lc.marks);
            if let Some(s) = s {
                lc.held_nodes.push((s, node));
            }
        }
        let n = d.len();
        for _ in 0..n {
            let p = d.i();
            lc.order.insert(p, u64::try_from(d.i()).unwrap_or(0));
        }
        lc.seq = u64::try_from(d.i()).unwrap_or(0);
        lc.nth = (d.node(), d.u32());
        lc.ended = d.i() != 0;
        (st, lc)
    }
}
