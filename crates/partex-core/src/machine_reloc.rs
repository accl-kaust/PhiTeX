//! Relocatable values in the machine (`reloc.rs`, `DESIGN.md` 4.1): what
//! `partex_incr` asks of [`TexMachine`] to reuse a region whose numbers
//! moved. A count register is an origin cell; its numbers are relocated
//! where the engine keeps them as tagged digits: in eqtb's lists and
//! boxes, the current marks, the page, and (checked to hold none that
//! move) the rest of the state, sealed lines and the PDF objects.

use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::node::{Node, Tokens};
use partex_engine::nodetoks;
use partex_incr::Shift;

use super::{
    CellHost, CellTracker, CellValue, LIST_CHUNK, MCell, MValue, MarkMap, Named, PageValue,
    SnapBody, V, is_cs_slot, mark_cells, marks_version, page_cells, page_version,
};
use crate::mem::MemoryWord;
use crate::reloc::{Shifts, origin_of_loc, shift_tokens};
use crate::track::Tracker;
use crate::web::TOKEN_LIST;

/// The engine's relocation for `sh`: origin cells are count registers.
pub(super) fn shifts_of(sh: &[Shift<MCell>]) -> Shifts {
    Shifts(
        sh.iter()
            .filter_map(|s| match s.cell {
                MCell::Eqtb(p) => origin_of_loc(p).map(|o| (o, s.base, s.delta)),
                _ => None,
            })
            .collect(),
    )
}

/// `t` relocated (`None`: unchanged).
fn shift_list(t: &Tokens, s: &Shifts) -> Result<Option<Tokens>, ()> {
    Ok(
        shift_tokens(t.tokens(), s)?
            .map(|v| Arc::new(crate::tok::TokenList::new(v, t.protected()))),
    )
}

/// `nodes` relocated (`None`: unchanged).
fn shift_nodes(nodes: &[Node], s: &Shifts) -> Result<Option<Vec<Node>>, ()> {
    nodetoks::map_list(nodes, &mut |t| shift_list(t, s))
}

/// Whether `nodes` hold a number that moves (or one that cannot be told).
fn nodes_move(nodes: &[Node], s: &Shifts) -> bool {
    !matches!(shift_nodes(nodes, s), Ok(None))
}

/// Whether `toks` hold a number that moves.
fn toks_move(toks: &[i32], s: &Shifts) -> bool {
    !matches!(shift_tokens(toks, s), Ok(None))
}

/// Eqtb value `cv` of location `p` relocated: a count register's number
/// if it is a moving origin, a macro's or token register's list, a box
/// register's box.
fn shift_cell(p: i32, cv: &CellValue, s: &Shifts) -> Result<Option<CellValue>, ()> {
    let mut out: Option<CellValue> = None;
    if let Some(o) = origin_of_loc(p)
        && s.moves(o)
    {
        let x = i64::from(cv.word.int());
        let y = s.map(o, x);
        if y != x {
            let mut w: MemoryWord = cv.word;
            w.set_int(i32::try_from(y).map_err(|_| ())?);
            out.get_or_insert_with(|| cv.clone()).word = w;
        }
    }
    match &cv.named {
        Named::List {
            toks,
            protected,
            interned,
        } => {
            if let Some(v) = shift_tokens(toks, s)? {
                out.get_or_insert_with(|| cv.clone()).named = Named::List {
                    toks: v.into(),
                    protected: *protected,
                    interned: *interned,
                };
            }
        }
        Named::Box(Some(b)) => {
            if let Some(Node::Box(nb)) =
                nodetoks::map_node(&Node::Box(b.clone()), &mut |t| shift_list(t, s))?
            {
                out.get_or_insert_with(|| cv.clone()).named = Named::Box(Some(nb));
            }
        }
        Named::Nothing | Named::Glue(..) | Named::Shape(_) | Named::Box(None) => {}
    }
    Ok(out)
}

/// `Machine::get`'s version of eqtb location `p` holding `cv`
/// (`Tex::mcell_content`, from the value instead of an engine).
fn cell_version(p: i32, cv: &CellValue) -> u128 {
    use core::hash::{Hash, Hasher};
    let obj = cv.named.obj();
    let c = crate::statehash::eqtb_content_parts(
        p,
        cv.word,
        obj.as_ref(),
        || cv.level.unwrap_or(0),
        <CellTracker as Tracker>::VALUES,
        false,
    );
    match &cv.name {
        Some(n) if is_cs_slot(p) => {
            let mut h = partex_engine::stablehash::StableHasher::new();
            h.write_u128(c);
            n[..].hash(&mut h);
            h.finish128()
        }
        _ => c,
    }
}

