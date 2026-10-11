//! The analyzer: the def-use graph over a trace and its shape.
//!
//! A read binds to the latest preceding write of its field (a group end
//! writes the outer values back, so a read after the group sees them);
//! `"?"` reads bind to nothing (the parser's `--conservative` turns them
//! into reads of `seq:prev`, the step before). Edges go from the writing
//! step to the reading step; the graph is a DAG in step order.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{self, BufRead};

use serde::{Deserialize, Serialize};

use crate::hash::{fnv1a, fnv1a_with};
use crate::record::same_file;
use crate::step::{Kind, Record};

const UNKNOWN: u32 = u32::MAX;

/// A step, compact.
struct Node {
    kind: Kind,
    name: u32,
    line: Option<u32>,
    in_main: bool,
    reads: (u32, u32),
    writes: (u32, u32),
    /// A macro call: (name, arguments hash).
    call: Option<(u32, u64)>,
}

/// The loaded trace.
pub struct Graph {
    nodes: Vec<Node>,
    reads: Vec<u32>,
    writes: Vec<(u32, u64)>,
    fields: Vec<String>,
    names: Vec<String>,
    /// Edges (from, to, field), in order of `to`.
    edges: Vec<(u32, u32, u32)>,
    pred_start: Vec<u32>,
    succ_start: Vec<u32>,
    succ: Vec<u32>,
    /// The longest def-use chain ending at each step (in steps).
    longest: Vec<u32>,
    /// Macro calls: (name, args hash, hash of read values) per call.
    calls: Vec<(u32, u64, u64)>,
    main: String,
}

struct Interner {
    map: HashMap<String, u32>,
    names: Vec<String>,
}

impl Interner {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            names: Vec::new(),
        }
    }

    fn get(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = u32::try_from(self.names.len()).expect("fewer than 2^32 names");
        self.map.insert(s.to_owned(), i);
        self.names.push(s.to_owned());
        i
    }
}

fn idx(n: usize) -> u32 {
    u32::try_from(n).expect("fewer than 2^32 steps")
}

