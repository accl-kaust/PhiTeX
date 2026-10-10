//! Math mode (tex.web §1136–§1206), copied: commands as nodes that emit
//! the entries of a math list (`mlist::MItem`), the way boxes are built. A
//! formula, a subformula `{…}`, `\left…\right` and each `\mathchoice` part
//! is a group (`GroupKind::Math`) whose `Close` node defines and emits
//! nothing: it reads what TeX reads before the group's end (`MathEnd`:
//! `\mathsurround`, the families' fonts, the spacing parameters; for a
//! display its width, indent and skips) and hands it to the packaging node
//! after it, which gathers the list from the items the nodes inside
//! emitted (`mlist::gather`, tex.web's `fin_mlist`) and converts it
//! (`mlist::mlist_to_hlist`).
//!
//! Where TeX opens a group after appending a noad to the enclosing list
//! (`^{`, `\mathop{`, `{` in math), the node appends the noad and hands the
//! group to the next node (`\0mathgroup`), which opens it: a node's items
//! are in the list of the level it ends at. A display is three nodes: the
//! `$$` node ends the paragraph so far (§1145: its lines are broken here
//! too, for `\predisplaysize` and `\prevgraf`), `\0display` opens the
//! group (§1149: `\displaywidth`, `\displayindent`), `\0mathinit` defines
//! the parameters inside it.

use std::sync::Arc;

use super::material::{BoxContext, BoxEnd, BoxGroup, BoxKind, box_start, para_start, push_horizontal};
use super::{Exec, Group, GroupKind, Kind, Mode};
use crate::item::{BoxNode, Interline, Item, PageCall, Para, param_glue};
use crate::linebreak;
use crate::mlist::{self, Delim, Field, Kind as NK, Limits, MItem, MathEnv, Noad, Which};
use crate::pack::{self, Params, Spec, hpack};
use crate::show::report;
use crate::tfm::Font;
use crate::val::{DimenPar, Glue, GluePar, IntPar, Key, Mat, MathP, Meaning, Prim, Tok, TokPar, Val, Xe};

/// `max_dimen`.
const MAX_DIMEN: i32 = 0o7_777_777_777;

/// Where a subformula's list goes: a field of the last noad of the
/// enclosing list, and whether that noad is an ord noad (§1186).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Target {
    which: Which,
    ord: bool,
}

/// What a math group is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MathKind {
    /// `$`…`$`.
    Inline,
    /// `$$`…`$$`: `\predisplaysize`, `\displaywidth`, `\displayindent` as
    /// the group began (§1146, §1149), and the equation number once made.
    Display { w: i32, l: i32, s: i32, eqno: Option<(Arc<BoxNode>, bool)> },
    /// What the `$$` node hands the next node: `\predisplaysize`.
    DisplayInit { w: i32 },
    /// A display whose alignment is set (§1206): its material, and the
    /// depth of its list (`aux_save`); only assignments and `$$` may
    /// follow.
    DisplayAlign { rows: Arc<Vec<Item>>, aux: i32 },
    /// `\eqno`, `\leqno`.
    EqNo { left: bool },
    /// `{`…`}` in math.
    Sub(Target),
    /// `\left`…`\right`; after e-TeX's `\middle` (`middle`), the
    /// group it reopened.
    Left { middle: bool },
    /// A `\mathchoice` part and the parts before it.
    Choice(u8, Arc<Vec<Vec<Item>>>),
}

/// What a display reads before its group ends (§1199–§1205).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DisplayEnd {
    z: i32,
    s: i32,
    pre_size: i32,
    skips: [Arc<Glue>; 4],
    pre_penalty: i32,
    post_penalty: i32,
    inter: Arc<Interline>,
    pack: Params,
}

/// What a math group's `Close` node read inside the group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MathEnd {
    params: mlist::Params,
    fonts: mlist::Fonts,
    surround: i32,
    right: Option<Delim>,
    display: Option<DisplayEnd>,
    /// e-TeX's `\middle`: the right delimiter is a middle one, and the
    /// group opens again with the list so far (`carry`, etex.web §1191).
    middle: bool,
    carry: Vec<Item>,
    /// §1195: the formula was deleted (not enough symbol or extension font
    /// parameters).
    danger: bool,
}

/// An ord noad around a box (§1076).
pub(super) fn box_noad(b: Arc<BoxNode>) -> Item {
    let mut n = Noad::new(NK::Ord);
    n.nucleus = Field::Box(b);
    mlist::item(MItem::Noad(n))
}

/// The formula's lazy reads: `\fontdimen`s and `\skewchar`s.
struct Reads<'e, 'a>(&'e mut Exec<'a>);

impl MathEnv for Reads<'_, '_> {
    fn param(&mut self, f: &Arc<Font>, k: u16) -> i32 {
        self.0.int(&Key::FontDimen(f.key.clone(), u32::from(k)))
    }
    fn skew_char(&mut self, f: &Arc<Font>) -> i32 {
        self.0.int(&Key::SkewChar(f.key.clone()))
    }
    fn native_char_glyph(&mut self, f: &Arc<Font>, c: u32) -> u16 {
        // (`new_native_character`: the font's mapping, its direction as
        // the main loop defined it, a lost character warned of)
        let w = self.0.native_character(f, c);
        w.glyphs.first().map_or(0, |g| g.gid)
    }
}

/// A TeX math code (15 bits, "8000 active) in XeTeX's form.
fn xe_form(c: i32) -> i32 {
    if c >= 0x8000 {
        return 0x1F_FFFF;
    }
    ((((c % 0x1000) / 0x100) as u32) << 24) as i32 + ((c / 0x1000) << 21) + (c % 0x100)
}

