//! E2: DVI output from shipped pages, exactly as tex.web §583–642: movement
//! optimisation (w/x/y/z, §607–615, including the half-buffer limit on
//! patching), font definitions, rules, glue rounding (§625), leaders, specials,
//! `push`/`pop` with the removal of an empty pair, postamble.
//!
//! A pure function of the pages: `Dvi` is a local writer value folded over
//! the shipped pages in order (no state outlives `finish`).
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::font::Fonts;
use crate::item::{BoxNode, Item, Whatsit, RUNNING};
use crate::output::Shipped;
use crate::prim::{dp, ip};
use crate::tok::Names;
use crate::val::{FontId, Key};

// DVI opcodes (tex.web §586).
const SET1: u8 = 128;
const SET_RULE: u8 = 132;
const PUT_RULE: u8 = 137;
const BOP: u8 = 139;
const EOP: u8 = 140;
const PUSH: u8 = 141;
const POP: u8 = 142;
const RIGHT1: u8 = 143;
const DOWN1: u8 = 157;
const FNT_NUM_0: u8 = 171;
const FNT1: u8 = 235;
const XXX1: u8 = 239;
const XXX4: u8 = 242;
const FNT_DEF1: u8 = 243;
const PRE: u8 = 247;
const POST: u8 = 248;
const POST_POST: u8 = 249;
const ID_BYTE: u8 = 2;
// y0 - down1, y1 - down1, z0 - down1, z1 - down1 (same offsets for w/x from right1).
const Y0_OFF: u8 = 4;
const Y1_OFF: u8 = 5;
const Z0_OFF: u8 = 9;
const Z1_OFF: u8 = 10;

// Movement node states (§608) and search states (§611).
const Y_HERE: u8 = 1;
const Z_HERE: u8 = 2;
const YZ_OK: u8 = 3;
const Y_OK: u8 = 4;
const Z_OK: u8 = 5;
const D_FIXED: u8 = 6;
const NONE_SEEN: u8 = 0;
const Y_SEEN: u8 = 6;
const Z_SEEN: u8 = 12;

/// tex.web's `dvi_buf_size` (TeX Live: 16384): patching a movement is only
/// possible while its byte is still in the buffer (`dvi_gone`).
const DVI_BUF_SIZE: usize = 16384;
const HALF_BUF: usize = DVI_BUF_SIZE / 2;
const MAX_DIMEN: i32 = 0o7777777777;

#[derive(Clone, Copy, Debug)]
struct Mov {
    width: i32,
    location: usize,
    info: u8,
}

/// Leader subtypes in `Item::Glue::sub`.
const A_LEADERS: u8 = 100;
const C_LEADERS: u8 = 101;

/// The DVI writer of one job.
pub struct Dvi<'f> {
    fonts: &'f dyn Fonts,
    names: &'f Names,
    buf: Vec<u8>,
    /// Bytes already flushed from TeX's buffer (cannot be patched).
    gone: usize,
    high: usize,
    down: Vec<Mov>,
    right: Vec<Mov>,
    dvi_h: i32,
    dvi_v: i32,
    cur_h: i32,
    cur_v: i32,
    dvi_f: Option<FontId>,
    cur_s: i32,
    max_push: i32,
    total_pages: i32,
    last_bop: i32,
    max_v: i32,
    max_h: i32,
    used: BTreeSet<FontId>,
    mag: i32,
    comment: String,
    escape: i32,
}

/// The preamble comment TeX writes (§617): ` TeX output YYYY.MM.DD:HHMM`.
pub fn comment(year: i32, month: i32, day: i32, time: i32) -> String {
    format!(" TeX output {}.{:02}.{:02}:{:02}{:02}", year, month, day, time / 60, time % 60)
}

/// The whole DVI file of these pages (empty when there are none: "No pages of output.").
pub fn write_dvi(pages: &[Arc<Shipped>], fonts: &dyn Fonts, names: &Names, comment: &str) -> Vec<u8> {
    let mut d = Dvi::new(fonts, names, comment);
    for p in pages {
        d.ship(p);
    }
    d.finish()
}

fn param(p: &Shipped, k: &Key) -> Option<i32> {
    p.params.iter().find(|(key, _)| key == k).map(|(_, v)| v.int())
}

