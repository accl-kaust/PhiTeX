//! DVI: tex.web part 32 (§583–§643), copied. The file is a fold over the
//! pages in order — as TeX's is: a font is defined where it is first used,
//! `w`/`x`/`y`/`z` reuse stops at what TeX's half-buffer has already
//! written out (`dvi_gone`), `bop` points back at the page before — so
//! [`assemble`] writes it whole, and [`page_body`] is empty (CORE.md).

use std::sync::Arc;

use crate::item::{BoxNode, Item, RUNNING};
use crate::page::Page;
use crate::show::zround;
use crate::tfm::Font;
use crate::val::Glue;

const SET1: u8 = 128;
const SET_RULE: u8 = 132;
const PUT_RULE: u8 = 137;
const BOP: u8 = 139;
const EOP: u8 = 140;
const PUSH: u8 = 141;
const POP: u8 = 142;
const RIGHT1: u8 = 143;
const DOWN1: u8 = 157;
const Y0: u8 = 161;
const Y1: u8 = 162;
const Z0: u8 = 166;
const Z1: u8 = 167;
const FNT_NUM_0: u8 = 171;
const FNT1: u8 = 235;
const FNT_DEF1: u8 = 243;
const PRE: u8 = 247;
const POST: u8 = 248;
const POST_POST: u8 = 249;
const ID_BYTE: u8 = 2;
/// tex.web's `dvi_buf_size` as TeX Live builds it.
const BUF_SIZE: i32 = 16384;
const HALF_BUF: i32 = BUF_SIZE / 2;

// (§608: the movement stack's states)
const Y_HERE: u8 = 1;
const Z_HERE: u8 = 2;
const YZ_OK: u8 = 3;
const Y_OK: u8 = 4;
const Z_OK: u8 = 5;
const D_FIXED: u8 = 6;
const NONE_SEEN: u8 = 0;
const Y_SEEN: u8 = 6;
const Z_SEEN: u8 = 12;

#[derive(Clone, Copy)]
struct Movement {
    width: i32,
    location: i32,
    info: u8,
}

/// §625: `billion`, `vet_glue`.
fn vet_glue(g: f64) -> f64 {
    g.clamp(-1_000_000_000.0, 1_000_000_000.0)
}

/// §625, §634: the glue of a box set so far (`cur_g`, `cur_glue`).
#[derive(Default)]
struct SetGlue {
    cur_g: i32,
    cur_glue: f64,
}

impl SetGlue {
    fn set(&mut self, b: &BoxNode, g: &Glue) -> i32 {
        let mut wd = g.width - self.cur_g;
        if b.glue.sign == 1 {
            if g.stretch_order == b.glue.order {
                self.cur_glue += f64::from(g.stretch);
                self.cur_g = zround(vet_glue(b.glue.ratio * self.cur_glue));
            }
        } else if b.glue.sign == -1 && g.shrink_order == b.glue.order {
            self.cur_glue -= f64::from(g.shrink);
            self.cur_g = zround(vet_glue(b.glue.ratio * self.cur_glue));
        }
        wd += self.cur_g;
        wd
    }
}

/// The DVI writer's state (tex.web 592–595, 605, 616).
struct Writer {
    out: Vec<u8>,
    total_pages: i32,
    max_v: i32,
    max_h: i32,
    max_push: i32,
    last_bop: i32,
    down: Vec<Movement>,
    right: Vec<Movement>,
    dvi_h: i32,
    dvi_v: i32,
    cur_h: i32,
    cur_v: i32,
    dvi_f: u16,
    cur_s: i32,
    /// The fonts used so far, by internal number (`font_used`).
    used: Vec<Option<Arc<Font>>>,
}

impl Writer {
    fn loc(&self) -> i32 {
        self.out.len() as i32
    }

    /// `dvi_gone`: what the half-buffer has written out (§598).
    fn gone(&self) -> i32 {
        let t = self.loc();
        if t >= BUF_SIZE { (t / HALF_BUF - 1) * HALF_BUF } else { 0 }
    }

    fn byte(&mut self, b: u8) {
        self.out.push(b);
    }

    fn int(&mut self, x: i32) {
        self.out.push((x & 0xFF) as u8);
    }

    fn four(&mut self, x: i32) {
        self.out.extend(x.to_be_bytes());
    }