impl Graph {
    /// Loads a trace (JSONL) and builds the graph.
    pub fn load<R: BufRead>(reader: R) -> io::Result<Self> {
        let mut fields = Interner::new();
        let mut names = Interner::new();
        let mut nodes = Vec::new();
        let mut reads = Vec::new();
        let mut writes = Vec::new();
        let mut main = String::new();
        let unknown_field = fields.get("?");
        debug_assert_eq!(unknown_field, 0);
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let rec: Record = serde_json::from_str(&line).map_err(io::Error::other)?;
            match rec {
                Record::Meta { meta } => main = meta.main,
                Record::Step(s) => {
                    let r0 = idx(reads.len());
                    for r in &s.reads {
                        reads.push(if r == "?" { UNKNOWN } else { fields.get(r) });
                    }
                    let w0 = idx(writes.len());
                    for w in &s.writes {
                        writes.push((fields.get(&w.field), w.value));
                    }
                    nodes.push(Node {
                        kind: s.kind,
                        name: names.get(&s.name),
                        line: s.line,
                        in_main: s.file.as_deref().is_some_and(|f| same_file(f, &main)),
                        reads: (r0, idx(reads.len())),
                        writes: (w0, idx(writes.len())),
                        call: s
                            .macro_call
                            .as_ref()
                            .map(|c| (names.get(&c.r#macro), c.args_text_hash)),
                    });
                }
            }
        }
        let mut g = Self {
            nodes,
            reads,
            writes,
            fields: fields.names,
            names: names.names,
            edges: Vec::new(),
            pred_start: Vec::new(),
            succ_start: Vec::new(),
            succ: Vec::new(),
            longest: Vec::new(),
            calls: Vec::new(),
            main,
        };
        g.build();
        Ok(g)
    }

    fn build(&mut self) {
        let n = self.nodes.len();
        let mut last: Vec<Option<u32>> = vec![None; self.fields.len()];
        let mut value: Vec<u64> = vec![0; self.fields.len()];
        self.pred_start = Vec::with_capacity(n + 1);
        self.longest = Vec::with_capacity(n);
        let mut call_hash = fnv1a(b"");
        let mut open_call: Option<(u32, u64)> = None;
        for i in 0..n {
            self.pred_start.push(idx(self.edges.len()));
            let node = &self.nodes[i];
            if let Some(c) = node.call {
                if let Some((name, args)) = open_call.take() {
                    self.calls.push((name, args, call_hash));
                }
                open_call = Some(c);
                call_hash = fnv1a(b"");
            }
            let mut best = 0;
            for k in node.reads.0..node.reads.1 {
                let f = self.reads[k as usize];
                if f == UNKNOWN {
                    continue;
                }
                call_hash = fnv1a_with(call_hash, &f.to_le_bytes());
                call_hash = fnv1a_with(call_hash, &value[f as usize].to_le_bytes());
                if let Some(w) = last[f as usize] {
                    self.edges.push((w, idx(i), f));
                    best = best.max(self.longest[w as usize]);
                }
            }
            self.longest.push(best + 1);
            for k in node.writes.0..node.writes.1 {
                let (f, v) = self.writes[k as usize];
                last[f as usize] = Some(idx(i));
                value[f as usize] = v;
            }
        }
        self.pred_start.push(idx(self.edges.len()));
        if let Some((name, args)) = open_call {
            self.calls.push((name, args, call_hash));
        }
        // Successors, CSR.
        let mut count = vec![0u32; n + 1];
        for &(from, _, _) in &self.edges {
            count[from as usize + 1] += 1;
        }
        for i in 1..=n {
            count[i] += count[i - 1];
        }
        self.succ_start.clone_from(&count);
        self.succ = vec![0; self.edges.len()];
        for &(from, to, _) in &self.edges {
            let slot = count[from as usize] as usize;
            self.succ[slot] = to;
            count[from as usize] += 1;
        }
    }

    fn preds(&self, i: usize) -> &[(u32, u32, u32)] {
        &self.edges[self.pred_start[i] as usize..self.pred_start[i + 1] as usize]
    }

    fn succs(&self, i: usize) -> &[u32] {
        &self.succ[self.succ_start[i] as usize..self.succ_start[i + 1] as usize]
    }

    /// Steps transitively dependent on any of `seeds`, the steps in
    /// `exclude` (the seeds' own line) not counted.
    fn dependents(&self, seeds: &[u32], exclude: &[u32], visited: &mut [u64]) -> u64 {
        visited.fill(0);
        let mut stack: Vec<u32> = seeds.to_vec();
        for &s in seeds.iter().chain(exclude) {
            visited[s as usize / 64] |= 1 << (s % 64);
        }
        let mut count = 0;
        while let Some(s) = stack.pop() {
            for &t in self.succs(s as usize) {
                let (w, b) = (t as usize / 64, 1u64 << (t % 64));
                if visited[w] & b == 0 {
                    visited[w] |= b;
                    count += 1;
                    stack.push(t);
                }
            }
        }
        count
    }

    /// The step of `\begin{document}`, if any.
    fn begin_document(&self) -> Option<usize> {
        let begin = self.names.iter().position(|n| n == "\\begin")?;
        let args = fnv1a_with(fnv1a_with(fnv1a(b""), b"document"), b"\0");
        self.nodes
            .iter()
            .position(|n| n.call == Some((idx(begin), args)))
    }

    /// How many steps the trace has.
    #[must_use]
    pub fn steps(&self) -> usize {
        self.nodes.len()
    }

    /// A step's name (the primitive's, the macro's, the character class).
    #[must_use]
    pub fn name(&self, i: usize) -> &str {
        &self.names[self.nodes[i].name as usize]
    }

    /// A step's kind.
    #[must_use]
    pub fn kind(&self, i: usize) -> Kind {
        self.nodes[i].kind
    }

    /// A step's input line, as the log last reported it.
    #[must_use]
    pub fn line(&self, i: usize) -> Option<u32> {
        self.nodes[i].line
    }

    /// The steps a step's reads bind to, with the field read.
    pub fn deps(&self, i: usize) -> impl Iterator<Item = (usize, &str)> + '_ {
        self.preds(i)
            .iter()
            .map(|&(from, _, f)| (from as usize, self.fields[f as usize].as_str()))
    }

    /// The longest def-use chain over the whole trace, in steps.
    #[must_use]
    pub fn critical_path(&self) -> u64 {
        u64::from(self.longest.iter().copied().max().unwrap_or(0))
    }

    fn is_par(&self, i: usize) -> bool {
        self.nodes[i].kind == Kind::Primitive && self.names[self.nodes[i].name as usize] == "\\par"
    }
}

/// Quantiles of a distribution.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Quantiles {
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
}

