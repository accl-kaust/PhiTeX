//! Math mode (§1136–§1206): the mlist as a list of values, built by the
//! math commands, and turned into an hlist by `phitex-typeset`'s
//! `mlist_to_hlist` when the formula ends.
//!
//! A noad is `[T_NOAD, type, a, b, nucleus, supscr, subscr]`, with
//! tex.web's types (16 Ord … 29 Vcenter); `a` is an Op's limits, a
//! radical's delimiter or an accent's family, `b` an accent's character.
//! A field is `[0]` empty, `[1, fam, ch]` a math character, `[3, box]`
//! a box, or `[4, items]` a formula. A sub- or superscript changes the
//! list's last noad: the list gets a delete marker and the new noad.

use std::rc::Rc;

use phitex_defuse::{ValueId, Values};
use phitex_typeset::arith::{MAX_DIMEN, half};
use phitex_typeset::glue::Glue;
use phitex_typeset::glue::GlueSign;
use phitex_typeset::linebreak::INF_PENALTY;
use phitex_typeset::math::{
    Delimiter, Field, Fraction, Limits, MItem, MList, MathParams, Noad, NoadKind, Style,
};
use phitex_typeset::node::{BoxNode, GlueKind, KernKind, MathKind, Node};
use phitex_typeset::pack::PackParams;
use phitex_typeset::pack::Spec;

use crate::build::Mode;
use crate::fields::Par;
use crate::nodes::{self, Side, T_DELETE};
use crate::run::{R, Run};
use crate::val::{Tok, cmd, group, int_of, list_of};

pub(crate) const T_NOAD: i64 = 20;
/// `[T_FRACTION, thickness (i64::MIN: default), numerator, denominator,
/// left delimiter, right delimiter]`.
pub(crate) const T_FRACTION: i64 = 21;
pub(crate) const T_LEFT: i64 = 22;
pub(crate) const T_RIGHT: i64 = 23;
pub(crate) const T_STYLE: i64 = 24;
/// `[T_CHOICE, display, text, script, scriptscript]`.
pub(crate) const T_CHOICE: i64 = 25;

const ORD: i64 = 16;
const OP: i64 = 17;
const INNER: i64 = 23;
const RADICAL: i64 = 24;
const ACCENT: i64 = 28;
const VCENTER: i64 = 29;
const VAR_CODE: i32 = 0x7000;
const LEFT_NOAD: i32 = 30;
const DEFAULT_CODE: i64 = i64::MIN;

const F_NUCLEUS: i64 = 1;

fn empty(v: &Values) -> ValueId {
    v.list(vec![v.int(0)])
}

fn math_char(v: &Values, fam: i32, ch: i32) -> ValueId {
    v.list(vec![v.int(1), v.int(i64::from(fam)), v.int(i64::from(ch))])
}

fn noad(v: &Values, t: i64, a: i64, b: i64, nuc: ValueId, sup: ValueId, sub: ValueId) -> ValueId {
    v.list(vec![
        v.int(T_NOAD),
        v.int(t),
        v.int(a),
        v.int(b),
        nuc,
        sup,
        sub,
    ])
}

fn sub_box(v: &Values, b: ValueId) -> ValueId {
    v.list(vec![v.int(3), b])
}

fn sub_mlist(v: &Values, items: Vec<ValueId>) -> ValueId {
    v.list(vec![v.int(4), v.list(items)])
}

fn is_empty_field(v: &Values, f: ValueId) -> bool {
    list_of(v, f).first().is_none_or(|&t| int_of(v, t) == 0)
}

/// An Ord noad whose nucleus is the box `b` (§1076, §1093).
pub(crate) fn box_noad(v: &Values, b: ValueId) -> ValueId {
    let e = empty(v);
    noad(v, ORD, 0, 0, sub_box(v, b), e, e)
}

fn delimiter(d: i64) -> Delimiter {
    // (XeTeX's extended delimiter code, `\Udelimiter`, `\Uradical`,
    // `\Udelcode`: a family of 8 bits and a character of 21, one size)
    if d >= XE_DELIM {
        return Delimiter {
            small_fam: u8::try_from((d / 0x20_0000) % 0x100).unwrap_or(0),
            small_char: u32::try_from(d % 0x20_0000).unwrap_or(0),
            large_fam: 0,
            large_char: 0,
        };
    }
    let g = |s: u32, m: i64| u32::try_from((d >> s) & m).unwrap_or(0);
    Delimiter {
        small_fam: u8::try_from(g(20, 15)).unwrap_or(0),
        small_char: g(12, 255),
        large_fam: u8::try_from(g(8, 15)).unwrap_or(0),
        large_char: g(0, 255),
    }
}

/// A `TeX82` math code (15 bits) as XeTeX's (`set_class_field` and
/// the others).
fn xe_of15(c: i32) -> i32 {
    crate::xetex::math_code(c / 0x1000, (c % 0x1000) / 0x100, c % 0x100)
}

/// XeTeX's extended delimiter code flag (`"40000000`).
pub(crate) const XE_DELIM: i64 = 0x4000_0000;

fn decode_field(v: &Values, f: ValueId, side: &mut Side) -> Field {
    let xs = list_of(v, f);
    let n = |i: usize| xs.get(i).map_or(0, |&y| int_of(v, y));
    match n(0) {
        1 => Field::MathChar {
            fam: u8::try_from(n(1)).unwrap_or(0),
            ch: u32::try_from(n(2)).unwrap_or(0),
        },
        3 => match xs.get(1).and_then(|&b| nodes::decode(v, b, side)) {
            Some(Node::Box(b)) => Field::SubBox(*b),
            _ => Field::Empty,
        },
        4 => Field::SubMlist(decode_mlist(
            v,
            &xs.get(1).map(|&l| list_of(v, l)).unwrap_or_default(),
            side,
        )),
        _ => Field::Empty,
    }
}

/// The mlist of the values `items`.
pub(crate) fn decode_mlist(v: &Values, items: &[ValueId], side: &mut Side) -> MList {
    let mut out = Vec::new();
    for &x in items {
        let xs = list_of(v, x);
        let n = |i: usize| xs.get(i).map_or(0, |&y| int_of(v, y));
        let sub = |i: usize, side: &mut Side| {
            decode_mlist(
                v,
                &xs.get(i).map(|&l| list_of(v, l)).unwrap_or_default(),
                side,
            )
        };
        match nodes::tag(v, x) {
            T_NOAD => {
                let kind = match n(1) {
                    17 => NoadKind::Op(match n(2) {
                        1 => Limits::Limits,
                        2 => Limits::NoLimits,
                        _ => Limits::Normal,
                    }),
                    18 => NoadKind::Bin,
                    19 => NoadKind::Rel,
                    20 => NoadKind::Open,
                    21 => NoadKind::Close,
                    22 => NoadKind::Punct,
                    23 => NoadKind::Inner,
                    24 => NoadKind::Radical(delimiter(n(2))),
                    26 => NoadKind::Under,
                    27 => NoadKind::Over,
                    // (XeTeX's subtype above the family)
                    28 => NoadKind::Accent(
                        u8::try_from(n(2) % 0x100).unwrap_or(0),
                        u32::try_from(n(3)).unwrap_or(0),
                        u8::try_from(n(2) / 0x100).unwrap_or(0),
                    ),
                    29 => NoadKind::Vcenter,
                    _ => NoadKind::Ord,
                };
                out.push(MItem::Noad(Noad {
                    kind,
                    nucleus: decode_field(v, xs[4], side),
                    supscr: decode_field(v, xs[5], side),
                    subscr: decode_field(v, xs[6], side),
                }));
            }
            T_FRACTION => out.push(MItem::Fraction(Fraction {
                thickness: if n(1) == DEFAULT_CODE {
                    None
                } else {
                    Some(i32::try_from(n(1)).unwrap_or(0))
                },
                numerator: sub(2, side),
                denominator: sub(3, side),
                left: delimiter(n(4)),
                right: delimiter(n(5)),
            })),
            T_LEFT => out.push(MItem::Left(delimiter(n(1)))),
            T_RIGHT => out.push(MItem::Right(delimiter(n(1)), n(2) != 0)),
            T_STYLE => out.push(MItem::Style(Style(u8::try_from(n(1)).unwrap_or(2)))),
            T_CHOICE => out.push(MItem::Choice(Box::new([
                sub(1, side),
                sub(2, side),
                sub(3, side),
                sub(4, side),
            ]))),
            _ => {
                if let Some(nd) = nodes::decode(v, x, side) {
                    out.push(MItem::Node(nd));
                }
            }
        }
    }
    out
}

