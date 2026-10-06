//! `PARTEX_DEPS=report.txt` (feature `deps`): measure the dependency graph
//! between regions (DESIGN.md §7.5), at two levels. Segments: each file
//! whose name contains `PARTEX_DEPS_MATCH` opens one, and the rest of its
//! parent after it closes is another. Pages: a segment is cut at every
//! shipout. `PARTEX_DEPS_BASE` names the segment whose start state a
//! two-pass cold build would speculate from. A region depends on an earlier one when it
//! reads a cell before writing it and that earlier region was the last to
//! change the cell's value (changes undone by a group's end do not count).
//! Only `eqtb` and hash cells are covered: node lists being built, the page
//! builder and output files are not.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use partex_core::{Cell, Host, Tex, Tracker};

struct Open {
    name: String,
    start: std::time::Instant,
    /// First reads before any write in the region, with the value read.
    reads: HashMap<Cell, u64>,
    /// The value before the region's first write.
    entry: HashMap<Cell, u64>,
    /// The value after its last write.
    last: HashMap<Cell, u64>,
    /// The most recent first read, which [`Tracker::retract`] may undo.
    newest: Option<Cell>,
}

impl Default for Open {
    fn default() -> Self {
        Self {
            name: String::new(),
            start: std::time::Instant::now(),
            reads: HashMap::new(),
            entry: HashMap::new(),
            last: HashMap::new(),
            newest: None,
        }
    }
}

struct Done {
    name: String,
    /// The segment the region belongs to.
    segment: usize,
    secs: f64,
    reads: Vec<Cell>,
    /// The values those reads saw.
    read_values: Vec<u64>,
    writes: Vec<Cell>,
}

#[derive(Default)]
struct State {
    done: Vec<Done>,
    cur: Open,
    /// Input levels of the open matching files, with their names.
    stack: Vec<(usize, String)>,
    segment: usize,
    /// The last value seen of every cell, until the base is taken.
    now: HashMap<Cell, u64>,
    /// The values when the first segment matching `PARTEX_DEPS_BASE`
    /// started (filled in lazily for cells first seen later: a cell cannot
    /// change unseen), and the index of that segment.
    base: Option<(usize, HashMap<Cell, u64>)>,
}

impl State {
    fn seen(&mut self, cell: Cell, before: u64, after: u64) {
        match &mut self.base {
            None => {
                self.now.insert(cell, after);
            }
            Some((_, b)) => {
                b.entry(cell).or_insert(before);
            }
        }
    }
}

impl State {
    /// End the current region; the next one starts a new segment if
    /// `segment`.
    fn switch(&mut self, name: String, segment: bool) {
        let o = std::mem::replace(
            &mut self.cur,
            Open {
                name,
                ..Open::default()
            },
        );
        let mut writes: Vec<Cell> = o
            .last
            .iter()
            .filter(|(c, v)| o.entry.get(c) != Some(v))
            .map(|(&c, _)| c)
            .collect();
        writes.sort_unstable();
        let mut rv: Vec<(Cell, u64)> = o.reads.into_iter().collect();
        rv.sort_unstable();
        let (reads, read_values) = rv.into_iter().unzip();
        self.done.push(Done {
            segment: self.segment,
            secs: o.start.elapsed().as_secs_f64(),
            name: o.name,
            reads,
            read_values,
            writes,
        });
        self.segment += usize::from(segment);
    }
}

/// The recording tracker.
pub struct Recorder {
    filter: String,
    base: String,
    /// `PARTEX_DEPS_PARS`: segments also end at blank lines.
    pars: bool,
    s: RefCell<State>,
}

impl Recorder {
    pub fn new(filter: String, base: String) -> Self {
        let s = State {
            cur: Open {
                name: "<start>".into(),
                ..Open::default()
            },
            ..State::default()
        };
        Self {
            filter,
            base,
            pars: std::env::var_os("PARTEX_DEPS_PARS").is_some(),
            s: RefCell::new(s),
        }
    }
}

