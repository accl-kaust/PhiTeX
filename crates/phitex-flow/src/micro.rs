//! pdfTeX's font expansion and character protrusion (pdftex.web:
//! `char_stretch`, `char_shrink`, `kern_stretch`, `kern_shrink`,
//! `do_subst_font`, `char_pw`, `find_protchar_left`/`right`, `total_pw`,
//! `hpack`'s `cal_expand_ratio` and `subst_ex_font` modes), copied as pure
//! functions of a paragraph's list and what its `\par` node read
//! ([`Micro`]): the line breaker calls them where `try_break` and
//! `post_line_break` do.
//!
//! A font's codes (`\efcode`, `\lpcode`, `\rpcode`, …) are a table per
//! font and code (`Key::FontCode`), its `\pdffontexpand` parameters one
//! value (`Key::FontExpand`); the `\par` node reads them for the fonts of
//! its paragraph only when `\pdfadjustspacing` or `\pdfprotrudechars` is
//! positive. An expanded font is a value made from its base and the
//! amount (`Font::expanded`): every font is taken as `autoexpand` (the
//! expanded TFM files a font without it names are not read: CORE.md).

use std::sync::Arc;

use crate::item::{Disc, Item, Lig};
use crate::pack::{Packed, Params, Spec, hpack};
use crate::tfm::Font;

/// `\lpcode`, …: pdftex.web's `*_code_base`, `Mat::FontInt`'s codes.
pub const LP: u8 = 2;
pub const RP: u8 = 3;
pub const EF: u8 = 4;
pub const TAG: u8 = 5;
/// `micro::TAG`'s bits: the lig/kern program's, the charlist's, the
/// extensible recipe's tag cleared.
pub const TAG_LIG: i32 = 1;
pub const TAG_LIST: i32 = 2;
pub const TAG_EXT: i32 = 4;
pub const NOLIG: u8 = 6;
pub const KNBS: u8 = 7;
pub const STBS: u8 = 8;
pub const SHBS: u8 = 9;
pub const KNBC: u8 = 10;
pub const KNAC: u8 = 11;

/// The codes' primitives' names, by code.
pub const CODE_NAMES: [&str; 12] = [
    "hyphenchar",
    "skewchar",
    "lpcode",
    "rpcode",
    "efcode",
    "tagcode",
    "pdfnoligatures",
    "knbscode",
    "stbscode",
    "shbscode",
    "knbccode",
    "knaccode",
];

/// A code's value before it is given (`init_font_base`): 1000 for
/// `\efcode`, else 0.
pub fn code_default(kind: u8) -> i32 {
    if kind == EF { 1000 } else { 0 }
}

/// The range a code is clamped to (`fix_int` in `set_*_code`).
pub fn code_range(kind: u8) -> (i32, i32) {
    if kind == EF { (0, 1000) } else { (-1000, 1000) }
}

/// Character `c`'s code in a font's table (empty: the defaults).
pub fn code(table: &[i32], kind: u8, c: u8) -> i32 {
    table.get(usize::from(c)).copied().unwrap_or_else(|| code_default(kind))
}

/// The table with character `c`'s code set to `v` (clamped).
pub fn set_code(table: &[i32], kind: u8, c: u8, v: i32) -> Arc<[i32]> {
    let mut t: Vec<i32> = if table.is_empty() { vec![code_default(kind); 256] } else { table.to_vec() };
    let (lo, hi) = code_range(kind);
    t[usize::from(c)] = v.clamp(lo, hi);
    t.into()
}

/// `\tagcode f c`'s value (`get_tag_code`): character `c`'s tag after the
/// tags `cleared` (1 a lig/kern program, 2 a charlist, 4 an extensible
/// recipe, 0 none), -1 if the font has no such character.
pub fn tag_code(f: &Font, cleared: &[i32], c: i32) -> i32 {
    let Some(g) = f.glyph(c) else { return -1 };
    let gone = cleared.get(c as usize).copied().unwrap_or(0);
    match g.tag {
        crate::tfm::Tag::Lig(_) if gone & TAG_LIG == 0 => 1,
        crate::tfm::Tag::List(_) if gone & TAG_LIST == 0 => 2,
        crate::tfm::Tag::Ext(_) if gone & TAG_EXT == 0 => 4,
        _ => 0,
    }
}

