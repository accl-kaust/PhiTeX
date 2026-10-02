//! The values of the families whose fields are structures of the engine
//! (DESIGN 7.17.12's `page`, `align`, `pdf`, `dvi`, `write_file`,
//! `read_file` and `random` rows): what a record keeps as a write's value,
//! and what a step that runs again is placed at (DESIGN 7.17.3's first
//! form). A value is the field as the engine holds it, cloned: the
//! structures share their parts (a persistent list, a `Val`'s `Arc`, a
//! table's chunks), so a clone costs what a pointer copy does, and
//! storing it back is a store of the field.

use alloc::sync::Arc;
use core::any::Any;
use core::fmt;

use crate::host::Host;
use crate::ssa::{Fam, Slot};
use crate::tex::Tex;
use crate::track::Tracker;

/// A field's value, by the field's own type.
#[derive(Clone)]
pub(crate) struct Field(Arc<dyn Any>);

impl fmt::Debug for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<field>")
    }
}

impl Field {
    fn of<T: Clone + 'static>(v: &T) -> Field {
        Field(Arc::new(v.clone()))
    }

    fn get<T: 'static>(&self) -> Option<&T> {
        self.0.downcast_ref::<T>()
    }
}

/// The value slot `s` of these families holds in `t` now.
pub(crate) fn value<H: Host, T: Tracker>(t: &Tex<H, T>, s: Slot) -> Option<Field> {
    if s.0 == Fam::Glyphs {
        let g = t.pdf.ship.glyphs.get(usize::try_from(s.1).ok()?)?;
        return Some(Field::of(g));
    }
    if s.0 == Fam::PageNode {
        let n = t.page.list.get(usize::try_from(s.1).ok()?)?;
        return Some(Field::of(n));
    }
    let k = u8::try_from(s.1).ok()?;
    Some(match s.0 {
        Fam::Page => page_value(t, k)?,
        Fam::List => align_value(t, k.checked_sub(crate::track::list::COUNT + 1)?)?,
        Fam::Pdf => pdf_value(t, k)?,
        Fam::Dvi => dvi_value(t, k)?,
        Fam::Out => {
            let n = usize::from(k);
            if k == crate::streams::LOG {
                Field::of(&t.log_file.id)
            } else {
                Field::of(&(t.streams.out_name(n), t.write_file.get(n)?.id))
            }
        }
        Fam::Read => {
            let n = usize::from(k);
            Field::of(&(t.read_file.get(n)?.clone(), t.streams.read_ver(n)))
        }
        Fam::Random => Field::of(&t.random),
        _ => return None,
    })
}

/// Store value `v` at slot `s` of these families; whether `v` was one.
pub(crate) fn set<H: Host, T: Tracker>(t: &mut Tex<H, T>, s: Slot, v: &Field) -> bool {
    if s.0 == Fam::Glyphs {
        let (Ok(n), Some(g)) = (usize::try_from(s.1), v.get::<crate::pdf::ship::Glyphs>()) else {
            return false;
        };
        let gl = &mut t.pdf.ship.glyphs;
        if gl.len() <= n {
            gl.resize(n + 1, Arc::from(&[][..]));
        }
        gl[n] = g.clone();
        return true;
    }
    if s.0 == Fam::PageNode {
        // (in its place on the list the length made; a node past the
        // length is not the page's now)
        let (Ok(k), Some(n)) = (usize::try_from(s.1), v.get::<partex_engine::node::Node>()) else {
            return false;
        };
        if k < t.page.list.len() {
            t.page.list.set(k, n.clone());
        }
        return true;
    }
    let Ok(k) = u8::try_from(s.1) else {
        return false;
    };
    match s.0 {
        Fam::Page => set_page(t, k, v),
        Fam::List => k
            .checked_sub(crate::track::list::COUNT + 1)
            .is_some_and(|f| set_align(t, f, v)),
        Fam::Pdf => set_pdf(t, k, v),
        Fam::Dvi => set_dvi(t, k, v),
        Fam::Out => {
            let n = usize::from(k);
            if k == crate::streams::LOG {
                let Some(id) = v.get::<Option<crate::host::WriteId>>() else {
                    return false;
                };
                t.log_file.id = *id;
                return true;
            }
            let Some((name, id)) = v.get::<(Option<Arc<[u8]>>, Option<crate::host::WriteId>)>()
            else {
                return false;
            };
            t.streams.set_out_name_value(n, name.clone());
            if let Some(f) = t.write_file.get_mut(n) {
                f.id = *id;
            }
            true
        }
        Fam::Read => {
            type Read = (Option<crate::input::AlphaFile>, u128);
            let Some((file, ver)) = v.get::<Read>() else {
                return false;
            };
            let n = usize::from(k);
            if let Some(f) = t.read_file.get_mut(n) {
                f.clone_from(file);
            }
            t.streams.set_read_ver(n, *ver);
            true
        }
        Fam::Random => match v.get::<crate::random::Randoms>() {
            Some(r) => {
                t.random = r.clone();
                true
            }
            None => false,
        },
        _ => false,
    }
}