impl Tracker for Recorder {
    const VALUES: bool = true;
    fn read(&self, _: Cell) {}
    fn write(&self, _: Cell) {}
    fn read_value(&self, cell: Cell, bits: u64) {
        let s = &mut *self.s.borrow_mut();
        if !s.cur.last.contains_key(&cell) && !s.cur.reads.contains_key(&cell) {
            s.cur.reads.insert(cell, bits);
            s.cur.newest = Some(cell);
        }
        s.seen(cell, bits, bits);
    }
    fn write_value(&self, cell: Cell, old: u64, new: u64) {
        let s = &mut *self.s.borrow_mut();
        s.seen(cell, old, new);
        let entry = *s.cur.entry.entry(cell).or_insert(old);
        if new == entry {
            // Back to the value it had on entry (a group ended): later
            // reads see the earlier region's value again.
            s.cur.last.remove(&cell);
        } else {
            s.cur.last.insert(cell, new);
        }
    }
    fn file(&self, depth: usize, name: Option<&[u8]>) {
        let s = &mut *self.s.borrow_mut();
        match name {
            Some(n) => {
                let n = String::from_utf8_lossy(n).into_owned();
                if n.contains(&self.filter) {
                    s.stack.push((depth, n.clone()));
                    s.switch(n.clone(), true);
                    if s.base.is_none() && !self.base.is_empty() && n.contains(&self.base) {
                        let now = std::mem::take(&mut s.now);
                        s.base = Some((s.segment, now));
                    }
                }
            }
            None => {
                if s.stack.last().is_some_and(|&(d, _)| d == depth) {
                    let (_, n) = s.stack.pop().unwrap_or_default();
                    let parent = s.stack.last().map_or("<main>", |(_, p)| p.as_str());
                    let after = format!("{parent} after {}", short(&n));
                    s.switch(after, true);
                }
            }
        }
    }
    fn retract(&self, cell: Cell) {
        let s = &mut *self.s.borrow_mut();
        if s.cur.newest == Some(cell) {
            s.cur.reads.remove(&cell);
            s.cur.newest = None;
        }
    }
    fn blank_line(&self) {
        let s = &mut *self.s.borrow_mut();
        if self.pars && s.base.is_some() {
            let name = s.cur.name.clone();
            s.switch(name, true);
        }
    }
    fn page(&self) {
        let s = &mut *self.s.borrow_mut();
        let name = s.cur.name.clone();
        s.switch(name, false);
    }
}

fn short(n: &str) -> &str {
    let n = n.rsplit('/').next().unwrap_or(n);
    n.strip_suffix(".tex").unwrap_or(n)
}

/// A graph of regions in execution order: `edges[b][a]` lists the cells
/// region `b` read that region `a` had last changed.
struct Graph<'a> {
    names: Vec<&'a str>,
    secs: Vec<f64>,
    edges: Vec<BTreeMap<usize, Vec<Cell>>>,
}

type Keep<'a> = &'a dyn Fn(&Cell) -> bool;

impl Graph<'_> {
    /// With unlimited workers, a region starts when region 0 (the format
    /// and what runs before the first file: an input, not a dependency)
    /// and every region it read from have finished. Returns the critical
    /// path length and its regions.
    fn span(&self, keep: Keep) -> (f64, Vec<usize>) {
        let n = self.secs.len();
        let mut finish = vec![0f64; n];
        let mut via: Vec<Option<usize>> = vec![None; n];
        finish[0] = self.secs[0];
        for b in 1..n {
            let mut ready = finish[0];
            for (&a, cs) in &self.edges[b] {
                if a > 0 && a < b && cs.iter().any(keep) && finish[a] > ready {
                    ready = finish[a];
                    via[b] = Some(a);
                }
            }
            finish[b] = ready + self.secs[b];
        }
        let end = (0..n)
            .max_by(|&x, &y| finish[x].total_cmp(&finish[y]))
            .unwrap_or(0);
        let mut path = vec![end];
        while let Some(a) = via[*path.last().unwrap_or(&0)] {
            path.push(a);
        }
        path.reverse();
        (finish[end], path)
    }

    fn report(&self, out: &mut String, label: &str, cell_name: &dyn Fn(&Cell) -> String) {
        let total: f64 = self.secs.iter().sum();
        let counter = |c: &Cell| {
            let n = cell_name(c);
            n.starts_with("\\c@") || n.starts_with("\\g_shipout_")
        };
        let _ = writeln!(out, "## {label}: {} regions, {total:.2} s", self.secs.len());
        let all: Keep = &|_| true;
        let eqtb: Keep = &|c| matches!(c, Cell::Eqtb(_));
        let spec: Keep = &|c| matches!(c, Cell::Eqtb(_)) && !counter(c);
        for (what, keep) in [
            ("all cells", all),
            ("eqtb only", eqtb),
            ("eqtb, counters speculated", spec),
        ] {
            let (span, path) = self.span(keep);
            let _ = writeln!(
                out,
                "# bound ({what}): critical path {span:.2} s over {} regions, max speedup {:.2}x",
                path.len(),
                total / span
            );
            // The heaviest hops on the path, with the cells that carry them.
            let mut hops: Vec<(usize, usize)> = path.windows(2).map(|w| (w[0], w[1])).collect();
            hops.sort_by(|x, y| self.secs[y.1].total_cmp(&self.secs[x.1]));
            for &(a, b) in hops.iter().take(6) {
                let cs: Vec<String> = self.edges[b][&a]
                    .iter()
                    .filter(|c| keep(c))
                    .take(8)
                    .map(cell_name)
                    .collect();
                let _ = writeln!(
                    out,
                    "#   {a} -> {b} ({:.2} s, {}): {}",
                    self.secs[b],
                    self.names[b],
                    cs.join(" ")
                );
            }
        }
    }
}

