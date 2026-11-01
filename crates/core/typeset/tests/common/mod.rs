//! Shared by the oracle tests: tex.web's `show_box` (§180–§198) for the
//! nodes the fixtures use, and a font without characters.
#![allow(
    dead_code,
    clippy::unreadable_literal,
    clippy::format_push_string,
    clippy::many_single_char_names,
    clippy::too_many_lines
)]

use std::collections::HashMap;

use phitex_typeset::arith::{Scaled, UNITY};
use phitex_typeset::fonts::{CharInfo, CharTag, FontId, Fonts, Left, LigKern, LigOp};
use phitex_typeset::glue::{Glue, GlueSet, GlueSign, Order};
use phitex_typeset::node::{
    BoxKind, BoxNode, Disc, GlueKind, GlueNode, KernKind, Ligature, MathKind, Node, RUNNING, Rule,
};

pub struct NoFonts;

thread_local! {
    static FONT_NAMES: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(vec!["tenrm".into()]);
}

/// The `\font` identifiers of the font ids, for printing and parsing.
pub fn set_font_names(names: Vec<String>) {
    FONT_NAMES.with(|n| *n.borrow_mut() = names);
}

fn font_name(f: FontId) -> String {
    FONT_NAMES.with(|n| n.borrow()[usize::try_from(f).unwrap()].clone())
}

/// Splits `\name rest` into the font id and the rest, if `name` is a font.
fn font_prefix(body: &str) -> Option<(FontId, &str)> {
    let rest = body.strip_prefix('\\')?;
    let (name, rest) = rest.split_once(' ')?;
    FONT_NAMES.with(|n| {
        n.borrow()
            .iter()
            .position(|x| x == name)
            .map(|i| (FontId::try_from(i).unwrap(), rest))
    })
}

/// Several fonts, numbered in order.
pub struct FontSet(pub Vec<TableFont>);

impl FontSet {
    /// Parses `font <name>` sections; returns the fonts and their names.
    pub fn parse(text: &str) -> (FontSet, Vec<String>) {
        let mut names = Vec::new();
        let mut bodies: Vec<String> = Vec::new();
        for l in text.lines() {
            if let Some(n) = l.strip_prefix("font ") {
                names.push(n.to_owned());
                bodies.push(String::new());
            } else {
                let b = bodies.last_mut().unwrap();
                b.push_str(l);
                b.push('\n');
            }
        }
        (
            FontSet(bodies.iter().map(|b| TableFont::parse(b)).collect()),
            names,
        )
    }
    fn f(&self, font: FontId) -> &TableFont {
        &self.0[usize::try_from(font).unwrap()]
    }
}

impl Fonts for FontSet {
    fn char_info(&self, font: FontId, ch: u16) -> Option<CharInfo> {
        self.f(font).char_info(0, ch)
    }
    fn lig_kern(&self, font: FontId, left: Left, right: u16) -> Option<LigKern> {
        self.f(font).lig_kern(0, left, right)
    }
    fn has_left_boundary(&self, font: FontId) -> bool {
        self.f(font).has_left_boundary(0)
    }
    fn bchar(&self, font: FontId) -> Option<u16> {
        self.f(font).bchar(0)
    }
    fn param(&self, font: FontId, n: u16) -> Scaled {
        self.f(font).param(0, n)
    }
    fn param_count(&self, font: FontId) -> u16 {
        self.f(font).param_count(0)
    }
}

/// One font from `fixtures/<name>.txt` (made by `gen/hyph.py`), as font 0.
#[derive(Default)]
pub struct TableFont {
    pub chars: HashMap<u16, CharInfo>,
    pub lk: HashMap<(Left, u16), LigKern>,
    pub bchar: Option<u16>,
    pub params: HashMap<u16, Scaled>,
}

