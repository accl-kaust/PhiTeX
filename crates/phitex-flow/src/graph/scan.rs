//! Scanning numbers, dimensions, glue, keywords and token lists (tex.web
//! 402-486), copied as methods of one command's execution: they pull
//! tokens with `next_x_m`/`next_raw` and read every internal quantity with
//! `Exec::get`.

use std::sync::Arc;

use super::{Exec, Mode};
use crate::arith::{round_decimals, xn_over_d};
use crate::val::{Code, Glue, IntPar, Key, Meaning, Prim, Reg, Tok, Val};

/// `\maxdimen` (tex.web 421).
pub(super) const MAX_DIMEN: i32 = 0o7_777_777_777;
const INFINITY: i32 = 0o17_777_777_777;

/// The level of an internal quantity (tex.web 410).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Lv {
    Int,
    Dimen,
    Glue,
    Mu,
    Ident,
    Tok,
}

/// An internal quantity's value.
#[derive(Clone, Debug)]
pub(super) enum IVal {
    Int(i32),
    Dimen(i32),
    Glue(Glue),
    Mu(Glue),
    /// A font identifier (`\the\font`).
    Font(Arc<crate::tfm::Font>),
    Toks(Arc<Vec<Tok>>),
}

impl IVal {
    fn level(&self) -> Lv {
        match self {
            IVal::Int(_) => Lv::Int,
            IVal::Dimen(_) => Lv::Dimen,
            IVal::Glue(_) => Lv::Glue,
            IVal::Mu(_) => Lv::Mu,
            IVal::Font(_) => Lv::Ident,
            IVal::Toks(_) => Lv::Tok,
        }
    }
}

/// Whether a meaning is an internal quantity (tex.web 209:
/// `min_internal..max_internal`).
pub(super) fn is_internal(m: &Meaning) -> bool {
    match m {
        Meaning::Prim(p) => matches!(
            p,
            Prim::Code(_)
                | Prim::TokPar(_)
                | Prim::IntPar(_)
                | Prim::DimenPar(_)
                | Prim::GluePar(_)
                | Prim::Register(_)
                | Prim::SpaceFactor
                | Prim::PrevDepth
                | Prim::PrevGraf
                | Prim::PageInt(_)
                | Prim::PageDimen(_)
                | Prim::BoxDimen(_)
                | Prim::Parshape
                | Prim::FontDimen
                | Prim::LastItem(_)
                | Prim::Font
                | Prim::DefFamily(_)
                | Prim::Mat(crate::val::Mat::FontInt(_))
        ),
        Meaning::Font(_)
        | Meaning::CharGiven(_)
        | Meaning::MathCharGiven(_)
        | Meaning::CountRef(_)
        | Meaning::DimenRef(_)
        | Meaning::SkipRef(_)
        | Meaning::MuskipRef(_)
        | Meaning::ToksRef(_) => true,
        _ => false,
    }
}

fn is_spacer(m: &Meaning) -> bool {
    matches!(m, Meaning::Char(10, _))
}

/// `n*x + y` unless its magnitude exceeds `max` (tex.web 105).
pub(super) fn mult_and_add(mut n: i32, mut x: i32, y: i32, max: i32) -> Option<i32> {
    if n < 0 {
        x = x.checked_neg()?;
        n = n.checked_neg()?;
    }
    if n == 0 {
        return Some(y);
    }
    if x <= (max - y) / n && -x <= (max + y) / n { Some(n * x + y) } else { None }
}

pub(super) fn nx_plus_y(n: i32, x: i32, y: i32) -> Option<i32> {
    mult_and_add(n, x, y, 0o7_777_777_777)
}

/// `x / n`, truncated (tex.web 106).
pub(super) fn x_over_n(x: i32, n: i32) -> Option<i32> {
    if n == 0 {
        return None;
    }
    Some((i64::from(x) / i64::from(n)) as i32)
}