/// A font's `\pdffontexpand` parameters: the stretch and shrink limits
/// (multiples of the step), the step, `autoexpand`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expand {
    pub stretch: i32,
    pub shrink: i32,
    pub step: i32,
    pub auto: bool,
}

impl Expand {
    /// From `Key::FontExpand`'s value (empty: not expanded).
    pub fn of(v: &[i32]) -> Option<Expand> {
        match *v {
            [stretch, shrink, step, auto] => Some(Expand { stretch, shrink, step, auto: auto != 0 }),
            _ => None,
        }
    }

    pub fn val(self) -> Arc<[i32]> {
        Arc::from([self.stretch, self.shrink, self.step, i32::from(self.auto)])
    }

    /// `check_expand_pars`: whether the font is expandable.
    pub fn expandable(&self) -> bool {
        self.step != 0 && (self.stretch > 0 || self.shrink > 0)
    }

    /// `fix_expand_value`: the multiple of the step nearest to `e`, within
    /// the limits.
    pub fn fix(&self, e: i32) -> i32 {
        if e == 0 {
            return 0;
        }
        let neg = e < 0;
        let mut a = e.abs();
        let max = if neg { self.shrink } else { self.stretch };
        if a > max {
            a = max;
        } else if self.step > 0 && a % self.step > 0 {
            a = self.step * Font::round_xn_over_d(a, 1, self.step);
        }
        if neg { -a } else { a }
    }
}

/// What line breaking reads of a font for expansion and protrusion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MicroFont {
    pub font: Arc<Font>,
    /// `\fontdimen6` as it is now (`quad`).
    pub quad: i32,
    pub ef: Arc<[i32]>,
    pub lp: Arc<[i32]>,
    pub rp: Arc<[i32]>,
    pub expand: Option<Expand>,
}

/// What the `\par` node read for expansion and protrusion: the
/// parameters and the paragraph's fonts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Micro {
    pub adjust_spacing: i32,
    pub protrude_chars: i32,
    pub fonts: Vec<MicroFont>,
    /// `check_expand_pars`' `max_stretch_ratio`, `max_shrink_ratio`,
    /// `cur_font_step` over the paragraph's expandable fonts (-1: none).
    pub max_stretch: i32,
    pub max_shrink: i32,
    pub step: i32,
}

/// The expanded fonts a paragraph's lines are set in, made once each.
#[derive(Default)]
pub struct Cache(Vec<(Arc<str>, i32, Arc<Font>)>);

impl Cache {
    /// `base` expanded by `e` thousandths.
    pub fn get(&mut self, base: &Arc<Font>, e: i32) -> Arc<Font> {
        if let Some((_, _, f)) = self.0.iter().find(|(k, x, _)| *x == e && **k == *base.key) {
            return f.clone();
        }
        let f = Arc::new(Font::expanded(base, e));
        self.0.push((base.key.clone(), e, f.clone()));
        f
    }
}

/// A character item's font and character (a ligature's own character).
pub fn char_of(it: &Item) -> Option<(&Arc<Font>, u8)> {
    match it {
        Item::Char(f, c) => Some((f, *c)),
        Item::Lig(l) => Some((&l.font, l.c)),
        _ => None,
    }
}

/// pdftex.web's `ext_xn_over_d`: `x*n/d` rounded, in floating point.
pub fn ext_xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    let mut r = f64::from(x) * f64::from(n) / f64::from(d);
    if r > f64::EPSILON {
        r += 0.5;
    } else {
        r -= 0.5;
    }
    r as i32
}

/// pdftex.web's `divide_scaled(s, m, dd)`: `s/m` rounded to `dd` decimal
/// digits, times `10^dd`.
pub fn divide_scaled(s: i32, m: i32, dd: u32) -> i32 {
    let mut sign = 1i64;
    let (mut s, mut m) = (i64::from(s), i64::from(m));
    if s < 0 {
        sign = -sign;
        s = -s;
    }
    if m < 0 {
        sign = -sign;
        m = -m;
    }
    if m == 0 {
        return 0;
    }
    let mut q = s / m;
    let mut r = s % m;
    for _ in 0..dd {
        q = 10 * q + (10 * r) / m;
        r = (10 * r) % m;
    }
    if 2 * r >= m {
        q += 1;
    }
    (sign * q) as i32
}