impl TableFont {
    pub fn parse(text: &str) -> TableFont {
        let mut f = TableFont::default();
        for l in text.lines() {
            let w: Vec<&str> = l.split(' ').collect();
            let n = |i: usize| w[i].parse::<i32>().unwrap();
            match w[0] {
                "char" => {
                    let c = |i: usize| u16::try_from(n(i)).unwrap();
                    let opt = |i: usize| Some(c(i)).filter(|x| *x != 0);
                    let tag = match w.get(6).copied() {
                        Some("list") => CharTag::List(c(7)),
                        Some("ext") => CharTag::Extensible {
                            top: opt(7),
                            mid: opt(8),
                            bot: opt(9),
                            rep: c(10),
                        },
                        _ => CharTag::None,
                    };
                    f.chars.insert(
                        c(1),
                        CharInfo {
                            width: n(2),
                            height: n(3),
                            depth: n(4),
                            italic: n(5),
                            tag,
                        },
                    );
                }
                "param" => {
                    f.params.insert(u16::try_from(n(1)).unwrap(), n(2));
                }
                "lk" => {
                    let left = if n(1) < 0 {
                        Left::Boundary
                    } else {
                        Left::Char(u16::try_from(n(1)).unwrap())
                    };
                    let v = if w[3] == "kern" {
                        LigKern::Kern(n(4))
                    } else {
                        let op = match n(4) {
                            0 => LigOp::Plain,
                            1 => LigOp::KeepRight,
                            2 => LigOp::KeepLeft,
                            3 => LigOp::KeepBoth,
                            5 => LigOp::KeepRightSkip,
                            6 => LigOp::KeepLeftSkip,
                            7 => LigOp::KeepBothSkip1,
                            11 => LigOp::KeepBothSkip2,
                            o => panic!("op {o}"),
                        };
                        LigKern::Lig(op, u16::try_from(n(5)).unwrap())
                    };
                    f.lk.insert((left, u16::try_from(n(2)).unwrap()), v);
                }
                "bchar" => f.bchar = Some(u16::try_from(n(1)).unwrap()),
                _ => panic!("{l}"),
            }
        }
        f
    }
}

impl Fonts for TableFont {
    fn char_info(&self, _: FontId, ch: u16) -> Option<CharInfo> {
        self.chars.get(&ch).copied()
    }
    fn lig_kern(&self, _: FontId, left: Left, right: u16) -> Option<LigKern> {
        self.lk.get(&(left, right)).copied()
    }
    fn has_left_boundary(&self, _: FontId) -> bool {
        self.lk.keys().any(|k| k.0 == Left::Boundary)
    }
    fn bchar(&self, _: FontId) -> Option<u16> {
        self.bchar
    }
    fn param(&self, _: FontId, n: u16) -> Scaled {
        self.params.get(&n).copied().unwrap_or(0)
    }
    fn param_count(&self, _: FontId) -> u16 {
        self.params.keys().copied().max().unwrap_or(0)
    }
}

/// `print_ASCII` (§68) for codes below 128.
pub fn ascii(c: u16) -> String {
    if (32..127).contains(&c) {
        char::from(u8::try_from(c).unwrap()).to_string()
    } else if c < 64 {
        format!("^^{}", char::from(u8::try_from(c + 64).unwrap()))
    } else {
        format!("^^{}", char::from(u8::try_from(c - 64).unwrap()))
    }
}

