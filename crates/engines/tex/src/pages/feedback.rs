//! Feedback up the layers (docs/spec-layers.md §3): every read by a
//! lower layer of what a higher one decides, named. Each is read on a
//! prediction or on the value reaching it, and validated where the
//! higher layer settles: only the readers that read wrong run again. No
//! case is a wait in a chain of nodes. A new case is added here, with
//! its mechanism, before any code reads across the layers otherwise.

use std::fmt::Write as _;

use super::{
    Contents, FoldState, IGNORE_DEPTH, MAX_DIMEN, PAGE_START, R, Run, RunReaching, ValueId,
    Values, int_of, list_of, nodes, params_of, some,
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
    /// last runs ([`FinDefs::predict`]). (Today's engine: the finishers
    /// are placed when the page builder's node ends, so the text before
    /// reads what reaches it without them and runs again where one
    /// changes it.)
    OutputState,
    /// What the routine gives back to the main list (§1026: its list, the
    /// page parameters it set) and `\deadcycles` as it leaves it where not
    /// 0 (a page not shipped, which §1054's `\end` after it reads): the
    /// page builder folds the next page on a guess
    /// ([`FinishIn::rtn_guess`]: what the same routine gave back on the
    /// last page fired on the text before it at the same
    /// `\outputpenalty`, `\deadcycles` always guessed 0, nothing for a
    /// page fired in a routine's material); a
    /// finisher whose routine gave otherwise contributes the real one to
    /// [`PAGE_MISS`], and the page builder's node runs again on it.
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

/// Where a page begins, as [`PAGE_START`] holds it.
pub(crate) struct PageStart {
    /// The piece the page before fired in: the text after it folds the
    /// profiles after it. `None`: the job's start.
    pub anchor: Option<phitex_defuse::NodeId>,
    pub j: i64,
    pub normal: i64,
    pub fold: FoldState,
    /// The page parameters in force (a `T_PAGE_PARAMS` marker).
    pub params: Option<ValueId>,
    /// What the page begins with, as a profile.
    pub prof: Vec<i64>,
}

impl PageStart {
    /// The value of [`PAGE_START`] (none: the job's start).
    pub(crate) fn decode(v: &Values, x: Option<ValueId>) -> PageStart {
        let xs = x.map(|x| list_of(v, x)).unwrap_or_default();
        if xs.len() < 8 {
            return PageStart {
                anchor: None,
                j: 0,
                normal: 0,
                fold: crate::build::IGNORE_DEPTH_STATE,
                params: None,
                prof: Vec::new(),
            };
        }
        let n = |i: usize| int_of(v, xs[i]);
        PageStart {
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
        }
    }

    /// The value of [`PAGE_START`].
    pub(crate) fn encode(&self, v: &Values) -> ValueId {
        let anchor = self
            .anchor
            .map_or(-1, |a| i64::from_ne_bytes(a.raw().to_ne_bytes()));
        v.list(vec![
            v.int(anchor),
            v.int(self.j),
            v.int(self.normal),
            v.int(i64::from(self.fold.sf)),
            v.int(i64::from(self.fold.pd)),
            v.int(i64::try_from(self.fold.since_bp).unwrap_or(0)),
            self.params.unwrap_or(ValueId::UNIT),
            crate::vprof::value(v, &self.prof),
        ])
    }

    /// The builder over profiles where the page began.
    pub(crate) fn state(&self, v: &Values) -> crate::vprof::ScanState {
        crate::vprof::ScanState::fresh(
            self.j,
            self.normal,
            self.fold,
            self.params.map(|m| params_of(v, m)),
        )
    }
}

impl Run<'_, '_> {
    /// `\pagegoal`, `\pagetotal`, … (§421, [`Feedback::PageSoFar`]): the
    /// page state reaching this piece with its own items up to its last
    /// candidate folded in (the contributions after it are not on the
    /// page), read, not defined.
    pub(crate) fn page_dimen(&mut self, m: i32) -> R<i32> {
        let (st, _) = self.page_so_far()?;
        if st.pb.contents == Contents::Empty {
            return Ok(if m == 0 { MAX_DIMEN } else { 0 });
        }
        Ok(st.pb.so_far[usize::try_from(m).unwrap_or(0).min(7)])
    }

    /// Whether a page fires at one of this piece's candidates so far
    /// ([`Feedback::PageSoFar`]: the page where the piece's page began,
    /// the profiles from there, its own items up to its last candidate):
    /// a page firing at none of them ships nothing before the commands
    /// after them (§1005), so what they read of the pages' objects is
    /// complete. True where a page fires before the piece (the fold does
    /// not know the routine's material after it).
    pub(crate) fn page_fires_in_own(&mut self) -> R<bool> {
        let (_, fired) = self.page_so_far()?;
        Ok(fired.is_some())
    }

    /// The page builder over the profiles where this piece's page began
    /// ([`PAGE_START`]) to its own last candidate: its state, stopped at
    /// the first page firing, and where that page fired (`Some(true)` in
    /// this piece's own items, `Some(false)` before them).
    fn page_so_far(&mut self) -> R<(crate::vprof::ScanState, Option<bool>)> {
        let v = self.v;
        let raw = self.own_outer_items();
        let last = raw
            .iter()
            .rposition(|&x| nodes::tag(v, x) == nodes::T_BUILD_PAGE)
            .map_or(0, |i| i + 1);
        let mut head = raw[..last].to_vec();
        self.resolve_page_params(&mut head)?;
        let s = PageStart::decode(v, self.eng_read(v.field(PAGE_START))?);
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
        let own = items.len() - 1;
        let mut rd = RunReaching(self, None);
        let mut fired = None;
        for (i, x) in items.iter().enumerate() {
            if st.fold_items(x, dead, &mut rd) {
                fired = Some(i == own);
                break;
            }
        }
        Ok((st, fired))
    }
}
