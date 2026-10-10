//! A page's content stream (pdftex.web part 32c, and part 32f's
//! `pdf_hlist_out`, `pdf_vlist_out`, `pdf_ship_out`'s stream), and a
//! form's (`\pdfxform`: the same walk with `shipping_page` false): a pure
//! function of the box, what shipping read, the fonts' numbering, what
//! the fold fixed at the first page (`fixed_decimal_digits`), and what
//! the pages before left of the walk's state ([`WalkState`]: the colour
//! and matrix stacks, the links still open, the article thread, the last
//! `\pdfsavepos`).
//!
//! What a page's stream takes from the pages before it — which font a
//! font shares its PDF font object with (`set_ff`), the walk's state at
//! its start, what a form or an image referred to by number is — the
//! page is made with a guess of (recorded in [`PageBody::ff`],
//! [`PageBody::entry`], [`PageBody::incomplete`]), and the fold makes it
//! again where the guess does not hold, as the DVI fold does with a
//! page's `fnt_def`s.
//!
//! The marks a page makes (annotations, links and their pieces on each
//! line, destinations, thread beads, the objects, forms and images it
//! refers to) are [`Event`]s in the walk's order, which is pdfTeX's order
//! of making their objects; their rectangles are the walk's, in the
//! page's coordinates (`cur_h`, `cur_v`).

use std::sync::Arc;

use crate::dvi::Numbers;
use crate::item::{A_LEADERS, BoxNode, C_LEADERS, Item, Leaders, RUNNING, Whatsit};
use crate::page::Page;
use crate::show::zround;
use crate::tfm::Font;
use crate::val::Glue;

use super::colorstack::Stacks;
use super::image::{Image, ext_xn_over_d};
use super::num::{ONE_BP, ONE_HUNDRED_BP, Out, TEN_POW, divide_scaled, round10};
use super::what::{self, Action, Id, Text, What};
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

/// A rectangle as pdfTeX keeps one (`pdf_left`, `pdf_top`, `pdf_right`,
/// `pdf_bottom`), in the page's coordinates.
pub type Rect = [i32; 4];

/// What, on a page, makes or refers to an object, in pdfTeX's order of
/// doing so while the page's stream is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A font set for the first time on this page (`pdf_set_font`): the
    /// fold initializes it if no page before did (`pdf_init_font`).
    Font(u32),
    /// `do_annot`: an annotation (a new object if its own was scheduled
    /// already).
    Annot { objnum: i32, rect: Rect, data: Text },
    /// `do_link`: a link's start (`links[link]`).
    Link { objnum: i32, link: usize },
    /// `append_link`: a link's piece in another line (`links[link]`), a
    /// new object.
    AppendLink { link: usize },
    /// `do_dest` (the fold ignores a duplicate, as `do_dest` does).
    Dest { id: Id, struct_num: Option<i32>, kind: u8, zoom: Option<i32>, rect: Rect },
    /// `do_thread`, `append_thread`: bead `beads[bead]` of thread `id`.
    Bead { id: Id, bead: usize, attr: Option<Text> },
    /// `\pdfrefobj`, `out_form`, `out_image`: an object, a form, an image
    /// the page refers to.
    RefObj(i32),
    Form(i32),
    Image(i32),
}

/// A link annotation of the page: its rectangle (as `end_link` left
/// it) and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRec {
    pub rect: Rect,
    pub attr: Option<Text>,
    pub action: Arc<Action>,
}

/// An entry of `pdf_link_stack`: a link begun and not ended, at box
/// nesting level `level`; its annotation on this page (`ref_link_node`),
/// if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkFrame {
    pub level: i32,
    pub objnum: i32,
    pub w: i32,
    pub h: i32,
    pub d: i32,
    pub attr: Option<Text>,
    pub action: Arc<Action>,
    pub ref_link: Option<usize>,
}

/// The walk's state that goes on from page to page (pdfTeX's globals the
/// next page's walk starts with).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkState {
    pub stacks: Stacks,
    pub links: Vec<LinkFrame>,
    /// `gen_running_link`.
    pub running_link: bool,
    /// `pdf_thread_level`; `pdf_thread_wd`, `_ht`, `_dp`;
    /// `pdf_last_thread_id`.
    pub thread_level: i32,
    pub thread_dims: (i32, i32, i32),
    pub thread_id: Option<Id>,
    /// `pdf_last_x_pos`, `pdf_last_y_pos`.
    pub last_pos: (i32, i32),
}

impl Default for WalkState {
    fn default() -> Self {
        let mut stacks = Stacks::default();
        stacks.settle();
        WalkState {
            stacks,
            links: Vec::new(),
            running_link: true,
            thread_level: -1,
            thread_dims: (0, 0, 0),
            thread_id: None,
            last_pos: (0, 0),
        }
    }
}

