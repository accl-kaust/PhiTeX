//! DVI output (tex.web §583–§642) and XeTeX's XDV variant (native glyph
//! runs, pictures), from shipped boxes to bytes.
//!
//! [`DviWriter`] is `ship_out` (§638–§640) with `hlist_out` and
//! `vlist_out` (§619–§637) over [`phitex_typeset`] nodes, writing the
//! bytes web2c's TeX writes. TeX's output buffer is not needed, but its
//! bookkeeping is: `movement` may only rewrite bytes still in the buffer
//! (§611) and `dvi_pop` only cancels a `push` in it (§601), so the
//! writer keeps `dvi_ptr`, `dvi_limit`, `dvi_offset` and `dvi_gone` as
//! counters over one growing vector, with web2c's `dvi_buf_size`.

// tex.web's octal constants are kept so that the code reads against the
// cited sections.
#![allow(clippy::unreadable_literal, clippy::many_single_char_names)]

use std::collections::BTreeMap;

use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;
use phitex_typeset::glue::{GlueSign, Order};
use phitex_typeset::node::{BoxKind, BoxNode, GlueKind, GlueNode, Node, RUNNING};

/// DVI opcodes (§586).
pub mod op {
    /// `set_char_0`.
    pub const SET_CHAR_0: u8 = 0;
    /// `set1`.
    pub const SET1: u8 = 128;
    /// `set_rule`.
    pub const SET_RULE: u8 = 132;
    /// `put_rule`.
    pub const PUT_RULE: u8 = 137;
    /// `bop`.
    pub const BOP: u8 = 139;
    /// `eop`.
    pub const EOP: u8 = 140;
    /// `push`.
    pub const PUSH: u8 = 141;
    /// `pop`.
    pub const POP: u8 = 142;
    /// `right1`.
    pub const RIGHT1: u8 = 143;
    /// `w0`.
    pub const W0: u8 = 147;
    /// `w1`.
    pub const W1: u8 = 148;
    /// `x0`.
    pub const X0: u8 = 152;
    /// `x1`.
    pub const X1: u8 = 153;
    /// `down1`.
    pub const DOWN1: u8 = 157;
    /// `y0`.
    pub const Y0: u8 = 161;
    /// `y1`.
    pub const Y1: u8 = 162;
    /// `z0`.
    pub const Z0: u8 = 166;
    /// `z1`.
    pub const Z1: u8 = 167;
    /// `fnt_num_0`.
    pub const FNT_NUM_0: u8 = 171;
    /// `fnt1`.
    pub const FNT1: u8 = 235;
    /// `xxx1`.
    pub const XXX1: u8 = 239;
    /// `xxx4`.
    pub const XXX4: u8 = 242;
    /// `fnt_def1`.
    pub const FNT_DEF1: u8 = 243;
    /// `pre`.
    pub const PRE: u8 = 247;
    /// `post`.
    pub const POST: u8 = 248;
    /// `post_post`.
    pub const POST_POST: u8 = 249;
    /// `id_byte` (§587).
    pub const ID_BYTE: u8 = 2;
}

/// web2c's default `dvi_buf_size` (texmf.cnf), a multiple of 8.
pub const DVI_BUF_SIZE: usize = 16384;

/// What the DVI file says about a font (§602), and its widths for
/// `hlist_out` (§620).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontDef {
    /// `font_check`.
    pub checksum: [u8; 4],
    /// `font_size`.
    pub size: Scaled,
    /// `font_dsize`.
    pub design_size: Scaled,
    /// `font_area`.
    pub area: Vec<u8>,
    /// `font_name`.
    pub name: Vec<u8>,
    /// `char_width` by character code; `None` (or past the end) where
    /// the character doesn't exist, which web2c's TeX doesn't output
    /// (`tex.ch`, `MLTeX` disabled).
    pub widths: Vec<Option<Scaled>>,
}

/// The direction of a [`DviWriter::movement`] (§607): `o` is `down1` or
/// `right1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// `down1`, `y`, `z`.
    Down,
    /// `right1`, `w`, `x`.
    Right,
}

impl Dir {
    fn base(self) -> u8 {
        match self {
            Dir::Down => op::DOWN1,
            Dir::Right => op::RIGHT1,
        }
    }
}

