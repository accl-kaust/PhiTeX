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

use phitex_defuse::{Effect, ValueId, Values};
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
    let g = |s: u32, m: i64| u16::try_from((d >> s) & m).unwrap_or(0);
    Delimiter {
        small_fam: u8::try_from(g(20, 15)).unwrap_or(0),
        small_char: g(12, 255),
        large_fam: u8::try_from(g(8, 15)).unwrap_or(0),
        large_char: g(0, 255),
    }
}

fn decode_field(v: &Values, f: ValueId, side: &mut Side) -> Field {
    let xs = list_of(v, f);
    let n = |i: usize| xs.get(i).map_or(0, |&y| int_of(v, y));
    match n(0) {
        1 => Field::MathChar {
            fam: u8::try_from(n(1)).unwrap_or(0),
            ch: u16::try_from(n(2)).unwrap_or(0),
        },
        3 => match xs.get(1).and_then(|&b| nodes::decode(v, b, side)) {
            Some(Node::Box(b)) => Field::SubBox(b),
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
                    28 => NoadKind::Accent(
                        u8::try_from(n(2)).unwrap_or(0),
                        u16::try_from(n(3)).unwrap_or(0),
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

    /// `init_math` (§1138–§1139, §1145).
    pub(crate) fn init_math(&mut self, md: &Mode) -> R<()> {
        self.get_token()?;
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
        self.push_math(md, group::MATH_SHIFT, -203, vec![v.int(code)])?;
        self.set("param:fam", v.int(-1), false);
        let e = self.get("param:everymath")?;
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
        let w = if items.is_empty() {
            -MAX_DIMEN
        } else {
            let last = self.line_break(md, &items, true)?;
            self.pre_display_size(last.as_ref())?
        };
        self.set(&self.par_cell(md.vlid), ValueId::UNIT, true);
        self.region_advance();
        // §1149
        let pg = self.int32(&format!("pg:{}", md.vlid))?;
        let shape: Vec<i32> = self
            .get("parshape")?
            .map(|x| list_of(v, x))
            .unwrap_or_default()
            .iter()
            .map(|&x| i32::try_from(int_of(v, x)).unwrap_or(0))
            .collect();
        let (l, s) = if shape.len() < 2 {
            let hi = self.param("hangindent")?;
            let ha = self.param("hangafter")?;
            let hsize = self.param("hsize")?;
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
        self.push_math(md, group::MATH_SHIFT, 203, extra)?;
        self.set("param:fam", v.int(-1), false);
        self.set("param:predisplaysize", v.int(i64::from(w)), false);
        self.set("param:displaywidth", v.int(i64::from(l)), false);
        self.set("param:displayindent", v.int(i64::from(s)), false);
        let e = self.get("param:everydisplay")?;
        let toks = crate::val::toks_of(v, e);
        self.push_toks(toks);
        let mut outer = md.clone();
        outer.m = vm;
        outer.km = vm;
        outer.lid = md.vlid;
        self.build_page_mark(&outer)
    }

    /// §1146: the width of the paragraph's last line up to its last
    /// visible item, plus two quads; `max_dimen` if glue stretched or
    /// shrank before it.
    fn pre_display_size(&mut self, last: Option<&BoxNode>) -> R<i32> {
        let Some(b) = last else { return Ok(-MAX_DIMEN) };
        let f = self.int("font")?;
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
    fn push_math(&mut self, md: &Mode, grp: i64, m: i32, extra: Vec<ValueId>) -> R<i64> {
        let sid = self.node_raw();
        let kind = self.kind_value(md, grp, sid, sid, m, extra)?;
        self.open_scope_here(kind);
        Ok(sid)
    }

    fn mathcode(&mut self, c: i32) -> R<i32> {
        self.int32(&format!("mc:{c}"))
    }

    fn fam_in_range(&mut self) -> R<Option<i32>> {
        let f = self.param("fam")?;
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
                cmd::MATH_CHAR_NUM => self.scan_fifteen_bit_int()?,
                cmd::MATH_GIVEN => self.cur.chr,
                cmd::DELIM_NUM => self.scan_twenty_seven_bit_int()? / 0x1000,
                _ => {
                    self.back_cur();
                    self.scan_left_brace()?;
                    let v = self.v;
                    self.push_math(md, group::MATH, -203, vec![v.int(code), v.int(md.lid)])?;
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
        let mut val = if radical {
            self.scan_twenty_seven_bit_int()?
        } else {
            self.skip_blanks_relax_x()?;
            match self.cur.cmd {
                cmd::LETTER | cmd::OTHER_CHAR => self.int32(&format!("dc:{}", self.cur.chr))?,
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
            cmd::MATH_CHAR_NUM => {
                let c = self.scan_fifteen_bit_int()?;
                self.set_math_char(md, c, 0)?;
            }
            cmd::MATH_GIVEN => self.set_math_char(md, self.cur.chr, 0)?,
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
                let sid = self.node_raw();
                let extra = vec![v.int(spec.0), v.int(i64::from(spec.1)), v.int(md.lid)];
                let kind = self.kind_value(md, group::VCENTER, sid, sid, -1, extra)?;
                self.open_scope_here(kind);
                self.normal_paragraph()?;
                let e = self.get("param:everyvbox")?;
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
                )?;
            }
            cmd::SUB_MARK | cmd::SUP_MARK => self.sub_sup(md)?,
            cmd::ABOVE => self.math_fraction(md)?,
            cmd::LEFT_RIGHT => self.math_left_right(md)?,
            cmd::MATH_SHIFT => {
                if md.group == group::MATH_SHIFT {
                    self.after_math(md)?;
                } else {
                    // off_save (§1064)
                    self.log("error", "Extra $, or forgotten }")?;
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
                let f = u32::try_from(self.int("font")?).unwrap_or(0);
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
                    let w = self.int32("param:parindent")?;
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
        let name = format!("inc:{}", md.lid);
        let have = self.get(&name)?.is_some_and(|x| x != ValueId::UNIT);
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
        self.set(&name, inc, true);
        Ok(())
    }

    /// `fin_mlist` (§1184–§1185): the list `lid`, with the incompleat
    /// noad and `p` (a right delimiter) in place.
    pub(crate) fn fin_mlist(&mut self, lid: i64, p: Option<ValueId>) -> R<Vec<ValueId>> {
        let v = self.v;
        let (items, _) = self.list_items(lid)?;
        let inc = self
            .get(&format!("inc:{lid}"))?
            .filter(|&x| x != ValueId::UNIT);
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
            // off_save (§1064)
            return self.log("error", "Extra \\right");
        }
        let d = self.scan_delimiter(false)?;
        if t == LEFT_NOAD {
            let sid = self.push_math(md, group::MATH_LEFT, -203, vec![v.int(md.lid)])?;
            self.append(sid, v.list(vec![v.int(T_LEFT), v.int(d)]));
        } else {
            let right = v.list(vec![v.int(T_RIGHT), v.int(d)]);
            let list = self.fin_mlist(md.lid, Some(right))?;
            self.effect = Effect::Close;
            self.unsave_extras(md)?;
            let outer = self.outer_mode(md)?;
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
        self.effect = Effect::Close;
        self.unsave_extras(md)?;
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
        let (items, _) = self.list_items(md.sid)?;
        let mut side = Side::default();
        let list: Vec<Node> = items
            .iter()
            .filter_map(|&y| nodes::decode(v, y, &mut side))
            .collect();
        let pp = self.pack_params(false)?;
        let p = phitex_typeset::pack::vpack(list, spec, &pp);
        self.effect = Effect::Close;
        self.unsave_extras(md)?;
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
        self.unsave_extras(md)?;
        if part < 3 {
            let mut extra = md.extra.clone();
            extra[usize::try_from(part).unwrap_or(0) + 2] = enc;
            extra[0] = v.int(part + 1);
            self.scan_left_brace()?;
            let sid = self.node_raw();
            let mut outer = md.clone();
            outer.depth -= 1;
            let cur = self.scope_kind()?.unwrap_or(ValueId::UNIT);
            let parent = list_of(v, cur).get(5).copied().unwrap_or(ValueId::UNIT);
            let kind =
                self.kind_with_parent(&outer, group::MATH_CHOICE, sid, sid, -203, extra, parent);
            self.effect = Effect::Scope(phitex_defuse::Brackets {
                closes: 1,
                opens: 1,
                before: false,
                kind: Some(kind),
                kind_pending: false,
            });
            return Ok(());
        }
        self.effect = Effect::Close;
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
                self.finish_display(md, None, false, 1)
            }
            2 | 3 => {
                // the end of an equation number: then of the display
                self.check_dollar()?;
                let hl = self.formula_hlist(md, Style::TEXT, false)?;
                let view = self.font_view(&hl)?;
                let a = phitex_typeset::pack::hpack(
                    hl,
                    Spec::Additional(0),
                    &view,
                    &PackParams::default(),
                    false,
                )
                .node;
                self.unsave_extras(md)?;
                let dmd = self.outer_mode(md)?;
                self.finish_display(&dmd, Some(a), code == 3, 2)
            }
            _ => self.finish_text_math(md),
        }
    }

    /// §1197: "Display math should end with $$".
    fn check_dollar(&mut self) -> R<()> {
        self.get_x_token()?;
        if self.cur.cmd != cmd::MATH_SHIFT {
            self.back_cur();
            self.log("error", "Display math should end with $$")?;
        }
        Ok(())
    }

    /// The formula of the math list `md.lid` as an hlist (§1194–§1195).
    fn formula_hlist(&mut self, md: &Mode, style: Style, penalties: bool) -> R<Vec<Node>> {
        let v = self.v;
        let mut list = self.fin_mlist(md.lid, None)?;
        if self.insufficient_fonts()? {
            self.log("error", "Math formula deleted: Insufficient symbol fonts")?;
            list.clear();
        }
        let mut side = Side::default();
        let mlist = decode_mlist(v, &list, &mut side);
        self.mlist_to_hlist(mlist, style, penalties)
    }

    /// §1196: the formula in text.
    fn finish_text_math(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let surround = self.param("mathsurround")?;
        let outer = self.outer_mode(md)?;
        let hl = self.formula_hlist(md, Style::TEXT, outer.m > 0)?;
        self.effect = Effect::Close;
        self.unsave_extras(md)?;
        let side = Side::default();
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
    fn finish_display(&mut self, dmd: &Mode, a: Option<BoxNode>, left: bool, closes: u8) -> R<()> {
        let v = self.v;
        let x = |i: usize| dmd.extra.get(i).map_or(0, |&y| int_of(v, y));
        let vlid = x(1);
        let vkm = i32::try_from(x(2)).unwrap_or(1);
        let danger = self.insufficient_fonts()?;
        let p = self.formula_hlist(dmd, Style::DISPLAY, false)?;
        let view = self.font_view(&p)?;
        let pp = PackParams::default();
        let natural = phitex_typeset::pack::hpack(p.clone(), Spec::Additional(0), &view, &pp, true);
        let t = natural.adjustments.clone();
        let mut b = natural.node;
        let mut w = b.width;
        let z = self.param("displaywidth")?;
        let s = self.param("displayindent")?;
        let (mut e, q) = match &a {
            Some(a) if !danger => {
                let f2 = self.int("fam:0:2")?;
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
        let mut vm = dmd.clone();
        vm.m = vkm;
        vm.km = vkm;
        vm.lid = vlid;
        vm.vlid = vlid;
        let side = Side::default();
        let pen = |r: &mut Self, n: i32| {
            let y = nodes::penalty_node(r.v, n);
            r.append(vlid, y);
        };
        let pre = self.param("predisplaypenalty")?;
        pen(self, pre);
        let presize = self.param("predisplaysize")?;
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
            let ab = a.take().map(Node::Box);
            let list: Vec<Node> = if left {
                d = 0;
                ab.into_iter().chain([r, Node::Box(b)]).collect()
            } else {
                std::iter::once(Node::Box(b)).chain([r]).chain(ab).collect()
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
        let post = self.param("postdisplaypenalty")?;
        pen(self, post);
        if g2 > 0 {
            skip(self, g2)?;
        }
        // §1200 resume_after_display
        self.effect = if closes == 1 {
            Effect::Close
        } else {
            Effect::Scope(phitex_defuse::Brackets {
                closes,
                opens: 0,
                before: false,
                kind: None,
                kind_pending: false,
            })
        };
        self.unsave_extras(dmd)?;
        let pg = self.int(&format!("pg:{vlid}"))?;
        self.set(&format!("pg:{vlid}"), v.int(pg + 3), true);
        let id = self.node_raw();
        let lang = i64::from(phitex_typeset::hyph::cur_lang(self.param("language")?));
        let lhm = Self::norm_min(self.param("lefthyphenmin")?);
        let rhm = Self::norm_min(self.param("righthyphenmin")?);
        let packed = (lhm * 64 + rhm) * 65536 + lang;
        let rec = v.list(vec![v.int(id), v.int(lang), v.int(packed)]);
        self.set(&self.par_cell(vlid), rec, true);
        // §443: an optional space
        self.get_x_token()?;
        if self.cur.cmd != cmd::SPACER {
            self.back_cur();
        }
        self.build_page_mark(&vm)
    }

    /// §1195: whether families 2 and 3 lack their parameters.
    fn insufficient_fonts(&mut self) -> R<bool> {
        for size in [0, 16, 32] {
            let f2 = self.int(&format!("fam:{size}:2"))?;
            let f3 = self.int(&format!("fam:{size}:3"))?;
            if self.int(&format!("fdn:{f2}"))? < 22 || self.int(&format!("fdn:{f3}"))? < 13 {
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
        let mut view = crate::fonts::View::default();
        let mut skew = crate::FxHashMap::default();
        for (s, row) in fam_fnt.iter_mut().enumerate() {
            for (k, slot) in row.iter_mut().enumerate() {
                let f = u32::try_from(self.int(&format!("fam:{}:{k}", s * 16))?).unwrap_or(0);
                if f != 0 {
                    *slot = Some(f);
                    self.add_font(&mut view, f)?;
                    skew.insert(f, self.int32(&format!("sc:{f}"))?);
                }
            }
        }
        let sk = move |f: u32| skew.get(&f).copied().unwrap_or(-1);
        let params = MathParams {
            fam_fnt,
            thin_mu_skip: self.glue_param("thinmuskip")?,
            med_mu_skip: self.glue_param("medmuskip")?,
            thick_mu_skip: self.glue_param("thickmuskip")?,
            script_space: self.param("scriptspace")?,
            null_delimiter_space: self.param("nulldelimiterspace")?,
            delimiter_factor: self.param("delimiterfactor")?,
            delimiter_shortfall: self.param("delimitershortfall")?,
            bin_op_penalty: self.param("binoppenalty")?,
            rel_penalty: self.param("relpenalty")?,
            skew_char: &sk,
        };
        let (h, _diags) =
            phitex_typeset::math::mlist_to_hlist(mlist, style, penalties, &params, &view);
        Ok(h)
    }
}
