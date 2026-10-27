//! Feedback up the layers (docs/spec-layers.md §3; `SPEC.md` §4): every
//! read of a lower layer's result by the text is a named case, read as
//! a value the lower layer defines, predicted where it is not there yet,
//! and never a wait.
//!
//! Each case is one function here, and each is as fine as its reader:
//! a piece that asks `\pagegoal` reads one dimension of the page it is
//! on, not the whole page record and the whole profile fold. The
//! course's three chapters refuted pieces 294 times through `pages:start`
//! and 161 through `vprof` for those three dimensions (`SPEC.md` §4.1):
//! the cost this module removes.

use phitex_defuse::ValueId;

use crate::run::{R, Run};

/// The named cases, counted in profile builds (`Feedback::count`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Feedback {
    /// `\pagegoal`, `\pagetotal`, …, §421: the page the piece is on, as
    /// far as its own last candidate.
    PageSoFar,
    /// A number of an object the pages shipped before this piece made
    /// (`\pdfobj`, links, images): the piece ends before the command on a
    /// second pass; the number is then the finishers' before it.
    ObjectNumbers,
    /// What a routine defined, read by the text after its page: an
    /// ordinary field read past the finisher, on its prediction.
    OutputState,
    /// The page parameters and insertion registers the builder reads at
    /// a piece (`[tag]` markers): the records at the piece.
    BuilderRegisters,
    /// What a routine gives back, read by the loop before the finisher
    /// ran: the guess.
    RoutineGivesBack,
}

impl Feedback {
    pub(crate) fn count(self) {
        todo!("a profile counter per case (crate::prof), as the old feedback.rs")
    }
}

/// The page a piece is on, where it begins: defined by the finisher of the
/// page before as `pages:start` (`SPEC.md` §3.2 "where the next page
/// begins"), read by the piece as the definition reaching it. Its parts
/// are separate fields so a reader of one re-asks on that one alone:
/// `pages:start:anchor` (the piece the page before fired in, and the
/// candidate), `pages:start:fold` (the main list's fold after the fire),
/// `pages:start:params` (the parameters as the routine left them),
/// `pages:start:head` (what the page begins with as a profile: held
/// insertions, the routine's list, the carried material).
pub(crate) const PAGE_START: &str = "pages:start";

impl Run<'_, '_> {
    /// `\pagegoal`, `\pagetotal`, `\pagestretch`, `\pagefilstretch`,
    /// `\pagefillstretch`, `\pagefilllstretch`, `\pageshrink`,
    /// `\pagedepth` (§421; `m` 0..7): the builder's `page_so_far[m]` on
    /// the page this piece is on, with the piece's own items up to its
    /// last candidate folded in (`SPEC.md` §4 first bullet). Reads:
    /// `pages:start:*` reaching the piece, the `vprof` fold from the
    /// page's anchor to this piece, the anchor's items after its
    /// candidate (`pages:start:head`), and its own items; nothing of
    /// the page record a dimension does not depend on. An empty page
    /// answers `\pagegoal = \maxdimen`, 0 for the rest. A page firing at
    /// a fresh candidate of the piece's own ends the piece there on a
    /// second pass ([`Run::order_after_own_page`]).
    pub(crate) fn page_dimen(&mut self, m: i32) -> R<i32> {
        todo!("page_dimen: the old feedback.rs page_so_far, on the split pages:start fields; the fold over vprof from the anchor stays a fold (the builder needs every profile), the record reads become per-part")
    }

    /// Whether a page fires at one of this piece's own candidates so far
    /// (the object-number path, `SPEC.md` §4 second bullet).
    pub(crate) fn page_fires_in_own(&mut self) -> R<bool> {
        todo!("page_fires_in_own: as page_dimen's fold, answering the first fire's place")
    }

    /// The parameters reaching this piece as a `T_PAGE_PARAMS` marker
    /// (what a bare `[tag]` marker of its own items stands for), read at
    /// the piece.
    pub(crate) fn page_params_reaching(&mut self) -> R<ValueId> {
        todo!("page_params_reaching: builder.rs at bc394b811, moved")
    }
}
