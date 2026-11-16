//! The exact PDF's link state saved with a build and made again on a
//! load (docs/spec-persisted-graph.md §2.2.4, `crate::api::Build::save`):
//! the pages kept for the link ([`PagePiece`], by key) and the read-time
//! commands ([`Cmd`], by serial). Each is saved as its values (by their
//! stable ids, given to the export as roots) and plain data; its fonts
//! as the record of what their run read ([`FontsRec`]), made again by
//! answering the same questions ([`fonts_again`]) and checked against
//! the saved digest; a page's box decoded again from its page value, a
//! form's from its box value, an image's and an object file's bytes from
//! their texts. Nothing the link places is left out: an edit after a load
//! links what the build would have linked.

use std::sync::{Arc, Mutex};

use phitex_defuse::{ValueId, Values};
use phitex_pdf::PageBox as ImgBox;
use phitex_typeset::node::Node;

use super::step::{Body, Step, WhatSrc, file_body, form_box, form_data, image_data};
use super::{
    Ans, BaseIn, Cmd, CmdMemo, Fonts, FontsRec, PageBox, PagePiece, PageWhats, fonts_again, pieces,
};
use crate::api::persist::{Dec, Enc};
use crate::exec::LockWait as _;
use crate::val::list_of;

type R<T> = Result<T, String>;

fn put_rec(e: &mut Enc<'_>, f: &Fonts) {
    let rec = f.rec.as_deref().cloned().unwrap_or_default();
    e.b(f.rec.is_some());
    e.u64s(&rec.ids.iter().map(|&x| u64::from(x)).collect::<Vec<_>>());
    e.b(rec.space);
    e.b(rec.read_in);
    e.b(rec.image);
    e.n(rec.ans.len());
    for a in &rec.ans {
        match a {
            Ans::X(x) => {
                e.byte(0);
                e.ox(*x);
            }
            Ans::Exp(x) => {
                e.byte(1);
                e.b(x.is_some());
                if let Some((a, b, c, d)) = *x {
                    e.i(i64::from(a));
                    e.i(i64::from(b));
                    e.i(i64::from(c));
                    e.b(d);
                }
            }
            Ans::Pk(a, b) => {
                e.byte(2);
                e.s(a);
                e.s(b);
            }
        }
    }
    e.u(f.digest());
}

/// Fonts made again of their saved record, their digest the saved one.
fn get_fonts(d: &mut Dec<'_>) -> R<Fonts> {
    let recorded = d.b()?;
    let ids = d.u64s()?.into_iter().map(u32_of).collect::<R<Vec<u32>>>()?;
    let space = d.b()?;
    let read_in = d.b()?;
    let image = d.b()?;
    let n = d.n()?;
    let mut ans = Vec::with_capacity(n);
    for _ in 0..n {
        ans.push(match d.byte()? {
            0 => Ans::X(d.ox()?),
            1 => Ans::Exp(if d.b()? {
                Some((d.i32()?, d.i32()?, d.i32()?, d.b()?))
            } else {
                None
            }),
            2 => Ans::Pk(d.s()?, d.s()?),
            _ => return Err("a font answer of no kind".into()),
        });
    }
    let digest = d.u()?;
    let f = if recorded {
        let rec = FontsRec {
            ids,
            space,
            read_in,
            image,
            ans,
        };
        fonts_again(d.v, rec)
            .ok_or("a page's or a command's fonts not made again of their record")?
    } else {
        Fonts::default()
    };
    if f.digest() != digest {
        return Err("fonts made again of their record are not the saved ones".into());
    }
    Ok(f)
}

fn put_wparams(e: &mut Enc<'_>, w: &super::WParams) {
    for &p in w {
        e.i(i64::from(p));
    }
}

fn get_wparams(d: &mut Dec<'_>) -> R<super::WParams> {
    let mut w = [0; 6];
    for p in &mut w {
        *p = d.i32()?;
    }
    Ok(w)
}