    /// §601: `dvi_pop`.
    fn pop(&mut self, l: i32) {
        if l == self.loc() && self.loc() % BUF_SIZE != 0 {
            self.out.pop();
        } else {
            self.byte(POP);
        }
    }

    /// §602: `dvi_font_def`.
    fn font_def(&mut self, f: &Font) {
        self.byte(FNT_DEF1);
        self.int(i32::from(f.id) - 1);
        self.out.extend(f.checksum.to_be_bytes());
        self.four(f.size);
        self.four(f.design_size);
        self.byte(0);
        self.int(f.name.len() as i32);
        self.out.extend(f.name.as_bytes());
    }

    /// §621: change font `dvi_f` to `f`.
    fn change_font(&mut self, f: &Arc<Font>) {
        let i = usize::from(f.id);
        if self.used.len() <= i {
            self.used.resize(i + 1, None);
        }
        if self.used[i].is_none() {
            self.font_def(f);
            self.used[i] = Some(f.clone());
        }
        let n = i32::from(f.id) - 1;
        if f.id <= 64 {
            self.int(n + i32::from(FNT_NUM_0));
        } else {
            self.byte(FNT1);
            self.int(n);
        }
        self.dvi_f = f.id;
    }

    /// §607: `movement(w, o)`.
    fn movement(&mut self, mut w: i32, o: u8) {
        let location = self.loc();
        let gone = self.gone();
        let stack = if o == DOWN1 { &mut self.down } else { &mut self.right };
        stack.push(Movement { width: w, location, info: YZ_OK });
        let q = stack.len() - 1;
        // (§611)
        let mut mstate = NONE_SEEN;
        let mut found = None;
        let mut retag = None;
        for p in (0..q).rev() {
            let m = stack[p];
            if m.width == w {
                // (§612)
                match mstate + m.info {
                    x if x == NONE_SEEN + YZ_OK
                        || x == NONE_SEEN + Y_OK
                        || x == Z_SEEN + YZ_OK
                        || x == Z_SEEN + Y_OK =>
                    {
                        if m.location < gone {
                            break;
                        }
                        // (§613: change the buffered instruction to `y` or `w`)
                        retag = Some((p, Y1 - DOWN1, Y_HERE));
                        found = Some(p);
                        break;
                    }
                    x if x == NONE_SEEN + Z_OK || x == Y_SEEN + YZ_OK || x == Y_SEEN + Z_OK => {
                        if m.location < gone {
                            break;
                        }
                        // (§614: to `z` or `x`)
                        retag = Some((p, Z1 - DOWN1, Z_HERE));
                        found = Some(p);
                        break;
                    }
                    x if x == NONE_SEEN + Y_HERE
                        || x == NONE_SEEN + Z_HERE
                        || x == Y_SEEN + Z_HERE
                        || x == Z_SEEN + Y_HERE =>
                    {
                        found = Some(p);
                        break;
                    }
                    _ => {}
                }
            } else {
                match mstate + m.info {
                    x if x == NONE_SEEN + Y_HERE => mstate = Y_SEEN,
                    x if x == NONE_SEEN + Z_HERE => mstate = Z_SEEN,
                    x if x == Y_SEEN + Z_HERE || x == Z_SEEN + Y_HERE => break,
                    _ => {}
                }
            }
        }
        if let Some((p, delta, tag)) = retag {
            let k = stack[p].location as usize;
            stack[p].info = tag;
            self.out[k] = self.out[k].wrapping_add(delta);
        }
        let stack = if o == DOWN1 { &mut self.down } else { &mut self.right };
        if let Some(p) = found {
            // (§609: `y0`/`w0` or `z0`/`x0`)
            let info = stack[p].info;
            stack[q].info = info;
            let (op, fix) = if info == Y_HERE {
                (o + (Y0 - DOWN1), [(YZ_OK, Z_OK), (Y_OK, D_FIXED)])
            } else {
                (o + (Z0 - DOWN1), [(YZ_OK, Y_OK), (Z_OK, D_FIXED)])
            };
            for m in &mut stack[p + 1..q] {
                for (from, to) in fix {
                    if m.info == from {
                        m.info = to;
                        break;
                    }
                }
            }
            self.byte(op);
            return;
        }
        // (§610: `down`/`right` with as few bytes as fit)
        if w.abs() >= 0o40_000_000 {
            self.byte(o + 3);
            self.four(w);
            return;
        }
        if w.abs() >= 0o100_000 {
            self.byte(o + 2);
            if w < 0 {
                w += 0o100_000_000;
            }
            self.int(w / 0o200_000);
            w %= 0o200_000;
            self.int(w / 0o400);
            self.int(w % 0o400);
            return;
        }
        if w.abs() >= 0o200 {
            self.byte(o + 1);
            if w < 0 {
                w += 0o200_000;
            }
            self.int(w / 0o400);
            self.int(w % 0o400);
            return;
        }
        self.byte(o);
        if w < 0 {
            w += 0o400;
        }
        self.int(w);
    }

