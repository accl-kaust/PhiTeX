//! Displaying material as TeX does in its log (tex.web 173–198: the short
//! display and `show_box`), and the diagnostics of packaging (tex.web
//! 660–678), as text for `print::log`.
//!
//! A log entry made here is "raw": it begins with [`RAW`] and may hold
//! [`LN`] (`print_ln`), [`NL`] (`print_nl`'s new line if the line is not
//! empty) and a line-number placeholder ([`LINE`] `P` or `L` [`LINE`]:
//! the paragraph's lines `a--b`, the current line) that the log's
//! gatherer fills in.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::item::{BoxNode, C_LEADERS, Item, RUNNING, Whatsit, X_LEADERS};
use crate::mlist::{Delim, Field, Kind as NK, Limits, MItem};
use crate::pack::Report;
use crate::print;
use crate::val::Tok;

pub const RAW: char = '\u{3}';
pub const LN: char = '\u{1}';
pub const NL: char = '\u{2}';
pub const LINE: char = '\u{4}';
/// A space if the log's line is not empty (tex.web 638's `[`).
pub const SP: char = '\u{6}';
/// An error's context line (tex.web 311–318): `CTX read CTX unread CTX`
/// after its label (`l.N `, `<template> `…), laid out as TeX's two lines
/// once the label's length is known (`print::log`).
pub const CTX: char = '\u{7}';
/// tex.web 82's `jump_out` after the hundredth error: the log ends here
/// (no file is closed: no ` )`).
pub const HALT: char = '\u{8}';
pub const THAT_MAKES_100: &str = "(That makes 100 errors; please try again.)";

/// What a display reads: the escape character, the font identifiers, and
/// `\showboxdepth`/`\showboxbreadth`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Ctx {
    pub esc: i32,
    /// Each font's name (`Font::key`) and the name it was last defined by.
    pub idents: Vec<(Arc<str>, Arc<str>)>,
    pub depth: i32,
    pub breadth: i32,
    /// `\newlinechar`: printing that character ends the line (tex.web 59).
    pub newline: i32,
}

impl Ctx {
    fn esc(&self, out: &mut String, s: &str) {
        if let Some(c) = u8::try_from(self.esc).ok().map(char::from) {
            out.push(c);
        }
        out.push_str(s);
    }
    /// tex.web 59 `print` of a character: the new-line character ends the
    /// line.
    fn chr(&self, out: &mut String, c: u8) {
        if i32::from(c) == self.newline {
            out.push(LN);
        } else {
            out.push(char::from(c));
        }
    }
    fn font_ident(&self, out: &mut String, key: &str) {
        let name = self.idents.iter().find(|(i, _)| &**i == key).map_or("FONT?", |(_, n)| &**n);
        self.esc(out, name);
    }
}

/// §174: `short_display`.
pub fn short_display(out: &mut String, list: &[Item], font: &mut Arc<str>, cx: &Ctx) {
    let mut skip = 0usize;
    for it in list {
        if skip > 0 {
            // (a discretionary's replacement is not shown)
            skip -= 1;
            continue;
        }
        match it {
            Item::Char(f, c) => {
                if f.key != *font {
                    cx.font_ident(out, &f.key);
                    out.push(' ');
                    font.clone_from(&f.key);
                }
                cx.chr(out, *c);
            }
            Item::Box(_)
            | Item::Mark(_)
            | Item::Marks(..)
            | Item::Lang { .. }
            | Item::Ins(_)
            | Item::Whatsit(_)
            | Item::Adjust(_)
            | Item::Unset(_) => out.push_str("[]"),
            // (§175, etex.web: a TeXXeT node as `[]`, a formula's edge as `$`)
            Item::Math { lr, .. } => out.push_str(if *lr >= 4 { "[]" } else { "$" }),
            Item::Rule { .. } => out.push('|'),
            Item::Glue { zero, .. } => {
                if !zero {
                    out.push(' ');
                }
            }
            Item::Leaders(l) => {
                if !l.zero {
                    out.push(' ');
                }
            }
            Item::Lig(l) => {
                for &c in &l.orig {
                    if l.font.key != *font {
                        cx.font_ident(out, &l.font.key);
                        out.push(' ');
                        font.clone_from(&l.font.key);
                    }
                    cx.chr(out, c);
                }
            }
            Item::Disc(d) => {
                short_display(out, &d.pre, font, cx);
                short_display(out, &d.post, font, cx);
                skip = usize::from(d.replace);
            }
            _ => {}
        }
    }
}