/// Parses a decimal like `-0.27779` as TeX scans it (§448, §452).
pub fn parse_dimen(s: &str) -> Scaled {
    let (neg, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
    let (i, f) = s.split_once('.').unwrap_or((s, ""));
    let digits: Vec<u8> = f.bytes().map(|b| b - b'0').take(17).collect();
    let v = i.parse::<Scaled>().unwrap() * UNITY + phitex_typeset::arith::round_decimals(&digits);
    if neg { -v } else { v }
}

fn parse_glue_text(s: &str) -> Glue {
    // "3.33333 plus 1.66666fil minus 1.11111"
    let mut g = Glue::ZERO;
    let mut it = s.split(' ');
    g.width = parse_dimen(it.next().unwrap());
    let amount = |x: &str| -> (Scaled, Order) {
        for (suf, o) in [
            ("filll", Order::Filll),
            ("fill", Order::Fill),
            ("fil", Order::Fil),
        ] {
            if let Some(v) = x.strip_suffix(suf) {
                return (parse_dimen(v), o);
            }
        }
        (parse_dimen(x), Order::Normal)
    };
    while let Some(k) = it.next() {
        let (v, o) = amount(it.next().unwrap());
        if k == "plus" {
            (g.stretch, g.stretch_order) = (v, o);
        } else {
            (g.shrink, g.shrink_order) = (v, o);
        }
    }
    g
}

fn parse_chars(s: &str) -> Vec<u16> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'^' && b.get(i + 1) == Some(&b'^') {
            let k = u16::from(b[i + 2]);
            out.push(if k >= 64 { k - 64 } else { k + 64 });
            i += 3;
        } else {
            out.push(u16::from(b[i]));
            i += 1;
        }
    }
    out
}

/// Parses `\\hbox(h+d)xw, glue set ..., shifted s`.
fn parse_box_header(body: &str, list: Vec<Node>) -> BoxNode {
    let kind = if body.starts_with("\\hbox") {
        BoxKind::H
    } else {
        BoxKind::V
    };
    let (dims, rest) = body[6..].split_once(")x").unwrap();
    let (h, d) = dims.split_once('+').unwrap();
    let mut parts = rest.split(", ");
    let w = parse_dimen(parts.next().unwrap());
    let mut b = BoxNode {
        kind,
        width: w,
        height: parse_dimen(h),
        depth: parse_dimen(d),
        list,
        ..BoxNode::default()
    };
    for p in parts {
        if let Some(g) = p.strip_prefix("glue set ") {
            let (sign, g) = g
                .strip_prefix("- ")
                .map_or((GlueSign::Stretching, g), |g| (GlueSign::Shrinking, g));
            let mut order = Order::Normal;
            let mut g = g;
            for (suf, o) in [
                ("filll", Order::Filll),
                ("fill", Order::Fill),
                ("fil", Order::Fil),
            ] {
                if let Some(v) = g.strip_suffix(suf) {
                    g = v;
                    order = o;
                    break;
                }
            }
            b.glue = GlueSet {
                ratio: f64::from(parse_dimen(g)) / f64::from(UNITY),
                sign,
                order,
            };
        } else if let Some(s) = p.strip_prefix("shifted ") {
            b.shift = parse_dimen(s);
        }
    }
    b
}