fn page_value<H: Host, T: Tracker>(t: &Tex<H, T>, f: u8) -> Option<Field> {
    use crate::track::page::*;
    let p = &t.page;
    Some(match f {
        CONTENTS => Field::of(&p.contents),
        LIST => Field::of(&p.list),
        MAX_DEPTH => Field::of(&p.max_depth),
        LEAST_COST => Field::of(&p.least_cost),
        BEST_BREAK => Field::of(&p.best_break),
        BEST_SIZE => Field::of(&p.best_size),
        INS => Field::of(&p.ins),
        INSERT_PENALTIES => Field::of(&p.insert_penalties),
        LAST_GLUE => Field::of(&p.last.glue),
        LAST_PENALTY => Field::of(&p.last.penalty),
        LAST_KERN => Field::of(&p.last.kern),
        LAST_NODE_TYPE => Field::of(&p.last.node_type),
        DISCARDS => Field::of(&p.discards),
        SPLIT_DISCARDS => Field::of(&t.split_discards),
        // (the page's length and tail, slots of their own: the nodes are
        // appends, `Fam::PageNode`)
        LIST_LEN => Field::of(&p.list.len()),
        LIST_TAIL => Field::of(&p.tail_precedes_break()),
        k if (SO_FAR..SO_FAR + 8).contains(&k) => Field::of(&p.so_far[usize::from(k - SO_FAR)]),
        _ => return None,
    })
}

fn set_page<H: Host, T: Tracker>(t: &mut Tex<H, T>, f: u8, v: &Field) -> bool {
    use crate::track::page::*;
    use partex_engine::builder::{Contents, PageIns};
    use partex_engine::node::GlueSpec;
    use partex_engine::nodelist::NodeList;
    fn put<X: Clone + 'static>(to: &mut X, v: &Field) -> bool {
        v.get::<X>().map(|x| to.clone_from(x)).is_some()
    }
    let p = &mut t.page;
    match f {
        CONTENTS => put::<Contents>(&mut p.contents, v),
        LIST => put::<NodeList>(&mut p.list, v),
        MAX_DEPTH => put::<i32>(&mut p.max_depth, v),
        LEAST_COST => put::<i32>(&mut p.least_cost, v),
        BEST_BREAK => put::<Option<usize>>(&mut p.best_break, v),
        BEST_SIZE => put::<i32>(&mut p.best_size, v),
        INS => put::<alloc::vec::Vec<PageIns>>(&mut p.ins, v),
        INSERT_PENALTIES => put::<i32>(&mut p.insert_penalties, v),
        LAST_GLUE => put::<Option<GlueSpec>>(&mut p.last.glue, v),
        LAST_PENALTY => put::<i32>(&mut p.last.penalty, v),
        LAST_KERN => put::<i32>(&mut p.last.kern, v),
        LAST_NODE_TYPE => put::<i32>(&mut p.last.node_type, v),
        DISCARDS => put::<NodeList>(&mut p.discards, v),
        SPLIT_DISCARDS => put::<NodeList>(&mut t.split_discards, v),
        LIST_LEN => {
            // (the list made this long: the nodes a step reads are put in
            // place after, the rest are stand-ins it never reads)
            let Some(&n) = v.get::<usize>() else {
                return false;
            };
            p.list.truncate(n);
            while p.list.len() < n {
                p.list.push(partex_engine::node::Node::Penalty(0));
            }
            true
        }
        LIST_TAIL => {
            // (a stand-in of the kind the tail is, where the list's last
            // node is not)
            let Some(&tail) = v.get::<bool>() else {
                return false;
            };
            if let Some(k) = p.list.len().checked_sub(1)
                && p.tail_precedes_break() != tail
            {
                let stand = if tail {
                    partex_engine::node::Node::Rule {
                        width: 0,
                        height: 0,
                        depth: 0,
                    }
                } else {
                    partex_engine::node::Node::Penalty(0)
                };
                p.list.set(k, stand);
            }
            true
        }
        k if (SO_FAR..SO_FAR + 8).contains(&k) => {
            put::<i32>(&mut p.so_far[usize::from(k - SO_FAR)], v)
        }
        _ => false,
    }
}