/// §178: `print_glue`.
fn glue_part(out: &mut String, d: i32, order: u8, unit: &str) {
    print::scaled(out, d);
    if order > 3 {
        out.push_str("foul");
    } else if order > 0 {
        out.push_str("fil");
        for _ in 1..order {
            out.push('l');
        }
    } else {
        out.push_str(unit);
    }
}

fn spec(out: &mut String, g: &crate::val::Glue, unit: &str) {
    print::scaled(out, g.width);
    out.push_str(unit);
    if g.stretch != 0 {
        out.push_str(" plus ");
        glue_part(out, g.stretch, g.stretch_order, unit);
    }
    if g.shrink != 0 {
        out.push_str(" minus ");
        glue_part(out, g.shrink, g.shrink_order, unit);
    }
}

fn rule_dim(out: &mut String, d: i32) {
    if d == RUNNING {
        out.push('*');
    } else {
        print::scaled(out, d);
    }
}

/// The names of the glue parameters, by number (tex.web 225).
const SKIPS: [&str; 18] = [
    "lineskip",
    "baselineskip",
    "parskip",
    "abovedisplayskip",
    "belowdisplayskip",
    "abovedisplayshortskip",
    "belowdisplayshortskip",
    "leftskip",
    "rightskip",
    "topskip",
    "splittopskip",
    "tabskip",
    "spaceskip",
    "xspaceskip",
    "parfillskip",
    "thinmuskip",
    "medmuskip",
    "thickmuskip",
];

struct Shower<'a> {
    out: &'a mut String,
    cx: &'a Ctx,
    prefix: String,
}