/// `cp_skipable`: an item protrusion looks past at a line's edge.
pub fn skipable(it: &Item) -> bool {
    match it {
        // (whatsits too but references to images and forms, which are not
        // made here yet)
        Item::Ins(_)
        | Item::Mark(_)
        | Item::Marks(..)
        | Item::Adjust(_)
        | Item::Penalty(_)
        | Item::Whatsit(_)
        | Item::Lang { .. } => true,
        Item::Disc(d) => d.pre.is_empty() && d.post.is_empty() && d.replace == 0,
        Item::Math { w, .. } => *w == 0,
        Item::Kern { w, sub } => *w == 0 || *sub == 0 || *sub == 3,
        Item::Glue { zero, .. } => *zero,
        Item::Leaders(l) => l.zero,
        Item::Box(b) => !b.vertical && b.w == 0 && b.h == 0 && b.d == 0 && b.list.is_empty(),
        _ => false,
    }
}

/// `find_protchar_left`'s search from item `i` on: the first item not
/// skipable, an hbox with a list being looked into (`None`: none).
fn first_unskipable(list: &[Item]) -> Option<&Item> {
    for it in list {
        if let Item::Box(b) = it
            && !b.vertical
            && !b.list.is_empty()
        {
            if let Some(x) = first_unskipable(&b.list) {
                return Some(x);
            }
            continue;
        }
        if !skipable(it) {
            return Some(it);
        }
    }
    None
}

/// `find_protchar_right`'s search back from the end of `list`.
fn last_unskipable(list: &[Item]) -> Option<&Item> {
    for it in list.iter().rev() {
        if let Item::Box(b) = it
            && !b.vertical
            && !b.list.is_empty()
        {
            if let Some(x) = last_unskipable(&b.list) {
                return Some(x);
            }
            continue;
        }
        if !skipable(it) {
            return Some(it);
        }
    }
    None
}

/// `find_protchar_left(l, d)` on `list[l..]`: past an empty first box, or
/// (`d`) past the discardable items a break drops; then the first item
/// not skipable.
pub fn protchar_left(list: &[Item], l: usize, d: bool) -> Option<&Item> {
    let mut l = l;
    let empty_box = |it: &Item| matches!(it, Item::Box(b) if !b.vertical && b.w == 0 && b.h == 0 && b.d == 0 && b.list.is_empty());
    if l + 1 < list.len() && empty_box(&list[l]) {
        l += 1;
    } else if d {
        while l + 1 < list.len() && !list[l].non_discardable() {
            l += 1;
        }
    }
    first_unskipable(list.get(l..)?)
}

/// `find_protchar_right(l, r)`: back from `list[r]` to `list[l]`.
pub fn protchar_right(list: &[Item], l: usize, r: usize) -> Option<&Item> {
    if r >= list.len() || l > r {
        return None;
    }
    last_unskipable(&list[l..=r])
}

impl Micro {
    /// The paragraph's entry for a font (an expanded font's base's).
    pub fn font(&self, f: &Font) -> Option<&MicroFont> {
        let k = f.ident_key();
        self.fonts.iter().find(|m| Arc::ptr_eq(&m.font.key, k) || *m.font.key == **k)
    }

    fn expand(&self, f: &Font) -> Option<(&MicroFont, Expand)> {
        let m = self.font(f)?;
        Some((m, m.expand?))
    }

    /// `check_expand_pars`.
    pub fn expandable(&self, f: &Font) -> bool {
        self.expand(f).is_some_and(|(_, e)| e.expandable())
    }

    fn ef(&self, f: &Font, c: u8) -> i32 {
        self.font(f).map_or(1000, |m| code(&m.ef, EF, c))
    }

    /// `char_stretch`.
    pub fn char_stretch(&self, f: &Font, c: u8) -> i32 {
        let Some((m, e)) = self.expand(f) else { return 0 };
        let ef = code(&m.ef, EF, c);
        if e.stretch <= 0 || ef <= 0 {
            return 0;
        }
        let w = f.glyph(i32::from(c)).map_or(0, |g| g.width);
        let dw = Font::round_xn_over_d(w, 1000 + e.stretch, 1000) - w;
        if dw > 0 { Font::round_xn_over_d(dw, ef, 1000) } else { 0 }
    }