fn quantiles(values: &mut [f64]) -> Quantiles {
    if values.is_empty() {
        return Quantiles::default();
    }
    values.sort_by(f64::total_cmp);
    let at = |q: f64| {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let i = ((values.len() - 1) as f64 * q).round() as usize;
        values[i]
    };
    Quantiles {
        p50: at(0.5),
        p90: at(0.9),
        p99: at(0.99),
        max: values[values.len() - 1],
    }
}

/// The graph's shape over a range of steps.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PartStats {
    pub steps: u64,
    pub fields: u64,
    pub edges: u64,
    pub critical_path: u64,
    pub critical_path_pct: f64,
}

/// A region kind's summary: averages over its regions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RegionStats {
    pub regions: u64,
    pub avg_steps: f64,
    pub avg_reads_from_outside: f64,
    pub avg_visible_writes: f64,
}

/// Everything the analyzer reports.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub main: String,
    pub steps: u64,
    pub fields: u64,
    pub edges: u64,
    pub unknown_reads: u64,
    pub critical_path: u64,
    pub critical_path_pct: f64,
    pub chain_length: Quantiles,
    pub edit_lines_sampled: u64,
    pub edit_impact_pct: Quantiles,
    pub macro_calls: u64,
    pub macro_call_groups: u64,
    pub memo_repeat_fraction: f64,
    pub preamble: Option<PartStats>,
    pub body: Option<PartStats>,
    pub preamble_summary_size: Option<u64>,
    pub body_writes_read_in_preamble: Option<u64>,
    pub paragraphs: RegionStats,
    pub environments: RegionStats,
}

#[allow(clippy::cast_precision_loss)]
fn pct(a: u64, b: u64) -> f64 {
    if b == 0 {
        0.0
    } else {
        100.0 * a as f64 / b as f64
    }
}

#[allow(clippy::cast_precision_loss)]
fn avg(sum: u64, n: u64) -> f64 {
    if n == 0 { 0.0 } else { sum as f64 / n as f64 }
}

impl Graph {
    fn part(&self, range: std::ops::Range<usize>) -> PartStats {
        let mut longest: HashMap<u32, u32> = HashMap::new();
        let mut fields = std::collections::HashSet::new();
        let mut edges = 0u64;
        let mut best = 0u32;
        for i in range.clone() {
            let node = &self.nodes[i];
            for k in node.reads.0..node.reads.1 {
                let f = self.reads[k as usize];
                if f != UNKNOWN {
                    fields.insert(f);
                }
            }
            for k in node.writes.0..node.writes.1 {
                fields.insert(self.writes[k as usize].0);
            }
            let mut l = 0;
            for &(from, _, _) in self.preds(i) {
                if range.contains(&(from as usize)) {
                    edges += 1;
                    l = l.max(longest[&from]);
                }
            }
            longest.insert(idx(i), l + 1);
            best = best.max(l + 1);
        }
        let steps = range.len() as u64;
        PartStats {
            steps,
            fields: fields.len() as u64,
            edges,
            critical_path: u64::from(best),
            critical_path_pct: pct(u64::from(best), steps),
        }
    }

    /// Regions as (start, end) ranges: reads from outside, writes read
    /// after the region (distinct fields).
    fn regions(&self, ranges: &[(usize, usize)]) -> RegionStats {
        let mut steps = 0u64;
        let mut outside = 0u64;
        let mut visible = 0u64;
        let mut seen = std::collections::HashSet::new();
        for &(a, b) in ranges {
            steps += (b - a) as u64;
            seen.clear();
            for i in a..b {
                for &(from, _, f) in self.preds(i) {
                    if (from as usize) < a {
                        outside += 1;
                    }
                    let _ = f;
                }
                for (_, to, f) in self.succ_edges(i) {
                    if to as usize >= b {
                        seen.insert(f);
                    }
                }
            }
            visible += seen.len() as u64;
        }
        let n = ranges.len() as u64;
        RegionStats {
            regions: n,
            avg_steps: avg(steps, n),
            avg_reads_from_outside: avg(outside, n),
            avg_visible_writes: avg(visible, n),
        }
    }