impl Exec<'_> {
    /// A character code table entry (tex.web 414).
    pub(super) fn code_key(c: Code, ch: char) -> Key {
        match c {
            Code::Cat => Key::Cat(ch),
            Code::Math => Key::Mathcode(ch),
            Code::Lc => Key::Lccode(ch),
            Code::Uc => Key::Uccode(ch),
            Code::Sf => Key::Sfcode(ch),
            Code::Del => Key::Delcode(ch),
        }
    }

    fn glue(&mut self, key: &Key) -> Glue {
        match self.get(key) {
            Val::Glue(g) => *g,
            Val::Int(w) => Glue { width: w, ..Glue::default() },
            _ => Glue::default(),
        }
    }

    fn toks(&mut self, key: &Key) -> Arc<Vec<Tok>> {
        match self.get(key) {
            Val::Toks(t) => t,
            _ => Arc::new(Vec::new()),
        }
    }

    /// The value of the internal quantity `t` (meaning `m`) begins, at
    /// most at `level` (tex.web 413, 429); `None` if it is none.
    pub(super) fn internal(&mut self, t: &Tok, m: &Meaning, level: Lv) -> Option<IVal> {
        let toks_ok = |x: &mut Self, l: Arc<Vec<Tok>>| {
            if level >= Lv::Ident {
                IVal::Toks(l)
            } else {
                x.err("Missing number, treated as zero".into());
                x.back(t.clone());
                IVal::Dimen(0)
            }
        };
        let v = match m {
            Meaning::Prim(p) => match *p {
                Prim::Code(c) => {
                    let n = self.scan_char_num();
                    let key = Self::code_key(c, char::from_u32(n as u32).unwrap_or('\0'));
                    IVal::Int(match self.get(&key) {
                        Val::Cat(k) => i32::from(k),
                        Val::Int(i) => i,
                        _ => 0,
                    })
                }
                Prim::TokPar(q) => {
                    if level != Lv::Tok {
                        return Some(toks_ok(self, Arc::new(Vec::new())));
                    }
                    IVal::Toks(self.toks(&Key::TokPar(q)))
                }
                Prim::Register(Reg::Toks) => {
                    if level != Lv::Tok {
                        return Some(toks_ok(self, Arc::new(Vec::new())));
                    }
                    let n = self.scan_eight_bit_int();
                    IVal::Toks(self.toks(&Key::Toks(n)))
                }
                Prim::IntPar(q) => IVal::Int(self.int_par(q)),
                Prim::DimenPar(q) => IVal::Dimen(self.dimen_par(q)),
                Prim::GluePar(q) => {
                    let g = self.glue(&Key::GluePar(q));
                    if q.is_mu() { IVal::Mu(g) } else { IVal::Glue(g) }
                }
                Prim::Register(r) => {
                    let n = self.scan_eight_bit_int();
                    let key = Key::reg(r, n);
                    match r {
                        Reg::Count => IVal::Int(self.int(&key)),
                        Reg::Dimen => IVal::Dimen(self.int(&key)),
                        Reg::Skip => IVal::Glue(self.glue(&key)),
                        _ => IVal::Mu(self.glue(&key)),
                    }
                }
                Prim::SpaceFactor | Prim::PrevDepth => {
                    let want = if *p == Prim::SpaceFactor { Mode::H } else { Mode::V };
                    if self.in_write || self.st.mode != want {
                        self.err(format!("Improper \\{}", p.name()));
                        if level == Lv::Tok { IVal::Int(0) } else { IVal::Dimen(0) }
                    } else if want == Mode::V {
                        IVal::Dimen(self.st.prev_depth)
                    } else {
                        IVal::Int(self.st.space_factor)
                    }
                }
                Prim::PrevGraf => IVal::Int(if self.in_write { 0 } else { self.st.prev_graf }),
                // (e-TeX's `\interactionmode`: a key)
                Prim::PageInt(2) => IVal::Int(self.int(&Key::Interaction)),
                // (the page builder's state: not a read of the reader yet,
                // CORE.md; an empty page's values)
                Prim::PageInt(_) => IVal::Int(0),
                Prim::PageDimen(k) => IVal::Dimen(if k == 0 { MAX_DIMEN } else { 0 }),
                Prim::BoxDimen(c) => IVal::Dimen(self.box_dimen(c)),
                Prim::Mat(crate::val::Mat::FontInt(k)) => {
                    let key = self.font_int_key(k);
                    IVal::Int(self.int(&key))
                }
                Prim::Parshape => IVal::Int(match self.get(&Key::Parshape) {
                    Val::Parshape(s) => s.len() as i32,
                    _ => 0,
                }),
                Prim::FontDimen => IVal::Dimen(match self.find_font_dimen() {
                    Some(k) => self.int(&k),
                    None => 0,
                }),
                Prim::LastItem(k) if k >= crate::prim::ETEX_ITEM => self.etex_item(k),
                Prim::LastItem(k) => {
                    // (tex.web 424: the current list's last item, the one read
                    // of material, CORE.md; `\inputlineno` and `\badness`
                    // not yet)
                    if k == 3 {
                        self.err(format!("\\{} is not supported yet", p.name()));
                        IVal::Int(0)
                    } else if k == 4 {
                        // (`last_badness`: the last box packed anywhere, a read
                        // of the packager's history; 0 before any, CORE.md)
                        IVal::Int(0)
                    } else if self.in_write {
                        if k == 2 { IVal::Glue(Glue::default()) } else { IVal::Int(0) }
                    } else {
                        let last = self.last_item();
                        match (k, last) {
                            (0, Some(crate::item::Item::Penalty(p))) => IVal::Int(p),
                            (0, _) => IVal::Int(0),
                            (1, Some(crate::item::Item::Kern { w, .. })) => IVal::Dimen(w),
                            (1, _) => IVal::Dimen(0),
                            (_, Some(crate::item::Item::Glue { g, sub, .. })) => {
                                if sub == 99 { IVal::Mu(*g) } else { IVal::Glue(*g) }
                            }
                            _ => IVal::Glue(Glue::default()),
                        }
                    }
                }
                Prim::Font => {
                    if level < Lv::Ident {
                        return Some(toks_ok(self, Arc::new(Vec::new())));
                    }
                    IVal::Font(self.font())
                }
                Prim::DefFamily(size) => {
                    let fam = self.scan_four_bit_int() as u8;
                    let f = self.fam_font(size, fam);
                    if level < Lv::Ident {
                        return Some(toks_ok(self, Arc::new(Vec::new())));
                    }
                    IVal::Font(f)
                }
                _ => return None,
            },
            Meaning::Font(f) => {
                if level < Lv::Ident {
                    return Some(toks_ok(self, Arc::new(Vec::new())));
                }
                IVal::Font(f.clone())
            }
            Meaning::CharGiven(c) | Meaning::MathCharGiven(c) => IVal::Int(*c as i32),
            Meaning::CountRef(n) => IVal::Int(self.int(&Key::Count(*n))),
            Meaning::DimenRef(n) => IVal::Dimen(self.int(&Key::Dimen(*n))),
            Meaning::SkipRef(n) => IVal::Glue(self.glue(&Key::Skip(*n))),
            Meaning::MuskipRef(n) => IVal::Mu(self.glue(&Key::Muskip(*n))),
            Meaning::ToksRef(n) => {
                if level != Lv::Tok {
                    return Some(toks_ok(self, Arc::new(Vec::new())));
                }
                IVal::Toks(self.toks(&Key::Toks(*n)))
            }
            _ => return None,
        };
        // (tex.web 429: down to the level asked for)
        let mut v = v;
        while v.level() > level {
            v = match v {
                IVal::Mu(g) => {
                    self.err("Incompatible glue units".into());
                    IVal::Glue(g)
                }
                IVal::Glue(g) => IVal::Dimen(g.width),
                IVal::Dimen(d) => IVal::Int(d),
                other => other,
            };
        }
        Some(v)
    }

    // ------------------------------------------------------------ small things

    /// tex.web 404: the next non-blank non-`\relax` token, expanded.
    pub(super) fn nonblank_nonrelax(&mut self) -> Option<(Tok, Meaning)> {
        loop {
            let x = self.next_x_m(false)?;
            if !is_spacer(&x.1) && x.1 != Meaning::Prim(Prim::Relax) {
                return Some(x);
            }
        }
    }

    pub(super) fn optional_equals(&mut self) {
        loop {
            match self.next_x_m(false) {
                Some((_, m)) if is_spacer(&m) => {}
                Some((t, _)) => {
                    if !t.is_char(12, '=') {
                        self.back(t);
                    }
                    return;
                }
                None => return,
            }
        }
    }

    fn optional_space(&mut self) {
        if let Some((t, m)) = self.next_x_m(false)
            && !is_spacer(&m)
        {
            self.back(t);
        }
    }

    /// Signs and spaces (tex.web 441), then the first token after them.
    fn signs(&mut self) -> (bool, Option<(Tok, Meaning)>) {
        let mut neg = false;
        loop {
            let x = self.next_x_m(false);
            match &x {
                Some((_, m)) if is_spacer(m) => {}
                Some((t, _)) if t.is_char(12, '-') => neg = !neg,
                Some((t, _)) if t.is_char(12, '+') => {}
                _ => return (neg, x),
            }
        }
    }

    /// Looks for a keyword (tex.web 407).
    pub(super) fn keyword(&mut self, kw: &str) -> bool {
        let mut matched: Vec<Tok> = Vec::new();
        for k in kw.chars() {
            loop {
                let Some((t, m)) = self.next_x_m(false) else {
                    for x in matched.into_iter().rev() {
                        self.back(x);
                    }
                    return false;
                };
                if matches!(t, Tok::Char(_, c) if c == k || c == k.to_ascii_uppercase()) {
                    matched.push(t);
                    break;
                }
                if is_spacer(&m) && matched.is_empty() {
                    continue;
                }
                self.back(t);
                for x in matched.into_iter().rev() {
                    self.back(x);
                }
                return false;
            }
        }
        true
    }

    fn limited(&mut self, max: i32, what: &str) -> i32 {
        let v = self.scan_int();
        if v < 0 || v > max {
            self.err(format!("Bad {what} ({v})"));
            0
        } else {
            v
        }
    }

    /// A register's number: up to 255, or 32767 in e-TeX's extended mode
    /// (etex.web §1495, `scan_register_num`; the mode is read only for a
    /// number past 255).
    pub(super) fn scan_eight_bit_int(&mut self) -> u16 {
        let v = self.scan_int();
        if (0..=255).contains(&v) || ((256..=32767).contains(&v) && self.etex_mode()) {
            return v as u16;
        }
        self.err(format!("Bad register code ({v})"));
        0
    }

    pub(super) fn scan_char_num(&mut self) -> i32 {
        self.limited(255, "character code")
    }

    pub(super) fn scan_four_bit_int(&mut self) -> i32 {
        self.limited(15, "number")
    }

    pub(super) fn scan_fifteen_bit_int(&mut self) -> i32 {
        self.limited(0o77777, "mathchar")
    }

    // ------------------------------------------------------------ numbers

    /// An integer (tex.web 440).
    pub(super) fn scan_int(&mut self) -> i32 {
        self.scan_int_ext().0
    }

    /// An integer, its radix (0: not a constant) and the token that ended
    /// a constant (put back, or a space).
    fn scan_int_ext(&mut self) -> (i32, u32, Option<Tok>) {
        let (neg, x) = self.signs();
        let Some((t, m)) = x else {
            self.err("Missing number, treated as zero".into());
            return (0, 0, None);
        };
        let v: i32;
        let mut radix = 0;
        let mut last = None;
        if t.is_char(12, '`') {
            // (tex.web 442: an alphabetic constant)
            let c = match self.next_raw() {
                Some(Tok::Char(_, c) | Tok::Active(c)) => Some(c),
                Some(Tok::Cs(n)) => {
                    let mut it = n.chars();
                    if let (Some(c), None) = (it.next(), it.next()) {
                        Some(c)
                    } else {
                        self.back(Tok::Cs(n.clone()));
                        None
                    }
                }
                _ => None,
            };
            match c {
                Some(c) if u32::from(c) <= 255 => {
                    v = u32::from(c) as i32;
                    self.optional_space();
                }
                _ => {
                    self.err("Improper alphabetic constant".into());
                    v = i32::from(b'0');
                }
            }
        } else if is_internal(&m) {
            let iv = self.internal(&t, &m, Lv::Int);
            let i = match iv {
                Some(IVal::Int(i)) => i,
                _ => 0,
            };
            return (if neg { i.wrapping_neg() } else { i }, 0, None);
        } else {
            // (tex.web 444: a constant)
            radix = 10u32;
            let mut max = 214_748_364i32;
            let mut cur = Some((t, m));
            if cur.as_ref().is_some_and(|(t, _)| t.is_char(12, '\'')) {
                radix = 8;
                max = 1 << 28;
                cur = self.next_x_m(false);
            } else if cur.as_ref().is_some_and(|(t, _)| t.is_char(12, '"')) {
                radix = 16;
                max = 1 << 27;
                cur = self.next_x_m(false);
            }
            let mut vacuous = true;
            let mut ok = true;
            let mut val = 0i32;
            while let Some((x, xm)) = cur.take() {
                let d = match &x {
                    Tok::Char(12, c) if c.is_ascii_digit() && c.to_digit(10).unwrap_or(10) < radix => c.to_digit(10),
                    Tok::Char(11 | 12, c) if radix == 16 && ('A'..='F').contains(c) => c.to_digit(16),
                    _ => None,
                };
                let Some(d) = d else {
                    if vacuous {
                        self.err("Missing number, treated as zero".into());
                        self.back(x.clone());
                    } else if !is_spacer(&xm) {
                        self.back(x.clone());
                    }
                    last = Some(x);
                    break;
                };
                vacuous = false;
                let d = d as i32;
                if val >= max && (val > max || d > 7 || radix != 10) {
                    if ok {
                        self.err("Number too big".into());
                        val = INFINITY;
                        ok = false;
                    }
                } else {
                    val = val * radix as i32 + d;
                }
                cur = self.next_x_m(false);
            }
            if vacuous && last.is_none() {
                self.err("Missing number, treated as zero".into());
            }
            v = val;
        }
        (if neg { v.wrapping_neg() } else { v }, radix, last)
    }

    // ------------------------------------------------------------ dimensions

    /// A dimension (tex.web 448).
    pub(super) fn scan_dimen(&mut self) -> i32 {
        self.scan_dimen_full(false, false, None).0
    }

    /// A dimension, in mu if `mu`, possibly infinite if `inf`; `shortcut`:
    /// the integer before the units is already scanned. Returns it and its
    /// order of infinity.
    fn scan_dimen_full(&mut self, mu: bool, inf: bool, shortcut: Option<i32>) -> (i32, u8) {
        // (where the units' scanning goes on: tex.web's labels)
        enum U {
            Fraction,
            Sign,
            Done,
        }
        let mut f = 0i32;
        let mut arith = false;
        let mut order = 0u8;
        let mut neg = false;
        let mut attach_sign = false;
        let mut v: i32;
        if let Some(x) = shortcut {
            v = x;
        } else {
            let (n, x) = self.signs();
            neg = n;
            let Some((t, m)) = x else {
                self.err("Missing number, treated as zero".into());
                return (0, 0);
            };
            if is_internal(&m) {
                // (tex.web 449)
                if mu {
                    match self.internal(&t, &m, Lv::Mu) {
                        Some(IVal::Int(i)) => v = i,
                        Some(IVal::Mu(g)) => {
                            v = g.width;
                            attach_sign = true;
                        }
                        Some(IVal::Glue(g)) => {
                            self.err("Incompatible glue units".into());
                            v = g.width;
                            attach_sign = true;
                        }
                        Some(IVal::Dimen(d)) => {
                            self.err("Incompatible glue units".into());
                            v = d;
                            attach_sign = true;
                        }
                        _ => v = 0,
                    }
                } else {
                    match self.internal(&t, &m, Lv::Dimen) {
                        Some(IVal::Dimen(d)) => {
                            v = d;
                            attach_sign = true;
                        }
                        Some(IVal::Int(i)) => v = i,
                        _ => v = 0,
                    }
                }
            } else {
                let point = t.is_char(12, '.') || t.is_char(12, ',');
                self.back(t);
                let (radix, last) = if point {
                    v = 0;
                    (10, Some(Tok::Char(12, '.')))
                } else {
                    let (x, r, l) = self.scan_int_ext();
                    v = x;
                    (r, l)
                };
                let at_point = last.as_ref().is_some_and(|l| l.is_char(12, '.') || l.is_char(12, ','));
                if radix == 10 && at_point {
                    f = self.decimal_fraction();
                }
            }
        }
        if !attach_sign {
            if v < 0 {
                neg = !neg;
                v = v.wrapping_neg();
            }
            // (tex.web 453-458: the units)
            let u = 'units: {
                if inf && self.keyword("fil") {
                    order = 1;
                    while self.keyword("l") {
                        if order == 3 {
                            self.err("Illegal unit of measure (replaced by filll)".into());
                        } else {
                            order += 1;
                        }
                    }
                    break 'units U::Fraction;
                }
                let save = v;
                let x = loop {
                    match self.next_x_m(false) {
                        Some((_, m)) if is_spacer(&m) => {}
                        other => break other,
                    }
                };
                if let Some((t, m)) = x {
                    if is_internal(&m) {
                        let unit = if mu {
                            match self.internal(&t, &m, Lv::Mu) {
                                Some(IVal::Mu(g)) => g.width,
                                Some(IVal::Glue(g)) => {
                                    self.err("Incompatible glue units".into());
                                    g.width
                                }
                                Some(IVal::Int(i) | IVal::Dimen(i)) => {
                                    self.err("Incompatible glue units".into());
                                    i
                                }
                                _ => 0,
                            }
                        } else {
                            match self.internal(&t, &m, Lv::Dimen) {
                                Some(IVal::Int(i) | IVal::Dimen(i)) => i,
                                _ => 0,
                            }
                        };
                        match nx_plus_y(save, unit, xn_over_d(unit, f, 0o200_000).0) {
                            Some(r) => v = r,
                            None => arith = true,
                        }
                        break 'units U::Sign;
                    }
                    self.back(t);
                }
                if mu {
                    if !self.keyword("mu") {
                        self.err("Illegal unit of measure (mu inserted)".into());
                    }
                    break 'units U::Fraction;
                }
                let em = if self.keyword("em") {
                    Some(6)
                } else if self.keyword("ex") {
                    Some(5)
                } else {
                    None
                };
                if let Some(k) = em {
                    let unit = self.font_param(k);
                    self.optional_space();
                    match nx_plus_y(save, unit, xn_over_d(unit, f, 0o200_000).0) {
                        Some(r) => v = r,
                        None => arith = true,
                    }
                    break 'units U::Sign;
                }
                if self.keyword("true") {
                    // (tex.web 457)
                    let mag = self.int_par(IntPar::Mag);
                    if mag != 1000 && mag > 0 {
                        let (q, r) = xn_over_d(v, 1000, mag);
                        f = ((1000 * i64::from(f) + 0o200_000 * i64::from(r)) / i64::from(mag)) as i32;
                        v = q + f / 0o200_000;
                        f %= 0o200_000;
                    }
                }
                if self.keyword("pt") {
                    break 'units U::Fraction;
                }
                let units: [(&str, i32, i32); 7] = [
                    ("in", 7227, 100),
                    ("pc", 12, 1),
                    ("cm", 7227, 254),
                    ("mm", 7227, 2540),
                    ("bp", 7227, 7200),
                    ("dd", 1238, 1157),
                    ("cc", 14856, 1157),
                ];
                for (kw, num, den) in units {
                    if self.keyword(kw) {
                        let (q, r) = xn_over_d(v, num, den);
                        f = ((i64::from(num) * i64::from(f) + 0o200_000 * i64::from(r)) / i64::from(den)) as i32;
                        v = q + f / 0o200_000;
                        f %= 0o200_000;
                        break 'units U::Fraction;
                    }
                }
                if self.keyword("sp") {
                    break 'units U::Done;
                }
                self.err("Illegal unit of measure (pt inserted)".into());
                U::Fraction
            };
            match u {
                U::Fraction => {
                    if v >= 0o40000 {
                        arith = true;
                    } else {
                        v = v * 65536 + f;
                    }
                    self.optional_space();
                }
                U::Done => self.optional_space(),
                U::Sign => {}
            }
        }
        if arith || v.unsigned_abs() >= 0o10_000_000_000 {
            self.err("Dimension too large".into());
            v = MAX_DIMEN;
        }
        (if neg { -v } else { v }, order)
    }

    /// The digits after a decimal point (tex.web 452); the point is next.
    fn decimal_fraction(&mut self) -> i32 {
        let _point = self.next_raw();
        let mut digits: Vec<u8> = Vec::new();
        while let Some((t, m)) = self.next_x_m(false) {
            match t {
                Tok::Char(12, c) if c.is_ascii_digit() => {
                    if digits.len() < 17 {
                        digits.push(c.to_digit(10).unwrap_or(0) as u8);
                    }
                }
                _ => {
                    if !is_spacer(&m) {
                        self.back(t);
                    }
                    break;
                }
            }
        }
        round_decimals(&digits)
    }

    // ------------------------------------------------------------ glue

    /// A glue specification, in mu if `mu` (tex.web 461).
    pub(super) fn scan_glue(&mut self, mu: bool) -> Glue {
        self.scratch.glue_shared = false;
        let level = if mu { Lv::Mu } else { Lv::Glue };
        let (neg, x) = self.signs();
        let Some((t, m)) = x else {
            self.err("Missing number, treated as zero".into());
            return Glue::default();
        };
        let width;
        if is_internal(&m) {
            match self.internal(&t, &m, level) {
                Some(IVal::Glue(g) | IVal::Mu(g)) => {
                    let is_mu = matches!(Self::internal_level_hint(&m), Lv::Mu);
                    if is_mu != mu {
                        self.err("Incompatible glue units".into());
                    }
                    self.scratch.glue_shared = !neg;
                    return if neg { Glue { width: -g.width, stretch: -g.stretch, shrink: -g.shrink, ..g } } else { g };
                }
                Some(IVal::Int(i)) => {
                    let i = if neg { i.wrapping_neg() } else { i };
                    width = self.scan_dimen_full(mu, false, Some(i)).0;
                }
                Some(IVal::Dimen(d)) => {
                    if mu {
                        self.err("Incompatible glue units".into());
                    }
                    width = if neg { -d } else { d };
                }
                _ => width = 0,
            }
        } else {
            self.back(t);
            let d = self.scan_dimen_full(mu, false, None).0;
            width = if neg { -d } else { d };
        }
        let mut g = Glue { width, ..Glue::default() };
        if self.keyword("plus") {
            let (s, o) = self.scan_dimen_full(mu, true, None);
            g.stretch = s;
            g.stretch_order = o;
        }
        if self.keyword("minus") {
            let (s, o) = self.scan_dimen_full(mu, true, None);
            g.shrink = s;
            g.shrink_order = o;
        }
        g
    }

    /// Whether a glue quantity is in mu (without reading it).
    fn internal_level_hint(m: &Meaning) -> Lv {
        match m {
            Meaning::Prim(Prim::GluePar(q)) if q.is_mu() => Lv::Mu,
            // (and e-TeX's `\gluetomu`, `\muexpr`)
            Meaning::Prim(Prim::Register(Reg::Muskip) | Prim::LastItem(24 | 28)) | Meaning::MuskipRef(_) => Lv::Mu,
            _ => Lv::Glue,
        }
    }

    // ------------------------------------------------------------ token lists

    /// tex.web 403: a left brace, after blanks and `\relax`es, expanded.
    pub(super) fn scan_left_brace(&mut self) {
        match self.nonblank_nonrelax() {
            Some((_, Meaning::Char(1, _))) => {}
            other => {
                self.err("Missing { inserted".into());
                if let Some((t, _)) = other {
                    self.back(t);
                }
            }
        }
    }

    /// A balanced text, unexpanded (`scan_toks(false, false)`, tex.web 473).
    pub(super) fn scan_toks_raw(&mut self) -> Vec<Tok> {
        self.scan_left_brace();
        self.balanced_raw()
    }

    /// The tokens up to the brace closing the one just read, unexpanded.
    pub(super) fn balanced_raw(&mut self) -> Vec<Tok> {
        let mut out = Vec::new();
        let mut depth = 1;
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning text".into());
                return out;
            };
            match t.cat() {
                Some(1) => depth += 1,
                Some(2) => {
                    depth -= 1;
                    if depth == 0 {
                        return out;
                    }
                }
                _ => {}
            }
            out.push(t);
        }
    }

    /// A balanced text, expanded (`scan_toks(false, true)`).
    pub(super) fn scan_toks_x(&mut self) -> Vec<Tok> {
        self.scan_left_brace();
        self.expand_text(None)
    }
}