/// Why a page was not shipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShipError {
    /// "Huge page cannot be shipped out" (§641).
    HugePage,
}

// §608: the `info` settings of a movement entry
const Y_HERE: u8 = 1;
const Z_HERE: u8 = 2;
const YZ_OK: u8 = 3;
const Y_OK: u8 = 4;
const Z_OK: u8 = 5;
const D_FIXED: u8 = 6;
// §611: the search states
const NONE_SEEN: u8 = 0;
const Y_SEEN: u8 = 6;
const Z_SEEN: u8 = 12;

/// An entry of the down or right stack (§605).
#[derive(Clone, Copy, Debug)]
struct Movement {
    width: Scaled,
    location: usize,
    info: u8,
}

/// `max_dimen` (§421).
const MAX_DIMEN: Scaled = 0o7777777777;
/// `billion` (§625).
const BILLION: f64 = 1_000_000_000.0;

/// A box being output: what `hlist_out` and `vlist_out` keep of
/// `this_box` and their glue locals (§619, §629).
struct Frame {
    width: Scaled,
    height: Scaled,
    depth: Scaled,
    g_sign: GlueSign,
    g_order: Order,
    glue_set: f64,
    cur_g: Scaled,
    cur_glue: f64,
    /// `base_line` in an hlist, `top_edge` in a vlist.
    base: Scaled,
    left_edge: Scaled,
}

impl Frame {
    fn new(b: &BoxNode, base: Scaled, left_edge: Scaled) -> Frame {
        Frame {
            width: b.width,
            height: b.height,
            depth: b.depth,
            g_sign: b.glue.sign,
            g_order: b.glue.order,
            glue_set: b.glue.ratio,
            cur_g: 0,
            cur_glue: 0.0,
            base,
            left_edge,
        }
    }

    /// The glue's size as set, `width(g)-cur_g` advanced to the new
    /// `cur_g` (§625, §634).
    fn glue(&mut self, g: &GlueNode) -> Scaled {
        let spec = g.spec;
        let mut size = spec.width - self.cur_g;
        match self.g_sign {
            GlueSign::Stretching if spec.stretch_order == self.g_order => {
                self.cur_glue += f64::from(spec.stretch);
                self.cur_g = vet_round(self.glue_set * self.cur_glue);
            }
            GlueSign::Shrinking if spec.shrink_order == self.g_order => {
                self.cur_glue -= f64::from(spec.shrink);
                self.cur_g = vet_round(self.glue_set * self.cur_glue);
            }
            _ => {}
        }
        size += self.cur_g;
        size
    }
}

/// `vet_glue(float(glue_set(this_box))*cur_glue)` then
/// `cur_g:=round(glue_temp)` (§625), in doubles as web2c computes it
/// (`glueratio` and `real` are C `double`; `round` is `zround`).
#[allow(clippy::cast_possible_truncation)]
fn vet_round(glue_temp: f64) -> Scaled {
    let t = glue_temp.clamp(-BILLION, BILLION);
    // web2c's zround: (integer)(r+0.5) for r>=0, (integer)(r-0.5) else
    if t >= 0.0 {
        (t + 0.5) as Scaled
    } else {
        (t - 0.5) as Scaled
    }
}

/// The DVI writer: the state `ship_out` keeps across pages (§595, §603,
/// §605, §616, §646).
pub struct DviWriter {
    out: Vec<u8>,
    buf_size: usize,
    dvi_ptr: usize,
    dvi_limit: usize,
    dvi_offset: usize,
    dvi_gone: usize,
    mag: i32,
    /// `h_offset` (§236), the left margin of every page.
    pub h_offset: Scaled,
    /// `v_offset` (§236), the top margin of every page.
    pub v_offset: Scaled,
    total_pages: i32,
    max_v: Scaled,
    max_h: Scaled,
    max_push: i32,
    last_bop: i32,
    fonts: BTreeMap<FontId, FontDef>,
    down: Vec<Movement>,
    right: Vec<Movement>,
    dvi_h: Scaled,
    dvi_v: Scaled,
    cur_h: Scaled,
    cur_v: Scaled,
    dvi_f: Option<FontId>,
    cur_s: i32,
    doing_leaders: bool,
}