/// A form referred to: its resource name's number (`obj_info`) and its
/// depth; an image: its number and its description.
#[derive(Clone, Debug)]
pub struct FormRef {
    pub index: i64,
    pub depth: i32,
}

#[derive(Clone, Debug)]
pub struct ImageRef {
    pub index: i64,
    pub image: Arc<Image>,
}

/// What the walk asks of the fold's object table: forms and images by
/// number. Where a page is made it knows none (`incomplete`).
pub trait Objs {
    fn form(&self, n: i32) -> Option<FormRef>;
    fn image(&self, n: i32) -> Option<ImageRef>;
}

/// The table where a page is made: nothing known.
pub struct NoObjs;

impl Objs for NoObjs {
    fn form(&self, _: i32) -> Option<FormRef> {
        None
    }
    fn image(&self, _: i32) -> Option<ImageRef> {
        None
    }
}

/// Where a glyph run or a rule was put (for SyncTeX and the viewer): its
/// origin in the page's coordinates (`cur_h`, `cur_v`), its font's
/// number (0: a rule) and first character, the item that made it as the
/// walk sees it (the indices of the boxes from the page's down, then the
/// item's index in its list), and the source of the node that made the
/// item, once items know it (`None` for now: CORE.md, the provenance
/// gap).
#[derive(Clone, Debug)]
pub struct Origin {
    pub h: i32,
    pub v: i32,
    pub font: u32,
    pub first_char: u8,
    pub item: Vec<u32>,
    pub src: Option<(u32, u32)>,
}

/// A page's origins: not part of its value (two pages equal but for
/// them are equal).
#[derive(Clone, Debug, Default)]
pub struct Origins(pub Vec<Origin>);

impl PartialEq for Origins {
    fn eq(&self, _: &Origins) -> bool {
        true
    }
}

impl Eq for Origins {}

/// `\pdflinkmargin`, `\pdfdestmargin`, `\pdfthreadmargin` where the page
/// is shipped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Margins {
    pub link: i32,
    pub dest: i32,
    pub thread: i32,
}

/// A page's value in PDF mode (or a form's).
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
    /// The page's link annotations and its beads' rectangles.
    pub links: Vec<LinkRec>,
    pub beads: Vec<Rect>,
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
    /// `pdf_last_x_pos`, `pdf_last_y_pos` at each of them (outside
    /// leaders, in the order `graph::io::whatsits` lists them), for the
    /// `\write`s that read `\pdflastxpos`.
    pub write_pos: Vec<(i32, i32)>,
    /// The walk's state it began with (a guess where the page is made)
    /// and left.
    pub entry: Arc<WalkState>,
    pub exit: Arc<WalkState>,
    /// A form or an image it refers to was not known where it was made:
    /// the fold makes it again.
    pub incomplete: bool,
    /// What the walk had to say: `pdf_error`s (fatal), warnings.
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub origins: Origins,
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