    /// `char_shrink`.
    pub fn char_shrink(&self, f: &Font, c: u8) -> i32 {
        let Some((m, e)) = self.expand(f) else { return 0 };
        let ef = code(&m.ef, EF, c);
        if e.shrink <= 0 || ef <= 0 {
            return 0;
        }
        let w = f.glyph(i32::from(c)).map_or(0, |g| g.width);
        let dw = w - Font::round_xn_over_d(w, 1000 - e.shrink, 1000);
        if dw > 0 { Font::round_xn_over_d(dw, ef, 1000) } else { 0 }
    }

    /// `kern_stretch` (`stretch`) or `kern_shrink` of a font's kern of
    /// width `w` between `prev` and `next` (the items around it; the kern
    /// counts only between two characters of one font that has the limit).
    pub fn kern_var(&self, prev: Option<&Item>, w: i32, next: Option<&Item>, stretch: bool) -> i32 {
        let (Some((lf, lc)), Some((rf, rc))) = (prev.and_then(char_of), next.and_then(char_of)) else { return 0 };
        if **lf != **rf {
            return 0;
        }
        let Some((m, e)) = self.expand(lf) else { return 0 };
        let limit = if stretch { e.stretch } else { -e.shrink };
        if limit == 0 {
            return 0;
        }
        // (lf is the base font here: the kern in the font expanded by the limit)
        let d = Font::round_xn_over_d(lf.get_kern(lc, rc), 1000 + limit, 1000);
        let ef = code(&m.ef, EF, lc);
        Font::round_xn_over_d(if stretch { d - w } else { w - d }, ef, 1000)
    }

    /// The font stretch and shrink item `i` of `list` adds where the line
    /// breaker passes it (`\pdfadjustspacing` above 1): a character's of
    /// an expandable font, a font kern's after one.
    pub fn widths_at(&self, list: &[Item], i: usize) -> (i64, i64) {
        match &list[i] {
            Item::Char(f, c) if self.expandable(f) => {
                (i64::from(self.char_stretch(f, *c)), i64::from(self.char_shrink(f, *c)))
            }
            Item::Lig(l) if self.expandable(&l.font) => {
                (i64::from(self.char_stretch(&l.font, l.c)), i64::from(self.char_shrink(&l.font, l.c)))
            }
            Item::Kern { w, sub: 0 } => {
                // (`prev_char_p`: the item before, a character of an
                // expandable font)
                let prev = i.checked_sub(1).map(|j| &list[j]).filter(|p| char_of(p).is_some_and(|(f, _)| self.expandable(f)));
                let next = list.get(i + 1);
                (i64::from(self.kern_var(prev, *w, next, true)), i64::from(self.kern_var(prev, *w, next, false)))
            }
            _ => (0, 0),
        }
    }

    /// `char_pw`: the protrusion of a character item at the `right` or
    /// left edge, with the character.
    pub fn char_pw(&self, it: Option<&Item>, right: bool) -> (i32, Option<(Arc<Font>, u8)>) {
        let Some((f, c)) = it.and_then(char_of) else { return (0, None) };
        let Some(m) = self.font(f) else { return (0, Some((f.clone(), c))) };
        let k = code(if right { &m.rp } else { &m.lp }, if right { RP } else { LP }, c);
        if k == 0 {
            return (0, Some((f.clone(), c)));
        }
        (Font::round_xn_over_d(m.quad, k, 1000), Some((f.clone(), c)))
    }

    /// `do_subst_font` of a character: the font it is set in for the
    /// line's expansion ratio `ratio` (`None`: its own).
    pub fn subst(&self, cache: &mut Cache, f: &Arc<Font>, c: u8, ratio: i32) -> Option<Arc<Font>> {
        let ef = self.ef(f, c);
        if ef == 0 {
            return None;
        }
        let (_, e) = self.expand(f)?;
        // (a font expanded already is not expanded again: no limits)
        if f.expanded_from().is_some() {
            return None;
        }
        let x = if e.stretch > 0 && ratio > 0 {
            ext_xn_over_d(ratio * ef, e.stretch, 1_000_000)
        } else if e.shrink > 0 && ratio < 0 {
            ext_xn_over_d(ratio * ef, e.shrink, 1_000_000)
        } else {
            return None;
        };
        let x = e.fix(x);
        (x != 0).then(|| cache.get(f, x))
    }

