//! The trace's text form (`DESIGN.md` §9), shaped like LLVM IR: one
//! instruction per line, versions named `%vN` in order of first
//! appearance with their content hash after `;`, a load as a `phi`, a
//! call's reads, writes, effects and nested calls indented beneath it,
//! and each evaluated call marked `hit`, `new` or `miss=@<first read
//! that differed>`, so two dumps diff like `-print-after-all`.
//!
//! ```text
//! trip 0 {
//!   %v0 = phi [%v0, undef] @stream:aux ; %v0=#…
//!   %v1 = call @main(%v2) ; name=#… new %v1=#… %v2=#…
//!     read @phi:stream:aux = %v0
//!     write @var:x = %v3 ; %v3=#…
//!     effect "emit hello"
//!     store @stream:aux %v4 ; %v4=#…
//!     wrote @var:x
//!   changed @stream:aux lines=1
//! }
//! ```

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write as _;

use crate::hash::Version;
use crate::machine::Machine;
use crate::runtime::{Item, RecId, Runtime, Status as RStatus};
use crate::value::{Value, version_opt};

/// A build's trace.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Trace {
    pub trips: Vec<Trip>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Trip {
    pub index: u32,
    pub phis: Vec<Phi>,
    pub body: Vec<Node>,
    pub notes: Vec<Note>,
}

/// A load's φ: the previous build's value, the previous trip's (none in
/// trip 0), and the value loaded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Phi {
    pub stream: String,
    pub prev_build: Version,
    pub prev_trip: Option<Version>,
    pub value: Version,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Status {
    Hit,
    New,
    Miss(String),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Call {
    pub func: String,
    pub args: Vec<Version>,
    pub result: Version,
    pub name: Version,
    pub status: Status,
    pub reads: Vec<(String, Version)>,
    pub writes: Vec<(String, Version)>,
    pub body: Vec<Node>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Node {
    Call(Call),
    Effect(String),
    Store(String, Version),
    Open(String),
    /// The call's body wrote the slot here first (`runtime::Item::Wrote`).
    Wrote(String),
}

/// A stream at the end of a trip: equal to its φ or changed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Note {
    pub stream: String,
    pub converged: bool,
    pub lines: u64,
}

impl<M: Machine> Runtime<M> {
    /// The last build's trace. Every record it names is kept (the last
    /// build is always among the kept ones).
    #[must_use]
    pub fn trace(&self) -> Trace {
        let mut trips = Vec::new();
        for t in &self.log {
            let mut phis = Vec::new();
            let mut names: Vec<&M::Addr> = t.phi.entries().into_iter().map(|(k, _)| k).collect();
            names.extend(self.start_phi.entries().into_iter().map(|(k, _)| k));
            names.sort();
            names.dedup();
            for s in names {
                let pb = self
                    .start_phi
                    .get(s)
                    .map_or(Version::ABSENT, crate::pvec::PVec::version);
                let pt = t
                    .phi
                    .get(s)
                    .map_or(Version::ABSENT, crate::pvec::PVec::version);
                phis.push(Phi {
                    stream: s.to_string(),
                    prev_build: pb,
                    prev_trip: (t.index > 0).then_some(pt),
                    value: if t.index > 0 { pt } else { pb },
                });
            }
            let mut cursor = 0;
            let body = self.nodes(&t.root, &t.statuses, &mut cursor, false);
            let mut notes: Vec<Note> = t
                .streams
                .iter()
                .map(|(k, n)| Note {
                    stream: k.to_string(),
                    converged: !t.changed.iter().any(|c| c.stream == *k),
                    lines: *n as u64,
                })
                .collect();
            for c in &t.changed {
                if !t.streams.contains_key(&c.stream) {
                    notes.push(Note {
                        stream: c.stream.to_string(),
                        converged: false,
                        lines: 0,
                    });
                }
            }
            trips.push(Trip {
                index: u32::try_from(t.index).expect("few trips"),
                phis,
                body,
                notes,
            });
        }
        Trace { trips }
    }

    fn nodes(
        &self,
        items: &[Item<M>],
        st: &[RStatus<M::Addr>],
        cur: &mut usize,
        reused: bool,
    ) -> Vec<Node> {
        items
            .iter()
            .map(|it| match it {
                Item::Out(e) => Node::Effect(e.to_string()),
                Item::Open(a) => Node::Open(a.to_string()),
                Item::Store(a, v) => Node::Store(a.to_string(), v.version()),
                Item::Call(id) => Node::Call(self.call_node(*id, st, cur, reused)),
                Item::Wrote(a) => Node::Wrote(a.to_string()),
            })
            .collect()
    }

    fn call_node(&self, id: RecId, st: &[RStatus<M::Addr>], cur: &mut usize, reused: bool) -> Call {
        let r = self.record(id);
        let status = if reused {
            Status::Hit
        } else {
            let s = st.get(*cur).cloned().unwrap_or(RStatus::Hit);
            *cur += 1;
            match s {
                RStatus::Hit => Status::Hit,
                RStatus::New => Status::New,
                RStatus::Miss(l) => Status::Miss(l.to_string()),
            }
        };
        let hit = status == Status::Hit;
        Call {
            func: r.func.to_string(),
            args: r.args.clone(),
            result: r.result.version(),
            name: r.name,
            status,
            reads: r.reads.iter().map(|(l, v)| (l.to_string(), *v)).collect(),
            writes: r
                .writes
                .iter()
                .map(|(a, v)| (a.to_string(), version_opt(v.as_ref())))
                .collect(),
            body: self.nodes(&r.items, st, cur, reused || hit),
        }
    }
}

struct Names {
    map: BTreeMap<Version, usize>,
}

impl Names {
    fn name(&mut self, v: Version, defs: &mut Vec<(usize, Version)>) -> String {
        let n = self.map.len();
        let i = *self.map.entry(v).or_insert_with(|| {
            defs.push((n, v));
            n
        });
        format!("%v{i}")
    }
}

fn defs_text(defs: &[(usize, Version)]) -> String {
    let mut s = String::new();
    for (i, v) in defs {
        let _ = write!(s, " %v{i}={v}");
    }
    s
}

fn escape(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn unescape(s: &str) -> Option<String> {
    let s = s.strip_prefix('"')?.strip_suffix('"')?;
    let mut o = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next()? {
                'n' => o.push('\n'),
                c => o.push(c),
            }
        } else {
            o.push(c);
        }
    }
    Some(o)
}