/// The writer of one page's (or form's) stream: pdfTeX's globals of part
/// 32c and 32f.
struct W<'a> {
    o: Out,
    numbers: &'a Numbers,
    extra: Vec<Arc<str>>,
    /// `ff` of a font's number, where it is not the font itself.
    ff_of: &'a dyn Fn(u32) -> Option<u32>,
    objs: &'a dyn Objs,
    dd: i32,
    min_bp_val: i32,
    /// `cur_page_height` (in a form too: the page's being shipped, or
    /// the last one's).
    page_height: i32,
    /// `is_shipping_page`.
    page: bool,
    margins: Margins,
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
    links: Vec<LinkRec>,
    beads: Vec<Rect>,
    /// `last_thread`: the bead whose bottom `\pdfendthread` sets.
    last_thread: Option<usize>,
    ff: Vec<(u32, u32)>,
    io: bool,
    write_pos: Vec<(i32, i32)>,
    st: WalkState,
    /// `cur_s`, `doing_leaders`.
    cur_s: i32,
    doing_leaders: bool,
    /// A form's `pdf_xform_height + pdf_xform_depth`.
    form_hd: i32,
    incomplete: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
    origins: Vec<Origin>,
    /// The path of the list being walked; whether the item before was a
    /// character of the run `origins`' last begun.
    path: Vec<u32>,
    in_run: bool,
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
        // (`cur_page_height`: the page's, also in a form)
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
            must_set_text_pos = v != 0 || s.abs() >= 0o100_000;
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
            what::DIRECT_ALWAYS
        } else if b[4..].starts_with(b"page:") {
            j += 5;
            what::DIRECT_PAGE
        } else {
            what::SET_ORIGIN
        };
        self.literal(&b[j..], mode);
    }

    /// `literal` in mode `set_origin` (0), `direct_page` (1),
    /// `direct_always` (2).
    fn literal(&mut self, s: &[u8], mode: u8) {
        match mode {
            what::SET_ORIGIN => {
                self.end_text();
                let (h, v) = (self.cur_h, self.cur_v);
                self.set_origin(h, v);
            }
            what::DIRECT_PAGE => self.end_text(),
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

    /// An origin for a glyph run or a rule, item `i` of the list walked.
    fn origin(&mut self, i: usize, font: u32, first_char: u8) {
        let mut item = self.path.clone();
        item.push(i as u32);
        self.origins.push(Origin { h: self.cur_h, v: self.cur_v, font, first_char, item, src: None });
    }

    /// `pdf_hlist_out`.
    fn hlist_out(&mut self, this: &BoxNode) {
        let mut glue = SetGlue::default();
        self.cur_s += 1;
        let base_line = self.cur_v;
        let left_edge = self.cur_h;
        // ("Create link annotations for the current hbox if needed")
        for i in 0..self.st.links.len() {
            let fr = &self.st.links[i];
            if fr.level == self.cur_s && self.st.running_link && fr.w == RUNNING {
                self.append_link(this, left_edge, base_line, i);
            }
        }
        self.in_run = false;
        for (i, it) in this.list.iter().enumerate() {
            let run = matches!(it, Item::Char(..) | Item::Lig(_));
            match it {
                Item::Char(f, c) => self.char_item(i, f, *c),
                Item::Lig(l) => {
                    let f = l.font.clone();
                    self.char_item(i, &f, l.c);
                }
                Item::Box(b) => {
                    if b.list.is_empty() {
                        self.cur_h += b.w;
                    } else {
                        self.cur_v = base_line + b.shift;
                        let edge = self.cur_h + b.w;
                        self.path.push(i as u32);
                        self.list_out(b);
                        self.path.pop();
                        self.cur_h = edge;
                        self.cur_v = base_line;
                    }
                }
                Item::Rule { w, h, d } => {
                    self.origin(i, 0, 0);
                    self.hrule(this, *h, *d, *w, base_line);
                }
                Item::Glue { g, .. } => {
                    let g = g.clone();
                    self.cur_h += glue.set(this, &g);
                }
                Item::Leaders(l) => {
                    let rule_wd = glue.set(this, &l.g);
                    self.path.push(i as u32);
                    self.hleaders(this, l, rule_wd, base_line, left_edge);
                    self.path.pop();
                }
                Item::Kern { w, .. } | Item::Math { w, .. } => self.cur_h += *w,
                Item::Whatsit(w) => self.whatsit(w, this, left_edge, base_line, false),
                _ => {}
            }
            self.in_run = run;
        }
        self.in_run = false;
        self.cur_s -= 1;
    }

    fn char_item(&mut self, i: usize, f: &Arc<Font>, c: u8) {
        // (`char_warning`: a missing character is not shown)
        if let Some(g) = f.glyph(i32::from(c)) {
            if !self.in_run {
                let k = self.number(f);
                self.origin(i, k, c);
            }
            self.char_out(f, c);
            self.cur_h += g.width;
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
                    let outer = self.doing_leaders;
                    self.doing_leaders = true;
                    self.list_out(lb);
                    self.doing_leaders = outer;
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
        self.cur_s += 1;
        let left_edge = self.cur_h;
        self.cur_v -= this.h;
        let top_edge = self.cur_v;
        // ("Create thread for the current vbox if needed")
        if self.last_thread.is_some() && self.st.thread_dims.2 == RUNNING && self.st.thread_level == self.cur_s {
            self.append_thread(this, left_edge, top_edge + this.h);
        }
        for (i, it) in this.list.iter().enumerate() {
            match it {
                Item::Box(b) => {
                    if b.list.is_empty() {
                        self.cur_v += b.h + b.d;
                    } else {
                        self.cur_v += b.h;
                        let save_v = self.cur_v;
                        self.cur_h = left_edge + b.shift;
                        self.path.push(i as u32);
                        self.list_out(b);
                        self.path.pop();
                        self.cur_v = save_v + b.d;
                        self.cur_h = left_edge;
                    }
                }
                Item::Rule { w, h, d } => {
                    self.origin(i, 0, 0);
                    self.vrule(this, *h + *d, *w, left_edge);
                }
                Item::Glue { g, .. } => self.cur_v += glue.set(this, g),
                Item::Leaders(l) => {
                    let rule_ht = glue.set(this, &l.g);
                    self.path.push(i as u32);
                    self.vleaders(this, l, rule_ht, left_edge, top_edge);
                    self.path.pop();
                }
                Item::Kern { w, .. } => self.cur_v += *w,
                Item::Whatsit(w) => self.whatsit(w, this, left_edge, top_edge + this.h, true),
                _ => {}
            }
        }
        self.cur_s -= 1;
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
                    let outer = self.doing_leaders;
                    self.doing_leaders = true;
                    self.list_out(lb);
                    self.doing_leaders = outer;
                    self.cur_h = left_edge;
                    self.cur_v = save_v - lb.h + leader_ht + lx;
                }
                self.cur_v = edge - 10;
            }
            _ => self.cur_v += rule_ht,
        }
    }

    // ---- whatsits ("Output the whatsit node |p| in |pdf_hlist_out|",
    // "... in |pdf_vlist_out|")

    /// A whatsit of the box `this` (`x`, `y`: its left edge and base
    /// line, or top edge plus height in a vlist).
    fn whatsit(&mut self, w: &Whatsit, this: &BoxNode, x: i32, y: i32, vlist: bool) {
        match w {
            Whatsit::Special(s) => self.special(s),
            Whatsit::Pdf(p) => self.pdf_whatsit(p, this, x, y, vlist),
            Whatsit::Open { .. } | Whatsit::Write { .. } | Whatsit::Close { .. } => {
                self.io = true;
                if !self.doing_leaders {
                    self.write_pos.push(self.st.last_pos);
                }
            }
        }
    }

    fn error(&mut self, s: &str) {
        self.errors.push(format!("pdfTeX error (ext4): {s}"));
    }

    #[allow(clippy::too_many_lines)]
    fn pdf_whatsit(&mut self, p: &What, this: &BoxNode, x: i32, y: i32, vlist: bool) {
        match p {
            What::Literal { mode, data } => self.literal(data, *mode),
            What::ColorStack { stack, cmd, data } => self.colorstack(*stack, *cmd, data.as_deref()),
            What::SetMatrix(data) => {
                // (`pdf_out_setmatrix`)
                let v = self.page_height - self.cur_v;
                if self.st.stacks.set_matrix(data, self.cur_h, v) {
                    let mut s = data.to_vec();
                    s.extend_from_slice(b" 0 0 cm");
                    self.literal(&s, what::SET_ORIGIN);
                } else {
                    self.errors.push("pdfTeX error (\\pdfsetmatrix): Unrecognized format.".into());
                }
            }
            What::Save => {
                let (h, v) = (self.cur_h, self.cur_v);
                self.st.stacks.save(h, v);
                self.literal(b"q", what::SET_ORIGIN);
            }
            What::Restore => {
                let (h, v) = (self.cur_h, self.cur_v);
                if let Some(w) = self.st.stacks.restore(h, v) {
                    self.warnings.push(format!("pdfTeX warning: {}", String::from_utf8_lossy(&w)));
                }
                self.literal(b"Q", what::SET_ORIGIN);
            }
            What::RefObj(n) => self.events.push(Event::RefObj(*n)),
            What::RefXform { n, w, h, d } => {
                if vlist {
                    self.cur_v += h;
                    let save_v = self.cur_v;
                    self.cur_h = x;
                    self.out_form(*n);
                    self.cur_v = save_v + d;
                    self.cur_h = x;
                } else {
                    self.cur_v = y;
                    let edge = self.cur_h;
                    self.out_form(*n);
                    self.cur_h = edge + w;
                    self.cur_v = y;
                }
            }
            What::RefXimage { n, w, h, d } => {
                if vlist {
                    self.cur_v += h + d;
                    let save_v = self.cur_v;
                    self.cur_h = x;
                    self.out_image(*n, *w, *h, *d);
                    self.cur_v = save_v;
                    self.cur_h = x;
                } else {
                    self.cur_v = y + d;
                    let edge = self.cur_h;
                    self.out_image(*n, *w, *h, *d);
                    self.cur_h = edge + w;
                    self.cur_v = y;
                }
            }
            What::Annot { objnum, w, h, d, data } => {
                // (`do_annot`)
                if !self.page {
                    self.error("annotations cannot be inside an XForm");
                    return;
                }
                if self.doing_leaders {
                    return;
                }
                let rect = self.rect_dimens(this, x, y, (*w, *h, *d), 0);
                self.events.push(Event::Annot { objnum: *objnum, rect, data: data.clone() });
            }
            What::StartLink { objnum, w, h, d, attr, action } => {
                if vlist {
                    self.error("\\pdfstartlink ended up in vlist");
                    return;
                }
                // (`do_link`, `push_link_level`)
                if !self.page {
                    self.error("link annotations cannot be inside an XForm");
                    return;
                }
                if self.st.links.len() >= 10 {
                    self.errors.push("! TeX capacity exceeded, sorry [pdf link stack size=10].".into());
                    return;
                }
                let link = self.links.len();
                self.st.links.push(LinkFrame {
                    level: self.cur_s,
                    objnum: *objnum,
                    w: *w,
                    h: *h,
                    d: *d,
                    attr: attr.clone(),
                    action: action.clone(),
                    ref_link: Some(link),
                });
                let rect = self.rect_dimens(this, x, y, (*w, *h, *d), self.margins.link);
                self.links.push(LinkRec { rect, attr: attr.clone(), action: action.clone() });
                self.events.push(Event::Link { objnum: *objnum, link });
            }
            What::EndLink => {
                if vlist {
                    self.error("\\pdfendlink ended up in vlist");
                    return;
                }
                self.end_link();
            }
            What::Dest { id, struct_num, kind, zoom, w, h, d } => {
                // (`do_dest`; the duplicate check is the fold's)
                if !self.page {
                    self.error("destinations cannot be inside an XForm");
                    return;
                }
                if self.doing_leaders {
                    return;
                }
                let matrix = self.st.stacks.matrix_used();
                let mut rect = [0; 4];
                let dims = (*w, *h, *d);
                match *kind {
                    what::DEST_FIT | what::DEST_FITB => {}
                    what::DEST_FITR => rect = self.rect_dimens(this, x, y, dims, self.margins.dest),
                    _ if matrix => rect = self.rect_dimens(this, x, y, dims, self.margins.dest),
                    what::DEST_XYZ => {
                        rect[0] = self.cur_h;
                        rect[1] = self.cur_v;
                    }
                    what::DEST_FITH | what::DEST_FITBH => rect[1] = self.cur_v,
                    _ => rect[0] = self.cur_h,
                }
                self.events.push(Event::Dest { id: id.clone(), struct_num: *struct_num, kind: *kind, zoom: *zoom, rect });
            }
            What::Thread { start, id, w, h, d, attr } => {
                if *start && !vlist {
                    self.error("\\pdfstartthread ended up in hlist");
                    return;
                }
                self.do_thread(*start, id, (*w, *h, *d), attr.clone(), (this, x, y));
            }
            What::EndThread => {
                if !vlist {
                    self.error("\\pdfendthread ended up in hlist");
                    return;
                }
                self.end_thread();
            }
            What::SavePos => {
                // ("Save current position to |pdf_last_x_pos|, ...")
                let y = if self.page { self.page_height - self.cur_v } else { self.form_hd - self.cur_v };
                self.st.last_pos = (self.cur_h, y);
            }
            What::RunningLink(on) => self.st.running_link = *on,
        }
    }

    /// `set_rect_dimens`: the rectangle of a mark of size `(w, h, d)`
    /// (running: the parent box's) at `cur_h`, `cur_v`, through the
    /// matrix of `\pdfsetmatrix` if any, with a margin.
    fn rect_dimens(&mut self, parent: &BoxNode, x: i32, y: i32, (w, h, d): (i32, i32, i32), margin: i32) -> Rect {
        let mut left = self.cur_h;
        let mut right = if w == RUNNING { x + parent.w } else { self.cur_h + w };
        let mut top = if h == RUNNING { y - parent.h } else { self.cur_v - h };
        let mut bottom = if d == RUNNING { y + parent.d } else { self.cur_v + d };
        if self.page && self.st.stacks.matrix_used() {
            let ph = self.page_height;
            let r = self.st.stacks.transform_rect(left, ph - bottom, right, ph - top);
            left = r[0];
            bottom = ph - r[1];
            right = r[2];
            top = ph - r[3];
        }
        [left - margin, top - margin, right + margin, bottom + margin]
    }

    /// `append_link`: link `i` of the stack goes on in this box.
    fn append_link(&mut self, this: &BoxNode, x: i32, y: i32, i: usize) {
        let fr = self.st.links[i].clone();
        let link = self.links.len();
        self.st.links[i].ref_link = Some(link);
        let rect = self.rect_dimens(this, x, y, (fr.w, fr.h, fr.d), self.margins.link);
        self.links.push(LinkRec { rect, attr: fr.attr, action: fr.action });
        self.events.push(Event::AppendLink { link });
    }

    /// `end_link`.
    fn end_link(&mut self) {
        let Some(top) = self.st.links.last().cloned() else {
            self.error("pdf_link_stack empty, \\pdfendlink used without \\pdfstartlink?");
            return;
        };
        if top.level != self.cur_s {
            self.warnings.push("pdfTeX warning: \\pdfendlink ended up in different nesting level than \\pdfstartlink".into());
        }
        if top.w == RUNNING
            && let Some(r) = top.ref_link
        {
            let m = self.margins.link;
            if self.page && self.st.stacks.matrix_used() {
                let ph = self.page_height;
                let t = self.st.stacks.recalculate(self.cur_h + m);
                self.links[r].rect = [t[0] - m, ph - t[3] - m, t[2] + m, ph - t[1] + m];
            } else {
                self.links[r].rect[2] = self.cur_h + m;
            }
        }
        self.st.links.pop();
    }

    /// `do_thread`, `append_bead`.
    fn do_thread(&mut self, start: bool, id: &Id, dims: (i32, i32, i32), attr: Option<Text>, (this, x, y): (&BoxNode, i32, i32)) {
        if self.doing_leaders {
            return;
        }
        if start {
            self.st.thread_dims = dims;
            self.st.thread_id = Some(id.clone());
            self.st.thread_level = self.cur_s;
        }
        let rect = self.rect_dimens(this, x, y, dims, self.margins.thread);
        if !self.page {
            self.error("threads cannot be inside an XForm");
            return;
        }
        let bead = self.beads.len();
        self.beads.push(rect);
        self.events.push(Event::Bead { id: id.clone(), bead, attr });
        self.last_thread = Some(bead);
    }

    /// `append_thread`.
    fn append_thread(&mut self, this: &BoxNode, x: i32, y: i32) {
        let Some(id) = self.st.thread_id.clone() else { return };
        let dims = self.st.thread_dims;
        let rect = self.rect_dimens(this, x, y, dims, self.margins.thread);
        if !self.page {
            self.error("threads cannot be inside an XForm");
            return;
        }
        let bead = self.beads.len();
        self.beads.push(rect);
        self.events.push(Event::Bead { id, bead, attr: None });
        self.last_thread = Some(bead);
    }

    /// `end_thread`.
    fn end_thread(&mut self) {
        if self.st.thread_level != self.cur_s {
            self.error("\\pdfendthread ended up in different nesting level than \\pdfstartthread");
            return;
        }
        if self.st.thread_dims.2 == RUNNING
            && let Some(b) = self.last_thread
        {
            self.beads[b][3] = self.cur_v + self.margins.thread;
        }
        self.last_thread = None;
    }

    /// `out_form`.
    fn out_form(&mut self, n: i32) {
        self.end_text();
        self.o.print_ln(b"q");
        let f = self.objs.form(n).unwrap_or_else(|| {
            self.incomplete = true;
            FormRef { index: 0, depth: 0 }
        });
        self.events.push(Event::Form(n));
        self.cur_v += f.depth;
        self.o.print(b"1 0 0 1 ");
        self.bp(self.cur_h - self.origin_h);
        self.o.out(b' ');
        self.bp(self.origin_v - self.cur_v);
        self.o.print_ln(b" cm");
        self.o.print(b"/Fm");
        self.o.int(f.index);
        self.o.print_ln(b" Do");
        self.o.print_ln(b"Q");
    }

    /// `out_image` (the page group an image asks for is the fold's).
    fn out_image(&mut self, n: i32, w: i32, h: i32, d: i32) {
        let r = self.objs.image(n);
        if r.is_none() {
            self.incomplete = true;
        }
        self.end_text();
        self.o.print_ln(b"q");
        self.events.push(Event::Image(n));
        let (x, y) = (self.cur_h - self.origin_h, self.origin_v - self.cur_v);
        let index = r.as_ref().map_or(0, |r| r.index);
        match r.as_ref().and_then(|r| r.image.pdf.as_ref().map(|p| (p, r.image.shown()))) {
            Some((p, (img_w, img_h))) => {
                self.o.real(ext_xn_over_d(w, TEN_POW[6], img_w), 6);
                self.o.print(b" 0 0 ");
                self.o.real(ext_xn_over_d(h + d, TEN_POW[6], img_h), 6);
                self.o.out(b' ');
                self.bp(x - ext_xn_over_d(w, p.orig_x, img_w));
                self.o.out(b' ');
                self.bp(y - ext_xn_over_d(h + d, p.orig_y, img_h));
            }
            None => {
                self.o.real(ext_xn_over_d(w, TEN_POW[6], ONE_HUNDRED_BP), 4);
                self.o.print(b" 0 0 ");
                self.o.real(ext_xn_over_d(h + d, TEN_POW[6], ONE_HUNDRED_BP), 4);
                self.o.out(b' ');
                self.bp(x);
                self.o.out(b' ');
                self.bp(y);
            }
        }
        self.o.print_ln(b" cm");
        self.o.print(b"/Im");
        self.o.int(index);
        self.o.print_ln(b" Do");
        self.o.print_ln(b"Q");
    }

    /// `pdf_out_colorstack`.
    fn colorstack(&mut self, n: i32, cmd: u8, data: Option<&[u8]>) {
        if n < 0 || n as usize >= self.st.stacks.used() {
            self.warnings.push(format!("Color stack {n} is not initialized for use!"));
            return;
        }
        match cmd {
            what::CS_SET | what::CS_PUSH => {
                let s = data.unwrap_or_default();
                let mode = if cmd == what::CS_SET { self.st.stacks.set(n, s) } else { self.st.stacks.push(n, s) };
                if !s.is_empty() {
                    self.literal(s, mode as u8);
                }
            }
            what::CS_POP => {
                let (mode, v) = self.st.stacks.pop(n);
                match v {
                    None => {
                        let which = if self.page { "page" } else { "form" };
                        self.warnings.push(format!("pdfTeX warning: pop empty color {which} stack {n}"));
                    }
                    Some(v) if !v.is_empty() => self.literal(&v, mode as u8),
                    Some(_) => {}
                }
            }
            _ => {
                let (mode, v) = self.st.stacks.current(n);
                if !v.is_empty() {
                    self.literal(&v, mode as u8);
                }
            }
        }
    }

    /// `pdf_out_colorstack_startpage`.
    fn colorstack_startpage(&mut self) {
        let max = self.st.stacks.used();
        for i in 0..max {
            let i = i as i32;
            if self.st.stacks.skip_page_start(i) == 0 {
                let (mode, v) = self.st.stacks.current(i);
                if !v.is_empty() {
                    self.literal(&v, mode as u8);
                }
            }
        }
    }
}

