//! A pass summed up: counts by status, the longest chain of pending
//! structure, and the causes.

use std::collections::BTreeMap;

use super::atoms::AtomKind;
use super::lex::TokKind;
use super::{Claim, Implicit, MayDefine, Pass, ScopeHint, Status};

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
    /// Items by what they may define: Nothing, Names, Prefixes, Any.
    pub may_define: [usize; 4],
    /// Items and marks by scope hint: None, Open, Close, Predicted,
    /// Pending.
    pub scope: [usize; 5],
    /// Items and marks by paragraph effect: None, Starts, Ends.
    pub implicit: [usize; 3],
    /// Items by claim class: Known, Predicted, Pending.
    pub claims: [usize; 3],
    /// Marks, in all.
    pub marks: usize,
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
        Stats::of_range(pass, text, 0..text.len())
    }

    /// The counts for the bytes `range` of the text (an item or pair is in
    /// it where it begins).
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn of_range(pass: &Pass, _text: &str, range: std::ops::Range<usize>) -> Stats {
        let in_tok = |k: usize| range.contains(&pass.tokens[k].start);
        let in_atom = |a: usize| range.contains(&pass.atoms[a].start);
        let mut s = Stats {
            bytes: range.len(),
            tokens: pass
                .tokens
                .iter()
                .filter(|t| range.contains(&t.start))
                .count(),
            cs: pass
                .tokens
                .iter()
                .filter(|t| t.kind == TokKind::Cs && range.contains(&t.start))
                .count(),
            brace_pairs: pass.braces.iter().filter(|b| in_tok(b.open)).count(),
            ..Stats::default()
        };
        for b in pass.braces.iter().filter(|b| in_tok(b.open)) {
            s.groups[ix(b.status)] += 1;
        }
        for g in pass
            .groups
            .iter()
            .filter(|g| g.open.or(g.close).is_some_and(in_atom))
        {
            s.groups[ix(g.status)] += 1;
        }
        for t in pass.tokens.iter().filter(|t| range.contains(&t.start)) {
            match t.tz {
                Status::Pending => s.hazard_bytes[0] += t.end - t.start,
                Status::Predicted => s.hazard_bytes[1] += t.end - t.start,
                Status::Known => {}
            }
        }
        for it in pass.items.iter().filter(|it| in_atom(it.atom)) {
            let h = &it.hints;
            s.may_define[match h.may_define {
                MayDefine::Nothing => 0,
                MayDefine::Names(_) => 1,
                MayDefine::Prefixes(_) => 2,
                MayDefine::Any => 3,
            }] += 1;
            s.claims[match h.claim {
                Claim::Known(_) => 0,
                Claim::Predicted { .. } => 1,
                Claim::Pending { .. } => 2,
            }] += 1;
        }
        let both = pass
            .items
            .iter()
            .filter(|it| in_atom(it.atom))
            .map(|it| &it.hints)
            .chain(
                pass.marks
                    .iter()
                    .filter(|m| in_atom(m.atom))
                    .map(|m| &m.hints),
            );
        for h in both {
            s.scope[match h.scope {
                ScopeHint::None => 0,
                ScopeHint::Open(_) => 1,
                ScopeHint::Close => 2,
                ScopeHint::Predicted { .. } => 3,
                ScopeHint::Pending { .. } => 4,
            }] += 1;
            s.implicit[match h.implicit {
                Implicit::None => 0,
                Implicit::StartsParagraph => 1,
                Implicit::EndsParagraph => 2,
            }] += 1;
        }
        s.marks = pass.marks.iter().filter(|m| in_atom(m.atom)).count();
        // (pending items' atoms, for the chain)
        let mut pending_at = vec![false; pass.atoms.len()];
        for it in pass.items.iter().filter(|it| in_atom(it.atom)) {
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
        for (a, at) in pass.atoms.iter().enumerate().filter(|(a, _)| in_atom(*a)) {
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
        for k in 0..4 {
            self.may_define[k] += o.may_define[k];
        }
        for k in 0..5 {
            self.scope[k] += o.scope[k];
        }
        for k in 0..3 {
            self.implicit[k] += o.implicit[k];
            self.claims[k] += o.claims[k];
        }
        self.marks += o.marks;
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