/// The current marks relocated.
fn shift_marks(m: &MarkMap, s: &Shifts) -> Result<Option<MarkMap>, ()> {
    let mut out: Option<MarkMap> = None;
    for (k, marks) in m {
        for (i, t) in marks.iter().enumerate() {
            if let Some(t) = t
                && let Some(n) = shift_list(t, s)?
                && let Some(e) = out.get_or_insert_with(|| m.clone()).get_mut(k)
            {
                e[i] = Some(n);
            }
        }
    }
    Ok(out)
}

/// The page relocated (its list; what it discarded holds no numbers).
fn shift_page(pv: &PageValue, s: &Shifts) -> Result<Option<PageValue>, ()> {
    if nodes_move(&pv.builder.discards.to_vec(), s) {
        return Err(());
    }
    let list: Vec<Node> = pv.list.concat();
    Ok(shift_nodes(&list, s)?.map(|l| PageValue {
        builder: pv.builder.clone(),
        list: l.chunks(LIST_CHUNK).map(Arc::from).collect(),
    }))
}

/// Whether a snapshot's `Rest` (the engine but its cells) holds a number
/// that moves: in its input, a list being built, the save stack, the
/// marks or the page when they are not cells, a box or math being
/// built, an alignment.
fn snapshot_moves<H: CellHost>(b: &SnapBody<H>, s: &Shifts) -> bool {
    let t = &b.tex;
    let toks = |x: Option<&Tokens>| x.is_some_and(|x| toks_move(x.tokens(), s));
    let level = |r: &crate::input::InStateRecord| r.state == TOKEN_LIST && toks(r.list.as_ref());
    if level(&t.cur_input) || t.input_stack.iter().any(level) {
        return true;
    }
    if t.param_stack.iter().any(|x| toks(x.as_ref())) || toks(t.cur_toks.as_ref()) {
        return true;
    }
    if toks_move(&t.def_ref, s) || toks_move(&t.arg_list, s) || toks_move(&t.preamble_list, s) {
        return true;
    }
    let obj_moves = |o: &Option<crate::objs::Obj>| match o {
        Some(crate::objs::Obj::Toks(x)) => toks_move(x.tokens(), s),
        Some(crate::objs::Obj::Box(x)) => nodes_move(&[Node::Box(x.clone())], s),
        _ => false,
    };
    if t.save_obj.iter().any(obj_moves) {
        return true;
    }
    // (a count register's value a group's end restores)
    if t.saved_counts()
        .into_iter()
        .any(|(loc, v)| origin_of_loc(loc).is_some_and(|o| s.map(o, i64::from(v)) != i64::from(v)))
    {
        return true;
    }
    if !mark_cells() && t.cur_mark.values().flatten().any(|x| toks(x.as_ref())) {
        return true;
    }
    for (i, l) in b.lists.0.iter().enumerate() {
        if i == 0 && page_cells() {
            continue;
        }
        if nodes_move(&l.concat(), s) {
            return true;
        }
    }
    if !page_cells() && nodes_move(&t.page.discards.to_vec(), s) {
        return true;
    }
    if !t.cur_list.mlist.is_empty() || t.nest.iter().any(|r| !r.mlist.is_empty()) {
        return true;
    }
    if t.cur_box
        .as_ref()
        .is_some_and(|n| nodes_move(core::slice::from_ref(n), s))
        || t.adjust.as_ref().is_some_and(|v| nodes_move(v, s))
        || nodes_move(&t.split_discards.to_vec(), s)
    {
        return true;
    }
    !t.align.stack.is_empty() || !t.align.cur.columns.is_empty()
}

/// Whether a PDF object table entry holds a number that moves.
fn entry_moves(e: &crate::pdf::objtab::Entry, s: &Shifts) -> bool {
    use crate::pdf::objtab::Aux;
    let toks = |x: Option<&Tokens>| x.is_some_and(|x| toks_move(x.tokens(), s));
    let action = |a: Option<&Arc<partex_engine::node::Action>>| {
        a.is_some_and(|a| {
            let id = |i: &partex_engine::node::PdfId| match i {
                partex_engine::node::PdfId::Name(x) => toks_move(x.tokens(), s),
                partex_engine::node::PdfId::Num(_) => false,
            };
            toks(a.tokens.as_ref())
                || toks(a.file.as_ref())
                || id(&a.id)
                || a.struct_id.as_ref().is_some_and(id)
        })
    };
    match &e.aux {
        Aux::None | Aux::Int(_) | Aux::Dest(_) => false,
        Aux::Obj(o) => toks(Some(&o.data)) || toks(o.stream_attr.as_ref()),
        Aux::XForm(x) => {
            toks(x.attr.as_ref())
                || toks(x.resources.as_ref())
                || x.boxed.as_ref().is_some_and(|b| nodes_move(&b.list, s))
        }
        Aux::XImage(x) => toks(x.attr.as_ref()),
        Aux::Outline(x) => toks(x.attr.as_ref()),
        Aux::Bead(x) => toks(x.attr.as_ref()),
        Aux::Mark(m) => toks(m.data.as_ref()) || action(m.action.as_ref()),
    }
}

