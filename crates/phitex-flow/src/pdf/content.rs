//! A page's content stream (pdftex.web part 32c, and part 32f's
//! `pdf_hlist_out`, `pdf_vlist_out`, `pdf_ship_out`'s stream): a pure
//! function of the page, what shipping read, the fonts' numbering and
//! what the fold fixed at the first page (`fixed_decimal_digits`).
//!
//! The only thing a page's stream takes from the pages before it is which
//! font a font shares its PDF font object with (`set_ff`: the first font
//! of the same name the document used): the page is made with a guess
//! (the first of that name on the page), recorded in [`PageBody::ff`],
//! and the fold makes it again where the guess does not hold, as the DVI
//! fold does with a page's `fnt_def`s.

use std::sync::Arc;

use crate::dvi::Numbers;
use crate::item::{A_LEADERS, BoxNode, C_LEADERS, Item, Leaders, RUNNING, Whatsit};
use crate::page::Page;
use crate::show::zround;
use crate::tfm::Font;
use crate::val::Glue;

use super::num::{ONE_BP, ONE_HUNDRED_BP, Out, TEN_POW, divide_scaled, round10};
use super::{Fixed, Ship};

/// A glyph set: characters 0–255.
pub type Glyphs = [u64; 4];

/// One font a page uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontUse {
    pub font: Arc<Font>,
    /// Its internal number (TeX's order of loading: `dvi::numbers`).
    pub num: u32,
    /// The characters the page showed of it (`pdf_mark_char`).
    pub glyphs: Glyphs,
}

/// What, on a page, makes an object in pdfTeX's order of creation while
/// the page's stream is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A font set for the first time on this page (`pdf_set_font`): the
    /// fold initializes it if no page before did (`pdf_init_font`).
    Font(u32),
}

/// A page's value in PDF mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageBody {
    /// The content stream, uncompressed.
    pub stream: Vec<u8>,
    /// The stream as the file holds it (compressed at the page's
    /// `\pdfcompresslevel`, or as it is).
    pub data: Vec<u8>,
    /// The fonts the page used, in the order it first set them.
    pub fonts: Vec<FontUse>,
    pub events: Vec<Event>,
    /// For each font set, the font whose `/F` name it was shown with
    /// (`set_ff`), as guessed.
    pub ff: Vec<(u32, u32)>,
    /// The fixed values it was made with.
    pub fixed: Fixed,
    /// `cur_page_width`, `cur_page_height`.
    pub width: i32,
    pub height: i32,
    /// It holds `\openout`, `\write` or `\closeout` whatsits.
    pub io: bool,
}

/// `vet_glue` (tex.web 625).
fn vet_glue(g: f64) -> f64 {
    g.clamp(-1_000_000_000.0, 1_000_000_000.0)
}

/// The glue of a box set so far (`cur_g`, `cur_glue`).
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

/// The writer of one page's stream: pdfTeX's globals of part 32c.
struct W<'a> {
    o: Out,
    numbers: &'a Numbers,
    extra: Vec<Arc<str>>,
    /// `ff` of a font's number, where it is not the font itself.
    ff_of: &'a dyn Fn(u32) -> Option<u32>,
    dd: i32,
    min_bp_val: i32,
    page_height: i32,
    cur_h: i32,
    cur_v: i32,
    pdf_h: i32,
    pdf_v: i32,
    origin_h: i32,
    origin_v: i32,
    tj_start_h: i32,
    delta_h: i32,
    doing_text: bool,
    doing_string: bool,
    /// `pdf_f`, `pdf_last_f` (a font's number; 0 the null font), and
    /// `pdf_last_fs`.
    pdf_f: u32,
    last_f: u32,
    last_fs: i32,
    fonts: Vec<FontUse>,
    events: Vec<Event>,
    ff: Vec<(u32, u32)>,
    io: bool,
}