/// `min_bp_val` for `dd` decimal digits.
pub fn min_bp_val(dd: i32) -> i32 {
    divide_scaled(ONE_HUNDRED_BP, TEN_POW[(dd + 2).clamp(0, 9) as usize], 0).map_or(0, |r| r.0)
}

/// What a page's or a form's walk is given: the fonts' numbering (by
/// `numbers`, then `extra` in the order met), the font each shares its
/// object with (`ff_of`: itself where it says nothing), what the first
/// page fixed, the forms and images known, the walk's state at its
/// start.
pub struct Given<'a> {
    pub numbers: &'a Numbers,
    pub extra: &'a [Arc<str>],
    pub ff_of: &'a dyn Fn(u32) -> Option<u32>,
    pub fixed: Fixed,
    pub objs: &'a dyn Objs,
    pub entry: &'a Arc<WalkState>,
}

fn new_writer<'a>(g: &Given<'a>, page: bool, margins: Margins, page_height: i32) -> W<'a> {
    let dd = g.fixed.decimal_digits;
    W {
        o: Out::new(),
        numbers: g.numbers,
        extra: g.extra.to_vec(),
        ff_of: g.ff_of,
        objs: g.objs,
        dd,
        min_bp_val: min_bp_val(dd),
        page_height,
        page,
        margins,
        cur_h: 0,
        cur_v: 0,
        pdf_h: 0,
        pdf_v: 0,
        origin_h: 0,
        origin_v: 0,
        tj_start_h: 0,
        delta_h: 0,
        doing_text: false,
        doing_string: false,
        pdf_f: 0,
        last_f: 0,
        last_fs: 0,
        fonts: Vec::new(),
        events: Vec::new(),
        links: Vec::new(),
        beads: Vec::new(),
        last_thread: None,
        ff: Vec::new(),
        io: false,
        write_pos: Vec::new(),
        st: (**g.entry).clone(),
        cur_s: -1,
        doing_leaders: false,
        form_hd: 0,
        incomplete: false,
        errors: Vec::new(),
        warnings: Vec::new(),
        origins: Vec::new(),
        path: Vec::new(),
        in_run: false,
    }
}