impl Trace {
    /// The canonical text form.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let mut names = Names {
            map: BTreeMap::new(),
        };
        for t in &self.trips {
            let _ = writeln!(out, "trip {} {{", t.index);
            for p in &t.phis {
                let mut d = Vec::new();
                let v = names.name(p.value, &mut d);
                let a = names.name(p.prev_build, &mut d);
                let b = p
                    .prev_trip
                    .map_or_else(|| "undef".to_string(), |x| names.name(x, &mut d));
                let _ = writeln!(
                    out,
                    "  {v} = phi [{a}, {b}] @{} ;{}",
                    p.stream,
                    defs_text(&d)
                );
            }
            for n in &t.body {
                node_text(n, 1, &mut names, &mut out);
            }
            for n in &t.notes {
                let w = if n.converged { "converged" } else { "changed" };
                let _ = writeln!(out, "  {w} @{} lines={}", n.stream, n.lines);
            }
            out.push_str("}\n");
        }
        out
    }

    /// Parse the text form.
    pub fn parse(text: &str) -> Result<Trace, String> {
        let lines: Vec<&str> = text.lines().collect();
        let mut names: BTreeMap<String, Version> = BTreeMap::new();
        let mut trips = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let l = lines[i];
            let idx = l
                .strip_prefix("trip ")
                .and_then(|r| r.strip_suffix(" {"))
                .ok_or_else(|| format!("line {}: expected `trip N {{`", i + 1))?;
            let index = idx
                .parse()
                .map_err(|_| format!("line {}: bad trip", i + 1))?;
            i += 1;
            let mut t = Trip {
                index,
                phis: Vec::new(),
                body: Vec::new(),
                notes: Vec::new(),
            };
            while i < lines.len() && lines[i] != "}" {
                let l = lines[i];
                let s = l.trim_start();
                if s.contains(" = phi [") {
                    let (instr, defs) = split_defs(s);
                    define(defs, &mut names)?;
                    let (v, rest) = instr.split_once(" = phi [").ok_or("bad phi")?;
                    let (ops, stream) = rest.split_once("] @").ok_or("bad phi")?;
                    let (a, b) = ops.split_once(", ").ok_or("bad phi")?;
                    t.phis.push(Phi {
                        stream: stream.to_string(),
                        prev_build: lookup(a, &names)?,
                        prev_trip: if b == "undef" {
                            None
                        } else {
                            Some(lookup(b, &names)?)
                        },
                        value: lookup(v, &names)?,
                    });
                    i += 1;
                } else if let Some(r) = s
                    .strip_prefix("converged @")
                    .or_else(|| s.strip_prefix("changed @"))
                {
                    let (stream, n) = r.split_once(" lines=").ok_or("bad note")?;
                    t.notes.push(Note {
                        stream: stream.to_string(),
                        converged: s.starts_with("converged"),
                        lines: n.parse().map_err(|_| "bad lines")?,
                    });
                    i += 1;
                } else {
                    let n = parse_node(&lines, &mut i, 2, &mut names)?;
                    t.body.push(n);
                }
            }
            i += 1;
            trips.push(t);
        }
        Ok(Trace { trips })
    }
}

