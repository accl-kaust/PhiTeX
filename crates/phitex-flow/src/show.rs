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

use crate::item::{BoxNode, Item, RUNNING};
use crate::pack::Report;
use crate::print;
use crate::val::Tok;

pub const RAW: char = '\u{3}';
pub const LN: char = '\u{1}';
pub const NL: char = '\u{2}';
pub const LINE: char = '\u{4}';
/// A space if the log's line is not empty (tex.web 638's `[`).
pub const SP: char = '\u{6}';

/// What a display reads: the escape character, the font identifiers, and
/// `\showboxdepth`/`\showboxbreadth`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Ctx {
    pub esc: i32,
    pub idents: Vec<(u16, Arc<str>)>,
    pub depth: i32,
    pub breadth: i32,
}

impl Ctx {
    fn esc(&self, out: &mut String, s: &str) {
        if let Some(c) = u8::try_from(self.esc).ok().map(char::from) {
            out.push(c);
        }
        out.push_str(s);
    }
    fn font_ident(&self, out: &mut String, id: u16) {
        let name = self.idents.iter().find(|(i, _)| *i == id).map_or("FONT?", |(_, n)| &**n);
        self.esc(out, name);
    }
}

/// §174: `short_display`.
pub fn short_display(out: &mut String, list: &[Item], font: &mut u16, cx: &Ctx) {
    let mut skip = 0usize;
    for it in list {
        if skip > 0 {
            // (a discretionary's replacement is not shown)
            skip -= 1;
            continue;
        }
        match it {
            Item::Char(f, c) => {
                if f.id != *font {
                    cx.font_ident(out, f.id);
                    out.push(' ');
                    *font = f.id;
                }
                out.push(char::from(*c));
            }
            Item::Box(_) | Item::Mark(_) => out.push_str("[]"),
            Item::Rule { .. } => out.push('|'),
            Item::Glue { zero, .. } => {
                if !zero {
                    out.push(' ');
                }
            }
            Item::Lig(l) => {
                for &c in &l.orig {
                    if l.font.id != *font {
                        cx.font_ident(out, l.font.id);
                        out.push(' ');
                        *font = l.font.id;
                    }
                    out.push(char::from(c));
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
    font: u16,
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
                self.cx.font_ident(self.out, f.id);
                self.out.push(' ');
                self.out.push(char::from(*c));
            }
            Item::Box(b) => self.boxed(b),
            Item::Rule { w, h, d } => {
                self.cx.esc(self.out, "rule(");
                rule_dim(self.out, *h);
                self.out.push('+');
                rule_dim(self.out, *d);
                self.out.push_str(")x");
                rule_dim(self.out, *w);
            }
            Item::Glue { g, sub, .. } => {
                self.cx.esc(self.out, "glue");
                if *sub != 0 && *sub < 98 {
                    self.out.push('(');
                    self.cx.esc(self.out, SKIPS.get(usize::from(*sub - 1)).copied().unwrap_or("?"));
                    self.out.push(')');
                }
                self.out.push(' ');
                spec(self.out, g, if *sub == 99 { "mu" } else { "" });
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
                self.cx.font_ident(self.out, l.font.id);
                self.out.push(' ');
                self.out.push(char::from(l.c));
                self.out.push_str(" (ligature ");
                if l.left {
                    self.out.push('|');
                }
                self.font = l.font.id;
                for &c in &l.orig {
                    self.out.push(char::from(c));
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
            Item::Mark(t) => {
                self.cx.esc(self.out, "mark");
                self.out.push('{');
                show_toks_plain(self.out, t, self.cx);
                self.out.push('}');
            }
            _ => {}
        }
    }
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
    let mut s = Shower { out, cx, prefix: String::new(), font: 0 };
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
    }
    if b.vertical {
        o.push(LN);
    } else {
        o.push(LN);
        let mut f = 0u16;
        short_display(&mut o, &b.list, &mut f, cx);
        o.push(LN);
    }
    show_box(&mut o, b, cx);
    o.push(NL);
    o.push(LN);
    o
}
