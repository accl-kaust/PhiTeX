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
//! At its end the node places every page's finisher right after the
//! piece its page fired in ([`Run::place_pages`]; today's engine:
//! `follow_after_any`), and defines [`PAGES_END`].
//!
//! What the loop reads where a piece is (the page parameters, an
//! insertion class's registers, `\deadcycles` at `\end`) it reads at that
//! piece, a read past its own node ([`Run::page_params_at`],
//! [`Run::insert_class_at`], [`Feedback::BuilderRegisters`]).
//!
//! The finisher's own fold of its page's items is the same builder on
//! real nodes ([`PageState`]), at the decisions the loop made.

#![expect(dead_code, reason = "the layers skeleton: used once its stubs are filled (docs/spec-layers.md §7)")]

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

/// The page builder's node's loop, kept over a park.
pub(crate) struct PageLoop {
    /// The builder over profiles.
    pub st: crate::vprof::ScanState,
    /// The last contributor folded (the cursor of `acc_next`).
    pub cur: Option<phitex_defuse::NodeId>,
    /// Where each item the builder holds came from, in order: the page
    /// so far, then the contributions not yet looked at. What a page
    /// that fires is made of ([`FinishIn::spans`]).
    pub srcs: Vec<Src>,
    /// The builder's state where the current page began: its fold and
    /// the page parameters in force ([`FinishIn::fold`]).
    pub began: (FoldState, Option<ValueId>),
    /// The marks in force where the current page began (`\topmark` and
    /// its classes, §1012).
    pub top_marks: ValueId,
    /// The routines' real material where it missed the guess
    /// ([`PAGE_MISS`]), read once at the loop's start: `(page, rtn)`.
    pub misses: Vec<(i64, ValueId)>,
    /// The pages fired so far: the piece each fired in, its finisher's
    /// input.
    pub pages: Vec<(phitex_defuse::NodeId, FinishIn)>,
    /// Whether the loop has read [`PAGE_MISS`] (once, at its start).
    pub misses_read: bool,
}

impl PageLoop {
    /// The loop at the job's start: no page fired, the main list empty.
    pub(crate) fn new() -> PageLoop {
        PageLoop {
            st: crate::vprof::ScanState::fresh(0, 0, crate::build::IGNORE_DEPTH_STATE, None),
            cur: None,
            srcs: Vec::new(),
            began: (crate::build::IGNORE_DEPTH_STATE, None),
            top_marks: ValueId::UNIT,
            misses: Vec::new(),
            pages: Vec::new(),
            misses_read: false,
        }
    }
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
    /// The page builder's loop over the profiles (the module's doc), from
    /// where `lp` stands to the unit's end:
    ///
    /// 1. once, the misses ([`PAGE_MISS`], `acc_fold` from this node to
    ///    the unit's end) into `lp.misses`;
    /// 2. each contributor after `lp.cur` ([`phitex_defuse::Ctx::acc_next`]
    ///    on [`crate::vprof::VPROF`]): its units decoded, their markers
    ///    resolved where the piece is ([`Run::resolve_prof_of`]),
    ///    `\deadcycles` read at the piece where it holds `\end` (§1054),
    ///    each item folded with `lp.st` and its source pushed on
    ///    `lp.srcs` as the builder takes it (§994–§1008: what the page
    ///    takes, what waits, what is discarded at the page's top);
    /// 3. where a page fires (§1005, §1014–§1017): its [`FinishIn`] from
    ///    `lp.srcs` up to the fire and the builder's decisions, pushed on
    ///    `lp.pages` with the piece it fired in; `lp.srcs` keeps what goes
    ///    back to the contributions (the page's items from the best break
    ///    on, §1017), after the held-over insertions ([`Src::Held`]) and
    ///    the routine's material ([`Src::Rtn`]: the miss read back where
    ///    the page has one, else the guess, [`Run::rtn_guess`]), which
    ///    are folded first; `lp.began` and `lp.top_marks` the new page's.
    ///
    /// A read of a piece still in work stops ([`Halt::Stop`]): the node
    /// parks `lp` and goes on from it.
    pub(crate) fn page_loop(&mut self, lp: &mut PageLoop) -> R<()> {
        let _ = lp;
        todo!("layer 4: the page builder's loop over the profiles (docs/spec-layers.md §7 step 1)")
    }

    /// What page `j`'s routine is guessed to give back to the main list
    /// (§1026, [`FinishIn::rtn_guess`]): its last value on another page
    /// (`crate::run::Ops::fin_defs` as the evaluation began, so every
    /// attempt of the loop guesses the same), else nothing given back.
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
            .unwrap_or_else(empty)
    }

    /// The loop's end: every page's finisher placed right after the
    /// piece its page fired in, in page order (today's engine:
    /// `follow_after_any`; on the cells engine a node of layer 5's unit
    /// at the piece's position), and where the pages end defined.
    pub(crate) fn place_pages(&mut self, lp: &PageLoop) {
        let v = self.v;
        for (at, fin) in &lp.pages {
            if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
                eprintln!(
                    "place finisher of page {} (piece {} candidate {})",
                    fin.j,
                    at.raw(),
                    fin.k
                );
            }
            self.cx
                .follow_after_any(*at, self.ops.finish, fin.args(v));
        }
        self.cx
            .define_with(v.field(PAGES_END), ValueId::UNIT, Locality::Global);
    }
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
    /// The best break's index on the page, and the insertions held over
    /// ([`crate::vprof::ScanFire`]'s).
    pub best: usize,
    pub held: usize,
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
        let (best, held) = (f.best, f.held_inserts.len());
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
            held,
        }
    }

    /// The page fired at candidate `k` of a later piece at its known best
    /// break (`best`, `\outputpenalty` there, `\holdinginserts` positive
    /// where it fired), the page built as far as the break
    /// ([`PageBuilder::fire_known`]): its finisher's page from the items
    /// before the break alone. `None` where the page is not built to it.
    pub(crate) fn fire_known(
        &mut self,
        v: &Values,
        best: usize,
        output_penalty: i32,
        holding: bool,
        k: u32,
    ) -> Option<Fire> {
        let mut params = self.params_pre.unwrap_or_else(|| {
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
        params.holding_inserts = i32::from(holding);
        let f = self.pb.fire_known(best, output_penalty, &params)?;
        Some(self.fired(v, &f, k, Vec::new()))
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

/// Whether a fired page's `\outputpenalty` is -10001 or less.
pub(crate) fn fire_special(v: &Values, data: ValueId) -> bool {
    list_of(v, data)
        .get(4)
        .is_some_and(|&x| int_of(v, x) <= -10001)
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