fn node_text(n: &Node, depth: usize, names: &mut Names, out: &mut String) {
    let ind = "  ".repeat(depth);
    match n {
        Node::Effect(e) => {
            let _ = writeln!(out, "{ind}effect {}", escape(e));
        }
        Node::Open(a) => {
            let _ = writeln!(out, "{ind}open @{a}");
        }
        Node::Wrote(a) => {
            let _ = writeln!(out, "{ind}wrote @{a}");
        }
        Node::Store(a, v) => {
            let mut d = Vec::new();
            let v = names.name(*v, &mut d);
            let _ = writeln!(out, "{ind}store @{a} {v} ;{}", defs_text(&d));
        }
        Node::Call(c) => {
            let mut d = Vec::new();
            let r = names.name(c.result, &mut d);
            let args: Vec<String> = c.args.iter().map(|a| names.name(*a, &mut d)).collect();
            let st = match &c.status {
                Status::Hit => "hit".to_string(),
                Status::New => "new".to_string(),
                Status::Miss(l) => format!("miss=@{l}"),
            };
            let _ = writeln!(
                out,
                "{ind}{r} = call @{}({}) ; name={} {st}{}",
                c.func,
                args.join(", "),
                c.name,
                defs_text(&d)
            );
            for (l, v) in &c.reads {
                let mut d = Vec::new();
                let v = names.name(*v, &mut d);
                let _ = writeln!(out, "{ind}  read @{l} = {v} ;{}", defs_text(&d));
            }
            for (a, v) in &c.writes {
                let mut d = Vec::new();
                let v = names.name(*v, &mut d);
                let _ = writeln!(out, "{ind}  write @{a} = {v} ;{}", defs_text(&d));
            }
            for b in &c.body {
                node_text(b, depth + 1, names, out);
            }
        }
    }
}

fn split_defs(s: &str) -> (&str, &str) {
    s.split_once(" ;").unwrap_or((s, ""))
}

