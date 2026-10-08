//! The text form (DESIGN 7.14): one node per line in position order,
//! indented by region, with canonical numbers (a node's place in that
//! order), so two graphs of the same program print the same.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::graph::{Graph, Kind, NONE, Opd};
use crate::lang::{Class, Lang};
use crate::value::Value;

impl<L: Lang> Graph<L> {
    /// The canonical text of the graph. Element identities and keys are
    /// left out: they decide reuse, never values ([`Graph::to_text_ids`]
    /// shows them).
    #[must_use]
    pub fn to_text(&self) -> String {
        self.text(false)
    }

    /// The text with steps' keys and cursors.
    #[must_use]
    pub fn to_text_ids(&self) -> String {
        self.text(true)
    }

    fn text(&self, ids: bool) -> String {
        let mut order = Vec::new();
        self.walk(0, &mut order);
        let num: HashMap<u32, usize> = order.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let mut out = String::new();
        for &n in &order {
            self.line(n, &num, ids, &mut out);
        }
        out
    }

    fn walk(&self, n: u32, out: &mut Vec<u32>) {
        let mut c = self.first(n);
        while c != NONE {
            out.push(c);
            self.walk(c, out);
            c = self.n.h[c as usize].next;
        }
    }

    fn opd_text(&self, o: &Opd, num: &HashMap<u32, usize>) -> String {
        let mut s = String::new();
        if o.name != NONE {
            let _ = write!(
                s,
                "@{}:",
                String::from_utf8_lossy(&self.names_spell(o.name))
            );
        }
        if o.src == NONE {
            s += "undef";
        } else {
            let _ = write!(
                s,
                "%{}",
                num.get(&o.src)
                    .map_or_else(|| format!("?{}", o.src), ToString::to_string)
            );
        }
        if !o.sel.is_whole() {
            let _ = write!(s, ".{}", o.sel.0);
        }
        s
    }

    fn line(&self, n: u32, num: &HashMap<u32, usize>, ids: bool, out: &mut String) {
        let u = n as usize;
        let depth = self.n.h[u].depth as usize;
        for _ in 1..depth {
            out.push_str("  ");
        }
        let kind = match self.n.h[u].kind {
            Kind::Root => "root",
            Kind::Input => "input",
            Kind::Const => "const",
            Kind::Leaf => "leaf",
            Kind::Unfold => "unfold",
            Kind::Step => "step",
            Kind::Scan => "scan",
            Kind::Cross => "cross",
            Kind::ChainRead => "chain",
            Kind::Family => "family",
        };
        let _ = write!(out, "%{} = {kind}", num[&n]);
        match self.n.class(n) {
            Class::Pure => {}
            Class::Effect(c) => {
                let _ = write!(out, " effect({})", c.0);
            }
            Class::Barrier => out.push_str(" barrier"),
            Class::Publish(s) => {
                let _ = write!(out, " publish(#{:x})", s.0);
            }
            Class::Entry(f, s) => {
                let _ = write!(out, " entry({}, #{:x})", f.0, s.0);
            }
        }
        if matches!(self.n.h[u].kind, Kind::Leaf | Kind::Unfold | Kind::Scan) {
            let _ = write!(out, " {}", L::fmt_op(self.n.h[u].op));
        }
        match self.n.h[u].kind {
            Kind::Cross => {
                let _ = write!(out, " #{:x}", self.n.h[u].aux);
            }
            Kind::ChainRead | Kind::Family => {
                let aux = self.n.h[u].aux;
                if aux >> 32 != 0 {
                    out.push_str(" before");
                }
                let _ = write!(out, " {}", aux & 0xffff_ffff);
            }
            Kind::Step => {
                let si = &self.steps[self.n.h[u].aux as usize];
                if ids {
                    let _ = write!(out, " #{:x} at {}", si.key, si.at.0);
                }
                if self.sealed(n) {
                    out.push_str(" sealed");
                }
                let _ = write!(out, " took {}", si.took);
            }
            _ => {}
        }
        let os: Vec<String> = self
            .n
            .opds_of(n)
            .iter()
            .map(|o| self.opd_text(o, num))
            .collect();
        if !os.is_empty() {
            let _ = write!(out, "({})", os.join(", "));
        }
        let _ = writeln!(out, " = {}", L::fmt_val(&self.n.val[u]));
        if self.n.h[u].kind == Kind::Step {
            for d in self.step_defs(n) {
                for _ in 0..depth {
                    out.push_str("  ");
                }
                let o = Opd {
                    src: d.0,
                    sel: d.1,
                    name: NONE,
                };
                let _ = writeln!(
                    out,
                    "def @{} = {}{}",
                    String::from_utf8_lossy(&self.names_spell(d.2)),
                    self.opd_text(&o, num),
                    if d.3 { " global" } else { "" }
                );
            }
        }
        let _ = self.n.val[u].ver();
    }
}