/// Write the report on `tex`'s recorded graph to `path`.
pub fn report<H: Host>(tex: &Tex<H, Recorder>, path: &std::path::Path) {
    let mut st = tex.tracker().s.take();
    st.now = HashMap::new();
    st.switch("<end>".into(), true);
    let regions = st.done;
    // Last region to change each cell, as the regions run in order.
    let mut last_writer: HashMap<Cell, usize> = HashMap::new();
    let mut edges: Vec<BTreeMap<usize, Vec<Cell>>> = Vec::new();
    for (b, r) in regions.iter().enumerate() {
        let mut e: BTreeMap<usize, Vec<Cell>> = BTreeMap::new();
        for c in &r.reads {
            if let Some(&a) = last_writer.get(c) {
                e.entry(a).or_default().push(*c);
            }
        }
        edges.push(e);
        for &c in &r.writes {
            last_writer.insert(c, b);
        }
    }
    let mut cells: Vec<Cell> = edges
        .iter()
        .flat_map(|e| e.values().flatten().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    cells.sort_unstable();
    let names: HashMap<Cell, String> = cells
        .iter()
        .copied()
        .zip(
            tex.cell_names(&cells)
                .into_iter()
                .map(|n| String::from_utf8_lossy(&n).into_owned()),
        )
        .collect();
    let cell_name = |c: &Cell| names.get(c).cloned().unwrap_or_default();

    let pages = Graph {
        names: regions.iter().map(|r| r.name.as_str()).collect(),
        secs: regions.iter().map(|r| r.secs).collect(),
        edges: edges.clone(),
    };
    // Segments: pages folded into the segment they ran in.
    let nseg = regions.last().map_or(0, |r| r.segment + 1);
    let mut seg = Graph {
        names: vec![""; nseg],
        secs: vec![0.0; nseg],
        edges: vec![BTreeMap::new(); nseg],
    };
    for (b, r) in regions.iter().enumerate() {
        seg.names[r.segment] = r.name.as_str();
        seg.secs[r.segment] += r.secs;
        for (&a, cs) in &edges[b] {
            let sa = regions[a].segment;
            if sa != r.segment {
                seg.edges[r.segment].entry(sa).or_default().extend(cs);
            }
        }
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# files matching {:?}; times include tracking overhead",
        tex.tracker().filter
    );
    if let Some((base_seg, base)) = &st.base {
        let blocks = Blocks::new(&regions, *base_seg, base);
        blocks.two_pass(&mut out, &cell_name);
        blocks.iterated(&mut out, &cell_name);
        blocks.main_line(&mut out, &cell_name);
    }
    seg.report(&mut out, "segments", &cell_name);
    pages.report(&mut out, "pages", &cell_name);
    let _ = writeln!(
        out,
        "# idx segment secs reads deps-cells dep-regions net-writes name"
    );
    for (i, r) in regions.iter().enumerate() {
        let deps: usize = edges[i].values().map(Vec::len).sum();
        let _ = writeln!(
            out,
            "{i:5} {:4} {:7.3} {:7} {:6} {:4} {:7} {}",
            r.segment,
            r.secs,
            r.reads.len(),
            deps,
            edges[i].len(),
            r.writes.len(),
            r.name
        );
    }
    fan_report(&mut out, &edges, &cell_name);
    cut_report(&mut out, &regions, &edges, &cell_name);
    if let Err(e) = std::fs::write(path, out) {
        eprintln!("phitex: cannot write {}: {e}", path.display());
    }
}

/// Value differences a speculation estimate disregards, cumulatively.
fn is_counter(n: &str) -> bool {
    n.starts_with("\\c@") || n.starts_with("\\g_shipout_") || n.starts_with("\\@currentH")
}

fn is_scratch(n: &str) -> bool {
    n.starts_with("\\reserved@")
        || n.starts_with("\\@temp")
        || n.starts_with("\\@gtemp")
        || n.starts_with("\\@let@token")
        || n.contains("tmp")
        || n.contains("temp")
}

/// Whether a difference in a cell (with its name) is disregarded.
type Ignore = fn(&Cell, &str) -> bool;

const CLASSES: [(&str, Ignore); 4] = [
    ("every difference", |_, _| false),
    ("hash slots ignored", |c, _| {
        matches!(c, Cell::Hash(_) | Cell::HashNext(_))
    }),
    ("+ counters resolved", |c, n| {
        matches!(c, Cell::Hash(_) | Cell::HashNext(_)) || is_counter(n)
    }),
    ("+ scratch macros ignored", |c, n| {
        matches!(c, Cell::Hash(_) | Cell::HashNext(_)) || is_counter(n) || is_scratch(n)
    }),
];

/// Segments from the speculation base on, with what their reads saw.
struct Blocks<'a> {
    base_seg: usize,
    secs: Vec<f64>,
    names: Vec<&'a str>,
    /// Cells read before the segment wrote them whose value differs from
    /// the base, each with the last segment (from the base on) to change
    /// it: the writer the reader waits for.
    waits: Vec<Vec<(Cell, Option<usize>)>>,
    /// Time before the base.
    pre: f64,
    total: f64,
}