impl W<'_> {
    fn number(&mut self, f: &Font) -> u32 {
        if let Some(&k) = self.numbers.get(&f.key) {
            return k;
        }
        let i = self.extra.iter().position(|k| *k == f.key).unwrap_or_else(|| {
            self.extra.push(f.key.clone());
            self.extra.len() - 1
        });
        (self.numbers.len() + i + 1) as u32
    }

    /// `pdf_font_size[f]`: the font's size on the raster of 100bp/10^6.
    fn font_size(f: &Font) -> i32 {
        divide_scaled(f.size, ONE_HUNDRED_BP, 6).map_or(0, |r| r.1)
    }

    fn bp(&mut self, s: i32) -> i32 {
        self.o.bp(s, self.dd)
    }

    /// `pdf_set_origin`.
    fn set_origin(&mut self, h: i32, v: i32) {
        if (h - self.origin_h).abs() >= self.min_bp_val || (v - self.origin_v).abs() >= self.min_bp_val {
            self.o.print(b"1 0 0 1 ");
            let out = self.bp(h - self.origin_h);
            self.origin_h += out;
            self.o.out(b' ');
            let out = self.bp(self.origin_v - v);
            self.origin_v -= out;
            self.o.print_ln(b" cm");
        }
        self.pdf_h = self.origin_h;
        self.tj_start_h = self.pdf_h;
        self.pdf_v = self.origin_v;
    }

    /// `pdf_set_origin_temp`.
    fn set_origin_temp(&mut self, h: i32, v: i32) {
        if (h - self.origin_h).abs() >= self.min_bp_val || (v - self.origin_v).abs() >= self.min_bp_val {
            self.o.print(b"1 0 0 1 ");
            self.bp(h - self.origin_h);
            self.o.out(b' ');
            self.bp(self.origin_v - v);
            self.o.print_ln(b" cm");
        }
    }

    fn end_string(&mut self) {
        if self.doing_string {
            self.o.print(b")]TJ");
            self.doing_string = false;
        }
    }

    fn end_string_nl(&mut self) {
        if self.doing_string {
            self.o.print_ln(b")]TJ");
            self.doing_string = false;
        }
    }

    /// `pdf_set_text_pos` (fonts are not expanded: `pdf_cur_Tm_a` is 0).
    fn set_text_pos(&mut self, v: i32, v_out: i32) {
        self.o.out(b' ');
        let out = self.bp(self.cur_h - self.tj_start_h);
        self.pdf_h = self.tj_start_h + out;
        self.o.out(b' ');
        self.o.real(v, self.dd);
        self.pdf_v -= v_out;
        self.o.print(b" Td");
        self.tj_start_h = self.pdf_h;
        self.delta_h = 0;
    }

    /// The font `f` (number `k`) shares its object with (`set_ff`).
    fn ff(&self, k: u32) -> u32 {
        (self.ff_of)(k).unwrap_or(k)
    }

    /// `pdf_set_font`.
    fn set_font(&mut self, f: &Arc<Font>, k: u32) {
        self.pdf_f = k;
        if !self.fonts.iter().any(|u| u.num == k) {
            self.fonts.push(FontUse { font: f.clone(), num: k, glyphs: [0; 4] });
            self.events.push(Event::Font(k));
            let ff = self.ff(k);
            self.ff.push((k, ff));
        }
        let ff = self.ff(k);
        if ff == self.last_f && f.size == self.last_fs {
            return;
        }
        self.o.print(b"/F");
        self.o.int(i64::from(ff));
        self.o.out(b' ');
        let q = divide_scaled(f.size, ONE_HUNDRED_BP, 6).map_or(0, |r| r.0);
        self.o.real(q, 4);
        self.o.print(b" Tf");
        self.last_f = ff;
        self.last_fs = f.size;
    }

    /// `pdf_begin_text`.
    fn begin_text(&mut self) {
        let h = self.page_height;
        self.set_origin(0, h);
        self.o.print_ln(b"BT");
        self.doing_text = true;
        self.pdf_f = 0;
        self.last_f = 0;
        self.last_fs = 0;
        self.doing_string = false;
    }

    /// `pdf_begin_string` (no faked interword spaces, no expansion).
    fn begin_string(&mut self, f: &Arc<Font>, k: u32) {
        let mut must_set_text_pos = false;
        if !self.doing_text {
            self.begin_text();
            must_set_text_pos = true;
        }
        if self.pdf_f != k {
            self.end_string();
            self.set_font(f, k);
        }
        let size = Self::font_size(f);
        let (mut s, s_out) =
            divide_scaled(self.cur_h - (self.tj_start_h + self.delta_h), size, 3).unwrap_or((0, 0));
        let (v, v_out) = if (self.cur_v - self.pdf_v).abs() >= self.min_bp_val {
            divide_scaled(self.pdf_v - self.cur_v, ONE_HUNDRED_BP, self.dd + 2).unwrap_or((0, 0))
        } else {
            (0, 0)
        };
        if !must_set_text_pos {
            must_set_text_pos = v != 0 || s.abs() >= 0o100000;
        }
        if must_set_text_pos {
            self.end_string();
            self.set_font(f, k);
            self.set_text_pos(v, v_out);
            s = 0;
        }
        if !self.doing_string {
            self.o.print(b" [");
            if s == 0 {
                self.o.out(b'(');
            }
        }
        if s != 0 {
            if self.doing_string {
                self.o.out(b')');
            }
            self.o.int(i64::from(-s));
            self.o.out(b'(');
            self.delta_h += s_out;
        }
        self.doing_string = true;
    }

    /// `pdf_end_text`.
    fn end_text(&mut self) {
        if self.doing_text {
            self.end_string_nl();
            self.o.print_ln(b"ET");
            self.doing_text = false;
        }
    }

    /// `pdf_mark_char`.
    fn mark(&mut self, k: u32, c: u8) {
        if let Some(u) = self.fonts.iter_mut().find(|u| u.num == k) {
            u.glyphs[usize::from(c >> 6)] |= 1 << (c & 63);
        }
    }

    /// `output_one_char`: `pdf_begin_string`, `pdf_print_char`,
    /// `adv_char_width`.
    fn char_out(&mut self, f: &Arc<Font>, c: u8) {
        let k = self.number(f);
        self.begin_string(f, k);
        self.mark(k, c);
        if c <= 32 || c == 92 || c == 40 || c == 41 || c > 127 {
            self.o.out(b'\\');
            self.o.octal(c);
        } else {
            self.o.out(c);
        }
        // (`adv_char_width`: the width on the raster of the `/Widths`)
        let w = f.glyph(i32::from(c)).map_or(0, |g| g.width);
        let (s, s_out) = divide_scaled(w, Self::font_size(f), 4).unwrap_or((0, 0));
        let _ = round10(s);
        self.delta_h += s_out;
    }

    /// `pdf_set_rule`.
    fn set_rule(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.end_text();
        self.o.print_ln(b"q");
        if h <= ONE_BP {
            // (`y - (h + 1)/2`: a real division, truncated)
            let origin = ((2 * i64::from(y) - i64::from(h) - 1) / 2) as i32;
            self.set_origin_temp(x, origin);
            self.o.print(b"[]0 d 0 J ");
            self.bp(h);
            self.o.print(b" w 0 0 m ");
            self.bp(w);
            self.o.print_ln(b" 0 l S");
        } else if w <= ONE_BP {
            let origin = ((2 * i64::from(x) + i64::from(w) + 1) / 2) as i32;
            self.set_origin_temp(origin, y);
            self.o.print(b"[]0 d 0 J ");
            self.bp(w);
            self.o.print(b" w 0 0 m 0 ");
            self.bp(h);
            self.o.print_ln(b" l S");
        } else {
            self.set_origin_temp(x, y);
            self.o.print(b"0 0 ");
            self.bp(w);
            self.o.out(b' ');
            self.bp(h);
            self.o.print_ln(b" re f");
        }
        self.o.print_ln(b"Q");
    }

    /// `literal` of a `\special` (`scan_special`): only `PDF:`/`pdf:` ones
    /// are written.
    fn special(&mut self, s: &str) {
        let b: Vec<u8> = s.chars().map(|c| u32::from(c).min(255) as u8).collect();
        if !(b.starts_with(b"PDF:") || b.starts_with(b"pdf:")) {
            return;
        }
        let mut j = 4;
        let mode = if b[4..].starts_with(b"direct:") {
            j += 7;
            2
        } else if b[4..].starts_with(b"page:") {
            j += 5;
            1
        } else {
            0
        };
        self.literal(&b[j..], mode);
    }

    /// `literal` in mode `set_origin` (0), `direct_page` (1),
    /// `direct_always` (2).
    fn literal(&mut self, s: &[u8], mode: u8) {
        match mode {
            0 => {
                self.end_text();
                let (h, v) = (self.cur_h, self.cur_v);
                self.set_origin(h, v);
            }
            1 => self.end_text(),
            _ => self.end_string_nl(),
        }
        self.o.print(s);
        self.o.nl();
    }

    fn list_out(&mut self, b: &BoxNode) {
        if b.vertical {
            self.vlist_out(b);
        } else {
            self.hlist_out(b);
        }
    }

    /// `pdf_hlist_out`.
    fn hlist_out(&mut self, this: &BoxNode) {
        let mut glue = SetGlue::default();
        let base_line = self.cur_v;
        let left_edge = self.cur_h;
        for it in &this.list {
            match it {
                Item::Char(f, c) => self.char_item(f, *c),
                Item::Lig(l) => {
                    let f = l.font.clone();
                    self.char_item(&f, l.c);
                }
                Item::Box(b) => {
                    if b.list.is_empty() {
                        self.cur_h += b.w;
                    } else {
                        self.cur_v = base_line + b.shift;
                        let edge = self.cur_h + b.w;
                        self.list_out(b);
                        self.cur_h = edge;
                        self.cur_v = base_line;
                    }
                }
                Item::Rule { w, h, d } => self.hrule(this, *h, *d, *w, base_line),
                Item::Glue { g, .. } => {
                    let g = g.clone();
                    self.cur_h += glue.set(this, &g);
                }
                Item::Leaders(l) => {
                    let rule_wd = glue.set(this, &l.g);
                    self.hleaders(this, l, rule_wd, base_line, left_edge);
                }
                Item::Kern { w, .. } => self.cur_h += *w,
                Item::Math { w, .. } => self.cur_h += *w,
                Item::Whatsit(w) => self.whatsit(w),
                _ => {}
            }
        }
    }

    fn char_item(&mut self, f: &Arc<Font>, c: u8) {
        match f.glyph(i32::from(c)) {
            Some(g) => {
                self.char_out(f, c);
                self.cur_h += g.width;
            }
            // (`char_warning`: a missing character is not shown)
            None => {}
        }
    }

    /// A rule in an hlist, then past it.
    fn hrule(&mut self, this: &BoxNode, h: i32, d: i32, w: i32, base_line: i32) {
        let ht = if h == RUNNING { this.h } else { h };
        let dp = if d == RUNNING { this.d } else { d };
        let ht = ht + dp;
        if ht > 0 && w > 0 {
            self.cur_v = base_line + dp;
            let (x, y) = (self.cur_h, self.cur_v);
            self.set_rule(x, y, w, ht);
            self.cur_v = base_line;
        }
        self.cur_h += w;
    }

    fn hleaders(&mut self, this: &BoxNode, l: &Leaders, rule_wd: i32, base_line: i32, left_edge: i32) {
        match &l.b {
            Item::Rule { h, d, .. } => self.hrule(this, *h, *d, rule_wd, base_line),
            Item::Box(lb) if lb.w > 0 && rule_wd > 0 => {
                let leader_wd = lb.w;
                let rule_wd = rule_wd + 10;
                let edge = self.cur_h + rule_wd;
                let mut lx = 0;
                if l.sub == A_LEADERS {
                    let save_h = self.cur_h;
                    self.cur_h = left_edge + leader_wd * ((self.cur_h - left_edge) / leader_wd);
                    if self.cur_h < save_h {
                        self.cur_h += leader_wd;
                    }
                } else {
                    let lq = rule_wd / leader_wd;
                    let lr = rule_wd % leader_wd;
                    if l.sub == C_LEADERS {
                        self.cur_h += lr / 2;
                    } else {
                        lx = lr / (lq + 1);
                        self.cur_h += (lr - (lq - 1) * lx) / 2;
                    }
                }
                while self.cur_h + leader_wd <= edge {
                    self.cur_v = base_line + lb.shift;
                    let save_h = self.cur_h;
                    self.list_out(lb);
                    self.cur_v = base_line;
                    self.cur_h = save_h + leader_wd + lx;
                }
                self.cur_h = edge - 10;
            }
            _ => self.cur_h += rule_wd,
        }
    }

    /// `pdf_vlist_out`.
    fn vlist_out(&mut self, this: &BoxNode) {
        let mut glue = SetGlue::default();
        let left_edge = self.cur_h;
        self.cur_v -= this.h;
        let top_edge = self.cur_v;
        for it in &this.list {
            match it {
                Item::Box(b) => {
                    if b.list.is_empty() {
                        self.cur_v += b.h + b.d;
                    } else {
                        self.cur_v += b.h;
                        let save_v = self.cur_v;
                        self.cur_h = left_edge + b.shift;
                        self.list_out(b);
                        self.cur_v = save_v + b.d;
                        self.cur_h = left_edge;
                    }
                }
                Item::Rule { w, h, d } => self.vrule(this, *h + *d, *w, left_edge),
                Item::Glue { g, .. } => self.cur_v += glue.set(this, g),
                Item::Leaders(l) => {
                    let rule_ht = glue.set(this, &l.g);
                    self.vleaders(this, l, rule_ht, left_edge, top_edge);
                }
                Item::Kern { w, .. } => self.cur_v += *w,
                Item::Whatsit(w) => self.whatsit(w),
                _ => {}
            }
        }
    }

    /// A rule of thickness `ht` in a vlist.
    fn vrule(&mut self, this: &BoxNode, ht: i32, w: i32, left_edge: i32) {
        let wd = if w == RUNNING { this.w } else { w };
        self.cur_v += ht;
        if ht > 0 && wd > 0 {
            let (x, y) = (self.cur_h, self.cur_v);
            self.set_rule(x, y, wd, ht);
            self.cur_h = left_edge;
        }
    }

    fn vleaders(&mut self, this: &BoxNode, l: &Leaders, rule_ht: i32, left_edge: i32, top_edge: i32) {
        match &l.b {
            Item::Rule { w, .. } => self.vrule(this, rule_ht, *w, left_edge),
            Item::Box(lb) if lb.h + lb.d > 0 && rule_ht > 0 => {
                let leader_ht = lb.h + lb.d;
                let rule_ht = rule_ht + 10;
                let edge = self.cur_v + rule_ht;
                let mut lx = 0;
                if l.sub == A_LEADERS {
                    let save_v = self.cur_v;
                    self.cur_v = top_edge + leader_ht * ((self.cur_v - top_edge) / leader_ht);
                    if self.cur_v < save_v {
                        self.cur_v += leader_ht;
                    }
                } else {
                    let lq = rule_ht / leader_ht;
                    let lr = rule_ht % leader_ht;
                    if l.sub == C_LEADERS {
                        self.cur_v += lr / 2;
                    } else {
                        lx = lr / (lq + 1);
                        self.cur_v += (lr - (lq - 1) * lx) / 2;
                    }
                }
                while self.cur_v + leader_ht <= edge {
                    self.cur_h = left_edge + lb.shift;
                    self.cur_v += lb.h;
                    let save_v = self.cur_v;
                    self.list_out(lb);
                    self.cur_h = left_edge;
                    self.cur_v = save_v - lb.h + leader_ht + lx;
                }
                self.cur_v = edge - 10;
            }
            _ => self.cur_v += rule_ht,
        }
    }

    fn whatsit(&mut self, w: &Whatsit) {
        match w {
            Whatsit::Special(s) => self.special(s),
            _ => self.io = true,
        }
    }
}

