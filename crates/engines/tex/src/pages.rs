//! Pages: where the document's pages break and where their output
//! routines run, as pieces of the graph.
//!
//! Text pieces (`exec`) run whole and in parallel. Each item a piece puts
//! on the outer vertical list carries a [`Cand`]: the piece, and the
//! index `k` of that moment among the piece's finish candidates (the
//! moments TeX looks for a full page: a paragraph's start and end, an item
//! between paragraphs, after a display, §1091 §1096 §1100 §1103 §1200).
//!
//! The driver ([`settle`]) evaluates the graph, breaks the whole item
//! list into pages ([`break_pages`], a pure function: Knuth's cost rule,
//! §980–§1008), and places one finisher piece ([`Finish`]) per page where
//! TeX finishes it ([`plan`], [`place_finishers`]):
//!
//! - at a piece's last candidate: the finishers right after the piece;
//! - inside a piece, at candidate `k`: the piece re-run with
//!   [`stop_arg`]`(k)` (it stops after its k-th candidate and defines its
//!   continuation, [`cont_field`]), the finishers, then a [`Resume`]
//!   piece that reads the continuation and runs the rest.
//!
//! A finisher runs the output routine on its page's items and defines
//! its `\global` assignments where it stands (its local ones end with the
//! routine's group, §1025). Every value moves by def-use: text before a
//! finisher never sees it, text after it reads what it set. Nothing reads
//! "the state after a whole piece", nothing is fed back to a later round,
//! nothing is replayed.
//!
//! Before the first placement no finisher exists: a piece reading page
//! state (`\c@page`, the float lists) gets what reaches it, the
//! preamble's values; once the finishers stand, the readers whose
//! reaching value changed run again, and only those. The driver repeats
//! until the breaks stop moving (a re-run reader changed a height, or a
//! finisher changed what the next page's break needs: `\vsize`, held
//! insertions).

use phitex_defuse::{
    Ctx, Engine, Field, FxHashMap, Locality, MayDefine, NodeId, OpId, Operation, Stop, UnitId, ValueId, Values,
};
use std::collections::BTreeMap;

use crate::run::{R, Run};

/// A finish candidate: the piece that made it and its index among that
/// piece's candidates (in the piece's own text: a resume piece goes on
/// counting where the piece it resumes stopped).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct Cand {
    pub piece: NodeId,
    pub k: u32,
}

/// One item on the outer vertical list, as the breaker sees it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Item {
    /// The candidate it arrived with: the page builder looks at the page
    /// right after it.
    pub at: Cand,
    pub kind: ItemKind,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum ItemKind {
    Box { height: i32, depth: i32 },
    Glue { width: i32, stretch: i32, shrink: i32 },
    Kern(i32),
    Penalty(i32),
    Insert { class: u16, height: i32 },
    Mark { class: u16, text: ValueId },
    Other,
}

/// What the next page's break needs that a finisher may change: the
/// page parameters as it left them (§980) and what it put back on the
/// list (held insertions, a float it deferred).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct PageState {
    pub vsize: i32,
    pub max_depth: i32,
    pub put_back: Vec<Item>,
}

/// One page as the breaker found it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct PageBreak {
    /// The page's items: `items[start..end]`.
    pub start: usize,
    pub end: usize,
    /// Where TeX finishes it: the candidate after which the page builder
    /// found the page full, or met a penalty of -10000 or less.
    pub finish: Cand,
}

/// Knuth's page breaking over the whole item list: each page from where
/// the last one ended, under what page `j-1`'s finisher left
/// (`states[j-1]`, from the last settle; the text's parameters `first`
/// for page 0 and for a page whose finisher has not run yet).
pub(crate) fn break_pages(items: &[Item], first: &PageState, states: &[PageState]) -> Vec<PageBreak> {
    let mut pages = Vec::new();
    let mut start = 0;
    let mut st = first;
    while start < items.len() {
        let Some((len, finish)) = break_one(&items[start..], st) else {
            break;
        };
        pages.push(PageBreak {
            start,
            end: start + len,
            finish,
        });
        start += len.max(1);
        st = states.get(pages.len() - 1).unwrap_or(first);
    }
    pages
}

