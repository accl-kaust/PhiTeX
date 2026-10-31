//! `show_box` (§180–§198, pdfTeX's and e-TeX's node kinds): a box's
//! list displayed as TeX displays it, one node a line, each line's
//! prefix its depth in `.`s (`|` for a discretionary's post-break list),
//! `\showboxdepth` and `\showboxbreadth` bounding it (§236).

use phitex_defuse::ValueId;
use phitex_typeset::glue::GlueSign;
use phitex_typeset::node::{BoxKind, GlueKind, KernKind, MathKind, NativeKind, Node, RUNNING};

use crate::nodes::Side;
use crate::print::{print_esc, print_glue, print_int, print_scaled, print_spec};
use crate::run::{R, Run};
use crate::val::{int_of, list_of, str_of};

/// The skip parameters by code (§225).
const SKIP_PARAMS: [&str; 18] = [
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

/// The display: its lines (each a `print_ln` before it but the first),
/// the prefix (`cur_length`'s string) and the bounds.
struct Shower {
    lines: Vec<String>,
    prefix: String,
    depth: i64,
    breadth: i64,
    esc: i64,
}

impl Shower {
    fn line(&mut self) -> &mut String {
        self.lines.last_mut().expect("a line")
    }

    /// `print_ln; print_current_string` (§182).
    fn new_line(&mut self) {
        let p = self.prefix.clone();
        self.lines.push(p);
    }

    fn esc(&mut self, name: &str) {
        let e = self.esc;
        print_esc(self.line(), name, e);
    }

    fn push(&mut self, s: &str) {
        self.line().push_str(s);
    }

    fn scaled(&mut self, d: i32) {
        print_scaled(self.line(), d);
    }

    /// `print_rule_dimen` (§176).
    fn rule_dimen(&mut self, d: i32) {
        if d == RUNNING {
            self.push("*");
        } else {
            self.scaled(d);
        }
    }
}

impl Run<'_, '_> {
    /// `show_box` (§198) of box value `b`: the lines TeX prints, the
    /// first on the line it is begun on.
    pub(crate) fn show_box_lines(&mut self, b: ValueId) -> R<Vec<String>> {
        let depth = i64::from(self.param("showboxdepth")?);
        let mut breadth = i64::from(self.param("showboxbreadth")?);
        if breadth <= 0 {
            breadth = 5;
        }
        let esc = i64::from(self.ipar(crate::fields::Par::escapechar)?);
        let mut side = Side::default();
        let node = crate::nodes::decode(self.v, b, &mut side);
        let mut sh = Shower {
            lines: vec![String::new()],
            prefix: String::new(),
            depth,
            breadth,
            esc,
        };
        let list: Vec<Node> = node.into_iter().collect();
        self.show_node_list(&mut sh, &list, &side)?;
        Ok(sh.lines)
    }

    /// `show_box` (§198) of a node not encoded (a box just packed, its
    /// marks and whatsits in `side`).
    pub(crate) fn show_node_lines(&mut self, node: &Node, side: &Side) -> R<Vec<String>> {
        let depth = i64::from(self.param("showboxdepth")?);
        let mut breadth = i64::from(self.param("showboxbreadth")?);
        if breadth <= 0 {
            breadth = 5;
        }
        let esc = i64::from(self.ipar(crate::fields::Par::escapechar)?);
        let mut sh = Shower {
            lines: vec![String::new()],
            prefix: String::new(),
            depth,
            breadth,
            esc,
        };
        self.show_node_list(&mut sh, std::slice::from_ref(node), side)?;
        Ok(sh.lines)
    }

    /// `show_node_list` (§182).
    fn show_node_list(&mut self, sh: &mut Shower, list: &[Node], side: &Side) -> R<()> {
        if i64::try_from(sh.prefix.chars().count()).unwrap_or(i64::MAX) > sh.depth {
            if !list.is_empty() {
                sh.push(" []");
            }
            return Ok(());
        }
        let mut n = 0;
        // (a discretionary's replaced nodes follow it in TeX's list)
        let mut items: Vec<&Node> = Vec::with_capacity(list.len());
        for x in list {
            items.push(x);
            if let Node::Disc(d) = x {
                items.extend(d.replace.iter());
            }
        }
        for x in items {
            sh.new_line();
            n += 1;
            if n > sh.breadth {
                sh.push("etc.");
                return Ok(());
            }
            self.display_node(sh, x, side)?;
        }
        Ok(())
    }

    /// `node_list_display` (§180): the list one `.` deeper.
    fn node_list_display(&mut self, sh: &mut Shower, list: &[Node], side: &Side, c: char) -> R<()> {
        sh.prefix.push(c);
        let r = self.show_node_list(sh, list, side);
        sh.prefix.pop();
        r
    }

    /// The font identifier of font `f` (§174's `print_font_and_char`).
    fn font_ident(&mut self, sh: &mut Shower, f: u32) -> R<()> {
        let x = self.font_get(i64::from(f), crate::fields::FontCell::Id)?;
        let name = x.map_or_else(|| "nullfont".to_owned(), |x| str_of(self.v, x));
        sh.esc(&name);
        Ok(())
    }

    /// The tokens of a mark or a `\write` (`print_mark`, §176).
    fn show_mark(&mut self, sh: &mut Shower, toks: ValueId) -> R<()> {
        let t = crate::val::toks_of(self.v, Some(toks));
        let mut s = String::new();
        self.show_toks_into(&mut s, &t)?;
        sh.push("{");
        sh.push(&s);
        sh.push("}");
        Ok(())
    }

    /// `display node p` (§183).
    #[allow(clippy::too_many_lines)]
    fn display_node(&mut self, sh: &mut Shower, x: &Node, side: &Side) -> R<()> {
        match x {
            Node::Char { font, ch } => {
                self.font_ident(sh, *font)?;
                sh.push(" ");
                crate::print::push_code(sh.line(), u32::from(*ch));
            }
            Node::Box(b) => {
                sh.esc(if b.kind == BoxKind::H { "h" } else { "v" });
                sh.push("box(");
                sh.scaled(b.height);
                sh.push("+");
                sh.scaled(b.depth);
                sh.push(")x");
                sh.scaled(b.width);
                // §186: the glue setting
                let g = b.glue.ratio;
                if g != 0.0 && b.glue.sign != GlueSign::Normal {
                    sh.push(", glue set ");
                    if b.glue.sign == GlueSign::Shrinking {
                        sh.push("- ");
                    }
                    if g.abs() > 20000.0 {
                        sh.push(if g > 0.0 { ">" } else { "< -" });
                        print_glue(sh.line(), 20000 * 65536, b.glue.order, "");
                    } else {
                        #[allow(clippy::cast_possible_truncation)]
                        let r = (65536.0 * g).round() as i32;
                        print_glue(sh.line(), r, b.glue.order, "");
                    }
                }
                if b.shift != 0 {
                    sh.push(", shifted ");
                    sh.scaled(b.shift);
                }
                self.node_list_display(sh, &b.list, side, '.')?;
            }
            Node::Unset(u) => {
                sh.esc("unset");
                sh.push("box(");
                sh.scaled(u.height);
                sh.push("+");
                sh.scaled(u.depth);
                sh.push(")x");
                sh.scaled(u.width);
                // §185
                if u.span_count != 0 {
                    sh.push(" (");
                    print_int(sh.line(), i64::from(u.span_count) + 1);
                    sh.push(" columns)");
                }
                if u.stretch != 0 {
                    sh.push(", stretch ");
                    print_glue(sh.line(), u.stretch, u.stretch_order, "");
                }
                if u.shrink != 0 {
                    sh.push(", shrink ");
                    print_glue(sh.line(), u.shrink, u.shrink_order, "");
                }
                self.node_list_display(sh, &u.list, side, '.')?;
            }
            Node::Rule(r) => {
                // §187
                sh.esc("rule(");
                sh.rule_dimen(r.height);
                sh.push("+");
                sh.rule_dimen(r.depth);
                sh.push(")x");
                sh.rule_dimen(r.width);
            }
            Node::Ins(i) => {
                // §188
                sh.esc("insert");
                print_int(sh.line(), i64::from(i.number));
                sh.push(", natural size ");
                sh.scaled(i.height);
                sh.push("; split(");
                print_spec(sh.line(), &i.split_top_skip, "");
                sh.push(",");
                sh.scaled(i.depth);
                sh.push("); float cost ");
                print_int(sh.line(), i64::from(i.float_cost));
                self.node_list_display(sh, &i.list, side, '.')?;
            }
            Node::Mark(k) => {
                // §196, e-TeX's class
                let p = side
                    .marks
                    .get(*k as usize)
                    .copied()
                    .unwrap_or(ValueId::UNIT);
                let xs = list_of(self.v, p);
                sh.esc("mark");
                let toks = if xs.len() == 3 && int_of(self.v, xs[0]) == -1 {
                    sh.push("s");
                    print_int(sh.line(), int_of(self.v, xs[1]));
                    xs[2]
                } else {
                    p
                };
                self.show_mark(sh, toks)?;
            }
            Node::Adjust(l) => {
                // §197
                sh.esc("vadjust");
                self.node_list_display(sh, l, side, '.')?;
            }
            Node::Lig(l) => {
                // §193
                self.font_ident(sh, l.font)?;
                sh.push(" ");
                crate::print::push_code(sh.line(), u32::from(l.ch));
                sh.push(" (ligature ");
                if l.left_hit {
                    sh.push("|");
                }
                for &c in &l.original {
                    crate::print::push_code(sh.line(), u32::from(c));
                }
                if l.right_hit {
                    sh.push("|");
                }
                sh.push(")");
            }
            Node::Disc(d) => {
                // §195
                sh.esc("discretionary");
                if !d.replace.is_empty() {
                    sh.push(" replacing ");
                    print_int(sh.line(), i64::try_from(d.replace.len()).unwrap_or(0));
                }
                self.node_list_display(sh, &d.pre, side, '.')?;
                self.node_list_display(sh, &d.post, side, '|')?;
            }
            Node::Whatsit(k, _) => {
                let w = side
                    .whats
                    .get(*k as usize)
                    .copied()
                    .unwrap_or(ValueId::UNIT);
                self.display_whatsit(sh, w)?;
            }
            Node::Language { lang, lhm, rhm } => {
                // §1356
                sh.esc("setlanguage");
                print_int(sh.line(), i64::from(*lang));
                sh.push(" (hyphenmin ");
                print_int(sh.line(), i64::from(*lhm));
                sh.push(",");
                print_int(sh.line(), i64::from(*rhm));
                sh.push(")");
            }
            Node::Math(k, w) => {
                // §192, e-TeX's
                match k {
                    MathKind::Before | MathKind::After => {
                        sh.esc("math");
                        sh.push(if *k == MathKind::Before { "on" } else { "off" });
                        if *w != 0 {
                            sh.push(", surrounded ");
                            sh.scaled(*w);
                        }
                    }
                    _ => {
                        let end = matches!(k, MathKind::EndM | MathKind::EndL | MathKind::EndR);
                        sh.esc(if end { "end" } else { "begin" });
                        sh.push(match k {
                            MathKind::BeginR | MathKind::EndR => "R",
                            MathKind::BeginL | MathKind::EndL => "L",
                            _ => "M",
                        });
                    }
                }
            }
            Node::Glue(g) => match g.kind {
                GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders => {
                    // §190
                    sh.esc("");
                    match g.kind {
                        GlueKind::CLeaders => sh.push("c"),
                        GlueKind::XLeaders => sh.push("x"),
                        _ => {}
                    }
                    sh.push("leaders ");
                    print_spec(sh.line(), &g.spec, "");
                    let l: Vec<Node> = g.leader.as_deref().cloned().into_iter().collect();
                    self.node_list_display(sh, &l, side, '.')?;
                }
                _ => {
                    // §189
                    sh.esc("glue");
                    match g.kind {
                        GlueKind::Param(c) => {
                            sh.push("(");
                            match SKIP_PARAMS.get(usize::from(c)) {
                                Some(n) => sh.esc(n),
                                None => sh.push("[unknown glue parameter!]"),
                            }
                            sh.push(")");
                        }
                        GlueKind::CondMath => {
                            sh.push("(");
                            sh.esc("nonscript");
                            sh.push(")");
                        }
                        GlueKind::Mu => {
                            sh.push("(");
                            sh.esc("mskip");
                            sh.push(")");
                        }
                        _ => {}
                    }
                    if g.kind != GlueKind::CondMath {
                        sh.push(" ");
                        print_spec(
                            sh.line(),
                            &g.spec,
                            if g.kind == GlueKind::Mu { "mu" } else { "" },
                        );
                    }
                }
            },
            Node::Kern(w, k) => match k {
                // §191, pdfTeX's margin kerns
                KernKind::Mu => {
                    sh.esc("mkern");
                    sh.scaled(*w);
                    sh.push("mu");
                }
                KernKind::Margin { left, .. } => {
                    sh.esc("kern");
                    sh.scaled(*w);
                    sh.push(if *left {
                        " (left margin)"
                    } else {
                        " (right margin)"
                    });
                }
                _ => {
                    sh.esc("kern");
                    if *k != KernKind::Normal {
                        sh.push(" ");
                    }
                    sh.scaled(*w);
                    if *k == KernKind::AccKern {
                        sh.push(" (for accent)");
                    }
                }
            },
            Node::Penalty(p) => {
                // §194
                sh.esc("penalty ");
                print_int(sh.line(), i64::from(*p));
            }
            Node::Native(n) => {
                // XeTeX's `print_native_word`, a glyph by its id
                self.font_ident(sh, n.font)?;
                sh.push(" ");
                if n.kind == NativeKind::Glyph {
                    sh.push("glyph#");
                    print_int(sh.line(), n.glyphs.first().map_or(0, |g| i64::from(g.gid)));
                } else {
                    let s = String::from_utf16_lossy(&n.text);
                    sh.push(&s);
                }
            }
        }
        Ok(())
    }

    /// A whatsit (§1356, pdfTeX's): its kind and what it carries.
    fn display_whatsit(&mut self, sh: &mut Shower, w: ValueId) -> R<()> {
        let xs = list_of(self.v, w);
        let kind = xs.first().map(|&k| str_of(self.v, k)).unwrap_or_default();
        // `print_write_whatsit`: the stream, `*` for 16, `-` past it
        let stream = |sh: &mut Shower, n: i64| {
            if (0..16).contains(&n) {
                print_int(sh.line(), n);
            } else if n == 16 {
                sh.push("*");
            } else {
                sh.push("-");
            }
        };
        match kind.as_str() {
            "open" => {
                sh.esc("openout");
                stream(sh, xs.get(1).map_or(0, |&n| int_of(self.v, n)));
                sh.push("=");
                let name = xs.get(2).map(|&n| str_of(self.v, n)).unwrap_or_default();
                sh.push(&name);
            }
            "write" => {
                sh.esc("write");
                stream(sh, xs.get(1).map_or(0, |&n| int_of(self.v, n)));
                self.show_mark(sh, xs.get(2).copied().unwrap_or(ValueId::UNIT))?;
            }
            "close" => {
                sh.esc("closeout");
                stream(sh, xs.get(1).map_or(0, |&n| int_of(self.v, n)));
            }
            "special" => {
                sh.esc("special");
                let s = xs.get(1).map(|&n| str_of(self.v, n)).unwrap_or_default();
                sh.push("{");
                sh.push(&s);
                sh.push("}");
            }
            _ => sh.push("whatsit?"),
        }
        Ok(())
    }
}