/// Parses `show_box` output for the nodes the fixtures use, at `prefix`.
pub fn parse_list(lines: &[&str], prefix: &str) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    let mut i = 0;
    let mut pending_replace: Option<(usize, usize)> = None; // (disc index, count)
    while i < lines.len() {
        let l = lines[i];
        let body = &l[prefix.len()..];
        i += 1;
        let node = if let Some((font, rest)) = font_prefix(body) {
            if let Some((c, lig)) = rest.split_once(" (ligature ") {
                let lig = lig.strip_suffix(')').unwrap();
                let left_hit = lig.starts_with('|');
                let lig = lig.strip_prefix('|').unwrap_or(lig);
                let right_hit = lig.ends_with('|') && !lig.is_empty();
                let lig = if right_hit {
                    &lig[..lig.len() - 1]
                } else {
                    lig
                };
                Node::Lig(Ligature {
                    font,
                    ch: parse_chars(c)[0],
                    original: parse_chars(lig),
                    left_hit,
                    right_hit,
                })
            } else {
                Node::Char {
                    font,
                    ch: parse_chars(rest)[0],
                }
            }
        } else if body.starts_with("\\hbox(") || body.starts_with("\\vbox(") {
            let inner = format!("{prefix}.");
            let start = i;
            while i < lines.len() && lines[i].starts_with(&inner) {
                i += 1;
            }
            Node::Box(Box::new(parse_box_header(
                body,
                parse_list(&lines[start..i], &inner),
            )))
        } else if let Some(rest) = body.strip_prefix("\\rule(") {
            let (hd, w) = rest.split_once(")x").unwrap();
            let (h, d) = hd.split_once('+').unwrap();
            let dim = |x: &str| if x == "*" { RUNNING } else { parse_dimen(x) };
            Node::Rule(Rule {
                width: dim(w),
                height: dim(h),
                depth: dim(d),
            })
        } else if body == "\\mathon" {
            Node::Math(MathKind::Before, 0)
        } else if body == "\\mathoff" {
            Node::Math(MathKind::After, 0)
        } else if body == "\\glue(\\nonscript)" {
            Node::Glue(GlueNode {
                spec: Glue::ZERO,
                kind: GlueKind::CondMath,
                own_spec: false,
                leader: None,
            })
        } else if let Some(rest) = body.strip_prefix("\\glue(\\mskip) ") {
            Node::Glue(GlueNode {
                spec: parse_glue_text(&rest.replace("mu", "")),
                kind: GlueKind::Mu,
                own_spec: false,
                leader: None,
            })
        } else if let Some(rest) = body.strip_prefix("\\mkern") {
            Node::Kern(parse_dimen(rest.strip_suffix("mu").unwrap()), KernKind::Mu)
        } else if let Some(rest) = body.strip_prefix("\\glue(\\") {
            let (name, g) = rest.split_once(") ").unwrap();
            let code = (0..=17).find(|c| param_name(*c) == name).unwrap();
            Node::param_glue(code, parse_glue_text(g))
        } else if let Some(rest) = body.strip_prefix("\\glue ") {
            Node::glue(parse_glue_text(rest))
        } else if let Some(rest) = body.strip_prefix("\\kern ") {
            Node::Kern(parse_dimen(rest), KernKind::Explicit)
        } else if let Some(rest) = body.strip_prefix("\\kern") {
            Node::Kern(parse_dimen(rest), KernKind::Normal)
        } else if let Some(rest) = body.strip_prefix("\\penalty ") {
            Node::Penalty(rest.parse().unwrap())
        } else if let Some(rest) = body.strip_prefix("\\discretionary") {
            let mut pre = Vec::new();
            let mut post = Vec::new();
            let dot = format!("{prefix}.");
            let bar = format!("{prefix}|");
            let start = i;
            while i < lines.len() && (lines[i].starts_with(&dot) || lines[i].starts_with(&bar)) {
                i += 1;
            }
            let sub: Vec<&str> = lines[start..i].to_vec();
            let pre_lines: Vec<&str> = sub
                .iter()
                .copied()
                .filter(|x| x.starts_with(&dot))
                .collect();
            let post_lines: Vec<&str> = sub
                .iter()
                .copied()
                .filter(|x| x.starts_with(&bar))
                .collect();
            pre.extend(parse_list(&pre_lines, &dot));
            post.extend(parse_list(&post_lines, &bar));
            if let Some(n) = rest.strip_prefix(" replacing ") {
                pending_replace = Some((out.len(), n.parse().unwrap()));
            }
            out.push(Node::Disc(Box::new(Disc {
                pre,
                post,
                replace: Vec::new(),
            })));
            continue;
        } else {
            panic!("parse_list: {l}");
        };
        if let Some((d, n)) = pending_replace
            && let Node::Disc(disc) = &mut out[d]
        {
            disc.replace.push(node);
            if disc.replace.len() == n {
                pending_replace = None;
            }
            continue;
        }
        out.push(node);
    }
    out
}

