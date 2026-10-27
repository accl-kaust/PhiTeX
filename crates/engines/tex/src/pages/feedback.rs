//! Feedback up the layers (docs/spec-layers.md §3; `SPEC.md` §4): every
//! read of a lower layer's result by the text is a named case, read as
//! a value the lower layer defines, predicted where it is not there yet,
//! and never a wait.
//!
//! Each case is one function here, and each is as fine as its reader:
//! a piece that asks `\pagegoal` reads the page it is on as the parts the
//! finisher before defined ([`PAGE_START_ANCHOR`], [`PAGE_START_FOLD`],
//! [`PAGE_START_PARAMS`], [`PAGE_START_HEAD`]), each re-asking it alone
//! where it moves, and the fold over the profiles from the anchor. The
//! course's three chapters refuted pieces 294 times through
//! `pages:start` whole and 161 through `vprof` for those three
//! dimensions (`SPEC.md` §4.1).

use std::fmt::Write as _;

use super::{
    Contents, FoldState, IGNORE_DEPTH, Locality, MAX_DIMEN, R, Run, RunReaching, ValueId, Values, glue_of,
    glue_value, int_of, list_of, node_of, nodes, params_of, some,
};

/// The named cases of a lower layer reading a higher one's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Feedback {
    /// `\pagegoal`, `\pagetotal` and their kin read in the text (§421):
    /// the page so far where the piece is. The piece reads where its page
    /// began ([`PAGE_START`], defined by the finisher of the page before,
    /// or nothing before the first page's) and folds the profiles from
    /// there to itself ([`Run::page_dimen`]); a finisher placed or
    /// changed before it runs it again.
    PageSoFar,
    /// What an output routine defines, read by the text after its page
    /// (`\count0` and the page counters, a float list, a column's height,
    /// a mark's class registers): fields, defined by the finisher placed
    /// after the piece its page fired in, predicted from the routine's
    /// last runs ([`FinDefs::predict`]).
    OutputState,
    /// What the routine gives back to the main list (§1026: its list, the
    /// page parameters it set) and `\deadcycles` as it leaves it where not
    /// 0 (a page not shipped, which §1054's `\end` after it reads): the
    /// page builder folds the next page on a guess
    /// ([`Run::guess_give_back`]: what the same routine gave back on the
    /// last page fired on the text before it at the same
    /// `\outputpenalty`, `\deadcycles` always guessed 0, nothing for a
    /// page fired in a routine's material); a
    /// finisher's output is what its routine gave
    /// ([`phitex_defuse::Ctx::placed_output`]), and the page builder's
    /// node runs again where that is not the guess.
    RoutineGivesBack,
    /// The page parameters and an insertion class's registers (§980,
    /// §1008), set by text or by a routine: read by the page builder
    /// where each piece is ([`Run::page_params_at`],
    /// [`Run::insert_class_at`]), a read past the builder's own node.
    BuilderRegisters,
    /// The objects the pages shipped, read back in the text
    /// (`\pdflastobj`, `\pdflastlink`, `\pdfximage`'s number and their
    /// kin): the piece ends before the read ([`Run::order_after_pages`]),
    /// its rest a resume after the finishers of the pages firing in it.
    ObjectNumbers,
    /// What a paragraph's line breaking leaves the list it was on
    /// (`\prevgraf`, the last line for `\lastbox`, `\lastpenalty`,
    /// `\lastskip`, `\prevdepth`): the line-break node's definitions,
    /// predicted (`crate::parbreak::Break::predicted_defs`).
    LastLine,
    /// The width of a display's last line (§1146's `pre_display_size`):
    /// the display reads its paragraph's lines, so the lines before a
    /// display are broken in the piece itself, not by a node of their
    /// own (`crate::parbreak`).
    PreDisplay,
    /// A catcode change: the lines after it tokenized again under the
    /// regime it makes (layer 1's key).
    Catcodes,
    /// Page references, contents, an index, citations: the files a build
    /// writes and reads back (loop-back fields), the rounds.
    Rounds,
}

impl Feedback {
    /// The cases, in the order of docs/spec-layers.md §3.
    pub(crate) const ALL: [Feedback; 9] = [
        Feedback::PageSoFar,
        Feedback::OutputState,
        Feedback::RoutineGivesBack,
        Feedback::BuilderRegisters,
        Feedback::ObjectNumbers,
        Feedback::LastLine,
        Feedback::PreDisplay,
        Feedback::Catcodes,
        Feedback::Rounds,
    ];

