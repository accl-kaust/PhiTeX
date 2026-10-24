//! Layer 5: a page's finisher, one node a page ([`Finish`]), placed by
//! the page builder's node right after the piece its page fired in.
//!
//! - Its input is its arguments alone ([`FinishIn`]): the page's index
//!   and where it stands among the pages, the output routine's hint, and
//!   what the page is made of and how the builder decided it. It reads
//!   its page's items by range ([`PITEMS`], the window the spans name)
//!   and assembles the page with the builder on real nodes at those
//!   decisions ([`Run::finisher_args`], [`PageState`]): `\box255`, the
//!   insertion boxes, the marks.
//! - It runs the output routine (§1012–§1026) as one run of its own on
//!   its page ([`Run::fire_page`], [`Run::output_end`],
//!   [`Run::ship_out`]), reading what reaches its place as any node does.
//!   What the finisher before it left (the page counters, a float list,
//!   `\deadcycles`) it reads on that finisher's prediction
//!   ([`FinDefs::predict`]): the finishers run in parallel.
//! - It defines what the routine defines for the text after it
//!   ([`Feedback::OutputState`]), what the routine gives back ([`RTN`],
//!   also its output, which the page builder reads where it placed it),
//!   and where the next page begins for the text's `\pagetotal`
//!   ([`PAGE_START`], [`Run::define_page_start`]).
//! - A page firing at a candidate inside a piece that could not end
//!   there has its finisher run on the piece's state at the candidate
//!   ([`Run::preload_candidate`]), the piece's later definitions standing
//!   over its routine's ([`Run::give_way`]).

use super::*;

/// A routine's give-back as [`RTN`] holds it: `[markers, list]`, and
/// `[markers, list, dead, maps]` where `\deadcycles` is not 0 or the
/// routine wrote integer registers globally (`maps`: `[[n, keep, add]…]`,
/// [`crate::regfold::Affine`]).
pub(crate) fn rtn_value(
    v: &Values,
    markers: ValueId,
    list: ValueId,
    dead: i64,
    maps: &[(i32, crate::regfold::Affine)],
) -> ValueId {
    if dead == 0 && maps.is_empty() {
        return v.list(vec![markers, list]);
    }
    let ms = maps
        .iter()
        .map(|&(n, m)| v.list(vec![v.int(i64::from(n)), v.int(i64::from(m.keep)), v.int(i64::from(m.add))]))
        .collect();
    v.list(vec![markers, list, v.int(dead), v.list(ms)])
}

/// A give-back without its register maps: what the page builder folds
/// (its markers, its list, `\deadcycles`), the finisher's output; the
/// maps, which differ from page to page as the routine's own reads of a
/// register make its write a map or a definition, stay in [`RTN`].
pub(crate) fn rtn_strip(v: &Values, x: ValueId) -> ValueId {
    let xs = list_of(v, x);
    if xs.len() < 4 {
        return x;
    }
    rtn_value(v, xs[0], xs[1], int_of(v, xs[2]), &[])
}

/// Give-back `x` with register maps `maps` ([`rtn_value`]).
pub(crate) fn rtn_with_maps(v: &Values, x: ValueId, maps: &[(i32, crate::regfold::Affine)]) -> ValueId {
    let xs = list_of(v, x);
    if xs.len() < 2 {
        return x;
    }
    rtn_value(v, xs[0], xs[1], xs.get(2).map_or(0, |&d| int_of(v, d)), maps)
}

/// The register maps of a give-back ([`rtn_value`]).
pub(crate) fn rtn_maps(v: &Values, x: ValueId) -> Vec<(i32, crate::regfold::Affine)> {
    list_of(v, x)
        .get(3)
        .map(|&ms| list_of(v, ms))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| match list_of(v, e)[..] {
            [n, k, a] => Some((
                i32::try_from(int_of(v, n)).ok()?,
                crate::regfold::Affine {
                    keep: int_of(v, k) != 0,
                    add: i32::try_from(int_of(v, a)).unwrap_or(0),
                },
            )),
            _ => None,
        })
        .collect()
}

/// A give-back as a guess: `\deadcycles` 0 (a page shipped: a dead cycle
/// guessed would grow a chain of pages on guesses), its maps kept.
pub(crate) fn rtn_as_guess(v: &Values, x: ValueId) -> ValueId {
    let xs = list_of(v, x);
    if xs.len() < 3 {
        return x;
    }
    rtn_value(v, xs[0], xs[1], 0, &rtn_maps(v, x))
}

/// What a finisher's output routine gives back to the main list
/// (§1026): `[markers, list]`, the run's own main-list items (page
/// parameter markers) and the routine's list, without the held-over
/// insertions (the page builder has them); then, where not nothing,
/// `\deadcycles` as the routine left it (a page not shipped: §1054's
/// `\end` after it reads it) and the maps of its global integer register
/// writes ([`rtn_value`]). The page builder folds the
/// next page on its guess of it ([`FinishIn::rtn_guess`]) until the
/// finisher's output, this value, says otherwise
/// ([`phitex_defuse::Ctx::placed_output`]).
pub(crate) const RTN: &str = "rtn";

/// A finisher's arguments (a node's arguments are what the engine knows
/// of it before it runs: [`Finish::defs`] and its kin read them): `[j,
/// (8 unused), hint, normal, special, page, classes]`. Its run builds its
/// arguments in full from them ([`Run::finisher_args`]: `[j, data,
/// carried, locals, rest, piece, k, reads, defs, hint, normal, special,
/// cuttable]`), which [`FinCtx::args`] holds; the positions both share
/// are these.
pub(crate) mod arg {
    /// The page's index (0 for the first page).
    pub(crate) const J: usize = 0;
    /// The output routine's hint where the page fired
    /// ([`crate::hints::routine_hint`]), encoded; unit for the default
    /// routine.
    pub(crate) const HINT: usize = 9;
    /// The ordinary pages fired up to this one, with it where it is one.
    pub(crate) const NORMAL: usize = 10;
    /// Whether `\outputpenalty` is -10001 or less (§1013).
    pub(crate) const SPECIAL: usize = 11;
    /// The page ([`super::FinishIn`]'s other fields, encoded).
    pub(crate) const PAGE: usize = 12;
    /// The insertion classes on the page (§1018: the boxes its fire
    /// fills).
    pub(crate) const CLASSES: usize = 13;
}

/// What a finisher is given: everything the page builder decided about
/// its page, and where the page's items are. Its arguments
/// ([`FinishIn::args`], [`arg`]).
#[derive(Clone, Debug)]
pub(crate) struct FinishIn {
    pub j: i64,
    pub normal: i64,
    pub special: bool,
    /// The output routine's hint where the page fired ([`arg::HINT`]).
    pub hint: ValueId,
    pub classes: Vec<u16>,
    /// The piece the page fired in: the finisher is placed right after it.
    pub piece: phitex_defuse::NodeId,
    /// The candidate it fired at (§994's `build_page` call).
    pub k: u32,
    /// Whether it fired in the material the page before left (the
    /// routine's, the material carried), before any item of `piece`
    /// after that page's candidate.
    pub in_material: bool,
    /// The window of [`PITEMS`] its spans read: from after this node (none:
    /// the unit's start) to `piece`.
    pub after: Option<phitex_defuse::NodeId>,
    /// Where each item the builder took for the page came from, in order,
    /// from where the page began to its fire: the held-over insertions and
    /// the routine's material of the page before first, the material
    /// carried, then the pieces' items.
    pub spans: Vec<Span>,
    /// The routines' material the spans name ([`Src::Rtn`]), by page.
    pub rtns: Vec<(i64, ValueId)>,
    /// The builder's state where the page began: the main list's fold
    /// and the page parameters in force (a `T_PAGE_PARAMS` marker).
    pub fold: FoldState,
    pub params: Option<ValueId>,
    /// The builder's decisions where it fired (§1005–§1017): the best
    /// break's index on the page, `\outputpenalty` there, whether
    /// `\holdinginserts` was positive, the insertions held over.
    pub best: u32,
    pub output_penalty: i32,
    pub holding: bool,
    pub held: u32,
    /// The marks in force where the page began (`\topmark` and its
    /// classes, §1012): `[[class, mark]…]`.
    pub top_marks: ValueId,
    /// What the page builder guessed this page's routine gives back,
    /// and folded the next page on ([`Feedback::RoutineGivesBack`],
    /// [`Run::rtn_guess`]): from the pages before this one only, never
    /// this page's real value read back, so its own miss does not change
    /// this node.
    pub rtn_guess: ValueId,
    /// How many of the spans the post-output build of the page before
    /// takes (§1026: the routine's material and the held-over insertions
    /// as they are, a piece's items carried folded again with their
    /// builds and markers spent), and what that build reads besides: the
    /// insertion classes' registers on its material (`T_INS_PARAMS`
    /// markers) and its candidate. None for the first page.
    pub post: u32,
    pub post_markers: Vec<ValueId>,
    pub post_k: u32,
    /// The piece the page before fired in (its raw id, -1 none) and
    /// `\deadcycles` as its routine left it ([`RTN`]'s third element):
    /// what §1054's `\end` reads in the post-output build and in the
    /// rest of that piece.
    pub prev_fire: (i64, i64),
    /// The items of its spans that are the best break of a page before,
    /// a penalty: 10000 since that page fired (§1013).
    pub inf: Vec<Src>,
    /// The version of each piece its spans name, as the page builder
    /// folded it (the piece's [`crate::vprof::VPROF`] unit, which its
    /// [`PITEMS`] unit holds too): a finisher reading a unit of another
    /// version runs on a placement made before that piece ran again, and
    /// makes no page ([`Run::finisher_args`]).
    pub vers: Vec<(i64, ValueId)>,
}