fn align_value<H: Host, T: Tracker>(t: &Tex<H, T>, f: u8) -> Option<Field> {
    use crate::track::align::*;
    let c = &t.align.cur;
    Some(match f {
        COLUMN => Field::of(&c.cur_align),
        SPAN => Field::of(&c.cur_span),
        LOOP => Field::of(&c.cur_loop),
        ADJUST => Field::of(&c.adjust),
        COLUMNS => Field::of(&c.columns),
        TABSKIPS => Field::of(&c.tabskips),
        STACK => Field::of(&(t.align.stack.clone(), c.align_state)),
        _ => return None,
    })
}

fn set_align<H: Host, T: Tracker>(t: &mut Tex<H, T>, f: u8, v: &Field) -> bool {
    use crate::track::align::*;
    fn put<X: Clone + 'static>(to: &mut X, v: &Field) -> bool {
        v.get::<X>().map(|x| to.clone_from(x)).is_some()
    }
    let c = &mut t.align.cur;
    match f {
        COLUMN => put(&mut c.cur_align, v),
        SPAN => put(&mut c.cur_span, v),
        LOOP => put(&mut c.cur_loop, v),
        ADJUST => put(&mut c.adjust, v),
        COLUMNS => put(&mut c.columns, v),
        TABSKIPS => put(&mut c.tabskips, v),
        STACK => {
            type Stack = (
                alloc::vec::Vec<(crate::align::AlignLevel, partex_ssa::Version)>,
                i32,
            );
            let Some((stack, state)) = v.get::<Stack>() else {
                return false;
            };
            t.align.stack.clone_from(stack);
            t.align.cur.align_state = *state;
            true
        }
        _ => false,
    }
}

fn pdf_value<H: Host, T: Tracker>(t: &Tex<H, T>, f: u8) -> Option<Field> {
    use crate::pdf::val::field::*;
    let p = &t.pdf;
    Some(match f {
        LAST_MATCH => Field::of(&p.last_match),
        OBJS => Field::of(&(p.objs.tab.clone(), p.objs.head, p.objs.obj_ptr)),
        OBJ_TREES => Field::of(&p.objs.trees),
        DESTS => Field::of(&(p.objs.dest_names.clone(), p.objs.dest_hash)),
        OUT => Field::of(&p.out),
        OBJ_COUNT => Field::of(&p.obj_count),
        XFORM_COUNT => Field::of(&p.xform_count),
        XIMAGE_COUNT => Field::of(&p.ximage_count),
        k if (LAST..LAST + 10).contains(&k) => {
            Field::of(&p.last(crate::pdf::PdfLast::ALL[usize::from(k - LAST)]))
        }
        INFO_TOKS => Field::of(&p.info_toks),
        CATALOG_TOKS => Field::of(&p.catalog_toks),
        NAMES_TOKS => Field::of(&p.names_toks),
        TRAILER_TOKS => Field::of(&p.trailer_toks),
        TRAILER_ID_TOKS => Field::of(&p.trailer_id_toks),
        CATALOG_OPENACTION => Field::of(&p.catalog_openaction),
        OUTLINES => Field::of(&(p.first_outline, p.last_outline, p.parent_outline)),
        SPACE_FONT_NAME => Field::of(&p.space_font_name),
        FONT_ATTR => Field::of(&p.font_attr),
        NOBUILTIN_TOUNICODE => Field::of(&p.nobuiltin_tounicode),
        STACKS => Field::of(&p.stacks),
        SHIP => Field::of(&p.ship.st),
        PDF_FONTS => Field::of(&p.ship.fonts),
        ENCODINGS => Field::of(&p.ship.encodings),
        FONTW => Field::of(&p.fontw),
        TOUNICODE => Field::of(&t.tounicode),
        FONTMAP => Field::of(&t.fontmap),
        FONTS_MAPPED => Field::of(&t.fonts_mapped),
        _ => return None,
    })
}