/// Whether `c` is a command that only math mode knows (§1046's
/// `non_math` cases).
pub(crate) fn non_math(c: u8) -> bool {
    matches!(
        c,
        cmd::SUP_MARK
            | cmd::SUB_MARK
            | cmd::MATH_CHAR_NUM
            | cmd::MATH_GIVEN
            | cmd::MATH_COMP
            | cmd::DELIM_NUM
            | cmd::LEFT_RIGHT
            | cmd::ABOVE
            | cmd::RADICAL
            | cmd::MATH_STYLE
            | cmd::MATH_CHOICE
            | cmd::VCENTER
            | cmd::NON_SCRIPT
            | cmd::MKERN
            | cmd::LIMIT_SWITCH
            | cmd::MSKIP
            | cmd::MATH_ACCENT
    )
}

impl Run<'_, '_> {
    /// `insert_dollar_sign` (§1047).
    pub(crate) fn insert_dollar_sign(&mut self) -> R<()> {
        self.back_cur();
        self.back_input(Tok::ch(3, u32::from(b'$')));
        self.log("error", "Missing $ inserted")
    }

    /// `off_save` (§1064–§1066): a token that closes a group other than
    /// the innermost one. At the bottom level it is an extra one, an
    /// error, and ignored; else it is put back, and what closes the
    /// innermost group is inserted before it, with an error.
    pub(crate) fn off_save(&mut self, md: &Mode) -> R<()> {
        if md.group == group::BOTTOM {
            let what = match self.cur.cmd {
                cmd::MATH_SHIFT => "math shift character $",
                cmd::END_GROUP => "\\endgroup",
                _ => "\\right",
            };
            return self.log("error", &format!("Extra {what}"));
        }
        self.back_cur();
        let (toks, what) = match md.group {
            group::SEMI_SIMPLE => (vec![Tok::cs("\u{10FFFE}endgroup")], "\\endgroup"),
            group::MATH_SHIFT => (vec![Tok::ch(3, u32::from(b'$'))], "$"),
            group::MATH_LEFT => (
                vec![Tok::cs("\u{10FFFE}right"), Tok::ch(12, u32::from(b'.'))],
                "\\right. ",
            ),
            _ => (vec![Tok::ch(2, u32::from(b'}'))], "}"),
        };
        self.push_toks(toks);
        self.log("error", &format!("Missing {what} inserted"))
    }

    /// `init_math` (§1138–§1139, §1145).
    pub(crate) fn init_math(&mut self, md: &Mode) -> R<()> {
        self.get_token();
        self.ck()?;
        if self.cur.cmd == cmd::MATH_SHIFT && md.m > 0 {
            return self.init_display(md);
        }
        self.back_cur();
        self.go_into_math(md, 0)
    }

    /// §1139: ordinary math mode; `code` is the formula's kind (0 text,
    /// 2 `\eqno`, 3 `\leqno`), kept in the group's kind.
    fn go_into_math(&mut self, md: &Mode, code: i64) -> R<()> {
        let v = self.v;
        self.push_math(md, group::MATH_SHIFT, -203, vec![v.int(code)]);
        self.set_par(Par::fam, v.int(-1), false);
        let e = self.par(Par::everymath)?;
        let toks = crate::val::toks_of(v, e);
        self.push_toks(toks);
        Ok(())
    }

    /// §1145: `$$` in a paragraph. The paragraph so far is broken into
    /// lines (with `\displaywidowpenalty`); the display's group keeps
    /// `[1, vertical list, its mode]`.
    fn init_display(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let (items, _) = self.list_items(md.lid)?;
        // the paragraph's level is popped before line_break (as end_graf;
        // §815 `pop_nest`), under groups opened inside the paragraph too
        // (`\\begin{equation}`): they stay, in the vertical list (§1063)
        if let Some(l) = self.top()
            && l.group == 0
            && l.mode == 102
        {
            let l = self.pop_level().expect("the paragraph's level");
            self.muts += 1;
            self.last_par = l.par.unwrap_or((0, 0));
        } else if let Some(k) = self.par_level_under_groups() {
            self.end_graf_under_groups(k)?;
        }
        let w = if items.is_empty() {
            -MAX_DIMEN
        } else {
            let last = self.line_break(md, crate::nest::of_values(items), true)?;
            self.pre_display_size(last.as_ref())?
        };
        // §1149
        let vmd = self.mode()?;
        let pg = self.prev_graf(&vmd)?;
        let shape: Vec<i32> = self
            .get_loc(crate::fields::cell::PARSHAPE)?
            .map(|x| list_of(v, x))
            .unwrap_or_default()
            .iter()
            .map(|&x| i32::try_from(int_of(v, x)).unwrap_or(0))
            .collect();
        let (l, s) = if shape.len() < 2 {
            let hi = self.ipar(Par::hangindent)?;
            let ha = self.ipar(Par::hangafter)?;
            let hsize = self.ipar(Par::hsize)?;
            if hi != 0 && ((ha >= 0 && pg + 2 > ha) || (pg + 1 < -ha)) {
                (hsize - hi.abs(), hi.max(0))
            } else {
                (hsize, 0)
            }
        } else {
            let n = i32::try_from(shape.len() / 2).unwrap_or(1);
            let k = usize::try_from(if pg + 2 >= n { n } else { pg + 2 }).unwrap_or(1);
            (shape[2 * k - 1], shape[2 * k - 2])
        };
        let vm = md.vm;
        let extra = vec![v.int(1), v.int(md.vlid), v.int(i64::from(vm))];
        self.push_list(203, group::MATH_SHIFT, extra);
        self.set_par(Par::fam, v.int(-1), false);
        self.set_par(Par::predisplaysize, v.int(i64::from(w)), false);
        self.set_par(Par::displaywidth, v.int(i64::from(l)), false);
        self.set_par(Par::displayindent, v.int(i64::from(s)), false);
        let e = self.par(Par::everydisplay)?;
        let toks = crate::val::toks_of(v, e);
        self.push_toks(toks);
        // §1145: `if nest_ptr=1 then build_page` (the paragraph's lines
        // so far are on the page's list: a page can finish here, the
        // display not begun; `\everydisplay` is read after the routine)
        self.finish_candidate()?;
        Ok(())
    }