/// One line of the text form, parsed.
#[derive(Clone, Debug, PartialEq)]
pub enum Line<V> {
    Node {
        depth: usize,
        num: usize,
        /// Kind, class, op and step details, as printed.
        head: String,
        /// Operands: (`@name:` if read by name, the node's number or
        /// `None` if undefined, the field if any).
        opds: Vec<(Option<String>, Option<usize>, Option<u32>)>,
        val: String,
        /// The value, if the client parses its text.
        parsed: Option<V>,
    },
    Def {
        depth: usize,
        name: String,
        src: Option<usize>,
        field: Option<u32>,
        global: bool,
    },
}

/// The text form parsed back: printing it gives the same text.
#[derive(Clone, Debug, PartialEq)]
pub struct Dump<V> {
    pub lines: Vec<Line<V>>,
}

fn parse_opd(s: &str) -> Option<(Option<String>, Option<usize>, Option<u32>)> {
    let (name, rest) = match s.strip_prefix('@') {
        Some(r) => {
            let (n, r) = r.rsplit_once(':')?;
            (Some(n.to_string()), r)
        }
        None => (None, s),
    };
    let (node, field) = match rest.split_once('.') {
        Some((a, f)) => (a, Some(f.parse().ok()?)),
        None => (rest, None),
    };
    let src = if node == "undef" {
        None
    } else {
        Some(node.strip_prefix('%')?.parse().ok()?)
    };
    Some((name, src, field))
}

fn print_opd(o: &(Option<String>, Option<usize>, Option<u32>)) -> String {
    let mut s = String::new();
    if let Some(n) = &o.0 {
        let _ = write!(s, "@{n}:");
    }
    match o.1 {
        Some(k) => {
            let _ = write!(s, "%{k}");
        }
        None => s.push_str("undef"),
    }
    if let Some(f) = o.2 {
        let _ = write!(s, ".{f}");
    }
    s
}

impl<V> Dump<V> {
    /// Parse the text form. Values are parsed with `parse` where it can.
    pub fn parse(text: &str, parse: impl Fn(&str) -> Option<V>) -> Result<Self, String> {
        let mut lines = Vec::new();
        for (k, l) in text.lines().enumerate() {
            let body = l.trim_start_matches(' ');
            let indent = l.len() - body.len();
            if indent % 2 != 0 {
                return Err(format!("line {}: odd indent", k + 1));
            }
            if let Some(d) = body.strip_prefix("def @") {
                let (name, rest) = d.split_once(" = ").ok_or(format!("line {}: def", k + 1))?;
                let (o, global) = match rest.strip_suffix(" global") {
                    Some(o) => (o, true),
                    None => (rest, false),
                };
                let (_, src, field) = parse_opd(o).ok_or(format!("line {}: operand", k + 1))?;
                lines.push(Line::Def {
                    depth: indent / 2,
                    name: name.to_string(),
                    src,
                    field,
                    global,
                });
                continue;
            }
            let rest = body
                .strip_prefix('%')
                .ok_or(format!("line {}: no node", k + 1))?;
            let (num, rest) = rest
                .split_once(" = ")
                .ok_or(format!("line {}: no '='", k + 1))?;
            let num: usize = num.parse().map_err(|_| format!("line {}: number", k + 1))?;
            let (desc, val) = rest
                .split_once(" = ")
                .ok_or(format!("line {}: no value", k + 1))?;
            let (head, opds) = match desc.strip_suffix(')').and_then(|d| d.rsplit_once('(')) {
                Some((h, os)) => {
                    let opds = os
                        .split(", ")
                        .map(|o| parse_opd(o).ok_or(format!("line {}: operand {o}", k + 1)))
                        .collect::<Result<Vec<_>, _>>()?;
                    (h.to_string(), opds)
                }
                None => (desc.to_string(), Vec::new()),
            };
            lines.push(Line::Node {
                depth: indent / 2 + 1,
                num,
                head,
                opds,
                val: val.to_string(),
                parsed: parse(val),
            });
        }
        Ok(Dump { lines })
    }

    /// The text again.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for l in &self.lines {
            match l {
                Line::Node {
                    depth,
                    num,
                    head,
                    opds,
                    val,
                    ..
                } => {
                    for _ in 1..*depth {
                        out.push_str("  ");
                    }
                    let _ = write!(out, "%{num} = {head}");
                    if !opds.is_empty() {
                        let os: Vec<String> = opds.iter().map(print_opd).collect();
                        let _ = write!(out, "({})", os.join(", "));
                    }
                    let _ = writeln!(out, " = {val}");
                }
                Line::Def {
                    depth,
                    name,
                    src,
                    field,
                    global,
                } => {
                    for _ in 0..*depth {
                        out.push_str("  ");
                    }
                    let _ = writeln!(
                        out,
                        "def @{name} = {}{}",
                        print_opd(&(None, *src, *field)),
                        if *global { " global" } else { "" }
                    );
                }
            }
        }
        out
    }
}
