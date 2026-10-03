//! `cargo xtask parallel`: how much of an SSA build could run at once,
//! measured on the step graph `PARTEX_SSA_DAG=FILE` writes
//! (`partex_core::ssa::dag`). The models and schedules of
//! `scripts/ssa-parallel.py`, fast enough for the PGF manual (about 600 K
//! steps and 190 M reads), and speculation from predicted entry states:
//!
//!     cargo xtask parallel cold DAG [--log LOG]... [--segments REGEX]
//!     cargo xtask parallel rebuild DAG.K DAG.N [--log LOG]... [--show N]
//!
//! A step (a window of the fold) costs the commands its last run ran. It
//! depends on step A if it read an address whose definition reaching it
//! is A's (the dump's `R A at name` lines; a file it loaded, `L`, makes no
//! edge). Step boundaries are taken as known. Two schedules, a worker per
//! step: *whole steps* (a step begins once every step it read from has
//! ended) and *pipelined* (a step may begin before the steps it reads
//! from end, each read waiting for its definition's last write, by the
//! dump's times in commands into each step).
//!
//! Four models of the reads, as in the Python: *all reads*; *soft
//! definitions* (a definition equal to the one before it is passed
//! through to that one); *soft reads* (also a step's read of an address it
//! so passes through is dropped); *blind writes* (a step's read of any
//! address it defines is dropped: DESIGN 3.2's scoped definitions' bound).
//!
//! Speculation, added here: a step run before the steps it reads from
//! have run, from an entry state that is *predicted*, kept if every read
//! finds the version it would have found, else run again. Predictors:
//! - *the setup's end*: each step, or each *segment* (the steps between
//!   two loads of a source file matching `--segments`, by default any
//!   `.tex`: a chapter of an `\include`d document), starts from the state
//!   the setup (the steps before the first that ships a page) left; a
//!   read of a definition made after the setup and before the segment is
//!   mispredicted unless its version is the setup's. Then whole classes
//!   of addresses taken as predicted too (what a pre-pass or a
//!   renumbering would have to supply), the class that validates the most
//!   first;
//! - *the last build's records* (`rebuild`): every step from the entry
//!   state it had in the build before.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::needless_range_loop
)]

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::fmt::Write as _;
use std::fs::File;
use std::hash::{BuildHasherDefault, Hasher};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use regex::Regex;

const USAGE: &str = "\
usage: cargo xtask parallel cold DAG [--log LOG]... [--segments REGEX]
       cargo xtask parallel rebuild DAG.K DAG.N [--log LOG]... [--show N]";

/// No step, no write, no time.
const NONE: u32 = u32::MAX;
/// No version (`-`: the job ended inside the step).
const NOVER: u64 = u64::MAX;

const CLASSES: [&str; 21] = [
    "page builder",
    "current list (nest)",
    "save stack",
    "allocators",
    "log and terminal",
    "PDF writer and fonts",
    "\\write streams",
    "\\read streams",
    "call results",
    "conditionals",
    "marks",
    "counters (\\count)",
    "registers (\\dimen, \\skip, \\toks, \\box)",
    "hash table",
    "codes",
    "meanings (macros)",
    "hyphenation",
    "source",
    "unknown",
    "engine scalars",
    "parameters",
];
const EVERY: u32 = (1 << CLASSES.len()) - 1;

fn bit(class: &str) -> u32 {
    1 << CLASSES.iter().position(|c| *c == class).expect("a class")
}

/// `FxHash`, for the names.
#[derive(Default)]
struct Fx(u64);

impl Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for c in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..c.len()].copy_from_slice(c);
            self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(b))
                .wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
}

type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

/// An address's class (an index in `CLASSES`), by its name in the view.
struct Klass {
    marks: Regex,
    reg: Regex,
    codes: Regex,
}

impl Klass {
    fn new() -> Klass {
        Klass {
            marks: Regex::new(r"^(top|first|bot|splitfirst|splitbot)marks?\d*$").expect("re"),
            reg: Regex::new(r"^\\(count|dimen|skip|muskip|toks|box)\d+$").expect("re"),
            codes: Regex::new(r"^\\(catcode|lccode|uccode|sfcode|mathcode|delcode)\d+$")
                .expect("re"),
        }
    }