/// `Machine::relocate_value` for [`super::TexMachine`].
pub(super) fn relocate_value<H: CellHost>(
    c: &MCell,
    v: &MValue<H>,
    sh: &[Shift<MCell>],
) -> Result<Option<MValue<H>>, ()> {
    let s = shifts_of(sh);
    let cell = |p: i32, cv: &CellValue| -> Result<Option<MValue<H>>, ()> {
        Ok(shift_cell(p, cv, &s)?.map(|n| MValue {
            version: cell_version(p, &n),
            v: V::Word(Arc::new(n)),
        }))
    };
    match (c, &v.v) {
        (MCell::Eqtb(p), V::Word(cv)) => cell(*p, cv),
        (MCell::Eqtb(p), V::Lazy(snap, q)) => cell(*p, &snap.tex.export_cell(*q)),
        (MCell::Marks, V::Marks(m)) => Ok(shift_marks(m, &s)?.map(|n| MValue {
            version: marks_version(&n),
            v: V::Marks(Arc::new(n)),
        })),
        (MCell::Page, V::Page(pv)) => Ok(shift_page(pv, &s)?.map(|n| MValue {
            version: page_version(&n.builder, n.list.iter().map(|c| &c[..])),
            v: V::Page(Arc::new(n)),
        })),
        (MCell::Rest, V::Rest(snap)) => {
            if snapshot_moves(snap, &s) {
                Err(())
            } else {
                Ok(None)
            }
        }
        (MCell::Sealed(..), V::Sealed(x)) => {
            if nodes_move(&x.list, &s) {
                Err(())
            } else {
                Ok(None)
            }
        }
        (MCell::Obj(_), V::Obj(e)) => {
            if entry_moves(e, &s) {
                Err(())
            } else {
                Ok(None)
            }
        }
        // (an entry taken away)
        (MCell::Obj(_), V::Version) => Ok(None),
        (
            MCell::Eqtb(_)
            | MCell::Marks
            | MCell::Page
            | MCell::Rest
            | MCell::Sealed(..)
            | MCell::Obj(_),
            _,
        ) => Err(()),
        // (the other cells hold no numbers an origin gives out)
        _ => Ok(None),
    }
}

/// `Machine::origin_number` for [`super::TexMachine`]: a count register's
/// number.
pub(super) fn origin_number<H: CellHost>(c: &MCell, v: Option<&MValue<H>>) -> Option<i64> {
    let MCell::Eqtb(p) = c else {
        return None;
    };
    origin_of_loc(*p)?;
    match &v?.v {
        V::Word(cv) => Some(i64::from(cv.word.int())),
        V::Lazy(snap, q) => Some(i64::from(snap.tex.peek_eqtb(*q).int())),
        _ => None,
    }
}

/// `Machine::guard_relocates` for [`super::TexMachine`].
pub(super) fn guard_relocates(c: &MCell, sh: &[Shift<MCell>]) -> bool {
    match c {
        MCell::Origin(o) => {
            let s = shifts_of(sh);
            if *o == super::ANY_ORIGIN {
                s.0.iter().all(|x| x.2 == 0)
            } else {
                !s.moves(*o)
            }
        }
        MCell::IntCmp(o, x, rel, y) => {
            let s = shifts_of(sh);
            let answer = super::int_cmp_version(*x, *rel, *y) & 1 == 1;
            crate::reloc::answer_holds(&s, *o, *x, *rel, *y, answer)
        }
        _ => true,
    }
}

/// `Machine::relocate_guard` for [`super::TexMachine`].
pub(super) fn relocate_guard(c: &MCell, sh: &[Shift<MCell>]) -> MCell {
    match c {
        MCell::IntCmp(o, x, rel, y) => {
            let s = shifts_of(sh);
            let nx = i32::try_from(s.map(*o, i64::from(*x))).unwrap_or(*x);
            MCell::IntCmp(*o, nx, *rel, *y)
        }
        _ => c.clone(),
    }
}