impl<'a> Blocks<'a> {
    fn new(regions: &'a [Done], base_seg: usize, base: &HashMap<Cell, u64>) -> Self {
        let nseg = regions.last().map_or(0, |r| r.segment + 1);
        let mut b = Blocks {
            base_seg,
            secs: vec![0.0; nseg],
            names: vec![""; nseg],
            waits: vec![Vec::new(); nseg],
            pre: 0.0,
            total: 0.0,
        };
        let mut writer: HashMap<Cell, usize> = HashMap::new();
        let mut written: HashSet<Cell> = HashSet::new();
        let mut cur = usize::MAX;
        for r in regions {
            if r.segment != cur {
                cur = r.segment;
                written.clear();
            }
            b.secs[cur] += r.secs;
            b.names[cur] = &r.name;
            if cur >= base_seg {
                for (c, v) in r.reads.iter().zip(&r.read_values) {
                    if !written.contains(c) && base.get(c).is_some_and(|x| x != v) {
                        b.waits[cur].push((*c, writer.get(c).copied()));
                    }
                }
                for &c in &r.writes {
                    writer.insert(c, cur);
                }
            }
            written.extend(r.writes.iter().copied());
        }
        b.pre = b.secs[..base_seg].iter().sum();
        b.total = b.secs.iter().sum();
        b
    }

    fn range(&self) -> std::ops::Range<usize> {
        self.base_seg..self.secs.len()
    }

    /// Two passes: every segment starts at once from the base state, then
    /// in order a segment is kept if every value it read matches the real
    /// run, and rerun otherwise.
    fn two_pass(&self, out: &mut String, cell_name: &dyn Fn(&Cell) -> String) {
        let longest = self.range().map(|k| self.secs[k]).fold(0.0, f64::max);
        let _ = writeln!(
            out,
            "## two-pass speculation from segment {}: before it {:.2} s, after it {:.2} s \
             in {} segments, longest {longest:.2} s",
            self.base_seg,
            self.pre,
            self.total - self.pre,
            self.range().len()
        );
        for (label, ignore) in CLASSES {
            let mut rerun = 0f64;
            let mut n = 0;
            let mut why: HashMap<String, usize> = HashMap::new();
            for k in self.range() {
                let bad: Vec<String> = self.waits[k]
                    .iter()
                    .map(|(c, _)| (c, cell_name(c)))
                    .filter(|(c, n)| !ignore(c, n))
                    .map(|(_, n)| n)
                    .collect();
                if !bad.is_empty() {
                    rerun += self.secs[k];
                    n += 1;
                    for b in bad {
                        *why.entry(b).or_default() += 1;
                    }
                }
            }
            let t = self.pre + longest + rerun;
            let _ = writeln!(
                out,
                "# {label}: {n} segments rerun ({rerun:.2} s); estimate {t:.2} s, speedup {:.2}x",
                self.total / t
            );
            let mut why: Vec<(String, usize)> = why.into_iter().collect();
            why.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let top: Vec<String> = why
                .iter()
                .take(25)
                .map(|(c, k)| format!("{c}({k})"))
                .collect();
            let _ = writeln!(out, "#   {}", top.join(" "));
        }
    }