    /// §1146: the width of the paragraph's last line up to its last
    /// visible item, plus two quads; `max_dimen` if glue stretched or
    /// shrank before it.
    fn pre_display_size(&mut self, last: Option<&BoxNode>) -> R<i32> {
        let Some(b) = last else { return Ok(-MAX_DIMEN) };
        let f = self.cint(crate::fields::cell::FONT)?;
        let quad = self.font_dimen(f, 6)?;
        let view = self.font_view(&b.list)?;
        let mut v = b.shift + 2 * quad;
        let mut w = -MAX_DIMEN;
        let cw = |font: u32, ch: u16| view.char_dims(font, u32::from(ch)).map_or(0, |d| d.width);
        for n in &b.list {
            let (d, found) = match n {
                Node::Char { font, ch } => (cw(*font, *ch), true),
                Node::Lig(l) => (cw(l.font, l.ch), true),
                Node::Box(x) => (x.width, true),
                Node::Rule(r) => (r.width, true),
                Node::Kern(k, _) | Node::Math(_, k) => (*k, false),
                // pdfTeX §1146: "Let d be the width of the whatsit p"
                Node::Whatsit(_, Some(s)) => (s.width, false),
                Node::Glue(g) => {
                    let q = &g.spec;
                    match b.glue.sign {
                        GlueSign::Stretching
                            if b.glue.order == q.stretch_order && q.stretch != 0 =>
                        {
                            v = MAX_DIMEN;
                        }
                        GlueSign::Shrinking if b.glue.order == q.shrink_order && q.shrink != 0 => {
                            v = MAX_DIMEN;
                        }
                        _ => {}
                    }
                    let leaders = matches!(
                        g.kind,
                        GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders
                    );
                    (q.width, leaders)
                }
                _ => (0, false),
            };
            if found {
                if v < MAX_DIMEN {
                    v += d;
                    w = v;
                } else {
                    w = MAX_DIMEN;
                    break;
                }
            } else if v < MAX_DIMEN {
                v += d;
            }
        }
        Ok(w)
    }

    /// `push_math` (§1136): a math group whose list is this node's.
    fn push_math(&mut self, md: &Mode, grp: i64, m: i32, extra: Vec<ValueId>) -> i64 {
        let _ = md;
        self.push_list(m, grp, extra)
    }

    fn mathcode(&mut self, c: i32) -> R<i32> {
        let x = self.code(3, c)?;
        if self.xe {
            // (XeTeX's math code in TeX's form where it has one: a
            // character past 255 or a family past 15 is Unicode math)
            return Ok(crate::xetex::math15(x));
        }
        Ok(x)
    }

    /// The math code of the `math_given` command in `cur` (a
    /// `\\Umathchardef`'s in `TeX82`'s form, [`crate::xetex::math15`]).
    fn math_given(&mut self) -> R<i32> {
        if self.xe
            && let crate::val::Mean::MathCharDef(n) = self.cur_mean()?
            && n >= crate::xetex::U_MATH
        {
            #[allow(clippy::cast_possible_truncation)]
            return Ok(crate::xetex::math15(n as i32));
        }
        Ok(self.cur.chr)
    }

    /// XeTeX's `fam_in_range` (`number_math_families`: 256).
    fn xe_fam_in_range(&mut self) -> R<Option<i32>> {
        let f = self.ipar(Par::fam)?;
        Ok((0..256).contains(&f).then_some(f))
    }

    /// XeTeX's math char field of code `c` (class, family, character):
    /// the family `\fam` for a variable one in range.
    fn xe_math_field(&mut self, c: i32) -> R<(i32, i32, i32)> {
        let (class, mut fam, ch) = crate::xetex::math_fields(c);
        if class == 7
            && let Some(f) = self.xe_fam_in_range()?
        {
            fam = f;
        }
        Ok((class, fam, ch))
    }

    /// XeTeX's `set_math_char(c)`: `c` a math code of 21-bit characters
    /// and 8-bit families (an active one: `chr` as an active character).
    fn xe_set_math_char(&mut self, md: &Mode, c: i32, chr: i32) -> R<()> {
        if crate::xetex::math_fields(c).2 == crate::xetex::ACTIVE_MATH_CHAR {
            self.back_input(Tok::ch(13, u32::try_from(chr).unwrap_or(0)));
            return Ok(());
        }
        let (class, fam, ch) = self.xe_math_field(c)?;
        let t = if class == 7 { ORD } else { ORD + i64::from(class) };
        let v = self.v;
        let e = empty(v);
        let n = noad(v, t, 0, 0, math_char(v, fam, ch), e, e);
        self.append(md.lid, n);
        Ok(())
    }

    /// XeTeX's math code of the math character command in `cur`
    /// (`\mathchar`, `\Umathchar`, `\Umathcharnum`, a math character
    /// given, `\delimiter`, `\Udelimiter`), scanned.
    fn xe_math_char_code(&mut self) -> R<i32> {
        Ok(match self.cur.cmd {
            cmd::MATH_CHAR_NUM => match self.cur.chr {
                2 => self.scan_umath(true)?,
                1 => self.scan_umath(false)?,
                _ => xe_of15(self.scan_fifteen_bit_int()?),
            },
            cmd::MATH_GIVEN => {
                if let crate::val::Mean::MathCharDef(n) = self.cur_mean()?
                    && n >= crate::xetex::U_MATH
                {
                    i32::try_from(n & 0xFFFF_FFFF).unwrap_or(0)
                } else {
                    xe_of15(self.cur.chr)
                }
            }
            _ => {
                if self.cur.chr == 1 {
                    self.scan_umath(true)?
                } else {
                    xe_of15(self.scan_twenty_seven_bit_int()? / 0x1000)
                }
            }
        })
    }

    fn fam_in_range(&mut self) -> R<Option<i32>> {
        let f = self.ipar(Par::fam)?;
        Ok((0..16).contains(&f).then_some(f))
    }

    /// `set_math_char` (§1155).
    fn set_math_char(&mut self, md: &Mode, c: i32, chr: i32) -> R<()> {
        if c >= 0x8000 {
            // an active character
            self.back_input(Tok::ch(13, u32::try_from(chr).unwrap_or(0)));
            return Ok(());
        }
        let v = self.v;
        let mut fam = (c / 256) % 16;
        let t = if c >= VAR_CODE {
            if let Some(f) = self.fam_in_range()? {
                fam = f;
            }
            ORD
        } else {
            ORD + i64::from(c / 0x1000)
        };
        let e = empty(v);
        let n = noad(v, t, 0, 0, math_char(v, fam, c % 256), e, e);
        self.append(md.lid, n);
        Ok(())
    }

    /// `scan_math` (§1151): the field, or `None` when a math group
    /// opened whose end fills field `code` of the list's last noad.
    fn scan_math(&mut self, md: &Mode, code: i64) -> R<Option<ValueId>> {
        if self.xe {
            return self.xe_scan_math(md, code);
        }
        let c = loop {
            self.skip_blanks_relax_x()?;
            let c = match self.cur.cmd {
                cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN => {
                    let c = self.mathcode(self.cur.chr)?;
                    if c == 0x8000 {
                        let ch = u32::try_from(self.cur.chr).unwrap_or(0);
                        self.back_input(Tok::ch(13, ch));
                        continue;
                    }
                    c
                }
                cmd::CHAR_NUM => {
                    let n = self.scan_char_num()?;
                    let c = self.mathcode(n)?;
                    if c == 0x8000 {
                        self.back_input(Tok::ch(13, u32::try_from(n).unwrap_or(0)));
                        continue;
                    }
                    c
                }
                cmd::MATH_CHAR_NUM if self.cur.chr > 0 => {
                    let x = self.scan_umath(self.cur.chr == 2)?;
                    crate::xetex::math15(x)
                }
                cmd::MATH_CHAR_NUM => self.scan_fifteen_bit_int()?,
                cmd::MATH_GIVEN => self.math_given()?,
                cmd::DELIM_NUM if self.cur.chr > 0 => {
                    let x = self.scan_umath(true)?;
                    crate::xetex::math15(x)
                }
                cmd::DELIM_NUM => self.scan_twenty_seven_bit_int()? / 0x1000,
                _ => {
                    self.back_cur();
                    self.scan_left_brace()?;
                    let v = self.v;
                    self.push_math(md, group::MATH, -203, vec![v.int(code), v.int(md.lid)]);
                    return Ok(None);
                }
            };
            break c;
        };
        let mut fam = (c / 256) % 16;
        if c >= VAR_CODE
            && let Some(f) = self.fam_in_range()?
        {
            fam = f;
        }
        Ok(Some(math_char(self.v, fam, c % 256)))
    }

