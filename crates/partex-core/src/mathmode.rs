//! Part 48: Building math lists (§1136–§1207), with the interface to the
//! engine's math typesetting (parts 34–36).

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

use partex_engine::font::Font;
use partex_engine::lr;
use partex_engine::math::{self, Delim, Env, Event, Field, Item, Kind, Limits, Noad};
use partex_engine::node::{BoxNode, FontId, GlueSign, GlueSpec, Node};
use partex_engine::pack::{Fonts, Spec};

use crate::arith::{Scaled, half};
use crate::build::norm_min;
use crate::cmds::*;
use crate::fonts::font_id;
use crate::host::Host;
use crate::nest::IGNORE_DEPTH;
use crate::nodes::*;
use crate::pack::spec;
use crate::scan::MAX_DIMEN;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// §1178: `chr_code`s of \above and friends.
const ABOVE_CODE: i32 = 0;
const OVER_CODE: i32 = 1;
const ATOP_CODE: i32 = 2;
const DELIMITED_CODE: i32 = 3;
/// §700, §701
const TOTAL_MATHSY_PARAMS: usize = 22;
const TOTAL_MATHEX_PARAMS: usize = 13;
/// §232: `var_code`.
const VAR_CODE: i32 = 0o70000;

/// The fields of a noad that `scan_math` fills (tex.web's pointers
/// `nucleus(p)`, `supscr(p)`, `subscr(p)` into the tail noad).
pub(crate) const NUCLEUS: i32 = 0;
pub(crate) const SUPSCR: i32 = 1;
pub(crate) const SUBSCR: i32 = 2;

/// §682–§687: the noad of type `t` (`\mathord`…`\overline` codes).
pub(crate) fn noad_kind(t: i32) -> Kind {
    match t {
        OP_NOAD => Kind::Op(Limits::Normal),
        BIN_NOAD => Kind::Bin,
        REL_NOAD => Kind::Rel,
        OPEN_NOAD => Kind::Open,
        CLOSE_NOAD => Kind::Close,
        PUNCT_NOAD => Kind::Punct,
        INNER_NOAD => Kind::Inner,
        UNDER_NOAD => Kind::Under,
        OVER_NOAD => Kind::Over,
        VCENTER_NOAD => Kind::Vcenter,
        _ => Kind::Ord,
    }
}

/// Field `f` of noad `n`.
fn field(n: &mut Noad, f: i32) -> &mut Field {
    match f {
        SUPSCR => &mut n.sup,
        SUBSCR => &mut n.sub,
        _ => &mut n.nucleus,
    }
}

/// §1147 `found:`: a visible node of width `d`; false if the line is
/// affected by stretching or shrinking (`w:=max_dimen`).
fn found_width(v: &mut Scaled, w: &mut Scaled, d: Scaled) -> bool {
    if *v < MAX_DIMEN {
        *v += d;
        *w = *v;
        true
    } else {
        false
    }
}

/// A node of the last line before a display, as measured in visual
/// order (e-TeX: reflected segments end in an edge that restores the
/// direction and carries the width of the closing math node).
#[derive(Clone)]
enum Seen {
    Node(Node),
    Edge { width: Scaled, rtl: bool },
}

/// e-TeX's LR stack while measuring the line before a display.
#[derive(Default)]
struct LrScan {
    /// Closing subtypes of the open segments.
    open: Vec<u8>,
    problems: u32,
}

impl LrScan {
    /// e-TeX's `just_reverse`: `rest` follows a node that opened a
    /// segment of the other direction; reverse it up to the node closing
    /// that segment, which becomes an edge restoring the direction.
    fn just_reverse(&mut self, rest: Vec<Seen>, rtl: &mut bool) -> Vec<Seen> {
        let mut edge = (0, *rtl);
        *rtl = !*rtl;
        let (mut m, mut n) = (0u32, 0u32);
        let mut reversed: Vec<Seen> = Vec::with_capacity(rest.len() + 1);
        let mut rest = rest.into_iter();
        for q in rest.by_ref() {
            let q = match q {
                Seen::Node(Node::Math { width, subtype, .. }) => {
                    let kern = Seen::Node(Node::Kern {
                        width,
                        subtype: 0,
                        sync: partex_engine::origin::Side(0),
                    });
                    if lr::is_end(subtype) {
                        if self.open.last() == Some(&lr::end_of(subtype)) {
                            self.open.pop();
                            if n > 0 {
                                n -= 1;
                                Seen::Node(Node::Math {
                                    width,
                                    subtype: subtype - 1,
                                    sync: partex_engine::origin::Side(0),
                                })
                            } else if m > 0 {
                                m -= 1;
                                kern
                            } else {
                                edge.0 = width;
                                break;
                            }
                        } else {
                            self.problems += 1;
                            kern
                        }
                    } else {
                        self.open.push(lr::end_of(subtype));
                        if n > 0 || lr::is_rtl(subtype) != *rtl {
                            n += 1;
                            Seen::Node(Node::Math {
                                width,
                                subtype: subtype + 1,
                                sync: partex_engine::origin::Side(0),
                            })
                        } else {
                            m += 1;
                            kern
                        }
                    }
                }
                // (a run of glyphs is measured the same either way)
                q => q,
            };
            reversed.push(q);
        }
        reversed.reverse();
        reversed.push(Seen::Edge {
            width: edge.0,
            rtl: edge.1,
        });
        reversed.extend(rest);
        reversed
    }
}

/// The math fonts, as the engine's math typesetting sees them, and its
/// packs, which are `hpack` and `vpack` calls (DESIGN 7.17.2), holding on
/// to what a pack's confusion raised.
struct MathEnv<'a, H: Host, T: Tracker> {
    t: &'a mut Tex<H, T>,
    /// `fam_fnt(0..48)`
    fams: [i32; 48],
    jump: Option<Jump>,
}

impl<H: Host, T: Tracker> Fonts for MathEnv<'_, H, T> {
    fn font(&self, f: FontId) -> &Font {
        self.t
            .font_read(i32::from(f.0), crate::track::font::METRICS);
        self.t.fonts.font(f)
    }
}

