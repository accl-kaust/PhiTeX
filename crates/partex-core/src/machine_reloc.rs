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
    SnapBody, V, is_cs_slot, mark_cells, marks_version, page_version,
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
                MCell::ObjCount => Some((crate::reloc::NUM_ORIGIN, s.base, s.delta)),
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

/// A snapshot's `Rest` (the engine but its cells) with every moving
/// number relocated: in its input, the lists being built, the save stack
/// (its saved lists, boxes and count values), the marks or the page when
/// they are not cells, a box being built, what migrates. `Ok(None)` if it
/// holds none; `Err` if math or an alignment is being built (not walked).
fn shift_snapshot<H: CellHost>(b: &SnapBody<H>, s: &Shifts) -> Result<Option<SnapBody<H>>, ()> {
    let t = &b.tex;
    if !t.cur_list.mlist.is_empty()
        || t.nest.iter().any(|r| !r.mlist.is_empty())
        || !t.align.stack.is_empty()
        || !t.align.cur.columns.is_empty()
    {
        return Err(());
    }
    let mut new: Option<crate::tex::Tex<H, CellTracker>> = None;
    macro_rules! tex {
        () => {
            new.get_or_insert_with(|| t.clone())
        };
    }
    let level = |r: &crate::input::InStateRecord| -> Result<Option<Tokens>, ()> {
        match &r.list {
            Some(l) if r.state == TOKEN_LIST => shift_list(l, s),
            _ => Ok(None),
        }
    };
    if let Some(l) = level(&t.cur_input)? {
        tex!().cur_input.list = Some(l);
    }
    for (i, r) in t.input_stack.iter().enumerate() {
        if let Some(l) = level(r)? {
            tex!().input_stack[i].list = Some(l);
        }
    }
    for (i, p) in t.param_stack.iter().enumerate() {
        if let Some(p) = p
            && let Some(l) = shift_list(p, s)?
        {
            tex!().param_stack[i] = Some(l);
        }
    }
    if let Some(c) = &t.cur_toks
        && let Some(l) = shift_list(c, s)?
    {
        tex!().cur_toks = Some(l);
    }
    if let Some(v) = shift_tokens(&t.def_ref, s)? {
        tex!().def_ref = v;
    }
    if let Some(v) = shift_tokens(&t.arg_list, s)? {
        tex!().arg_list = v;
    }
    if let Some(v) = shift_tokens(&t.preamble_list, s)? {
        tex!().preamble_list = v;
    }
    for (i, o) in t.save_obj.iter().enumerate() {
        let n = match o {
            Some(crate::objs::Obj::Toks(x)) => shift_list(x, s)?.map(crate::objs::Obj::Toks),
            Some(crate::objs::Obj::Box(x)) => {
                match nodetoks::map_node(&Node::Box(x.clone()), &mut |l| shift_list(l, s))? {
                    Some(Node::Box(nb)) => Some(crate::objs::Obj::Box(nb)),
                    _ => None,
                }
            }
            _ => None,
        };
        if n.is_some() {
            tex!().save_obj[i] = n;
        }
    }
    // (a count register's value a group's end restores)
    let moved = |loc: i32, v: i32| {
        let o = origin_of_loc(loc)?;
        let w = s.map(o, i64::from(v));
        (w != i64::from(v)).then(|| i32::try_from(w).ok()).flatten()
    };
    if t.saved_counts()
        .into_iter()
        .any(|(l, v)| moved(l, v).is_some())
    {
        tex!().map_saved_counts(&moved);
    }
    if !mark_cells() {
        for (k, marks) in &t.cur_mark {
            for (i, m) in marks.iter().enumerate() {
                if let Some(m) = m
                    && let Some(l) = shift_list(m, s)?
                    && let Some(e) = tex!().cur_mark.get_mut(k)
                {
                    e[i] = Some(l);
                }
            }
        }
    }
    if let Some(n) = &t.cur_box
        && let Some(m) = nodetoks::map_node(n, &mut |l| shift_list(l, s))?
    {
        tex!().cur_box = Some(m);
    }
    if let Some(v) = &t.adjust
        && let Some(m) = shift_nodes(v, s)?
    {
        tex!().adjust = Some(m);
    }
    if let Some(m) = shift_nodes(&t.split_discards.to_vec(), s)? {
        tex!().split_discards = partex_engine::nodelist::NodeList::from_vec(m);
    }
    if let Some(m) = shift_nodes(&t.page.discards.to_vec(), s)? {
        tex!().page.discards = partex_engine::nodelist::NodeList::from_vec(m);
    }
    let mut lists: Option<Vec<Vec<Arc<[Node]>>>> = None;
    for (i, l) in b.lists.0.iter().enumerate() {
        if let Some(m) = shift_nodes(&l.concat(), s)? {
            lists.get_or_insert_with(|| b.lists.0.clone())[i] =
                m.chunks(LIST_CHUNK).map(Arc::from).collect();
        }
    }
    if new.is_none() && lists.is_none() {
        return Ok(None);
    }
    let tex = new.unwrap_or_else(|| t.clone());
    let lists = super::Lists(lists.unwrap_or_else(|| b.lists.0.clone()));
    Ok(Some(SnapBody::new(tex, lists, b.started, b.halted)))
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

/// `Machine::relocate_value` for [`super::TexMachine`] `m` (whose state
/// hash memo hashes a relocated `Rest`).
pub(super) fn relocate_value<H: CellHost>(
    m: &mut super::TexMachine<H>,
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
        (MCell::ObjCount, V::Int(n)) => {
            let x = s.map(crate::reloc::NUM_ORIGIN, i64::from(*n));
            if x == i64::from(*n) {
                Ok(None)
            } else {
                let x = i32::try_from(x).map_err(|_| ())?;
                Ok(Some(MValue {
                    version: super::obj_count_version(x),
                    v: V::Int(x),
                }))
            }
        }
        (MCell::Marks, V::Marks(m)) => Ok(shift_marks(m, &s)?.map(|n| MValue {
            version: marks_version(&n),
            v: V::Marks(Arc::new(n)),
        })),
        (MCell::Page, V::Page(pv)) => Ok(shift_page(pv, &s)?.map(|n| MValue {
            version: page_version(&n.builder, n.list.iter().map(|c| &c[..])),
            v: V::Page(Arc::new(n)),
        })),
        (MCell::Rest, V::Rest(snap)) => Ok(shift_snapshot(snap, &s)?.map(|body| {
            let snap = super::Snapshot::here(body);
            MValue {
                version: m.rest_version_of_snapshot(&snap),
                v: V::Rest(Arc::new(snap)),
            }
        })),
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
    if let (MCell::ObjCount, Some(V::Int(n))) = (c, v.map(|v| &v.v)) {
        return Some(i64::from(*n));
    }
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

/// `Machine::relocate_answer` for [`super::TexMachine`]: an answer about
/// the numbering (`FinalNum`, `OfFinal`, `NumState`) as a run whose object
/// numbers moved by `sh` would have recorded it, if `m` (at the region's
/// entry) answers so, with the guard as `m` holds it. A guard keeps only
/// a hash of its answer, so the answer `m` gives now is moved back (each
/// number it may have come from) and hashed.
pub(super) fn relocate_answer<H: CellHost>(
    m: &mut super::TexMachine<H>,
    c: &MCell,
    v: partex_incr::Version,
    sh: &[Shift<MCell>],
) -> Option<(MCell, partex_incr::Version)> {
    use partex_incr::Machine as _;
    let s = shifts_of(sh);
    let o = crate::reloc::NUM_ORIGIN;
    let map = |n: i32| i32::try_from(s.map(o, i64::from(n))).ok();
    // (the numbers `map` takes to `y`: itself, or itself less the shift)
    let back = |y: i32| -> Vec<i32> {
        let mut xs: Vec<i32> =
            s.0.iter()
                .filter(|x| x.0 == o)
                .filter_map(|x| i32::try_from(i64::from(y) - x.2).ok())
                .collect();
        xs.push(y);
        xs.retain(|&x| map(x) == Some(y));
        xs.sort_unstable();
        xs.dedup();
        xs
    };
    let held = |raw: u128| partex_incr::version_of(&Some(MValue::<H>::version(raw)));
    let g = match c {
        MCell::OfFinal(n) => MCell::OfFinal(map(*n)?),
        _ => c.clone(),
    };
    let now = m.get(&g)?;
    let w = partex_incr::version_of(&Some(&now));
    let was = match c {
        // (a virtual id: the same in both runs)
        MCell::OfFinal(_) => w == v,
        MCell::FinalNum(_) => {
            #[allow(clippy::cast_possible_truncation)] // (the number)
            let n = (now.version as u32).cast_signed();
            super::final_num_version(n) == now.version
                && back(n)
                    .into_iter()
                    .any(|x| held(super::final_num_version(x)) == v)
        }
        MCell::NumState(_) => {
            let (sys, obj_ptr) = super::num_state_counts(now.version);
            back(sys).into_iter().any(|xs| {
                back(obj_ptr)
                    .into_iter()
                    .any(|xo| held(super::num_state_with_counts(now.version, xs, xo)) == v)
            })
        }
        _ => false,
    };
    was.then_some((g, w))
}