impl Shower<'_> {
    /// §182: `show_node_list`.
    fn list(&mut self, list: &[Item]) {
        if self.prefix.len() as i32 > self.cx.depth {
            if !list.is_empty() {
                self.out.push_str(" []");
            }
            return;
        }
        let breadth = if self.cx.breadth <= 0 { 5 } else { self.cx.breadth };
        for (n, it) in list.iter().enumerate() {
            self.out.push(LN);
            self.out.push_str(&self.prefix);
            if n as i32 + 1 > breadth {
                self.out.push_str("etc.");
                return;
            }
            self.item(it);
        }
    }

    fn sub(&mut self, c: char, list: &[Item]) {
        self.prefix.push(c);
        self.list(list);
        self.prefix.pop();
    }

    fn boxed(&mut self, b: &BoxNode) {
        let o = &mut *self.out;
        self.cx.esc(o, if b.vertical { "vbox" } else { "hbox" });
        o.push('(');
        print::scaled(o, b.h);
        o.push('+');
        print::scaled(o, b.d);
        o.push_str(")x");
        print::scaled(o, b.w);
        let g = b.glue.ratio;
        if g != 0.0 && b.glue.sign != 0 {
            o.push_str(", glue set ");
            if b.glue.sign == -1 {
                o.push_str("- ");
            }
            if g.abs() > 20000.0 {
                o.push_str(if g > 0.0 { ">" } else { "< -" });
                glue_part(o, 20000 * 65536, b.glue.order, "");
            } else {
                glue_part(o, zround(65536.0 * g), b.glue.order, "");
            }
        }
        if b.shift != 0 {
            o.push_str(", shifted ");
            print::scaled(o, b.shift);
        }
        self.sub('.', &b.list);
    }

    fn item(&mut self, it: &Item) {
        match it {
            Item::Char(f, c) => {
                self.cx.font_ident(self.out, &f.key);
                self.out.push(' ');
                self.cx.chr(self.out, *c);
            }
            Item::Box(b) => self.boxed(b),
            Item::Unset(u) => {
                // (§184, §185)
                let o = &mut *self.out;
                self.cx.esc(o, "unsetbox(");
                print::scaled(o, u.h);
                o.push('+');
                print::scaled(o, u.d);
                o.push_str(")x");
                print::scaled(o, u.w);
                if u.span != 0 {
                    let _ = write!(o, " ({} columns)", u32::from(u.span) + 1);
                }
                if u.stretch != 0 {
                    o.push_str(", stretch ");
                    glue_part(o, u.stretch, u.stretch_order, "");
                }
                if u.shrink != 0 {
                    o.push_str(", shrink ");
                    glue_part(o, u.shrink, u.shrink_order, "");
                }
                self.sub('.', &u.list);
            }
            Item::Rule { w, h, d } => {
                self.cx.esc(self.out, "rule(");
                rule_dim(self.out, *h);
                self.out.push('+');
                rule_dim(self.out, *d);
                self.out.push_str(")x");
                rule_dim(self.out, *w);
            }
            Item::Glue { g, sub, .. } => {
                // (§189)
                self.cx.esc(self.out, "glue");
                if *sub != 0 {
                    self.out.push('(');
                    let name = match *sub {
                        98 => "nonscript",
                        99 => "mskip",
                        s => SKIPS.get(usize::from(s - 1)).copied().unwrap_or("?"),
                    };
                    self.cx.esc(self.out, name);
                    self.out.push(')');
                }
                if *sub != 98 {
                    self.out.push(' ');
                    spec(self.out, g, if *sub == 99 { "mu" } else { "" });
                }
            }
            Item::Kern { w, sub: 99 } => {
                // (§191)
                self.cx.esc(self.out, "mkern");
                print::scaled(self.out, *w);
                self.out.push_str("mu");
            }
            Item::Kern { w, sub } => {
                self.cx.esc(self.out, "kern");
                if *sub != 0 {
                    self.out.push(' ');
                }
                print::scaled(self.out, *w);
                if *sub == 2 {
                    self.out.push_str(" (for accent)");
                }
            }
            Item::Penalty(p) => {
                self.cx.esc(self.out, "penalty ");
                let _ = write!(self.out, "{p}");
            }
            Item::Lig(l) => {
                self.cx.font_ident(self.out, &l.font.key);
                self.out.push(' ');
                self.cx.chr(self.out, l.c);
                self.out.push_str(" (ligature ");
                if l.left {
                    self.out.push('|');
                }
                for &c in &l.orig {
                    self.cx.chr(self.out, c);
                }
                if l.right {
                    self.out.push('|');
                }
                self.out.push(')');
            }
            Item::Disc(d) => {
                self.cx.esc(self.out, "discretionary");
                if d.replace > 0 {
                    let _ = write!(self.out, " replacing {}", d.replace);
                }
                self.sub('.', &d.pre);
                self.sub('|', &d.post);
            }
            Item::Math { w, after, lr } => {
                // (§192, etex.web §1435)
                if *lr > 0 {
                    self.cx.esc(self.out, if *after { "end" } else { "begin" });
                    self.out.push(match lr {
                        10 => 'R',
                        6 => 'L',
                        _ => 'M',
                    });
                } else {
                    self.cx.esc(self.out, if *after { "mathoff" } else { "mathon" });
                    if *w != 0 {
                        self.out.push_str(", surrounded ");
                        print::scaled(self.out, *w);
                    }
                }
            }
            Item::Lang { lang, lhm, rhm } => {
                // (§1356)
                self.cx.esc(self.out, "setlanguage");
                let _ = write!(self.out, "{lang} (hyphenmin {lhm},{rhm})");
            }
            Item::Noad(m) => self.noad(m),
            Item::Mark(t) => {
                self.cx.esc(self.out, "mark");
                self.out.push('{');
                show_toks_plain(self.out, t, self.cx);
                self.out.push('}');
            }
            Item::Leaders(l) => {
                // (§190)
                self.cx.esc(self.out, "");
                if l.sub == C_LEADERS {
                    self.out.push('c');
                } else if l.sub == X_LEADERS {
                    self.out.push('x');
                }
                self.out.push_str("leaders ");
                spec(self.out, &l.g, "");
                self.sub('.', std::slice::from_ref(&l.b));
            }
            Item::Ins(i) => {
                // (§188)
                self.cx.esc(self.out, "insert");
                let _ = write!(self.out, "{}, natural size ", i.n);
                print::scaled(self.out, i.height);
                self.out.push_str("; split(");
                spec(self.out, &i.split_top, "");
                self.out.push(',');
                print::scaled(self.out, i.depth);
                let _ = write!(self.out, "); float cost {}", i.float_cost);
                self.sub('.', &i.list);
            }
            Item::Adjust(l) => {
                // (§197)
                self.cx.esc(self.out, "vadjust");
                self.sub('.', l);
            }
            Item::Whatsit(w) => self.whatsit(w),
            Item::Marks(n, t) => {
                // (etex.web §196: the class after `\marks`)
                self.cx.esc(self.out, "marks");
                let _ = write!(self.out, "{n}");
                self.out.push('{');
                show_toks_plain(self.out, t, self.cx);
                self.out.push('}');
            }
            _ => {}
        }
    }

    /// §1356: a whatsit; `print_mark`'s texts cut at `max_print_line-10`.
    fn whatsit(&mut self, w: &Whatsit) {
        let stream = |s: &mut Self, n: u8| match n {
            0..=15 => {
                let _ = write!(s.out, "{n}");
            }
            16 => s.out.push('*'),
            _ => s.out.push('-'),
        };
        match w {
            Whatsit::Open { stream: n, name } => {
                self.cx.esc(self.out, "openout");
                stream(self, *n);
                self.out.push('=');
                self.out.push_str(name);
            }
            Whatsit::Write { stream: n, toks } => {
                self.cx.esc(self.out, "write");
                stream(self, *n);
                let mut t = String::new();
                show_toks_plain(&mut t, toks, self.cx);
                self.mark_text(&t);
            }
            Whatsit::Close { stream: n } => {
                self.cx.esc(self.out, "closeout");
                stream(self, *n);
            }
            Whatsit::Special(s) => {
                self.cx.esc(self.out, "special");
                self.mark_text(s);
            }
        }
    }

    /// tex.web 176's `print_mark`: braces round a text, cut with `\ETC.`
    /// past 69 characters.
    fn mark_text(&mut self, t: &str) {
        const LIMIT: usize = crate::print::MAX_PRINT_LINE - 10;
        self.out.push('{');
        if t.chars().count() > LIMIT {
            self.out.extend(t.chars().take(LIMIT));
            self.cx.esc(self.out, "ETC.");
        } else {
            self.out.push_str(t);
        }
        self.out.push('}');
    }
}