impl Exec<'_> {
    // ------------------------------------------------------------ dispatch

    /// A command in math mode that is math's own (tex.web 1045's `mmode+`
    /// cases); `false` for the commands every mode shares.
    pub(super) fn math_command(&mut self, t: &Tok, m: &Meaning) -> bool {
        if self.math_kind().is_some_and(|k| matches!(*k, MathKind::DisplayAlign { .. })) {
            return self.after_display_alignment(t, m);
        }
        match m {
            Meaning::Char(11 | 12, c) => {
                let v = self.math_code_x(*c);
                self.set_math_char(v, *c);
            }
            Meaning::CharGiven(c) => {
                let ch = char::from_u32(*c).unwrap_or('\0');
                let v = self.math_code_x(ch);
                self.set_math_char(v, ch);
            }
            Meaning::MathCharGiven(v) => self.set_math_char(xe_form(*v as i32), '\0'),
            Meaning::XMathCharGiven(v) => self.set_math_char(*v as i32, '\0'),
            Meaning::Prim(Prim::Char) => {
                let c = self.scan_char_num();
                let ch = char::from_u32(c as u32).unwrap_or('\0');
                let v = self.math_code_x(ch);
                self.set_math_char(v, ch);
            }
            Meaning::Prim(Prim::Xe(Xe::MathCharNum(k))) => {
                // (`\Umathcharnum`, `\Umathchar`)
                let v = self.scan_xe_math_char(*k);
                self.set_math_char(v, '\0');
            }
            Meaning::Prim(Prim::Xe(Xe::Delimiter)) => {
                // (`\Udelimiter class fam usv` as a character)
                let v = self.scan_xe_math_char(2);
                self.set_math_char(v, '\0');
            }
            Meaning::Prim(Prim::Xe(Xe::Radical)) => {
                // (xetex.web: `\Uradical fam usv`, then the nucleus)
                let fam = self.scan_math_fam_int();
                let u = self.scan_usv_num();
                let d = Delim::from_code(0x4000_0000 + fam * 0x20_0000 + u);
                self.noad_with_nucleus(Noad::new(NK::Radical(d)), false);
            }
            Meaning::Prim(Prim::Xe(Xe::MathAccent)) => {
                // (xetex.web `math_ac`: `\Umathaccent [fixed|bottom [fixed]]
                // class fam usv`)
                let mut sub = 0;
                if self.keyword("fixed") {
                    sub = mlist::FIXED_ACC;
                } else if self.keyword("bottom") {
                    sub = if self.keyword("fixed") { mlist::BOTTOM_ACC + mlist::FIXED_ACC } else { mlist::BOTTOM_ACC };
                }
                let v = self.scan_xe_math_char(2);
                let fam = self.var_fam_x(v);
                self.noad_with_nucleus(Noad::new(NK::Accent { fam, ch: (v & 0x1F_FFFF) as u32, sub }), false);
            }
            Meaning::Char(1, _) => {
                // (§1154: a new ord noad, its nucleus the subformula)
                self.emit.push(mlist::item(MItem::Noad(Noad::new(NK::Ord))));
                self.open_sub_later(Target { which: Which::Nucleus, ord: true }, t.clone());
            }
            Meaning::Prim(Prim::Mat(Mat::ItalCorr)) => self.emit.push(Item::Kern { w: 0, sub: 0 }),
            Meaning::Prim(Prim::Mat(Mat::Unhbox(_))) => {
                // (§1110: a box's list can't go into a math list)
                let n = self.scan_eight_bit_int();
                if !matches!(self.get(&Key::Box(n)), Val::Void) {
                    self.err("Incompatible list can't be unboxed".into());
                }
            }
            Meaning::Prim(Prim::Indent) => {
                // (§1093)
                let w = self.dimen_par(DimenPar::Parindent);
                self.emit.push(box_noad(Arc::new(BoxNode { w, ..BoxNode::default() })));
            }
            Meaning::Prim(Prim::Noindent) => {}
            Meaning::Prim(
                Prim::Par | Prim::End | Prim::Dump | Prim::Mat(Mat::Vskip(_) | Mat::Hrule | Mat::Unvbox(_)),
            ) => self.insert_dollar_sign(t),
            _ => return false,
        }
        true
    }

    /// tex.web 1047: `insert_dollar_sign`.
    pub(super) fn insert_dollar_sign(&mut self, t: &Tok) {
        self.err("Missing $ inserted".into());
        self.rewrite(vec![Tok::Char(3, '$'), t.clone()]);
    }

    /// A math primitive.
    pub(super) fn math_prim(&mut self, x: MathP, t: &Tok) {
        if self.st.mode != Mode::M {
            if x == MathP::EqNo(false) || x == MathP::EqNo(true) {
                self.err(format!("You can't use `{}' in {} mode", self.shown(t), self.mode_name()));
            } else {
                self.insert_dollar_sign(t);
            }
            return;
        }
        match x {
            MathP::MathChar => {
                let v = self.scan_fifteen_bit_int();
                self.set_math_char(xe_form(v), '\0');
            }
            MathP::Delimiter => {
                let v = self.scan_twenty_seven_bit_int();
                self.set_math_char(xe_form(v / 0o10_000), '\0');
            }
            MathP::Comp(c) => {
                // (§1158)
                let k = NK::comp(c);
                self.noad_with_nucleus(Noad::new(k), c == 0);
            }
            MathP::Limits(k) => {
                let l = match k {
                    0 => Limits::Normal,
                    1 => Limits::Limits,
                    _ => Limits::NoLimits,
                };
                self.limit_switch(l);
            }
            MathP::Radical => {
                // (§1163)
                let d = self.scan_delimiter(true);
                self.noad_with_nucleus(Noad::new(NK::Radical(d)), false);
            }
            MathP::Accent => {
                // (§1165)
                let v = xe_form(self.scan_fifteen_bit_int());
                let fam = self.var_fam_x(v);
                self.noad_with_nucleus(Noad::new(NK::Accent { fam, ch: (v & 0x1F_FFFF) as u32, sub: 0 }), false);
            }
            MathP::Vcenter => self.vcenter(),
            MathP::Style(s) => self.emit.push(mlist::item(MItem::Style(s))),
            MathP::NonScript => {
                self.emit.push(Item::Glue { g: Arc::new(Glue::default()), sub: mlist::COND_MATH_GLUE, zero: true });
            }
            MathP::Choice => {
                // (§1172)
                let g = self.math_group_of(MathKind::Choice(0, Arc::new(Vec::new())));
                self.push_math(g);
                self.scan_left_brace();
            }
            MathP::Above(c) => self.math_fraction(c),
            MathP::Left => {
                // (§1191)
                let d = self.scan_delimiter(false);
                let g = self.math_group_of(MathKind::Left { middle: false });
                self.push_math(g);
                self.emit.push(mlist::item(MItem::Noad(Noad::new(NK::Left(d)))));
            }
            MathP::Right => self.math_right(false),
            MathP::Middle => self.math_right(true),
            MathP::EqNo(left) => {
                // (§1140, §1142)
                if self.st.inner {
                    self.err(format!("You can't use `{}' in math mode", self.shown(t)));
                } else if self.math_kind().is_some_and(|k| matches!(*k, MathKind::Display { .. })) {
                    let g = self.math_group_of(MathKind::EqNo { left });
                    self.push_math(g);
                    let em = self.toks_par(TokPar::Everymath);
                    self.rewrite(em);
                    let i = self.cs("\u{0}mathinit");
                    self.back(i);
                } else {
                    self.err("Missing $ inserted".into());
                }
            }
            MathP::Mskip => {
                let g = self.scan_glue(true);
                let zero = self.scratch.glue_shared && crate::item::is_zero(&g);
                self.emit.push(Item::Glue { g: Arc::new(g), sub: mlist::MU_GLUE, zero });
            }
            MathP::Mkern => {
                let w = self.scan_mu_dimen();
                self.emit.push(Item::Kern { w, sub: mlist::MU_GLUE });
            }
        }
    }

    pub(super) fn mode_name(&self) -> &'static str {
        match (self.st.mode, self.st.inner) {
            (Mode::V, false) => "vertical",
            (Mode::V, true) => "internal vertical",
            (Mode::H, false) => "horizontal",
            (Mode::H, true) => "restricted horizontal",
            (Mode::M, false) => "display math",
            (Mode::M, true) => "math",
        }
    }

    /// The innermost math group's kind, if the innermost group is one.
    fn math_kind(&self) -> Option<Arc<MathKind>> {
        let g = self.st.groups.last()?;
        if g.kind != GroupKind::Math {
            return None;
        }
        match &g.boxg.as_ref()?.kind {
            BoxKind::Math(k) => Some(k.clone()),
            _ => None,
        }
    }

    /// A math group's record, with the enclosing list's state.
    fn math_group_of(&self, k: MathKind) -> BoxGroup {
        BoxGroup {
            kind: BoxKind::Math(Arc::new(k)),
            context: BoxContext::Shift(0),
            spec: Spec::NATURAL,
            adjusted: false,
            mode: self.st.mode,
            inner: self.st.inner,
            space_factor: self.st.space_factor,
            prev_depth: self.st.prev_depth,
            hyph_lang: self.st.hyph_lang,
            prev_graf: self.st.prev_graf,
        }
    }

    /// §1136 `push_math`: a scope and a new math list (`-mmode`; a display
    /// is made non-inner by its opener).
    fn push_math(&mut self, mut g: BoxGroup) {
        g.mode = self.st.mode;
        g.inner = self.st.inner;
        g.space_factor = self.st.space_factor;
        g.prev_depth = self.st.prev_depth;
        g.hyph_lang = self.st.hyph_lang;
        g.prev_graf = self.st.prev_graf;
        let at = self.chain.nodes.len() as u32;
        self.st.groups.push(Group { kind: GroupKind::Math, after: Vec::new(), boxg: Some(Arc::new(g)), at, align: None });
        self.kind = Kind::Open;
        self.st.level += 1;
        self.st.mode = Mode::M;
        self.st.inner = true;
        self.st.prev_graf = 0;
    }

    /// `\fam` if it is a family (`fam_in_range`: XeTeX's 256).
    fn cur_fam(&mut self) -> Option<u8> {
        let f = self.int_par(IntPar::Fam);
        let n = if self.xetex { 256 } else { 16 };
        (0..n).contains(&f).then_some(f as u8)
    }

    /// The math code of `ch` in XeTeX's form (`fam<<24 | class<<21 |
    /// usv`, active "1FFFFF), TeX's 15 bits converted.
    fn math_code_x(&mut self, ch: char) -> i32 {
        let v = self.int(&Key::Mathcode(ch));
        if self.xetex { v } else { xe_form(v) }
    }

    /// The family of a math code in XeTeX's form: `\fam` for a variable
    /// one (class 7, §1151).
    fn var_fam_x(&mut self, c: i32) -> u8 {
        if (c >> 21) & 7 == 7
            && let Some(f) = self.cur_fam()
        {
            return f;
        }
        (c as u32 >> 24) as u8
    }

    /// §1155 (xetex.web `set_math_char`): a noad for math code `c` in
    /// XeTeX's form; an active one reads `ch` as an active character.
    fn set_math_char(&mut self, c: i32, ch: char) {
        if c & 0x1F_FFFF == 0x1F_FFFF {
            // (§1152: the character as an active one)
            self.back(Tok::Active(ch));
            return;
        }
        let fam = self.var_fam_x(c);
        let class = (c >> 21) & 7;
        let kind = if class == 7 { NK::Ord } else { NK::comp(class as u8) };
        let mut n = Noad::new(kind);
        n.nucleus = Field::Char { fam, ch: (c & 0x1F_FFFF) as u32 };
        self.emit.push(mlist::item(MItem::Noad(n)));
    }

    /// §1151 (xetex.web `scan_math`): a math character's field, or `None`
    /// when a subformula follows (the next node opens its group).
    fn scan_math(&mut self, target: Target) -> Option<Field> {
        loop {
            let (t, m) = self.nonblank_nonrelax()?;
            let mut ch = '\0';
            let c = match m {
                Meaning::Char(11 | 12, x) => {
                    ch = x;
                    self.math_code_x(x)
                }
                Meaning::CharGiven(c) => {
                    ch = char::from_u32(c).unwrap_or('\0');
                    self.math_code_x(ch)
                }
                Meaning::Prim(Prim::Char) => {
                    let c = self.scan_char_num();
                    ch = char::from_u32(c as u32).unwrap_or('\0');
                    self.math_code_x(ch)
                }
                Meaning::Prim(Prim::Math(MathP::MathChar)) => xe_form(self.scan_fifteen_bit_int()),
                Meaning::Prim(Prim::Xe(Xe::MathCharNum(k))) => self.scan_xe_math_char(k),
                Meaning::MathCharGiven(v) => xe_form(v as i32),
                Meaning::XMathCharGiven(v) => v as i32,
                Meaning::Prim(Prim::Math(MathP::Delimiter)) => xe_form(self.scan_twenty_seven_bit_int() / 0o10_000),
                Meaning::Prim(Prim::Xe(Xe::Delimiter)) => self.scan_xe_math_char(2),
                _ => {
                    self.open_sub_later(target, t);
                    return None;
                }
            };
            if c & 0x1F_FFFF == 0x1F_FFFF {
                // (an active character: read again as one)
                self.back(Tok::Active(ch));
                continue;
            }
            let fam = self.var_fam_x(c);
            return Some(Field::Char { fam, ch: (c & 0x1F_FFFF) as u32 });
        }
    }

    /// The subformula whose `{` (or what stands for it) is `t` opens in the
    /// next node.
    fn open_sub_later(&mut self, target: Target, t: Tok) {
        let g = self.math_group_of(MathKind::Sub(target));
        self.st.boxing = Some(Arc::new(BoxEnd { g: Arc::new(g), boxmaxdepth: 0, math: None }));
        // (§1153: `back_input`, then `scan_left_brace` reads it again: as
        // TeX's, which an alignment's brace count sees)
        let n = self.cs("\u{0}mathgroup");
        self.back(t);
        self.rewrite(vec![n]);
    }

    /// `\0mathgroup`: the subformula's group opens (§1151's `push_math`).
    pub(super) fn math_group_open(&mut self) {
        let Some(be) = self.st.boxing.take() else { return };
        self.scan_left_brace();
        self.push_math((*be.g).clone());
    }

    /// A noad whose nucleus `scan_math` fills (§1158, §1163, §1165).
    fn noad_with_nucleus(&mut self, mut n: Noad, ord: bool) {
        if let Some(f) = self.scan_math(Target { which: Which::Nucleus, ord }) {
            n.nucleus = f;
        }
        self.emit.push(mlist::item(MItem::Noad(n)));
    }

    /// The items of the current math list so far (a read of the list: the
    /// node runs again whenever the list's nodes do, as `\lastskip` does).
    fn math_items(&mut self) -> Vec<Item> {
        if self.kind == Kind::Other {
            self.kind = Kind::Par;
        }
        let level = self.st.level;
        let nodes = &self.chain.nodes;
        let start = box_start(self.chain, nodes.len(), level);
        let mut list = Vec::new();
        for n in nodes.range(start, nodes.len()) {
            if n.out.level == level {
                for it in n.emit.iter() {
                    push_horizontal(&mut list, it.clone());
                }
            }
        }
        for it in self.emit.clone() {
            push_horizontal(&mut list, it);
        }
        list
    }

    /// The current math list's last noad.
    fn mlist_tail(&mut self) -> Option<Noad> {
        let list = self.math_items();
        match mlist::current(list).pop() {
            Some(Item::Noad(m)) => match &*m {
                MItem::Noad(n) => Some(n.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// `^`, `_` (§1176 `sub_sup`); outside math, a `$` is missing.
    pub(super) fn sub_sup_cmd(&mut self, sup: bool, t: &Tok) {
        if self.st.mode != Mode::M {
            self.insert_dollar_sign(t);
            return;
        }
        let which = if sup { Which::Sup } else { Which::Sub };
        let tail = self.mlist_tail();
        let (ok, full) = match &tail {
            Some(n) if n.kind.scripts_allowed() => {
                (true, if sup { n.sup != Field::Empty } else { n.sub != Field::Empty })
            }
            _ => (false, false),
        };
        if !ok || full {
            self.emit.push(mlist::item(MItem::Noad(Noad::new(NK::Ord))));
            if full {
                self.err(if sup { "Double superscript".into() } else { "Double subscript".into() });
            }
        }
        if let Some(f) = self.scan_math(Target { which, ord: false }) {
            self.emit.push(mlist::item(MItem::Set(which, f)));
        }
    }

    /// §1159: `\limits` and friends.
    fn limit_switch(&mut self, l: Limits) {
        let tail = self.mlist_tail();
        if tail.is_some_and(|n| matches!(n.kind, NK::Op(_))) {
            self.emit.push(mlist::item(MItem::Limits(l)));
        } else {
            self.err("Limit controls must follow a math operator".into());
        }
    }

    /// §1160: `scan_delimiter` (`r`: `\radical`'s 24-bit code).
    fn scan_delimiter(&mut self, r: bool) -> Delim {
        let v = if r {
            self.scan_twenty_seven_bit_int()
        } else {
            match self.nonblank_nonrelax() {
                Some((_, Meaning::Char(11 | 12, c))) => self.int(&Key::Delcode(c)),
                Some((_, Meaning::Prim(Prim::Math(MathP::Delimiter)))) => self.scan_twenty_seven_bit_int(),
                Some((_, Meaning::Prim(Prim::Xe(Xe::Delimiter)))) => {
                    // (`\Udelimiter`: the class is read and dropped)
                    let _ = self.scan_math_class_int();
                    let fam = self.scan_math_fam_int();
                    let u = self.scan_usv_num();
                    0x4000_0000 + fam * 0x20_0000 + u
                }
                Some((t, _)) => {
                    self.back(t);
                    -1
                }
                None => -1,
            }
        };
        if v < 0 {
            self.err("Missing delimiter (. inserted)".into());
            return Delim::default();
        }
        Delim::from_code(v)
    }

    /// §1181: `\over` and friends.
    fn math_fraction(&mut self, c: u8) {
        let (left, right) =
            if c >= 3 { (self.scan_delimiter(false), self.scan_delimiter(false)) } else { Default::default() };
        let thickness = match c % 3 {
            0 => self.scan_dimen(),
            1 => mlist::DEFAULT_CODE,
            _ => 0,
        };
        let list = self.math_items();
        if list.iter().any(|i| matches!(i, Item::Noad(m) if matches!(**m, MItem::Above { .. }))) {
            self.err("Ambiguous; you need another { and }".into());
            return;
        }
        self.emit.push(mlist::item(MItem::Above { thickness, left, right }));
    }

    /// `\right`, e-TeX's `\middle` (§1191–§1192, etex.web's
    /// `math_left_right`).
    fn math_right(&mut self, middle: bool) {
        match self.math_kind().as_deref() {
            Some(MathKind::Left { .. }) => {
                let d = self.scan_delimiter(false);
                let end = MathEnd {
                    params: mlist::Params::default(),
                    fonts: mlist::Fonts::default(),
                    surround: 0,
                    right: Some(d),
                    display: None,
                    middle,
                    carry: Vec::new(),
                    danger: false,
                };
                self.math_close(Some(end));
            }
            Some(MathKind::Inline | MathKind::Display { .. } | MathKind::EqNo { .. }) => {
                let _ = self.scan_delimiter(false);
                self.err(format!("Extra \\{}", if middle { "middle" } else { "right" }));
            }
            _ => {
                let t = self.cmd.clone();
                self.math_off_save(&t);
            }
        }
    }

    /// tex.web 1064 `off_save` where the innermost group is not the one a
    /// math command ends: the token that ends that group goes in first.
    pub(super) fn math_off_save(&mut self, t: &Tok) {
        let kind = self.st.groups.last().map(|g| g.kind);
        let (ins, what) = match (kind, self.math_kind().as_deref()) {
            (None, _) => {
                self.err(format!("Extra {}", self.shown(t)));
                return;
            }
            (Some(GroupKind::SemiSimple), _) => (vec![self.cs("endgroup")], "\\endgroup".to_string()),
            (
                Some(GroupKind::Math),
                Some(
                    MathKind::Inline | MathKind::Display { .. } | MathKind::DisplayAlign { .. } | MathKind::EqNo { .. },
                ),
            ) => {
                (vec![Tok::Char(3, '$')], "$".to_string())
            }
            (Some(GroupKind::Math), Some(MathKind::Left { .. })) => {
                (vec![self.cs("right"), Tok::Char(12, '.')], "\\right.".to_string())
            }
            _ => (vec![Tok::Char(2, '}')], "}".to_string()),
        };
        self.err(format!("Missing {what} inserted"));
        let mut toks = ins;
        toks.push(t.clone());
        self.rewrite(toks);
    }

    /// `}` of a math group (tex.web 1186, 1174; 1069 for the others).
    pub(super) fn math_right_brace(&mut self) {
        match self.math_kind().as_deref() {
            Some(MathKind::Sub(_) | MathKind::Choice(..)) => self.math_close(None),
            Some(MathKind::Left { .. }) => self.err("Extra }, or forgotten \\right".into()),
            _ => self.err("Extra }, or forgotten $".into()),
        }
    }

    /// A math group's `Close` node: it defines nothing; what it read inside
    /// goes to the packaging node after it.
    fn math_close(&mut self, end: Option<MathEnd>) {
        let Some(g) = self.st.groups.last().and_then(|g| g.boxg.clone()) else { return };
        let after = self.st.groups.pop().map(|g| g.after).unwrap_or_default();
        self.st.mode = g.mode;
        self.st.inner = g.inner;
        self.st.space_factor = g.space_factor;
        self.st.prev_depth = g.prev_depth;
        self.st.hyph_lang = g.hyph_lang;
        self.st.prev_graf = g.prev_graf;
        self.st.level -= 1;
        self.st.boxing = Some(Arc::new(BoxEnd { g, boxmaxdepth: 0, math: end.map(Arc::new) }));
        self.rewrite(after);
        let p = self.cs("\u{0}package");
        self.back(p);
        self.closes();
    }

    /// What a formula reads where it ends.
    fn math_end(&mut self, display: bool) -> MathEnd {
        // (TeX's 16 families; XeTeX's others where the formula is
        // converted, `mlist_hlist`)
        let mut fonts = vec![None; mlist::FAM_SLOTS];
        for size in 0..3u8 {
            for fam in 0..16usize {
                let f = self.fam_font(size, fam as u8);
                fonts[usize::from(size) * mlist::SCRIPT_SIZE + fam] = (&*f.key != "nullfont").then_some(f);
            }
        }
        let params = mlist::Params {
            script_space: self.dimen_par(DimenPar::Scriptspace),
            null_delimiter_space: self.dimen_par(DimenPar::Nulldelimiterspace),
            delimiter_factor: self.int_par(IntPar::Delimiterfactor),
            delimiter_shortfall: self.dimen_par(DimenPar::Delimitershortfall),
            bin_op_penalty: self.int_par(IntPar::Binoppenalty),
            rel_penalty: self.int_par(IntPar::Relpenalty),
            thin_mu_skip: self.glue_par(GluePar::Thinmuskip),
            med_mu_skip: self.glue_par(GluePar::Medmuskip),
            thick_mu_skip: self.glue_par(GluePar::Thickmuskip),
        };
        let surround = if display { 0 } else { self.dimen_par(DimenPar::Mathsurround) };
        let display = display.then(|| DisplayEnd {
            z: self.dimen_par(DimenPar::Displaywidth),
            s: self.dimen_par(DimenPar::Displayindent),
            pre_size: self.dimen_par(DimenPar::Predisplaysize),
            skips: [
                self.glue_par(GluePar::Abovedisplayskip),
                self.glue_par(GluePar::Belowdisplayskip),
                self.glue_par(GluePar::Abovedisplayshortskip),
                self.glue_par(GluePar::Belowdisplayshortskip),
            ],
            pre_penalty: self.int_par(IntPar::Predisplaypenalty),
            post_penalty: self.int_par(IntPar::Postdisplaypenalty),
            inter: self.interline(),
            pack: Params {
                badness: self.int_par(IntPar::Hbadness),
                fuzz: self.dimen_par(DimenPar::Hfuzz),
                overfull_rule: self.dimen_par(DimenPar::Overfullrule),
            },
        });
        MathEnd {
            params,
            fonts: mlist::Fonts(fonts),
            surround,
            right: None,
            display,
            middle: false,
            carry: Vec::new(),
            danger: false,
        }
    }

    // ------------------------------------------------------------ $

    /// A math shift character, by mode (tex.web 1090, 1137, 1193).
    pub(super) fn math_shift(&mut self, t: &Tok) {
        match self.st.mode {
            Mode::V => {
                self.back(t.clone());
                self.new_graf(true);
            }
            Mode::H => self.init_math(),
            Mode::M => match self.math_kind().as_deref() {
                // (§1194: the fonts first, then the second `$`)
                Some(MathKind::Inline) => {
                    let danger = !self.fonts_ok();
                    let mut end = self.math_end(false);
                    end.danger = danger;
                    self.math_close(Some(end));
                }
                Some(MathKind::EqNo { .. }) => {
                    let danger = !self.fonts_ok();
                    self.check_dollar();
                    let mut end = self.math_end(false);
                    end.danger = danger;
                    self.math_close(Some(end));
                }
                Some(MathKind::Display { .. }) => {
                    let danger = !self.fonts_ok();
                    self.check_dollar();
                    let mut end = self.math_end(true);
                    end.danger = danger;
                    self.math_close(Some(end));
                }
                // (§1206: no formula, no fonts checked)
                Some(MathKind::DisplayAlign { .. }) => self.display_alignment_end(None),
                _ => self.math_off_save(t),
            },
        }
    }

    /// §1197: "Check that another `$` follows".
    fn check_dollar(&mut self) {
        match self.next_x_m(false) {
            Some((_, Meaning::Char(3, _))) => {}
            other => {
                self.err("Display math should end with $$".into());
                if let Some((t, _)) = other {
                    self.back(t);
                }
            }
        }
    }

    /// §1138: `init_math`: `$` in horizontal mode.
    fn init_math(&mut self) {
        let next = self.next_raw();
        if !self.st.inner
            && let Some(n) = &next
            && matches!(self.tok_meaning(n), Meaning::Char(3, _))
        {
            self.start_display();
            return;
        }
        if let Some(n) = next {
            self.back(n);
        }
        // (§1139: "Go into ordinary math mode")
        let g = self.math_group_of(MathKind::Inline);
        self.push_math(g);
        let em = self.toks_par(TokPar::Everymath);
        self.rewrite(em);
        let i = self.cs("\u{0}mathinit");
        self.back(i);
    }

    /// `\0mathinit`: the parameters a formula defines inside its group
    /// (§1139, §1145: `\fam`; a display's size, width and indent).
    pub(super) fn math_init(&mut self) {
        self.define(Key::IntPar(IntPar::Fam), Val::Int(-1), false);
        if let Some(k) = self.math_kind()
            && let MathKind::Display { w, l, s, .. } = *k
        {
            self.define(Key::DimenPar(DimenPar::Predisplaysize), Val::Int(w), false);
            self.define(Key::DimenPar(DimenPar::Displaywidth), Val::Int(l), false);
            self.define(Key::DimenPar(DimenPar::Displayindent), Val::Int(s), false);
        }
    }

    /// §1145: `$$` in a paragraph: the paragraph so far is broken into
    /// lines (`line_break(display_widow_penalty)`); its last line gives
    /// `\predisplaysize` (§1146).
    fn start_display(&mut self) {
        self.kind = Kind::Par;
        let level = self.st.level;
        let nodes = &self.chain.nodes;
        let start = para_start(self.chain, nodes.len(), level);
        let mut hlist = Vec::new();
        for n in nodes.range(start, nodes.len()) {
            if n.out.level == level && n.out.mode == Mode::H {
                for it in n.emit.iter() {
                    push_horizontal(&mut hlist, it.clone());
                }
            }
        }
        for it in std::mem::take(&mut self.emit) {
            push_horizontal(&mut hlist, it);
        }
        let ships: Vec<Item> = hlist.iter().filter(|i| matches!(i, Item::Shipout(_))).cloned().collect();
        hlist.retain(|i| !matches!(i, Item::Shipout(_)));
        self.st.mode = Mode::V;
        self.st.inner = level > 0;
        let mut para = Para { before: ships, ..Para::default() };
        let mut w = -MAX_DIMEN;
        if !hlist.is_empty() {
            if matches!(hlist.last(), Some(Item::Glue { .. })) {
                hlist.pop();
            }
            hlist.push(Item::Penalty(10000));
            let g = self.glue_par(GluePar::Parfillskip);
            hlist.push(param_glue(&g, GluePar::Parfillskip as u8));
            let mut lp = self.line_params(&hlist);
            lp.widowpenalty = self.int_par(IntPar::Displaywidowpenalty);
            lp.widowpenalties = self.penalties(3);
            let lines = linebreak::lines(&hlist, &lp);
            if level == 0 {
                para.hlist = hlist;
                para.lb = Some(Arc::new(lp.clone()));
            } else {
                self.vemit.append(&mut para.before);
                for l in &lines {
                    if let Some(r) = &l.report {
                        self.log_raw(r);
                    }
                    self.vemit.push(Item::Append(l.b.clone(), lp.inter.clone()));
                    self.vemit.extend(l.adjust.iter().cloned());
                    if let Some(x) = l.pen {
                        self.vemit.push(Item::Penalty(x));
                    }
                }
                if let Some(l) = lines.last() {
                    self.st.prev_depth = l.b.d;
                }
            }
            self.st.prev_graf = linebreak::prev_graf_after(&lp, lines.len());
            if let Some(l) = lines.last() {
                w = self.pre_display_size(&l.b);
            }
        }
        if level == 0 {
            if !para.before.is_empty() || para.lb.is_some() {
                self.para = Some(Arc::new(para));
            }
            // (§1145: `build_page` once the display's list is begun)
            let c = self.page_call();
            self.vemit.push(Item::BuildPage(c));
        }
        let g = self.math_group_of(MathKind::DisplayInit { w });
        self.st.boxing = Some(Arc::new(BoxEnd { g: Arc::new(g), boxmaxdepth: 0, math: None }));
        let d = self.cs("\u{0}display");
        self.back(d);
    }

    /// §1146–§1148: the natural width of the line before a display, as far
    /// as it is visible (`max_dimen` if glue in it stretches or shrinks).
    fn pre_display_size(&mut self, just: &BoxNode) -> i32 {
        let quad = self.font_param(6);
        let mut v = just.shift + 2 * quad;
        let mut w = -MAX_DIMEN;
        for it in &just.list {
            let (d, visible) = match it {
                Item::Char(..) | Item::Lig(_) | Item::Box(_) | Item::Rule { .. } => (it.width(), true),
                Item::Kern { w, .. } | Item::Math { w, .. } => (*w, false),
                Item::Glue { .. } | Item::Leaders(_) => {
                    let g = it.glue().expect("glue");
                    if (just.glue.sign == 1 && just.glue.order == g.stretch_order && g.stretch != 0)
                        || (just.glue.sign == -1 && just.glue.order == g.shrink_order && g.shrink != 0)
                    {
                        v = MAX_DIMEN;
                    }
                    // (§1148: leaders are visible)
                    (g.width, matches!(it, Item::Leaders(_)))
                }
                _ => (0, false),
            };
            if visible {
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
        w
    }

    /// `\0display`: the display's group opens (§1145, §1149).
    pub(super) fn display_open(&mut self) {
        let Some(be) = self.st.boxing.take() else { return };
        let BoxKind::Math(k) = &be.g.kind else { return };
        let MathKind::DisplayInit { w } = **k else { return };
        // (§1149: the display's line length and shift)
        let pg = self.st.prev_graf;
        let (l, s) = match self.get(&Key::Parshape) {
            Val::Parshape(ps) if !ps.is_empty() => {
                let n = ps.len() as i32;
                let (i, ll) = if pg + 2 >= n { ps[ps.len() - 1] } else { ps[(pg + 1) as usize] };
                (ll, i)
            }
            _ => {
                let hi = self.dimen_par(DimenPar::Hangindent);
                let ha = self.int_par(IntPar::Hangafter);
                let hsize = self.dimen_par(DimenPar::Hsize);
                if hi != 0 && ((ha >= 0 && pg + 2 > ha) || (pg + 1 < -ha)) {
                    (hsize - hi.abs(), hi.max(0))
                } else {
                    (hsize, 0)
                }
            }
        };
        let g = self.math_group_of(MathKind::Display { w, l, s, eqno: None });
        self.push_math(g);
        self.st.inner = false;
        let ed = self.toks_par(TokPar::Everydisplay);
        self.rewrite(ed);
        let i = self.cs("\u{0}mathinit");
        self.back(i);
    }

    /// `\0displayend`: after an equation number, the display's own end
    /// (§1194: the rest of `after_math`).
    pub(super) fn display_end(&mut self) {
        let danger = !self.fonts_ok();
        let mut end = self.math_end(true);
        end.danger = danger;
        self.math_close(Some(end));
    }

    /// `\0choicenext`: the next `\mathchoice` part (§1174).
    /// `\0middlenext`: the `\left` group opens again after `\middle`, its
    /// list the one so far (etex.web §1191).
    pub(super) fn middle_next(&mut self) {
        let Some(be) = self.st.boxing.take() else { return };
        self.push_math((*be.g).clone());
        if let Some(m) = &be.math {
            self.emit.extend(m.carry.iter().cloned());
        }
    }

    pub(super) fn choice_next(&mut self) {
        let Some(be) = self.st.boxing.take() else { return };
        self.push_math((*be.g).clone());
        self.scan_left_brace();
    }

    /// §1167: `\vcenter`.
    fn vcenter(&mut self) {
        let spec = if self.keyword("to") {
            Spec::Exactly(self.scan_dimen())
        } else if self.keyword("spread") {
            Spec::Additional(self.scan_dimen())
        } else {
            Spec::NATURAL
        };
        self.scan_left_brace();
        let shaped = self.paragraph_shaped();
        let g = BoxGroup {
            kind: BoxKind::Vcenter,
            context: BoxContext::Shift(0),
            spec,
            adjusted: false,
            mode: self.st.mode,
            inner: self.st.inner,
            space_factor: self.st.space_factor,
            prev_depth: self.st.prev_depth,
            hyph_lang: self.st.hyph_lang,
            prev_graf: self.st.prev_graf,
        };
        self.open_box(g, Mode::V);
        let every = self.toks_par(TokPar::Everyvbox);
        self.rewrite(every);
        if shaped {
            let t = self.cs("\u{0}normalparagraph");
            self.back(t);
        }
    }

    /// §1168: a `\vcenter`'s box, as a noad.
    pub(super) fn vcenter_package(&mut self, be: &BoxEnd, list: Vec<Item>, ships: Vec<Item>) {
        for s in ships {
            self.emit_here(s);
        }
        let params =
            Params { badness: self.int_par(IntPar::Vbadness), fuzz: self.dimen_par(DimenPar::Vfuzz), overfull_rule: 0 };
        let packed = pack::vpack(list, be.g.spec, MAX_DIMEN, &params);
        if let Some(r) = packed.report {
            let cx = self.show_ctx(std::slice::from_ref(&Item::Box(Arc::new(packed.b.clone()))));
            let s = report(r, &packed.b, self.box_place(), &cx);
            self.log_raw(&s);
        }
        let mut n = Noad::new(NK::Vcenter);
        n.nucleus = Field::Box(Arc::new(packed.b));
        self.emit.push(mlist::item(MItem::Noad(n)));
    }

    // ------------------------------------------------------------ packaging

    /// The packaging node of a math group: its list, gathered and put
    /// where it goes.
    pub(super) fn math_package(&mut self, be: &BoxEnd, k: &MathKind, list: Vec<Item>, ships: Vec<Item>) {
        for s in ships {
            self.emit_here(s);
        }
        match k {
            MathKind::Sub(target) => {
                let p = self.gather(list, None);
                // (§1186)
                if let [Item::Noad(m)] = p.as_slice()
                    && let MItem::Noad(n) = &**m
                {
                    if n.kind == NK::Ord {
                        if n.sub == Field::Empty && n.sup == Field::Empty {
                            self.emit.push(mlist::item(MItem::Set(target.which, n.nucleus.clone())));
                            return;
                        }
                    } else if matches!(n.kind, NK::Accent { .. }) && target.which == Which::Nucleus && target.ord {
                        self.emit.push(mlist::item(MItem::Replace(n.clone())));
                        return;
                    }
                }
                self.emit.push(mlist::item(MItem::Set(target.which, Field::Mlist(p))));
            }
            MathKind::Left { .. } => {
                let d = be.math.as_ref().and_then(|m| m.right).unwrap_or_default();
                let middle = be.math.as_ref().is_some_and(|m| m.middle);
                let p = self.gather(list, Some(Noad::new(NK::Right(d, middle))));
                if middle {
                    // (etex.web §1191: the group opens again, its list the
                    // one so far, ending with the middle noad; in a node of
                    // its own, as `\mathchoice`'s parts)
                    let mut g = (*be.g).clone();
                    g.kind = BoxKind::Math(Arc::new(MathKind::Left { middle: true }));
                    let end = MathEnd {
                        params: mlist::Params::default(),
                        fonts: mlist::Fonts::default(),
                        surround: 0,
                        right: None,
                        display: None,
                        middle: true,
                        carry: p,
                        danger: false,
                    };
                    self.st.boxing = Some(Arc::new(BoxEnd { g: Arc::new(g), boxmaxdepth: 0, math: Some(Arc::new(end)) }));
                    let t = self.cs("\u{0}middlenext");
                    self.back(t);
                    return;
                }
                let mut n = Noad::new(NK::Inner);
                n.nucleus = Field::Mlist(p);
                self.emit.push(mlist::item(MItem::Noad(n)));
            }
            MathKind::Choice(stage, parts) => {
                let p = self.gather(list, None);
                let mut parts = (**parts).clone();
                parts.push(p);
                if *stage < 3 {
                    let mut g = (*be.g).clone();
                    g.kind = BoxKind::Math(Arc::new(MathKind::Choice(stage + 1, Arc::new(parts))));
                    self.st.boxing = Some(Arc::new(BoxEnd { g: Arc::new(g), boxmaxdepth: 0, math: None }));
                    let t = self.cs("\u{0}choicenext");
                    self.back(t);
                } else {
                    let mut it = parts.into_iter();
                    let mut next = || it.next().unwrap_or_default();
                    let c = [next(), next(), next(), next()];
                    self.emit.push(mlist::item(MItem::Choice(Box::new(c))));
                }
            }
            MathKind::Inline => {
                let Some(end) = be.math.clone() else { return };
                let mut p = self.gather(list, None);
                if end.danger {
                    p.clear();
                }
                // (§1196)
                self.emit.push(Item::Math { w: end.surround, after: false, lr: 0 });
                let penalties = !self.st.inner;
                let hl = self.mlist_hlist(p, mlist::TEXT, penalties, &end);
                self.emit.extend(hl);
                self.emit.push(Item::Math { w: end.surround, after: true, lr: 0 });
                self.st.space_factor = 1000;
            }
            MathKind::EqNo { left } => {
                let Some(end) = be.math.clone() else { return };
                let mut p = self.gather(list, None);
                if end.danger {
                    p.clear();
                }
                // (§1194: the equation number's box, kept in the display's
                // group record)
                let hl = self.mlist_hlist(p, mlist::TEXT, false, &end);
                let a = Arc::new(mlist::hpack_natural(hl));
                if let Some(g) = self.st.groups.last_mut()
                    && let Some(bg) = &g.boxg
                    && let BoxKind::Math(k) = &bg.kind
                    && let MathKind::Display { w, l, s, .. } = **k
                {
                    let mut nb = (**bg).clone();
                    nb.kind = BoxKind::Math(Arc::new(MathKind::Display { w, l, s, eqno: Some((a, *left)) }));
                    g.boxg = Some(Arc::new(nb));
                }
                let t = self.cs("\u{0}displayend");
                self.back(t);
            }
            MathKind::Display { eqno, .. } => {
                let Some(end) = be.math.clone() else { return };
                self.finish_display(list, &end, eqno.as_ref());
            }
            MathKind::DisplayAlign { rows, aux } => {
                let Some(end) = be.math.clone() else { return };
                self.finish_display_alignment(rows, *aux, &end);
            }
            MathKind::DisplayInit { .. } => {}
        }
    }

    /// `fin_mlist`, with its errors.
    fn gather(&mut self, list: Vec<Item>, right: Option<Noad>) -> Vec<Item> {
        let (p, errors) = mlist::gather(list, right);
        for _ in errors {
            self.err("Ambiguous; you need another { and }".into());
        }
        p
    }

    /// §1195: the symbol and extension fonts a formula needs.
    fn fonts_ok(&mut self) -> bool {
        // (xetex.web: an OpenType math font has what it needs)
        let mut params = |size: u8, fam: u8| {
            let f = self.fam_font(size, fam);
            if crate::native::is_math(&f) { usize::MAX } else { self.font_params(&f) }
        };
        if [0, 1, 2].into_iter().any(|z| params(z, 2) < 22) {
            self.err("Math formula deleted: Insufficient symbol fonts".into());
            return false;
        }
        if [0, 1, 2].into_iter().any(|z| params(z, 3) < 13) {
            self.err("Math formula deleted: Insufficient extension fonts".into());
            return false;
        }
        true
    }

    /// `mlist_to_hlist`, with what it reports.
    fn mlist_hlist(&mut self, p: Vec<Item>, style: u8, penalties: bool, end: &MathEnd) -> Vec<Item> {
        // (XeTeX's families past 15 the formula uses, read where it is
        // converted: after its group, CORE.md)
        let mut used = [false; 256];
        if self.xetex {
            mlist::families(&p, &mut used);
        }
        let fonts = if used[16..].iter().any(|&u| u) {
            let mut fonts = end.fonts.clone();
            for size in 0..3u8 {
                for (fam, _) in used.iter().enumerate().skip(16).filter(|(_, u)| **u) {
                    let f = self.fam_font(size, fam as u8);
                    fonts.0[usize::from(size) * mlist::SCRIPT_SIZE + fam] = (&*f.key != "nullfont").then_some(f);
                }
            }
            std::borrow::Cow::Owned(fonts)
        } else {
            std::borrow::Cow::Borrowed(&end.fonts)
        };
        let (hl, events) = mlist::mlist_to_hlist(p, style, penalties, &fonts, &end.params, &mut Reads(self));
        for e in events {
            match e {
                mlist::Event::UndefinedFamily { size, fam, ch } => {
                    let s = match size {
                        mlist::TEXT_SIZE => "textfont",
                        mlist::SCRIPT_SIZE => "scriptfont",
                        _ => "scriptscriptfont",
                    };
                    self.err(format!("\\{s} {fam} is undefined (character {})", char::from_u32(ch).unwrap_or('?')));
                }
                mlist::Event::MissingChar { font, ch } => self.char_warning(&font, ch as i32),
            }
        }
        hl
    }

    /// §1199–§1205: "Finish displayed math", then §1200
    /// `resume_after_display`.
    fn finish_display(&mut self, list: Vec<Item>, end: &MathEnd, eqno: Option<&(Arc<BoxNode>, bool)>) {
        let Some(de) = &end.display else { return };
        let mut p = self.gather(list, None);
        let danger = end.danger;
        if danger {
            p.clear();
        }
        let hl = self.mlist_hlist(p, mlist::DISPLAY, false, end);
        let mut adjust = Vec::new();
        let packed = hpack(hl, Spec::NATURAL, &Params::QUIET, Some(&mut adjust));
        let mut b = packed.b;
        let list = b.list.clone();
        let mut w = b.w;
        let (z, s) = (de.z, de.s);
        let (a, l) = match eqno {
            Some((a, l)) => (Some(a.clone()), *l),
            None => (None, false),
        };
        let (mut e, q) = match (&a, danger) {
            (Some(a), false) => {
                let quad = match end.fonts.0.get(2).and_then(Option::as_ref) {
                    Some(f) => self.int(&Key::FontDimen(f.key.clone(), 6)),
                    None => 0,
                };
                (a.w, a.w + quad)
            }
            _ => (0, 0),
        };
        if w + q > z {
            // (§1201)
            let mut shrink = [0i32; 4];
            for it in &list {
                if let Item::Glue { g, .. } = it {
                    shrink[usize::from(g.shrink_order.min(3))] += g.shrink;
                }
            }
            if e != 0 && (w - shrink[0] + q <= z || shrink[1] != 0 || shrink[2] != 0 || shrink[3] != 0) {
                b = self.display_hpack(list.clone(), z - q, &de.pack);
            } else {
                e = 0;
                if w > z {
                    b = self.display_hpack(list.clone(), z, &de.pack);
                }
            }
            w = b.w;
        }
        // (§1202)
        let mut d = mlist::half(z - w);
        if e > 0 && d < 2 * e {
            d = mlist::half(z - w - e);
            if matches!(list.first(), Some(Item::Glue { .. })) {
                d = 0;
            }
        }
        // (§1203)
        self.vemit.push(Item::Penalty(de.pre_penalty));
        let short = !(d + s <= de.pre_size || l);
        let (g1, g2) = if short { (2, 3) } else { (0, 1) };
        let mut g2 = Some(g2);
        if l && e == 0 {
            let mut a = (**a.as_ref().expect("an equation number")).clone();
            a.shift = s;
            self.append_vlist(Arc::new(a), &de.inter);
            self.vemit.push(Item::Penalty(10000));
        } else {
            self.vemit.push(param_glue(&de.skips[g1], GluePar::Abovedisplayskip as u8 + g1 as u8));
        }
        // (§1204)
        if e != 0 {
            let a = a.clone().expect("an equation number");
            let r = Item::Kern { w: z - w - e - d, sub: 0 };
            let row = if l {
                d = 0;
                vec![Item::Box(a), r, Item::Box(Arc::new(b))]
            } else {
                vec![Item::Box(Arc::new(b)), r, Item::Box(a)]
            };
            b = mlist::hpack_natural(row);
        }
        b.shift = s + d;
        self.append_vlist(Arc::new(b), &de.inter);
        // (§1205)
        if let Some(a) = &a
            && e == 0
            && !l
        {
            self.vemit.push(Item::Penalty(10000));
            let mut a = (**a).clone();
            a.shift = s + z - a.w;
            self.append_vlist(Arc::new(a), &de.inter);
            g2 = None;
        }
        self.vemit.extend(adjust);
        self.vemit.push(Item::Penalty(de.post_penalty));
        if let Some(g2) = g2 {
            self.vemit.push(param_glue(&de.skips[g2], GluePar::Abovedisplayskip as u8 + g2 as u8));
        }
        self.resume_after_display();
    }

    /// §1200: `resume_after_display`, the display's group closed: the
    /// paragraph goes on.
    fn resume_after_display(&mut self) {
        self.st.prev_graf += 3;
        self.st.mode = Mode::H;
        self.st.inner = false;
        self.st.space_factor = 1000;
        self.begin_paragraph_language();
        match self.next_x_m(false) {
            Some((_, Meaning::Char(10, _))) | None => {}
            Some((t, _)) => self.back(t),
        }
        if self.st.level == 0 {
            let c: Arc<PageCall> = self.page_call();
            self.vemit.push(Item::BuildPage(c));
        }
    }

    // ------------------------------------------------------------ display alignments

    /// §1145, `mmode+halign`: an alignment may begin a display (not an
    /// inline formula: `privileged`; in the display's own group, else
    /// `off_save`); the vertical list's depth before the display, for the
    /// alignment's list (§775).
    pub(super) fn display_for_alignment(&mut self, t: &Tok) -> Option<i32> {
        if self.st.inner {
            // (§1049: `report_illegal_case`)
            self.err(format!("You can't use `{}' in {} mode", self.shown(t), self.mode_name()));
            return None;
        }
        if !self.math_kind().is_some_and(|k| matches!(*k, MathKind::Display { .. })) {
            self.math_off_save(t);
            return None;
        }
        if !self.math_items().is_empty() {
            // (§776: TeX flushes the formula; not here)
            self.err("Improper \\halign inside $$'s".into());
        }
        self.st.groups.last().and_then(|g| g.boxg.clone()).map(|g| g.prev_depth)
    }

    /// The display's alignment is set (§812, `mode=mmode`): its material
    /// and depth wait in the display's group record for its `$$`.
    pub(super) fn display_aligned(&mut self, rows: Vec<Item>, aux: i32) {
        if let Some(g) = self.st.groups.last_mut()
            && g.kind == GroupKind::Math
            && let Some(bg) = &g.boxg
        {
            let mut nb = (**bg).clone();
            nb.kind = BoxKind::Math(Arc::new(MathKind::DisplayAlign { rows: Arc::new(rows), aux }));
            g.boxg = Some(Arc::new(nb));
        }
    }

    /// §1206 `do_assignments`: after a display's alignment only
    /// assignments come before `$$`; anything else ends the display
    /// ("Missing $$ inserted").
    fn after_display_alignment(&mut self, t: &Tok, m: &Meaning) -> bool {
        let ok = match m {
            Meaning::Char(3 | 10, _) | Meaning::Prim(Prim::Relax) => true,
            Meaning::Prim(p) if p.conditional() => true,
            _ => super::assign::prefixable(m),
        };
        if ok {
            return false;
        }
        self.display_alignment_end(Some(t));
        true
    }

    /// The end of a display alignment: `$$` checked (§1197, §1207, with
    /// TeX's errors), what the display reads before its group ends, and the
    /// group's `Close` node; its packaging node does the rest
    /// ([`Exec::finish_display_alignment`]).
    fn display_alignment_end(&mut self, bad: Option<&Tok>) {
        self.display_alignment_check(bad);
        let end = self.math_end(true);
        self.math_close(Some(end));
    }

    /// §1206: the alignment between `\predisplaypenalty` and
    /// `\abovedisplayskip`, `\postdisplaypenalty` and `\belowdisplayskip`
    /// (read in the display's group); the depth after it the alignment's;
    /// then the paragraph resumes (§1200).
    fn finish_display_alignment(&mut self, rows: &Arc<Vec<Item>>, aux: i32, end: &MathEnd) {
        let Some(de) = &end.display else { return };
        self.vemit.push(Item::Penalty(de.pre_penalty));
        self.vemit.push(param_glue(&de.skips[0], GluePar::Abovedisplayskip as u8));
        self.vemit.extend(rows.iter().cloned());
        self.vemit.push(Item::Penalty(de.post_penalty));
        self.vemit.push(param_glue(&de.skips[1], GluePar::Belowdisplayskip as u8));
        self.st.prev_depth = aux;
        self.resume_after_display();
    }

    /// `append_to_vlist` into the list the display is in.
    fn append_vlist(&mut self, b: Arc<BoxNode>, inter: &Arc<Interline>) {
        self.st.prev_depth = b.d;
        self.vemit.push(Item::Append(b, inter.clone()));
    }

    /// A display's `hpack(p, x, exactly)`, with its report.
    fn display_hpack(&mut self, list: Vec<Item>, x: i32, params: &Params) -> BoxNode {
        let packed = hpack(list, Spec::Exactly(x), params, None);
        if let Some(r) = packed.report {
            let cx = self.show_ctx(std::slice::from_ref(&Item::Box(Arc::new(packed.b.clone()))));
            let s = report(r, &packed.b, self.box_place(), &cx);
            self.log_raw(&s);
        }
        packed.b
    }
}