/// One page from `items[0]`: how many items it takes and where it
/// finishes (`None`: the items run out before the page is full and no
/// end of the job forces it).
fn break_one(items: &[Item], st: &PageState) -> Option<(usize, Cand)> {
    let _ = (items, st);
    todo!(
        "§980–§1008 with phitex_typeset::page::PageBuilder: insertions charged, \
         the best break kept; the page ends at the best break, it finishes at the \
         candidate of the item at which the cost became awful_bad or a penalty <= -10000 \
         arrived; st.put_back goes first"
    )
}

impl Run<'_, '_> {
    /// A finish candidate: TeX looks for a full page here (`build_page`
    /// at the outer level, §1091 §1096 §1100 §1103 §1200). The run counts
    /// it (the items it puts on the outer list from here on carry
    /// `[piece, k]`); at its stop ([`stop_of`] of its arguments, or the
    /// resume piece's) it defines its continuation ([`cont_field`]) and
    /// ends.
    pub(crate) fn finish_candidate(&mut self) -> R<()> {
        todo!("k += 1; tag the next items [piece, k]; at the stop: define cont:<piece> (the pending input relative to the piece's region, the open groups) and end the run")
    }

    /// `\\shipout` (§1023, §638): in a finisher, the page value it
    /// outputs; anywhere else the same, a page of its own.
    pub(crate) fn ship_out(&mut self, b: ValueId) -> R<()> {
        let _ = b;
        todo!("the shipped box as a page value of the finisher (no PDF in the skeleton)")
    }

    /// `\\pagegoal`, `\\pagetotal`, … (§421, §982): the page builder's
    /// totals as they stand at this candidate.
    pub(crate) fn page_dimen<M>(&mut self, m: M) -> R<i32> {
        let _ = m;
        todo!("the totals of the page open at this candidate: from the last break, by Cand (a read the driver re-runs when the break moves)")
    }

    /// The output routine's group ends (§1100): in a finisher, its
    /// `\\global`s stand, its locals end; the page's leftovers go back.
    pub(crate) fn output_end<M>(&mut self, md: M) -> R<()> {
        let _ = md;
        todo!("§1100: check \\box255 is void, the routine's put-back material to the finisher's output, end the run")
    }

    /// The page a finisher finishes (its argument `[j]`).
    pub(crate) fn page_index(&self) -> usize {
        todo!("a finisher's argument j")
    }
}

/// Where the finishers stand, by text piece: the candidates inside it
/// at which pages finish and how many pages finish at each (in order).
/// A candidate equal to the piece's last means after the piece.
pub(crate) type Plan = BTreeMap<NodeId, Vec<(u32, Vec<usize>)>>;

/// The plan for `pages`.
pub(crate) fn plan(pages: &[PageBreak]) -> Plan {
    let mut p: Plan = BTreeMap::new();
    for (j, pg) in pages.iter().enumerate() {
        let at = p.entry(pg.finish.piece).or_default();
        match at.last_mut() {
            Some((k, js)) if *k == pg.finish.k => js.push(j),
            _ => at.push((pg.finish.k, vec![j])),
        }
    }
    p
}

/// The last candidate of each piece on the list (a page finishing there
/// needs no split).
pub(crate) fn last_cands(items: &[Item]) -> FxHashMap<NodeId, u32> {
    let mut last = FxHashMap::default();
    for it in items {
        let e = last.entry(it.at.piece).or_insert(it.at.k);
        *e = (*e).max(it.at.k);
    }
    last
}

/// The operations the driver places.
#[derive(Clone, Copy)]
pub(crate) struct PageOps {
    pub exec: OpId,
    pub finish: OpId,
    pub resume: OpId,
}

/// What the driver placed for one text piece: its full text (its `exec`
/// arguments, no stop), the plan it placed, and the pieces it added
/// after it (finishers and resume pieces, in line order).
struct Placed {
    text: Vec<ValueId>,
    at: Vec<(u32, Vec<usize>)>,
    added: Vec<NodeId>,
}

/// The finishers in the line, by the text piece they follow or split.
#[derive(Default)]
pub(crate) struct Placement {
    placed: FxHashMap<NodeId, Placed>,
    /// Page `j`'s finisher.
    finisher: BTreeMap<usize, NodeId>,
}

/// The last argument of a piece that stops after its `k`-th candidate.
pub(crate) fn stop_arg(v: &Values, k: u32) -> ValueId {
    v.list(vec![v.str("stop"), v.int(i64::from(k))])
}