impl Shower<'_> {
    /// §690: `print_fam_and_char`.
    fn fam_and_char(&mut self, fam: u8, ch: u8) {
        self.cx.esc(self.out, "fam");
        let _ = write!(self.out, "{fam} ");
        print_ascii(self.out, ch);
    }

    /// §691: `print_delimiter`.
    fn delimiter(&mut self, d: Delim) {
        let a = (i32::from(d.small_fam) * 256 + i32::from(d.small_char)) * 0x1000
            + i32::from(d.large_fam) * 256
            + i32::from(d.large_char);
        let _ = write!(self.out, "\"{a:X}");
    }

    /// §692: `print_subsidiary_data`.
    fn field(&mut self, f: &Field, c: char) {
        if self.prefix.len() as i32 >= self.cx.depth {
            if *f != Field::Empty {
                self.out.push_str(" []");
            }
            return;
        }
        self.prefix.push(c);
        match f {
            Field::Char { fam, ch } | Field::TextChar { fam, ch } => {
                self.out.push(LN);
                self.out.push_str(&self.prefix.clone());
                self.fam_and_char(*fam, *ch);
            }
            Field::Box(b) => self.list(std::slice::from_ref(&Item::Box(b.clone()))),
            Field::Mlist(l) => {
                if l.is_empty() {
                    self.out.push(LN);
                    self.out.push_str(&self.prefix.clone());
                    self.out.push_str("{}");
                } else {
                    self.list(l);
                }
            }
            Field::Empty => {}
        }
        self.prefix.pop();
    }

    /// §694–§697: a math list's entry.
    fn noad(&mut self, m: &MItem) {
        match m {
            MItem::Style(s) => self.cx.esc(
                self.out,
                match s / 2 {
                    0 => "displaystyle",
                    1 => "textstyle",
                    2 => "scriptstyle",
                    _ => "scriptscriptstyle",
                },
            ),
            MItem::Choice(c) => {
                self.cx.esc(self.out, "mathchoice");
                for (k, l) in ['D', 'T', 'S', 's'].into_iter().zip(c.iter()) {
                    self.sub(k, l);
                }
            }
            MItem::Noad(n) => {
                if let NK::Fraction { thickness, left, right } = &n.kind {
                    self.cx.esc(self.out, "fraction, thickness ");
                    if *thickness == crate::mlist::DEFAULT_CODE {
                        self.out.push_str("= default");
                    } else {
                        print::scaled(self.out, *thickness);
                    }
                    if *left != Delim::default() {
                        self.out.push_str(", left-delimiter ");
                        self.delimiter(*left);
                    }
                    if *right != Delim::default() {
                        self.out.push_str(", right-delimiter ");
                        self.delimiter(*right);
                    }
                    self.field(&n.sup, '\\');
                    self.field(&n.sub, '/');
                    return;
                }
                let name = match &n.kind {
                    NK::Ord => "mathord",
                    NK::Op(_) => "mathop",
                    NK::Bin => "mathbin",
                    NK::Rel => "mathrel",
                    NK::Open => "mathopen",
                    NK::Close => "mathclose",
                    NK::Punct => "mathpunct",
                    NK::Inner => "mathinner",
                    NK::Over => "overline",
                    NK::Under => "underline",
                    NK::Vcenter => "vcenter",
                    NK::Radical(_) => "radical",
                    NK::Accent { .. } => "accent",
                    NK::Left(_) => "left",
                    NK::Right(_, middle) => {
                        if *middle {
                            "middle"
                        } else {
                            "right"
                        }
                    }
                    NK::Fraction { .. } => unreachable!(),
                };
                self.cx.esc(self.out, name);
                match &n.kind {
                    NK::Radical(d) | NK::Left(d) | NK::Right(d, _) => self.delimiter(*d),
                    NK::Accent { fam, ch } => self.fam_and_char(*fam, *ch),
                    NK::Op(Limits::Limits) => self.cx.esc(self.out, "limits"),
                    NK::Op(Limits::NoLimits) => self.cx.esc(self.out, "nolimits"),
                    _ => {}
                }
                if !matches!(n.kind, NK::Left(_) | NK::Right(..)) {
                    self.field(&n.nucleus, '.');
                }
                self.field(&n.sup, '^');
                self.field(&n.sub, '_');
            }
            _ => {}
        }
    }
}