/// A whatsit's handle and whether leaders are being output; the answer
/// is a `\special`'s bytes, or `None` (§1366–§1368). `\write`,
/// `\openout` and `\closeout` are the caller's work at this moment,
/// skipped inside leaders (§1374).
pub type Whatsits<'a> = dyn FnMut(u32, bool) -> Option<Vec<u8>> + 'a;

impl DviWriter {
    /// A writer whose preamble (§617) is `pre`, `id_byte`, the ratio
    /// 25400000/473628672, `mag` and `comment` (TeX's is
    /// `" TeX output YYYY.MM.DD:HHMM"`, at most 255 bytes kept).
    #[must_use]
    pub fn new(mag: i32, comment: &str) -> DviWriter {
        DviWriter::with_buf_size(mag, comment, DVI_BUF_SIZE)
    }

    /// [`DviWriter::new`] with another `dvi_buf_size` (a multiple of 8).
    #[must_use]
    pub fn with_buf_size(mag: i32, comment: &str, buf_size: usize) -> DviWriter {
        let mut w = DviWriter {
            out: Vec::new(),
            buf_size,
            dvi_ptr: 0,
            dvi_limit: buf_size,
            dvi_offset: 0,
            dvi_gone: 0,
            mag,
            h_offset: 0,
            v_offset: 0,
            total_pages: 0,
            max_v: 0,
            max_h: 0,
            max_push: 0,
            last_bop: -1,
            fonts: BTreeMap::new(),
            down: Vec::new(),
            right: Vec::new(),
            dvi_h: 0,
            dvi_v: 0,
            cur_h: 0,
            cur_v: 0,
            dvi_f: None,
            cur_s: -1,
            doing_leaders: false,
        };
        // §617
        w.dvi_out(op::PRE);
        w.dvi_out(op::ID_BYTE);
        w.dvi_four(25400000);
        w.dvi_four(473628672);
        w.dvi_four(mag);
        let c = &comment.as_bytes()[..comment.len().min(255)];
        w.dvi_out(u8::try_from(c.len()).unwrap_or(255));
        for &b in c {
            w.dvi_out(b);
        }
        w
    }