/// The stream's end (`pdf_end_text`, `pdfshipoutend`) and the value.
fn finish(mut w: W<'_>, entry: &Arc<WalkState>, compress_level: i32, width: i32, height: i32) -> PageBody {
    w.end_text();
    if !w.st.stacks.pos.is_empty() {
        let what = if w.page { "page" } else { "form" };
        w.errors.push(format!("pdfTeX error: {} unmatched \\pdfsave after {what} shipout", w.st.stacks.pos.len()));
    }
    let stream = std::mem::take(&mut w.o.b);
    let data = if compress_level > 0 { super::zlib::compress(&stream, compress_level) } else { stream.clone() };
    w.st.stacks.settle();
    // (a link's annotation on this page is written with it: the next
    // page's walk makes its own, `append_link`)
    for f in &mut w.st.links {
        f.ref_link = None;
    }
    let exit = if w.st == **entry { entry.clone() } else { Arc::new(w.st) };
    PageBody {
        stream,
        data,
        fonts: w.fonts,
        events: w.events,
        links: w.links,
        beads: w.beads,
        ff: w.ff,
        fixed: Fixed { decimal_digits: w.dd },
        width,
        height,
        io: w.io,
        write_pos: w.write_pos,
        entry: entry.clone(),
        exit,
        incomplete: w.incomplete,
        errors: w.errors,
        warnings: w.warnings,
        origins: Origins(w.origins),
    }
}

