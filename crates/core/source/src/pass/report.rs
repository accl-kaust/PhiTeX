//! A pass summed up: counts by status, the longest chain of pending
//! structure, and the causes.

use std::collections::BTreeMap;

use super::atoms::AtomKind;
use super::lex::TokKind;
use super::{Pass, Status};

/// Counts for one text or many.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub bytes: usize,
    pub tokens: usize,
    /// Control sequences (operands included).
    pub cs: usize,
    pub brace_pairs: usize,
    /// Items that consume something, by status (Known, Predicted, Pending).
    pub args: [usize; 3],
    /// Every classified item, by status.
    pub items: [usize; 3],
    /// Brace pairs and written group pairs, by status.
    pub groups: [usize; 3],
    /// Bytes whose tokenization is pending, and predicted.
    pub hazard_bytes: [usize; 2],
    /// The longest chain of pending items, each in the reach of the one
    /// before (the rest of its brace group).
    pub chain: usize,
    /// The same, if a blank line (`\par`) ends every reach (true of
    /// macros not `\long`).
    pub chain_par: usize,
    /// Pending items, in all.
    pub pending_items: usize,
    pub causes: BTreeMap<String, usize>,
    pub pending_names: BTreeMap<String, usize>,
}

fn ix(s: Status) -> usize {
    match s {
        Status::Known => 0,
        Status::Predicted => 1,
        Status::Pending => 2,
    }
}

impl Stats {
    #[must_use]
    pub fn of(pass: &Pass, text: &str) -> Stats {
        let mut s = Stats {
            bytes: text.len(),
            tokens: pass.tokens.len(),
            cs: pass.tokens.iter().filter(|t| t.kind == TokKind::Cs).count(),
            brace_pairs: pass.braces.len(),
            ..Stats::default()
        };
        for b in &pass.braces {
            s.groups[ix(b.status)] += 1;
        }
        for g in &pass.groups {
            s.groups[ix(g.status)] += 1;
        }
        for t in &pass.tokens {
            match t.tz {
                Status::Pending => s.hazard_bytes[0] += t.end - t.start,
                Status::Predicted => s.hazard_bytes[1] += t.end - t.start,
                Status::Known => {}
            }
        }
        // (pending items' atoms, for the chain)
        let mut pending_at = vec![false; pass.atoms.len()];
        for it in &pass.items {
            s.items[ix(it.status)] += 1;
            if it.consumes {
                s.args[ix(it.status)] += 1;
            }
            if it.status == Status::Pending || it.hazard {
                pending_at[it.atom] = true;
                s.pending_items += 1;
                let (kind, name) = match (&it.cause, it.hazard && it.status != Status::Pending) {
                    (_, true) => ("catcode hazard", it.name.as_str()),
                    (Some(c), _) => (c.kind, c.detail.as_str()),
                    (None, _) => ("unknown", ""),
                };
                *s.causes.entry(kind.to_owned()).or_default() += 1;
                if !name.is_empty() {
                    *s.pending_names
                        .entry(format!("{kind}: \\{name}"))
                        .or_default() += 1;
                }
            }
        }
        let (mut best, mut best_par) = (0usize, 0usize);
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for (a, at) in pass.atoms.iter().enumerate() {
            match at.kind {
                AtomKind::Open => stack.push((best, best_par)),
                AtomKind::Close => {
                    if let Some((b, bp)) = stack.pop() {
                        best = b;
                        best_par = bp;
                    }
                }
                AtomKind::Par => best_par = stack.last().map_or(0, |x| x.1),
                _ => {}
            }
            if pending_at[a] {
                best += 1;
                best_par += 1;
                s.chain = s.chain.max(best);
                s.chain_par = s.chain_par.max(best_par);
            }
        }
        s
    }

    /// Adds another text's counts (chains: the longest).
    pub fn add(&mut self, o: &Stats) {
        self.bytes += o.bytes;
        self.tokens += o.tokens;
        self.cs += o.cs;
        self.brace_pairs += o.brace_pairs;
        for k in 0..3 {
            self.args[k] += o.args[k];
            self.items[k] += o.items[k];
            self.groups[k] += o.groups[k];
        }
        for k in 0..2 {
            self.hazard_bytes[k] += o.hazard_bytes[k];
        }
        self.chain = self.chain.max(o.chain);
        self.chain_par = self.chain_par.max(o.chain_par);
        self.pending_items += o.pending_items;
        for (k, v) in &o.causes {
            *self.causes.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &o.pending_names {
            *self.pending_names.entry(k.clone()).or_default() += v;
        }
    }
}