    fn subst_item(&self, cache: &mut Cache, it: &Item, ratio: i32) -> Option<Item> {
        match it {
            Item::Char(f, c) => self.subst(cache, f, *c, ratio).map(|k| Item::Char(k, *c)),
            Item::Lig(l) => {
                self.subst(cache, &l.font, l.c, ratio).map(|k| Item::Lig(Arc::new(Lig { font: k, ..(**l).clone() })))
            }
            _ => None,
        }
    }

    /// `total_pw`: the protrusion of a line from the break at
    /// `items[from]` (`None`: the paragraph's start) to the break at
    /// `items[cur]` (`None`: its end).
    pub fn total_pw(&self, items: &[Item], from: Option<usize>, cur: Option<usize>) -> i64 {
        let l = from.unwrap_or(0);
        // (the right edge: a discretionary's pre-break list's last item, or
        // back from the item before the break; at the end, from the last)
        let r = match cur.map(|b| &items[b]) {
            Some(Item::Disc(d)) if !d.pre.is_empty() => d.pre.last(),
            _ => {
                let rp = match cur {
                    Some(b) => b.checked_sub(1),
                    None => items.len().checked_sub(1),
                };
                rp.and_then(|rp| protchar_right(items, l, rp))
            }
        };
        let left = match items.get(l) {
            Some(Item::Disc(d)) if !d.post.is_empty() => d.post.first(),
            Some(Item::Disc(d)) => {
                let mut k = l;
                for _ in 0..=usize::from(d.replace) {
                    if k + 1 < items.len() {
                        k += 1;
                    }
                }
                protchar_left(items, k, true)
            }
            _ => protchar_left(items, l, true),
        };
        i64::from(self.char_pw(left, false).0) + i64::from(self.char_pw(r, true).0)
    }

    /// `post_line_break`'s margin kerns (`\pdfprotrudechars` positive):
    /// the right one after item `ptmp` of `line` (the protruding character
    /// found back from it, or — `direct` — that item itself), then the left
    /// one at the line's start: where the right one went, and whether the
    /// left one did.
    pub fn margin_kerns(&self, line: &mut Vec<Item>, ptmp: Option<usize>, direct: bool) -> (Option<usize>, bool) {
        let mut right = None;
        if let Some(pt) = ptmp {
            let found = if direct { line.get(pt) } else { protchar_right(line, 0, pt) };
            let (w, ch) = self.char_pw(found, true);
            if w != 0
                && let Some((font, c)) = ch
            {
                line.insert(pt + 1, Item::MarginKern { w: -w, right: true, font, c });
                right = Some(pt + 1);
            }
        }
        let (w, ch) = self.char_pw(protchar_left(line, 0, false), false);
        if w != 0
            && let Some((font, c)) = ch
        {
            line.insert(0, Item::MarginKern { w: -w, right: false, font, c });
            return (right, true);
        }
        (right, false)
    }