/// `min_bp_val` for `dd` decimal digits.
pub fn min_bp_val(dd: i32) -> i32 {
    divide_scaled(ONE_HUNDRED_BP, TEN_POW[(dd + 2).clamp(0, 9) as usize], 0).map_or(0, |r| r.0)
}

/// Page `p`'s content stream, with fonts numbered by `numbers` (then in
/// the order met) and shown by the font `ff_of` says they share their
/// object with (themselves where it says nothing), with `fixed` the
/// values the first page fixed.
pub fn page_body(p: &Page, ship: &Ship, numbers: &Numbers, ff_of: &dyn Fn(u32) -> Option<u32>, fixed: Fixed) -> PageBody {
    let b = &p.b;
    let dd = fixed.decimal_digits;
    // (§752: "Calculate page dimensions and margins")
    let h_offset = ship.horigin + p.ship.hoffset;
    let v_offset = ship.vorigin + p.ship.voffset;
    let width = if ship.page_width != 0 { ship.page_width } else { b.w + 2 * h_offset };
    let height = if ship.page_height != 0 { ship.page_height } else { b.h + b.d + 2 * v_offset };
    let mut w = W {
        o: Out::new(),
        numbers,
        extra: Vec::new(),
        ff_of,
        dd,
        min_bp_val: min_bp_val(dd),
        page_height: height,
        cur_h: h_offset,
        cur_v: b.h + v_offset,
        pdf_h: 0,
        pdf_v: 0,
        origin_h: 0,
        origin_v: height,
        tj_start_h: 0,
        delta_h: 0,
        doing_text: false,
        doing_string: false,
        pdf_f: 0,
        last_f: 0,
        last_fs: 0,
        fonts: Vec::new(),
        events: Vec::new(),
        ff: Vec::new(),
        io: false,
    };
    // ("Adjust transformation matrix for the magnification ratio")
    let mag = p.ship.mag;
    if mag != 1000 {
        w.o.real(mag, 3);
        w.o.print(b" 0 0 ");
        w.o.real(mag, 3);
        w.o.print_ln(b" 0 0 cm");
    }
    w.list_out(b);
    w.end_text();
    let stream = w.o.b;
    let data = if ship.compress_level > 0 { super::zlib::compress(&stream, ship.compress_level) } else { stream.clone() };
    PageBody { stream, data, fonts: w.fonts, events: w.events, ff: w.ff, fixed, width, height, io: w.io }
}