fn define(defs: &str, names: &mut BTreeMap<String, Version>) -> Result<(), String> {
    for d in defs.split_whitespace() {
        if let Some((n, h)) = d.split_once("=#")
            && n.starts_with("%v")
        {
            let v = u128::from_str_radix(h, 16).map_err(|_| format!("bad hash {h}"))?;
            names.insert(n.to_string(), Version(v));
        }
    }
    Ok(())
}

fn lookup(n: &str, names: &BTreeMap<String, Version>) -> Result<Version, String> {
    names
        .get(n.trim())
        .copied()
        .ok_or_else(|| format!("undefined {n}"))
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start_matches(' ').len()
}

fn parse_node(
    lines: &[&str],
    i: &mut usize,
    depth: usize,
    names: &mut BTreeMap<String, Version>,
) -> Result<Node, String> {
    let l = lines[*i];
    let at = *i + 1;
    if indent(l) != depth {
        return Err(format!("line {at}: bad indentation"));
    }
    let s = &l[depth..];
    *i += 1;
    if let Some(e) = s.strip_prefix("effect ") {
        return Ok(Node::Effect(
            unescape(e).ok_or_else(|| format!("line {at}: bad effect"))?,
        ));
    }
    if let Some(a) = s.strip_prefix("open @") {
        return Ok(Node::Open(a.to_string()));
    }
    if let Some(a) = s.strip_prefix("wrote @") {
        return Ok(Node::Wrote(a.to_string()));
    }
    let (instr, defs) = split_defs(s);
    if let Some(r) = instr.strip_prefix("store @") {
        define(defs, names)?;
        let (a, v) = r
            .split_once(' ')
            .ok_or_else(|| format!("line {at}: bad store"))?;
        return Ok(Node::Store(a.to_string(), lookup(v, names)?));
    }
    let (res, rest) = instr
        .split_once(" = call @")
        .ok_or_else(|| format!("line {at}: expected an instruction"))?;
    let mut ann = defs.split_whitespace();
    let name = ann
        .next()
        .and_then(|n| n.strip_prefix("name=#"))
        .ok_or_else(|| format!("line {at}: no name"))?;
    let name = Version(u128::from_str_radix(name, 16).map_err(|_| format!("line {at}: bad name"))?);
    let st = ann.next().ok_or_else(|| format!("line {at}: no status"))?;
    let status = match st {
        "hit" => Status::Hit,
        "new" => Status::New,
        m => Status::Miss(
            m.strip_prefix("miss=@")
                .ok_or_else(|| format!("line {at}: bad status"))?
                .to_string(),
        ),
    };
    define(defs, names)?;
    let (func, args) = rest
        .split_once('(')
        .ok_or_else(|| format!("line {at}: bad call"))?;
    let args = args
        .strip_suffix(')')
        .ok_or_else(|| format!("line {at}: bad call"))?;
    let args = if args.is_empty() {
        Vec::new()
    } else {
        args.split(", ")
            .map(|a| lookup(a, names))
            .collect::<Result<_, _>>()?
    };
    let mut c = Call {
        func: func.to_string(),
        args,
        result: lookup(res, names)?,
        name,
        status,
        reads: Vec::new(),
        writes: Vec::new(),
        body: Vec::new(),
    };
    while *i < lines.len() && indent(lines[*i]) == depth + 2 && lines[*i] != "}" {
        let s = &lines[*i][depth + 2..];
        let (instr, defs) = split_defs(s);
        if let Some(r) = instr
            .strip_prefix("read @")
            .or_else(|| instr.strip_prefix("write @"))
        {
            define(defs, names)?;
            let (a, v) = r
                .split_once(" = ")
                .ok_or_else(|| format!("line {}: bad read", *i + 1))?;
            let e = (a.to_string(), lookup(v, names)?);
            if instr.starts_with("read") {
                c.reads.push(e);
            } else {
                c.writes.push(e);
            }
            *i += 1;
        } else {
            c.body.push(parse_node(lines, i, depth + 2, names)?);
        }
    }
    Ok(Node::Call(c))
}