    /// `hpack(q, w, cal_expand_ratio)`: a line packed to width `w`, its
    /// fonts expanded by the ratio its excess and its font stretch or
    /// shrink give (`subst_ex_font`), or as `hpack` packs it.
    pub fn hpack_line(&self, list: Vec<Item>, w: i32, params: &Params, adjust: &mut Vec<Item>, cache: &mut Cache) -> Packed {
        // (the material moving out of the line moves out in the first pass)
        let (moved, mut list): (Vec<Item>, Vec<Item>) =
            list.into_iter().partition(|i| matches!(i, Item::Mark(_) | Item::Marks(..) | Item::Ins(_) | Item::Adjust(_)));
        for m in moved {
            match m {
                Item::Adjust(l) => adjust.extend(l.iter().cloned()),
                other => adjust.push(other),
            }
        }
        // (the first pass: the natural width, the glue, the font stretch and
        // shrink)
        let (mut x, mut fs, mut fsh) = (0i64, 0i64, 0i64);
        let mut stretch = [0i64; 4];
        let mut shrink = [0i64; 4];
        for (i, it) in list.iter().enumerate() {
            match it {
                Item::Char(f, c) => {
                    fs += i64::from(self.char_stretch(f, *c));
                    fsh += i64::from(self.char_shrink(f, *c));
                }
                Item::Lig(l) => {
                    fs += i64::from(self.char_stretch(&l.font, l.c));
                    fsh += i64::from(self.char_shrink(&l.font, l.c));
                }
                Item::MarginKern { w, right, font, c } => {
                    if let Some(k) = self.subst(cache, font, *c, 1000) {
                        fs -= i64::from(*w) + i64::from(self.char_pw(Some(&Item::Char(k, *c)), *right).0);
                    }
                    if let Some(k) = self.subst(cache, font, *c, -1000) {
                        fsh -= i64::from(*w) + i64::from(self.char_pw(Some(&Item::Char(k, *c)), *right).0);
                    }
                }
                Item::Kern { w, sub: 0 } => {
                    let prev = i.checked_sub(1).map(|j| &list[j]);
                    fs += i64::from(self.kern_var(prev, *w, list.get(i + 1), true));
                    fsh += i64::from(self.kern_var(prev, *w, list.get(i + 1), false));
                }
                _ => {}
            }
            if let Some(g) = it.glue() {
                stretch[usize::from(g.stretch_order.min(3))] += i64::from(g.stretch);
                shrink[usize::from(g.shrink_order.min(3))] += i64::from(g.shrink);
            }
            x += i64::from(it.width());
        }
        let excess = i64::from(w) - x;
        let normal = |t: &[i64; 4]| t[3] == 0 && t[2] == 0 && t[1] == 0;
        let ratio = if excess > 0 && normal(&stretch) && fs > 0 {
            Some(divide_scaled(clamp(excess), clamp(fs), 3))
        } else if excess < 0 && normal(&shrink) && fsh > 0 {
            Some(divide_scaled(clamp(excess), clamp(fsh), 3))
        } else {
            None
        };
        let Some(ratio) = ratio else {
            return hpack(list, Spec::Exactly(w), params, None);
        };
        let ratio = ratio.clamp(-1000, 1000);
        if ratio == 0 {
            // (the first pass returned its box before setting its glue)
            let mut p = hpack(list, Spec::Exactly(w), &Params::QUIET, None);
            p.b.glue = crate::item::GlueSet::default();
            p.badness = 0;
            p.report = None;
            return p;
        }
        // (the second pass: the fonts substituted)
        for i in 0..list.len() {
            let new = match &list[i] {
                Item::Char(..) | Item::Lig(_) => self.subst_item(cache, &list[i], ratio),
                Item::Disc(d) => {
                    let sub = |l: &[Item], cache: &mut Cache| -> Vec<Item> {
                        l.iter().map(|it| self.subst_item(cache, it, ratio).unwrap_or_else(|| it.clone())).collect()
                    };
                    let pre = sub(&d.pre, cache);
                    let post = sub(&d.post, cache);
                    (pre != d.pre || post != d.post).then(|| Item::Disc(Arc::new(Disc { pre, post, replace: d.replace })))
                }
                Item::MarginKern { right, font, c, .. } => {
                    let k = self.subst(cache, font, *c, ratio).unwrap_or_else(|| font.clone());
                    let w = -self.char_pw(Some(&Item::Char(k.clone(), *c)), *right).0;
                    Some(Item::MarginKern { w, right: *right, font: k, c: *c })
                }
                Item::Kern { w, sub: 0 } => {
                    let prev = i.checked_sub(1).map(|j| &list[j]);
                    let next = list.get(i + 1);
                    let k = self.kern_var(prev, *w, next, ratio > 0);
                    match (k, prev.and_then(char_of), next.and_then(char_of)) {
                        (k, Some((pf, pc)), Some((_, nc))) if k != 0 => Some(Item::Kern { w: pf.get_kern(pc, nc), sub: 0 }),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(n) = new {
                list[i] = n;
            }
        }
        hpack(list, Spec::Exactly(w), params, None)
    }
}

fn clamp(x: i64) -> i32 {
    x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}