impl FinishIn {
    /// The finisher's arguments ([`arg`]).
    pub(crate) fn args(&self, v: &Values) -> Vec<ValueId> {
        let raw = |n: phitex_defuse::NodeId| v.int(i64::from_ne_bytes(n.raw().to_ne_bytes()));
        let page = v.list(vec![
            raw(self.piece),
            v.int(i64::from(self.k)),
            v.int(i64::from(self.in_material)),
            self.after.map_or_else(|| v.int(-1), raw),
            v.list(self.spans.iter().map(|s| s.encode(v)).collect()),
            v.list(
                self.rtns
                    .iter()
                    .map(|&(j, x)| v.list(vec![v.int(j), x]))
                    .collect(),
            ),
            v.int(i64::from(self.fold.sf)),
            v.int(i64::from(self.fold.pd)),
            v.int(i64::try_from(self.fold.since_bp).unwrap_or(0)),
            self.params.unwrap_or(ValueId::UNIT),
            v.int(i64::from(self.best)),
            v.int(i64::from(self.output_penalty)),
            v.int(i64::from(self.holding)),
            v.int(i64::from(self.held)),
            self.top_marks,
            self.rtn_guess,
            v.int(i64::from(self.post)),
            v.list(self.post_markers.clone()),
            v.int(i64::from(self.post_k)),
            v.int(self.prev_fire.0),
            v.int(self.prev_fire.1),
            v.list(
                self.inf
                    .iter()
                    .map(|&src| Span { src, len: 1 }.encode(v))
                    .collect(),
            ),
            v.list(self.vers.iter().map(|&(p, x)| v.list(vec![v.int(p), x])).collect()),
        ]);
        let mut out = vec![v.int(self.j)];
        out.extend([ValueId::UNIT; 8]);
        out.extend([
            self.hint,
            v.int(self.normal),
            v.int(i64::from(self.special)),
            page,
            v.list(self.classes.iter().map(|&n| v.int(i64::from(n))).collect()),
        ]);
        out
    }

    /// A finisher's input from its arguments; `None` where they hold no
    /// page.
    pub(crate) fn of(v: &Values, pos: &[ValueId]) -> Option<FinishIn> {
        let p = list_of(v, *pos.get(arg::PAGE)?);
        if p.len() < 16 {
            return None;
        }
        let n = |i: usize| int_of(v, p[i]);
        let node = |x: i64| phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(x.to_ne_bytes()));
        let at = |i: usize| pos.get(i).map_or(0, |&x| int_of(v, x));
        Some(FinishIn {
            j: at(arg::J),
            normal: at(arg::NORMAL),
            special: at(arg::SPECIAL) != 0,
            hint: pos.get(arg::HINT).copied().unwrap_or(ValueId::UNIT),
            classes: pos
                .get(arg::CLASSES)
                .map(|&c| {
                    list_of(v, c)
                        .into_iter()
                        .filter_map(|x| u16::try_from(int_of(v, x)).ok())
                        .collect()
                })
                .unwrap_or_default(),
            piece: node(n(0)),
            k: u32::try_from(n(1)).unwrap_or(0),
            in_material: n(2) != 0,
            after: (n(3) != -1).then(|| node(n(3))),
            spans: list_of(v, p[4])
                .into_iter()
                .filter_map(|x| Span::decode(v, x))
                .collect(),
            rtns: list_of(v, p[5])
                .into_iter()
                .filter_map(|e| match list_of(v, e)[..] {
                    [j, x] => Some((int_of(v, j), x)),
                    _ => None,
                })
                .collect(),
            fold: FoldState {
                sf: i32::try_from(n(6)).unwrap_or(1000),
                pd: i32::try_from(n(7)).unwrap_or(IGNORE_DEPTH),
                since_bp: usize::try_from(n(8)).unwrap_or(0),
            },
            params: some(p[9]),
            best: u32::try_from(n(10)).unwrap_or(0),
            output_penalty: i32::try_from(n(11)).unwrap_or(0),
            holding: n(12) != 0,
            held: u32::try_from(n(13)).unwrap_or(0),
            top_marks: p[14],
            rtn_guess: p[15],
            post: p.get(16).map_or(0, |&x| u32::try_from(int_of(v, x)).unwrap_or(0)),
            post_markers: p.get(17).map(|&x| list_of(v, x)).unwrap_or_default(),
            post_k: p.get(18).map_or(0, |&x| u32::try_from(int_of(v, x)).unwrap_or(0)),
            prev_fire: (
                p.get(19).map_or(-1, |&x| int_of(v, x)),
                p.get(20).map_or(0, |&x| int_of(v, x)),
            ),
            inf: p
                .get(21)
                .map(|&x| list_of(v, x))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|x| Span::decode(v, x).map(|s| s.src))
                .collect(),
            vers: p
                .get(22)
                .map(|&x| list_of(v, x))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|e| match list_of(v, e)[..] {
                    [p, x] => Some((int_of(v, p), x)),
                    _ => None,
                })
                .collect(),
        })
    }
}