/// The candidate a piece stops after (its last argument, [`stop_arg`]).
pub(crate) fn stop_of(v: &Values, args: &[ValueId]) -> Option<u32> {
    let xs = crate::val::list_of(v, *args.last()?);
    match xs[..] {
        [t, k] if v.as_str(t).as_deref() == Some("stop") => v.as_int(k).and_then(|k| u32::try_from(k).ok()),
        _ => None,
    }
}

/// The field a piece stopped at a candidate defines: its continuation
/// (the text still to run, relative to its own region, and the groups
/// still open), which the resume piece after it reads.
pub(crate) fn cont_field(v: &Values, piece: NodeId) -> Field {
    v.field(&format!("cont:{:x}", piece.raw()))
}

/// The line brought to `p` (between settles): a piece whose plan
/// changed is put back whole and its added pieces removed, then placed
/// again. Text pieces of an unchanged plan are not touched.
pub(crate) fn place_finishers(
    eng: &mut Engine,
    pl: &mut Placement,
    p: &Plan,
    last: &FxHashMap<NodeId, u32>,
    ops: PageOps,
) {
    // pieces no longer split or followed: whole again
    let gone: Vec<NodeId> = pl.placed.keys().filter(|n| !p.contains_key(n)).copied().collect();
    for n in gone {
        unplace(eng, pl, n, ops);
    }
    for (&piece, at) in p {
        if pl.placed.get(&piece).is_some_and(|x| &x.at == at) {
            continue;
        }
        let text = if pl.placed.contains_key(&piece) {
            unplace(eng, pl, piece, ops)
        } else {
            todo!("the piece's exec arguments, from the incremental layer's region table")
        };
        let mut added = Vec::new();
        let mut after = piece;
        let end = last.get(&piece).copied().unwrap_or(0);
        for (i, (k, js)) in at.iter().enumerate() {
            let split = *k < end;
            if i == 0 && split {
                // the piece stops after its k-th candidate
                let mut args = text.clone();
                args.push(stop_arg(eng.values(), *k));
                eng.replace(piece, ops.exec, args);
            }
            for &j in js {
                let a = vec![eng.values().int(j as i64)];
                let f = eng.insert_after(after, ops.finish, a);
                pl.finisher.insert(j, f);
                added.push(f);
                after = f;
            }
            if split {
                // the rest of the piece, up to its next stop if any
                let v = eng.values();
                let mut args = vec![v.int(i64::from_ne_bytes(piece.raw().to_ne_bytes()))];
                if let Some((k2, _)) = at.get(i + 1)
                    && *k2 < end
                {
                    args.push(stop_arg(v, *k2));
                }
                let _ = v;
                let r = eng.insert_after(after, ops.resume, args);
                added.push(r);
                after = r;
            }
        }
        pl.placed.insert(
            piece,
            Placed {
                text,
                at: at.clone(),
                added,
            },
        );
    }
}

/// `piece` whole again, what was added after it removed: its text.
fn unplace(eng: &mut Engine, pl: &mut Placement, piece: NodeId, ops: PageOps) -> Vec<ValueId> {
    let Some(x) = pl.placed.remove(&piece) else {
        return Vec::new();
    };
    for n in x.added {
        eng.remove(n);
    }
    pl.finisher.retain(|_, f| eng.is_live(*f));
    eng.replace(piece, ops.exec, x.text.clone());
    x.text
}

/// The items on the outer vertical list after a settle, with their
/// candidates.
pub(crate) fn collect_items(eng: &Engine, unit: UnitId) -> Vec<Item> {
    let _ = (eng, unit);
    todo!("the `items` sequence of `unit`, each entry tagged [piece, k] by the run that put it there")
}

/// What each placed finisher left for the next page's break (its
/// output), by page.
pub(crate) fn finisher_states(eng: &Engine, pl: &Placement) -> Vec<PageState> {
    pl.finisher
        .values()
        .map(|&f| eng.output(f).map(|x| page_state(eng.values(), x)).unwrap_or_default())
        .collect()
}

fn page_state(v: &Values, x: ValueId) -> PageState {
    let _ = (v, x);
    todo!("decode a finisher's output: [vsize, max_depth, put-back items]")
}

/// The most times the driver breaks and places before giving up.
const MAX_SETTLES: usize = 16;