    /// A main line runs every segment in order, as a sequential build
    /// does; workers run segments ahead of it, and the main line adopts a
    /// worker's result when that segment's reads hold, else runs the
    /// segment itself. Never slower than the sequential build. `refresh`:
    /// whenever the main line finishes a segment, workers restart the
    /// later segments from that exact state; without it they run once,
    /// from the base state. Unlimited workers.
    fn main_line(&self, out: &mut String, cell_name: &dyn Fn(&Cell) -> String) {
        let _ = writeln!(out, "## main line with speculative workers");
        let n = self.secs.len();
        for (label, ignore) in CLASSES {
            // The last segment whose writes a segment's reads need: a
            // worker's run is right if it started from the state after it.
            let need: Vec<Option<usize>> = (0..n)
                .map(|k| {
                    self.waits[k]
                        .iter()
                        .filter(|(c, _)| !ignore(c, &cell_name(c)))
                        .map(|&(_, w)| w.unwrap_or(k))
                        .max()
                })
                .collect();
            for refresh in [false, true] {
                let mut t = self.pre;
                let mut end = vec![self.pre; n];
                let mut adopted = 0;
                for k in self.range() {
                    let start = match need[k] {
                        None => Some(self.pre),
                        Some(w) if refresh && w < k => Some(end[w]),
                        Some(_) => None,
                    };
                    let own = t + self.secs[k];
                    t = match start {
                        Some(s) if s + self.secs[k] < own => {
                            adopted += 1;
                            t.max(s + self.secs[k])
                        }
                        _ => own,
                    };
                    end[k] = t;
                }
                let _ = writeln!(
                    out,
                    "# {label}, {}: {adopted} of {} segments adopted, estimate {t:.2} s, \
                     speedup {:.2}x",
                    if refresh {
                        "workers restart from each finished segment"
                    } else {
                        "workers from the base only"
                    },
                    self.range().len(),
                    self.total / t
                );
            }
        }
    }

    /// Iterated passes: every segment first runs from the base state;
    /// after each pass the writes are composed in order and a segment
    /// reruns, from the composed state, while a value it read is still
    /// wrong. It becomes right in the pass after the last segment it waits
    /// for did. Time with unlimited workers: the slowest segment still
    /// running in each pass.
    fn iterated(&self, out: &mut String, cell_name: &dyn Fn(&Cell) -> String) {
        let _ = writeln!(out, "## iterated passes (the graph made real as it runs)");
        let n = self.secs.len();
        for (label, ignore) in CLASSES {
            let mut pass = vec![1usize; n];
            let mut why: Vec<Option<(Cell, usize)>> = vec![None; n];
            for k in self.range() {
                for &(c, w) in &self.waits[k] {
                    if let Some(w) = w
                        && w < k
                        && pass[w] + 1 > pass[k]
                        && !ignore(&c, &cell_name(&c))
                    {
                        pass[k] = pass[w] + 1;
                        why[k] = Some((c, w));
                    }
                }
            }
            let passes = self.range().map(|k| pass[k]).max().unwrap_or(0);
            let mut t = self.pre;
            for p in 1..=passes {
                t += self
                    .range()
                    .filter(|&k| pass[k] >= p)
                    .map(|k| self.secs[k])
                    .fold(0.0, f64::max);
            }
            let _ = writeln!(
                out,
                "# {label}: {passes} passes, estimate {t:.2} s, speedup {:.2}x",
                self.total / t
            );
            // The chain that sets the number of passes.
            let mut k = self.range().max_by_key(|&k| (pass[k], k)).unwrap_or(0);
            let mut chain = Vec::new();
            while let Some((c, w)) = why[k] {
                if chain.len() < 12 {
                    chain.push(format!("{k}<-{w} {}", cell_name(&c)));
                }
                k = w;
            }
            let _ = writeln!(out, "#   chain: {}", chain.join(" | "));
        }
        // The heaviest segments and how many of their reads differ.
        let mut order: Vec<usize> = self.range().collect();
        order.sort_by(|&a, &b| self.secs[b].total_cmp(&self.secs[a]));
        for &k in order.iter().take(10) {
            let _ = writeln!(
                out,
                "#   seg {k} {:.2} s {}: {} values differ",
                self.secs[k],
                self.names[k],
                self.waits[k].len()
            );
        }
    }
}

