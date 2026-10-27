//! The client's part of a collection of the value store
//! (`phitex_defuse::Engine::collect`, through `Operation::roots`): with
//! every worker at the barrier, what the client holds outside the
//! engine either given as a root or dropped.
//!
//! Given (each holds values a later run or the link decodes):
//! - the pages the live runs that shipped them name
//!   (`pdfout::PAGE_KEYS`, `pdfout::collect_pieces`: their parameters,
//!   sealed lines' stand-ins, glyph table and color stacks); the rest
//!   dropped with their written records;
//! - the finishers' tables (`Ops::fin_defs`, `Ops::fin_defs_start`) and
//!   the pages' routine hints the loop left for them (`Ops::fin_class`);
//! - the cut document files' kept arguments (`Ops::docs`);
//! - the format's values made for the build (`Ops::fmt_made`);
//! - the build's environment, job and files (`Ops::held`).
//!
//! Dropped (caches of ids, made again from what stands):
//! - the link's memos (`pdfout::clear_link_memos`,
//!   `assemble::clear_line_memos`);
//! - the decoded profiles (`vprof::clear_decoded`), the folded blocks
//!   (`build::clear_blocks`), the made lines (`nodes::clear_made`), the
//!   routine guesses seen (`pages::clear_guess_seen`, observe).
//!
//! Keyed by the store's epoch (`Values::epoch`, which a collection
//! moves), so made again after one: the line's records
//! (`pdfout::recs`), the allocations' folds, the link's inputs, the
//! list memberships (`val::clist_projection`), each worker's encoded and
//! decoded values (`val`), the fields by name (`fields`).
//!
//! Holding no value a run decodes: the command steps (`Ops::pdf_cmds`,
//! closures over bytes), the hints (strings and fields), the package
//! records (bytes), the font, map and file memos (keyed by ids, holding
//! bytes), the output memos (compared by id, made again where one moved).

use phitex_defuse::{Engine, FxHashSet, ValueId};

use crate::exec::LockWait as _;
use crate::run::Ops;

pub(crate) fn roots(o: &Ops, eng: &Engine, keep: &mut dyn FnMut(ValueId)) {
    let v = eng.values();
    // (the pages the runs that shipped them name, `pdfout::PAGE_KEYS`)
    let live: FxHashSet<u64> = eng
        .defined_by_live(v.field(crate::pdfout::PAGE_KEYS))
        .into_iter()
        .flat_map(|(_, ks)| crate::val::list_of(v, ks))
        .filter_map(|k| v.as_int(k))
        .map(|k| u64::from_ne_bytes(k.to_ne_bytes()))
        .collect();
    crate::pdfout::collect_pieces(v, &live, keep);
    crate::pdfout::clear_link_memos();
    crate::assemble::clear_line_memos();
    crate::vprof::clear_decoded();
    crate::build::clear_blocks();
    crate::nodes::clear_made();
    crate::pages::clear_guess_seen();
    for d in o.fin_defs.locked().values() {
        d.roots(keep);
    }
    for d in o.fin_defs_start.locked().values() {
        d.roots(keep);
    }
    for &(hint, _, _) in o.fin_class.locked().values() {
        keep(hint);
    }
    for f in o.docs.locked().values() {
        f.locked().roots(keep);
    }
    if let Some(m) = o.fmt_made.read().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
        m.roots(keep);
    }
    o.held.read().unwrap_or_else(std::sync::PoisonError::into_inner).iter().for_each(|&x| keep(x));
}