/// Page `p`'s content stream and marks (`pdf_ship_out` with
/// `shipping_page` true).
pub fn page_body(p: &Page, ship: &Ship, g: &Given<'_>) -> PageBody {
    let b = &p.b;
    // ("Calculate page dimensions and margins")
    let h_offset = ship.horigin + p.ship.hoffset;
    let v_offset = ship.vorigin + p.ship.voffset;
    let width = if ship.page_width != 0 { ship.page_width } else { b.w + 2 * h_offset };
    let height = if ship.page_height != 0 { ship.page_height } else { b.h + b.d + 2 * v_offset };
    let mut w = new_writer(g, true, ship.margins, height);
    w.cur_h = h_offset;
    w.cur_v = b.h + v_offset;
    w.origin_v = height;
    // ("Adjust transformation matrix for the magnification ratio")
    let mag = p.ship.mag;
    if mag != 1000 {
        w.o.real(mag, 3);
        w.o.print(b" 0 0 ");
        w.o.real(mag, 3);
        w.o.print_ln(b" 0 0 cm");
    }
    w.st.stacks.ship_begin(true);
    w.colorstack_startpage();
    w.list_out(b);
    finish(w, g.entry, ship.compress_level, width, height)
}

/// A form's content stream (`pdf_ship_out` with `shipping_page` false):
/// box `b` at the form's origin, `page_height` the `cur_page_height` it is
/// written under (the page's being shipped, or the last page's: pdfTeX
/// begins text there), `compress_level` the one it is written with.
pub fn form_body(b: &BoxNode, page_height: i32, compress_level: i32, margins: Margins, g: &Given<'_>) -> PageBody {
    let hd = b.h + b.d;
    let mut w = new_writer(g, false, margins, page_height);
    w.form_hd = hd;
    w.cur_v = b.h;
    w.cur_h = 0;
    w.origin_v = hd;
    w.st.stacks.ship_begin(false);
    w.list_out(b);
    finish(w, g.entry, compress_level, b.w, hd)
}