fn set_pdf<H: Host, T: Tracker>(t: &mut Tex<H, T>, f: u8, v: &Field) -> bool {
    use crate::pdf::val::field::*;
    fn put<X: Clone + 'static>(to: &mut X, v: &Field) -> bool {
        v.get::<X>().map(|x| to.clone_from(x)).is_some()
    }
    let p = &mut t.pdf;
    match f {
        LAST_MATCH => put(&mut p.last_match, v),
        OBJS => {
            type Objs = (
                crate::pdf::val::VTab<crate::pdf::objtab::Entry>,
                [i32; crate::pdf::objtab::HEAD_TAB_MAX + 1],
                i32,
            );
            let Some((tab, head, ptr)) = v.get::<Objs>() else {
                return false;
            };
            p.objs.tab.clone_from(tab);
            p.objs.head = *head;
            p.objs.obj_ptr = *ptr;
            true
        }
        OBJ_TREES => put(&mut p.objs.trees, v),
        DESTS => {
            type Dests = (crate::pdf::val::VTab<(Arc<[u8]>, i32)>, u128);
            let Some((names, hash)) = v.get::<Dests>() else {
                return false;
            };
            p.objs.dest_names.clone_from(names);
            p.objs.dest_hash = *hash;
            true
        }
        OUT => put(&mut p.out, v),
        OBJ_COUNT => put(&mut p.obj_count, v),
        XFORM_COUNT => put(&mut p.xform_count, v),
        XIMAGE_COUNT => put(&mut p.ximage_count, v),
        k if (LAST..LAST + 10).contains(&k) => match v.get::<i32>() {
            Some(x) => {
                p.set_last(crate::pdf::PdfLast::ALL[usize::from(k - LAST)], *x);
                true
            }
            None => false,
        },
        INFO_TOKS => put(&mut p.info_toks, v),
        CATALOG_TOKS => put(&mut p.catalog_toks, v),
        NAMES_TOKS => put(&mut p.names_toks, v),
        TRAILER_TOKS => put(&mut p.trailer_toks, v),
        TRAILER_ID_TOKS => put(&mut p.trailer_id_toks, v),
        CATALOG_OPENACTION => put(&mut p.catalog_openaction, v),
        OUTLINES => match v.get::<(i32, i32, i32)>() {
            Some(&(a, b, c)) => {
                (p.first_outline, p.last_outline, p.parent_outline) = (a, b, c);
                true
            }
            None => false,
        },
        SPACE_FONT_NAME => put(&mut p.space_font_name, v),
        FONT_ATTR => put(&mut p.font_attr, v),
        NOBUILTIN_TOUNICODE => put(&mut p.nobuiltin_tounicode, v),
        STACKS => put(&mut p.stacks, v),
        SHIP => put(&mut p.ship.st, v),
        PDF_FONTS => put(&mut p.ship.fonts, v),
        ENCODINGS => put(&mut p.ship.encodings, v),
        FONTW => put(&mut p.fontw, v),
        TOUNICODE => put(&mut t.tounicode, v),
        FONTMAP => put(&mut t.fontmap, v),
        FONTS_MAPPED => put(&mut t.fonts_mapped, v),
        _ => false,
    }
}

fn dvi_value<H: Host, T: Tracker>(t: &Tex<H, T>, f: u8) -> Option<Field> {
    use crate::pdf::val::dvi_field::*;
    let f = f + crate::pdf::val::DVI;
    Some(match f {
        FILE => Field::of(&t.dvi.file_part()),
        FONTS | TOTALS | WRITER => Field::of(&t.dvi.writer),
        _ => return None,
    })
}

fn set_dvi<H: Host, T: Tracker>(t: &mut Tex<H, T>, f: u8, v: &Field) -> bool {
    use crate::pdf::val::dvi_field::*;
    let f = f + crate::pdf::val::DVI;
    match f {
        FILE => match v.get::<crate::dvi::FilePart>() {
            Some(x) => {
                t.dvi.set_file_part(x);
                true
            }
            None => false,
        },
        FONTS | TOTALS | WRITER => {
            match v.get::<Option<crate::pdf::val::Val<crate::dviout::DviWriter>>>() {
                Some(w) => {
                    t.dvi.writer.clone_from(w);
                    true
                }
                None => false,
            }
        }
        _ => false,
    }
}