fn put_obytes(e: &mut Enc<'_>, b: Option<&[u8]>) {
    e.b(b.is_some());
    if let Some(b) = b {
        e.by(b);
    }
}

fn get_obytes(d: &mut Dec<'_>) -> R<Option<Vec<u8>>> {
    Ok(if d.b()? { Some(d.by()?) } else { None })
}

fn put_xs(e: &mut Enc<'_>, xs: &[ValueId]) {
    e.n(xs.len());
    for &x in xs {
        e.x(x);
    }
}

fn get_xs(d: &mut Dec<'_>) -> R<Vec<ValueId>> {
    let n = d.n()?;
    (0..n).map(|_| d.x()).collect()
}

fn u32_of(x: u64) -> R<u32> {
    u32::try_from(x).map_err(|_| "a number out of range".into())
}

fn put_piece(e: &mut Enc<'_>, key: u64, pc: &PagePiece) {
    e.u(key);
    e.u64s(&pc.words);
    put_wparams(e, &pc.wparams);
    put_xs(e, &pc.params);
    for &m in &pc.margins {
        e.i(i64::from(m));
    }
    e.n(pc.whats.at.len());
    for &(h, x, special) in &pc.whats.at {
        e.u(u64::from(h));
        e.x(x);
        e.b(special);
    }
    e.u64s(&pc.whats.named);
    e.u64s(&pc.whats.rels);
    e.x(pc.page);
    put_rec(e, &pc.fonts);
    put_xs(e, &pc.lazy);
    e.n(pc.bases.len());
    for &(a, b, c) in &pc.bases {
        e.u(u64::from(a));
        e.u(u64::from(b));
        e.u(u64::from(c));
    }
    e.n(pc.pieces.len());
    for &p in &pc.pieces {
        e.i(p);
    }
    e.b(pc.places.is_some());
    if let Some((after, to, at)) = &pc.places {
        e.b(after.is_some());
        e.i(after.unwrap_or(0));
        e.i(*to);
        e.n(at.len());
        for &(p, i) in at {
            e.i(p);
            e.u(u64::from(i));
        }
    }
    let b = &pc.base_in;
    e.s(&b.date);
    e.s(&b.jobname);
    e.ox(b.g2u);
    e.ox(b.stacks);
    e.ox(b.maps);
    e.b(b.first);
    e.i(pc.pj);
    e.i(pc.node);
    e.b(pc.brk.is_some());
    if let Some((a, c)) = pc.brk {
        e.i(a);
        e.i(c);
    }
}

fn get_piece(d: &mut Dec<'_>) -> R<(u64, PagePiece)> {
    let v = d.v;
    let key = d.u()?;
    let words = d.u64s()?.into_boxed_slice();
    let wparams = get_wparams(d)?;
    let params = get_xs(d)?;
    let margins = [d.i32()?, d.i32()?, d.i32()?];
    let n = d.n()?;
    let mut at = Vec::with_capacity(n);
    for _ in 0..n {
        at.push((u32_of(d.u()?)?, d.x()?, d.b()?));
    }
    let whats = PageWhats {
        at: at.into(),
        named: d.u64s()?.into(),
        rels: d.u64s()?.into(),
    };
    let page = d.x()?;
    let fonts = get_fonts(d)?;
    let lazy = get_xs(d)?;
    let n = d.n()?;
    let mut bases = Vec::with_capacity(n);
    for _ in 0..n {
        bases.push((u32_of(d.u()?)?, u32_of(d.u()?)?, u32_of(d.u()?)?));
    }
    let n = d.n()?;
    let pcs = (0..n).map(|_| d.i()).collect::<R<Vec<i64>>>()?;
    let places = if d.b()? {
        let some = d.b()?;
        let after = d.i()?;
        let to = d.i()?;
        let n = d.n()?;
        let mut at = Vec::with_capacity(n);
        for _ in 0..n {
            at.push((d.i()?, u32_of(d.u()?)?));
        }
        Some((some.then_some(after), to, at))
    } else {
        None
    };
    let base_in = BaseIn {
        date: d.s()?,
        jobname: d.s()?,
        g2u: d.ox()?,
        stacks: d.ox()?,
        maps: d.ox()?,
        first: d.b()?,
    };
    let pj = d.i()?;
    let node = d.i()?;
    let brk = if d.b()? { Some((d.i()?, d.i()?)) } else { None };
    // (the box: its page value's, decoded as the page's step decoded it,
    // its sealed lines' stand-ins the saved ones)
    let b = list_of(v, page)
        .first()
        .copied()
        .ok_or("a page whose value holds no box")?;
    let mut side = crate::nodes::Side::default();
    let prev = crate::val::NO_KEEP.with(|c| c.replace(true));
    let decoded = crate::nodes::decode(v, b, &mut side);
    crate::val::NO_KEEP.with(|c| c.set(prev));
    let Some(Node::Box(bx)) = decoded else {
        return Err("a page's box that does not decode".into());
    };
    if side.lazy != lazy || crate::nodes::sealed_bases(&side) != bases {
        return Err("a page's box decoded again is not the saved one".into());
    }
    let pc = PagePiece {
        words,
        wparams,
        params,
        margins,
        whats,
        page,
        fonts,
        bx: Mutex::new(PageBox {
            bx: *bx,
            filled: None,
            saved: Vec::new(),
        }),
        lazy,
        bases,
        pieces: pcs,
        places,
        base_in,
        pj,
        node,
        brk,
        inst: v.instance(),
    };
    Ok((key, pc))
}