/// A page's own guess of `set_ff`: each font shares its object with the
/// first font of the same name the page sets (the fold checks the guess
/// against the fonts the pages before used).
pub fn guess_ff(p: &Page, numbers: &Numbers) -> Vec<(u32, u32)> {
    let mut first: Vec<(String, u32)> = Vec::new();
    let mut out = Vec::new();
    let mut extra: Vec<Arc<str>> = Vec::new();
    let walk = |list: &[Item], f: &mut dyn FnMut(&Arc<Font>)| {
        fn rec(list: &[Item], f: &mut dyn FnMut(&Arc<Font>)) {
            for it in list {
                match it {
                    Item::Char(font, _) => f(font),
                    Item::Lig(l) => f(&l.font),
                    Item::Box(b) => rec(&b.list, f),
                    Item::Leaders(l) => {
                        if let Item::Box(b) = &l.b {
                            rec(&b.list, f);
                        }
                    }
                    _ => {}
                }
            }
        }
        rec(list, f);
    };
    walk(&p.b.list, &mut |font: &Arc<Font>| {
        let k = numbers.get(&font.key).copied().unwrap_or_else(|| {
            let i = extra.iter().position(|x| *x == font.key).unwrap_or_else(|| {
                extra.push(font.key.clone());
                extra.len() - 1
            });
            (numbers.len() + i + 1) as u32
        });
        if out.iter().any(|&(n, _)| n == k) {
            return;
        }
        let owner = match first.iter().find(|(n, _)| *n == font.name) {
            Some(&(_, o)) => o,
            None => {
                first.push((font.name.clone(), k));
                k
            }
        };
        out.push((k, owner));
    });
    out
}

/// Page `p`'s value in PDF mode, where it is shipped: its stream made
/// with its own guesses (its own `\pdfdecimaldigits` as the fixed one,
/// [`guess_ff`] for the fonts' objects); the fold makes it again where
/// they do not hold.
pub fn page(p: &Page, ship: &Ship, numbers: &Numbers) -> PageBody {
    let ff = guess_ff(p, numbers);
    let ff_of = |k: u32| ff.iter().find(|x| x.0 == k).map(|x| x.1).filter(|&o| o != k);
    page_body(p, ship, numbers, &ff_of, Fixed::of(ship))
}

/// A page body with nothing (a page shipped in DVI mode in a PDF file).
pub fn empty() -> PageBody {
    PageBody {
        stream: Vec::new(),
        data: Vec::new(),
        fonts: Vec::new(),
        events: Vec::new(),
        ff: Vec::new(),
        fixed: Fixed { decimal_digits: 3 },
        width: 0,
        height: 0,
        io: false,
    }
}