    /// The edges out of step `i`, with their fields (found through the
    /// reader's predecessor list).
    fn succ_edges(&self, i: usize) -> impl Iterator<Item = (u32, u32, u32)> + '_ {
        self.succs(i).iter().map(move |&to| {
            let f = self
                .preds(to as usize)
                .iter()
                .find(|e| e.0 as usize == i)
                .map_or(UNKNOWN, |e| e.2);
            (idx(i), to, f)
        })
    }

    /// Analyzes the graph; `samples` source lines for the edit impact.
    #[allow(clippy::too_many_lines)]
    pub fn report(&self, samples: usize) -> Report {
        let n = self.nodes.len();
        let steps = n as u64;
        let mut report = Report {
            main: self.main.clone(),
            steps,
            fields: (self.fields.len() - 1) as u64,
            edges: self.edges.len() as u64,
            unknown_reads: self.reads.iter().filter(|&&r| r == UNKNOWN).count() as u64,
            ..Report::default()
        };
        report.critical_path = self.critical_path();
        report.critical_path_pct = pct(report.critical_path, steps);
        let mut chain: Vec<f64> = self.longest.iter().map(|&l| f64::from(l)).collect();
        report.chain_length = quantiles(&mut chain);

        // Edit impact: the steps of each line of the main file, and those
        // of them that write.
        let mut on_line: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut by_line: HashMap<u32, Vec<u32>> = HashMap::new();
        for (i, node) in self.nodes.iter().enumerate() {
            if node.in_main
                && let Some(l) = node.line
            {
                on_line.entry(l).or_default().push(idx(i));
                if node.writes.1 > node.writes.0 {
                    by_line.entry(l).or_default().push(idx(i));
                }
            }
        }
        let mut lines: Vec<u32> = by_line.keys().copied().collect();
        lines.sort_unstable();
        let mut rng = fastrand::Rng::with_seed(1);
        rng.shuffle(&mut lines);
        lines.truncate(samples);
        let mut visited = vec![0u64; n.div_ceil(64)];
        let mut impact: Vec<f64> = lines
            .iter()
            .map(|l| {
                pct(
                    self.dependents(&by_line[l], &on_line[l], &mut visited),
                    steps,
                )
            })
            .collect();
        report.edit_lines_sampled = lines.len() as u64;
        report.edit_impact_pct = quantiles(&mut impact);

        // Memo potential.
        let mut groups: HashMap<(u32, u64, u64), u32> = HashMap::new();
        for &c in &self.calls {
            *groups.entry(c).or_insert(0) += 1;
        }
        report.macro_calls = self.calls.len() as u64;
        report.macro_call_groups = groups.len() as u64;
        #[allow(clippy::cast_precision_loss)]
        {
            report.memo_repeat_fraction = if self.calls.is_empty() {
                0.0
            } else {
                (self.calls.len() - groups.len()) as f64 / self.calls.len() as f64
            };
        }

        // Preamble and body.
        if let Some(b) = self.begin_document() {
            report.preamble = Some(self.part(0..b));
            report.body = Some(self.part(b..n));
            let mut summary = std::collections::HashSet::new();
            let mut back = 0u64;
            for &(from, to, f) in &self.edges {
                if (from as usize) < b && to as usize >= b {
                    summary.insert(f);
                }
                if (to as usize) < b && from as usize >= b {
                    back += 1;
                }
            }
            report.preamble_summary_size = Some(summary.len() as u64);
            report.body_writes_read_in_preamble = Some(back);
        }

        // Regions.
        let mut pars = Vec::new();
        let mut start = 0;
        for i in 0..n {
            if self.is_par(i) {
                if i > start {
                    pars.push((start, i));
                }
                start = i + 1;
            }
        }
        if n > start {
            pars.push((start, n));
        }
        report.paragraphs = self.regions(&pars);
        let mut envs = Vec::new();
        let mut open: Vec<(u64, usize)> = Vec::new();
        let begin = self.names.iter().position(|s| s == "\\begin").map(idx);
        let end = self.names.iter().position(|s| s == "\\end").map(idx);
        for (i, node) in self.nodes.iter().enumerate() {
            match node.call {
                Some((name, args)) if Some(name) == begin => open.push((args, i)),
                Some((name, args)) if Some(name) == end => {
                    if let Some(p) = open.iter().rposition(|&(a, _)| a == args) {
                        let (_, s) = open.remove(p);
                        envs.push((s, i + 1));
                    }
                }
                _ => {}
            }
        }
        report.environments = self.regions(&envs);
        report
    }
}

