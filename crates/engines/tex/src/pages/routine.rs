//! The output routine's run (§1012–§1028, §638–§640, §1366–§1374;
//! `SPEC.md` §3.2): pdfTeX's semantics over the run machinery, moved
//! here verbatim from `finish.rs` at `bc394b811` (branch `latex-pages`)
//! with their docs: `preload_candidate`, `lazy_locals`, `lazy_names_at`,
//! `fin_lazy_value`, `fire_page` (the marks, insertion boxes,
//! `\outputpenalty`, `\deadcycles`, no routine / the routine's tokens),
//! `output_end` (the markers, `\box255` not void, `\insertpenalties`),
//! `ship_out`, `give_way`, `transient`, the `etex` mark classes. Their
//! input is a [`super::Page`] and the assembled items
//! ([`super::Assembled`]); their result a [`super::Shipped`]. Nothing of
//! the old `FinishIn`'s decoding, window reads, stale checks, predictions
//! or step reads lives here: those are the finisher's (`finisher.rs`).

pub(crate) fn placeholder() {
    todo!("routine.rs: the moved functions listed in the module doc")
}
