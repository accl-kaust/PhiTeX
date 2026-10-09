//! DVI: a page's commands are a pure function of the page (memoised by its
//! content); the file is those pages joined with their back pointers.

use std::sync::Arc;

use crate::linebreak::{GlueSet, Line};
use crate::page::{Page, VItem};
use crate::piece::HItem;
use crate::tfm::Font;

const SET1: u8 = 128;
const BOP: u8 = 139;
const EOP: u8 = 140;
const RIGHT4: u8 = 146;
const DOWN4: u8 = 160;
const FNT_NUM_0: u8 = 171;
const FNT_DEF1: u8 = 243;
const PRE: u8 = 247;
const POST: u8 = 248;
const POST_POST: u8 = 249;
const NUM: u32 = 25_400_000;
const DEN: u32 = 473_628_672;
const MAG: u32 = 1000;

fn glue_amount(g: &GlueSet, st: i32, st_ord: u8, sh: i32, sh_ord: u8) -> i64 {
    if g.sign == 1 && g.order == st_ord {
        (g.ratio * f64::from(st)).round() as i64
    } else if g.sign == -1 && g.order == sh_ord {
        -(g.ratio * f64::from(sh)).round() as i64
    } else {
        0
    }
}

#[derive(Default)]
struct Out {
    b: Vec<u8>,
    h: i64,
    v: i64,
}

impl Out {
    fn right(&mut self, x: i64) {
        if x != 0 {
            self.b.push(RIGHT4);
            self.b.extend((x as i32).to_be_bytes());
        }
    }
    fn down(&mut self, x: i64) {
        if x != 0 {
            self.b.push(DOWN4);
            self.b.extend((x as i32).to_be_bytes());
        }
    }
    fn hlist(&mut self, l: &Line) {
        let mut cur_h: i64 = 0;
        for it in &l.items {
            match *it {
                HItem::Char { c, w, .. } => {
                    self.right(cur_h - self.h);
                    self.h = cur_h;
                    if c < 128 {
                        self.b.push(c);
                    } else {
                        self.b.push(SET1);
                        self.b.push(c);
                    }
                    self.h += i64::from(w);
                    cur_h += i64::from(w);
                }
                HItem::Box { w, .. } => cur_h += i64::from(w),
                HItem::Glue { w, st, st_ord, sh, sh_ord } => {
                    cur_h += i64::from(w) + glue_amount(&l.glue, st, st_ord, sh, sh_ord);
                }
                HItem::Penalty(_) => {}
            }
        }
    }
}

/// A page's commands (between its `bop` and `eop`, font selected).
pub fn page_body(p: &Page) -> Vec<u8> {
    let mut o = Out::default();
    let mut cur_v: i64 = 0;
    for it in &p.items {
        match it {
            VItem::Line(l, _) => {
                cur_v += i64::from(l.h);
                o.down(cur_v - o.v);
                o.v = cur_v;
                o.hlist(l);
                cur_v += i64::from(l.d);
            }
            VItem::Glue { w, st, st_ord, sh } => {
                cur_v += i64::from(*w) + glue_amount(&p.glue, *st, *st_ord, *sh, 0);
            }
            VItem::Join { .. } | VItem::Penalty(_) => {}
        }
    }
    o.b
}

fn fnt_def(out: &mut Vec<u8>, f: &Font) {
    out.push(FNT_DEF1);
    out.push(0);
    out.extend(f.checksum.to_be_bytes());
    out.extend(f.size.to_be_bytes());
    out.extend(f.design_size.to_be_bytes());
    out.push(0);
    out.push(f.name.len() as u8);
    out.extend(f.name.as_bytes());
}

/// The DVI file: preamble, pages with back pointers, postamble.
pub fn assemble(font: &Font, pages: &[Page], bodies: &[Arc<Vec<u8>>]) -> Vec<u8> {
    let mut out = vec![PRE, 2];
    out.extend(NUM.to_be_bytes());
    out.extend(DEN.to_be_bytes());
    out.extend(MAG.to_be_bytes());
    let comment = b" phitex-flow";
    out.push(comment.len() as u8);
    out.extend(comment);
    let mut prev: i32 = -1;
    let (mut max_v, mut max_h) = (0i32, 0i32);
    for (k, body) in bodies.iter().enumerate() {
        let here = out.len() as i32;
        out.push(BOP);
        out.extend(((k + 1) as i32).to_be_bytes());
        for _ in 0..9 {
            out.extend(0i32.to_be_bytes());
        }
        out.extend(prev.to_be_bytes());
        prev = here;
        if k == 0 {
            fnt_def(&mut out, font);
        }
        out.push(FNT_NUM_0);
        out.extend(body.iter());
        out.push(EOP);
        let p = &pages[k];
        max_v = max_v.max(p.h + p.d);
        for it in &p.items {
            if let VItem::Line(l, _) = it {
                max_h = max_h.max(l.w);
            }
        }
    }
    let post = out.len() as i32;
    out.push(POST);
    out.extend(prev.to_be_bytes());
    out.extend(NUM.to_be_bytes());
    out.extend(DEN.to_be_bytes());
    out.extend(MAG.to_be_bytes());
    out.extend(max_v.to_be_bytes());
    out.extend(max_h.to_be_bytes());
    out.extend(1u16.to_be_bytes());
    out.extend((bodies.len() as u16).to_be_bytes());
    fnt_def(&mut out, font);
    out.push(POST_POST);
    out.extend(post.to_be_bytes());
    out.push(2);
    out.extend([223u8; 4]);
    while out.len() % 4 != 0 {
        out.push(223);
    }
    out
}