    /// XeTeX's `scan_math`.
    fn xe_scan_math(&mut self, md: &Mode, code: i64) -> R<Option<ValueId>> {
        let c = loop {
            self.skip_blanks_relax_x()?;
            let c = match self.cur.cmd {
                cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN | cmd::CHAR_NUM => {
                    let ch = if self.cur.cmd == cmd::CHAR_NUM {
                        self.scan_char_num()?
                    } else {
                        self.cur.chr
                    };
                    let c = self.code(3, ch)?;
                    if crate::xetex::math_fields(c).2 == crate::xetex::ACTIVE_MATH_CHAR {
                        self.back_input(Tok::ch(13, u32::try_from(ch).unwrap_or(0)));
                        continue;
                    }
                    c
                }
                cmd::MATH_CHAR_NUM | cmd::MATH_GIVEN | cmd::DELIM_NUM => self.xe_math_char_code()?,
                _ => {
                    self.back_cur();
                    self.scan_left_brace()?;
                    let v = self.v;
                    self.push_math(md, group::MATH, -203, vec![v.int(code), v.int(md.lid)]);
                    return Ok(None);
                }
            };
            break c;
        };
        let (_, fam, ch) = self.xe_math_field(c)?;
        Ok(Some(math_char(self.v, fam, ch)))
    }

    /// XeTeX's `scan_delimiter`: an extended code ([`XE_DELIM`]) for
    /// `\Uradical`, `\Udelimiter` and a `\Udelcode`.
    fn xe_scan_delimiter(&mut self, radical: bool) -> R<i64> {
        let ext = |r: &mut Self| -> R<i64> {
            let fam = r.scan_bounded(255, "Bad math family")?;
            let ch = r.scan_usv_num()?;
            Ok(XE_DELIM + i64::from(fam) * 0x20_0000 + i64::from(ch))
        };
        let mut val = if radical {
            if self.cur.chr == 1 {
                ext(self)?
            } else {
                i64::from(self.scan_twenty_seven_bit_int()?)
            }
        } else {
            self.skip_blanks_relax_x()?;
            match self.cur.cmd {
                cmd::LETTER | cmd::OTHER_CHAR => i64::from(self.code(4, self.cur.chr)?),
                cmd::DELIM_NUM if self.cur.chr == 1 => {
                    let _ = self.scan_bounded(7, "Bad math class")?;
                    ext(self)?
                }
                cmd::DELIM_NUM => i64::from(self.scan_twenty_seven_bit_int()?),
                _ => -1,
            }
        };
        if val < 0 {
            self.log("error", "Missing delimiter (. inserted)")?;
            self.back_cur();
            val = 0;
        }
        Ok(val)
    }

    /// XeTeX's `math_ac`: `\Umathaccent` with `fixed` and `bottom`.
    fn xe_math_ac(&mut self, md: &Mode) -> R<()> {
        if self.cur.cmd == cmd::ACCENT {
            self.log("error", "Please use \\mathaccent for accents in math mode")?;
        }
        let mut sub = 0;
        let c = if self.cur.chr == 1 {
            if self.scan_keyword("fixed")? {
                sub = 1;
            } else if self.scan_keyword("bottom")? {
                sub = if self.scan_keyword("fixed")? { 3 } else { 2 };
            }
            self.scan_umath(true)?
        } else {
            xe_of15(self.scan_fifteen_bit_int()?)
        };
        let (_, fam, ch) = self.xe_math_field(c)?;
        self.noad_with_nucleus(md, ACCENT, i64::from(fam) + sub * 0x100, i64::from(ch))
    }

    /// A new noad of type `t` whose nucleus is scanned (§1154, §1158).
    fn noad_with_nucleus(&mut self, md: &Mode, t: i64, a: i64, b: i64) -> R<()> {
        let v = self.v;
        let e = empty(v);
        let nuc = self.scan_math(md, F_NUCLEUS)?;
        let n = noad(v, t, a, b, nuc.unwrap_or(e), e, e);
        self.append(md.lid, n);
        Ok(())
    }

    /// `scan_delimiter` (§1160).
    pub(crate) fn scan_delimiter(&mut self, radical: bool) -> R<i64> {
        if self.xe {
            return self.xe_scan_delimiter(radical);
        }
        let mut val = if radical {
            self.scan_twenty_seven_bit_int()?
        } else {
            self.skip_blanks_relax_x()?;
            match self.cur.cmd {
                cmd::LETTER | cmd::OTHER_CHAR => self.code(4, self.cur.chr)?,
                cmd::DELIM_NUM => self.scan_twenty_seven_bit_int()?,
                _ => -1,
            }
        };
        if val < 0 {
            self.log("error", "Missing delimiter (. inserted)")?;
            self.back_cur();
            val = 0;
        }
        Ok(i64::from(val))
    }

