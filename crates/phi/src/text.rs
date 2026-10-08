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
        let mut c = self.n.first[n as usize];
        while c != NONE {
            out.push(c);
            self.walk(c, out);
            c = self.n.next[c as usize];
        }
    }

    fn opd_text(&self, o: &Opd, num: &HashMap<u32, usize>) -> String {
        let mut s = String::new();
        if o.name != NONE {
            let _ = write!(s, "@{}:", String::from_utf8_lossy(&self.names_spell(o.name)));
        }
        if o.src == NONE {
            s += "undef";
        } else {
            let _ = write!(s, "%{}", num.get(&o.src).map_or_else(|| format!("?{}", o.src), ToString::to_string));
        }
        if !o.sel.is_whole() {
            let _ = write!(s, ".{}", o.sel.0);
        }
        s
    }

    fn line(&self, n: u32, num: &HashMap<u32, usize>, ids: bool, out: &mut String) {
        let u = n as usize;
        let depth = self.n.depth[u] as usize;
        for _ in 1..depth {
            out.push_str("  ");
        }
        let kind = match self.n.kind[u] {
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
        match self.n.class[u] {
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
        if matches!(self.n.kind[u], Kind::Leaf | Kind::Unfold | Kind::Scan) {
            let _ = write!(out, " {}", L::fmt_op(self.n.op[u]));
        }
        match self.n.kind[u] {
            Kind::Cross => {
                let _ = write!(out, " #{:x}", self.n.aux[u]);
            }
            Kind::ChainRead | Kind::Family => {
                let _ = write!(out, " {}", self.n.aux[u]);
            }
            Kind::Step => {
                let si = &self.steps[self.n.aux[u] as usize];
                if ids {
                    let _ = write!(out, " #{:x} at {}", si.key, si.at.0);
                }
                let _ = write!(out, " took {}", si.took);
            }
            _ => {}
        }
        let os: Vec<String> = self.n.opds_of(n).iter().map(|o| self.opd_text(o, num)).collect();
        if !os.is_empty() {
            let _ = write!(out, "({})", os.join(", "));
        }
        let _ = writeln!(out, " = {}", L::fmt_val(&self.n.val[u]));
        if self.n.kind[u] == Kind::Step {
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