/// A page's own guess of `set_ff`: each font shares its object with the
/// first font of the same name the page sets (the fold checks the guess
/// against the fonts the pages before used).
pub fn guess_ff(p: &Page, numbers: &Numbers, extra: &[Arc<str>]) -> Vec<(u32, u32)> {
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
    let mut first: Vec<(String, u32)> = Vec::new();
    let mut out = Vec::new();
    let mut extra: Vec<Arc<str>> = extra.to_vec();
    rec(&p.b.list, &mut |font: &Arc<Font>| {
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
        let owner = if let Some(&(_, o)) = first.iter().find(|(n, _)| *n == font.name) {
            o
        } else {
            first.push((font.name.clone(), k));
            k
        };
        out.push((k, owner));
    });
    out
}

/// Page `p`'s value in PDF mode, where it is shipped: its stream made
/// with its own guesses (its own `\pdfdecimaldigits` as the fixed one,
/// [`guess_ff`] for the fonts' objects, the walk's state a document
/// starts with, no form or image known); the fold makes it again where
/// they do not hold, and where a font's number (`numbers`, then `extra`
/// as the DVI writer numbers them where the page is shipped) is not the
/// job's.
pub fn page(p: &Page, ship: &Ship, numbers: &Numbers, extra: &[Arc<str>]) -> PageBody {
    let ff = guess_ff(p, numbers, extra);
    let ff_of = |k: u32| ff.iter().find(|x| x.0 == k).map(|x| x.1).filter(|&o| o != k);
    let entry = Arc::new(WalkState::default());
    let g = Given { numbers, extra, ff_of: &ff_of, fixed: Fixed::of(ship), objs: &NoObjs, entry: &entry };
    page_body(p, ship, &g)
}