impl Run<'_, '_> {
    /// A finisher's arguments in full ([`arg`]'s layout) and its page's
    /// break in its piece (`[piece, candidate, inside]`, [`FinCtx::brk`]),
    /// from its input alone:
    ///
    /// 1. the items its spans name: the [`PITEMS`] units of the window
    ///    `(fin.after, fin.piece]` read by range, each piece's markers
    ///    resolved where the piece is ([`Run::resolve_params_of`]), a
    ///    routine's material from `fin.rtns`, a held-over insertion's
    ///    list from its split (§1021);
    /// 2. folded with the builder on real nodes from `fin.fold` and
    ///    `fin.params` ([`PageState::fold_items`]): the post-output build
    ///    first (`fin.post` spans: the material as it is, a piece's items
    ///    carried folded again with their builds and markers spent, then
    ///    `fin.post_markers` and the build at `fin.post_k`), then the
    ///    pieces' items to the fire: the page's data, `\box255` native
    ///    ([`Run::fin_box`]), the material carried;
    /// 3. where the candidate is inside the piece with commands after it
    ///    (not its last, [`PITEMS`]' `last`), the piece's names at that
    ///    candidate ([`Run::check_data`]) and its locals.
    ///
    /// A fire the real nodes do not make where the profiles made it is a
    /// defect: a panic in debug and observe builds, an engine diagnostic
    /// otherwise.
    #[allow(
        clippy::type_complexity,
        clippy::too_many_lines,
        clippy::assertions_on_constants,
        reason = "a long assembly; a debug or observe build stops on the defect"
    )]
    pub(crate) fn finisher_args(
        &mut self,
        fin: &FinishIn,
        pos: &[ValueId],
    ) -> R<(Vec<ValueId>, Option<(i64, i64, bool)>)> {
        let v = self.v;
        let me = |n: phitex_defuse::NodeId| i64::from_ne_bytes(n.raw().to_ne_bytes());
        let pid = me(fin.piece);
        // (the window's units, by piece: their items with the markers
        // resolved where each piece is)
        // (the window read through paths, [`Run::window_units`]: each
        // unit's piece, then only the units the page names, their items
        // whole and the own piece's other parts; a unit of the window
        // the page does not name changing (its items, its check) leaves
        // this finisher standing)
        let (pids, units, own_view) = self.window_units(fin, pid)?;
        let names_own = fin.spans.iter().any(|s| {
            matches!(s.src, Src::Piece { piece, .. } | Src::Held { piece, .. } if piece == pid)
        });
        if names_own && !pids.contains(&pid) {
            // (the piece stands after this finisher in the view, placed
            // again, the page builder to place the finisher after it
            // again: a view the engine replaces, the window read here to
            // the piece re-asked when the piece comes before this node.
            // No page made on it, and none waited for: the piece may read
            // what this node defines)
            return Ok((Vec::new(), None));
        }
        if self.stale_placement(fin, &pids)? {
            // (a piece the page names ran again since the page builder
            // folded it: the placement is stale, the builder's fold of
            // the new version places this finisher again; no page made
            // on it, the mismatch no defect)
            if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
                eprintln!("finisher {} of page {}: stale placement", self.cx.node().raw(), fin.j);
            }
            return Ok((Vec::new(), None));
        }
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            eprintln!(
                "finisher {} of page {} runs (piece {} candidate {}, after {:?}, {} spans, window {:?})",
                self.cx.node().raw(),
                fin.j,
                fin.piece.raw(),
                fin.k,
                fin.after.map(phitex_defuse::NodeId::raw),
                fin.spans.len(),
                pids
            );
        }
        let mut unit_ix: phitex_defuse::FxHashMap<i64, usize> = phitex_defuse::FxHashMap::default();
        for (i, u) in units.iter().enumerate() {
            unit_ix.insert(u.0, i);
        }
        let defect = |r: &mut Self, what: String| {
            let msg = format!(
                "page {} (finisher {}): {what}: the page builder over the profiles and the finisher's page part",
                fin.j,
                r.cx.node().raw()
            );
            assert!(!cfg!(any(debug_assertions, feature = "observe")), "{msg}");
            r.engine_diag(&msg);
        };
        // (each page's given-back items decoded once, not per item)
        let rtns: std::cell::RefCell<phitex_defuse::FxHashMap<i64, std::rc::Rc<Vec<ValueId>>>> =
            std::cell::RefCell::default();
        let rtn_items = |page: i64| -> std::rc::Rc<Vec<ValueId>> {
            if let Some(x) = rtns.borrow().get(&page) {
                return x.clone();
            }
            let x = std::rc::Rc::new(
                fin.rtns
                    .iter()
                    .find(|r| r.0 == page)
                    .map(|r| {
                        let rs = list_of(v, r.1);
                        let mut out = rs.first().map(|&x| list_of(v, x)).unwrap_or_default();
                        out.extend(rs.get(1).map(|&x| list_of(v, x)).unwrap_or_default());
                        out
                    })
                    .unwrap_or_default(),
            );
            rtns.borrow_mut().insert(page, x.clone());
            x
        };
        let unit_of = |piece: i64| unit_ix.get(&piece).map(|&i| &units[i]);
        // (an item a source names, real)
        let item_raw = |src: Src| -> Option<ValueId> {
            match src {
                Src::Piece { piece, item } => {
                    let raw = &unit_of(piece)?.2;
                    match crate::pages::end_sub_of(item) {
                        None => raw.get(item as usize).copied(),
                        Some((e, s)) => list_of(v, *raw.get(e as usize)?).get(1 + s as usize).copied(),
                    }
                }
                Src::Rtn { page, item } => rtn_items(page).get(item as usize).copied(),
                Src::Held { piece, item, from } => {
                    let raw = &unit_of(piece)?.2;
                    let x = *raw.get(item as usize)?;
                    if from == 0 {
                        return Some(x);
                    }
                    let mut side = Side::default();
                    let Some(Node::Ins(mut ins)) = nodes::decode(v, x, &mut side) else {
                        return None;
                    };
                    // (§1021: the rest of a split, its top pruned)
                    let tail = ins.list.split_off((from as usize).min(ins.list.len()));
                    let (rem, _) = phitex_typeset::page::prune_page_top(tail, ins.split_top_skip, false);
                    let b = phitex_typeset::pack::vpack(
                        rem.clone(),
                        Spec::Additional(0),
                        &PackParams::default(),
                    )
                    .node;
                    ins.height = b.height + b.depth;
                    ins.list = rem;
                    Some(nodes::encode(v, &Node::Ins(ins), &side))
                }
            }
        };
        // (§1013: a page before's best break, a penalty, is 10000 since)
        let item = |src: Src| -> Option<ValueId> {
            let x = item_raw(src)?;
            if fin.inf.contains(&src) && nodes::tag(v, x) == nodes::T_PENALTY {
                return Some(nodes::encode(
                    v,
                    &Node::Penalty(phitex_typeset::linebreak::INF_PENALTY),
                    &Side::default(),
                ));
            }
            Some(x)
        };
        let mut ps = PageState::fresh(fin.j);
        ps.normal = fin.normal - i64::from(!fin.special);
        ps.fold = fin.fold;
        ps.params = fin.params;
        // (`\deadcycles` as the routine before left it, §1012: the page
        // builder's, never the finisher before's; the text's settings
        // after it come in order with the items, `DEAD_SLOT` markers)
        ps.dead = fin.prev_fire.1;
        let at = fin.piece;
        let mut fire: Option<Fire> = None;
        let mut fired_at_end = false;
        // 1. the post-output build
        let post = (fin.post as usize).min(fin.spans.len());
        if post > 0 {
            for s in &fin.spans[..post] {
                for src in crate::pages::span_srcs(s) {
                    let Some(x) = item(src) else {
                        defect(self, format!("no item for {src:?}"));
                        continue;
                    };
                    let carried_raw = matches!(src, Src::Piece { item, .. } if crate::pages::end_sub_of(item).is_none())
                        && nodes::tag(v, x) != nodes::T_END;
                    if carried_raw {
                        match nodes::tag(v, x) {
                            nodes::T_BUILD_PAGE => ps.fold.since_bp = 0,
                            nodes::T_PAGE_PARAMS | nodes::T_SET_PAGE | nodes::T_INS_PARAMS => {}
                            nodes::T_ABORT => ps.pending.push(x),
                            _ => fold_item(v, 0, x, &mut ps.fold, &mut ps.pending),
                        }
                    } else {
                        ps.pending.push(x);
                    }
                }
            }
            ps.pending.extend(fin.post_markers.iter().copied());
            let bp = v.list(vec![v.int(nodes::T_BUILD_PAGE), v.int(i64::from(fin.post_k))]);
            let mut live = |c: u16| self.insert_class_at(c, at);
            fire = ps.fold_items(v, &[bp], &mut live)?;
            fired_at_end = post == fin.spans.len();
        }
        // 2. the pieces' items, piece by piece
        let mut i = post;
        while fire.is_none() && i < fin.spans.len() {
            let s = fin.spans[i];
            i += 1;
            let mut raw: Vec<ValueId> = Vec::new();
            for src in crate::pages::span_srcs(&s) {
                match item(src) {
                    Some(x) => raw.push(x),
                    None => defect(self, format!("no item for {src:?}")),
                }
            }
            let p_at = match s.src {
                Src::Piece { piece, .. } => crate::pages::node_of(piece),
                _ => at,
            };
            let mut live = |c: u16| self.insert_class_at(c, p_at);
            fire = ps.fold_items(v, &raw, &mut live)?;
            fired_at_end = i == fin.spans.len() && fire.as_ref().is_some_and(|f| f.rest.is_empty());
        }
        let Some(mut fire) = fire else {
            defect(self, "no page fires on the real nodes".to_owned());
            return Ok((pos.to_vec(), None));
        };
        if !fired_at_end || fire.k != fin.k || fire.best != fin.best as usize {
            defect(
                self,
                format!(
                    "the real nodes fire at candidate {} best {} (at the input's end {fired_at_end}), the profiles at {} best {}",
                    fire.k, fire.best, fin.k, fin.best
                ),
            );
        }
        self.fin_box = fire.b255.take();
        // (the piece the page fired in: its check where the candidate is
        // inside it, the rest of its items after the candidate)
        let own = units
            .iter()
            .find(|u| u.0 == pid)
            .map(|u| list_of(v, u.1))
            .unwrap_or_default();
        let a = |i: usize| own.get(i).copied().unwrap_or(ValueId::UNIT);
        let last = if own.is_empty() { -1 } else { int_of(v, a(1)) };
        let at_end = i64::from(fire.k) == last;
        let rest: Vec<ValueId> = if fin.in_material {
            // (the piece's items after its candidate, all to come)
            units
                .iter()
                .find(|u| u.0 == pid)
                .map(|u| {
                    let k = i64::from(fin.k);
                    u.2.iter()
                        .position(|&x| nodes::tag(v, x) == nodes::T_BUILD_PAGE && int_at(v, x, 1) == k)
                        .map_or_else(Vec::new, |p| u.2[p + 1..].to_vec())
                })
                .unwrap_or_default()
        } else {
            std::mem::take(&mut fire.rest)
        };
        // (the own unit's other parts read as they are used: its locals
        // always, its check only where the candidate is inside it; of a
        // local or a name standing at the candidate, which it is, and its
        // value only where the routine uses it, [`Run::fin_lazy_value`])
        let part = |r: &mut Self, k: u32| {
            own_view
                .and_then(|(w, _)| r.cx.get(w, phitex_defuse::Step::Index(k)))
                .unwrap_or(ValueId::UNIT)
        };
        let locals = self.lazy_locals(own_view);
        let mut args = vec![
            pos.first().copied().unwrap_or(ValueId::UNIT),
            fire.data,
            v.list(fire.carried.clone()),
            v.list(locals),
        ];
        let mut cuttable = false;
        if at_end || own.is_empty() {
            args.extend([ValueId::UNIT; 5]);
        } else {
            // (the names at the candidate, and whether the run could end
            // there)
            let defs_k;
            (defs_k, cuttable) = self.lazy_names_at(own_view, fire.k);
            let reads = ValueId::UNIT;
            if crate::exec::prof_flag!("PHITEX_FIN_ITEMS") {
                let d4 = part(self, 4);
                eprintln!(
                    "fin-parts: node {} piece {pid} k {} part3 {:016x} part4 {:016x} names {:016x} {}",
                    self.cx.node().raw(),
                    fire.k,
                    reads.raw(),
                    d4.raw(),
                    defs_k.raw(),
                    v.render(defs_k).chars().take(4000).collect::<String>()
                );
                eprintln!(
                    "fin-part3: node {} {}",
                    self.cx.node().raw(),
                    v.render(reads).chars().take(4000).collect::<String>()
                );
            }
            args.extend([v.list(rest.clone()), v.int(pid), v.int(i64::from(fire.k)), reads, defs_k]);
        }
        if crate::exec::prof_flag!("PHITEX_FIN_ITEMS") {
            eprintln!(
                "fin-locals: node {} {:016x} {}",
                self.cx.node().raw(),
                args[3].raw(),
                v.render(args[3]).chars().take(2000).collect::<String>()
            );
        }
        args.extend(pos.get(arg::HINT..arg::PAGE).unwrap_or_default().iter().copied());
        args.resize(12, ValueId::UNIT);
        args.push(v.int(i64::from(cuttable)));
        // (where the next page begins, for [`Run::define_page_start`]:
        // the fold after the fire, the parameters in force, the rest)
        args.push(v.list(vec![
            v.int(i64::from(ps.fold.sf)),
            v.int(i64::from(ps.fold.pd)),
            v.int(i64::try_from(ps.fold.since_bp).unwrap_or(0)),
            ps.params.unwrap_or(ValueId::UNIT),
            v.list(rest),
            // (`\deadcycles` where the page fires, §1012)
            v.int(ps.dead),
        ]));
        let brk = Some((pid, i64::from(fire.k), !at_end && !own.is_empty()));
        Ok((args, brk))
    }

    /// Whether a piece finisher `fin`'s page names holds another version
    /// in the window (`pids`, the window's pieces) than the page builder
    /// folded ([`FinishIn::vers`]): read as that comparison alone.
    fn stale_placement(&mut self, fin: &FinishIn, pids: &[i64]) -> R<bool> {
        use phitex_defuse::Step;
        let v = self.v;
        if fin.vers.is_empty() {
            return Ok(false);
        }
        let w = self.cx.acc_view(v.field(PITEMS), fin.after, Some(fin.piece))?;
        for &(p, ver) in &fin.vers {
            let Some(i) = pids.iter().position(|&q| q == p) else { continue };
            let same = self
                .cx
                .at(w, Step::Index(u32::try_from(i).unwrap_or(u32::MAX)))
                .and_then(|u| self.cx.at(u, Step::Index(8)))
                .and_then(|x| self.cx.get(x, Step::Eq(ver)));
            if same == Some(v.bool(false)) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The [`PITEMS`] window of finisher `fin` (after `fin.after`, to its
    /// piece `pid`) read through paths: every unit's piece (one read),
    /// then for each unit whose piece the page's spans name (or `pid`)
    /// its items, whole, with their markers resolved where the piece is,
    /// and for `pid` its last candidate (`[pid, last]` as the unit's
    /// shape) and its view, through which the finisher reads its other
    /// parts as it uses them. The finisher depends on those alone.
    #[allow(clippy::type_complexity)]
    pub(crate) fn window_units(
        &mut self,
        fin: &FinishIn,
        pid: i64,
    ) -> R<(Vec<i64>, Vec<(i64, ValueId, Vec<ValueId>)>, Option<(phitex_defuse::View, u32)>)> {
        use phitex_defuse::Step;
        let v = self.v;
        if cfg!(feature = "observe") {
            let k = PageNode::of(self.ops, v, self.cx.op(), &self.cx.args());
            assert!(k.is_some_and(|k| k.folds().contains(&PITEMS)), "a window of {PITEMS} outside a finisher");
        }
        let w = self.cx.acc_view(v.field(PITEMS), fin.after, Some(fin.piece))?;
        let first = self.cx.path(&[Step::Index(0)]);
        let pids: Vec<i64> = self
            .cx
            .get(w, Step::Each(first))
            .map(|x| v.seq_to_vec(x).into_iter().map(|p| int_of(v, p)).collect())
            .unwrap_or_default();
        let mut want: phitex_defuse::FxHashSet<i64> = phitex_defuse::FxHashSet::default();
        want.insert(pid);
        // (the items of each piece the page's spans name: a unit read at
        // those alone, so a change to a piece's items after the page's
        // cut (the next page's part of a piece that spans the break)
        // leaves this finisher standing; the own piece whole where the
        // page fired in the material before it, whose items after the
        // candidate all go on to the next page)
        for s in &fin.spans {
            if let Src::Piece { piece, .. } | Src::Held { piece, .. } = s.src {
                want.insert(piece);
            }
        }
        let named = named_items(fin);
        let whole = |p: i64| (p == pid && fin.in_material) || !named.contains_key(&p);
        let pick = |r: &mut Self, list: phitex_defuse::View, p: i64| r.items_at(list, &named[&p]);
        let mut units: Vec<(i64, ValueId, Vec<ValueId>)> = Vec::new();
        let mut own = None;
        // (a PDF page's lines read by their shape: their dimensions and
        // fonts, not their characters, `crate::pages::seal`;
        // `PHITEX_NO_SEAL=1` reads them whole)
        let seal = !crate::exec::obs_flag!("PHITEX_NO_SEAL") && self.param("pdfoutput")? > 0;
        for (i, &p) in pids.iter().enumerate() {
            if !want.contains(&p) || units.iter().any(|u| u.0 == p) {
                continue;
            }
            let ui = u32::try_from(i).unwrap_or(u32::MAX);
            let Some(u) = self.cx.at(w, Step::Index(ui)) else {
                continue;
            };
            // (a PDF page's unit read by its shape, its items' view kept
            // for its sealed lines, `crate::pages::seal`)
            let shape = if seal {
                self.cx.at(u, Step::Index(7)).filter(|&w| self.cx.get(w, Step::Eq(ValueId::UNIT)) == Some(v.bool(false)))
            } else {
                None
            };
            let mut raw = match self.cx.at(u, Step::Index(2)) {
                Some(x) if let Some(sh) = shape => {
                    let raw_id = |n: phitex_defuse::NodeId| i64::from_ne_bytes(n.raw().to_ne_bytes());
                    let place = u32::try_from(i).unwrap_or(u32::MAX);
                    self.seal_unit(p, x, place, fin.after.map(raw_id), raw_id(fin.piece));
                    if whole(p) {
                        let sh = self.cx.whole(sh);
                        list_of(v, sh)
                    } else {
                        pick(self, sh, p)
                    }
                }
                Some(x) if whole(p) => {
                    let x = self.cx.whole(x);
                    list_of(v, x)
                }
                Some(x) => pick(self, x, p),
                None => Vec::new(),
            };
            // (`PHITEX_FIN_ITEMS=1`, profile: each unit's items a
            // finisher reads, one line each, to compare two evaluations)
            if crate::exec::prof_flag!("PHITEX_FIN_ITEMS") {
                for (k, &x) in raw.iter().enumerate() {
                    eprintln!(
                        "fin-items: node {} unit {p} item {k} {:016x} {}",
                        self.cx.node().raw(),
                        x.raw(),
                        v.render(x).chars().take(300).collect::<String>()
                    );
                }
            }
            self.resolve_params_of(p, &mut raw)?;
            // (whether the page's candidate is the own piece's last, read
            // as that alone (a piece growing after the page's cut leaves
            // it standing): the candidate where so, else none (-2); its
            // other parts read later)
            let shape = if p == pid {
                own = Some((u, ui));
                let k = i64::from(fin.k);
                let at_last = self.cx.at(u, Step::Index(1)).and_then(|w| self.cx.get(w, Step::Eq(v.int(k)))) == Some(v.bool(true));
                v.list(vec![v.int(p), v.int(if at_last { k } else { -2 })])
            } else {
                ValueId::UNIT
            };
            units.push((p, shape, raw));
        }
        Ok((pids, units, own))
    }

    /// A unit's items, or their shapes, at the indices `ixs` (sorted) of
    /// `list`, read as those alone ([`Run::window_units`]): the others
    /// unit, never read. A named index the list does not have ends the
    /// items there, as the list whole would (a miss, never a stand-in).
    fn items_at(&mut self, list: phitex_defuse::View, ixs: &[u32]) -> Vec<ValueId> {
        let n = ixs.last().map_or(0, |&i| i as usize + 1);
        let mut out = vec![ValueId::UNIT; n];
        for &i in ixs {
            let Some(x) = self.cx.get(list, phitex_defuse::Step::Index(i)) else {
                out.truncate(i as usize);
                break;
            };
            out[i as usize] = x;
        }
        out
    }

    /// Where the next page begins, for the text after this finisher
    /// ([`PAGE_START`], [`Feedback::PageSoFar`]): the piece the page
    /// fired in, the pages fired, the ordinary ones, the main list's fold
    /// and the page parameters after the fire, and what the next page
    /// begins with as a profile: the held-over insertions, the routine's
    /// list ([`FinCtx::pending`]), the material carried and the rest of
    /// the piece after the candidate.
    pub(crate) fn define_page_start(&mut self) -> R<()> {
        let v = self.v;
        let Some(c) = self.fin.clone() else {
            return Ok(());
        };
        let Some(&st) = c.args.get(13) else {
            return Ok(());
        };
        let xs = list_of(v, st);
        let n = |i: usize| xs.get(i).map_or(0, |&x| int_of(v, x));
        let fold = FoldState {
            sf: i32::try_from(n(0)).unwrap_or(1000),
            pd: i32::try_from(n(1)).unwrap_or(IGNORE_DEPTH),
            since_bp: usize::try_from(n(2)).unwrap_or(0),
        };
        let params = xs.get(3).copied().and_then(some);
        let mut mat: Vec<ValueId> = c.pending.clone();
        mat.extend(c.args.get(2).map(|&x| list_of(v, x)).unwrap_or_default());
        mat.push(v.list(vec![v.int(nodes::T_BUILD_PAGE), v.int(0)]));
        mat.extend(xs.get(4).map(|&x| list_of(v, x)).unwrap_or_default());
        self.resolve_page_params(&mut mat)?;
        let anchor = c.brk.map(|b| crate::pages::node_of(b.0));
        let normal = c.args.get(arg::NORMAL).map_or(0, |&x| int_of(v, x));
        let s = PageStart {
            anchor,
            j: c.j + 1,
            normal,
            fold,
            params,
            prof: crate::vprof::encode(v, &mat),
        };
        self.cx
            .define_with(v.field(PAGE_START), s.encode(v), Locality::Global);
        Ok(())
    }
}


/// A finisher's page: its index and what the routine leaves for the
/// next page (§1026: the held-over insertions, then its list; the page
/// after it begins with them, with the material carried over and the
/// rest of the piece, [`Run::define_page_start`]).
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
    /// Its page's break in its piece (`[piece, candidate]`) and whether
    /// the piece goes on after it (the break is inside it).
    pub brk: Option<(i64, i64, bool)>,
}

// -------------------------------------------------------------- state

impl Run<'_, '_> {
    /// A finisher's end: what its routine gives back to the main list
    /// ([`RTN`]): its items, then the page parameters it set where no
    /// item of its own noted them, its list, and `\deadcycles` where not
    /// 0 (§1054's `\end` reads it) (§987: the build after the routine,
    /// §1026, reads them as the routine left them, not as they reached
    /// the piece the page fired in).
    pub(crate) fn define_rtn(&mut self, mut raw: Vec<ValueId>) -> R<()> {
        let v = self.v;
        if self.st.take_page_params() {
            raw.push(self.page_params_marker()?);
        }
        let c = self.fin.clone().unwrap_or_default();
        let list = c.pending.get(c.held_n..).unwrap_or_default().to_vec();
        // (and `\deadcycles` as it leaves it where not 0, a page not
        // shipped: what `\end` reads after it, §1054; and the maps its
        // global writes of integer registers are, `crate::regfold`)
        let dead = self.cint(crate::fields::cell::DEADCYCLES)?;
        let mut maps: Vec<(i32, crate::regfold::Affine)> = Vec::new();
        for (&l, &m) in &self.reg_pending {
            if let Some(n) = self.count_reg(l) {
                maps.push((n, m));
            }
        }
        maps.sort_by_key(|e| e.0);
        let x = rtn_value(v, v.list(raw), v.list(list), dead, &maps);
        // (what it gives back leaves the run: its sealed lines' lists)
        let x = self.unseal(x);
        self.cx.define_with(v.field(RTN), x, Locality::Global);
        // (a routine that gave back other than the page builder folded
        // the next page on: counted; the builder's node reads the real
        // material as the finisher's output, [`Feedback::RoutineGivesBack`])
        let pos = self.cx.args();
        let guess = FinishIn::of(v, &pos).map(|f| f.rtn_guess);
        if guess.is_some_and(|g| rtn_strip(v, g) != rtn_strip(v, x)) {
            Feedback::RoutineGivesBack.missed();
            let j = pos.first().map_or(0, |&y| int_of(v, y));
            if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
                let r = |y: ValueId| v.render(y).chars().take(300).collect::<String>();
                eprintln!(
                    "miss: page {j} (finisher {}): guess {} real {}",
                    self.cx.node().raw(),
                    guess.map_or_else(String::new, r),
                    r(x)
                );
            }
        }
        if let Some(c) = self.fin.as_mut() {
            c.rtn = Some(x);
        }
        Ok(())
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
    pub(crate) fn fire_page(&mut self, d: ValueId) -> R<bool> {
        let v = self.v;
        let data = list_of(v, d);
        let a = |i: usize| data.get(i).copied().unwrap_or(ValueId::UNIT);
        // (spec-final A4: `\box255` from the builder's native box, its
        // line boxes their own values: a box made once, here)
        let b255 = match self.fin_box.take() {
            Some((b, side)) => nodes::encode_box(v, &b, &side),
            None => a(0),
        };
        let (inserts, marks, held) = (list_of(v, a(1)), list_of(v, a(2)), list_of(v, a(3)));
        let output_penalty = data.get(4).map_or(0, |&x| int_of(v, x));
        if crate::exec::obs_flag!("PHITEX_FIRE_TEXT") {
            // (the page's first letters, to tell the pages apart)
            fn text(list: &[Node], out: &mut String) {
                for n in list {
                    if out.len() >= 60 {
                        return;
                    }
                    match n {
                        Node::Char { ch, .. } => {
                            out.push(char::from_u32(u32::from(*ch)).unwrap_or('?'));
                        }
                        Node::Lig(l) => {
                            for &c in &l.original {
                                out.push(char::from_u32(u32::from(c)).unwrap_or('?'));
                            }
                        }
                        Node::Box(b) => {
                            text(&b.list, out);
                            out.push(' ');
                        }
                        Node::Glue(_) => out.push(' '),
                        _ => {}
                    }
                }
            }
            let mut t = String::new();
            if let Some(Node::Box(b)) = nodes::decode(v, b255, &mut Side::default()) {
                text(&b.list, &mut t);
            }
            eprintln!(
                "fire-text: node {} page {} penalty {output_penalty} held {} spec {}: {t:?}",
                self.cx.node().raw(),
                self.fin.as_ref().map_or(-1, |c| c.j),
                held.len(),
                self.cx.is_speculative()
            );
        }
        if let Some(c) = self.fin.as_mut() {
            c.held_n = held.len();
        }
        // §1012: the marks; those in force where the page began (every
        // class's `\botmark` before it) the page builder's, from its
        // arguments ([`FinishIn::top_marks`]), not the finisher before's
        let before: Vec<(i64, ValueId)> = FinishIn::of(v, &self.cx.args())
            .map(|f| f.top_marks)
            .filter(|&m| m != ValueId::UNIT)
            .map(|m| {
                list_of(v, m)
                    .into_iter()
                    .filter_map(|e| match list_of(v, e)[..] {
                        [c, m] => Some((int_of(v, c), m)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let bot = before.iter().find(|e| e.0 == 0).map(|e| e.1);
        // (TeX's `top_mark:=bot_mark` where one is; none before the first)
        let top = bot;
        let mut first = None;
        let mut new_bot = bot;
        let classes: Vec<ValueId> = before
            .iter()
            .filter(|e| e.0 != 0)
            .map(|&(c, m)| v.list(vec![v.int(c), m, ValueId::UNIT, m]))
            .collect();
        crate::etex::fire_marks_values(self, &marks, Some(&classes))?;
        for &m in &marks {
            if crate::etex::is_class_mark(v, Some(m)) {
                continue;
            }
            if first.is_none() {
                first = Some(m);
            }
            new_bot = Some(m);
        }
        if first.is_none() {
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
            let l = self.box_loc(n);
            self.set_cell(l, v.list(vec![v.int(0), b]), true);
        }
        self.set("param:outputpenalty", v.int(output_penalty), true);
        let output = self.par(Par::output)?;
        let mut toks = toks_of(v, output);
        // (`\deadcycles` where the page fired, as the page builder had it:
        // its arguments' ([`Run::finisher_args`]), not the finisher
        // before's definition)
        let dead = match self
            .fin
            .as_ref()
            .and_then(|c| c.args.get(13))
            .and_then(|&x| list_of(v, x).get(5).copied())
        {
            Some(x) => int_of(v, x),
            None => self.cint(crate::fields::cell::DEADCYCLES)?,
        };
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
    /// happen now (§1366–§1374). It defines nothing for the text after
    /// (spec-final §3.2: no shipped mark).
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
        let timed = crate::exec::prof_flag!("PHITEX_TEX_TIME");
        let t0 = std::time::Instant::now();
        let mut side = Side::default();
        // (the page's boxes are decoded once, here, for the writer: their
        // decodings are not kept for a later decode)
        // (its lines' lists as line breaking made them, where it kept
        // them, [`nodes::keep_made`])
        let prev = crate::val::NO_KEEP.with(|c| c.replace(true));
        let take = nodes::TAKE_MADE.with(|c| c.replace(true));
        let decoded = nodes::decode(v, b, &mut side);
        nodes::TAKE_MADE.with(|c| c.set(take));
        crate::val::NO_KEEP.with(|c| c.set(prev));
        if crate::exec::obs_flag!("PHITEX_MADE_STATS") {
            let n: Vec<u64> = nodes::MADE_STATS
                .iter()
                .map(|x| x.load(std::sync::atomic::Ordering::Relaxed))
                .collect();
            eprintln!(
                "made-stats: kept {} side {} full {} taken {} decoded {} lazy kept {} lazy not {}",
                n[0], n[1], n[2], n[3], n[4], n[5], n[6]
            );
        }
        let Some(Node::Box(bx)) = decoded else {
            return Ok(());
        };
        let t_decode = t0.elapsed();
        phitex_pdf::phase::add(1, t0);
        let t_p2 = std::time::Instant::now();
        // (the positions walk only for a whatsit that reads one: the
        // others act on their data; a page without one skips the walk,
        // the second copy of the box it makes and the font view)
        let needs_pos = side.whats.iter().any(|&w| {
            let xs = list_of(v, w);
            let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
            crate::pdfprim::reads_position_at_shipout(&kind)
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
        // (its sealed lines' fonts, from their stand-ins: their lists
        // are not read, `crate::pages::seal`)
        let sealed_fonts: Vec<u32> = side
            .lazy
            .iter()
            .filter_map(|&l| nodes::sealed_of(v, l))
            .flat_map(|s| list_of(v, s.fonts))
            .filter_map(|f| u32::try_from(int_of(v, f)).ok())
            .collect();
        let view = self.font_view_with(std::slice::from_ref(&page_box), &sealed_fonts)?;
        // (spec-final §3.2: the page's own font usage, by id; the end
        // node resolves names and files from the fonts' fields)
        let mut ids: Vec<u32> = view.fonts.ids();
        ids.sort_unstable();
        let fonts: Vec<ValueId> = ids.iter().map(|&f| v.int(i64::from(f))).collect();
        let cs = v.list(counts.iter().map(|&c| v.int(i64::from(c))).collect());
        let pdf = self.pdf_page_params(pos.pdf)?;
        let page = v.list(vec![b, cs, v.list(fonts), pdf, v.list(pdf_data)]);
        let t_fonts = t0.elapsed();
        phitex_pdf::phase::add(3, t_p3);
        let t_p4 = std::time::Instant::now();
        // (the page's hash: its value, what it is made of, known on the
        // write side every round; its PDF side runs once, at the end)
        // (the box handed over: the writer's piece keeps it, no copy)
        if pos.pdf
            && let Node::Box(bx) = page_box
        {
            crate::pdfout::ship(self, page, *bx, &side, &ids)?;
        }
        let shown = Some(page.raw());
        phitex_pdf::phase::add(4, t_p4);
        if timed {
            crate::exec::later(format!(
                "shipout: decode_us {} positions_us {} whatsits_us {} fonts_us {} pdf_us {}",
                t_decode.as_micros(),
                t_pos.checked_sub(t_decode).unwrap().as_micros(),
                t_whats.checked_sub(t_pos).unwrap().as_micros(),
                t_fonts.checked_sub(t_whats).unwrap().as_micros(),
                t0.elapsed().checked_sub(t_fonts).unwrap().as_micros()
            ));
        }
        let su = v.seq_unit(page);
        self.contribute("pages", su);
        if let Some(shown) = shown {
            let h = v.seq_unit(v.int(i64::from_ne_bytes(shown.to_ne_bytes())));
            self.contribute("pagehash", h);
        }
        self.log("shipend", "]")?;
        self.set_cell(crate::fields::cell::DEADCYCLES, v.int(0), true);
        phitex_pdf::phase::add(15, t0);
        Ok(())
    }

    /// The parameters pdfTeX reads at each page (pdfTeX §750–§757), with
    /// whether the file is PDF and the colour stacks made so far.
    pub(crate) fn pdf_page_params(&mut self, pdf: bool) -> R<ValueId> {
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
            let f = self.ops.fields.param(v, p);
            let toks = toks_of(v, self.get_f(f)?);
            let mut s = String::new();
            self.show_toks_into(&mut s, &toks)?;
            xs.push(v.str(&s));
        }
        Ok(v.list(xs))
    }
}

/// A finisher's unit of [`GDEFS`]: the fields its routine defined
/// globally, and where its page fired, the piece and the candidate
/// ([`fin_at`]).
pub(crate) struct GDef {
    names: Vec<ValueId>,
    pid: i64,
    k: i64,
}

/// Where a finisher's page fired ([`FinishIn::piece`], [`FinishIn::k`]):
/// the piece and its candidate; a page firing in the material the page
/// before left (the routine's, the material carried), the piece that page
/// fired in and a candidate after all of its own (`i64::MAX`).
pub(crate) fn fin_at(v: &Values, pos: &[ValueId]) -> (i64, i64) {
    FinishIn::of(v, pos).map_or((-2, i64::MAX), |f| {
        (
            i64::from_ne_bytes(f.piece.raw().to_ne_bytes()),
            if f.in_material { i64::MAX } else { i64::from(f.k) },
        )
    })
}

/// A finisher's [`GDEFS`] unit: `[names, piece, candidate]`, nothing
/// where it defined nothing globally.
pub(crate) fn gdefs_unit(v: &Values, names: ValueId, (pid, k): (i64, i64)) -> ValueId {
    if names == ValueId::NIL {
        return ValueId::NIL;
    }
    v.seq_unit(v.list(vec![names, v.int(pid), v.int(k)]))
}

/// Whether a group-local binding of `f` the run of `piece` made after
/// its candidate `after` (-1: before its first, or before the run) is
/// replaced by a global definition of one of the routines `gdefs` (§279:
/// at every level): one of a page firing in another piece's text, or in
/// this piece's at a candidate the binding was made before.
pub(crate) fn replaced(gdefs: &[GDef], f: Field, piece: i64, after: i64) -> bool {
    gdefs
        .iter()
        .any(|g| g.names.contains(&f.0) && (g.pid != piece || after < g.k))
}

impl Run<'_, '_> {
    /// The [`GDEFS`] units of the finishers between node `pid` and this
    /// one.
    pub(crate) fn gdefs_after(&mut self, pid: i64) -> R<Vec<GDef>> {
        let v = self.v;
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
        let acc = self.fold_own(GDEFS, Some(at), None)?;
        Ok(v.seq_to_vec(acc)
            .into_iter()
            .filter_map(|e| match list_of(v, e)[..] {
                [n, p, k] => Some(GDef {
                    names: list_of(v, n),
                    pid: int_of(v, p),
                    k: int_of(v, k),
                }),
                _ => None,
            })
            .collect())
    }

    /// The last of the run's candidates passed before the definition
    /// with stamp `st` (-1: before the first, or none).
    pub(crate) fn made_after(&self, st: Option<u64>) -> i64 {
        let Some(st) = st else { return -1 };
        let n = self.cand_marks.iter().filter(|m| m.stamp < st).count();
        i64::try_from(n).unwrap_or(0) - 1
    }
}

// -------------------------------------------------------- operation

/// A page's finisher (the module's doc): its arguments are its
/// [`FinishIn`] ([`arg`]); it reads what reaches its place as any node
/// does and defines what the routine defines; its page is shipped from
/// here.
pub(crate) struct Finish(pub Shared);

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
    /// Per field a run defined: the value the latest such run (by page)
    /// left, its page, and whether the run before that defined it to
    /// the same value ([`FinDefs::class_boxes`]).
    pub recent: Vec<(Field, ValueId, i64, bool)>,
    /// What the last run contributed to `gdefs` (the fields it defined
    /// globally, [`Run::contribute_global_defs`]): the next finisher's
    /// declared contribution, a guess ([`Finish::static_contribs`]).
    pub gdefs: Option<ValueId>,
    /// Each page's last run (by the page's index, the finisher's first
    /// argument): its ordinary pages fired and its definitions
    /// ([`FinDefs::at`]).
    pub pages: std::collections::BTreeMap<i64, (i64, Vec<(Field, ValueId)>)>,
}

impl FinDefs {
    /// The history a finisher of page `idx` predicts from: the page's
    /// own last run where it ran before (a finisher placed again with
    /// other arguments, a round after the first), both runs that one;
    /// else the last two pages before it; else the last two runs. Never
    /// pages after it: what a routine leaves on a later page (a column
    /// kept, a float pending, a page counter) is not what it leaves on
    /// this one.
    pub(crate) fn at(&self, idx: i64) -> FinDefs {
        let mut d = FinDefs {
            recent: self.recent.clone(),
            ..FinDefs::default()
        };
        if let Some((j, defs)) = self.pages.get(&idx) {
            d.last.clone_from(defs);
            d.prev.clone_from(defs);
            (d.last_j, d.prev_j) = (*j, *j);
            return d;
        }
        let mut before = self.pages.range(..idx).rev();
        if let Some((_, (lj, l))) = before.next() {
            d.last.clone_from(l);
            d.last_j = *lj;
            if let Some((_, (pj, pd))) = before.next() {
                d.prev.clone_from(pd);
                d.prev_j = *pj;
            }
        } else {
            d.last.clone_from(&self.last);
            d.prev.clone_from(&self.prev);
            (d.last_j, d.prev_j) = (self.last_j, self.prev_j);
        }
        d
    }

    /// The fields any run of the routine defined.
    pub(crate) fn names(&self) -> Vec<Field> {
        self.seen.clone()
    }

    /// The boxes of the insertion classes on a page (`classes`, a
    /// finisher's fourteenth argument): its fire defines each (§1018),
    /// and the value the routine leaves is predicted where the last two
    /// runs that defined it left the same (`\box\footins` emptied).
    pub(crate) fn class_boxes(&self, v: &Values, classes: Option<ValueId>) -> Vec<(Field, Option<ValueId>)> {
        let Some(cs) = classes else {
            return Vec::new();
        };
        list_of(v, cs)
            .iter()
            .filter_map(|&c| {
                let n = int_of(v, c);
                (0..255).contains(&n).then(|| {
                    let f = v.field(&format!("box:{n}"));
                    let x = self.recent.iter().find(|r| r.0 == f && r.3).map(|r| r.1);
                    (f, x)
                })
            })
            .collect()
    }

    /// The definitions both runs made with the same value: stable
    /// across pages, so a reader may run on them; a page number or a
    /// mark, changing every page, is not predicted (a wrong prediction
    /// runs its readers again).
    pub(crate) fn stable(&self, v: &Values) -> Vec<(Field, ValueId)> {
        self.last
            .iter()
            .filter(|d| {
                self.prev.contains(d)
                    && predictable(v, &crate::val::field_name(v, d.0), d.1, Some(d.1))
            })
            .copied()
            .collect()
    }

    /// [`FinDefs::stable`] for the finisher of page `j`, with the
    /// integers that step uniformly by page (`\count0`, a page
    /// counter) carried on to page `j` (spec-predicted-breaks §3).
    pub(crate) fn predict(&self, v: &Values, j: i64) -> Vec<(Field, ValueId)> {
        let dj = self.last_j - self.prev_j;
        self.last
            .iter()
            .filter_map(|&(f, x)| {
                // (the next finisher reads the outgoing state of this one
                // only as a prediction, spec-final §3.5: a value not
                // stable across pages the last run's, a guess that runs
                // its readers again where wrong; `\deadcycles` only as a
                // stable 0, [`predictable`], and no mark that changed,
                // [`varies_by_page`])
                let name = crate::val::field_name(v, f);
                let before = self.prev.iter().find(|d| d.0 == f).map(|d| d.1);
                if !predictable(v, &name, x, before) {
                    return None;
                }
                // (one run of the routine so far, spec-final §9b: its
                // values carried on, a page counter stepping by the pages
                // between; a wrong guess runs its readers again, a piece
                // never parks on a finisher)
                let Some(&(_, y)) = self.prev.iter().find(|d| d.0 == f) else {
                    if !name.starts_with("count:") {
                        return Some((f, x));
                    }
                    let b = v.as_int(x)?;
                    return Some((f, v.int(b.checked_add(j - self.last_j)?)));
                };
                if x == y {
                    return Some((f, x));
                }
                // (a mark or the penalty the page fired at that differed
                // between the last two runs: the page's own, not guessed)
                if varies_by_page(&name) {
                    return None;
                }
                // (registers only: a page counter; not a hash, a penalty)
                if !name.starts_with("count:") {
                    return Some((f, x));
                }
                let (Some(a), Some(b)) = (v.as_int(y), v.as_int(x)) else {
                    return Some((f, x));
                };
                let Some(d) = b.checked_sub(a) else {
                    return Some((f, x));
                };
                if dj <= 0 || d % dj != 0 {
                    return Some((f, x));
                }
                let x = (d / dj).checked_mul(j - self.last_j)?.checked_add(b)?;
                Some((f, v.int(x)))
            })
            .collect()
    }
}

/// Whether a finisher's definition `x` of field `name`, which the run
/// before left as `y` (`None`: no run before), is predicted
/// ([`FinDefs::predict`]). `\deadcycles` only as 0 where both runs left
/// it 0 (both shipped their page, §1023): the fire steps it and a
/// `\shipout` clears it (§1012), so what a run leaves depends on the
/// page, and at `\end` it decides whether the page chain goes on
/// (§1054). Guessed as 1, as a page's run left it that shipped nothing,
/// a dead cycle predicted the next page fired by `\end`, whose finisher
/// predicted another: the chain grew without end on guesses while the
/// page it stood on was still to run. A guessed 0 ends a chain at most,
/// which the run, where wrong, starts again.
pub(crate) fn predictable(v: &Values, name: &str, x: ValueId, y: Option<ValueId>) -> bool {
    name != "deadcycles" || (y == Some(x) && v.as_int(x) == Some(0))
}

/// The fields a page's own items decide (the marks, §1012, and the
/// penalty the page fired at, §1013): predicted only where the last two
/// runs left them equal (a document without marks), never as the last
/// run's value where they differed ([`FinDefs::predict`]).
pub(crate) fn varies_by_page(name: &str) -> bool {
    name.starts_with("mark") || name == "param:outputpenalty"
}

/// Whether a finisher with arguments `args` may define field `f`, as
/// far as its page's insertion classes tell (the fourteenth argument,
/// [`Run::place_break`]): a box register below 255 is defined by the
/// fire only for a class on the page (§1018), and by the routine only
/// where its text may (its hint `t`); any other field, and any field
/// where the classes are not known, may be.
pub(crate) fn fin_may_box(v: &Values, args: &[ValueId], t: ValueId, f: Field) -> bool {
    let Some(&cs) = args.get(13) else {
        return true;
    };
    let name = crate::val::field_name(v, f);
    let Some(n) = name
        .strip_prefix("box:")
        .and_then(|n| n.parse::<i64>().ok())
        .filter(|&n| (0..255).contains(&n))
    else {
        return true;
    };
    if list_of(v, cs).iter().any(|&c| int_of(v, c) == n) {
        return true;
    }
    match crate::hints::Hint::decode(&str_of(v, t)).may_define() {
        MayDefine::Nothing => false,
        MayDefine::Any => true,
        MayDefine::Prefixes(ps) => ps.iter().any(|p| name.starts_with(p.as_str())),
        MayDefine::Names(ns) => ns.contains(&f),
    }
}

/// Whether field `f` is a command's transient state (§1266's
/// `after_token`): what a routine leaves there depends on what reached it
/// (it publishes only a change, `crate::runstate`), not on its page, so a
/// finisher declares and predicts none of it; a run that defines it
/// files it at its commit.
pub(crate) fn transient(v: &Values, f: Field) -> bool {
    f == v.field("aftertoken")
}

/// What a finisher defines besides its routine's definitions
/// (`fire_page`, `output_end`, `ship_out`): field prefixes.
/// The fields every finisher defines itself, whatever its routine.
pub(crate) const FIN_OWN: [&str; 8] = [
    RTN,
    PAGE_START,
    "deadcycles",
    "param:outputpenalty",
    "inspen",
    "badness",
    "box:255",
    crate::pdfout::PAGE_KEYS,
];

pub(crate) const FINISHER_FIELDS: &[&str] = &[
    "deadcycles",
    PAGE_START,
    "mark:",
    "param:outputpenalty",
    "inspen",
    "box:",
    "badness",
    "write:",
    "pdf:",
];

/// A finisher's entry in `Ops::fin_defs`: its routine (the hint `t`)
/// and its page's class ([`pen_class`]), from its arguments `args` (an
/// ordinary page where they hold none).
pub(crate) fn fin_key(v: &Values, t: ValueId, args: &[ValueId]) -> u64 {
    let pen = FinishIn::of(v, args).map_or(i32::MAX, |f| pen_class(f.output_penalty));
    fin_key_class(t, pen)
}

/// The class of a page fired at `\outputpenalty` `p`: every ordinary
/// one (above -10001, §1013) alike, `i32::MAX`; a special one its own,
/// which its routine branches on (a float page, a cleared page): the
/// routine defines and gives back otherwise in each class, so each has
/// its own history ([`FinDefs`]).
pub(crate) fn pen_class(p: i32) -> i32 {
    if p > -10001 { i32::MAX } else { p }
}

/// [`fin_key`] of routine `t` on a page of class `pen`.
pub(crate) fn fin_key_class(t: ValueId, pen: i32) -> u64 {
    t.raw() ^ u64::from(pen.cast_unsigned()).rotate_left(32).wrapping_mul(0x9e37_79b9_7f4a_7c15)
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
    /// the main list ([`RTN`]): the last run's value, or nothing given
    /// back (the routine that ships its page, §1026). (The page builder
    /// guesses the same for itself, [`Run::rtn_guess`].)
    pub(crate) fn predicted_defs_(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        if !ops(&self.0)
            .fin_predict
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return None;
        }
        let t = args.get(9).copied().unwrap_or(ValueId::UNIT);
        let j = args.get(10).map_or(0, |&x| int_of(v, x));
        let o = ops(&self.0);
        let m = o.fin_defs.locked();
        let idx = args.first().map_or(0, |&x| int_of(v, x));
        let d = (t != ValueId::UNIT)
            .then(|| m.get(&fin_key(v, t, args)))
            .flatten()
            .map(|d| d.at(idx));
        let d = d.as_ref();
        let mut out = d
            .map(|d| if false { d.stable(v) } else { d.predict(v, j) })
            .unwrap_or_default();
        out.retain(|e| fin_may_box(v, args, t, e.0) && !transient(v, e.0));
        if let Some(d) = d {
            for (f, x) in d.class_boxes(v, args.get(13).copied()) {
                if let Some(x) = x
                    && !out.iter().any(|e| e.0 == f)
                {
                    out.push((f, x));
                }
            }
        }
        // (`PHITEX_PRED_SHOW=1`, observe: whether a finisher's
        // prediction has the page parameters)
        if crate::exec::obs_flag!("PHITEX_PRED_SHOW") {
            let pp = v.field("pagepar");
            eprintln!(
                "pred-show: {} defs, pagepar {}",
                out.len(),
                out.iter().any(|e| e.0 == pp)
            );
        }
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
    pub(crate) fn gdefs_guess(&self, args: &[ValueId], v: &Values) -> ValueId {
        let t = args.get(9).copied().unwrap_or(ValueId::UNIT);
        if t == ValueId::UNIT {
            return ValueId::NIL;
        }
        ops(&self.0)
            .fin_defs_start
            .locked()
            .get(&fin_key(v, t, args))
            .and_then(|d| d.gdefs)
            .map_or(ValueId::NIL, |x| gdefs_unit(v, x, fin_at(v, args)))
    }
}

impl Operation for Finish {
    fn name(&self) -> &'static str {
        "finish"
    }
    /// What the last run of the same routine defined, and the fields a
    /// finisher sets itself (spec-final §9b): a definition list, each
    /// with its prediction ([`Finish::predicted_defs`]), so that a reader
    /// after the finisher (the next finisher, the continuation reading
    /// [`RTN`]) finds its site and runs on the prediction instead of
    /// waiting; a run defining outside it widens it at its commit and
    /// its readers run again (a field the routine defines on some pages
    /// only is not claimed for every page). None before any run of the
    /// routine.
    fn defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let t = *args.get(9)?;
        if t == ValueId::UNIT {
            return None;
        }
        let mut names: Vec<Field> = ops(&self.0)
            .fin_defs
            .locked()
            .get(&fin_key(v, t, args))
            .map(|d| {
                // (the history this finisher predicts from, [`FinDefs::at`])
                let d = d.at(args.first().map_or(0, |&x| int_of(v, x)));
                let mut ns: Vec<Field> = d
                    .last
                    .iter()
                    .map(|e| e.0)
                    .filter(|&f| fin_may_box(v, args, t, f) && !transient(v, f))
                    .collect();
                for (f, _) in d.class_boxes(v, args.get(13).copied()) {
                    if !ns.contains(&f) {
                        ns.push(f);
                    }
                }
                ns
            })?;
        // (and what it gives back to the main list, always predicted)
        let rtn = v.field(RTN);
        if !names.contains(&rtn) {
            names.push(rtn);
        }
        // (the keys of the pages it ships, its PDF sides' read)
        names.push(v.field(crate::pdfout::PAGE_KEYS));
        ops(&self.0).fields.add_record_fields(&mut names);
        Some(names.into_iter().map(|f| (f, Locality::Global)).collect())
    }
    /// The output routine's hint (encoded, the tenth argument:
    /// [`crate::hints::routine_hint`]) and the fields a finisher defines
    /// itself; anything where the routine's text cannot be bounded, or
    /// the hint is not given.
    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        let Some(&t) = args.get(9) else {
            return MayDefine::Any;
        };
        if t == ValueId::UNIT {
            return MayDefine::Any;
        }
        // (a finisher ran this routine before: what it defined then,
        // by name, and the fields a finisher sets itself; a definition
        // outside them refutes the declaration, as any node's does, and
        // only its readers run again)
        let known = ops(&self.0)
            .fin_defs
            .locked()
            .get(&fin_key(v, t, args))
            .map(FinDefs::names);
        if let Some(mut names) = known {
            names.retain(|&f| fin_may_box(v, args, t, f));
            for n in FIN_OWN {
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
    /// No pieces' units ([`no_units`]), its global definitions as the
    /// last run of the routine gave them ([`Finish::gdefs_guess`]), and
    /// its register maps as the page builder guessed them.
    fn static_contribs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let mut out = no_units(v);
        out.push((v.field(GDEFS), self.gdefs_guess(args, v)));
        // (the maps of its routine's global integer register writes, as
        // the page builder guessed them, [`FinishIn::rtn_guess`]: the
        // page counter's readers fold past it while it runs,
        // `crate::regfold`)
        if let Some(fin) = FinishIn::of(v, args) {
            for (n, m) in rtn_maps(v, fin.rtn_guess) {
                out.push((crate::regfold::acc_field(v, n), m.encode(v)));
            }
        }
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
    /// What it folds ([`PageNode::folds`]) only: its break is its
    /// argument, and the material its page begins with ([`SCAN_MAT`]) it
    /// reads in its run, where the continuation before it is predicted
    /// ([`Scan::predicted_defs`]): the engine's gate takes no prediction.
    fn reads(&self, _: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(PageNode::Finish.reads(v, &[]))
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let t_fin = std::time::Instant::now();
        let _ = phitex_pdf::phase::take();
        let pos = cx.args();
        let o = ops(&self.0);
        let out = run_node_parking(cx, o, true, |r| {
            if crate::exec::obs_flag!("PHITEX_FIN_TRACE") {
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
                // (its page assembled from its input alone, the page
                // builder's decisions: [`Run::finisher_args`])
                let (args, brk) = match FinishIn::of(v, &pos) {
                    Some(fin) => r.finisher_args(&fin, &pos)?,
                    None => (pos.clone(), None),
                };
                // (a view in which the page's piece is not before this
                // node: no page, [`Run::finisher_args`])
                // (its output the guess the page builder folded on, which
                // leaves the builder as it is)
                if args.is_empty() {
                    r.fin = Some(FinCtx {
                        j: pos.first().map_or(0, |&x| int_of(v, x)),
                        pending: Vec::new(),
                        held_n: 0,
                        rtn: FinishIn::of(v, &pos).map(|f| f.rtn_guess),
                        args: Vec::new(),
                        brk: None,
                    });
                    give_no_units(r);
                    return Ok(());
                }
                let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
                r.fin = Some(FinCtx {
                    j: int_of(v, a(0)),
                    pending: Vec::new(),
                    held_n: 0,
                    rtn: None,
                    args: args.clone(),
                    brk,
                });
                r.preload_candidate(&args)?;
                if let Some(names) = crate::exec::obs_str!("PHITEX_FIN_SHOW") {
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
            r.give_way(&args)?;
            let g = r.contribute_global_defs();
            r.record_finisher_defs(&args, g)?;
            r.define_page_start()?;
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
            crate::exec::later(format!("finphase: {}", us.join(" ")));
        }
        out
    }
}

impl Run<'_, '_> {
    /// A finisher's state where its page fired, before the routine
    /// runs: the bindings local to the groups open at the candidate
    /// (its fourth argument), and what the piece defined after the
    /// candidate as it was there, where the piece had it (its ninth): the
    /// routine runs at the candidate, before those definitions. One the
    /// piece had not read stays as it reaches here.
    pub(crate) fn preload_candidate(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
        let locals = list_of(v, a(3));
        // (the routines of the pages firing in the piece before this one,
        // placed after it before this finisher: a binding they defined
        // globally is gone at every level where it was made before
        // their candidate, §279)
        let piece = self.fin.as_ref().and_then(|c| c.brk).map(|b| b.0);
        let gdefs = match piece {
            Some(p) if !locals.is_empty() => self.gdefs_after(p)?,
            _ => Vec::new(),
        };
        for e in locals {
            let ys = list_of(v, e);
            if let [f, x, ..] = ys[..] {
                let after = ys.get(2).map_or(-1, |&c| int_of(v, c));
                if replaced(&gdefs, Field(f), piece.unwrap_or(-2), after) {
                    continue;
                }
                // (a value read where the routine uses it, [`Run::fin_lazy_value`])
                if self.is_lazy(x) {
                    self.fin_lazy.insert(Field(f), x);
                    continue;
                }
                self.fin_lazy.remove(&Field(f));
                let loc = self.loc_of(Field(f));
                self.st.preload(loc, some(x));
            }
        }
        if args.len() < 9 {
            return Ok(());
        }
        let piece =
            phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(int_of(v, a(5)).to_ne_bytes()));
        let before = v.str(BEFORE_PIECE);
        for e in list_of(v, a(8)) {
            let ys = list_of(v, e);
            let [f, _, kv] = ys[..] else { continue };
            if kv == ValueId::UNIT {
                continue;
            }
            let x = if kv == before {
                match self.field_at(Field(f), piece, true)? {
                    phitex_defuse::FieldAt::Def(x, _) => Some(x),
                    phitex_defuse::FieldAt::Unset => None,
                    phitex_defuse::FieldAt::Gone => continue,
                }
            } else if self.is_lazy(kv) {
                self.fin_lazy.insert(Field(f), kv);
                continue;
            } else {
                list_of(v, kv).first().copied()
            };
            self.fin_lazy.remove(&Field(f));
            let loc = self.loc_of(Field(f));
            self.st.preload(loc, x);
        }
        Ok(())
    }

    /// The fields this finisher defined globally, for the resume after
    /// it ([`Run::cont_load`]): a group-local value the resume carries
    /// is replaced by a global definition even of an equal value.
    pub(crate) fn contribute_global_defs(&mut self) -> ValueId {
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
            v.list(names)
        };
        if crate::exec::obs_flag!("PHITEX_FIN_DEFS") {
            let all: Vec<String> = self
                .st
                .published(v, false)
                .into_iter()
                .map(|d| crate::val::field_name(v, self.loc_field(d.0)))
                .collect();
            let same: Vec<String> = self
                .st
                .published_as_read(v)
                .into_iter()
                .map(|l| crate::val::field_name(v, self.loc_field(l)))
                .collect();
            eprintln!(
                "fin-defs node {} spec {}: {} published, {} global as read: {}  ALL: {}",
                self.cx.node().raw(),
                self.cx.is_speculative(),
                all.len(),
                same.len(),
                same.join(" "),
                all.join(" ")
            );
        }
        let at = fin_at(v, &self.cx.args());
        self.contribute(GDEFS, gdefs_unit(v, x, at));
        x
    }

    /// The definitions this finisher publishes, kept for the next one
    /// running the same routine ([`Finish::predicted_defs`]).
    pub(crate) fn record_finisher_defs(&mut self, args: &[ValueId], gdefs: ValueId) -> R<()> {
        let v = self.v;
        let Some(&t) = args.get(9) else { return Ok(()) };
        // (a speculative run's definitions may be refuted: they would
        // seed the predictions of the finishers after it)
        if t == ValueId::UNIT || self.cx.is_speculative() {
            return Ok(());
        }
        let mut out = Vec::new();
        // (the records the run defines, as its readers read them whole:
        // the page builder reads the page parameters' record after a
        // finisher, and runs on its prediction, [`Run::ready_records`])
        self.ready_records()?;
        for &(f, x, l) in &self.rec_out {
            if !matches!(l, Locality::ScopeAt(_)) {
                out.push((f, x));
            }
        }
        for (loc, x, _) in self.st.published(v, self.rec_load) {
            let f = self.loc_field(loc);
            let name = crate::val::field_name(v, f);
            if is_page_field(&name) {
                continue;
            }
            let x = self.unseal(x);
            out.push((f, x));
        }
        if let Some(x) = self.fin.as_ref().and_then(|c| c.rtn) {
            out.push((v.field(RTN), x));
        }
        // (the ordinary pages fired up to this one: page counters step
        // by them)
        let j = args.get(10).map_or(0, |&x| int_of(v, x));
        // (`PHITEX_FIN_FIELD=name`: what each finisher's run left of that
        // field, for the predictions)
        if let Some(name) = crate::exec::obs_str!("PHITEX_FIN_FIELD") {
            let f = v.field(name);
            eprintln!(
                "fin-field page {j} node {}: {name} {}",
                self.cx.node().raw(),
                out.iter().find(|d| d.0 == f).map_or_else(
                    || "not defined".to_owned(),
                    |d| v.render(d.1).chars().take(80).collect()
                )
            );
        }
        let mut m = self.ops.fin_defs.locked();
        let pos = self.cx.args();
        let idx = pos.first().map_or(0, |&x| int_of(v, x));
        let e = m.entry(fin_key(v, t, &pos)).or_default();
        e.gdefs = Some(gdefs);
        for &(f, x) in &out {
            if !e.seen.contains(&f) {
                e.seen.push(f);
            }
            match e.recent.iter_mut().find(|r| r.0 == f) {
                Some(r) if j >= r.2 => *r = (f, x, j, r.1 == x),
                Some(_) => {}
                None => e.recent.push((f, x, j, true)),
            }
        }
        if crate::exec::obs_flag!("PHITEX_PRED_SHOW") {
            // (the page's class: the penalty it fired at, whether in a
            // routine's material, the dead cycles before it)
            let fin = FinishIn::of(v, &self.cx.args());
            let class = fin.as_ref().map_or_else(String::new, |f| {
                format!("pen {} mat {} prev_dead {} classes {:?} j {}", f.output_penalty, f.in_material, f.prev_fire.1, f.classes, f.j)
            });
            let r = |x: ValueId| v.render(x).chars().take(120).collect::<String>();
            let e = e.at(idx);
            for (f, x) in e.predict(v, j) {
                let got = out.iter().find(|d| d.0 == f).map(|d| d.1);
                if got != Some(x) {
                    let name = crate::val::field_name(v, f);
                    let prev = e.prev.iter().find(|d| d.0 == f).map(|d| d.1);
                    eprintln!(
                        "pred page {j} ({class}; last {} prev {}): {name} predicted {} got {} (last two {})",
                        e.last_j,
                        e.prev_j,
                        r(x),
                        got.map_or_else(|| "nothing".to_owned(), r),
                        if prev == e.last.iter().find(|d| d.0 == f).map(|d| d.1) { "equal" } else { "differ" }
                    );
                }
            }
        }
        e.pages.insert(idx, (j, out.clone()));
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
        Ok(())
    }
}

/// The tag of a value a finisher reads only where it uses it
/// ([`Run::fin_lazy_value`]): `[LAZY, unit, part, i]`, the own unit's
/// index in the window, its part (4 the check data, 5 the locals) and the
/// element.
const LAZY: &str = "\u{1}lazy";

impl Run<'_, '_> {
    /// The own piece's locals ([`Run::finisher_args`]): each `[field,
    /// value, after]`, the value a lazy tag: the run reads the fields and
    /// where they were made, a value only where the routine uses it.
    fn lazy_locals(&mut self, own: Option<(phitex_defuse::View, u32)>) -> Vec<ValueId> {
        use phitex_defuse::Step;
        let v = self.v;
        let Some((u, ui)) = own else {
            return Vec::new();
        };
        let Some(l) = self.cx.at(u, Step::Index(5)) else {
            return Vec::new();
        };
        let p0 = self.cx.path(&[Step::Index(0)]);
        let p2 = self.cx.path(&[Step::Index(2)]);
        let fs = self.cx.get(l, Step::Each(p0)).map(|x| list_of(v, x)).unwrap_or_default();
        let afters = self.cx.get(l, Step::Each(p2)).map(|x| list_of(v, x)).unwrap_or_default();
        let tag = v.str(LAZY);
        fs
            .iter()
            .enumerate()
            .map(|(j, &f)| {
                let lazy = v.list(vec![tag, v.int(i64::from(ui)), v.int(5), v.int(i64::try_from(j).unwrap_or(0))]);
                v.list(vec![f, lazy, afters.get(j).copied().unwrap_or_else(|| v.int(-1))])
            })
            .collect()
    }

    /// The own piece's names at candidate `k` from its check data's index
    /// ([`super::piece::names_index_at`]): each `[field, standing, value]`, a
    /// value a lazy tag where the piece had one; and whether the run
    /// could end there.
    fn lazy_names_at(&mut self, own: Option<(phitex_defuse::View, u32)>, k: u32) -> (ValueId, bool) {
        use phitex_defuse::Step;
        let v = self.v;
        let Some((u, ui)) = own else {
            return (ValueId::UNIT, false);
        };
        let Some(d) = self.cx.at(u, Step::Index(4)) else {
            return (ValueId::UNIT, false);
        };
        let Some(cuts) = self.cx.at(d, Step::Index(1)) else {
            return (ValueId::UNIT, false);
        };
        let Some(cut) = self.cx.get(cuts, Step::Index(k)) else {
            return (ValueId::UNIT, false);
        };
        let index = self.cx.get(d, Step::Index(2)).unwrap_or(ValueId::UNIT);
        let tag = v.str(LAZY);
        let before = v.str(BEFORE_PIECE);
        let names = super::piece::names_index_at(v, index, k)
            .into_iter()
            .map(|(f, g, kind, i)| {
                let kv = match kind {
                    0 => ValueId::UNIT,
                    1 => before,
                    _ => v.list(vec![
                        tag,
                        v.int(i64::from(ui)),
                        v.int(4),
                        v.int(i64::try_from(i * 5 + 2).unwrap_or(0)),
                    ]),
                };
                v.list(vec![f, g, kv])
            })
            .collect();
        (v.list(names), int_of(v, cut) != 0)
    }

    /// The value a lazily preloaded field takes ([`Run::preload_candidate`]):
    /// a local's value as it is, a name's `[value]` or `[]`.
    pub(crate) fn fin_lazy_preload(&mut self, x: ValueId) -> R<Option<ValueId>> {
        let v = self.v;
        let part = list_of(v, x).get(2).map_or(0, |&p| int_of(v, p));
        let y = self.fin_lazy_value(x)?;
        Ok(if part == 5 { some(y) } else { list_of(v, y).first().copied() })
    }

    /// Whether `x` is a value a finisher reads only where it uses it.
    pub(crate) fn is_lazy(&self, x: ValueId) -> bool {
        let v = self.v;
        v.with_list(x, |xs| xs.len() == 4 && v.with_str(xs[0], |s| s == LAZY).unwrap_or(false))
            .unwrap_or(false)
    }

    /// The value lazy tag `x` stands for ([`Run::lazy_locals`],
    /// [`Run::lazy_names_at`]), read through the window now: the run
    /// depends on that element alone.
    pub(crate) fn fin_lazy_value(&mut self, x: ValueId) -> R<ValueId> {
        use phitex_defuse::Step;
        let v = self.v;
        let xs = list_of(v, x);
        let n = |i: usize| u32::try_from(xs.get(i).map_or(0, |&y| int_of(v, y))).unwrap_or(u32::MAX);
        let pos = self.cx.args();
        let Some(fin) = FinishIn::of(v, &pos) else {
            return Ok(ValueId::UNIT);
        };
        let w = self.cx.acc_view(v.field(PITEMS), fin.after, Some(fin.piece))?;
        let Some(u) = self.cx.at(w, Step::Index(n(1))) else {
            return Ok(ValueId::UNIT);
        };
        let Some(p) = self.cx.at(u, Step::Index(n(2))) else {
            return Ok(ValueId::UNIT);
        };
        Ok(if n(2) == 5 {
            // (a local: `[field, value, after]`)
            self.cx
                .at(p, Step::Index(n(3)))
                .and_then(|e| self.cx.get(e, Step::Index(1)))
                .unwrap_or(ValueId::UNIT)
        } else {
            // (a name's value in the check data's stretches)
            self.cx
                .at(p, Step::Index(0))
                .and_then(|s| self.cx.get(s, Step::Index(n(3))))
                .unwrap_or(ValueId::UNIT)
        })
    }
}

/// The items of each piece finisher `fin`'s page names (its spans'
/// sources, sorted, once each): a unit of its window read at those alone
/// ([`Run::window_units`]).
fn named_items(fin: &FinishIn) -> phitex_defuse::FxHashMap<i64, Vec<u32>> {
    let mut named: phitex_defuse::FxHashMap<i64, Vec<u32>> = phitex_defuse::FxHashMap::default();
    for s in &fin.spans {
        for src in crate::pages::span_srcs(s) {
            let (piece, ix) = match src {
                Src::Piece { piece, item } => (piece, crate::pages::end_sub_of(item).map_or(item, |(e, _)| e)),
                Src::Held { piece, item, .. } => (piece, item),
                Src::Rtn { .. } => continue,
            };
            named.entry(piece).or_default().push(ix);
        }
    }
    for ixs in named.values_mut() {
        ixs.sort_unstable();
        ixs.dedup();
    }
    named
}