    /// Counts a reader that read wrong and runs again (`PHITEX_TEX_TIME`
    /// reports them, [`Feedback::report`]).
    pub(crate) fn missed(self) {
        MISSES[self as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// `feedback: <case> <misses> …`, the cases with a miss.
    pub(crate) fn report() -> String {
        let mut out = String::from("feedback:");
        for c in Feedback::ALL {
            let n = MISSES[c as usize].load(std::sync::atomic::Ordering::Relaxed);
            if n > 0 {
                let _ = write!(out, " {c:?} {n}");
            }
        }
        out
    }
}

/// The misses per case ([`Feedback::missed`]).
static MISSES: [std::sync::atomic::AtomicU64; 9] = [const { std::sync::atomic::AtomicU64::new(0) }; 9];

/// The page a piece is on, where it begins: defined by the finisher of
/// the page before (`SPEC.md` §3.2 "where the next page begins"), read
/// by the piece as the definitions reaching it, in parts so a reader of
/// one re-asks on that one alone: the anchor (the piece the page before
/// fired in, and the candidate and window where it fired inside it), the
/// main list's fold after the fire, the parameters as the routine left
/// them, what the page begins with as a profile (held insertions, the
/// routine's list, the carried material, the rest of the piece).
pub(crate) const PAGE_START: &str = "pages:start";
/// `[anchor, k, after]`: the anchor's raw id (-1 none), the candidate
/// the page before fired at inside it and the window its finisher read
/// (-1 none: it fired at the anchor's end or in the material).
pub(crate) const PAGE_START_ANCHOR: &str = "pages:start:anchor";
/// `[sf, pd, since_bp]`.
pub(crate) const PAGE_START_FOLD: &str = "pages:start:fold";
/// A `T_PAGE_PARAMS` marker, unit for none.
pub(crate) const PAGE_START_PARAMS: &str = "pages:start:params";
/// The page's head as a profile ([`crate::vprof::value`]).
pub(crate) const PAGE_START_HEAD: &str = "pages:start:head";

/// Where a page begins, as [`PAGE_START`]'s parts hold it.
pub(crate) struct PageStart {
    /// The piece the page before fired in: the text after it folds the
    /// profiles after it. `None`: the job's start.
    pub anchor: Option<phitex_defuse::NodeId>,
    pub fold: FoldState,
    /// The page parameters in force (a `T_PAGE_PARAMS` marker).
    pub params: Option<ValueId>,
    /// What the page begins with, as a profile.
    pub prof: Vec<i64>,
    /// Where the page before fired at a candidate inside the anchor, the
    /// piece going on: the window its finisher read (after this node, to
    /// the anchor) and the candidate. The anchor's items after it begin
    /// the page after `prof`, read by the text after it, not the finisher.
    pub tail: Option<(Option<phitex_defuse::NodeId>, u32)>,
}

impl PageStart {
    /// The builder over profiles where the page began.
    pub(crate) fn state(&self, v: &Values) -> crate::vprof::ScanState {
        crate::vprof::ScanState::fresh(0, 0, self.fold, self.params.map(|m| params_of(v, m)))
    }
}

impl Run<'_, '_> {
    /// The page start reaching this node, part by part (none: the job's
    /// start).
    pub(crate) fn page_start(&mut self) -> R<PageStart> {
        let v = self.v;
        let part = |r: &mut Self, name: &str| -> R<Option<ValueId>> {
            let f = v.field(name);
            if crate::exec::obs_flag!("PHITEX_READS") {
                r.trace_read(f, false);
            }
            Ok(r.eng_read(f)?)
        };
        let a = part(self, PAGE_START_ANCHOR)?.map(|x| list_of(v, x)).unwrap_or_default();
        let fold = part(self, PAGE_START_FOLD)?.map(|x| list_of(v, x)).unwrap_or_default();
        let params = part(self, PAGE_START_PARAMS)?.and_then(some);
        let head = part(self, PAGE_START_HEAD)?;
        let n = |xs: &[ValueId], i: usize| xs.get(i).map_or(-1, |&x| int_of(v, x));
        Ok(PageStart {
            anchor: (n(&a, 0) != -1).then(|| node_of(n(&a, 0))),
            fold: if fold.len() == 3 {
                FoldState {
                    sf: i32::try_from(n(&fold, 0)).unwrap_or(1000),
                    pd: i32::try_from(n(&fold, 1)).unwrap_or(IGNORE_DEPTH),
                    since_bp: usize::try_from(n(&fold, 2)).unwrap_or(0),
                }
            } else {
                crate::build::IGNORE_DEPTH_STATE
            },
            params,
            prof: head.map(|x| list_of(v, x).into_iter().map(|y| int_of(v, y)).collect()).unwrap_or_default(),
            tail: (n(&a, 1) >= 0).then(|| ((n(&a, 2) != -1).then(|| node_of(n(&a, 2))), u32::try_from(n(&a, 1)).unwrap_or(0))),
        })
    }

    /// Where the next page begins, for the text after this finisher
    /// ([`PAGE_START`]'s parts, [`Feedback::PageSoFar`]): the piece the
    /// page fired in, the main list's fold and the page parameters after
    /// the fire (§1026: as the routine left them), and what the next
    /// page begins with as a profile: the held-over insertions, the
    /// routine's list ([`FinCtx::pending`]), the material carried and
    /// the rest of the piece after the candidate.
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
        // (§1026: the page parameters as the routine left them, the build
        // after it freezing the next page's specs on them)
        let set = self
            .own_outer_items()
            .into_iter()
            .rev()
            .find(|&x| nodes::tag(v, x) == nodes::T_PAGE_PARAMS && list_of(v, x).len() > 1);
        let params = set.or_else(|| xs.get(3).copied().and_then(some));
        let mut mat: Vec<ValueId> = c.pending.clone();
        mat.extend(c.args.get(2).map(|&x| list_of(v, x)).unwrap_or_default());
        mat.push(v.list(vec![v.int(nodes::T_BUILD_PAGE), v.int(0)]));
        mat.extend(xs.get(4).map(|&x| list_of(v, x)).unwrap_or_default());
        self.resolve_page_params(&mut mat)?;
        let anchor = c.brk.map_or(-1, |b| b.0);
        let (k, after) = if xs.len() > 7 && n(6) >= 0 { (n(6), n(7)) } else { (-1, -1) };
        let defs = [
            (PAGE_START_ANCHOR, v.list(vec![v.int(anchor), v.int(k), v.int(after)])),
            (PAGE_START_FOLD, v.list(vec![v.int(n(0)), v.int(n(1)), v.int(n(2))])),
            (PAGE_START_PARAMS, params.unwrap_or(ValueId::UNIT)),
            (PAGE_START_HEAD, crate::vprof::value(v, &crate::vprof::encode(v, &mat))),
        ];
        for (name, x) in defs {
            self.cx.define_with(v.field(name), x, Locality::Global);
        }
        Ok(())
    }

    /// The page parameters reaching this node, as a `T_PAGE_PARAMS`
    /// marker: what a `[tag]` marker of its own items stands for, the
    /// builds before its later settings (a setting at a candidate
    /// puts a marker of its values there).
    pub(crate) fn page_params_reaching(&mut self) -> R<ValueId> {
        let v = self.v;
        let mut xs = Vec::with_capacity(4);
        for p in crate::pages::PAGE_PARAMS {
            let f = self.loc_field(p as crate::state::Loc);
            xs.push(self.cx.read(f)?);
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
}

impl Run<'_, '_> {
    /// `\pagegoal`, `\pagetotal`, … (§421, [`Feedback::PageSoFar`]): the
    /// page state reaching this piece with its own items up to its last
    /// candidate folded in (the contributions after it are not on the
    /// page), read, not defined.
    pub(crate) fn page_dimen(&mut self, m: i32) -> R<i32> {
        let (st, fired, own_k) = self.page_so_far()?;
        if fired == Some(true)
            && let Some(k) = own_k
        {
            self.order_after_own_page(k)?;
        }
        if st.pb.contents == Contents::Empty {
            return Ok(if m == 0 { MAX_DIMEN } else { 0 });
        }
        Ok(st.pb.so_far[usize::try_from(m).unwrap_or(0).min(7)])
    }

    /// `\insertpenalties` (§419) outside the output routine: the page
    /// builder's sum of the split insertions' penalties on the page so far
    /// (§1008), as [`Run::page_dimen`] reads the page; inside the routine,
    /// the count of held-over insertions the routine's run keeps (§1018).
    pub(crate) fn insert_penalties(&mut self) -> R<i32> {
        if self.fin.is_some() {
            return self.int_loc(crate::fields::cell::INSPEN);
        }
        let (st, fired, own_k) = self.page_so_far()?;
        if fired == Some(true)
            && let Some(k) = own_k
        {
            self.order_after_own_page(k)?;
        }
        Ok(st.pb.insert_penalties)
    }

    /// Whether a page fires at one of this piece's candidates so far
    /// ([`Feedback::PageSoFar`]: the page where the piece's page began,
    /// the profiles from there, its own items up to its last candidate):
    /// a page firing at none of them ships nothing before the commands
    /// after them (§1005), so what they read of the pages' objects is
    /// complete. A page firing before the piece's own items says the page
    /// start read is not yet the finisher's placed before the piece (its
    /// page fired after that start): the run goes again when it is, and
    /// decides then.
    pub(crate) fn page_fires_in_own(&mut self) -> R<bool> {
        let (_, fired, _) = self.page_so_far()?;
        Ok(fired == Some(true))
    }

    /// The page builder over the profiles where this piece's page began
    /// ([`PAGE_START`]) to its own last candidate: its state, stopped at
    /// the first page firing, and where that page fired (`Some(true)` in
    /// this piece's own items, `Some(false)` before them), and the own
    /// candidate it fired at.
    fn page_so_far(&mut self) -> R<(crate::vprof::ScanState, Option<bool>, Option<u32>)> {
        let v = self.v;
        let raw = self.own_outer_items();
        let last = raw
            .iter()
            .rposition(|&x| nodes::tag(v, x) == nodes::T_BUILD_PAGE)
            .map_or(0, |i| i + 1);
        let mut head = raw[..last].to_vec();
        // (its `[tag]` markers the parameters reaching it: a setting since
        // its last candidate is not the builds')
        let mut reach = None;
        for x in &mut head {
            if nodes::tag(v, *x) == nodes::T_PAGE_PARAMS && list_of(v, *x).len() == 1 {
                *x = match reach {
                    Some(m) => m,
                    None => *reach.insert(self.page_params_reaching()?),
                };
            }
        }
        self.resolve_page_params(&mut head)?;
        let s = self.page_start()?;
        let units = v.seq_to_vec(self.cx.fold(v.field(crate::vprof::VPROF), s.anchor, None)?);
        let dead = self.cint(crate::fields::cell::DEADCYCLES)?;
        let mut st = s.state(v);
        let mut items = vec![crate::vprof::decode(&s.prof)];
        if let (Some((after, k)), Some(a)) = (s.tail, s.anchor)
            && a != self.cx.node()
        {
            // (the anchor's items after the candidate the page fired at,
            // its unit's alone read)
            let w = self.view_through(crate::vprof::VPROF, after, a)?;
            if let Some(u) = self.cx.get(w, phitex_defuse::Step::Last) {
                let (pid, ints) = crate::vprof::unit_parts(v, u);
                if pid == i64::from_ne_bytes(a.raw().to_ne_bytes()) {
                    let mut x = crate::vprof::decode(&ints);
                    if let Some(p) = x
                        .iter()
                        .position(|y| matches!(y, crate::vprof::PItem::Build(c) if *c == k))
                    {
                        x.drain(..=p);
                        self.resolve_prof_of(pid, &mut x)?;
                        items.push(x);
                    }
                }
            }
        }
        for &u in &units {
            let (pid, ints) = crate::vprof::unit_parts(v, u);
            let mut x = crate::vprof::decode(&ints);
            self.resolve_prof_of(pid, &mut x)?;
            items.push(x);
        }
        items.push(crate::vprof::decode(&crate::vprof::encode(v, &head)));
        let own = items.len() - 1;
        let mut rd = RunReaching(self, None);
        let mut fired = None;
        let mut own_k = None;
        'items: for (i, x) in items.iter().enumerate() {
            let mut rest: &[crate::vprof::PItem] = x;
            let mut n_fires = 0;
            while st.fold_items(rest, dead, &mut rd) {
                n_fires += 1;
                // (the candidate the first page fired at in its own items:
                // its build)
                if fired.is_none() && i == own {
                    own_k = match rest.get(st.fired_at) {
                        Some(crate::vprof::PItem::Build(c)) => Some(*c),
                        _ => None,
                    };
                }
                fired = Some(i == own);
                if i != own || n_fires > 1000 {
                    break 'items;
                }
                // (a page fired in its own items: its routine run, the
                // build after it (§1026) takes what is left, the routine
                // giving back nothing; the page the text goes on with)
                rest = &rest[st.fired_at..];
                st.resume();
            }
        }
        // (§1245: a page dimension set since its last candidate, in this
        // paragraph, stands for what it reads after it)
        if fired != Some(true) {
            for &x in &raw[last..] {
                if nodes::tag(v, x) == nodes::T_SET_PAGE {
                    let i = usize::try_from(crate::val::int_at(v, x, 1)).unwrap_or(8);
                    if i < 8 {
                        st.pb.so_far[i] = i32::try_from(crate::val::int_at(v, x, 2)).unwrap_or(0);
                    }
                }
            }
        }
        Ok((st, fired, own_k))
    }
}