/// The cells that carry the most edges (not from region 0).
fn fan_report(
    out: &mut String,
    edges: &[BTreeMap<usize, Vec<Cell>>],
    cell_name: &dyn Fn(&Cell) -> String,
) {
    let mut fan: HashMap<Cell, usize> = HashMap::new();
    for e in edges {
        for c in e.iter().filter(|(a, _)| **a > 0).flat_map(|(_, cs)| cs) {
            *fan.entry(*c).or_default() += 1;
        }
    }
    let mut fan: Vec<(Cell, usize)> = fan.into_iter().collect();
    fan.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let _ = writeln!(
        out,
        "# cells read across regions (not from region 0): {}",
        fan.len()
    );
    for (c, n) in fan.iter().take(300) {
        let _ = writeln!(out, "{n:6} {}", cell_name(c));
    }
}

/// Cut sizes: for each boundary between regions, the distinct cells live
/// across it (last changed before it, read after it from that change,
/// region 0 excluded). A small cut is a good place for a region to start;
/// regions can be chosen from these instead of declared.
fn cut_report(
    out: &mut String,
    regions: &[Done],
    edges: &[BTreeMap<usize, Vec<Cell>>],
    cell_name: &dyn Fn(&Cell) -> String,
) {
    let n = regions.len();
    // Per cell, the boundaries it is live across: (writer, reader] as
    // boundary indices writer+1..=reader.
    let mut spans: HashMap<Cell, Vec<(usize, usize)>> = HashMap::new();
    for (b, e) in edges.iter().enumerate() {
        for (&a, cs) in e.iter().filter(|(a, _)| **a > 0) {
            for &c in cs {
                spans.entry(c).or_default().push((a + 1, b));
            }
        }
    }
    let mut all = vec![0i64; n + 1];
    let mut firm = vec![0i64; n + 1];
    for (c, mut v) in spans {
        let name = cell_name(&c);
        let loose = matches!(c, Cell::Hash(_) | Cell::HashNext(_))
            || is_counter(&name)
            || is_scratch(&name);
        v.sort_unstable();
        // The union of the spans, added to a difference array.
        let mut cur: Option<(usize, usize)> = None;
        let mut add = |lo: usize, hi: usize| {
            all[lo] += 1;
            all[hi + 1] -= 1;
            if !loose {
                firm[lo] += 1;
                firm[hi + 1] -= 1;
            }
        };
        for (lo, hi) in v {
            match cur {
                Some((l, h)) if lo <= h + 1 => cur = Some((l, h.max(hi))),
                Some((l, h)) => {
                    add(l, h);
                    cur = Some((lo, hi));
                }
                None => cur = Some((lo, hi)),
            }
        }
        if let Some((l, h)) = cur {
            add(l, h);
        }
    }
    let mut run_all = 0i64;
    let mut run_firm = 0i64;
    let mut cuts: Vec<(i64, i64, usize)> = Vec::new();
    for i in 1..n {
        run_all += all[i];
        run_firm += firm[i];
        cuts.push((run_firm, run_all, i));
    }
    let mut sorted: Vec<i64> = cuts.iter().map(|c| c.0).collect();
    sorted.sort_unstable();
    let pct = |p: usize| sorted.get(sorted.len() * p / 100).copied().unwrap_or(0);
    let _ = writeln!(
        out,
        "# cut sizes over {} boundaries (cells live across, excluding hash slots, \
         counters and scratch): min {} p10 {} median {} p90 {} max {}",
        cuts.len(),
        pct(0),
        pct(10),
        pct(50),
        pct(90),
        sorted.last().copied().unwrap_or(0)
    );
    let _ = writeln!(out, "# boundary firm-cut all-cut secs-before-it region");
    let mut before = 0f64;
    for (firm, all, i) in &cuts {
        before += regions[i - 1].secs;
        let _ = writeln!(
            out,
            "{i:5} {firm:6} {all:6} {before:8.2} {}",
            regions[*i].name
        );
    }
}