fn img_box_of(k: u64) -> ImgBox {
    match k {
        1 => ImgBox::Media,
        3 => ImgBox::Bleed,
        4 => ImgBox::Trim,
        5 => ImgBox::Art,
        _ => ImgBox::Crop,
    }
}

#[allow(clippy::too_many_lines)]
fn put_step(e: &mut Enc<'_>, s: &Step) {
    match s {
        Step::FontNum { fid } => {
            e.byte(0);
            e.u(u64::from(*fid));
        }
        Step::PageRef { n } => {
            e.byte(1);
            e.i(i64::from(*n));
        }
        Step::ColorStackInit { page, direct, init } => {
            e.byte(2);
            e.b(*page);
            e.b(*direct);
            e.by(init);
        }
        Step::ObjReserve => e.byte(3),
        Step::Obj {
            use_n,
            stream,
            attr,
            body,
            immediate,
        } => {
            e.byte(4);
            e.b(use_n.is_some());
            e.i(use_n.unwrap_or(0));
            e.b(*stream);
            put_obytes(e, attr.as_deref());
            match body {
                Body::Text(t) => {
                    e.b(false);
                    e.by(t);
                }
                Body::File(t, _) => {
                    e.b(true);
                    e.ox(*t);
                }
            }
            e.b(*immediate);
        }
        Step::NewObjNum => e.byte(5),
        Step::Outline {
            attr,
            act,
            count,
            title,
            ..
        } => {
            e.byte(6);
            put_obytes(e, attr.as_deref());
            e.x(*act);
            e.i(i64::from(*count));
            e.by(title);
        }
        Step::FontAttr { fid, text } => {
            e.byte(7);
            e.u(u64::from(*fid));
            e.by(text);
        }
        Step::IncludeChars { fid, text } => {
            e.byte(8);
            e.u(u64::from(*fid));
            e.by(text);
        }
        Step::Map { file, text } => {
            e.byte(9);
            e.b(*file);
            e.by(text);
        }
        Step::GlyphToUnicode { glyph, uni } => {
            e.byte(10);
            e.by(glyph);
            e.by(uni);
        }
        Step::NoBuiltinToUnicode { fid } => {
            e.byte(11);
            e.u(u64::from(*fid));
        }
        Step::XImage {
            ps,
            spec,
            data,
            immediate,
        } => {
            e.byte(12);
            e.n(ps.len());
            for &p in ps {
                e.i(i64::from(p));
            }
            e.i(i64::from(spec.width));
            e.i(i64::from(spec.height));
            e.i(i64::from(spec.depth));
            put_obytes(e, spec.attr.as_deref());
            e.i(i64::from(spec.page));
            put_obytes(e, spec.named.as_deref());
            e.i(i64::from(spec.colorspace));
            e.u(spec.page_box as u64);
            e.by(&spec.name);
            e.i(i64::from(spec.image_resolution));
            e.i(i64::from(spec.inclusion_errorlevel));
            e.ox(*data);
            e.b(*immediate);
        }
        Step::XForm {
            attr,
            res,
            b,
            whats,
            reloc,
            ids,
            immediate,
            ..
        } => {
            e.byte(13);
            put_obytes(e, attr.as_deref());
            put_obytes(e, res.as_deref());
            e.x(*b);
            e.n(whats.len());
            for w in whats {
                match *w {
                    WhatSrc::None => e.byte(0),
                    WhatSrc::Special(x) => {
                        e.byte(1);
                        e.x(x);
                    }
                    WhatSrc::Data(x) => {
                        e.byte(2);
                        e.x(x);
                    }
                }
            }
            e.n(reloc.len());
            for &(k, n) in reloc {
                e.u(k);
                e.n(n);
            }
            e.u64s(&ids.iter().map(|&x| u64::from(x)).collect::<Vec<_>>());
            e.b(*immediate);
        }
    }
}