    /// The bytes so far.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.out
    }

    /// `dvi_offset+dvi_ptr`: the location of the next byte (§601).
    #[must_use]
    pub fn location(&self) -> usize {
        self.out.len()
    }

    /// `dvi_out` (§598) with `dvi_swap`'s bookkeeping (§598): a full half
    /// of the buffer is gone and can no longer change.
    pub fn dvi_out(&mut self, b: u8) {
        self.out.push(b);
        self.dvi_ptr += 1;
        if self.dvi_ptr == self.dvi_limit {
            let half = self.buf_size / 2;
            if self.dvi_limit == self.buf_size {
                self.dvi_limit = half;
                self.dvi_offset += self.buf_size;
                self.dvi_ptr = 0;
            } else {
                self.dvi_limit = self.buf_size;
            }
            self.dvi_gone += half;
        }
    }

    /// `dvi_four` (§600): four bytes, two's complement.
    pub fn dvi_four(&mut self, x: i32) {
        for b in x.to_be_bytes() {
            self.dvi_out(b);
        }
    }

    /// `dvi_pop(l)` (§601): a `pop`, or cancel the `push` at `l-1` when
    /// nothing followed it and it is still in the buffer.
    pub fn dvi_pop(&mut self, l: usize) {
        if l == self.dvi_offset + self.dvi_ptr && self.dvi_ptr > 0 {
            self.dvi_ptr -= 1;
            self.out.pop();
        } else {
            self.dvi_out(op::POP);
        }
    }

    /// `dvi_font_def(f)` (§602; web2c's two-byte form above 256 fonts).
    fn dvi_font_def(&mut self, f: FontId) {
        let n = f - 1;
        if f <= 256 {
            self.dvi_out(op::FNT_DEF1);
            self.dvi_out(low(n));
        } else {
            self.dvi_out(op::FNT_DEF1 + 1);
            self.dvi_out(low(n / 0o400));
            self.dvi_out(low(n));
        }
        let d = self.fonts[&f].clone();
        for b in d.checksum {
            self.dvi_out(b);
        }
        self.dvi_four(d.size);
        self.dvi_four(d.design_size);
        self.dvi_out(low(u32::try_from(d.area.len()).unwrap_or(0)));
        self.dvi_out(low(u32::try_from(d.name.len()).unwrap_or(0)));
        // §603
        for &b in d.area.iter().chain(&d.name) {
            self.dvi_out(b);
        }
    }

    /// `movement(w, o)` (§607): a `down`/`right` command for `w`, or a
    /// `y0`/`z0` (`w0`/`x0`) reusing an earlier one (§608–§615).
    pub fn movement(&mut self, w: Scaled, dir: Dir) {
        let o = dir.base();
        let location = self.location();
        let gone = self.dvi_gone;
        let stack = match dir {
            Dir::Down => &mut self.down,
            Dir::Right => &mut self.right,
        };
        stack.push(Movement {
            width: w,
            location,
            info: 0,
        });
        let q = stack.len() - 1;
        // §611: look at the other entries, top down
        let mut mstate = NONE_SEEN;
        let mut found: Option<usize> = None;
        for p in (0..q).rev() {
            let e = stack[p];
            if e.width == w {
                // §612
                match mstate + e.info {
                    s if s == NONE_SEEN + YZ_OK
                        || s == NONE_SEEN + Y_OK
                        || s == Z_SEEN + YZ_OK
                        || s == Z_SEEN + Y_OK =>
                    {
                        if e.location < gone {
                            break;
                        }
                        // §613
                        self.out[e.location] += op::Y1 - op::DOWN1;
                        stack[p].info = Y_HERE;
                        found = Some(p);
                        break;
                    }
                    s if s == NONE_SEEN + Z_OK || s == Y_SEEN + YZ_OK || s == Y_SEEN + Z_OK => {
                        if e.location < gone {
                            break;
                        }
                        // §614
                        self.out[e.location] += op::Z1 - op::DOWN1;
                        stack[p].info = Z_HERE;
                        found = Some(p);
                        break;
                    }
                    s if s == NONE_SEEN + Y_HERE
                        || s == NONE_SEEN + Z_HERE
                        || s == Y_SEEN + Z_HERE
                        || s == Z_SEEN + Y_HERE =>
                    {
                        found = Some(p);
                        break;
                    }
                    _ => {}
                }
            } else {
                match mstate + e.info {
                    s if s == NONE_SEEN + Y_HERE => mstate = Y_SEEN,
                    s if s == NONE_SEEN + Z_HERE => mstate = Z_SEEN,
                    s if s == Y_SEEN + Z_HERE || s == Z_SEEN + Y_HERE => break,
                    _ => {}
                }
            }
        }
        if let Some(p) = found {
            // §609: reuse, and restrict the entries between q and p
            let info = stack[p].info;
            stack[q].info = info;
            let (code, from, to) = if info == Y_HERE {
                (o + (op::Y0 - op::DOWN1), YZ_OK, (Z_OK, Y_OK))
            } else {
                (o + (op::Z0 - op::DOWN1), YZ_OK, (Y_OK, Z_OK))
            };
            for e in &mut stack[p + 1..q] {
                if e.info == from {
                    e.info = to.0;
                } else if e.info == to.1 {
                    e.info = D_FIXED;
                }
            }
            self.dvi_out(code);
            return;
        }
        // §610
        stack[q].info = YZ_OK;
        if w.abs() >= 0o40000000 {
            self.dvi_out(o + 3);
            self.dvi_four(w);
            return;
        }
        let bytes = w.to_be_bytes();
        if w.abs() >= 0o100000 {
            self.dvi_out(o + 2);
            self.dvi_out(bytes[1]);
            self.dvi_out(bytes[2]);
        } else if w.abs() >= 0o200 {
            self.dvi_out(o + 1);
            self.dvi_out(bytes[2]);
        } else {
            self.dvi_out(o);
        }
        self.dvi_out(bytes[3]);
    }

    /// `prune_movements(l)` (§615): drop the stack entries at or past `l`.
    pub fn prune_movements(&mut self, l: usize) {
        while self.down.last().is_some_and(|e| e.location >= l) {
            self.down.pop();
        }
        while self.right.last().is_some_and(|e| e.location >= l) {
            self.right.pop();
        }
    }

    /// `push` for an opened box (§619), returning `save_loc`.
    fn enter(&mut self) -> usize {
        self.cur_s += 1;
        if self.cur_s > 0 {
            self.dvi_out(op::PUSH);
        }
        self.max_push = self.max_push.max(self.cur_s);
        self.location()
    }

    /// The end of `hlist_out`/`vlist_out` (§619, §629).
    fn leave(&mut self, save_loc: usize) {
        self.prune_movements(save_loc);
        if self.cur_s > 0 {
            self.dvi_pop(save_loc);
        }
        self.cur_s -= 1;
    }

    /// `synch_h` (§616).
    fn synch_h(&mut self) {
        if self.cur_h != self.dvi_h {
            self.movement(self.cur_h - self.dvi_h, Dir::Right);
            self.dvi_h = self.cur_h;
        }
    }

    /// `synch_v` (§616).
    fn synch_v(&mut self) {
        if self.cur_v != self.dvi_v {
            self.movement(self.cur_v - self.dvi_v, Dir::Down);
            self.dvi_v = self.cur_v;
        }
    }

    /// Outputs a box whose reference point is `(cur_h, cur_v)`.
    fn box_out(&mut self, b: &BoxNode, fonts: &dyn Fn(FontId) -> FontDef, wh: &mut Whatsits<'_>) {
        match b.kind {
            BoxKind::H => self.hlist_out(b, fonts, wh),
            BoxKind::V => self.vlist_out(b, fonts, wh),
        }
    }

    /// `special_out` (§1368) for a whatsit the caller says is a special.
    fn out_what(&mut self, h: u32, wh: &mut Whatsits<'_>) {
        // §1366
        if let Some(s) = wh(h, self.doing_leaders) {
            self.synch_h();
            self.synch_v();
            if s.len() < 256 {
                self.dvi_out(op::XXX1);
                self.dvi_out(low(u32::try_from(s.len()).unwrap_or(0)));
            } else {
                self.dvi_out(op::XXX4);
                self.dvi_four(i32::try_from(s.len()).unwrap_or(i32::MAX));
            }
            for b in s {
                self.dvi_out(b);
            }
        }
    }

    /// One character in an hlist (§620–§621).
    fn char_out(&mut self, f: FontId, c: u16, fonts: &dyn Fn(FontId) -> FontDef) {
        self.synch_h();
        self.synch_v();
        if self.dvi_f != Some(f) {
            // §621
            if let std::collections::btree_map::Entry::Vacant(e) = self.fonts.entry(f) {
                e.insert(fonts(f));
                self.dvi_font_def(f);
            }
            let n = f - 1;
            if f <= 64 {
                self.dvi_out(op::FNT_NUM_0 + low(n));
            } else if f <= 256 {
                self.dvi_out(op::FNT1);
                self.dvi_out(low(n));
            } else {
                self.dvi_out(op::FNT1 + 1);
                self.dvi_out(low(n / 0o400));
                self.dvi_out(low(n));
            }
            self.dvi_f = Some(f);
        }
        // §620 (web2c: a character that doesn't exist is not output)
        let width = self.fonts[&f].widths.get(usize::from(c)).copied().flatten();
        if let Some(w) = width {
            let c = low(u32::from(c));
            if c >= 128 {
                self.dvi_out(op::SET1);
            }
            self.dvi_out(c);
            self.cur_h += w;
        }
        self.dvi_h = self.cur_h;
    }

    /// `hlist_out` (§619).
    fn hlist_out(&mut self, b: &BoxNode, fonts: &dyn Fn(FontId) -> FontDef, wh: &mut Whatsits<'_>) {
        let save_loc = self.enter();
        let mut fr = Frame::new(b, self.cur_v, self.cur_h);
        self.hlist_nodes(&b.list, &mut fr, fonts, wh);
        self.leave(save_loc);
    }

    /// The nodes of an hlist (§620, §622); a discretionary's replacement
    /// text is part of the list (tex.web's `replace_count` nodes).
    #[allow(clippy::too_many_lines)]
    fn hlist_nodes(
        &mut self,
        list: &[Node],
        fr: &mut Frame,
        fonts: &dyn Fn(FontId) -> FontDef,
        wh: &mut Whatsits<'_>,
    ) {
        for p in list {
            let rule = match p {
                Node::Char { font, ch } => {
                    self.char_out(*font, *ch, fonts);
                    None
                }
                // §652: a ligature is output as its character
                Node::Lig(l) => {
                    self.char_out(l.font, l.ch, fonts);
                    None
                }
                Node::Box(bx) => {
                    // §623
                    if bx.list.is_empty() {
                        self.cur_h += bx.width;
                    } else {
                        let (save_h, save_v) = (self.dvi_h, self.dvi_v);
                        self.cur_v = fr.base + bx.shift;
                        let edge = self.cur_h;
                        self.box_out(bx, fonts, wh);
                        self.dvi_h = save_h;
                        self.dvi_v = save_v;
                        self.cur_h = edge + bx.width;
                        self.cur_v = fr.base;
                    }
                    None
                }
                Node::Rule(r) => Some((r.height, r.depth, r.width)),
                Node::Whatsit(h) => {
                    self.out_what(*h, wh);
                    None
                }
                Node::Glue(g) => self.hglue(g, fr, fonts, wh),
                Node::Kern(w, _) | Node::Math(_, w) => {
                    self.cur_h += w;
                    None
                }
                Node::Disc(d) => {
                    self.hlist_nodes(&d.replace, fr, fonts, wh);
                    None
                }
                Node::Unset(_)
                | Node::Ins(_)
                | Node::Mark(_)
                | Node::Adjust(_)
                | Node::Language { .. }
                | Node::Penalty(_) => None,
            };
            if let Some((mut rule_ht, mut rule_dp, rule_wd)) = rule {
                // §624
                if rule_ht == RUNNING {
                    rule_ht = fr.height;
                }
                if rule_dp == RUNNING {
                    rule_dp = fr.depth;
                }
                rule_ht += rule_dp;
                if rule_ht > 0 && rule_wd > 0 {
                    self.synch_h();
                    self.cur_v = fr.base + rule_dp;
                    self.synch_v();
                    self.dvi_out(op::SET_RULE);
                    self.dvi_four(rule_ht);
                    self.dvi_four(rule_wd);
                    self.cur_v = fr.base;
                    self.dvi_h += rule_wd;
                }
                // §622 `move_past`
                self.cur_h += rule_wd;
            }
        }
    }

    /// Glue in an hlist (§625–§628): the rule to output (`fin_rule`), or
    /// `None` once `cur_h` has moved past it.
    fn hglue(
        &mut self,
        g: &GlueNode,
        fr: &mut Frame,
        fonts: &dyn Fn(FontId) -> FontDef,
        wh: &mut Whatsits<'_>,
    ) -> Option<(Scaled, Scaled, Scaled)> {
        let mut rule_wd = fr.glue(g);
        if matches!(
            g.kind,
            GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders
        ) {
            // §626
            match g.leader.as_deref() {
                Some(Node::Rule(r)) => return Some((r.height, r.depth, rule_wd)),
                Some(Node::Box(lb)) => {
                    let leader_wd = lb.width;
                    if leader_wd > 0 && rule_wd > 0 {
                        rule_wd += 10;
                        let edge = self.cur_h + rule_wd;
                        let mut lx = 0;
                        // §627
                        if g.kind == GlueKind::ALeaders {
                            let save_h = self.cur_h;
                            self.cur_h = fr.left_edge
                                + leader_wd * ((self.cur_h - fr.left_edge) / leader_wd);
                            if self.cur_h < save_h {
                                self.cur_h += leader_wd;
                            }
                        } else {
                            let lq = rule_wd / leader_wd;
                            let lr = rule_wd % leader_wd;
                            if g.kind == GlueKind::CLeaders {
                                self.cur_h += lr / 2;
                            } else {
                                lx = lr / (lq + 1);
                                self.cur_h += (lr - (lq - 1) * lx) / 2;
                            }
                        }
                        while self.cur_h + leader_wd <= edge {
                            // §628
                            self.cur_v = fr.base + lb.shift;
                            self.synch_v();
                            let save_v = self.dvi_v;
                            self.synch_h();
                            let save_h = self.dvi_h;
                            let outer = self.doing_leaders;
                            self.doing_leaders = true;
                            self.box_out(lb, fonts, wh);
                            self.doing_leaders = outer;
                            self.dvi_v = save_v;
                            self.dvi_h = save_h;
                            self.cur_v = fr.base;
                            self.cur_h = save_h + leader_wd + lx;
                        }
                        self.cur_h = edge - 10;
                        return None;
                    }
                }
                _ => {}
            }
        }
        // §622 `move_past`
        self.cur_h += rule_wd;
        None
    }

    /// `vlist_out` (§629).
    fn vlist_out(&mut self, b: &BoxNode, fonts: &dyn Fn(FontId) -> FontDef, wh: &mut Whatsits<'_>) {
        let save_loc = self.enter();
        let left_edge = self.cur_h;
        self.cur_v -= b.height;
        let mut fr = Frame::new(b, self.cur_v, left_edge);
        for p in &b.list {
            // §630–§631
            let rule = match p {
                Node::Char { .. } | Node::Lig(_) => panic!("vlistout"),
                Node::Box(bx) => {
                    // §632
                    if bx.list.is_empty() {
                        self.cur_v += bx.height + bx.depth;
                    } else {
                        self.cur_v += bx.height;
                        self.synch_v();
                        let (save_h, save_v) = (self.dvi_h, self.dvi_v);
                        self.cur_h = left_edge + bx.shift;
                        self.box_out(bx, fonts, wh);
                        self.dvi_h = save_h;
                        self.dvi_v = save_v;
                        self.cur_v = save_v + bx.depth;
                        self.cur_h = left_edge;
                    }
                    None
                }
                Node::Rule(r) => Some((r.height, r.depth, r.width)),
                Node::Whatsit(h) => {
                    self.out_what(*h, wh);
                    None
                }
                Node::Glue(g) => self.vglue(g, &mut fr, fonts, wh),
                Node::Kern(w, _) => {
                    self.cur_v += w;
                    None
                }
                _ => None,
            };
            if let Some((rule_ht, rule_dp, mut rule_wd)) = rule {
                // §633
                if rule_wd == RUNNING {
                    rule_wd = fr.width;
                }
                let rule_ht = rule_ht + rule_dp;
                self.cur_v += rule_ht;
                if rule_ht > 0 && rule_wd > 0 {
                    self.synch_h();
                    self.synch_v();
                    self.dvi_out(op::PUT_RULE);
                    self.dvi_four(rule_ht);
                    self.dvi_four(rule_wd);
                }
            }
        }
        self.leave(save_loc);
    }

    /// Glue in a vlist (§634–§637): the rule to output (`fin_rule`), or
    /// `None` once `cur_v` has moved past it.
    fn vglue(
        &mut self,
        g: &GlueNode,
        fr: &mut Frame,
        fonts: &dyn Fn(FontId) -> FontDef,
        wh: &mut Whatsits<'_>,
    ) -> Option<(Scaled, Scaled, Scaled)> {
        let mut rule_ht = fr.glue(g);
        if matches!(
            g.kind,
            GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders
        ) {
            // §635
            match g.leader.as_deref() {
                Some(Node::Rule(r)) => return Some((rule_ht, 0, r.width)),
                Some(Node::Box(lb)) => {
                    let leader_ht = lb.height + lb.depth;
                    if leader_ht > 0 && rule_ht > 0 {
                        rule_ht += 10;
                        let edge = self.cur_v + rule_ht;
                        let mut lx = 0;
                        // §636
                        if g.kind == GlueKind::ALeaders {
                            let save_v = self.cur_v;
                            self.cur_v = fr.base + leader_ht * ((self.cur_v - fr.base) / leader_ht);
                            if self.cur_v < save_v {
                                self.cur_v += leader_ht;
                            }
                        } else {
                            let lq = rule_ht / leader_ht;
                            let lr = rule_ht % leader_ht;
                            if g.kind == GlueKind::CLeaders {
                                self.cur_v += lr / 2;
                            } else {
                                lx = lr / (lq + 1);
                                self.cur_v += (lr - (lq - 1) * lx) / 2;
                            }
                        }
                        while self.cur_v + leader_ht <= edge {
                            // §637
                            self.cur_h = fr.left_edge + lb.shift;
                            self.synch_h();
                            let save_h = self.dvi_h;
                            self.cur_v += lb.height;
                            self.synch_v();
                            let save_v = self.dvi_v;
                            let outer = self.doing_leaders;
                            self.doing_leaders = true;
                            self.box_out(lb, fonts, wh);
                            self.doing_leaders = outer;
                            self.dvi_v = save_v;
                            self.dvi_h = save_h;
                            self.cur_h = fr.left_edge;
                            self.cur_v = save_v - lb.height + leader_ht + lx;
                        }
                        self.cur_v = edge - 10;
                        return None;
                    }
                }
                _ => {}
            }
        }
        // §631 `move_past`
        self.cur_v += rule_ht;
        None
    }

    /// `ship_out(p)` (§638, §640–§641): one page with `\count0..9`.
    /// `fonts` answers each font's definition the first time it is used
    /// (font ids are TeX's internal font numbers, `null_font` being 0,
    /// so font `f` is DVI font `f-1`); `whatsits` answers specials.
    ///
    /// # Errors
    /// [`ShipError::HugePage`] for a page TeX refuses (§641); nothing is
    /// written then.
    pub fn ship_out(
        &mut self,
        page: &BoxNode,
        counts: [i32; 10],
        fonts: &dyn Fn(FontId) -> FontDef,
        whatsits: &mut Whatsits<'_>,
    ) -> Result<(), ShipError> {
        // §641
        let too_big = |a: Scaled, b: Scaled| a.checked_add(b).is_none_or(|s| s > MAX_DIMEN);
        if page.height > MAX_DIMEN
            || page.depth > MAX_DIMEN
            || page
                .height
                .checked_add(page.depth)
                .is_none_or(|hd| too_big(hd, self.v_offset))
            || too_big(page.width, self.h_offset)
        {
            return Err(ShipError::HugePage);
        }
        self.max_v = self.max_v.max(page.height + page.depth + self.v_offset);
        self.max_h = self.max_h.max(page.width + self.h_offset);
        // §617 (the preamble went out in `new`)
        self.dvi_h = 0;
        self.dvi_v = 0;
        self.cur_h = self.h_offset;
        self.dvi_f = None;
        // §640
        let page_loc = i32::try_from(self.location()).unwrap_or(i32::MAX);
        self.dvi_out(op::BOP);
        for c in counts {
            self.dvi_four(c);
        }
        self.dvi_four(self.last_bop);
        self.last_bop = page_loc;
        self.cur_v = page.height + self.v_offset;
        self.box_out(page, fonts, whatsits);
        self.dvi_out(op::EOP);
        self.total_pages += 1;
        self.cur_s = -1;
        Ok(())
    }

    /// The postamble (§642–§643); no file at all without pages.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        if self.total_pages == 0 {
            return Vec::new();
        }
        self.dvi_out(op::POST);
        self.dvi_four(self.last_bop);
        self.last_bop = i32::try_from(self.location() - 5).unwrap_or(i32::MAX);
        self.dvi_four(25400000);
        self.dvi_four(473628672);
        self.dvi_four(self.mag);
        self.dvi_four(self.max_v);
        self.dvi_four(self.max_h);
        let [_, _, a, b] = self.max_push.to_be_bytes();
        self.dvi_out(a);
        self.dvi_out(b);
        let [_, _, a, b] = self.total_pages.to_be_bytes();
        self.dvi_out(a);
        self.dvi_out(b);
        // §643: from the highest font number down
        let used: Vec<FontId> = self.fonts.keys().rev().copied().collect();
        for f in used {
            self.dvi_font_def(f);
        }
        self.dvi_out(op::POST_POST);
        self.dvi_four(self.last_bop);
        self.dvi_out(op::ID_BYTE);
        let k = 4 + (self.buf_size - self.dvi_ptr) % 4;
        for _ in 0..k {
            self.dvi_out(223);
        }
        self.out
    }
}

/// The low byte of `n`.
fn low(n: u32) -> u8 {
    n.to_be_bytes()[3]
}