fn tex_round(r: f64) -> i32 {
    // web2c's zround.
    if r > 2147483647.0 {
        2147483647
    } else if r < -2147483647.0 {
        -2147483647
    } else if r >= 0.0 {
        (r + 0.5) as i32
    } else {
        (r - 0.5) as i32
    }
}

fn vet_glue(g: f64) -> f64 {
    const BILLION: f64 = 1_000_000_000.0;
    if g > BILLION {
        BILLION
    } else if g < -BILLION {
        -BILLION
    } else {
        g
    }
}

impl<'f> Dvi<'f> {
    pub fn new(fonts: &'f dyn Fonts, names: &'f Names, comment: &str) -> Self {
        Dvi {
            fonts,
            names,
            buf: Vec::new(),
            gone: 0,
            high: 0,
            down: Vec::new(),
            right: Vec::new(),
            dvi_h: 0,
            dvi_v: 0,
            cur_h: 0,
            cur_v: 0,
            dvi_f: None,
            cur_s: -1,
            max_push: 0,
            total_pages: 0,
            last_bop: -1,
            max_v: 0,
            max_h: 0,
            used: BTreeSet::new(),
            mag: 1000,
            comment: comment.to_string(),
            escape: 92,
        }
    }

    pub fn total_pages(&self) -> i32 {
        self.total_pages
    }

    fn out(&mut self, b: u8) {
        self.buf.push(b);
        let l = self.buf.len();
        if l > self.high {
            self.high = l;
            if l >= DVI_BUF_SIZE && l % HALF_BUF == 0 {
                self.gone += HALF_BUF;
            }
        }
    }

    fn four(&mut self, x: i32) {
        for b in x.to_be_bytes() {
            self.out(b);
        }
    }

    /// `dvi_pop` (§601): a `push` just written is taken back.
    fn pop(&mut self, l: usize) {
        let len = self.buf.len();
        let ptr_zero = len % DVI_BUF_SIZE == 0;
        if l == len && !ptr_zero {
            self.buf.pop();
        } else {
            self.out(POP);
        }
    }

    fn font_def(&mut self, f: FontId) {
        let font = self.fonts.font(f);
        let tfm = font.tfm().clone();
        let byte = |i: usize| tfm.get(i).copied().unwrap_or(0);
        self.out(FNT_DEF1);
        self.out(f.wrapping_sub(1) as u8);
        for i in 24..28 {
            self.out(byte(i));
        }
        self.four(font.size());
        let dsize = (((byte(28) as i32) << 16 | (byte(29) as i32) << 8 | byte(30) as i32) << 4) + (byte(31) as i32 >> 4);
        self.four(dsize);
        let name = font.name().to_string();
        let bytes = text_bytes(&name);
        self.out(0); // area length
        self.out(bytes.len() as u8);
        for b in bytes {
            self.out(b);
        }
    }