/// The document's pages settled: evaluate, break, place the finishers,
/// until the breaks stop moving. `evaluate` is one engine evaluation
/// (incremental: only what changed runs).
pub(crate) fn settle(
    eng: &mut Engine,
    unit: UnitId,
    pl: &mut Placement,
    first: &PageState,
    ops: PageOps,
    evaluate: &mut dyn FnMut(&mut Engine) -> Result<(), String>,
) -> Result<Vec<PageBreak>, String> {
    let mut last_pages: Option<Vec<PageBreak>> = None;
    for _ in 0..MAX_SETTLES {
        evaluate(eng)?;
        let items = collect_items(eng, unit);
        let states = finisher_states(eng, pl);
        let pages = break_pages(&items, first, &states);
        if last_pages.as_ref() == Some(&pages) {
            return Ok(pages);
        }
        place_finishers(eng, pl, &plan(&pages), &last_cands(&items), ops);
        last_pages = Some(pages);
    }
    Err("pages: the breaks did not settle".to_owned())
}

/// A page's finisher: TeX's output routine for page `j` (§1012–§1025)
/// where the page finishes. Argument `[j]`; it reads page `j`'s items
/// (`items[start..end]` of the last break, the driver's), and anything
/// else as any piece does: what reaches its place (the text's values
/// before it, earlier finishers' `\global`s). It defines its `\global`
/// assignments; its output is the [`PageState`] the next break needs.
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
            "box255 from page j's items, marks and insertions (§1012–§1022); the output \
             routine run in a group (§1025): its \\global assignments published, its local \
             ones dropped; \\shipout a page value; output [vsize, max_depth, put-back items]"
        )
    }
}

/// The rest of a piece after it stopped: argument `[piece, stop?]`; it
/// reads the piece's continuation ([`cont_field`]) and runs it, counting
/// candidates on from there, stopping too if it has a stop.
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
        todo!("read cont:<piece>, run the pending text in its open groups; at a stop define cont:<self's piece> again")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: u64) -> NodeId {
        NodeId::from_raw(x)
    }
    fn it(piece: u64, k: u32, kind: ItemKind) -> Item {
        Item {
            at: Cand { piece: n(piece), k },
            kind,
        }
    }

    /// Three pieces:
    ///
    /// ```tex
    /// Paragraph A ... (fills page 1 nearly to the bottom)    % piece 1
    ///
    /// \newpage Paragraph B: we are on page \thepage.         % piece 2
    ///
    /// Paragraph C: this is page \thepage.                    % piece 3
    /// \end{document}
    /// ```
    ///
    /// Page 1 finishes at the `\newpage` penalty, candidate 0 of piece 2:
    /// piece 2 splits there (stop at 0, finisher of page 0, resume), so
    /// B's `\thepage` runs after the finisher and reads 2, as does C's.
    /// Page 2 finishes at the end of the job, piece 3's last candidate:
    /// its finisher follows piece 3, no split.
    #[test]
    #[ignore = "skeleton: break_one is todo"]
    fn three_paragraphs() {
        let line = || ItemKind::Box {
            height: 10 << 16,
            depth: 0,
        };
        let items = vec![
            it(1, 0, ItemKind::Glue {
                width: 0,
                stretch: 0,
                shrink: 0,
            }),
            it(1, 1, line()),
            it(2, 0, ItemKind::Penalty(-10000)),
            it(2, 1, ItemKind::Glue {
                width: 0,
                stretch: 1 << 16,
                shrink: 0,
            }),
            it(2, 2, line()),
            it(3, 0, ItemKind::Glue {
                width: 0,
                stretch: 1 << 16,
                shrink: 0,
            }),
            it(3, 1, line()),
            it(3, 2, ItemKind::Penalty(-0o10000000000)),
        ];
        let first = PageState {
            vsize: 100 << 16,
            max_depth: 2 << 16,
            put_back: Vec::new(),
        };
        let pages = break_pages(&items, &first, &[]);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].finish, Cand { piece: n(2), k: 0 });
        assert_eq!(pages[1].finish, Cand { piece: n(3), k: 2 });
        let p = plan(&pages);
        let last = last_cands(&items);
        // piece 2: split after candidate 0 (not its last), page 0's
        // finisher between the halves
        assert_eq!(p[&n(2)], vec![(0, vec![0])]);
        assert!(p[&n(2)][0].0 < last[&n(2)]);
        // piece 3: page 1's finisher after it, no split
        assert_eq!(p[&n(3)], vec![(2, vec![1])]);
        assert_eq!(last[&n(3)], 2);
        // (the page numbers B and C print, 2 and 2, need the engine run:
        // the end-to-end test once Finish and Resume run)
    }
}