/// Where `\pdfsavepos` was last at each `\openout`, `\write`, `\closeout`
/// of page `p` (in the order `graph::io::whatsits` lists them), for the
/// `\write`s that read `\pdflastxpos`: none in DVI mode or without a
/// `\pdfsavepos` on the page (a `\write` before the page's first one
/// reads 0, not the page before's: CORE.md, gaps).
pub fn write_positions(p: &Page) -> Vec<(i32, i32)> {
    fn has_savepos(list: &[Item]) -> bool {
        list.iter().any(|it| match it {
            Item::Box(b) => has_savepos(&b.list),
            Item::Leaders(l) => matches!(&l.b, Item::Box(b) if has_savepos(&b.list)),
            Item::Whatsit(w) => matches!(&**w, Whatsit::Pdf(p) if matches!(**p, What::SavePos)),
            _ => false,
        })
    }
    match &p.ship.pdf {
        Some(ship) if has_savepos(&p.b.list) => page(p, ship, &Numbers::default(), &[]).write_pos,
        _ => Vec::new(),
    }
}

/// A page body with nothing (a page shipped in DVI mode in a PDF file).
pub fn empty() -> PageBody {
    let entry = Arc::new(WalkState::default());
    PageBody {
        stream: Vec::new(),
        data: Vec::new(),
        fonts: Vec::new(),
        events: Vec::new(),
        links: Vec::new(),
        beads: Vec::new(),
        ff: Vec::new(),
        fixed: Fixed { decimal_digits: 3 },
        width: 0,
        height: 0,
        io: false,
        write_pos: Vec::new(),
        entry: entry.clone(),
        exit: entry,
        incomplete: false,
        errors: Vec::new(),
        warnings: Vec::new(),
        origins: Origins::default(),
    }
}