    /// §615
    fn prune_movements(&mut self, l: i32) {
        while self.down.last().is_some_and(|m| m.location >= l) {
            self.down.pop();
        }
        while self.right.last().is_some_and(|m| m.location >= l) {
            self.right.pop();
        }
    }

    fn synch_h(&mut self) {
        if self.cur_h != self.dvi_h {
            self.movement(self.cur_h - self.dvi_h, RIGHT1);
            self.dvi_h = self.cur_h;
        }
    }

    fn synch_v(&mut self) {
        if self.cur_v != self.dvi_v {
            self.movement(self.cur_v - self.dvi_v, DOWN1);
            self.dvi_v = self.cur_v;
        }
    }

    /// §619, §629: a box (its list; the caller has set the position).
    fn list_out(&mut self, b: &BoxNode) {
        self.cur_s += 1;
        if self.cur_s > 0 {
            self.byte(PUSH);
        }
        if self.cur_s > self.max_push {
            self.max_push = self.cur_s;
        }
        let save_loc = self.loc();
        if b.vertical {
            self.vlist_out(b);
        } else {
            self.hlist_out(b);
        }
        self.prune_movements(save_loc);
        if self.cur_s > 0 {
            self.pop(save_loc);
        }
        self.cur_s -= 1;
    }

    fn char_out(&mut self, f: &Arc<Font>, c: u8) {
        self.synch_h();
        self.synch_v();
        if f.id != self.dvi_f {
            self.change_font(f);
        }
        if c >= 128 {
            self.byte(SET1);
        }
        self.byte(c);
        self.cur_h += f.glyph(i32::from(c)).map_or(0, |g| g.width);
        self.dvi_h = self.cur_h;
    }

    /// §619–§628
    fn hlist_out(&mut self, this: &BoxNode) {
        let mut glue = SetGlue::default();
        let base_line = self.cur_v;
        for it in &this.list {
            match it {
                Item::Char(f, c) => self.char_out(f, *c),
                Item::Lig(l) => self.char_out(&l.font, l.c),
                Item::Box(b) => {
                    // (§623)
                    if b.list.is_empty() {
                        self.cur_h += b.w;
                    } else {
                        let (save_h, save_v) = (self.dvi_h, self.dvi_v);
                        self.cur_v = base_line + b.shift;
                        let edge = self.cur_h;
                        self.list_out(b);
                        self.dvi_h = save_h;
                        self.dvi_v = save_v;
                        self.cur_h = edge + b.w;
                        self.cur_v = base_line;
                    }
                }
                Item::Rule { w, h, d } => {
                    // (§624)
                    let rule_ht = if *h == RUNNING { this.h } else { *h };
                    let rule_dp = if *d == RUNNING { this.d } else { *d };
                    let rule_ht = rule_ht + rule_dp;
                    if rule_ht > 0 && *w > 0 {
                        self.synch_h();
                        self.cur_v = base_line + rule_dp;
                        self.synch_v();
                        self.byte(SET_RULE);
                        self.four(rule_ht);
                        self.four(*w);
                        self.cur_v = base_line;
                        self.dvi_h += *w;
                    }
                    self.cur_h += *w;
                }
                Item::Glue { g, .. } => self.cur_h += glue.set(this, g),
                Item::Kern { w, .. } => self.cur_h += *w,
                _ => {}
            }
        }
    }