impl<H: Host, T: Tracker> Env for MathEnv<'_, H, T> {
    fn fam_fnt(&self, n: usize) -> Option<FontId> {
        let f = self.fams[n];
        (f != NULL_FONT).then(|| font_id(f))
    }
    fn param(&self, f: FontId, k: usize) -> Scaled {
        self.t.font_read(i32::from(f.0), crate::track::font::PARAMS);
        self.t.fonts.font(f).param(k)
    }
    fn skew_char(&self, f: FontId) -> i32 {
        self.t
            .font_read(i32::from(f.0), crate::track::font::SKEW_CHAR);
        self.t.fonts.skew_char[usize::from(f.0)]
    }
    fn hpack(&mut self, list: Vec<Node>, spec: Spec) -> BoxNode {
        self.t.hpack(list, spec, None)
    }
    fn vpack(&mut self, list: Vec<Node>) -> Result<BoxNode, partex_engine::pack::Confusion> {
        match self.t.vpack(list, Spec::NATURAL) {
            Ok(b) => Ok(b),
            Err(j) => {
                self.jump = Some(j);
                Err(partex_engine::pack::Confusion("vpack"))
            }
        }
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §1151: `fam_in_range`.
    fn fam_in_range(&self) -> bool {
        (0..16).contains(&self.int_par(CUR_FAM_CODE))
    }

    /// §1136
    fn push_math(&mut self, c: i32) -> Result<(), Jump> {
        self.push_nest()?;
        self.set_mode(-MMODE);
        *self.incompleat_mut() = None;
        self.new_save_level(c)
    }

    /// Append noad `n` to the current mlist.
    pub(crate) fn push_noad(&mut self, n: Noad) {
        self.mlist_mut().push(Item::Noad(Box::new(n)));
    }

    /// The noad at the end of the current mlist.
    fn tail_noad(&mut self) -> Option<&mut Noad> {
        match self.mlist_mut().last_mut() {
            Some(Item::Noad(n)) => Some(n),
            _ => None,
        }
    }

    /// §719–§767: typeset `mlist` in `style` (`mlist_penalties` is
    /// `penalties`), reporting what tex.web reports on the way.
    fn mlist_to_hlist(
        &mut self,
        mlist: Vec<Item>,
        style: i32,
        penalties: bool,
    ) -> Result<Vec<Node>, Jump> {
        let params = math::Params {
            script_space: self.dimen_par(SCRIPT_SPACE_CODE),
            null_delimiter_space: self.dimen_par(NULL_DELIMITER_SPACE_CODE),
            delimiter_factor: self.int_par(DELIMITER_FACTOR_CODE),
            delimiter_shortfall: self.dimen_par(DELIMITER_SHORTFALL_CODE),
            bin_op_penalty: self.int_par(BIN_OP_PENALTY_CODE),
            rel_penalty: self.int_par(REL_PENALTY_CODE),
            thin_mu_skip: self.glue_par(THIN_MU_SKIP_CODE),
            med_mu_skip: self.glue_par(MED_MU_SKIP_CODE),
            thick_mu_skip: self.glue_par(THICK_MU_SKIP_CODE),
        };
        let mut fams = [NULL_FONT; 48];
        for (n, f) in fams.iter_mut().enumerate() {
            *f = self.fam_fnt(i32::try_from(n).unwrap_or(0));
        }
        // (math reads the parameters and skew characters of these fonts)
        for &f in &fams {
            if f != NULL_FONT {
                self.tracker.read(crate::track::Cell::Font(f));
            }
        }
        let mut env = MathEnv {
            t: self,
            fams,
            jump: None,
        };
        let style = u8::try_from(style).unwrap_or(0);
        let r = math::mlist_to_hlist(mlist, style, penalties, &params, &mut env);
        if let Some(j) = env.jump {
            // (a pack's confusion, printed where it happened)
            return Err(j);
        }
        let (list, events) = match r {
            Ok(r) => r,
            Err(c) => return self.confusion(c.0.as_bytes()),
        };
        for e in events {
            match e {
                Event::UndefinedFamily { size, fam, ch } => {
                    // §723: complain about an undefined family.
                    self.print_err(b"");
                    self.print_size(i32::from(size));
                    self.print_char(b' ');
                    self.print_int(i32::from(fam));
                    self.print_str(b" is undefined (character ");
                    self.print(i32::from(ch)); // `print_ASCII`
                    self.print_char(b')');
                    self.help(&[
                        b"Somewhere in the math formula just ended, you used the",
                        b"stated character from an undefined font family. For example,",
                        b"plain TeX doesn't allow \\it or \\sl in subscripts. Proceed,",
                        b"and I'll try to forget that I needed that character.",
                    ]);
                    self.error()?;
                }
                Event::MissingChar { font, ch } => {
                    self.char_warning(i32::from(font.0), i32::from(ch))?;
                }
            }
        }
        Ok(list)
    }

    /// §1142
    pub(crate) fn start_eq_no(&mut self) -> Result<(), Jump> {
        self.set_saved(0, self.cur_chr);
        self.set_save_ptr(self.save_ptr() + 1);
        self.go_into_ordinary_math()
    }

    /// §1139: go into ordinary math mode.
    fn go_into_ordinary_math(&mut self) -> Result<(), Jump> {
        self.push_math(MATH_SHIFT_GROUP)?;
        self.eq_word_define(INT_BASE + CUR_FAM_CODE, -1)?;
        self.begin_toks_at(EVERY_MATH_LOC, EVERY_MATH_TEXT)?;
        Ok(())
    }

    /// §1138
    pub(crate) fn init_math(&mut self) -> Result<(), Jump> {
        self.get_token()?; // `get_x_token` would fail on \ifmmode!
        if self.cur_cmd == MATH_SHIFT && self.mode() > 0 {
            self.go_into_display_math()
        } else {
            self.back_input()?;
            self.go_into_ordinary_math()
        }
    }

    /// §1145: go into display math mode.
    fn go_into_display_math(&mut self) -> Result<(), Jump> {
        // e-TeX: the text direction before the display.
        let direction = |t: &Self| match t.lr_save().last() {
            None => 0,
            Some(&e) if e >= lr::R_CODE => -1,
            Some(_) => 1,
        };
        let mut j = None; // e-TeX's prototype box
        let (w, x) = if self.list_is_empty() {
            // `\noindent$$' or `$${ }$$'
            self.pop_nest();
            (-MAX_DIMEN, direction(self))
        } else {
            let just_box = self.line_break(true)?;
            if self.etex_ex() {
                j = Some(self.prototype_box(&just_box));
            }
            let x = direction(self);
            (self.natural_width_of_last_line(&just_box, x), x)
        };
        // now we are in vertical mode, working on the list that will
        // contain the display
        // §1149: calculate the length, `l`, and the shift amount, `s`, of
        // the display lines.
        let (l, s);
        let pg = self.pg();
        if let Some(ps) = self.par_shape() {
            let n = ps.len();
            let i = usize::try_from(pg + 2).unwrap_or(usize::MAX).min(n);
            (s, l) = ps[i - 1];
        } else {
            let hang_indent = self.dimen_par(HANG_INDENT_CODE);
            let hang_after = self.int_par(HANG_AFTER_CODE);
            if hang_indent != 0
                && ((hang_after >= 0 && pg + 2 > hang_after) || pg + 1 < -hang_after)
            {
                l = self.dimen_par(HSIZE_CODE) - hang_indent.abs();
                s = if hang_indent > 0 { hang_indent } else { 0 };
            } else {
                l = self.dimen_par(HSIZE_CODE);
                s = 0;
            }
        }
        self.push_math(MATH_SHIFT_GROUP)?;
        self.set_mode(MMODE);
        self.eq_word_define(INT_BASE + CUR_FAM_CODE, -1)?;
        self.eq_word_define(DIMEN_BASE + PRE_DISPLAY_SIZE_CODE, w)?;
        *self.lr_box_mut() = j;
        if self.etex_ex() {
            self.eq_word_define(INT_BASE + PRE_DISPLAY_DIRECTION_CODE, x)?;
        }
        self.eq_word_define(DIMEN_BASE + DISPLAY_WIDTH_CODE, l)?;
        self.eq_word_define(DIMEN_BASE + DISPLAY_INDENT_CODE, s)?;
        self.begin_toks_at(EVERY_DISPLAY_LOC, EVERY_DISPLAY_TEXT)?;
        if self.nest_ptr() == 1 {
            self.build_page()?;
        }
        Ok(())
    }

    /// pdfTeX marks display boxes `dlist` (never reversed).
    fn dlist(&self) -> u8 {
        if self.params.flavor == crate::params::Flavor::PdfTex {
            lr::DLIST
        } else {
            0
        }
    }

    /// e-TeX: the prototype box of a display: the width, glue setting
    /// and shift of the line before it, holding its \leftskip and
    /// \rightskip (kerns for zero glue).
    fn prototype_box(&self, jb: &BoxNode) -> BoxNode {
        let skip = |t: &Self, n: i32| {
            if t.glue_par(n).shared_zero {
                new_kern(0)
            } else {
                t.new_param_glue(n)
            }
        };
        BoxNode {
            width: jb.width,
            shift: jb.shift,
            glue_order: jb.glue_order,
            glue_sign: jb.glue_sign,
            glue_set: jb.glue_set,
            list: alloc::vec![skip(self, LEFT_SKIP_CODE), skip(self, RIGHT_SKIP_CODE)],
            ..BoxNode::default()
        }
    }

    /// e-TeX's `app_display`: append display line `b` (a display, an
    /// equation number, or both), displaced by `d`, laid out for the text
    /// direction before the display and prototype box `j`.
    fn app_display(
        &mut self,
        j: Option<&BoxNode>,
        mut b: BoxNode,
        mut d: Scaled,
    ) -> Result<(), Jump> {
        let mut s = self.dimen_par(DISPLAY_INDENT_CODE);
        let x = self.int_par(PRE_DISPLAY_DIRECTION_CODE);
        if x == 0 {
            b.shift = s + d;
            self.append_to_vlist(Node::Box(b.share()));
            return Ok(());
        }
        let z = self.dimen_par(DISPLAY_WIDTH_CODE);
        let p = b;
        // Set up the hlist for the display line.
        let mut e;
        if x > 0 {
            e = z - d - p.width;
        } else {
            e = d;
            d = z - e - p.width;
        }
        let mut outer = None;
        if let Some(j) = j {
            let mut b = j.clone();
            b.height = p.height;
            b.depth = p.depth;
            s -= b.shift;
            d += s;
            e = e + b.width - z - s;
            outer = Some(b);
        }
        let inner = if i32::from(p.subtype) == DLIST {
            alloc::vec![Node::Box(p.share())] // display or equation number
        } else {
            // display and equation number
            let mut r = p.list;
            if r.is_empty() {
                return self.confusion(b"LR4");
            }
            if x < 0 {
                r.reverse();
            }
            r
        };
        // Package the display line: kerns (or glue cancelling \leftskip and
        // \rightskip) around the hlist, between \beginM and \endM.
        let cancel = |g: &GlueSpec, n: i32, amount: Scaled| Node::Glue {
            spec: GlueSpec {
                width: amount - g.width,
                stretch: -g.stretch,
                shrink: -g.shrink,
                shared_zero: false,
                ..*g
            },
            subtype: u8::try_from(n + 1).unwrap_or(0),
            sync: partex_engine::origin::Side(0),
        };
        let (r, t) = match &outer {
            None => (new_kern(0), new_kern(0)),
            Some(b) => (b.list[0].clone(), b.list[1].clone()),
        };
        let begin = new_math(0, lr::BEGIN_M_CODE.into());
        let end = new_math(0, lr::END_M_CODE.into());
        let mut list = Vec::with_capacity(inner.len() + 6);
        match &r {
            Node::Glue { spec, .. } => {
                list.extend([r.clone(), begin, cancel(spec, LEFT_SKIP_CODE, d)]);
            }
            _ => list.extend([begin, new_kern(d)]),
        }
        list.extend(inner);
        match &t {
            Node::Glue { spec, .. } => {
                list.extend([cancel(spec, RIGHT_SKIP_CODE, e), end, t.clone()]);
            }
            _ => list.extend([new_kern(e), end]),
        }
        let b = if let Some(mut b) = outer {
            b.list = list;
            b
        } else {
            let mut b = self.hpack(list, Spec::NATURAL, None);
            b.shift = s;
            b
        };
        self.append_to_vlist(Node::Box(b.share()));
        Ok(())
    }

    /// §1148: the width of glue `g` in line `jb`; `v:=max_dimen` if it
    /// stretches or shrinks there.
    fn display_glue_width(jb: &BoxNode, g: &GlueSpec, v: &mut Scaled) -> Scaled {
        if jb.glue_sign == GlueSign::Stretching {
            if jb.glue_order == g.stretch_order && g.stretch != 0 {
                *v = MAX_DIMEN;
            }
        } else if jb.glue_sign == GlueSign::Shrinking
            && jb.glue_order == g.shrink_order
            && g.shrink != 0
        {
            *v = MAX_DIMEN;
        }
        g.width
    }

    /// §1146: calculate the natural width, `w`, by which the characters of
    /// the final line extend to the right of the reference point, plus two
    /// ems; or `max_dimen` if the non-blank information on that line is
    /// affected by stretching or shrinking. `x`: e-TeX's text direction
    /// before the display (-1: right-to-left).
    fn natural_width_of_last_line(&mut self, jb: &BoxNode, x: i32) -> Scaled {
        let quad = self.font_param(QUAD_CODE, self.cur_font());
        let pdftex = self.params.flavor == crate::params::Flavor::PdfTex;
        let texxet = self.texxet_en();
        // tex.web has a discretionary's replaced nodes after it.
        let mut nodes: VecDeque<Seen> = VecDeque::with_capacity(jb.list.len() + 2);
        for n in &jb.list {
            nodes.push_back(Seen::Node(n.clone()));
            if let Node::Disc(d) = n {
                nodes.extend(d.replace.iter().cloned().map(Seen::Node));
            }
        }
        // e-TeX: a line ending in right-to-left text is measured
        // reflected about the left edge.
        let mut rtl = false;
        let mut v = if x >= 0 {
            jb.shift
        } else {
            nodes.push_front(Seen::Node(new_math(0, BEGIN_L_CODE)));
            nodes.push_back(Seen::Node(new_math(0, END_L_CODE)));
            rtl = true;
            -jb.shift - jb.width
        } + 2 * quad;
        let mut w = -MAX_DIMEN;
        let mut lr = LrScan::default();
        while let Some(p) = nodes.pop_front() {
            // §1147: let `d` be the natural width of node `p`; if the node
            // is "visible," `goto found`; if the node is glue that
            // stretches or shrinks, set `v:=max_dimen`.
            let (d, found) = match &p {
                Seen::Edge { width, rtl: r } => {
                    rtl = *r;
                    (*width, false)
                }
                Seen::Node(Node::Glyphs(g)) => {
                    let f = i32::from(g.font.0);
                    for &c in g.chars() {
                        let d = self.char_metrics(f, i32::from(c)).width;
                        if !found_width(&mut v, &mut w, d) {
                            return MAX_DIMEN;
                        }
                    }
                    continue;
                }
                Seen::Node(Node::Ligature(l)) => {
                    let d = self
                        .char_metrics(i32::from(l.font.0), i32::from(l.ch))
                        .width;
                    (d, true)
                }
                Seen::Node(Node::Box(b)) => (b.width, true),
                Seen::Node(Node::Rule { width, .. }) => (*width, true),
                Seen::Node(Node::Math { width, subtype, .. }) if pdftex => {
                    let s = *subtype;
                    if texxet {
                        if lr::is_end(s) {
                            if lr.open.last() == Some(&lr::end_of(s)) {
                                lr.open.pop();
                            } else if s > lr::L_CODE {
                                return MAX_DIMEN;
                            }
                        } else {
                            lr.open.push(lr::end_of(s));
                            if lr::is_rtl(s) != rtl {
                                let rest: Vec<Seen> = nodes.drain(..).collect();
                                nodes = lr.just_reverse(rest, &mut rtl).into();
                            }
                        }
                    } else if s >= lr::L_CODE {
                        return MAX_DIMEN;
                    }
                    (*width, false)
                }
                Seen::Node(
                    Node::Kern { width, .. }
                    | Node::Math { width, .. }
                    | Node::MarginKern { width, .. },
                ) => (*width, false),
                Seen::Node(Node::Glue { spec, .. }) => {
                    (Self::display_glue_width(jb, spec, &mut v), false)
                }
                Seen::Node(Node::Leaders(l)) => {
                    (Self::display_glue_width(jb, &l.spec, &mut v), true)
                }
                // `whatsit_node`: pdfTeX counts forms and images
                Seen::Node(Node::Whatsit(wh)) => (wh.ref_dims().map_or(0, |r| r.width), false),
                Seen::Node(_) => (0, false),
            };
            if found {
                if !found_width(&mut v, &mut w, d) {
                    return MAX_DIMEN;
                }
            } else if v < MAX_DIMEN {
                v += d;
            }
        }
        if texxet && lr.problems != 0 {
            return MAX_DIMEN;
        }
        w
    }

    /// §1152: treat `cur_chr` as an active character.
    fn treat_as_active(&mut self) -> Result<(), Jump> {
        self.cur_cs = self.cur_chr + ACTIVE_BASE;
        self.cur_cmd = self.eq_type(self.cur_cs);
        self.cur_chr = self.equiv(self.cur_cs);
        self.x_token()?;
        self.back_input()
    }

    /// §1151: fill field `p` of the tail noad (or scan a subformula for
    /// it).
    pub(crate) fn scan_math(&mut self, p: i32) -> Result<(), Jump> {
        let c;
        'restart: loop {
            self.get_nonblank_nonrelax_noncall()?;
            loop {
                // reswitch:
                match self.cur_cmd {
                    LETTER | OTHER_CHAR | CHAR_GIVEN => {
                        let m = self.math_code(self.cur_chr);
                        if m == 0o100000 {
                            self.treat_as_active()?;
                            continue 'restart;
                        }
                        c = m;
                    }
                    CHAR_NUM => {
                        self.scan_char_num()?;
                        self.cur_chr = self.cur_val;
                        self.cur_cmd = CHAR_GIVEN;
                        continue;
                    }
                    MATH_CHAR_NUM => {
                        self.scan_fifteen_bit_int()?;
                        c = self.cur_val;
                    }
                    MATH_GIVEN => c = self.cur_chr,
                    DELIM_NUM => {
                        self.scan_twenty_seven_bit_int()?;
                        c = self.cur_val / 0o10000;
                    }
                    _ => {
                        // §1153: scan a subformula enclosed in braces and
                        // `return`.
                        self.back_input()?;
                        self.scan_left_brace()?;
                        self.set_saved(0, p);
                        self.set_save_ptr(self.save_ptr() + 1);
                        return self.push_math(MATH_GROUP);
                    }
                }
                break 'restart;
            }
        }
        let fam = if c >= VAR_CODE && self.fam_in_range() {
            self.int_par(CUR_FAM_CODE)
        } else {
            (c / 256) % 16
        };
        let f = Field::Char {
            fam: u8::try_from(fam).unwrap_or(0),
            ch: u8::try_from(c % 256).unwrap_or(0),
        };
        if let Some(n) = self.tail_noad() {
            *field(n, p) = f;
        }
        Ok(())
    }

    /// §1155
    pub(crate) fn set_math_char(&mut self, c: i32) -> Result<(), Jump> {
        if c >= 0o100000 {
            return self.treat_as_active();
        }
        let mut fam = (c / 256) % 16;
        let kind = if c >= VAR_CODE {
            if self.fam_in_range() {
                fam = self.int_par(CUR_FAM_CODE);
            }
            Kind::Ord
        } else {
            noad_kind(ORD_NOAD + c / 0o10000)
        };
        let mut n = Noad::new(kind);
        n.nucleus = Field::Char {
            fam: u8::try_from(fam).unwrap_or(0),
            ch: u8::try_from(c % 256).unwrap_or(0),
        };
        self.push_noad(n);
        Ok(())
    }

    /// §1159
    pub(crate) fn math_limit_switch(&mut self) -> Result<(), Jump> {
        let limits = match self.cur_chr {
            LIMITS => Limits::Limits,
            NO_LIMITS => Limits::NoLimits,
            _ => Limits::Normal,
        };
        if let Some(Noad {
            kind: Kind::Op(l), ..
        }) = self.tail_noad()
        {
            *l = limits;
            return Ok(());
        }
        self.print_err(b"Limit controls must follow a math operator");
        self.help(&[b"I'm ignoring this misplaced \\limits or \\nolimits command."]);
        self.error()
    }

    /// §1160: scan a delimiter (a 27-bit code if `r`).
    fn scan_delimiter(&mut self, r: bool) -> Result<Delim, Jump> {
        if r {
            self.scan_twenty_seven_bit_int()?;
        } else {
            self.get_nonblank_nonrelax_noncall()?;
            match self.cur_cmd {
                LETTER | OTHER_CHAR => self.cur_val = self.del_code(self.cur_chr),
                DELIM_NUM => self.scan_twenty_seven_bit_int()?,
                _ => self.cur_val = -1,
            }
        }
        if self.cur_val < 0 {
            // §1161: report that an invalid delimiter code is being changed
            // to null; set `cur_val:=0`.
            self.print_err(b"Missing delimiter (. inserted)");
            self.help(&[
                b"I was expecting to see something like `(' or `\\{' or",
                b"`\\}' here. If you typed, e.g., `{' instead of `\\{', you",
                b"should probably delete the `{' by typing `1' now, so that",
                b"braces don't get unbalanced. Otherwise just proceed.",
                b"Acceptable delimiters are characters whose \\delcode is",
                b"nonnegative, or you can use `\\delimiter <delimiter code>'.",
            ]);
            self.back_error()?;
            self.cur_val = 0;
        }
        let v = self.cur_val;
        let byte = |x: i32| u8::try_from(x).unwrap_or(0);
        Ok(Delim {
            small_fam: byte((v / 0o4000000) % 16),
            small_char: byte((v / 0o10000) % 256),
            large_fam: byte((v / 256) % 16),
            large_char: byte(v % 256),
        })
    }

    /// §1163
    pub(crate) fn math_radical(&mut self) -> Result<(), Jump> {
        // The noad is the tail while the delimiter is scanned.
        self.push_noad(Noad::new(Kind::Radical(Delim::default())));
        let d = self.scan_delimiter(true)?;
        if let Some(n) = self.tail_noad() {
            n.kind = Kind::Radical(d);
        }
        self.scan_math(NUCLEUS)
    }

    /// §1165
    pub(crate) fn math_ac(&mut self) -> Result<(), Jump> {
        if self.cur_cmd == ACCENT {
            // §1166: complain that the user should have said \mathaccent.
            self.print_err(b"Please use ");
            self.print_esc(b"mathaccent");
            self.print_str(b" for accents in math mode");
            self.help(&[
                b"I'm changing \\accent to \\mathaccent here; wish me luck.",
                b"(Accents are not the same in formulas as they are in text.)",
            ]);
            self.error()?;
        }
        self.push_noad(Noad::new(Kind::Accent { fam: 0, ch: 0 }));
        self.scan_fifteen_bit_int()?;
        let ch = u8::try_from(self.cur_val % 256).unwrap_or(0);
        let fam = if self.cur_val >= VAR_CODE && self.fam_in_range() {
            self.int_par(CUR_FAM_CODE)
        } else {
            (self.cur_val / 256) % 16
        };
        let fam = u8::try_from(fam).unwrap_or(0);
        if let Some(n) = self.tail_noad() {
            n.kind = Kind::Accent { fam, ch };
        }
        self.scan_math(NUCLEUS)
    }

    /// §1167
    pub(crate) fn begin_vcenter(&mut self) -> Result<(), Jump> {
        self.scan_spec(VCENTER_GROUP, false)?;
        self.normal_paragraph()?;
        self.push_nest()?;
        self.set_mode(-VMODE);
        self.set_prev_depth(IGNORE_DEPTH);
        self.begin_toks_at(EVERY_VBOX_LOC, EVERY_VBOX_TEXT)?;
        Ok(())
    }

    /// §1168: `vcenter_group` in `handle_right_brace`.
    pub(crate) fn finish_vcenter(&mut self) -> Result<(), Jump> {
        self.end_graf()?;
        self.unsave()?;
        self.set_save_ptr(self.save_ptr() - 2);
        let list = core::mem::take(self.nodes_mut()).into_vec();
        let p = self.vpack(list, spec(self.saved(0), self.saved(1)))?;
        self.pop_nest();
        let mut n = Noad::new(Kind::Vcenter);
        n.nucleus = Field::Box(p.share());
        self.push_noad(n);
        Ok(())
    }

    /// §1172
    pub(crate) fn append_choices(&mut self) -> Result<(), Jump> {
        self.mlist_mut().push(Item::Choice(Box::default()));
        self.set_save_ptr(self.save_ptr() + 1);
        self.set_saved(-1, 0);
        self.push_math(MATH_CHOICE_GROUP)?;
        self.scan_left_brace()
    }

    /// §1174
    pub(crate) fn build_choices(&mut self) -> Result<(), Jump> {
        self.unsave()?;
        let p = self.fin_mlist(None)?;
        let k = self.saved(-1);
        if let Some(Item::Choice(c)) = self.mlist_mut().last_mut() {
            c[usize::try_from(k).unwrap_or(0).min(3)] = p;
        }
        if k == 3 {
            self.set_save_ptr(self.save_ptr() - 1);
            return Ok(());
        }
        self.set_saved(-1, k + 1);
        self.push_math(MATH_CHOICE_GROUP)?;
        self.scan_left_brace()
    }

    /// §1176
    pub(crate) fn sub_sup(&mut self) -> Result<(), Jump> {
        let p = SUPSCR + self.cur_cmd - SUP_MARK; // `supscr` or `subscr`
        let mut t = None; // the field's current value: `None` if not allowed
        if let Some(n) = self.tail_noad()
            && !matches!(n.kind, Kind::Left(_) | Kind::Right(_) | Kind::Middle(_))
        {
            // `scripts_allowed`
            t = Some(*field(n, p) != Field::Empty);
        }
        if t != Some(false) {
            // §1177: insert a dummy noad to be sub/superscripted.
            self.push_noad(Noad::new(Kind::Ord));
            if t == Some(true) {
                if self.cur_cmd == SUP_MARK {
                    self.print_err(b"Double superscript");
                    self.help(&[b"I treat `x^1^2' essentially like `x^1{}^2'."]);
                } else {
                    self.print_err(b"Double subscript");
                    self.help(&[b"I treat `x_1_2' essentially like `x_1{}_2'."]);
                }
                self.error()?;
            }
        }
        self.scan_math(p)
    }

    /// §1181
    pub(crate) fn math_fraction(&mut self) -> Result<(), Jump> {
        let c = self.cur_chr;
        if self.incompleat().is_some() {
            // §1183: ignore the fraction operation and complain about this
            // ambiguous case.
            if c >= DELIMITED_CODE {
                self.scan_delimiter(false)?;
                self.scan_delimiter(false)?;
            }
            if c % DELIMITED_CODE == ABOVE_CODE {
                self.scan_normal_dimen()?;
            }
            self.print_err(b"Ambiguous; you need another { and }");
            self.help(&[
                b"I'm ignoring this fraction specification, since I don't",
                b"know whether a construction like `x \\over y \\over z'",
                b"means `{x \\over y} \\over z' or `x \\over {y \\over z}'.",
            ]);
            return self.error();
        }
        let numerator = core::mem::take(self.mlist_mut());
        let mut n = Noad::new(Kind::Fraction {
            thickness: 0,
            left: Delim::default(),
            right: Delim::default(),
        });
        n.sup = Field::Mlist(numerator);
        *self.incompleat_mut() = Some(Box::new(n));
        // §1182: use code `c` to distinguish between generalized fractions.
        let (mut left, mut right) = (Delim::default(), Delim::default());
        if c >= DELIMITED_CODE {
            left = self.scan_delimiter(false)?;
            right = self.scan_delimiter(false)?;
        }
        let thickness = match c % DELIMITED_CODE {
            ABOVE_CODE => {
                self.scan_normal_dimen()?;
                self.cur_val
            }
            OVER_CODE => math::DEFAULT_CODE,
            _ => 0, // `atop_code`
        };
        if let Some(n) = self.incompleat_mut().as_mut() {
            n.kind = Kind::Fraction {
                thickness,
                left,
                right,
            };
        }
        Ok(())
    }

    /// §1184: finish the current mlist (with `p`, a right delimiter, at
    /// its end) and leave its level.
    pub(crate) fn fin_mlist(&mut self, p: Option<Noad>) -> Result<Vec<Item>, Jump> {
        let mut list = core::mem::take(self.mlist_mut());
        let q = match self.incompleat_mut().take() {
            None => {
                if let Some(p) = p {
                    list.push(Item::Noad(Box::new(p)));
                }
                list
            }
            Some(mut n) => {
                // §1185: compleat the incompleat noad.
                n.sub = Field::Mlist(list);
                match p {
                    None => alloc::vec![Item::Noad(n)],
                    Some(p) => {
                        let Field::Mlist(num) = &mut n.sup else {
                            return self.confusion(b"right");
                        };
                        // The numerator starts after the last `\left` or
                        // `\middle` (`delim_ptr`).
                        let Some(k) = num.iter().rposition(|q| {
                            matches!(q, Item::Noad(q) if matches!(q.kind, Kind::Left(_) | Kind::Middle(_)))
                        }) else {
                            return self.confusion(b"right");
                        };
                        let mut q: Vec<Item> = num.drain(..=k).collect();
                        q.push(Item::Noad(n));
                        q.push(Item::Noad(Box::new(p)));
                        q
                    }
                }
            }
        };
        self.pop_nest();
        Ok(q)
    }

    /// §1186: `math_group` in `handle_right_brace`.
    pub(crate) fn finish_math_group(&mut self) -> Result<(), Jump> {
        self.unsave()?;
        self.set_save_ptr(self.save_ptr() - 1);
        let s = self.saved(0);
        let mut p = self.fin_mlist(None)?;
        let mut f = None;
        if let [Item::Noad(q)] = p.as_slice() {
            if q.kind == Kind::Ord {
                if q.sub == Field::Empty && q.sup == Field::Empty {
                    let Some(Item::Noad(q)) = p.pop() else {
                        unreachable!()
                    };
                    f = Some(q.nucleus);
                }
            } else if matches!(q.kind, Kind::Accent { .. })
                && s == NUCLEUS
                && self.tail_noad().is_some_and(|t| t.kind == Kind::Ord)
            {
                // §1187: replace the tail of the list by `p`.
                self.mlist_mut().pop();
                self.mlist_mut().extend(p);
                return Ok(());
            }
        }
        let f = f.unwrap_or(Field::Mlist(p));
        if let Some(n) = self.tail_noad() {
            *field(n, s) = f;
        }
        Ok(())
    }

    /// §1191
    pub(crate) fn math_left_right(&mut self) -> Result<(), Jump> {
        let t = self.cur_chr;
        if t != LEFT_NOAD && self.cur_group() != MATH_LEFT_GROUP {
            // §1192: try to recover from mismatched \right (or \middle).
            if self.cur_group() == MATH_SHIFT_GROUP {
                self.scan_delimiter(false)?;
                self.print_err(b"Extra ");
                if t == MIDDLE_NOAD {
                    self.print_esc(b"middle");
                    self.help(&[b"I'm ignoring a \\middle that had no matching \\left."]);
                } else {
                    self.print_esc(b"right");
                    self.help(&[b"I'm ignoring a \\right that had no matching \\left."]);
                }
                return self.error();
            }
            return self.off_save();
        }
        let d = self.scan_delimiter(false)?; // `delimiter`
        if t == LEFT_NOAD {
            self.push_math(MATH_LEFT_GROUP)?;
            self.push_noad(Noad::new(Kind::Left(d)));
        } else if t == MIDDLE_NOAD {
            // e-TeX: close the group and open another with the list so far.
            let p = self.fin_mlist(Some(Noad::new(Kind::Middle(d))))?;
            self.unsave()?;
            self.push_math(MATH_LEFT_GROUP)?;
            *self.mlist_mut() = p;
            self.set_middle(true);
        } else {
            let p = self.fin_mlist(Some(Noad::new(Kind::Right(d))))?;
            self.unsave()?; // end of `math_left_group`
            let mut n = Noad::new(Kind::Inner);
            n.nucleus = Field::Mlist(p);
            self.push_noad(n);
        }
        Ok(())
    }

    /// §1195: check that the necessary fonts for math symbols are present;
    /// if not, flush the current math lists and return true (`danger`).
    fn check_math_fonts(&mut self) -> Result<bool, Jump> {
        let params = |t: &Self, n: i32| {
            let f = t.fam_fnt(n);
            t.font_read(f, crate::track::font::PARAMS);
            t.fonts.get(f).params.len()
        };
        if params(self, 2 + TEXT_SIZE) < TOTAL_MATHSY_PARAMS
            || params(self, 2 + SCRIPT_SIZE) < TOTAL_MATHSY_PARAMS
            || params(self, 2 + SCRIPT_SCRIPT_SIZE) < TOTAL_MATHSY_PARAMS
        {
            self.print_err(b"Math formula deleted: Insufficient symbol fonts");
            self.help(&[
                b"Sorry, but I can't typeset math unless \\textfont 2",
                b"and \\scriptfont 2 and \\scriptscriptfont 2 have all",
                b"the \\fontdimen values needed in math symbol fonts.",
            ]);
            self.error()?;
            self.flush_math();
            return Ok(true);
        }
        if params(self, 3 + TEXT_SIZE) < TOTAL_MATHEX_PARAMS
            || params(self, 3 + SCRIPT_SIZE) < TOTAL_MATHEX_PARAMS
            || params(self, 3 + SCRIPT_SCRIPT_SIZE) < TOTAL_MATHEX_PARAMS
        {
            self.print_err(b"Math formula deleted: Insufficient extension fonts");
            self.help(&[
                b"Sorry, but I can't typeset math unless \\textfont 3",
                b"and \\scriptfont 3 and \\scriptscriptfont 3 have all",
                b"the \\fontdimen values needed in math extension fonts.",
            ]);
            self.error()?;
            self.flush_math();
            return Ok(true);
        }
        Ok(false)
    }

    /// §719: `flush_math`.
    pub(crate) fn flush_math(&mut self) {
        self.mlist_mut().clear();
        *self.incompleat_mut() = None;
    }

    /// §700: `math_quad` in size `s`.
    fn math_quad(&self, s: i32) -> Scaled {
        self.font_param(6, self.fam_fnt(2 + s))
    }

    /// §1197: check that another $ follows.
    pub(crate) fn check_dollar_follows(&mut self) -> Result<(), Jump> {
        self.get_x_token()?;
        if self.cur_cmd != MATH_SHIFT {
            self.print_err(b"Display math should end with $$");
            self.help(&[
                b"The `$' that I just saw supposedly matches a previous `$$'.",
                b"So I shall assume that you typed `$$' both times.",
            ]);
            self.back_error()?;
        }
        Ok(())
    }

    /// §1194
    pub(crate) fn after_math(&mut self) -> Result<(), Jump> {
        let mut j = self.lr_box_mut().take(); // retrieve the prototype box
        let mut danger = self.check_math_fonts()?;
        let mut m = self.mode();
        let mut l = false;
        let mut p = self.fin_mlist(None)?; // this pops the nest
        let a;
        if self.mode() == -m {
            // end of equation number
            self.check_dollar_follows()?;
            let h = self.mlist_to_hlist(p, TEXT_STYLE, false)?;
            let mut eqno = self.hpack(h, Spec::NATURAL, None);
            eqno.subtype = self.dlist();
            a = Some(eqno);
            self.unsave()?;
            self.set_save_ptr(self.save_ptr() - 1); // now `cur_group=math_shift_group`
            if self.saved(0) == 1 {
                l = true;
            }
            j = self.lr_box_mut().take();
            danger = self.check_math_fonts()?;
            m = self.mode();
            p = self.fin_mlist(None)?;
        } else {
            a = None;
        }
        if m < 0 {
            // §1196: finish math in text.
            let ms = self.dimen_par(MATH_SURROUND_CODE);
            self.tail_append(new_math(ms, BEFORE));
            let h = self.mlist_to_hlist(p, TEXT_STYLE, self.mode() > 0)?;
            self.append_nodes(h);
            self.tail_append(new_math(ms, AFTER));
            self.set_space_factor(1000);
            self.unsave()
        } else {
            if a.is_none() {
                self.check_dollar_follows()?;
            }
            self.finish_displayed_math(p, a, l, danger, j.as_ref())
        }
    }

    /// §1199: finish displayed math.
    fn finish_displayed_math(
        &mut self,
        p: Vec<Item>,
        a: Option<BoxNode>,
        l: bool,
        danger: bool,
        j: Option<&BoxNode>,
    ) -> Result<(), Jump> {
        let p = self.mlist_to_hlist(p, DISPLAY_STYLE, false)?;
        let mut adjust = Vec::new();
        let packed = self.hpack_full(p, Spec::NATURAL, Some(&mut adjust));
        let ts = packed.total_shrink;
        let mut b = packed.node;
        let mut w = b.width;
        let z = self.dimen_par(DISPLAY_WIDTH_CODE);
        let s = self.dimen_par(DISPLAY_INDENT_CODE);
        let (mut e, q) = match &a {
            Some(a) if !danger => (a.width, a.width + self.math_quad(TEXT_SIZE)),
            _ => (0, 0),
        };
        if w + q > z {
            // §1201: squeeze the equation as much as possible; if there is
            // an equation number that should go on a separate line by
            // itself, set `e:=0`.
            if e != 0 && (w - ts[0] + q <= z || ts[1] != 0 || ts[2] != 0 || ts[3] != 0) {
                b = self.hpack(b.list, Spec::Exactly(z - q), None);
            } else {
                e = 0;
                if w > z {
                    b = self.hpack(b.list, Spec::Exactly(z), None);
                }
            }
            w = b.width;
        }
        // §1202: determine the displacement, `d`, of the left edge of the
        // equation, with respect to the line size `z`, assuming that
        // `l=false`.
        b.subtype = self.dlist();
        let mut d = half(z - w);
        if e > 0 && d < 2 * e {
            // too close
            d = half(z - w - e);
            if matches!(b.list.first(), Some(Node::Glue { .. } | Node::Leaders(_))) {
                d = 0;
            }
        }
        // §1203: append the glue or equation number preceding the display.
        self.tail_append(Node::Penalty(self.int_par(PRE_DISPLAY_PENALTY_CODE)));
        let (g1, mut g2) = if d + s <= self.dimen_par(PRE_DISPLAY_SIZE_CODE) || l {
            // not enough clearance
            (ABOVE_DISPLAY_SKIP_CODE, BELOW_DISPLAY_SKIP_CODE)
        } else {
            (ABOVE_DISPLAY_SHORT_SKIP_CODE, BELOW_DISPLAY_SHORT_SKIP_CODE)
        };
        let mut a = a;
        if l && e == 0 {
            // it follows that `type(a)=hlist_node`
            if let Some(a) = a.take() {
                self.app_display(j, a, 0)?;
            }
            self.tail_append(Node::Penalty(INF_PENALTY));
        } else {
            let g = self.new_param_glue(g1);
            self.tail_append(g);
        }
        // §1204: append the display and perhaps also the equation number.
        if e != 0 {
            let r = new_kern(z - w - e - d);
            let a = a.take().unwrap_or_default();
            let list = if l {
                d = 0;
                alloc::vec![Node::Box(a.share()), r, Node::Box(b.share())]
            } else {
                alloc::vec![Node::Box(b.share()), r, Node::Box(a.share())]
            };
            b = self.hpack(list, Spec::NATURAL, None);
        }
        self.app_display(j, b, d)?;
        // §1205: append the glue or equation number following the display.
        if let Some(a) = a
            && e == 0
            && !l
        {
            self.tail_append(Node::Penalty(INF_PENALTY));
            let d = z - a.width;
            self.app_display(j, a, d)?;
            g2 = 0;
        }
        // migrating material comes after equation number
        self.sync_list(&mut adjust);
        self.nodes_mut().extend(adjust);
        self.tail_append(Node::Penalty(self.int_par(POST_DISPLAY_PENALTY_CODE)));
        if g2 > 0 {
            let g = self.new_param_glue(g2);
            self.tail_append(g);
        }
        self.resume_after_display()
    }

    /// §1200
    pub(crate) fn resume_after_display(&mut self) -> Result<(), Jump> {
        if self.cur_group() != MATH_SHIFT_GROUP {
            return self.confusion(b"display");
        }
        self.unsave()?;
        self.set_pg(self.pg() + 3);
        self.push_nest()?;
        self.set_mode(HMODE);
        self.set_space_factor(1000);
        self.set_cur_lang();
        self.set_clang(self.hyph.cur_lang);
        self.set_pg(
            (norm_min(self.int_par(LEFT_HYPHEN_MIN_CODE)) * 0o100
                + norm_min(self.int_par(RIGHT_HYPHEN_MIN_CODE)))
                * 0o200000
                + self.hyph.cur_lang,
        );
        // §443: scan an optional space.
        self.get_x_token()?;
        if self.cur_cmd != SPACER {
            self.back_input()?;
        }
        if self.nest_ptr() == 1 {
            // (the paragraph goes on: its start again, as at §1091)
            self.par_start = self.stop_at_candidate && crate::machine::clean_cuts();
            self.build_page()?;
        }
        Ok(())
    }

    /// §1206: finish an alignment in a display: `p` is the alignment.
    pub(crate) fn finish_display_alignment(
        &mut self,
        p: Vec<Node>,
        aux_save: Scaled,
    ) -> Result<(), Jump> {
        self.do_assignments()?;
        if self.cur_cmd == MATH_SHIFT {
            self.check_dollar_follows()?;
        } else {
            // §1207: pontificate about improper alignment in display.
            self.print_err(b"Missing $$ inserted");
            self.help(&[
                b"Displays can use special alignments (like \\eqalignno)",
                b"only if nothing but the alignment itself is between $$'s.",
            ]);
            self.back_error()?;
        }
        self.pop_nest();
        self.tail_append(Node::Penalty(self.int_par(PRE_DISPLAY_PENALTY_CODE)));
        let g = self.new_param_glue(ABOVE_DISPLAY_SKIP_CODE);
        self.tail_append(g);
        let mut p = p;
        self.sync_list(&mut p);
        self.nodes_mut().extend(p);
        self.tail_append(Node::Penalty(self.int_par(POST_DISPLAY_PENALTY_CODE)));
        let g = self.new_param_glue(BELOW_DISPLAY_SKIP_CODE);
        self.tail_append(g);
        self.set_prev_depth(aux_save);
        self.resume_after_display()
    }
}