    fn of(&self, n: &str) -> u8 {
        let starts = |ps: &[&str]| ps.iter().any(|p| n.starts_with(p));
        let is = |ns: &[&str]| ns.contains(&n);
        if starts(&["page.", "page["]) || is(&["page_step_result", "splitdiscards"]) {
            0
        } else if starts(&["list.", "align."]) || n == "nest" {
            1
        } else if starts(&["save.", "save["]) {
            2
        } else if is(&["str_ptr", "hash_used", "hash_high", "glue_lineage"])
            || starts(&["string:", "strings:"])
        {
            3
        } else if is(&[
            "file_offset",
            "term_offset",
            "selector",
            "error_count",
            "history",
            "write:log",
            "log_opened",
            "interaction",
            "long_help_seen",
            "open_parens",
            "log_name",
            "job_name",
            "output_file_name",
            "shown_mode",
        ]) {
            4
        } else if starts(&["pdf.", "glyphs:", "dvi.", "font:", "fontid:"])
            || is(&[
                "fonts",
                "pdfinfo",
                "pdfcatalog",
                "pdfnames",
                "pdftrailer",
                "pdftrailerid",
            ])
        {
            5
        } else if starts(&["write:", "write_open["]) {
            6
        } else if starts(&["read:", "read_open["]) {
            7
        } else if is(&[
            "hpack_result",
            "vpack_result",
            "line_break_result",
            "output_result",
            "last_badness",
        ]) {
            8
        } else if n == "cond" {
            9
        } else if self.marks.is_match(n) {
            10
        } else if let Some(c) = self.reg.captures(n) {
            if &c[1] == "count" { 11 } else { 12 }
        } else if starts(&["text:", "next:", "lookup:", "hash:", "search:"]) {
            13
        } else if self.codes.is_match(n) {
            14
        } else if starts(&["\\", "active:", "frozen:", "primitive:", "undefined"]) {
            15
        } else if n.starts_with("hyph") {
            16
        } else if starts(&["source:", "line:"]) {
            17
        } else if n.starts_with("unknown:") {
            18
        } else if is(&[
            "output_active",
            "dead_cycles",
            "after_token",
            "align_state",
            "mag_set",
            "random",
            "sys_time",
            "sys_day",
            "sys_month",
            "sys_year",
        ]) {
            19
        } else {
            20
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Model {
    All,
    SoftDefs,
    SoftReads,
    Blind,
}

const MODELS: [(Model, &str); 4] = [
    (Model::All, "all reads"),
    (Model::SoftDefs, "soft definitions"),
    (Model::SoftReads, "soft reads"),
    (Model::Blind, "blind writes"),
];

/// A dump, by position in program order. Reads and writes are flat
/// arrays, each step's a range (`rs`, `ws`); a step's writes sorted by
/// name.
#[derive(Default)]
struct Dag {
    names: Vec<String>,
    ix: FxMap<Vec<u8>, u32>,
    cls: Vec<u8>,
    ids: Vec<u32>,
    run: Vec<u32>,
    cost: Vec<u64>,
    /// Step id -> position.
    pos_of: Vec<u32>,
    /// Files loaded: (position, name).
    loads: Vec<(u32, u32)>,
    rs: Vec<usize>,
    /// The position of the step whose definition the read found.
    r_from: Vec<u32>,
    r_name: Vec<u32>,
    /// Commands into the step at the read.
    r_at: Vec<u32>,
    /// The write that is the read's definition.
    r_def: Vec<u32>,
    /// The reader's own write of the name.
    r_own: Vec<u32>,
    ws: Vec<usize>,
    w_name: Vec<u32>,
    w_ver: Vec<u64>,
    w_at: Vec<u32>,
    w_pos: Vec<u32>,
    /// Soft definitions: the write a definition equal to the one before
    /// it passes through to, and whether it does.
    w_eff: Vec<u32>,
    w_through: Vec<bool>,
    /// The writes of each name, in program order.
    dn: Vec<usize>,
    d_w: Vec<u32>,
    timed: bool,
}

fn num(s: &[u8]) -> u32 {
    if s == b"-" {
        return NONE;
    }
    let mut v: u64 = 0;
    for &c in s {
        v = v * 10 + u64::from(c.wrapping_sub(b'0'));
    }
    v.min(u64::from(NONE - 1)) as u32
}

fn num64(s: &[u8]) -> u64 {
    let mut v: u64 = 0;
    for &c in s {
        v = v * 10 + u64::from(c.wrapping_sub(b'0'));
    }
    v
}

fn hex(s: &[u8]) -> u64 {
    if s == b"-" {
        return NOVER;
    }
    let mut v: u64 = 0;
    for &c in s {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => c.wrapping_sub(b'A').wrapping_add(10),
        };
        v = v << 4 | u64::from(d & 15);
    }
    v
}

impl Dag {
    fn intern(&mut self, n: &[u8]) -> u32 {
        if let Some(&i) = self.ix.get(n) {
            return i;
        }
        let i = self.names.len() as u32;
        self.names.push(String::from_utf8_lossy(n).into_owned());
        self.ix.insert(n.to_vec(), i);
        i
    }

    fn open(path: &Path) -> Result<Dag> {
        let t0 = Instant::now();
        let f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let mut r = BufReader::with_capacity(1 << 22, f);
        let mut line = Vec::with_capacity(512);
        let mut d = Dag::default();
        loop {
            line.clear();
            if r.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.is_empty() {
                continue;
            }
            let mut f = line.splitn(4, |&c| c == b'\t');
            let kind = f.next().unwrap_or_default();
            let a = f.next().unwrap_or_default();
            let b = f.next().unwrap_or_default();
            let c = f.next().unwrap_or_default();
            match kind {
                b"S" => {
                    d.ids.push(num(a));
                    d.run.push(num(b));
                    d.cost.push(num64(c));
                    d.rs.push(d.r_name.len());
                    d.ws.push(d.w_name.len());
                }
                b"R" => {
                    let n = d.intern(c);
                    d.r_from.push(num(a));
                    d.r_at.push(num(b));
                    d.r_name.push(n);
                }
                b"L" => {
                    let n = d.intern(c);
                    let p = d.ids.len().saturating_sub(1) as u32;
                    d.loads.push((p, n));
                }
                b"W" => {
                    let n = d.intern(c);
                    d.w_name.push(n);
                    d.w_ver.push(hex(a));
                    d.w_at.push(num(b));
                }
                _ => bail!(
                    "{}: a line of kind {:?}",
                    path.display(),
                    String::from_utf8_lossy(kind)
                ),
            }
        }
        d.rs.push(d.r_name.len());
        d.ws.push(d.w_name.len());
        let n = d.ids.len();
        let max_id = d.ids.iter().copied().max().unwrap_or(0) as usize;
        d.pos_of = vec![NONE; max_id + 1];
        for (p, &id) in d.ids.iter().enumerate() {
            d.pos_of[id as usize] = p as u32;
        }
        let mut lost = 0u64;
        for a in &mut d.r_from {
            if *a != NONE {
                let p = d.pos_of.get(*a as usize).copied().unwrap_or(NONE);
                if p == NONE {
                    lost += 1;
                }
                *a = p;
            }
        }
        d.timed = d.r_at.iter().any(|&t| t != NONE);
        // (each step's writes by name, a name once: its last version, and
        // its last time, as the Python's dicts keep them)
        let mut tmp: Vec<(u32, u64, u32)> = Vec::new();
        let (mut names_w, mut vers, mut ats) = (Vec::new(), Vec::new(), Vec::new());
        let mut ws = Vec::with_capacity(n + 1);
        for p in 0..n {
            let (s, e) = (d.ws[p], d.ws[p + 1]);
            tmp.clear();
            tmp.extend((s..e).map(|i| (d.w_name[i], d.w_ver[i], d.w_at[i])));
            tmp.sort_by_key(|t| t.0);
            ws.push(names_w.len());
            for t in &tmp {
                if names_w.len() > ws[p] && *names_w.last().expect("a write") == t.0 {
                    *vers.last_mut().expect("a write") = t.1;
                    if t.2 != NONE {
                        *ats.last_mut().expect("a write") = t.2;
                    }
                    continue;
                }
                names_w.push(t.0);
                vers.push(t.1);
                ats.push(t.2);
            }
        }
        ws.push(names_w.len());
        (d.w_name, d.w_ver, d.w_at, d.ws) = (names_w, vers, ats, ws);
        d.w_pos = vec![0; d.w_name.len()];
        for p in 0..n {
            for i in d.ws[p]..d.ws[p + 1] {
                d.w_pos[i] = p as u32;
            }
        }
        // (each read's definition, and the reader's own write of its name)
        d.r_def = vec![NONE; d.r_name.len()];
        d.r_own = vec![NONE; d.r_name.len()];
        for p in 0..n {
            for k in d.rs[p]..d.rs[p + 1] {
                let nm = d.r_name[k];
                let a = d.r_from[k];
                if a != NONE {
                    d.r_def[k] = d.find_write(a as usize, nm);
                }
                d.r_own[k] = d.find_write(p, nm);
            }
        }
        // (the writes of each name in program order)
        let names = d.names.len();
        let mut count = vec![0usize; names + 1];
        for &nm in &d.w_name {
            count[nm as usize + 1] += 1;
        }
        for i in 0..names {
            count[i + 1] += count[i];
        }
        d.dn.clone_from(&count);
        d.d_w = vec![0; d.w_name.len()];
        let mut fill = count;
        for (i, &nm) in d.w_name.iter().enumerate() {
            d.d_w[fill[nm as usize]] = i as u32;
            fill[nm as usize] += 1;
        }
        // (soft definitions)
        d.w_eff = (0..d.w_name.len() as u32).collect();
        d.w_through = vec![false; d.w_name.len()];
        for nm in 0..names {
            let (mut last_v, mut last_eff) = (NOVER, NONE);
            for j in d.dn[nm]..d.dn[nm + 1] {
                let w = d.d_w[j] as usize;
                let v = d.w_ver[w];
                if v != NOVER && v == last_v && last_eff != NONE {
                    d.w_eff[w] = last_eff;
                    d.w_through[w] = true;
                } else {
                    last_eff = w as u32;
                }
                last_v = v;
            }
        }
        let k = Klass::new();
        d.cls = d.names.iter().map(|n| k.of(n)).collect();
        eprintln!(
            "parallel: {}: {} steps, {} reads, {} definitions, {} names, read in {:.1} s{}",
            path.display(),
            fmt(n as u64),
            fmt(d.r_name.len() as u64),
            fmt(d.w_name.len() as u64),
            fmt(d.names.len() as u64),
            t0.elapsed().as_secs_f64(),
            if lost > 0 {
                format!(" ({lost} reads from steps not in the dump)")
            } else {
                String::new()
            }
        );
        Ok(d)
    }

    fn n(&self) -> usize {
        self.ids.len()
    }

    fn find_write(&self, p: usize, nm: u32) -> u32 {
        let (s, e) = (self.ws[p], self.ws[p + 1]);
        match self.w_name[s..e].binary_search(&nm) {
            Ok(i) => (s + i) as u32,
            Err(_) => NONE,
        }
    }

    fn total(&self) -> u64 {
        self.cost.iter().sum()
    }

    /// Read `k` in model `m`: the position of the step whose definition it
    /// waits for, and that definition's write (`NONE` if the dump has no
    /// write for it); `None` if the model drops it.
    fn kept(&self, k: usize, m: Model) -> Option<(u32, u32)> {
        let a = self.r_from[k];
        if a == NONE {
            return None;
        }
        let d = self.r_def[k];
        if m == Model::All {
            return Some((a, d));
        }
        let own = self.r_own[k];
        if m == Model::SoftReads && own != NONE && self.w_through[own as usize] {
            return None;
        }
        if m == Model::Blind && own != NONE {
            return None;
        }
        if d == NONE {
            return Some((a, d));
        }
        let e = self.w_eff[d as usize];
        Some((self.w_pos[e as usize], e))
    }

    fn bit_of(&self, k: usize) -> u32 {
        1 << self.cls[self.r_name[k] as usize]
    }

    /// Each step's dependencies in model `m`, with the classes each edge
    /// carries.
    fn edges(&self, m: Model) -> Edges {
        let n = self.n();
        let mut e = Edges {
            off: Vec::with_capacity(n + 1),
            from: Vec::new(),
            mask: Vec::new(),
        };
        let mut tmp: Vec<(u32, u32)> = Vec::new();
        for p in 0..n {
            e.off.push(e.from.len());
            tmp.clear();
            for k in self.rs[p]..self.rs[p + 1] {
                if let Some((a, _)) = self.kept(k, m) {
                    tmp.push((a, self.bit_of(k)));
                }
            }
            tmp.sort_unstable_by_key(|t| t.0);
            for &(a, b) in &tmp {
                if e.from.len() > e.off[p] && *e.from.last().expect("an edge") == a {
                    *e.mask.last_mut().expect("an edge") |= b;
                } else {
                    e.from.push(a);
                    e.mask.push(b);
                }
            }
        }
        e.off.push(e.from.len());
        e
    }

    /// The reads of the pipelined schedule in model `m`: per kept read, the
    /// step it waits for, and the definition's time in that step less the
    /// read's time in this one.
    fn lags(&self, m: Model) -> Lags {
        let n = self.n();
        let mut l = Lags {
            off: Vec::with_capacity(n + 1),
            from: Vec::new(),
            read: Vec::new(),
            lag: Vec::new(),
        };
        for p in 0..n {
            l.off.push(l.from.len());
            for k in self.rs[p]..self.rs[p + 1] {
                if let Some((a, w)) = self.kept(k, m) {
                    let wat = if w != NONE && self.w_at[w as usize] != NONE {
                        i64::from(self.w_at[w as usize])
                    } else {
                        self.cost[a as usize] as i64
                    };
                    let rat = if self.r_at[k] == NONE {
                        0
                    } else {
                        i64::from(self.r_at[k])
                    };
                    l.from.push(a);
                    l.read.push(k as u32);
                    l.lag
                        .push((wat - rat).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
                }
            }
        }
        l.off.push(l.from.len());
        l
    }

    /// The names by which position `p` depends on position `a` in `m`.
    fn carried(&self, p: usize, a: u32, m: Model) -> Vec<u32> {
        let mut out: Vec<u32> = (self.rs[p]..self.rs[p + 1])
            .filter(|&k| self.kept(k, m).is_some_and(|(b, _)| b == a))
            .map(|k| self.r_name[k])
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The version of the definition of name `nm` reaching position `pos`
    /// (the last before it), or `None`.
    fn version_at(&self, nm: u32, pos: u32) -> Option<u64> {
        let ws = &self.d_w[self.dn[nm as usize]..self.dn[nm as usize + 1]];
        let i = ws.partition_point(|&w| self.w_pos[w as usize] < pos);
        (i > 0).then(|| self.w_ver[ws[i - 1] as usize])
    }

    /// The first step that ships a page (writes the PDF writer's ship
    /// state, or the DVI's).
    fn first_ship(&self) -> Option<usize> {
        let ship: Vec<u32> = ["pdf.ship", "dvi.ship", "dvi.out"]
            .iter()
            .filter_map(|n| self.ix.get(n.as_bytes()).copied())
            .collect();
        (0..self.n()).find(|&p| {
            self.w_name[self.ws[p]..self.ws[p + 1]]
                .iter()
                .any(|n| ship.contains(n))
        })
    }
}

struct Edges {
    off: Vec<usize>,
    from: Vec<u32>,
    mask: Vec<u32>,
}

struct Lags {
    off: Vec<usize>,
    from: Vec<u32>,
    read: Vec<u32>,
    lag: Vec<i32>,
}

fn fmt(x: u64) -> String {
    let s = x.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn ratio(a: u64, b: u64) -> String {
    if b == 0 {
        "-".into()
    } else {
        format!("{:.2}", a as f64 / b as f64)
    }
}

fn mask_names(m: u32) -> Vec<&'static str> {
    (0..CLASSES.len())
        .filter(|k| m >> k & 1 == 1)
        .map(|k| CLASSES[k])
        .collect()
}

/// Earliest ends of whole steps with unbounded workers, in program order,
/// edges whose classes are all in `skip` ignored; `among`: only those
/// positions run (the others' definitions are there at the start).
fn longest(cost: &[u64], e: &Edges, skip: u32, among: Option<&[bool]>) -> (Vec<u64>, Vec<u32>) {
    let n = cost.len();
    let mut fin = vec![0u64; n];
    let mut pred = vec![NONE; n];
    let keep = !skip;
    for p in 0..n {
        if among.is_some_and(|m| !m[p]) {
            continue;
        }
        let (mut best, mut who) = (0u64, NONE);
        for i in e.off[p]..e.off[p + 1] {
            if e.mask[i] & keep == 0 {
                continue;
            }
            let a = e.from[i] as usize;
            if among.is_some_and(|m| !m[a]) {
                continue;
            }
            if fin[a] > best {
                best = fin[a];
                who = a as u32;
            }
        }
        fin[p] = best + cost[p];
        pred[p] = who;
    }
    (fin, pred)
}

/// Earliest ends of pipelined steps: each begins as late as its
/// latest-bound read needs. Classes in `skip` ignored; `among` as in
/// [`longest`]; with `ok`, a step whose speculation validates begins at
/// once. Returns the ends and, per position, the kept read that set its
/// start.
fn pipelined(
    d: &Dag,
    l: &Lags,
    skip: u32,
    among: Option<&[bool]>,
    ok: Option<&[bool]>,
) -> (Vec<u64>, Vec<u32>) {
    let n = d.n();
    let mut start = vec![0i64; n];
    let mut fin = vec![0u64; n];
    let mut why = vec![NONE; n];
    for p in 0..n {
        if among.is_some_and(|m| !m[p]) {
            continue;
        }
        let (mut s, mut w) = (0i64, NONE);
        if !ok.is_some_and(|o| o[p]) {
            for i in l.off[p]..l.off[p + 1] {
                if d.bit_of(l.read[i] as usize) & skip != 0 {
                    continue;
                }
                let a = l.from[i] as usize;
                if among.is_some_and(|m| !m[a]) {
                    continue;
                }
                let need = start[a] + i64::from(l.lag[i]);
                if need > s {
                    s = need;
                    w = i as u32;
                }
            }
        }
        start[p] = s;
        fin[p] = s as u64 + d.cost[p];
        why[p] = w;
    }
    (fin, why)
}

fn argmax(fin: &[u64]) -> usize {
    let mut best = 0;
    for (p, &f) in fin.iter().enumerate() {
        if f > fin[best] {
            best = p;
        }
    }
    best
}

fn path_of(fin: &[u64], pred: &[u32]) -> Vec<usize> {
    let mut path = vec![argmax(fin)];
    while pred[*path.last().expect("a step")] != NONE {
        path.push(pred[*path.last().expect("a step")] as usize);
    }
    path.reverse();
    path
}

/// The reads that set the starts, back from the last step to end: the
/// chain's positions and each binding read (reader, read index).
fn binding_chain(l: &Lags, fin: &[u64], why: &[u32]) -> (Vec<usize>, Vec<(usize, usize)>) {
    let mut p = argmax(fin);
    let (mut chain, mut reads) = (vec![p], Vec::new());
    while why[p] != NONE {
        let i = why[p] as usize;
        reads.push((p, l.read[i] as usize));
        p = l.from[i] as usize;
        chain.push(p);
    }
    chain.reverse();
    reads.reverse();
    (chain, reads)
}

/// Greedy list scheduling of whole steps on `workers` workers, in program
/// order: the makespan.
fn schedule(cost: &[u64], e: &Edges, workers: usize) -> u64 {
    let mut free: BinaryHeap<Reverse<u64>> = (0..workers).map(|_| Reverse(0)).collect();
    let mut fin = vec![0u64; cost.len()];
    for p in 0..cost.len() {
        let ready = (e.off[p]..e.off[p + 1])
            .map(|i| fin[e.from[i] as usize])
            .max()
            .unwrap_or(0);
        let Reverse(w) = free.pop().expect("a worker");
        fin[p] = w.max(ready) + cost[p];
        free.push(Reverse(fin[p]));
    }
    fin.into_iter().max().unwrap_or(0)
}

/// The most steps running at once, and the mean running in each tenth of
/// the critical path.
fn profile(cost: &[u64], fin: &[u64]) -> (u64, Vec<f64>) {
    let cp = fin.iter().copied().max().unwrap_or(0);
    let mut ev: Vec<(u64, i64)> = Vec::new();
    for (p, &c) in cost.iter().enumerate() {
        if c > 0 {
            ev.push((fin[p] - c, 1));
            ev.push((fin[p], -1));
        }
    }
    ev.sort_unstable();
    let (mut run, mut peak) = (0i64, 0i64);
    for (_, e) in ev {
        run += e;
        peak = peak.max(run);
    }
    let width = (cp as f64 / 10.0).max(1.0);
    let mut busy = [0f64; 10];
    for (p, &c) in cost.iter().enumerate() {
        let (mut a, b) = ((fin[p] - c) as f64, fin[p] as f64);
        let mut k = (a / width) as usize;
        while a < b && k < 10 {
            let hi = b.min((k + 1) as f64 * width);
            busy[k] += hi - a;
            a = hi;
            k += 1;
        }
    }
    (peak as u64, busy.iter().map(|x| x / width).collect())
}

/// The widest depth (unweighted) and the number of depths.
fn widest_level(e: &Edges) -> (u64, u64) {
    let n = e.off.len() - 1;
    let mut lv = vec![0u32; n];
    let mut count: FxMap<u32, u64> = FxMap::default();
    for p in 0..n {
        let l = 1
            + (e.off[p]..e.off[p + 1])
                .map(|i| lv[e.from[i] as usize])
                .max()
                .unwrap_or(0);
        lv[p] = l;
        *count.entry(l).or_default() += 1;
    }
    (
        count.values().copied().max().unwrap_or(0),
        u64::from(lv.into_iter().max().unwrap_or(0)),
    )
}

/// `\countNN` -> `\c@page`, from LaTeX's allocation lines in the logs.
fn register_names(logs: &[String]) -> FxMap<String, String> {
    let re = Regex::new(r"^(\\\S+)=\\(count|dimen|skip|muskip|toks|box)(\d+)\s*$").expect("re");
    let mut out = FxMap::default();
    for path in logs {
        let Ok(text) = std::fs::read(path) else {
            continue;
        };
        for line in String::from_utf8_lossy(&text).lines() {
            if let Some(c) = re.captures(line) {
                out.entry(format!("\\{}{}", &c[2], &c[3]))
                    .or_insert_with(|| c[1].to_string());
            }
        }
    }
    out
}

struct Ctx<'a> {
    d: &'a Dag,
    regs: FxMap<String, String>,
    /// Each position's segment (an index in `segs`), or `NONE` in the setup.
    seg_of: Vec<u32>,
    segs: Vec<Segment>,
}

struct Segment {
    name: String,
    start: usize,
    end: usize,
    cost: u64,
}

impl Ctx<'_> {
    fn shown(&self, nm: u32) -> String {
        let n = &self.d.names[nm as usize];
        match self.regs.get(n) {
            Some(r) => format!("{n}={r}"),
            None => n.clone(),
        }
    }

    fn step(&self, p: usize) -> String {
        let seg = self.seg_of[p];
        let s = if seg == NONE {
            "setup"
        } else {
            self.segs[seg as usize].name.as_str()
        };
        format!("step {} {} [{s}]", self.d.ids[p], fmt(self.d.cost[p]))
    }

    fn steps_shown(&self, ps: &[usize], k: usize) -> String {
        let mut v = ps.to_vec();
        v.sort_unstable_by_key(|&p| Reverse(self.d.cost[p]));
        v.truncate(k);
        v.iter()
            .map(|&p| self.step(p))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// What a whole-step critical path is made of.
    fn composition(&self, e: &Edges, fin: &[u64], pred: &[u32], m: Model) {
        let d = self.d;
        let path = path_of(fin, pred);
        let mut by_class = [0u64; 21];
        let mut alone = [0u64; 21];
        let mut weight = [0u64; 21];
        let mut by_name: FxMap<u32, (u64, u64)> = FxMap::default();
        for w in path.windows(2) {
            let (a, b) = (w[0], w[1]);
            let mask = (e.off[b]..e.off[b + 1])
                .find(|&i| e.from[i] as usize == a)
                .map_or(0, |i| e.mask[i]);
            for k in 0..21 {
                if mask >> k & 1 == 1 {
                    by_class[k] += 1;
                    weight[k] += d.cost[b];
                }
            }
            if mask.is_power_of_two() {
                alone[mask.trailing_zeros() as usize] += 1;
            }
            for nm in d.carried(b, a as u32, m) {
                let x = by_name.entry(nm).or_default();
                x.0 += 1;
                x.1 += d.cost[b];
            }
        }
        let mut cs: Vec<usize> = (0..21).filter(|&k| by_class[k] > 0).collect();
        cs.sort_unstable_by_key(|&k| Reverse(by_class[k]));
        println!(
            "  the path: {} steps, {} commands; its edges by class (edges, carried by the class \
             alone, commands of the steps they lead to): {}",
            fmt(path.len() as u64),
            fmt(path.iter().map(|&p| d.cost[p]).sum()),
            cs.iter()
                .take(10)
                .map(|&k| format!(
                    "{} {} ({}, {})",
                    CLASSES[k],
                    fmt(by_class[k]),
                    fmt(alone[k]),
                    fmt(weight[k])
                ))
                .collect::<Vec<_>>()
                .join("; ")
        );
        let mut names: Vec<(u32, (u64, u64))> = by_name.into_iter().collect();
        names.sort_unstable_by_key(|x| Reverse(x.1.0));
        println!(
            "  the names on most of its edges: {}",
            names
                .iter()
                .take(12)
                .map(|(n, c)| format!("{} {}", self.shown(*n), fmt(c.0)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        names.sort_unstable_by_key(|x| Reverse(x.1.1));
        println!(
            "  the names before the most commands: {}",
            names
                .iter()
                .take(12)
                .map(|(n, c)| format!("{} {}", self.shown(*n), fmt(c.1)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("  its costliest steps: {}", self.steps_shown(&path, 4));
    }

    /// What a pipelined critical path is made of: the binding reads.
    fn chain_composition(&self, l: &Lags, fin: &[u64], why: &[u32]) {
        let d = self.d;
        let (chain, reads) = binding_chain(l, fin, why);
        let mut by_class = [0u64; 21];
        let mut by_name: FxMap<u32, u64> = FxMap::default();
        for &(p, k) in &reads {
            by_class[d.cls[d.r_name[k] as usize] as usize] += d.cost[p];
            *by_name.entry(d.r_name[k]).or_default() += d.cost[p];
        }
        let mut cs: Vec<usize> = (0..21).filter(|&k| by_class[k] > 0).collect();
        cs.sort_unstable_by_key(|&k| Reverse(by_class[k]));
        println!(
            "  the chain: {} steps, {} commands; its binding reads by class (commands of the \
             steps they hold back): {}",
            fmt(chain.len() as u64),
            fmt(chain.iter().map(|&p| d.cost[p]).sum()),
            cs.iter()
                .take(8)
                .map(|&k| format!("{} {}", CLASSES[k], fmt(by_class[k])))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut names: Vec<(u32, u64)> = by_name.into_iter().collect();
        names.sort_unstable_by_key(|x| Reverse(x.1));
        println!(
            "  by name: {}",
            names
                .iter()
                .take(12)
                .map(|(n, c)| format!("{} {}", self.shown(*n), fmt(*c)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("  its costliest steps: {}", self.steps_shown(&chain, 4));
    }
}

/// Classes ignored one after another, each time the one (or pair) on the
/// current critical path whose removal shortens it most.
fn peel(total: u64, big: u64, label: &str, compute: &dyn Fn(u32) -> (u64, u32)) {
    println!("  classes ignored one after another ({label}):");
    let mut skip = 0u32;
    let (mut cur, mut on) = compute(0);
    for _ in 0..8 {
        let cands: Vec<u32> = (0..CLASSES.len())
            .map(|k| 1u32 << k)
            .filter(|c| (on & !skip) & c != 0)
            .collect();
        let mut best: Option<(u64, u32, u32)> = None;
        for &c in &cands {
            let (v, o) = compute(skip | c);
            if best.is_none_or(|b| v < b.0) {
                best = Some((v, c, o));
            }
        }
        if best.is_some_and(|b| b.0 >= cur) {
            for (i, &c1) in cands.iter().enumerate() {
                for &c2 in &cands[i + 1..] {
                    let (v, o) = compute(skip | c1 | c2);
                    if best.is_none_or(|b| v < b.0) {
                        best = Some((v, c1 | c2, o));
                    }
                }
            }
        }
        let Some(b) = best.filter(|b| b.0 < cur) else {
            println!("    (no class on the path shortens it)");
            break;
        };
        cur = b.0;
        skip |= b.1;
        on = b.2;
        println!(
            "    + {}: critical path {}, bound {}x",
            mask_names(b.1).join(" + "),
            fmt(cur),
            ratio(total, cur)
        );
        if cur <= big {
            break;
        }
    }
}

fn keep_only(total: u64, compute: &dyn Fn(u32) -> (u64, u32)) {
    let page = bit("page builder");
    let nest = bit("current list (nest)");
    let count = bit("counters (\\count)");
    let cursors = bit("allocators")
        | bit("log and terminal")
        | bit("engine scalars")
        | bit("conditionals")
        | bit("save stack")
        | bit("hash table");
    for (label, keep) in [
        ("the page builder's alone", page),
        ("the page builder's and the nest's", page | nest),
        (
            "the page builder's, the nest's and the counters'",
            page | nest | count,
        ),
        (
            "all but the cursors' (allocators, log, engine scalars, conditionals, save stack, hash)",
            EVERY & !cursors,
        ),
    ] {
        let (v, _) = compute(EVERY & !keep);
        println!(
            "    edges kept: {label}: critical path {}, bound {}x",
            fmt(v),
            ratio(total, v)
        );
    }
}

fn whole_compute<'a>(d: &'a Dag, e: &'a Edges) -> impl Fn(u32) -> (u64, u32) + 'a {
    move |skip| {
        let (fin, pred) = longest(&d.cost, e, skip, None);
        let path = path_of(&fin, &pred);
        let mut on = 0;
        for w in path.windows(2) {
            let (a, b) = (w[0], w[1]);
            for i in e.off[b]..e.off[b + 1] {
                if e.from[i] as usize == a {
                    on |= e.mask[i];
                }
            }
        }
        (fin.into_iter().max().unwrap_or(0), on)
    }
}

fn piped_compute<'a>(d: &'a Dag, l: &'a Lags) -> impl Fn(u32) -> (u64, u32) + 'a {
    move |skip| {
        let (fin, why) = pipelined(d, l, skip, None, None);
        let (_, reads) = binding_chain(l, &fin, &why);
        let mut on = 0;
        for (_, k) in reads {
            on |= d.bit_of(k);
        }
        (fin.into_iter().max().unwrap_or(0), on)
    }
}

/// The body's segments: a new one at each load (after the setup) of a
/// name `re` matches.
fn segments(d: &Dag, body: usize, re: &Regex) -> (Vec<u32>, Vec<Segment>) {
    let n = d.n();
    let mut starts: Vec<(usize, String)> = vec![(body, "(the body's start)".into())];
    for &(p, nm) in &d.loads {
        let p = p as usize;
        let name = &d.names[nm as usize];
        if p >= body && re.is_match(name) {
            if starts.last().is_some_and(|s| s.0 == p) {
                starts.last_mut().expect("a segment").1.clone_from(name);
            } else {
                starts.push((p, name.clone()));
            }
        }
    }
    let mut segs = Vec::new();
    let mut seg_of = vec![NONE; n];
    for (i, (s, name)) in starts.iter().enumerate() {
        let e = starts.get(i + 1).map_or(n, |x| x.0);
        if e <= *s {
            continue;
        }
        for q in seg_of.iter_mut().take(e).skip(*s) {
            *q = segs.len() as u32;
        }
        segs.push(Segment {
            name: name.clone(),
            start: *s,
            end: e,
            cost: d.cost[*s..e].iter().sum(),
        });
    }
    (seg_of, segs)
}

/// Speculation from the setup's end, by segment and by step: the classes
/// of each unit's mispredicted reads (a read of a definition made after
/// the setup and before the unit, whose version is not the setup's).
fn from_setup(cx: &Ctx, body: usize, m: Model, label: &str) {
    let d = cx.d;
    let n = d.n();
    // (the version each name had at the setup's end, looked up once)
    let mut at_body: FxMap<u32, Option<u64>> = FxMap::default();
    let mut seg_mask = vec![0u32; cx.segs.len()];
    let mut step_mask = vec![0u32; n];
    let mut seg_names: Vec<FxMap<u32, u64>> = vec![FxMap::default(); 21];
    let mut step_names: Vec<FxMap<u32, u64>> = vec![FxMap::default(); 21];
    // (each unit's mispredicted reads by the unit that made the definition
    // read, and its class: the rounds ([`report_units`]))
    let mut seg_src: Vec<Vec<(u32, u8)>> = vec![Vec::new(); cx.segs.len()];
    let mut step_src: Vec<Vec<(u32, u8)>> = vec![Vec::new(); n - body];
    for p in body..n {
        let s = cx.seg_of[p];
        let s_start = if s == NONE {
            body
        } else {
            cx.segs[s as usize].start
        };
        for k in d.rs[p]..d.rs[p + 1] {
            let Some((a, w)) = d.kept(k, m) else { continue };
            let a = a as usize;
            if a < body {
                continue;
            }
            let nm = d.r_name[k];
            let v = if w == NONE {
                NOVER
            } else {
                d.w_ver[w as usize]
            };
            let pred = *at_body
                .entry(nm)
                .or_insert_with(|| d.version_at(nm, body as u32));
            if pred == Some(v) && v != NOVER {
                continue;
            }
            let c = d.cls[nm as usize] as usize;
            step_mask[p] |= 1 << c;
            *step_names[c].entry(nm).or_default() += 1;
            step_src[p - body].push(((a - body) as u32, c as u8));
            if a < s_start && s != NONE {
                seg_mask[s as usize] |= 1 << c;
                *seg_names[c].entry(nm).or_default() += 1;
                let from = cx.seg_of[a];
                if from != NONE {
                    seg_src[s as usize].push((from, c as u8));
                }
            }
        }
        let v = &mut step_src[p - body];
        v.sort_unstable();
        v.dedup();
    }
    for v in &mut seg_src {
        v.sort_unstable();
        v.dedup();
    }
    let setup: u64 = d.cost[..body].iter().sum();
    let body_cost: u64 = d.cost[body..].iter().sum();
    println!("\n{label}:");
    report_units(
        cx,
        "segments",
        &cx.segs.iter().map(|s| s.cost).collect::<Vec<_>>(),
        &seg_mask,
        &seg_names,
        &seg_src,
        setup,
        body_cost,
        true,
    );
    report_units(
        cx,
        "steps",
        &d.cost[body..],
        &step_mask[body..],
        &step_names,
        &step_src,
        setup,
        body_cost,
        false,
    );
}

/// One kind of unit's validation from the setup's end: as is, then with
/// classes taken as predicted, the one that validates the most commands
/// first; the bound (the setup, then every unit that validates at once,
/// a unit that does not after the one before it).
#[allow(clippy::too_many_arguments)]
fn report_units(
    cx: &Ctx,
    what: &str,
    cost: &[u64],
    mask: &[u32],
    names: &[FxMap<u32, u64>],
    src: &[Vec<(u32, u8)>],
    setup: u64,
    body_cost: u64,
    chained: bool,
) {
    let total = setup + body_cost;
    // (rounds, as Jacobi's iteration: every unit runs at once from the
    // setup's end; a unit that misread a definition of an earlier unit
    // runs again, in the round after that unit's last; each round takes as
    // long as its costliest unit: the rounds, their time and the work)
    let rounds = |known: u32| -> (u32, u64, u64) {
        let mut round = vec![1u32; mask.len()];
        for u in 0..mask.len() {
            let mut r = 1;
            for &(v, c) in &src[u] {
                if known >> c & 1 == 0 {
                    r = r.max(round[v as usize] + 1);
                }
            }
            round[u] = r;
        }
        let rmax = round.iter().copied().max().unwrap_or(1) as usize;
        let mut best = vec![0u64; rmax + 2];
        let mut work = 0u64;
        for (u, &r) in round.iter().enumerate() {
            best[r as usize] = best[r as usize].max(cost[u]);
            work += u64::from(r) * cost[u];
        }
        for r in (1..=rmax).rev() {
            best[r] = best[r].max(best[r + 1]);
        }
        (rmax as u32, best[1..=rmax].iter().sum(), work)
    };
    let round_line = |known: u32| -> String {
        let (r, t, w) = rounds(known);
        format!(
            "in rounds: {r} rounds, {} commands' time, bound {}x, work {:.2}x",
            fmt(setup + t),
            ratio(total, setup + t),
            w as f64 / body_cost.max(1) as f64
        )
    };
    let validates = |known: u32| -> (usize, u64, u64) {
        let (mut k, mut c) = (0usize, 0u64);
        let mut fin = setup;
        let mut cp = setup;
        for (i, &m) in mask.iter().enumerate() {
            let ok = m & !known == 0;
            if ok {
                k += 1;
                c += cost[i];
            }
            // (a unit that validates runs at once after the setup; one
            // that does not, after the unit before it)
            fin = if ok { setup + cost[i] } else { fin + cost[i] };
            cp = cp.max(fin);
        }
        (k, c, cp)
    };
    let (k, c, cp) = validates(0);
    let biggest = cost.iter().copied().max().unwrap_or(0);
    println!(
        "  {what}: {} ({} commands, the largest {}); from the setup's end {} validate ({} \
         commands, {:.1}%); critical path {}, bound {}x; were all to validate, {}x",
        fmt(cost.len() as u64),
        fmt(body_cost),
        fmt(biggest),
        fmt(k as u64),
        fmt(c),
        100.0 * c as f64 / body_cost.max(1) as f64,
        fmt(cp),
        ratio(total, cp),
        ratio(total, setup + biggest)
    );
    println!("    {}", round_line(0));
    // (the classes that block, by the units and commands they block)
    let mut blocks: Vec<(usize, u64, usize)> = (0..21)
        .map(|cl| {
            let (mut u, mut cc) = (0usize, 0u64);
            for (i, &m) in mask.iter().enumerate() {
                if m >> cl & 1 == 1 {
                    u += 1;
                    cc += cost[i];
                }
            }
            (u, cc, cl)
        })
        .filter(|x| x.0 > 0)
        .collect();
    blocks.sort_unstable_by_key(|x| Reverse(x.1));
    for &(u, cc, cl) in blocks.iter().take(if chained { 12 } else { 8 }) {
        let mut ns: Vec<(&u32, &u64)> = names[cl].iter().collect();
        ns.sort_unstable_by_key(|x| Reverse(*x.1));
        println!(
            "    {}: blocks {} {what} ({} commands); names: {}",
            CLASSES[cl],
            fmt(u as u64),
            fmt(cc),
            ns.iter()
                .take(6)
                .map(|(n, k)| format!("{} {}", cx.shown(**n), fmt(**k)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // (classes taken as predicted, greedily)
    let mut known = 0u32;
    let mut cur = c;
    for _ in 0..10 {
        let mut best: Option<(u64, u32)> = None;
        for cl in 0..21u32 {
            if known >> cl & 1 == 1 {
                continue;
            }
            let (_, cc, _) = validates(known | 1 << cl);
            if best.is_none_or(|b| cc > b.0) {
                best = Some((cc, 1 << cl));
            }
        }
        let Some((cc, b)) = best.filter(|b| b.0 > cur) else {
            break;
        };
        known |= b;
        cur = cc;
        let (kk, _, cp) = validates(known);
        println!(
            "    + {} predicted: {} validate ({:.1}% of the commands), critical path {}, bound {}x; {}",
            mask_names(b).join(""),
            fmt(kk as u64),
            100.0 * cc as f64 / body_cost.max(1) as f64,
            fmt(cp),
            ratio(total, cp),
            round_line(known)
        );
    }
}

fn cold(path: &str, logs: &[String], seg_re: &str) -> Result<()> {
    let d = Dag::open(Path::new(path))?;
    let n = d.n();
    if n == 0 {
        bail!("{path}: no steps");
    }
    let total = d.total();
    let big = d.cost.iter().copied().max().unwrap_or(0);
    let mut sorted = d.cost.clone();
    sorted.sort_unstable_by_key(|&c| Reverse(c));
    let body = d.first_ship();
    let re = Regex::new(seg_re).context("--segments")?;
    let (seg_of, segs) = match body {
        Some(b) => segments(&d, b, &re),
        None => (vec![NONE; n], Vec::new()),
    };
    let cx = Ctx {
        d: &d,
        regs: register_names(logs),
        seg_of,
        segs,
    };
    println!(
        "steps {}, commands {} (each step's last run); the costliest step {} commands (alone, a \
         bound of {}x), the mean {}, the median {}; reads from outside a step {}, definitions {}",
        fmt(n as u64),
        fmt(total),
        fmt(big),
        ratio(total, big),
        fmt(total / n as u64),
        fmt(sorted[n / 2]),
        fmt(d.r_name.len() as u64),
        fmt(d.w_name.len() as u64)
    );
    let body_mask: Option<Vec<bool>> = body.map(|b| (0..n).map(|p| p >= b).collect());
    if let Some(b) = body {
        let setup: u64 = d.cost[..b].iter().sum();
        println!(
            "the setup (before the first step that ships, step {}): {} steps, {} commands ({:.1}%); \
             the body {} steps, {} commands in {} segments",
            d.ids[b],
            fmt(b as u64),
            fmt(setup),
            100.0 * setup as f64 / total as f64,
            fmt((n - b) as u64),
            fmt(total - setup),
            cx.segs.len()
        );
    }
    println!(
        "the costliest steps: {}",
        cx.steps_shown(&(0..n).collect::<Vec<_>>(), 8)
    );
    // (the commands in steps by size)
    let mut line = String::from("commands in steps of at least");
    for k in 2..=7 {
        let lim = 10u64.pow(k);
        let c: u64 = d.cost.iter().filter(|&&c| c >= lim).sum();
        let _ = write!(line, " 10^{k}: {:.1}%;", 100.0 * c as f64 / total as f64);
    }
    println!("{line}");
    if !cx.segs.is_empty() {
        let mut ss: Vec<&Segment> = cx.segs.iter().collect();
        ss.sort_unstable_by_key(|s| Reverse(s.cost));
        println!(
            "the costliest segments: {}",
            ss.iter()
                .take(8)
                .map(|s| format!(
                    "{} {} ({} steps)",
                    s.name,
                    fmt(s.cost),
                    fmt((s.end - s.start) as u64)
                ))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    {
        let mut defs: Vec<(usize, u32)> = (0..d.names.len())
            .map(|nm| (d.dn[nm + 1] - d.dn[nm], nm as u32))
            .collect();
        defs.sort_unstable_by_key(|x| Reverse(x.0));
        let mut readers: FxMap<u32, u64> = FxMap::default();
        for k in 0..d.r_name.len() {
            if d.r_from[k] != NONE {
                *readers.entry(d.r_name[k]).or_default() += 1;
            }
        }
        println!(
            "the addresses defined by the most steps (definitions/reads of another step's): {}",
            defs.iter()
                .take(14)
                .map(|(k, nm)| format!(
                    "{} {}/{}",
                    cx.shown(*nm),
                    fmt(*k as u64),
                    fmt(readers.get(nm).copied().unwrap_or(0))
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut by_cls = [0u64; 21];
        let mut through = 0u64;
        for (w, &t) in d.w_through.iter().enumerate() {
            if t {
                through += 1;
                by_cls[d.cls[d.w_name[w] as usize] as usize] += 1;
            }
        }
        let mut cs: Vec<usize> = (0..21).filter(|&k| by_cls[k] > 0).collect();
        cs.sort_unstable_by_key(|&k| Reverse(by_cls[k]));
        println!(
            "definitions equal to the one before them: {} of {} ({})",
            fmt(through),
            fmt(d.w_name.len() as u64),
            cs.iter()
                .take(6)
                .map(|&k| format!("{} {}", CLASSES[k], fmt(by_cls[k])))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "\n== whole steps (the input chain alone, every step after the one before it, is 1.00x)"
    );
    for (m, label) in MODELS {
        let t0 = Instant::now();
        let e = d.edges(m);
        let (fin, pred) = longest(&d.cost, &e, 0, None);
        let cp = fin.iter().copied().max().unwrap_or(0);
        let mut line = format!(
            "  {label}: edges {}, critical path {}, bound {}x",
            fmt(e.from.len() as u64),
            fmt(cp),
            ratio(total, cp)
        );
        if let (Some(b), Some(bm)) = (body, &body_mask) {
            let (bf, _) = longest(&d.cost, &e, 0, Some(bm));
            let bt: u64 = d.cost[b..].iter().sum();
            let _ = write!(
                line,
                "; the body alone {}x",
                ratio(bt, bf[b..].iter().copied().max().unwrap_or(0))
            );
        }
        println!("{line} ({:.1} s)", t0.elapsed().as_secs_f64());
        if matches!(m, Model::All | Model::Blind) {
            println!("\n{label}:");
            cx.composition(&e, &fin, &pred, m);
            let (peak, prof) = profile(&d.cost, &fin);
            let (width, depth) = widest_level(&e);
            println!(
                "  profile: at most {} steps at once; the widest depth {} steps (of {}); mean steps \
                 running per tenth of the critical path: {}",
                fmt(peak),
                fmt(width),
                fmt(depth),
                prof.iter()
                    .map(|x| format!("{x:.1}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            println!(
                "  list schedules (program order, P workers): {}",
                [2usize, 8, 64]
                    .iter()
                    .map(|&k| format!("P={k} {}x", ratio(total, schedule(&d.cost, &e, k))))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            peel(
                total,
                big,
                &format!("whole steps, {label}"),
                &whole_compute(&d, &e),
            );
            println!("  classes kept (whole steps, {label}):");
            keep_only(total, &whole_compute(&d, &e));
            println!();
        }
    }
    if d.timed {
        println!("\n== pipelined (a step may begin before the steps it reads from end)");
        for (m, label) in MODELS {
            let l = d.lags(m);
            let (fin, why) = pipelined(&d, &l, 0, None, None);
            let cp = fin.iter().copied().max().unwrap_or(0);
            let mut line = format!(
                "  {label}: critical path {}, bound {}x",
                fmt(cp),
                ratio(total, cp)
            );
            if let (Some(b), Some(bm)) = (body, &body_mask) {
                let (bf, _) = pipelined(&d, &l, 0, Some(bm), None);
                let bt: u64 = d.cost[b..].iter().sum();
                let _ = write!(
                    line,
                    "; the body alone {}x",
                    ratio(bt, bf[b..].iter().copied().max().unwrap_or(0))
                );
            }
            println!("{line}");
            if matches!(m, Model::SoftReads | Model::Blind) {
                cx.chain_composition(&l, &fin, &why);
                let (peak, prof) = profile(&d.cost, &fin);
                println!(
                    "  profile: at most {} steps at once; mean steps running per tenth of the critical \
                     path: {}",
                    fmt(peak),
                    prof.iter()
                        .map(|x| format!("{x:.1}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
            if m == Model::Blind {
                peel(
                    total,
                    big,
                    &format!("pipelined, {label}"),
                    &piped_compute(&d, &l),
                );
                println!("  classes kept (pipelined, {label}):");
                keep_only(total, &piped_compute(&d, &l));
            }
        }
    }
    if let Some(b) = body {
        println!(
            "\n== speculation from the setup's end (each unit from the state the setup left; \
             segments run their steps in order)"
        );
        for (m, label) in MODELS {
            if m != Model::SoftDefs {
                from_setup(&cx, b, m, label);
            }
        }
    }
    Ok(())
}

/// Whether the step at position `p` of `new`, run from its entry state in
/// `old`, reads what it reads in `new`: the first read that differs, or
/// `None` (as `scripts/ssa-parallel.py`'s `validated`).
fn validated(old: &Dag, new: &Dag, tr: &[u32], p: usize, m: Model) -> Option<String> {
    let id = new.ids[p] as usize;
    let q = old.pos_of.get(id).copied().unwrap_or(NONE);
    if q == NONE {
        return Some("(a new step)".into());
    }
    for k in new.rs[p]..new.rs[p + 1] {
        let a = new.r_from[k];
        if a == NONE {
            continue;
        }
        let own = new.r_own[k];
        if m == Model::SoftReads && own != NONE && new.w_through[own as usize] {
            continue;
        }
        if m == Model::Blind && own != NONE {
            continue;
        }
        let nm = new.r_name[k];
        let w = new.r_def[k];
        let v_new = if w == NONE {
            None
        } else {
            Some(new.w_ver[w as usize])
        };
        let on = tr[nm as usize];
        let v_old = if on == NONE {
            None
        } else {
            old.version_at(on, q)
        };
        if v_new != v_old {
            return Some(new.names[nm as usize].clone());
        }
    }
    None
}

fn rebuild(old_path: &str, new_path: &str, logs: &[String], show: usize) -> Result<()> {
    let old = Dag::open(Path::new(old_path))?;
    let new = Dag::open(Path::new(new_path))?;
    let n = new.n();
    let tr: Vec<u32> = new
        .names
        .iter()
        .map(|s| old.ix.get(s.as_bytes()).copied().unwrap_or(NONE))
        .collect();
    let in_old = |p: usize| {
        let q = old.pos_of.get(new.ids[p] as usize).copied().unwrap_or(NONE);
        (q != NONE).then_some(q as usize)
    };
    let rr: Vec<usize> = (0..n)
        .filter(|&p| in_old(p).is_none_or(|q| old.run[q] != new.run[p]))
        .collect();
    let mut is_r = vec![false; n];
    for &p in &rr {
        is_r[p] = true;
    }
    let total: u64 = rr.iter().map(|&p| new.cost[p]).sum();
    let big_t = new.total();
    let newc = rr.iter().filter(|&&p| in_old(p).is_none()).count();
    let gone = old
        .ids
        .iter()
        .filter(|&&id| new.pos_of.get(id as usize).copied().unwrap_or(NONE) == NONE)
        .count();
    let body = new.first_ship().unwrap_or(0);
    let (seg_of, segs) = segments(&new, body, &Regex::new(r"\.tex$").expect("re"));
    let cx = Ctx {
        d: &new,
        regs: register_names(logs),
        seg_of,
        segs,
    };
    println!(
        "steps re-run {} ({} commands; new {newc}, removed {gone}); the build: {} steps, {} \
         commands, the costliest step {}",
        fmt(rr.len() as u64),
        fmt(total),
        fmt(n as u64),
        fmt(big_t),
        fmt(new.cost.iter().copied().max().unwrap_or(0))
    );
    if rr.is_empty() {
        return Ok(());
    }
    for (m, label) in MODELS {
        if m == Model::SoftDefs {
            continue;
        }
        let e = new.edges(m);
        let differs: Vec<Option<String>> = rr
            .iter()
            .map(|&p| validated(&old, &new, &tr, p, m))
            .collect();
        let mut ok = vec![false; n];
        for (i, &p) in rr.iter().enumerate() {
            ok[p] = differs[i].is_none();
        }
        println!("\n{label}:");
        for (i, &p) in rr.iter().enumerate().take(show) {
            let deps: Vec<String> = (e.off[p]..e.off[p + 1])
                .filter(|&j| is_r[e.from[j] as usize])
                .map(|j| {
                    let a = e.from[j];
                    format!(
                        "{}: {}",
                        new.ids[a as usize],
                        new.carried(p, a, m)
                            .iter()
                            .take(3)
                            .map(|&nm| cx.shown(nm))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                })
                .collect();
            let verdict = match &differs[i] {
                None => "validates".to_string(),
                Some(nm) => format!("fails ({nm} differs)"),
            };
            println!(
                "  {}: {verdict}; reads from re-run steps: {}",
                cx.step(p),
                if deps.is_empty() {
                    "-".to_string()
                } else {
                    deps.join("; ")
                }
            );
        }
        if rr.len() > show {
            println!("  ... {} more", rr.len() - show);
        }
        let mut reasons: FxMap<&str, u64> = FxMap::default();
        for x in differs.iter().flatten() {
            *reasons.entry(x.as_str()).or_default() += 1;
        }
        if !reasons.is_empty() {
            let mut v: Vec<(&str, u64)> = reasons.into_iter().collect();
            v.sort_unstable_by_key(|x| Reverse(x.1));
            println!(
                "  the first read that differs, by name: {}",
                v.iter()
                    .take(8)
                    .map(|(nm, k)| format!("{nm} {k}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let (fin, pred) = longest(&new.cost, &e, 0, Some(&is_r));
        let end = rr.iter().copied().max_by_key(|&p| fin[p]).expect("a step");
        let mut path = vec![end];
        while pred[*path.last().expect("a step")] != NONE {
            path.push(pred[*path.last().expect("a step")] as usize);
        }
        path.reverse();
        let cp = fin[end];
        let k = rr.iter().filter(|&&p| ok[p]).count();
        let mut spec = vec![0u64; n];
        for &p in &rr {
            spec[p] = if ok[p] {
                new.cost[p]
            } else {
                (e.off[p]..e.off[p + 1])
                    .filter(|&j| is_r[e.from[j] as usize])
                    .map(|j| spec[e.from[j] as usize])
                    .max()
                    .unwrap_or(0)
                    + new.cost[p]
            };
        }
        let sp = rr.iter().map(|&p| spec[p]).max().unwrap_or(0);
        println!(
            "  whole steps: dependency-respecting, critical path {} over {} steps, bound {}x; \
             speculated (every re-run step at once from its recorded entry state): {k} of {} \
             validate ({} of {} commands), critical path {}, bound {}x",
            fmt(cp),
            path.len(),
            ratio(total, cp),
            rr.len(),
            fmt(rr.iter().filter(|&&p| ok[p]).map(|&p| new.cost[p]).sum()),
            fmt(total),
            fmt(sp),
            ratio(total, sp)
        );
        let mut cfin = vec![0u64; n];
        for p in 0..n {
            let wait = if is_r[p] && !ok[p] {
                (e.off[p]..e.off[p + 1])
                    .map(|j| cfin[e.from[j] as usize])
                    .max()
                    .unwrap_or(0)
            } else {
                0
            };
            cfin[p] = new.cost[p] + wait;
        }
        let fails = rr.iter().filter(|&&p| !ok[p]).count();
        let ccp = cfin.iter().copied().max().unwrap_or(0);
        println!(
            "  a cold build speculated from the last build's records: {} of {} steps validate \
             ({:.3}%); critical path {}, bound {}x",
            fmt((n - fails) as u64),
            fmt(n as u64),
            100.0 * (n - fails) as f64 / n as f64,
            fmt(ccp),
            ratio(big_t, ccp)
        );
        if new.timed {
            let l = new.lags(m);
            let (a, _) = pipelined(&new, &l, 0, Some(&is_r), None);
            let (b, _) = pipelined(&new, &l, 0, Some(&is_r), Some(&ok));
            let everyone: Vec<bool> = (0..n).map(|p| !is_r[p] || ok[p]).collect();
            let (c, _) = pipelined(&new, &l, 0, None, Some(&everyone));
            let a = rr.iter().map(|&p| a[p]).max().unwrap_or(0);
            let b = rr.iter().map(|&p| b[p]).max().unwrap_or(0);
            let c = c.into_iter().max().unwrap_or(0);
            println!(
                "  pipelined: dependency-respecting {}, bound {}x; speculated {}, bound {}x; a cold \
                 build speculated {}, bound {}x",
                fmt(a),
                ratio(total, a),
                fmt(b),
                ratio(total, b),
                fmt(c),
                ratio(big_t, c)
            );
        }
    }
    Ok(())
}

pub fn run(args: &[String]) -> Result<()> {
    let mut pos: Vec<String> = Vec::new();
    let mut logs: Vec<String> = Vec::new();
    let mut seg_re = String::from(r"\.tex$");
    let mut show = 30usize;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--log" => logs.push(it.next().context("--log FILE")?.clone()),
            "--segments" => seg_re.clone_from(it.next().context("--segments REGEX")?),
            "--show" => show = it.next().context("--show N")?.parse()?,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => pos.push(a.clone()),
        }
    }
    match pos
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["cold", dag] => cold(dag, &logs, &seg_re),
        ["rebuild", old, new] => rebuild(old, new, &logs, show),
        _ => bail!("{USAGE}"),
    }
}