    /// §629–§637
    fn vlist_out(&mut self, this: &BoxNode) {
        let mut glue = SetGlue::default();
        let left_edge = self.cur_h;
        self.cur_v -= this.h;
        for it in &this.list {
            match it {
                Item::Box(b) => {
                    // (§632)
                    if b.list.is_empty() {
                        self.cur_v += b.h + b.d;
                    } else {
                        self.cur_v += b.h;
                        self.synch_v();
                        let (save_h, save_v) = (self.dvi_h, self.dvi_v);
                        self.cur_h = left_edge + b.shift;
                        self.list_out(b);
                        self.dvi_h = save_h;
                        self.dvi_v = save_v;
                        self.cur_v = save_v + b.d;
                        self.cur_h = left_edge;
                    }
                }
                Item::Rule { w, h, d } => {
                    // (§633)
                    let rule_wd = if *w == RUNNING { this.w } else { *w };
                    let rule_ht = *h + *d;
                    self.cur_v += rule_ht;
                    if rule_ht > 0 && rule_wd > 0 {
                        self.synch_h();
                        self.synch_v();
                        self.byte(PUT_RULE);
                        self.four(rule_ht);
                        self.four(rule_wd);
                    }
                }
                Item::Glue { g, .. } => self.cur_v += glue.set(this, g),
                Item::Kern { w, .. } => self.cur_v += *w,
                _ => {}
            }
        }
    }

    /// §640: one page.
    fn page(&mut self, p: &Page) {
        let b = &p.b;
        let s = &p.ship;
        // (§641)
        if b.h + b.d + s.voffset > self.max_v {
            self.max_v = b.h + b.d + s.voffset;
        }
        if b.w + s.hoffset > self.max_h {
            self.max_h = b.w + s.hoffset;
        }
        // (§617)
        self.dvi_h = 0;
        self.dvi_v = 0;
        self.cur_h = s.hoffset;
        self.dvi_f = 0;
        let page_loc = self.loc();
        self.byte(BOP);
        for &c in &s.counts {
            self.four(c);
        }
        self.four(self.last_bop);
        self.last_bop = page_loc;
        self.cur_v = b.h + s.voffset;
        self.list_out(b);
        self.byte(EOP);
        self.total_pages += 1;
        self.cur_s = -1;
    }
}

/// The preamble's comment (tex.web 617): ` TeX output YYYY.MM.DD:HHMM`.
fn comment(date: [i32; 4]) -> String {
    let two = |n: i32| format!("{:02}", n.rem_euclid(100));
    format!(" TeX output {}.{}.{}:{}{}", date[0], two(date[1]), two(date[2]), two(date[3] / 60), two(date[3] % 60))
}

/// A page's own commands: none (the file is one fold, [`assemble`]).
pub fn page_body(_p: &Page) -> Vec<u8> {
    Vec::new()
}

/// The DVI file of these pages (none: no file, as TeX writes none).
pub fn assemble(_font: &Font, pages: &[Page], _bodies: &[Arc<Vec<u8>>]) -> Vec<u8> {
    let Some(first) = pages.first() else { return Vec::new() };
    let mut w = Writer {
        out: Vec::new(),
        total_pages: 0,
        max_v: 0,
        max_h: 0,
        max_push: 0,
        last_bop: -1,
        down: Vec::new(),
        right: Vec::new(),
        dvi_h: 0,
        dvi_v: 0,
        cur_h: 0,
        cur_v: 0,
        dvi_f: 0,
        cur_s: -1,
        used: Vec::new(),
    };
    // (§617: the preamble, at the first page)
    w.byte(PRE);
    w.byte(ID_BYTE);
    w.four(25_400_000);
    w.four(473_628_672);
    w.four(first.ship.mag);
    let c = comment(first.ship.date);
    w.int(c.len() as i32);
    w.out.extend(c.as_bytes());
    for p in pages {
        w.page(p);
    }
    // (§642: the postamble)
    let mag = pages.last().map_or(1000, |p| p.ship.mag);
    w.byte(POST);
    w.four(w.last_bop);
    w.last_bop = w.loc() - 5;
    w.four(25_400_000);
    w.four(473_628_672);
    w.four(mag);
    w.four(w.max_v);
    w.four(w.max_h);
    w.int(w.max_push / 256);
    w.int(w.max_push % 256);
    w.int((w.total_pages / 256) % 256);
    w.int(w.total_pages % 256);
    // (§643: the fonts used, last loaded first)
    let used: Vec<Arc<Font>> = w.used.iter().rev().flatten().cloned().collect();
    for f in &used {
        w.font_def(f);
    }
    w.byte(POST_POST);
    w.four(w.last_bop);
    w.byte(ID_BYTE);
    let ptr = w.loc() % BUF_SIZE;
    let mut k = 4 + (BUF_SIZE - ptr) % 4;
    while k > 0 {
        w.byte(223);
        k -= 1;
    }
    w.out
}
