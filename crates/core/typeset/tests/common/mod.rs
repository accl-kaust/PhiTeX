//! Shared by the oracle tests: tex.web's `show_box` (§180–§198) for the
//! nodes the fixtures use, and a font without characters.
#![allow(dead_code, clippy::unreadable_literal, clippy::format_push_string)]

use phitex_typeset::arith::{Scaled, UNITY};
use phitex_typeset::fonts::{CharInfo, FontId, Fonts, Left, LigKern};
use phitex_typeset::glue::{Glue, GlueSign, Order};
use phitex_typeset::node::{BoxKind, GlueKind, KernKind, Node, RUNNING};

pub struct NoFonts;

impl Fonts for NoFonts {
    fn char_info(&self, _: FontId, _: u16) -> Option<CharInfo> {
        None
    }
    fn lig_kern(&self, _: FontId, _: Left, _: u16) -> Option<LigKern> {
        None
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
                if let GlueKind::Param(c) = g.kind {
                    out.push_str(&format!("(\\{})", param_name(c)));
                }
                assert!(matches!(g.kind, GlueKind::Normal | GlueKind::Param(_)));
                out.push(' ');
                out.push_str(&spec(&g.spec));
            }
            Node::Kern(w, k) => {
                out.push_str("\\kern");
                if *k != KernKind::Normal {
                    out.push(' ');
                }
                out.push_str(&scaled(*w));
            }
            Node::Penalty(p) => out.push_str(&format!("\\penalty {p}")),
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