/// The report as a Markdown table.
#[must_use]
pub fn markdown(r: &Report) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "| metric | value |\n|---|---|\n| document | `{}` |",
        r.main
    );
    let _ = writeln!(
        s,
        "| steps | {} |\n| distinct fields | {} |\n| edges | {} |\n| unknown reads (`?`) | {} |",
        r.steps, r.fields, r.edges, r.unknown_reads
    );
    let _ = writeln!(
        s,
        "| critical path (steps) | {} ({:.2}% of steps) |",
        r.critical_path, r.critical_path_pct
    );
    let q = &r.chain_length;
    let _ = writeln!(
        s,
        "| chain length ending at a step: p50 / p90 / p99 / max | {} / {} / {} / {} |",
        q.p50, q.p90, q.p99, q.max
    );
    let q = &r.edit_impact_pct;
    let _ = writeln!(
        s,
        "| edit impact ({} lines): p50 / p90 / p99 / max, % of steps | {:.3} / {:.3} / {:.3} / {:.3} |",
        r.edit_lines_sampled, q.p50, q.p90, q.p99, q.max
    );
    let _ = writeln!(
        s,
        "| macro calls / distinct (macro, args, read values) | {} / {} |\n| memo repeat fraction | {:.3} |",
        r.macro_calls, r.macro_call_groups, r.memo_repeat_fraction
    );
    if let (Some(p), Some(b)) = (&r.preamble, &r.body) {
        let _ = writeln!(
            s,
            "| preamble: steps / fields / edges / critical path | {} / {} / {} / {} ({:.2}%) |",
            p.steps, p.fields, p.edges, p.critical_path, p.critical_path_pct
        );
        let _ = writeln!(
            s,
            "| body: steps / fields / edges / critical path | {} / {} / {} / {} ({:.2}%) |",
            b.steps, b.fields, b.edges, b.critical_path, b.critical_path_pct
        );
        let _ = writeln!(
            s,
            "| preamble summary size (fields written before, read after `\\begin{{document}}`) | {} |",
            r.preamble_summary_size.unwrap_or(0)
        );
        let _ = writeln!(
            s,
            "| body writes read in the preamble | {} |",
            r.body_writes_read_in_preamble.unwrap_or(0)
        );
    } else {
        s.push_str("| preamble / body | no `\\begin{document}` |\n");
    }
    for (name, g) in [
        ("paragraphs", &r.paragraphs),
        ("environments", &r.environments),
    ] {
        let _ = writeln!(
            s,
            "| {name}: regions / avg steps / avg reads from outside / avg visible writes | {} / {:.1} / {:.1} / {:.1} |",
            g.regions, g.avg_steps, g.avg_reads_from_outside, g.avg_visible_writes
        );
    }
    s
}

/// Several reports as one Markdown table, a row per document.
#[must_use]
pub fn table(reports: &[Report]) -> String {
    let mut s = String::from(
        "| document | steps | fields | critical path | edit impact p50 / p99 (% of steps) | memo repeat | preamble summary |\n|---|---|---|---|---|---|---|\n",
    );
    for r in reports {
        let _ = writeln!(
            s,
            "| `{}` | {} | {} | {} ({:.2}%) | {:.3} / {:.3} | {:.3} | {} |",
            r.main.trim_start_matches("./"),
            r.steps,
            r.fields,
            r.critical_path,
            r.critical_path_pct,
            r.edit_impact_pct.p50,
            r.edit_impact_pct.p99,
            r.memo_repeat_fraction,
            r.preamble_summary_size
                .map_or_else(|| "-".to_owned(), |n| n.to_string())
        );
    }
    s
}