/// tex.web 68 `print_ASCII`: a printable character as itself, others in
/// `^^` notation.
fn print_ascii(out: &mut String, c: u8) {
    if (32..127).contains(&c) {
        out.push(char::from(c));
    } else if c < 64 {
        out.push_str("^^");
        out.push(char::from(c + 64));
    } else if c < 128 {
        out.push_str("^^");
        out.push(char::from(c - 64));
    } else {
        let _ = write!(out, "^^{c:02x}");
    }
}

/// §198 `show_box` of a list (what `\showlists` shows of each list): its
/// nodes, then `print_ln`.
pub fn show_list(out: &mut String, list: &[Item], cx: &Ctx) {
    let mut s = Shower { out, cx, prefix: String::new() };
    s.list(list);
    s.out.push(LN);
}

/// A token list as a mark shows it (tex.web 292), with every one-letter
/// name followed by a space (category codes are not at hand here).
fn show_toks_plain(out: &mut String, toks: &[Tok], cx: &Ctx) {
    for t in toks {
        match t {
            Tok::Cs(n) => {
                cx.esc(out, n);
                if n.chars().count() != 1 || n.chars().all(|c| c.is_ascii_alphabetic()) {
                    out.push(' ');
                }
            }
            Tok::Char(6, c) => {
                out.push(*c);
                out.push(*c);
            }
            Tok::Char(_, c) | Tok::Active(c) => out.push(*c),
            Tok::Param(n) => {
                out.push('#');
                out.push(char::from(b'0' + n));
            }
            Tok::DontExpand => {}
        }
    }
}