    /// §607–615.
    fn movement(&mut self, w0: i32, o: u8) {
        let loc = self.buf.len();
        let q = Mov { width: w0, location: loc, info: 0 };
        let down = o == DOWN1;
        {
            let st = if down { &mut self.down } else { &mut self.right };
            st.push(q);
        }
        let n = if down { self.down.len() } else { self.right.len() };
        let qi = n - 1;
        // Look at the other entries (newest first).
        let mut mstate = NONE_SEEN;
        let mut hit: Option<usize> = None;
        let mut patch: Option<(usize, u8, u8)> = None; // (index, byte offset, new info)
        {
            let st = if down { &self.down } else { &self.right };
            let mut i = qi;
            'search: while i > 0 {
                i -= 1;
                let p = st[i];
                if p.width == w0 {
                    match mstate + p.info {
                        s if s == NONE_SEEN + YZ_OK || s == NONE_SEEN + Y_OK || s == Z_SEEN + YZ_OK || s == Z_SEEN + Y_OK => {
                            if p.location < self.gone {
                                break 'search;
                            }
                            patch = Some((i, Y1_OFF, Y_HERE));
                            hit = Some(i);
                            break 'search;
                        }
                        s if s == NONE_SEEN + Z_OK || s == Y_SEEN + YZ_OK || s == Y_SEEN + Z_OK => {
                            if p.location < self.gone {
                                break 'search;
                            }
                            patch = Some((i, Z1_OFF, Z_HERE));
                            hit = Some(i);
                            break 'search;
                        }
                        s if s == NONE_SEEN + Y_HERE || s == NONE_SEEN + Z_HERE || s == Y_SEEN + Z_HERE || s == Z_SEEN + Y_HERE => {
                            hit = Some(i);
                            break 'search;
                        }
                        _ => {}
                    }
                } else {
                    match mstate + p.info {
                        s if s == NONE_SEEN + Y_HERE => mstate = Y_SEEN,
                        s if s == NONE_SEEN + Z_HERE => mstate = Z_SEEN,
                        s if s == Y_SEEN + Z_HERE || s == Z_SEEN + Y_HERE => break 'search,
                        _ => {}
                    }
                }
            }
        }
        if let Some((i, off, info)) = patch {
            let l = if down { self.down[i].location } else { self.right[i].location };
            self.buf[l] += off;
            let st = if down { &mut self.down } else { &mut self.right };
            st[i].info = info;
        }
        match hit {
            None => {
                // Generate a down or right command (§610).
                {
                    let st = if down { &mut self.down } else { &mut self.right };
                    st[qi].info = YZ_OK;
                }
                let mut w = w0;
                if w.unsigned_abs() >= 0o40000000 {
                    self.out(o + 3);
                    self.four(w);
                    return;
                }
                if w.unsigned_abs() >= 0o100000 {
                    self.out(o + 2);
                    if w < 0 {
                        w += 0o100000000;
                    }
                    self.out((w / 0o200000) as u8);
                    w %= 0o200000;
                    self.out((w / 0o400) as u8);
                    self.out((w % 0o400) as u8);
                    return;
                }
                if w.unsigned_abs() >= 0o200 {
                    self.out(o + 1);
                    if w < 0 {
                        w += 0o200000;
                    }
                    self.out((w / 0o400) as u8);
                    self.out((w % 0o400) as u8);
                    return;
                }
                self.out(o);
                if w < 0 {
                    w += 0o400;
                }
                self.out((w % 0o400) as u8);
            }
            Some(pi) => {
                // Generate a y0 or z0 (§612).
                let st = if down { &mut self.down } else { &mut self.right };
                let info = st[pi].info;
                st[qi].info = info;
                if info == Y_HERE {
                    for e in st[pi + 1..qi].iter_mut() {
                        match e.info {
                            YZ_OK => e.info = Z_OK,
                            Y_OK => e.info = D_FIXED,
                            _ => {}
                        }
                    }
                    self.out(o + Y0_OFF);
                } else {
                    for e in st[pi + 1..qi].iter_mut() {
                        match e.info {
                            YZ_OK => e.info = Y_OK,
                            Z_OK => e.info = D_FIXED,
                            _ => {}
                        }
                    }
                    self.out(o + Z0_OFF);
                }
            }
        }
    }

    fn prune(&mut self, l: usize) {
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

    fn set_char(&mut self, f: FontId, c: u16) {
        if self.dvi_f != Some(f) {
            if self.used.insert(f) {
                self.font_def(f);
            }
            let k = f.wrapping_sub(1);
            if k < 64 {
                self.out(FNT_NUM_0 + k as u8);
            } else {
                self.out(FNT1);
                self.out(k as u8);
            }
            self.dvi_f = Some(f);
        }
        if c >= 256 {
            self.out(SET1 + 1);
            self.out((c >> 8) as u8);
            self.out(c as u8);
        } else {
            if c >= 128 {
                self.out(SET1);
            }
            self.out(c as u8);
        }
        self.cur_h += self.fonts.font(f).width(c);
        self.dvi_h = self.cur_h;
    }

    fn special(&mut self, toks: &crate::tok::Toks) {
        self.synch_h();
        self.synch_v();
        let s = crate::diag::token_list_text(self.names, toks, self.escape, &|c| c.is_ascii_alphabetic(), usize::MAX);
        let b = text_bytes(&s);
        if b.len() < 256 {
            self.out(XXX1);
            self.out(b.len() as u8);
        } else {
            self.out(XXX4);
            self.four(b.len() as i32);
        }
        for x in b {
            self.out(x);
        }
    }

    fn list_out(&mut self, b: &BoxNode) {
        if b.vertical {
            self.vlist_out(b)
        } else {
            self.hlist_out(b)
        }
    }

    /// §619.
    fn hlist_out(&mut self, this: &BoxNode) {
        let mut cur_g: i32 = 0;
        let mut cur_glue: f64 = 0.0;
        let g_order = this.set.order;
        let g_sign = this.set.sign;
        self.cur_s += 1;
        if self.cur_s > 0 {
            self.out(PUSH);
        }
        if self.cur_s > self.max_push {
            self.max_push = self.cur_s;
        }
        let save_loc = self.buf.len();
        let base_line = self.cur_v;
        let left_edge = self.cur_h;
        let mut in_run = false;
        for p in this.list.iter() {
            let (f, c) = match p {
                Item::Char { font, c } => (Some(*font), *c),
                Item::Lig { font, c, .. } => (Some(*font), *c),
                _ => (None, 0),
            };
            if let Some(f) = f {
                if !in_run {
                    self.synch_h();
                    self.synch_v();
                    in_run = true;
                }
                self.set_char(f, c);
                continue;
            }
            in_run = false;
            let (mut rule_ht, mut rule_dp, mut rule_wd);
            match p {
                Item::Box(bx) => {
                    if bx.list.is_empty() {
                        self.cur_h += bx.width;
                    } else {
                        let (save_h, save_v) = (self.dvi_h, self.dvi_v);
                        self.cur_v = base_line + bx.shift;
                        let edge = self.cur_h;
                        self.list_out(bx);
                        self.dvi_h = save_h;
                        self.dvi_v = save_v;
                        self.cur_h = edge + bx.width;
                        self.cur_v = base_line;
                    }
                    continue;
                }
                Item::Rule { w, h, d } => {
                    rule_ht = *h;
                    rule_dp = *d;
                    rule_wd = *w;
                }
                Item::Whatsit(wh) => {
                    if let Whatsit::Special { toks } = &**wh {
                        self.special(toks);
                    }
                    continue;
                }
                Item::Glue { g, sub, leader } => {
                    rule_wd = g.width - cur_g;
                    if g_sign != 0 {
                        if g_sign > 0 {
                            if g.stretch_order == g_order {
                                cur_glue += g.stretch as f64;
                                cur_g = tex_round(vet_glue(this.set.ratio * cur_glue));
                            }
                        } else if g.shrink_order == g_order {
                            cur_glue -= g.shrink as f64;
                            cur_g = tex_round(vet_glue(this.set.ratio * cur_glue));
                        }
                    }
                    rule_wd += cur_g;
                    match (leader, *sub >= A_LEADERS) {
                        (Some(lb), true) => match &**lb {
                            Item::Rule { h, d, .. } => {
                                rule_ht = *h;
                                rule_dp = *d;
                            }
                            Item::Box(lbox) => {
                                let leader_wd = lbox.width;
                                if leader_wd > 0 && rule_wd > 0 {
                                    rule_wd += 10;
                                    let edge = self.cur_h + rule_wd;
                                    let mut lx = 0;
                                    if *sub == A_LEADERS {
                                        let save_h = self.cur_h;
                                        self.cur_h = left_edge + leader_wd * ((self.cur_h - left_edge) / leader_wd);
                                        if self.cur_h < save_h {
                                            self.cur_h += leader_wd;
                                        }
                                    } else {
                                        let lq = rule_wd / leader_wd;
                                        let lr = rule_wd % leader_wd;
                                        if *sub == C_LEADERS {
                                            self.cur_h += lr / 2;
                                        } else {
                                            lx = lr / (lq + 1);
                                            self.cur_h += (lr - (lq - 1) * lx) / 2;
                                        }
                                    }
                                    while self.cur_h + leader_wd <= edge {
                                        self.cur_v = base_line + lbox.shift;
                                        self.synch_v();
                                        let save_v = self.dvi_v;
                                        self.synch_h();
                                        let save_h = self.dvi_h;
                                        self.list_out(lbox);
                                        self.dvi_v = save_v;
                                        self.dvi_h = save_h;
                                        self.cur_v = base_line;
                                        self.cur_h = save_h + leader_wd + lx;
                                    }
                                    self.cur_h = edge - 10;
                                    continue;
                                }
                                self.cur_h += rule_wd;
                                continue;
                            }
                            _ => {
                                self.cur_h += rule_wd;
                                continue;
                            }
                        },
                        _ => {
                            self.cur_h += rule_wd;
                            continue;
                        }
                    }
                }
                Item::Kern { w, .. } | Item::Math { w, .. } | Item::MarginKern { w, .. } => {
                    self.cur_h += *w;
                    continue;
                }
                _ => continue,
            }
            // fin_rule (§624).
            if rule_ht == RUNNING {
                rule_ht = this.height;
            }
            if rule_dp == RUNNING {
                rule_dp = this.depth;
            }
            rule_ht += rule_dp;
            if rule_ht > 0 && rule_wd > 0 {
                self.synch_h();
                self.cur_v = base_line + rule_dp;
                self.synch_v();
                self.out(SET_RULE);
                self.four(rule_ht);
                self.four(rule_wd);
                self.cur_v = base_line;
                self.dvi_h += rule_wd;
            }
            self.cur_h += rule_wd;
        }
        self.prune(save_loc);
        if self.cur_s > 0 {
            self.pop(save_loc);
        }
        self.cur_s -= 1;
    }

    /// §629.
    fn vlist_out(&mut self, this: &BoxNode) {
        let mut cur_g: i32 = 0;
        let mut cur_glue: f64 = 0.0;
        let g_order = this.set.order;
        let g_sign = this.set.sign;
        self.cur_s += 1;
        if self.cur_s > 0 {
            self.out(PUSH);
        }
        if self.cur_s > self.max_push {
            self.max_push = self.cur_s;
        }
        let save_loc = self.buf.len();
        let left_edge = self.cur_h;
        self.cur_v -= this.height;
        let top_edge = self.cur_v;
        for p in this.list.iter() {
            let (mut rule_ht, rule_dp, mut rule_wd);
            match p {
                Item::Box(bx) => {
                    if bx.list.is_empty() {
                        self.cur_v += bx.height + bx.depth;
                    } else {
                        self.cur_v += bx.height;
                        self.synch_v();
                        let (save_h, save_v) = (self.dvi_h, self.dvi_v);
                        self.cur_h = left_edge + bx.shift;
                        self.list_out(bx);
                        self.dvi_h = save_h;
                        self.dvi_v = save_v;
                        self.cur_v = save_v + bx.depth;
                        self.cur_h = left_edge;
                    }
                    continue;
                }
                Item::Rule { w, h, d } => {
                    rule_ht = *h;
                    rule_dp = *d;
                    rule_wd = *w;
                }
                Item::Whatsit(wh) => {
                    if let Whatsit::Special { toks } = &**wh {
                        self.special(toks);
                    }
                    continue;
                }
                Item::Glue { g, sub, leader } => {
                    rule_ht = g.width - cur_g;
                    if g_sign != 0 {
                        if g_sign > 0 {
                            if g.stretch_order == g_order {
                                cur_glue += g.stretch as f64;
                                cur_g = tex_round(vet_glue(this.set.ratio * cur_glue));
                            }
                        } else if g.shrink_order == g_order {
                            cur_glue -= g.shrink as f64;
                            cur_g = tex_round(vet_glue(this.set.ratio * cur_glue));
                        }
                    }
                    rule_ht += cur_g;
                    match (leader, *sub >= A_LEADERS) {
                        (Some(lb), true) => match &**lb {
                            Item::Rule { w, .. } => {
                                rule_wd = *w;
                                rule_dp = 0;
                            }
                            Item::Box(lbox) => {
                                let leader_ht = lbox.height + lbox.depth;
                                if leader_ht > 0 && rule_ht > 0 {
                                    rule_ht += 10;
                                    let edge = self.cur_v + rule_ht;
                                    let mut lx = 0;
                                    if *sub == A_LEADERS {
                                        let save_v = self.cur_v;
                                        self.cur_v = top_edge + leader_ht * ((self.cur_v - top_edge) / leader_ht);
                                        if self.cur_v < save_v {
                                            self.cur_v += leader_ht;
                                        }
                                    } else {
                                        let lq = rule_ht / leader_ht;
                                        let lr = rule_ht % leader_ht;
                                        if *sub == C_LEADERS {
                                            self.cur_v += lr / 2;
                                        } else {
                                            lx = lr / (lq + 1);
                                            self.cur_v += (lr - (lq - 1) * lx) / 2;
                                        }
                                    }
                                    while self.cur_v + leader_ht <= edge {
                                        self.cur_h = left_edge + lbox.shift;
                                        self.synch_h();
                                        let save_h = self.dvi_h;
                                        self.cur_v += lbox.height;
                                        self.synch_v();
                                        let save_v = self.dvi_v;
                                        self.list_out(lbox);
                                        self.dvi_v = save_v;
                                        self.dvi_h = save_h;
                                        self.cur_h = left_edge;
                                        self.cur_v = save_v - lbox.height + leader_ht + lx;
                                    }
                                    self.cur_v = edge - 10;
                                    continue;
                                }
                                self.cur_v += rule_ht;
                                continue;
                            }
                            _ => {
                                self.cur_v += rule_ht;
                                continue;
                            }
                        },
                        _ => {
                            self.cur_v += rule_ht;
                            continue;
                        }
                    }
                }
                Item::Kern { w, .. } => {
                    self.cur_v += *w;
                    continue;
                }
                _ => continue,
            }
            // fin_rule (§633).
            if rule_wd == RUNNING {
                rule_wd = this.width;
            }
            rule_ht += rule_dp;
            self.cur_v += rule_ht;
            if rule_ht > 0 && rule_wd > 0 {
                self.synch_h();
                self.synch_v();
                self.out(PUT_RULE);
                self.four(rule_ht);
                self.four(rule_wd);
            }
        }
        self.prune(save_loc);
        if self.cur_s > 0 {
            self.pop(save_loc);
        }
        self.cur_s -= 1;
    }

    /// Ships one page (§640). A page too large for DVI ("Huge page cannot be
    /// shipped out") is skipped; returns false then.
    pub fn ship(&mut self, page: &Shipped) -> bool {
        let p = &*page.b;
        let h_offset = param(page, &Key::DimenPar(dp::of("hoffset"))).unwrap_or(0);
        let v_offset = param(page, &Key::DimenPar(dp::of("voffset"))).unwrap_or(0);
        if let Some(e) = param(page, &Key::IntPar(ip::of("escapechar"))) {
            self.escape = e;
        }
        if p.height > MAX_DIMEN
            || p.depth > MAX_DIMEN
            || p.height.saturating_add(p.depth).saturating_add(v_offset) > MAX_DIMEN
            || p.width.saturating_add(h_offset) > MAX_DIMEN
        {
            return false;
        }
        if p.height + p.depth + v_offset > self.max_v {
            self.max_v = p.height + p.depth + v_offset;
        }
        if p.width + h_offset > self.max_h {
            self.max_h = p.width + h_offset;
        }
        self.dvi_h = 0;
        self.dvi_v = 0;
        self.cur_h = h_offset;
        self.dvi_f = None;
        if self.total_pages == 0 {
            self.mag = param(page, &Key::IntPar(ip::of("mag"))).filter(|m| *m > 0 && *m <= 32768).unwrap_or(1000);
            self.out(PRE);
            self.out(ID_BYTE);
            self.four(25_400_000);
            self.four(473_628_672);
            self.four(self.mag);
            let c = text_bytes(&self.comment);
            self.out(c.len() as u8);
            for b in c {
                self.out(b);
            }
        }
        let page_loc = self.buf.len() as i32;
        self.out(BOP);
        for k in 0..10 {
            self.four(page.counts[k]);
        }
        self.four(self.last_bop);
        self.last_bop = page_loc;
        self.cur_v = p.height + v_offset;
        self.list_out(p);
        self.out(EOP);
        self.total_pages += 1;
        self.cur_s = -1;
        true
    }

    /// The postamble (§642); the file's bytes.
    pub fn finish(mut self) -> Vec<u8> {
        if self.total_pages == 0 {
            return Vec::new();
        }
        self.out(POST);
        self.four(self.last_bop);
        self.last_bop = self.buf.len() as i32 - 5;
        self.four(25_400_000);
        self.four(473_628_672);
        self.four(self.mag);
        self.four(self.max_v);
        self.four(self.max_h);
        self.out((self.max_push / 256) as u8);
        self.out((self.max_push % 256) as u8);
        self.out(((self.total_pages / 256) % 256) as u8);
        self.out((self.total_pages % 256) as u8);
        let used: Vec<FontId> = self.used.iter().rev().copied().collect();
        for f in used {
            self.font_def(f);
        }
        self.out(POST_POST);
        self.four(self.last_bop);
        self.out(ID_BYTE);
        let k = 4 + (4 - self.buf.len() % 4) % 4;
        for _ in 0..k {
            self.out(223);
        }
        self.buf
    }
}

/// A printed string as file bytes: chars up to 255 are one byte each.
fn text_bytes(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len());
    for ch in s.chars() {
        let c = ch as u32;
        if c < 256 {
            v.push(c as u8);
        } else {
            let mut b = [0u8; 4];
            v.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
        }
    }
    v
}