#[allow(clippy::too_many_lines)]
fn get_step(d: &mut Dec<'_>) -> R<Step> {
    let v = d.v;
    Ok(match d.byte()? {
        0 => Step::FontNum {
            fid: u32_of(d.u()?)?,
        },
        1 => Step::PageRef { n: d.i32()? },
        2 => Step::ColorStackInit {
            page: d.b()?,
            direct: d.b()?,
            init: d.by()?,
        },
        3 => Step::ObjReserve,
        4 => {
            let some = d.b()?;
            let n = d.i()?;
            let stream = d.b()?;
            let attr = get_obytes(d)?;
            let body = if d.b()? {
                let t = d.ox()?;
                Body::File(t, file_body(v, t))
            } else {
                Body::Text(d.by()?)
            };
            Step::Obj {
                use_n: some.then_some(n),
                stream,
                attr,
                body,
                immediate: d.b()?,
            }
        }
        5 => Step::NewObjNum,
        6 => {
            let attr = get_obytes(d)?;
            let act = d.x()?;
            Step::Outline {
                attr,
                act,
                action: super::action_of(v, act),
                count: d.i32()?,
                title: d.by()?,
            }
        }
        7 => Step::FontAttr {
            fid: u32_of(d.u()?)?,
            text: d.by()?,
        },
        8 => Step::IncludeChars {
            fid: u32_of(d.u()?)?,
            text: d.by()?,
        },
        9 => Step::Map {
            file: d.b()?,
            text: d.by()?,
        },
        10 => Step::GlyphToUnicode {
            glyph: d.by()?,
            uni: d.by()?,
        },
        11 => Step::NoBuiltinToUnicode {
            fid: u32_of(d.u()?)?,
        },
        12 => {
            let n = d.n()?;
            let ps = (0..n).map(|_| d.i32()).collect::<R<Vec<i32>>>()?;
            let (width, height, depth) = (d.i32()?, d.i32()?, d.i32()?);
            let attr = get_obytes(d)?;
            let page = d.i32()?;
            let named = get_obytes(d)?;
            let colorspace = d.i32()?;
            let page_box = img_box_of(d.u()?);
            let name = d.by()?;
            let image_resolution = d.i32()?;
            let inclusion_errorlevel = d.i32()?;
            let data = d.ox()?;
            let spec = phitex_pdf::ImageSpec {
                width,
                height,
                depth,
                attr,
                page,
                named,
                colorspace,
                page_box,
                name,
                data: image_data(v, data),
                image_resolution,
                inclusion_errorlevel,
            };
            Step::XImage {
                ps,
                spec,
                data,
                immediate: d.b()?,
            }
        }
        13 => {
            let attr = get_obytes(d)?;
            let res = get_obytes(d)?;
            let b = d.x()?;
            let n = d.n()?;
            let mut whats = Vec::with_capacity(n);
            for _ in 0..n {
                whats.push(match d.byte()? {
                    0 => WhatSrc::None,
                    1 => WhatSrc::Special(d.x()?),
                    2 => WhatSrc::Data(d.x()?),
                    _ => return Err("a form's whatsit of no kind".into()),
                });
            }
            let n = d.n()?;
            let mut reloc = Vec::with_capacity(n);
            for _ in 0..n {
                reloc.push((d.u()?, d.z()?));
            }
            let ids = d.u64s()?.into_iter().map(u32_of).collect::<R<Vec<u32>>>()?;
            let immediate = d.b()?;
            let bx = form_box(v, b).ok_or("a form's box that does not decode")?;
            let data = form_data(v, &whats, &reloc);
            Step::XForm {
                attr,
                res,
                b,
                bx,
                whats,
                reloc,
                data,
                ids,
                immediate,
            }
        }
        _ => return Err("a read-time command of no kind".into()),
    })
}