    /// The math-mode cases of the big switch (§1045); `false` for the
    /// commands that behave as in the other modes.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn math_command(&mut self, md: &Mode) -> R<bool> {
        let v = self.v;
        match self.cur.cmd {
            // (XeTeX: math codes of 21-bit characters, 8-bit families)
            cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN if self.xe => {
                let ch = self.cur.chr;
                let c = self.code(3, ch)?;
                self.xe_set_math_char(md, c, ch)?;
            }
            cmd::CHAR_NUM if self.xe => {
                let ch = self.scan_char_num()?;
                let c = self.code(3, ch)?;
                self.xe_set_math_char(md, c, ch)?;
            }
            cmd::MATH_CHAR_NUM | cmd::MATH_GIVEN | cmd::DELIM_NUM if self.xe => {
                let c = self.xe_math_char_code()?;
                self.xe_set_math_char(md, c, 0)?;
            }
            cmd::RADICAL if self.xe => {
                let d = self.xe_scan_delimiter(true)?;
                self.noad_with_nucleus(md, RADICAL, d, 0)?;
            }
            cmd::ACCENT | cmd::MATH_ACCENT if self.xe => self.xe_math_ac(md)?,
            cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN => {
                let ch = self.cur.chr;
                let c = self.mathcode(ch)?;
                self.set_math_char(md, c, ch)?;
            }
            cmd::CHAR_NUM => {
                let ch = self.scan_char_num()?;
                let c = self.mathcode(ch)?;
                self.set_math_char(md, c, ch)?;
            }
            cmd::MATH_CHAR_NUM if self.cur.chr > 0 => {
                let x = self.scan_umath(self.cur.chr == 2)?;
                self.set_math_char(md, crate::xetex::math15(x), 0)?;
            }
            cmd::MATH_CHAR_NUM => {
                let c = self.scan_fifteen_bit_int()?;
                self.set_math_char(md, c, 0)?;
            }
            cmd::MATH_GIVEN => {
                let c = self.math_given()?;
                self.set_math_char(md, c, 0)?;
            }
            cmd::DELIM_NUM if self.cur.chr > 0 => {
                let x = self.scan_umath(true)?;
                self.set_math_char(md, crate::xetex::math15(x), 0)?;
            }
            cmd::DELIM_NUM => {
                let c = self.scan_twenty_seven_bit_int()? / 0x1000;
                self.set_math_char(md, c, 0)?;
            }
            cmd::LEFT_BRACE => {
                self.back_cur();
                self.noad_with_nucleus(md, ORD, 0, 0)?;
            }
            cmd::MATH_COMP => {
                let t = i64::from(self.cur.chr);
                self.noad_with_nucleus(md, t, 0, 0)?;
            }
            cmd::LIMIT_SWITCH => {
                // §1159
                let (items, _) = self.list_items(md.lid)?;
                match items.last() {
                    Some(&x) if nodes::tag(v, x) == T_NOAD && crate::val::int_at(v, x, 1) == OP => {
                        let mut xs = list_of(v, x);
                        xs[2] = v.int(i64::from(self.cur.chr));
                        self.replace_tail(md.lid, v.list(xs));
                    }
                    _ => self.log("error", "Limit controls must follow a math operator")?,
                }
            }
            cmd::RADICAL => {
                let d = self.scan_delimiter(true)?;
                self.noad_with_nucleus(md, RADICAL, d, 0)?;
            }
            cmd::ACCENT | cmd::MATH_ACCENT => {
                // §1165
                if self.cur.cmd == cmd::ACCENT {
                    self.log("error", "Please use \\mathaccent for accents in math mode")?;
                }
                let c = self.scan_fifteen_bit_int()?;
                let mut fam = (c / 256) % 16;
                if c >= VAR_CODE
                    && let Some(f) = self.fam_in_range()?
                {
                    fam = f;
                }
                self.noad_with_nucleus(md, ACCENT, i64::from(fam), i64::from(c % 256))?;
            }
            cmd::VCENTER => {
                // §1167
                let spec = if self.scan_keyword("to")? {
                    (0, self.scan_normal_dimen()?)
                } else if self.scan_keyword("spread")? {
                    (1, self.scan_normal_dimen()?)
                } else {
                    (1, 0)
                };
                self.scan_left_brace()?;
                let extra = vec![v.int(spec.0), v.int(i64::from(spec.1)), v.int(md.lid)];
                self.push_list(-1, group::VCENTER, extra);
                self.normal_paragraph()?;
                let e = self.par(Par::everyvbox)?;
                let toks = crate::val::toks_of(v, e);
                self.push_toks(toks);
            }
            cmd::MATH_STYLE => {
                let s = v.list(vec![v.int(T_STYLE), v.int(i64::from(self.cur.chr))]);
                self.append(md.lid, s);
            }
            cmd::NON_SCRIPT => {
                let g = nodes::glue_node(v, &Glue::ZERO, GlueKind::CondMath);
                self.append(md.lid, g);
            }
            cmd::MATH_CHOICE => {
                // §1172
                let u = ValueId::UNIT;
                self.scan_left_brace()?;
                self.push_math(
                    md,
                    group::MATH_CHOICE,
                    -203,
                    vec![v.int(0), v.int(md.lid), u, u, u],
                );
            }
            cmd::SUB_MARK | cmd::SUP_MARK => self.sub_sup(md)?,
            cmd::ABOVE => self.math_fraction(md)?,
            cmd::LEFT_RIGHT => self.math_left_right(md)?,
            cmd::MATH_SHIFT => {
                if md.group == group::MATH_SHIFT {
                    self.after_math(md)?;
                } else {
                    self.off_save(md)?;
                }
            }
            cmd::EQ_NO => {
                // §1140: start_eq_no, in a display only
                if md.m > 0 && md.group == group::MATH_SHIFT {
                    let code = 2 + i64::from(self.cur.chr);
                    self.go_into_math(md, code)?;
                } else {
                    self.log("error", "You can't use \\eqno here")?;
                }
            }
            cmd::SPACER | cmd::NO_BOUNDARY => {}
            cmd::EX_SPACE => {
                // §1041 `app_space` with space factor 1000
                let f = u32::try_from(self.cint(crate::fields::cell::FONT)?).unwrap_or(0);
                let g = self.space_glue(f, 1000, true)?;
                let x = nodes::encode(v, &g, &Side::default());
                self.append(md.lid, x);
            }
            cmd::PAR_END | cmd::STOP | cmd::VSKIP | cmd::UN_VBOX | cmd::VALIGN | cmd::HRULE => {
                self.insert_dollar_sign()?;
            }
            cmd::START_PAR => {
                // §1093
                if self.cur.chr > 0 {
                    let w = self.ipar(Par::parindent)?;
                    let b = crate::build::null_box(v, w);
                    self.append(md.lid, box_noad(v, b));
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Replaces the last item of list `lid`.
    fn replace_tail(&mut self, lid: i64, x: ValueId) {
        let m = nodes::marker(self.v, T_DELETE);
        self.append(lid, m);
        self.append(lid, x);
    }

    /// `sub_sup` (§1176).
    fn sub_sup(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let k = if self.cur.cmd == cmd::SUP_MARK { 5 } else { 6 };
        let (items, _) = self.list_items(md.lid)?;
        let last = items.last().copied();
        let scripts =
            last.filter(|&x| nodes::tag(v, x) == T_NOAD || nodes::tag(v, x) == T_FRACTION);
        let base = match scripts {
            Some(x) if nodes::tag(v, x) == T_NOAD && is_empty_field(v, list_of(v, x)[k]) => Some(x),
            Some(_) => {
                self.log(
                    "error",
                    if k == 5 {
                        "Double superscript"
                    } else {
                        "Double subscript"
                    },
                )?;
                None
            }
            None => None,
        };
        let e = empty(v);
        let (x, fresh) = match base {
            Some(x) => (x, false),
            None => (noad(v, ORD, 0, 0, e, e, e), true),
        };
        let code = i64::try_from(k).unwrap_or(5) - 3;
        match self.scan_math(md, code)? {
            Some(f) => {
                let mut xs = list_of(v, x);
                xs[k] = f;
                let y = v.list(xs);
                if fresh {
                    self.append(md.lid, y);
                } else {
                    self.replace_tail(md.lid, y);
                }
            }
            None => {
                if fresh {
                    self.append(md.lid, x);
                }
            }
        }
        Ok(())
    }

    /// `math_fraction` (§1181–§1183): the list so far becomes the
    /// numerator of the incompleat noad, kept as `[thickness, left,
    /// right, items before it]` in `inc:<list>`.
    fn math_fraction(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let c = self.cur.chr;
        let have = self.incs.iter().any(|x| x.0 == md.lid);
        let (mut l, mut r) = (0, 0);
        if c >= 3 {
            l = self.scan_delimiter(false)?;
            r = self.scan_delimiter(false)?;
        }
        let thick = match c % 3 {
            0 => i64::from(self.scan_normal_dimen()?),
            1 => DEFAULT_CODE,
            _ => 0,
        };
        if have {
            return self.log("error", "Ambiguous; you need another { and }");
        }
        let (items, _) = self.list_items(md.lid)?;
        let n = i64::try_from(items.len()).unwrap_or(0);
        let inc = v.list(vec![v.int(thick), v.int(l), v.int(r), v.int(n)]);
        // (a change a rewind of the input does not undo, as any of the
        // run's own state, [`Run::mutations`])
        self.muts += 1;
        self.incs.push((md.lid, inc));
        Ok(())
    }

    /// `fin_mlist` (§1184–§1185): the list `lid`, with the incompleat
    /// noad and `p` (a right delimiter) in place.
    pub(crate) fn fin_mlist(&mut self, lid: i64, p: Option<ValueId>) -> R<Vec<ValueId>> {
        let v = self.v;
        let (items, _) = self.list_items(lid)?;
        // (the incompleat noad is consumed, §1185)
        let inc = self
            .incs
            .iter()
            .position(|x| x.0 == lid)
            .map(|i| self.incs.swap_remove(i).1);
        if inc.is_some() {
            self.muts += 1;
        }
        let Some(inc) = inc else {
            let mut out = items;
            out.extend(p);
            return Ok(out);
        };
        let g = |i: usize| crate::val::int_at(v, inc, i);
        let n = usize::try_from(g(3)).unwrap_or(0).min(items.len());
        let (mut num, den) = (items[..n].to_vec(), items[n..].to_vec());
        let mut out = Vec::new();
        if p.is_some() && num.first().is_some_and(|&x| nodes::tag(v, x) == T_LEFT) {
            out.push(num.remove(0));
        }
        out.push(v.list(vec![
            v.int(T_FRACTION),
            v.int(g(0)),
            v.list(num),
            v.list(den),
            v.int(g(1)),
            v.int(g(2)),
        ]));
        out.extend(p);
        Ok(out)
    }

    /// `math_left_right` (§1191).
    fn math_left_right(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let t = self.cur.chr;
        if t != LEFT_NOAD && md.group != group::MATH_LEFT {
            if md.group == group::MATH_SHIFT {
                self.scan_delimiter(false)?;
                return self.log("error", "Extra \\right");
            }
            return self.off_save(md);
        }
        let d = self.scan_delimiter(false)?;
        if t == LEFT_NOAD {
            let sid = self.push_math(md, group::MATH_LEFT, -203, vec![v.int(md.lid)]);
            self.append(sid, v.list(vec![v.int(T_LEFT), v.int(d)]));
        } else {
            let right = v.list(vec![v.int(T_RIGHT), v.int(d)]);
            let list = self.fin_mlist(md.lid, Some(right))?;
            let outer = self.close_group(md)?;
            let e = empty(v);
            let n = noad(v, INNER, 0, 0, sub_mlist(v, list), e, e);
            self.append(outer.lid, n);
        }
        Ok(())
    }

    /// The `}` of a math group (§1186–§1187).
    pub(crate) fn math_group_end(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let code = md.extra.first().map_or(1, |&y| int_of(v, y));
        let outer_lid = md.extra.get(1).map_or(0, |&y| int_of(v, y));
        let list = self.fin_mlist(md.lid, None)?;
        self.close_group(md)?;
        let (outer_items, _) = self.list_items(outer_lid)?;
        let Some(&tail) = outer_items.last() else {
            return Ok(());
        };
        if nodes::tag(v, tail) != T_NOAD {
            return Ok(());
        }
        let k = usize::try_from(code + 3).unwrap_or(4);
        let mut field = sub_mlist(v, list.clone());
        if let [p] = list[..]
            && nodes::tag(v, p) == T_NOAD
        {
            let ps = list_of(v, p);
            let pt = int_of(v, ps[1]);
            if pt == ORD && is_empty_field(v, ps[5]) && is_empty_field(v, ps[6]) {
                field = ps[4];
            } else if pt == ACCENT && code == F_NUCLEUS && int_of(v, list_of(v, tail)[1]) == ORD {
                // §1187: the accent noad replaces the Ord
                self.replace_tail(outer_lid, p);
                return Ok(());
            }
        }
        let mut xs = list_of(v, tail);
        xs[k] = field;
        self.replace_tail(outer_lid, v.list(xs));
        Ok(())
    }

    /// The `}` of `\vcenter` (§1168).
    pub(crate) fn vcenter_end(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let x = |i: usize| md.extra.get(i).map_or(0, |&y| int_of(v, y));
        let spec = if x(0) == 0 {
            Spec::Exactly(i32::try_from(x(1)).unwrap_or(0))
        } else {
            Spec::Additional(i32::try_from(x(1)).unwrap_or(0))
        };
        let (items, _) = self.list_items(md.lid)?;
        let mut side = Side::default();
        let list: Vec<Node> = items
            .iter()
            .filter_map(|&y| nodes::decode(v, y, &mut side))
            .collect();
        let pp = self.pack_params(false)?;
        let p = phitex_typeset::pack::vpack(list, spec, &pp);
        self.close_group(md)?;
        let b = nodes::encode_box(v, &p.node, &side);
        let e = empty(v);
        let n = noad(v, VCENTER, 0, 0, sub_box(v, b), e, e);
        self.append(x(2), n);
        Ok(())
    }

    /// The `}` of a `\mathchoice` part (§1174).
    pub(crate) fn build_choices(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let part = md.extra.first().map_or(0, |&y| int_of(v, y));
        let outer_lid = md.extra.get(1).map_or(0, |&y| int_of(v, y));
        let list = self.fin_mlist(md.lid, None)?;
        let enc = v.list(list);
        if part < 3 {
            let mut extra = md.extra.clone();
            extra[usize::try_from(part).unwrap_or(0) + 2] = enc;
            extra[0] = v.int(part + 1);
            // §1174: unsave, then the next part's group
            self.pop_level();
            self.scan_left_brace()?;
            self.push_list(-203, group::MATH_CHOICE, extra);
            return Ok(());
        }
        self.close_group(md)?;
        let mut xs = vec![v.int(T_CHOICE)];
        xs.extend(md.extra[2..5].iter().copied());
        xs.push(enc);
        self.append(outer_lid, v.list(xs));
        Ok(())
    }

    /// `after_math` (§1194): the `$` that ends a formula.
    pub(crate) fn after_math(&mut self, md: &Mode) -> R<()> {
        let code = md.extra.first().map_or(0, |&y| int_of(self.v, y));
        match code {
            1 => {
                self.check_dollar()?;
                self.finish_display(md, None, false, Side::default())
            }
            2 | 3 => {
                // the end of an equation number: then of the display
                self.check_dollar()?;
                // (the number's marks and whatsits, numbered with the
                // display's: one side table for the box holding both)
                let mut side = Side::default();
                let hl = self.formula_hlist(md, Style::TEXT, false, &mut side)?;
                let view = self.font_view(&hl)?;
                let a = phitex_typeset::pack::hpack(
                    hl,
                    Spec::Additional(0),
                    &view,
                    &PackParams::default(),
                    false,
                )
                .node;
                let dmd = self.close_group(md)?;
                self.finish_display(&dmd, Some(a), code == 3, side)
            }
            _ => self.finish_text_math(md),
        }
    }

    /// §1197: "Display math should end with $$".
    fn check_dollar(&mut self) -> R<()> {
        self.get_x_token();
        self.ck()?;
        if self.cur.cmd != cmd::MATH_SHIFT {
            self.back_cur();
            self.log("error", "Display math should end with $$")?;
        }
        Ok(())
    }

    /// The formula of the math list `md.lid` as an hlist (§1194–§1195).
    fn formula_hlist(
        &mut self,
        md: &Mode,
        style: Style,
        penalties: bool,
        side: &mut Side,
    ) -> R<Vec<Node>> {
        let v = self.v;
        let mut list = self.fin_mlist(md.lid, None)?;
        if self.insufficient_fonts()? {
            self.log("error", "Math formula deleted: Insufficient symbol fonts")?;
            list.clear();
        }
        let mlist = decode_mlist(v, &list, side);
        self.mlist_to_hlist(mlist, style, penalties)
    }

    /// §1196: the formula in text.
    fn finish_text_math(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let surround = self.ipar(Par::mathsurround)?;
        let outer = self.outer_mode(md)?;
        let mut side = Side::default();
        let hl = self.formula_hlist(md, Style::TEXT, outer.m > 0, &mut side)?;
        let outer = self.close_group(md)?;
        let mut out = vec![Node::Math(MathKind::Before, surround)];
        out.extend(hl);
        out.push(Node::Math(MathKind::After, surround));
        for n in &out {
            let x = nodes::encode(v, n, &side);
            self.append(outer.lid, x);
        }
        self.set_space_factor(&outer, 1000);
        Ok(())
    }

    /// §1199–§1205: the display `dmd` (with the equation number `a`, at
    /// the left if `left`) goes to the vertical list; then §1200.
    #[allow(clippy::too_many_lines)]
    fn finish_display(
        &mut self,
        dmd: &Mode,
        a: Option<BoxNode>,
        left: bool,
        mut side: Side,
    ) -> R<()> {
        let v = self.v;
        let x = |i: usize| dmd.extra.get(i).map_or(0, |&y| int_of(v, y));
        let vlid = x(1);
        let danger = self.insufficient_fonts()?;
        let p = self.formula_hlist(dmd, Style::DISPLAY, false, &mut side)?;
        let view = self.font_view(&p)?;
        // (§1199, §1201: hpack, its overfull rule and reports by
        // `\overfullrule`, `\hbadness`, `\hfuzz`)
        let pp = self.pack_params(true)?;
        let natural = phitex_typeset::pack::hpack(p.clone(), Spec::Additional(0), &view, &pp, true);
        let t = natural.adjustments.clone();
        let mut b = natural.node;
        let mut w = b.width;
        let z = self.ipar(Par::displaywidth)?;
        let s = self.ipar(Par::displayindent)?;
        let (mut e, q) = match &a {
            Some(a) if !danger => {
                let f2 = self.fam_font(0, 2)?;
                let mq = self.font_dimen(f2, 6)?;
                (a.width, a.width + mq)
            }
            _ => (0, 0),
        };
        if w + q > z {
            // §1201
            let mut shrink = [0i64; 4];
            for n in &b.list {
                if let Node::Glue(g) = n {
                    shrink[g.spec.shrink_order as usize] += i64::from(g.spec.shrink);
                }
            }
            let list = std::mem::take(&mut b.list);
            if e != 0
                && (i64::from(w) - shrink[0] + i64::from(q) <= i64::from(z)
                    || shrink[1..].iter().any(|&x| x != 0))
            {
                b = phitex_typeset::pack::hpack(list, Spec::Exactly(z - q), &view, &pp, false).node;
            } else {
                e = 0;
                b = if w > z {
                    phitex_typeset::pack::hpack(list, Spec::Exactly(z), &view, &pp, false).node
                } else {
                    BoxNode { list, ..b }
                };
            }
            w = b.width;
        }
        // §1202
        let mut d = half(z - w);
        if e > 0 && d < 2 * e {
            d = half(z - w - e);
            if matches!(b.list.first(), Some(Node::Glue(_))) {
                d = 0;
            }
        }
        // §1203
        let vm = self.display_outer(dmd);
        let pen = |r: &mut Self, n: i32| {
            let y = nodes::penalty_node(r.v, n);
            r.append(vlid, y);
        };
        let pre = self.ipar(Par::predisplaypenalty)?;
        pen(self, pre);
        let presize = self.ipar(Par::predisplaysize)?;
        let (g1, mut g2) = if d + s <= presize || left {
            (3u8, 4u8)
        } else {
            (5, 6)
        };
        let skip = |r: &mut Self, c: u8| -> R<()> {
            let name = match c {
                3 => "abovedisplayskip",
                4 => "belowdisplayskip",
                5 => "abovedisplayshortskip",
                _ => "belowdisplayshortskip",
            };
            let g = r.glue_param(name)?;
            let y = nodes::encode(r.v, &Node::param_glue(c, g), &Side::default());
            r.append(vlid, y);
            Ok(())
        };
        let mut a = a;
        if left && e == 0 {
            if let Some(mut ab) = a.take() {
                ab.shift = s;
                let y = nodes::encode_box(v, &ab, &side);
                self.append_to_vlist(&vm, y)?;
            }
            pen(self, INF_PENALTY);
        } else {
            skip(self, g1)?;
        }
        // §1204
        if e != 0 {
            let r = Node::Kern(z - w - e - d, KernKind::Normal);
            let ab = a.take().map(|a| Node::Box(Box::new(a)));
            let list: Vec<Node> = if left {
                d = 0;
                ab.into_iter().chain([r, Node::Box(Box::new(b))]).collect()
            } else {
                std::iter::once(Node::Box(Box::new(b))).chain([r]).chain(ab).collect()
            };
            let view2 = self.font_view(&list)?;
            b = phitex_typeset::pack::hpack(list, Spec::Additional(0), &view2, &pp, false).node;
        }
        b.shift = s + d;
        let y = nodes::encode_box(v, &b, &side);
        self.append_to_vlist(&vm, y)?;
        // §1205
        if let Some(mut ab) = a.take().filter(|_| e == 0 && !left) {
            pen(self, INF_PENALTY);
            ab.shift = s + z - ab.width;
            let y = nodes::encode_box(v, &ab, &side);
            self.append_to_vlist(&vm, y)?;
            g2 = 0;
        }
        for n in &t {
            let y = nodes::encode(v, n, &side);
            self.append(vlid, y);
        }
        let post = self.ipar(Par::postdisplaypenalty)?;
        pen(self, post);
        if g2 > 0 {
            skip(self, g2)?;
        }
        self.resume_after_display(dmd, &vm)
    }

    /// The vertical list around the display `dmd` (its level's extra
    /// `[1, list, mode]`), as a mode.
    pub(crate) fn display_outer(&self, dmd: &Mode) -> Mode {
        let x = |i: usize| dmd.extra.get(i).map_or(0, |&y| int_of(self.v, y));
        let vlid = x(1);
        let vkm = i32::try_from(x(2)).unwrap_or(1);
        let mut vm = dmd.clone();
        vm.m = vkm;
        vm.km = vkm;
        vm.lid = vlid;
        vm.vlid = vlid;
        vm
    }

    /// `resume_after_display` (§1200): the display's group ends, the
    /// paragraph goes on in a new list.
    pub(crate) fn resume_after_display(&mut self, dmd: &Mode, vm: &Mode) -> R<()> {
        self.close_group(dmd)?;
        let pg = self.prev_graf(vm)?;
        self.set_prev_graf(vm, pg + 3);
        let lang = i64::from(phitex_typeset::hyph::cur_lang(self.ipar(Par::language)?));
        let lhm = Self::norm_min(self.ipar(Par::lefthyphenmin)?);
        let rhm = Self::norm_min(self.ipar(Par::righthyphenmin)?);
        let packed = (lhm * 64 + rhm) * 65536 + lang;
        // §1200: a new paragraph (push_nest, mode hmode)
        self.push_list(102, 0, Vec::new());
        if let Some(l) = self.top_mut() {
            l.par = Some((lang, packed));
        }
        // §443: an optional space
        self.get_x_token();
        self.ck()?;
        if self.cur.cmd != cmd::SPACER {
            self.back_cur();
        }
        // §1200: `if nest_ptr=1 then build_page`
        self.finish_candidate()?;
        Ok(())
    }

    /// §1195: whether families 2 and 3 lack their parameters.
    fn insufficient_fonts(&mut self) -> R<bool> {
        for size in [0, 16, 32] {
            let f2 = self.fam_font(size, 2)?;
            let f3 = self.fam_font(size, 3)?;
            if i64::from(self.font_int(f2, crate::fields::FontCell::Fdn)?) < 22
                || i64::from(self.font_int(f3, crate::fields::FontCell::Fdn)?) < 13
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `mlist_to_hlist` with the families and parameters of eqtb.
    pub(crate) fn mlist_to_hlist(
        &mut self,
        mlist: MList,
        style: Style,
        penalties: bool,
    ) -> R<Vec<Node>> {
        let mut fam_fnt = [[None; 16]; 3];
        for (s, row) in fam_fnt.iter_mut().enumerate() {
            for (k, slot) in row.iter_mut().enumerate() {
                let size = i32::try_from(s * 16).unwrap_or(0);
                let k32 = i32::try_from(k).unwrap_or(0);
                let f = u32::try_from(self.fam_font(size, k32)?).unwrap_or(0);
                if f != 0 {
                    *slot = Some(f);
                }
            }
        }
        // (XeTeX's families past sixteen, up to the highest ever set)
        let mut xfams: Vec<[Option<u32>; 3]> = Vec::new();
        if self.xe {
            let v = self.v;
            let top = self.get("famxmax")?.map_or(15, |x| int_of(v, x));
            for k in 16..=i32::try_from(top).unwrap_or(15) {
                let mut row = [None; 3];
                for (s, slot) in row.iter_mut().enumerate() {
                    let size = i32::try_from(s * 16).unwrap_or(0);
                    let f = u32::try_from(self.fam_font(size, k)?).unwrap_or(0);
                    if f != 0 {
                        *slot = Some(f);
                    }
                }
                xfams.push(row);
            }
        }
        // (the last formula's fonts while no font cell changed since)
        let g = self.st.font_gen();
        let fonts = match &self.math_memo {
            Some(m) if m.0 == g && m.1 == fam_fnt && m.2.0.xfams == xfams && self.interp.is_none() => m.2.clone(),
            _ => {
                let f = Rc::new(self.math_fonts(&fam_fnt, &xfams)?);
                if self.interp.is_none() {
                    self.math_memo = Some((self.st.font_gen(), fam_fnt, f.clone()));
                }
                f
            }
        };
        // (§715 `rebox`: a box's characters packed again, their fonts'
        // dimensions with the families')
        let mut more = Vec::new();
        box_fonts(&mlist, &mut more);
        more.retain(|&f| !fonts.0.fonts.contains_key(f));
        more.sort_unstable();
        more.dedup();
        let fonts = if more.is_empty() {
            fonts
        } else {
            let mut f2 = (fonts.0.fonts_copy(), fonts.1.clone());
            for f in more {
                self.add_font(&mut f2.0, f)?;
            }
            Rc::new(f2)
        };
        let (view, skew) = (&fonts.0, &fonts.1);
        let sk = move |f: u32| skew.get(&f).copied().unwrap_or(-1);
        let params = MathParams {
            fam_fnt,
            thin_mu_skip: self.gpar(Par::thinmuskip)?,
            med_mu_skip: self.gpar(Par::medmuskip)?,
            thick_mu_skip: self.gpar(Par::thickmuskip)?,
            script_space: self.ipar(Par::scriptspace)?,
            null_delimiter_space: self.ipar(Par::nulldelimiterspace)?,
            delimiter_factor: self.ipar(Par::delimiterfactor)?,
            delimiter_shortfall: self.ipar(Par::delimitershortfall)?,
            bin_op_penalty: self.ipar(Par::binoppenalty)?,
            rel_penalty: self.ipar(Par::relpenalty)?,
            skew_char: &sk,
        };
        // (XeTeX's `cur_f` as the last formula left it)
        let curf = if self.xe {
            let v = self.v;
            let x = self.get("mathcurf")?.map_or(-1, |x| int_of(v, x));
            view.math_cur_f.store(x, std::sync::atomic::Ordering::Relaxed);
            Some(x)
        } else {
            None
        };
        let (h, _diags) =
            phitex_typeset::math::mlist_to_hlist(mlist, style, penalties, &params, view);
        if let Some(x) = curf {
            let y = view.math_cur_f.load(std::sync::atomic::Ordering::Relaxed);
            if y != x {
                let v = self.v;
                self.set("mathcurf", v.int(y), true);
            }
        }
        Ok(h)
    }

    /// The families' fonts `fam_fnt` decoded for a formula, with their
    /// `\skewchar`s.
    fn math_fonts(
        &mut self,
        fam_fnt: &[[Option<u32>; 16]; 3],
        xfams: &[[Option<u32>; 3]],
    ) -> R<(crate::fonts::View, crate::FxHashMap<u32, i32>)> {
        let mut view = crate::fonts::View::default();
        let mut skew = crate::FxHashMap::default();
        // (XeTeX: the native fonts with their `\fontdimen`s, and the
        // families past sixteen)
        if self.xe {
            view.xfams = xfams.to_vec();
            let all: Vec<u32> = fam_fnt
                .iter()
                .flatten()
                .flatten()
                .chain(xfams.iter().flatten().flatten())
                .copied()
                .collect();
            for f in all {
                if view.natives.contains_key(&f) || view.fonts.contains_key(f) {
                    continue;
                }
                if let Some(nf) = self.native_font(i64::from(f))? {
                    let n = self.font_int(f, crate::fields::FontCell::Fdn)?;
                    let mut params = Vec::new();
                    for k in 1..=n {
                        params.push(self.font_int(f, crate::fields::FontCell::Fd(k))?);
                    }
                    view.native_params.insert(f, params);
                    view.natives.insert(f, nf);
                } else {
                    self.add_font(&mut view, f)?;
                }
                skew.insert(f, self.font_int(f, crate::fields::FontCell::Sc)?);
            }
        }
        // (the symbol and extension families' fonts with all their
        // `\fontdimen`s, §700–§701; another family's with those math
        // reads of any font, the space and the x-height, §738, §752)
        let full: Vec<u32> = fam_fnt.iter().flat_map(|row| [row[2], row[3]]).flatten().collect();
        for row in fam_fnt {
            for &f in row.iter().flatten() {
                if view.fonts.contains_key(f) {
                    continue;
                }
                if full.contains(&f) {
                    self.add_font(&mut view, f)?;
                } else {
                    self.add_font_dimens(&mut view, f, &[2, 5])?;
                }
                skew.insert(f, self.font_int(f, crate::fields::FontCell::Sc)?);
            }
        }
        Ok((view, skew))
    }
}

/// The fonts of the characters of the boxes in `ml`'s fields, their own
/// lists only (a box inside them has its dimensions).
fn box_fonts(ml: &[phitex_typeset::math::MItem], out: &mut Vec<u32>) {
    use phitex_typeset::math::{Field, MItem};
    fn field(x: &Field, out: &mut Vec<u32>) {
        match x {
            Field::SubBox(b) => {
                for n in &b.list {
                    match n {
                        Node::Char { font, .. } => out.push(*font),
                        Node::Lig(l) => out.push(l.font),
                        _ => {}
                    }
                }
            }
            Field::SubMlist(m) => box_fonts(m, out),
            _ => {}
        }
    }
    for it in ml {
        match it {
            MItem::Noad(n) => {
                field(&n.nucleus, out);
                field(&n.supscr, out);
                field(&n.subscr, out);
            }
            MItem::Fraction(f) => {
                box_fonts(&f.numerator, out);
                box_fonts(&f.denominator, out);
            }
            MItem::Choice(c) => c.iter().for_each(|m| box_fonts(m, out)),
            _ => {}
        }
    }
}

/// [`Run::math_memo`]: the fonts' generation, the families' fonts, and
/// those fonts decoded with their `\skewchar`s.
pub(crate) type MathMemo = (u64, [[Option<u32>; 16]; 3], Rc<(crate::fonts::View, crate::FxHashMap<u32, i32>)>);