impl Fonts for NoFonts {
    fn char_info(&self, _: FontId, _: u16) -> Option<CharInfo> {
        None
    }
    fn lig_kern(&self, _: FontId, _: Left, _: u16) -> Option<LigKern> {
        None
    }
    fn has_left_boundary(&self, _: FontId) -> bool {
        false
    }
    fn bchar(&self, _: FontId) -> Option<u16> {
        None
    }
    fn param(&self, _: FontId, _: u16) -> Scaled {
        0
    }
    fn param_count(&self, _: FontId) -> u16 {
        0
    }
}

/// `print_scaled` (§103).
pub fn scaled(mut s: Scaled) -> String {
    let mut out = String::new();
    if s < 0 {
        out.push('-');
        s = -s;
    }
    out.push_str(&(s / UNITY).to_string());
    out.push('.');
    s = 10 * (s % UNITY) + 5;
    let mut delta = 10;
    loop {
        if delta > UNITY {
            s += 0o100000 - 50000;
        }
        out.push(char::from(b'0' + u8::try_from(s / UNITY).unwrap()));
        s = 10 * (s % UNITY);
        delta *= 10;
        if s <= delta {
            break;
        }
    }
    out
}

/// `print_glue` (§177).
pub fn glue_amount(d: Scaled, o: Order, unit: &str) -> String {
    let mut s = scaled(d);
    match o {
        Order::Normal => s.push_str(unit),
        Order::Fil => s.push_str("fil"),
        Order::Fill => s.push_str("fill"),
        Order::Filll => s.push_str("filll"),
    }
    s
}

/// `print_spec` (§178).
pub fn spec(g: &Glue) -> String {
    let mut s = scaled(g.width);
    if g.stretch != 0 {
        s.push_str(" plus ");
        s.push_str(&glue_amount(g.stretch, g.stretch_order, ""));
    }
    if g.shrink != 0 {
        s.push_str(" minus ");
        s.push_str(&glue_amount(g.shrink, g.shrink_order, ""));
    }
    s
}

fn rule_dimen(d: Scaled) -> String {
    if d == RUNNING { "*".into() } else { scaled(d) }
}

fn param_name(c: u8) -> &'static str {
    match c {
        0 => "lineskip",
        1 => "baselineskip",
        7 => "leftskip",
        8 => "rightskip",
        9 => "topskip",
        10 => "splittopskip",
        14 => "parfillskip",
        15 => "thinmuskip",
        16 => "medmuskip",
        17 => "thickmuskip",
        _ => "?",
    }
}

