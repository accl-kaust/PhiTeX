//! The page builder on real nodes and on profiles: TeX's §980–§1026 as
//! pure functions over the items, shared by the loop (over profiles) and
//! the finisher (over the real items, to the same fire). Nothing here
//! reads the engine but through the arguments it is given.
//!
//! Moved here verbatim from the old glue (`builder.rs` at `bc394b811`
//! on branch `latex-pages`), with their docs: `Src`, `Span`, `span_ints`,
//! `span_of_ints`, `Resolved`, `fold_item_t`, `is_marker_p`, `build_t`,
//! `held_srcs`, `spans_of`, `loop_item`, `loop_item_raw`, `PageState`
//! and its `impl` (the replay: `build`, `fold_items`, `fire`), `class_in`,
//! `page_vers`, the marker resolution (`page_params_at`,
//! `page_params_struct_at`, `page_cells_at`, `page_params_marker`,
//! `insert_class_at`, `ins_marker`, `resolve_params_of`,
//! `resolve_prof_of`, `resolve_prof_of_with`, `OwnerReads`,
//! `RunReaching` and its `Reaching` impl), `unit_classes`,
//! `rtn_items`/`unit_items` as the finisher's input decoding. Nothing of
//! the step chain: no `PageLoop`, `Pending`, `encode_state`,
//! `decode_state`, `cut`, `step_log`, `kick`-chaining, `pages:loop`.
//!
//! The parts of the loop's state that are not the builder's own
//! ([`crate::vprof::ScanState`]) live in [`LoopCarry`]: `SPEC.md` §2.1
//! "every fire is noted with its sources", §2.2 (the routines' material
//! by page, marks in force, `\deadcycles`, the best-break penalties, the
//! last give-backs by routine and page class). Its fields are the old
//! `PageLoop`'s `srcs`, `post`, `post_markers`, `post_k`, `began`,
//! `top_marks`, `rtn_dead`, `dead`, `inf`, `last_rtn`, `last_fire`,
//! `held_nodes`, `units`, `order`, `prev`, `page_src`, `pend`, `frontier`
//! (those that are state between fires; drop what only the chain needed).

use phitex_defuse::ValueId;

/// The loop's state between fires that is not the builder's own.
#[derive(Clone, Default)]
pub(crate) struct LoopCarry {
    // (fields: the old PageLoop's, see the module doc)
}

/// A fire as the loop sees it: the builder's decisions at a `build_page`
/// call that fired (the old `LoopFire`: best, `\outputpenalty`, holding,
/// held insertions, what is carried, the marks).
pub(crate) struct LoopFire {
    // (fields: the old LoopFire's)
}

/// What the finisher's assembly gives the replay: the units named, the
/// items at the indices named, the own piece's rest, resolved markers.
pub(crate) struct Assembled {
    // (fields: the old finisher_args' intermediate: units, unit_ix, rtns,
    // the items by span)
}

/// A shipped page: `\box255` as native nodes, the routine's give-back,
/// the page key, the routine's global definitions, the next page's start.
pub(crate) struct Shipped {
    // (fields: what publish_page needs from fire_page/output_end/ship_out)
}

pub(crate) fn placeholder(_v: ValueId) {
    todo!("replay.rs: the moved functions listed in the module doc")
}
