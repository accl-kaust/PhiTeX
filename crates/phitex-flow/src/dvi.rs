//! DVI: tex.web part 32 (§583–§643), copied. A page's commands are a pure
//! function of the page and of what crosses its `bop` (§640: at `bop`
//! h=v=w=x=y=z=0, the stack is empty, `dvi_f` is null; the top box's
//! `prune_movements` leaves no movement from before the page): the
//! numbers of its fonts, which of them are defined already (`fnt_def` comes
//! at a font's first use), and — only when a movement it looked back at is
//! farther than half TeX's buffer, or an empty push meets the buffer's
//! edge — where in the file it starts (`dvi_gone`, §611; `dvi_pop`, §601).
//! [`page_body`] writes a page under those assumptions and records them;
//! [`assemble`] lays the bodies end to end, patching each `bop`'s back
//! pointer, and writes again only a body whose assumptions do not hold.

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

/// Each font's internal number (TeX's: the order of first loading).
pub type Numbers = std::collections::HashMap<Arc<str>, u32>;

/// A page's DVI commands, from its `bop` to its `eop` (the back pointer
/// left 0), and what they assumed about what crosses the `bop`.
#[derive(Debug)]
pub struct Body {
    pub bytes: Vec<u8>,
    /// The fonts it uses, in the order of first use, with their numbers,
    /// where in `bytes` that first use is and how long the `fnt_def`
    /// written there is (0: none, the font taken as defined before).
    pub fonts: Vec<(Arc<Font>, u32, u32, u32)>,
    /// `None`: no command depends on where the page starts or on which
    /// fonts are defined before it — those `fnt_def`s are put in or left
    /// out where they stand. Otherwise what it was written for.
    pub fixed: Option<Fixed>,
    pub max_push: i32,
}

/// What a page that depends on where it starts was written for.
#[derive(Debug, PartialEq, Eq)]
pub struct Fixed {
    /// The numbers of its fonts taken as defined before it, sorted.
    pub defined: Vec<u32>,
    /// A movement looked back farther than half the buffer (`dvi_gone`,
    /// §611): the start's class, [`gone_class`].
    pub gone: Option<i32>,
    /// Where (from the start) `dvi_pop` met an empty push, and whether
    /// that was at the buffer's edge (§601).
    pub edges: Vec<(i32, bool)>,
}

/// What of a start offset `o` the `dvi_gone` test sees: all of it before
/// the buffer was first full, then `o` modulo half the buffer (§598).
fn gone_class(o: i32) -> i32 {
    if o < BUF_SIZE { o } else { BUF_SIZE + o % HALF_BUF }
}

impl Body {
    /// Is it this page's commands at offset `at`, with fonts numbered by
    /// `number` and those `used` already defined?
    fn holds(&self, at: i32, number: &mut impl FnMut(&Font) -> u32, used: &std::collections::HashSet<u32>) -> bool {
        if !self.fonts.iter().all(|(f, n, _, _)| number(f) == *n) {
            return false;
        }
        let Some(x) = &self.fixed else { return true };
        let mut defined: Vec<u32> = self.fonts.iter().map(|f| f.1).filter(|n| used.contains(n)).collect();
        defined.sort_unstable();
        defined == x.defined
            && x.gone.is_none_or(|c| c == gone_class(at))
            && x.edges.iter().all(|&(r, e)| ((at + r) % BUF_SIZE == 0) == e)
    }
}

/// The DVI writer's state for one page (tex.web 592–595, 605, 616).
struct Writer<'a> {
    out: Vec<u8>,
    /// Where `out` starts in the file.
    origin: i32,
    max_push: i32,
    down: Vec<Movement>,
    right: Vec<Movement>,
    dvi_h: i32,
    dvi_v: i32,
    cur_h: i32,
    cur_v: i32,
    dvi_f: u32,
    cur_s: i32,
    numbers: &'a Numbers,
    /// Fonts not in `numbers`, numbered after them in the order met.
    extra: Vec<Arc<str>>,
    /// The fonts defined before the page, and those it used.
    defined: &'a [u32],
    fonts: Vec<(Arc<Font>, u32, u32, u32)>,
    /// The farthest a movement search looked back at a matching buffered
    /// movement, and where `dvi_pop` met an empty push.
    lookback: i32,
    edges: Vec<(i32, bool)>,
}