/// `show_box` of a list with the given prefix.
pub fn show_list(list: &[Node], prefix: &str, out: &mut String) {
    for n in list {
        out.push_str(prefix);
        match n {
            Node::Box(b) => {
                out.push_str(if b.kind == BoxKind::H {
                    "\\hbox("
                } else {
                    "\\vbox("
                });
                out.push_str(&format!(
                    "{}+{})x{}",
                    scaled(b.height),
                    scaled(b.depth),
                    scaled(b.width)
                ));
                if b.glue.sign != GlueSign::Normal && b.glue.ratio != 0.0 {
                    out.push_str(", glue set ");
                    if b.glue.sign == GlueSign::Shrinking {
                        out.push_str("- ");
                    }
                    let g = b.glue.ratio;
                    if g.abs() > 20000.0 {
                        out.push_str(if g > 0.0 { ">" } else { "< -" });
                        out.push_str(&glue_amount(20000 * UNITY, b.glue.order, ""));
                    } else {
                        #[allow(clippy::cast_possible_truncation)]
                        let r = (f64::from(UNITY) * g).round() as i32;
                        out.push_str(&glue_amount(r, b.glue.order, ""));
                    }
                }
                if b.shift != 0 {
                    out.push_str(", shifted ");
                    out.push_str(&scaled(b.shift));
                }
                out.push('\n');
                show_list(&b.list, &format!("{prefix}."), out);
                continue;
            }
            Node::Rule(r) => out.push_str(&format!(
                "\\rule({}+{})x{}",
                rule_dimen(r.height),
                rule_dimen(r.depth),
                rule_dimen(r.width)
            )),
            Node::Glue(g) => {
                out.push_str("\\glue");
                match g.kind {
                    GlueKind::Param(c) => out.push_str(&format!("(\\{})", param_name(c))),
                    GlueKind::CondMath => out.push_str("(\\nonscript)"),
                    GlueKind::Mu => out.push_str("(\\mskip)"),
                    GlueKind::Normal => {}
                    k => panic!("{k:?}"),
                }
                if g.kind != GlueKind::CondMath {
                    out.push(' ');
                    out.push_str(&spec(&g.spec));
                }
            }
            Node::Kern(w, KernKind::Mu) => {
                out.push_str("\\mkern");
                out.push_str(&scaled(*w));
                out.push_str("mu");
            }
            Node::Kern(w, k) => {
                out.push_str("\\kern");
                if *k != KernKind::Normal {
                    out.push(' ');
                }
                out.push_str(&scaled(*w));
            }
            Node::Penalty(p) => out.push_str(&format!("\\penalty {p}")),
            Node::Char { font, ch } => {
                out.push_str(&format!("\\{} ", font_name(*font)));
                out.push_str(&ascii(*ch));
            }
            Node::Math(k, w) => {
                out.push_str(if *k == MathKind::Before {
                    "\\mathon"
                } else {
                    "\\mathoff"
                });
                if *w != 0 {
                    out.push_str(", surrounded ");
                    out.push_str(&scaled(*w));
                }
            }
            Node::Lig(l) => {
                out.push_str(&format!("\\{} ", font_name(l.font)));
                out.push_str(&ascii(l.ch));
                out.push_str(" (ligature ");
                if l.left_hit {
                    out.push('|');
                }
                for c in &l.original {
                    out.push_str(&ascii(*c));
                }
                if l.right_hit {
                    out.push('|');
                }
                out.push(')');
            }
            Node::Disc(d) => {
                out.push_str("\\discretionary");
                if !d.replace.is_empty() {
                    out.push_str(&format!(" replacing {}", d.replace.len()));
                }
                out.push('\n');
                show_list(&d.pre, &format!("{prefix}."), out);
                show_list(&d.post, &format!("{prefix}|"), out);
                show_list(&d.replace, prefix, out);
                continue;
            }
            other => panic!("show_list: {other:?}"),
        }
        out.push('\n');
    }
}

/// Parses `a,b,c` glue with optional `fil`/`fill`/`filll` suffixes.
pub fn parse_glue(s: &str) -> Glue {
    let parts: Vec<&str> = s.split(',').collect();
    let inf = |x: &str| -> (Scaled, Order) {
        for (suf, o) in [
            ("filll", Order::Filll),
            ("fill", Order::Fill),
            ("fil", Order::Fil),
        ] {
            if let Some(v) = x.strip_suffix(suf) {
                return (v.parse::<Scaled>().unwrap(), o);
            }
        }
        (x.parse().unwrap(), Order::Normal)
    };
    let (stretch, stretch_order) = inf(parts[1]);
    let (shrink, shrink_order) = inf(parts[2]);
    Glue {
        width: parts[0].parse().unwrap(),
        stretch,
        stretch_order,
        shrink,
        shrink_order,
    }
}

/// The fixture file split into cases: (key/value pairs, expected text).
pub fn cases(text: &str) -> Vec<(Vec<(String, String)>, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(l) = lines.next() {
        assert!(l.starts_with("case "), "{l}");
        let mut kv = Vec::new();
        for l in lines.by_ref() {
            if l == "expect" {
                break;
            }
            let (k, v) = l.split_once('=').unwrap();
            kv.push((k.to_owned(), v.to_owned()));
        }
        let mut exp = String::new();
        for l in lines.by_ref() {
            if l == "end" {
                break;
            }
            exp.push_str(l);
            exp.push('\n');
        }
        out.push((kv, exp));
    }
    out
}