/// The link's state of the build of store `e.v`: its pages, its
/// read-time commands (`cmds`), in the order of their keys.
pub(crate) fn save_link(e: &mut Enc<'_>, cmds: &CmdMemo) {
    let inst = e.v.instance();
    let mut ps: Vec<(u64, Arc<PagePiece>)> = pieces()
        .locked()
        .iter()
        .filter(|(k, _)| k.0 == inst)
        .map(|(k, pc)| (k.1, pc.clone()))
        .collect();
    ps.sort_unstable_by_key(|x| x.0);
    e.n(ps.len());
    for (k, pc) in &ps {
        put_piece(e, *k, pc);
    }
    let mut cs: Vec<(u64, Arc<Cmd>)> = cmds.locked().iter().map(|(&k, c)| (k, c.clone())).collect();
    cs.sort_unstable_by_key(|x| x.0);
    e.n(cs.len());
    for (serial, c) in &cs {
        e.u(*serial);
        e.u64s(&c.words);
        e.u(c.node);
        put_wparams(e, &c.wparams);
        put_rec(e, &c.fonts);
        put_step(e, &c.step);
    }
}

/// The link's state read back ([`load_link`]), installed by
/// [`LinkState::install`] once the whole load is checked.
pub(crate) struct LinkState {
    pieces: Vec<(u64, PagePiece)>,
    cmds: Vec<(u64, Cmd)>,
}

/// [`save_link`]'s bytes, each page and command made again in store
/// `d.v`.
pub(crate) fn load_link(d: &mut Dec<'_>) -> R<LinkState> {
    let n = d.n()?;
    let mut ps = Vec::with_capacity(n);
    for _ in 0..n {
        ps.push(get_piece(d)?);
    }
    let n = d.n()?;
    let mut cs = Vec::with_capacity(n);
    for _ in 0..n {
        let serial = d.u()?;
        let words = d.u64s()?.into_boxed_slice();
        let node = d.u()?;
        let wparams = get_wparams(d)?;
        let fonts = get_fonts(d)?;
        let step = get_step(d)?;
        cs.push((
            serial,
            Cmd {
                step,
                fonts,
                wparams,
                words,
                node,
            },
        ));
    }
    Ok(LinkState {
        pieces: ps,
        cmds: cs,
    })
}

impl LinkState {
    /// Its pages kept for the link under their keys in store `v`, its
    /// commands in `cmds` under their serials.
    pub(crate) fn install(self, v: &Values, cmds: &CmdMemo) {
        let inst = v.instance();
        {
            let mut m = pieces().locked();
            for (k, pc) in self.pieces {
                m.insert((inst, k), Arc::new(pc));
            }
        }
        let mut m = cmds.locked();
        for (k, c) in self.cmds {
            m.insert(k, Arc::new(c));
        }
    }
}