impl Writer<'_> {
    fn loc(&self) -> i32 {
        self.origin + self.out.len() as i32
    }

    fn number(&mut self, f: &Font) -> u32 {
        number(self.numbers, &mut self.extra, f)
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
        if l == self.loc() {
            self.edges.push((l - self.origin, self.loc() % BUF_SIZE == 0));
        }
        if l == self.loc() && self.loc() % BUF_SIZE != 0 {
            self.out.pop();
        } else {
            self.byte(POP);
        }
    }

    /// §602: `dvi_font_def` of font number `n`.
    fn font_def(&mut self, f: &Font, n: u32) {
        font_def(&mut self.out, f, n);
    }

    /// §621: change font `dvi_f` to `f`.
    fn change_font(&mut self, f: &Arc<Font>) {
        let k = self.number(f);
        if !self.fonts.iter().any(|x| x.1 == k) {
            let at = self.out.len();
            if !self.defined.contains(&k) {
                self.font_def(f, k);
            }
            self.fonts.push((f.clone(), k, at as u32, (self.out.len() - at) as u32));
        }
        let n = k as i32 - 1;
        if k <= 64 {
            self.int(n + i32::from(FNT_NUM_0));
        } else {
            self.byte(FNT1);
            self.int(n);
        }
        self.dvi_f = k;
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
        let mut lookback = self.lookback;
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
                        lookback = lookback.max(location - m.location);
                        if m.location < gone {
                            break;
                        }
                        // (§613: change the buffered instruction to `y` or `w`)
                        retag = Some((p, Y1 - DOWN1, Y_HERE));
                        found = Some(p);
                        break;
                    }
                    x if x == NONE_SEEN + Z_OK || x == Y_SEEN + YZ_OK || x == Y_SEEN + Z_OK => {
                        lookback = lookback.max(location - m.location);
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
        self.lookback = lookback;
        if let Some((p, delta, tag)) = retag {
            let k = (stack[p].location - self.origin) as usize;
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
        if self.number(f) != self.dvi_f {
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

    /// §640: one page, from `bop` to `eop` (the back pointer left 0).
    fn page(&mut self, p: &Page) {
        let b = &p.b;
        let s = &p.ship;
        // (§617)
        self.dvi_h = 0;
        self.dvi_v = 0;
        self.cur_h = s.hoffset;
        self.dvi_f = 0;
        self.byte(BOP);
        for &c in &s.counts {
            self.four(c);
        }
        self.four(0);
        self.cur_v = b.h + s.voffset;
        self.list_out(b);
        self.byte(EOP);
        self.cur_s = -1;
    }
}

/// Font `f`'s number: by `numbers`, else after them in the order of
/// `extra` (where a font not met before is put).
fn number(numbers: &Numbers, extra: &mut Vec<Arc<str>>, f: &Font) -> u32 {
    if let Some(&k) = numbers.get(&f.key) {
        return k;
    }
    let i = extra.iter().position(|k| *k == f.key).unwrap_or_else(|| {
        extra.push(f.key.clone());
        extra.len() - 1
    });
    (numbers.len() + i + 1) as u32
}

/// The length of font `f`'s `fnt_def` (§602).
fn def_len(f: &Font) -> i32 {
    16 + f.name.len() as i32
}

/// Page `p`'s commands, with fonts numbered by `numbers` (then `extra`),
/// those numbered `defined` already defined, written at file offset `at`.
pub fn page_body(p: &Page, numbers: &Numbers, extra: &[Arc<str>], defined: &[u32], at: i32) -> Body {
    let mut w = Writer {
        out: Vec::new(),
        origin: at,
        max_push: 0,
        down: Vec::new(),
        right: Vec::new(),
        dvi_h: 0,
        dvi_v: 0,
        cur_h: 0,
        cur_v: 0,
        dvi_f: 0,
        cur_s: -1,
        numbers,
        extra: extra.to_vec(),
        defined,
        fonts: Vec::new(),
        lookback: 0,
        edges: Vec::new(),
    };
    w.page(p);
    // (putting in or leaving out its `fnt_def`s moves a look back by at
    // most all of them)
    let slack: i32 = w.fonts.iter().map(|x| def_len(&x.0)).sum();
    let far = w.lookback + slack > HALF_BUF;
    let fixed = (far || !w.edges.is_empty()).then(|| {
        let mut defined: Vec<u32> = w.fonts.iter().filter(|x| x.3 == 0).map(|x| x.1).collect();
        defined.sort_unstable();
        Fixed { defined, gone: far.then(|| gone_class(at)), edges: w.edges }
    });
    Body { bytes: w.out, fonts: w.fonts, fixed, max_push: w.max_push }
}

/// The preamble's comment (tex.web 617): ` TeX output YYYY.MM.DD:HHMM`.
fn comment(date: [i32; 4]) -> String {
    let two = |n: i32| format!("{:02}", n.rem_euclid(100));
    format!(" TeX output {}.{}.{}:{}{}", date[0], two(date[1]), two(date[2]), two(date[3] / 60), two(date[3] % 60))
}

/// §602: `dvi_font_def` of font number `n`.
fn font_def(out: &mut Vec<u8>, f: &Font, n: u32) {
    out.push(FNT_DEF1);
    out.push((n - 1) as u8);
    out.extend(f.checksum.to_be_bytes());
    out.extend(f.size.to_be_bytes());
    out.extend(f.design_size.to_be_bytes());
    out.push(0);
    out.push(f.name.len() as u8);
    out.extend(f.name.as_bytes());
}

/// The DVI file of these pages, with fonts numbered by `numbers` (none: no
/// file, as TeX writes none): the preamble, the pages' bodies end to end,
/// each `bop` pointing back at the one before (§640), the postamble
/// (§642). A body that does not hold where it lands is written again
/// there, in `bodies`; returns the file and how many were.
pub fn assemble(numbers: &Numbers, pages: &[Page], bodies: &mut [Arc<Body>]) -> (Vec<u8>, usize) {
    let Some(first) = pages.first() else { return (Vec::new(), 0) };
    let mut out = Vec::new();
    // (§617: the preamble, at the first page)
    out.push(PRE);
    out.push(ID_BYTE);
    out.extend(25_400_000i32.to_be_bytes());
    out.extend(473_628_672i32.to_be_bytes());
    out.extend(first.ship.mag.to_be_bytes());
    let c = comment(first.ship.date);
    out.push(c.len() as u8);
    out.extend(c.as_bytes());
    let mut extra: Vec<Arc<str>> = Vec::new();
    let mut defined = std::collections::HashSet::new();
    let mut used: std::collections::BTreeMap<u32, Arc<Font>> = std::collections::BTreeMap::new();
    let (mut max_v, mut max_h, mut max_push, mut last_bop) = (0, 0, 0, -1i32);
    let mut again = 0;
    for (p, b) in pages.iter().zip(bodies.iter_mut()) {
        let (bx, s) = (&p.b, &p.ship);
        // (§641)
        max_v = max_v.max(bx.h + bx.d + s.voffset);
        max_h = max_h.max(bx.w + s.hoffset);
        let at = out.len() as i32;
        if !b.holds(at, &mut |f| number(numbers, &mut extra, f), &defined) {
            let mut d: Vec<u32> = defined.iter().copied().collect();
            d.sort_unstable();
            *b = Arc::new(page_body(p, numbers, &extra, &d, at));
            again += 1;
        }
        if b.fixed.is_some() {
            out.extend(&b.bytes);
        } else {
            let mut from = 0;
            for (f, n, pos, len) in &b.fonts {
                let (pos, len) = (*pos as usize, *len as usize);
                out.extend(&b.bytes[from..pos]);
                if !defined.contains(n) {
                    if len > 0 {
                        out.extend(&b.bytes[pos..pos + len]);
                    } else {
                        font_def(&mut out, f, *n);
                    }
                }
                from = pos + len;
            }
            out.extend(&b.bytes[from..]);
        }
        for (f, n, _, _) in &b.fonts {
            number(numbers, &mut extra, f);
            defined.insert(*n);
            used.entry(*n).or_insert_with(|| f.clone());
        }
        // (the back pointer, after `bop` and `count0..9`)
        let k = at as usize + 41;
        out[k..k + 4].copy_from_slice(&last_bop.to_be_bytes());
        last_bop = at;
        max_push = max_push.max(b.max_push);
    }
    // (§642: the postamble)
    let total_pages = pages.len() as i32;
    let mag = pages.last().map_or(1000, |p| p.ship.mag);
    let post = out.len() as i32;
    out.push(POST);
    for x in [last_bop, 25_400_000, 473_628_672, mag, max_v, max_h] {
        out.extend(x.to_be_bytes());
    }
    for x in [max_push / 256, max_push % 256, (total_pages / 256) % 256, total_pages % 256] {
        out.push((x & 0xFF) as u8);
    }
    // (§643: the fonts used, last loaded first)
    for (k, f) in used.iter().rev() {
        font_def(&mut out, f, *k);
    }
    out.push(POST_POST);
    out.extend(post.to_be_bytes());
    out.push(ID_BYTE);
    let ptr = out.len() as i32 % BUF_SIZE;
    let k = 4 + (BUF_SIZE - ptr) % 4;
    out.extend(std::iter::repeat_n(223, k as usize));
    (out, again)
}

/// Each font's number: the order in which the pieces load them first.
pub fn numbers<'a>(pieces: impl IntoIterator<Item = &'a [Arc<str>]>) -> Numbers {
    let mut m = Numbers::new();
    for fs in pieces {
        extend(&mut m, fs);
    }
    m
}

/// Numbers the fonts `fs` not numbered yet, in order.
pub fn extend(m: &mut Numbers, fs: &[Arc<str>]) {
    for f in fs {
        let n = m.len() as u32 + 1;
        m.entry(f.clone()).or_insert(n);
    }
}