/// web2c's `zround`.
pub fn zround(r: f64) -> i32 {
    if r > 2_147_483_647.0 {
        2_147_483_647
    } else if r < -2_147_483_647.0 {
        -2_147_483_647
    } else if r >= 0.0 {
        (r + 0.5) as i32
    } else {
        (r - 0.5) as i32
    }
}

/// §198: `show_box` of one box (with `print_ln`s as [`LN`]).
pub fn show_box(out: &mut String, b: &BoxNode, cx: &Ctx) {
    let mut s = Shower { out, cx, prefix: String::new() };
    // (the list of one box, at depth 0: `show_node_list`'s cut applies)
    if cx.depth < 0 {
        s.out.push_str(" []");
    } else {
        s.out.push(LN);
        s.boxed(b);
    }
    s.out.push(LN);
}

/// Where a packaging diagnostic happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// A line of a paragraph (`in paragraph at lines a--b`).
    Paragraph,
    /// A box made by a command (`detected at line n`).
    Here,
    /// An alignment's preamble (`in alignment at lines a--b`, tex.web 663,
    /// 675: from the alignment's first line to the current one).
    Alignment,
}

/// The diagnostic TeX prints for a box (tex.web 660–667 for an hbox,
/// 674–678 for a vbox): a raw log entry.
pub fn report(r: Report, b: &BoxNode, place: Place, cx: &Ctx) -> String {
    let mut o = String::new();
    o.push(RAW);
    o.push(LN);
    o.push(NL);
    let what = if b.vertical { "\\vbox" } else { "\\hbox" };
    match r {
        Report::Underfull(x) => {
            let _ = write!(o, "Underfull {what} (badness {x}");
        }
        Report::Loose(x) => {
            let _ = write!(o, "Loose {what} (badness {x}");
        }
        Report::Tight(x) => {
            let _ = write!(o, "Tight {what} (badness {x}");
        }
        Report::Overfull(x) => {
            o.push_str("Overfull ");
            o.push_str(what);
            o.push_str(" (");
            print::scaled(&mut o, x);
            o.push_str(if b.vertical { "pt too high" } else { "pt too wide" });
        }
    }
    match place {
        Place::Paragraph => {
            let _ = write!(o, ") in paragraph at lines {LINE}P{LINE}");
        }
        Place::Here => {
            let _ = write!(o, ") detected at line {LINE}L{LINE}");
        }
        Place::Alignment => {
            let _ = write!(o, ") in alignment at lines {LINE}A{LINE}");
        }
    }
    if b.vertical {
        o.push(LN);
    } else {
        o.push(LN);
        let mut f: Arc<str> = Arc::from("nullfont");
        short_display(&mut o, &b.list, &mut f, cx);
        o.push(LN);
    }
    show_box(&mut o, b, cx);
    o.push(NL);
    o.push(LN);
    o
}
