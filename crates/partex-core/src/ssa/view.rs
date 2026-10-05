//! The build printed as a program (DESIGN 4.3 item 7): `PhiTeX`'s
//! document as an SSA program, made of the recorder's steps, so exact.
//!
//! ```text
//! %0 = format                                  ; step 0: the job's start, (preloaded format=plain 2026.10.2), 631946 definitions: incr:1
//! %1 = file incr
//! %71 = window(incr:15-18=%1, \catcode92=%0, ..., \section=%66, \words=%64, align_state=%68, ...; page[56], ..., list.list) ; step 68: incr:15-18 "Opening Lorem ipsum dolor sit amet,"
//! ```
//!
//! - The job's start (the step that loaded the format and read the
//!   first command) is the constant `%0`: its definitions are the
//!   format's, and an address no step defined (the engine's initial
//!   state, a lookup, a line number read) is `%0`'s too. A file read is
//!   a constant `file NAME`.
//! - Then a value per live step of the fold (a *window*), in program
//!   order. Its operands are its *imports*: each address it read from
//!   outside itself (the step's reads, `Fold::steps`' `reads`), by name,
//!   with the value of the window whose definition reached it
//!   (`Fold::reaching`), or `%0`; the source it read are imports of the
//!   file's constant, by runs of lines (`file:first-last`: the line it
//!   began on if the step before left some of it, then the lines it
//!   read), and a file it loaded whole is imported by its name.
//! - Its names are its *exports*: the addresses its records wrote, by
//!   name ([`slot_name`]).
//! - Its comment (`shows`): the step's id (and the run, after a rebuild
//!   ran it again), its runs of lines, the pages it shipped, and an
//!   excerpt: the page it shipped, else the characters of the nodes it
//!   put on the page, else what it added to its list, else its source.
//!
//! An address's name: a control sequence's meaning by its name in the
//! hash (`\section`), the registers and codes as TeX writes them
//! (`\count12`, `\catcode92`, `\textfont1`), a parameter without its
//! escape (`baselineskip`, `everypar`: `\baselineskip` is the control
//! sequence), and the other families by a prefix and a field
//! (`page.contents`, `list.mode`, `font:cmr10.fontdimen`, `str_ptr`).
//!
//! Nothing here runs while the build does: the view reads the fold, the
//! records and the engine's tables afterwards, so with no view asked for
//! a build costs nothing more.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write as _;

use partex_ssa::fold::StepId;
use partex_ssa::runtime::{Item, RecId};
use partex_ssa::{Runtime, Trace};
use phitex_ir::{Def, Operand, Program, ValueId};

use super::{Fam, Func, SValue, Slot, SsaTracker, TexSsa, line_bounds};
use crate::host::Host;
use crate::tex::Tex;

/// How many characters of text a window's comment shows.
const EXCERPT: usize = 40;

/// The build `tex` holds as a program: the job's start as a constant
/// (the format: what no window defined, or the start defined), one per
/// file the windows read, then a window per live step of the fold after
/// the start, in program order. It passes [`Program::check`], and its
/// text form round-trips.
#[must_use]
pub fn view<H: Host>(tex: &Tex<H, SsaTracker>) -> Program {
    let rec = tex.tracker.rec.borrow();
    let st = &rec.st;
    let rt = &rec.rt;
    let fold = &rt.fold;
    let mut names = Names::new(tex);
    let mut prog = Program::default();
    // (the lines each live step's last run read, by step: each file's
    // runs of lines, numbered)
    let mut lines: BTreeMap<StepId, Vec<(u32, u32, u32)>> = BTreeMap::new();
    let mut numbering: BTreeMap<usize, LineIndex> = BTreeMap::new();
    let mut number = |bytes: &[u8], at: usize| {
        numbering
            .entry(bytes.as_ptr() as usize)
            .or_insert_with(|| LineIndex::of(bytes))
            .line(at)
    };
    for (name, bytes, from, to, step, run) in st.steps.line_runs() {
        let live = fold
            .steps
            .get(step as usize)
            .is_some_and(|s| s.live && s.run == run);
        if live && to > from {
            let span = (name, number(bytes, from), number(bytes, to - 1));
            lines.entry(step).or_default().push(span);
        }
    }
    // (each step after the first begins where the one before it left the
    // input: the line it reads the rest of, if any is left)
    let mut rests: BTreeMap<StepId, Vec<u8>> = BTreeMap::new();
    for w in fold.order.windows(2) {
        let Some((name, bytes, from, rest)) = st.steps.ended_at(w[0]) else {
            continue;
        };
        let line = number(&bytes, from);
        lines.entry(w[1]).or_default().insert(0, (name, line, line));
        rests.insert(w[1], rest);
    }
    for v in lines.values_mut() {
        // (by file, in the order loaded, and line: an edit's data is
        // loaded again, and its lines are where they were)
        v.sort_unstable();
        *v = runs(v);
    }
    let first = fold
        .order
        .first()
        .copied()
        .filter(|&s| starts(rt, &fold.steps[s as usize].recs));
    // (the job's start, which loaded the format: a constant)
    let ident = tex.str_bytes(usize::try_from(tex.format_ident).unwrap_or(0));
    let mut shows = printable(trim(ident));
    if let Some(s) = first {
        let spans = spans(st, lines.get(&s));
        // (each record's writes are each address once: the start's are
        // the format's, too many to gather)
        let w: usize = fold.steps[s as usize]
            .recs
            .iter()
            .map(|&r| rt.writes(r).len())
            .sum();
        shows = format!("step {s}: the job's start, {shows}, {w} definitions:{spans}");
    }
    let format = prog.define(Def::Const(String::from("format")), shows, 0);
    // (a constant per file: the ones read by lines, and the ones loaded)
    let mut files: BTreeMap<u32, ValueId> = BTreeMap::new();
    let mut loaded: BTreeSet<u32> = lines.values().flatten().map(|l| l.0).collect();
    for &s in &fold.order {
        for a in fold.reads_of(s) {
            if a.0 == Fam::Load {
                loaded.insert(u32::try_from(a.1).unwrap_or(u32::MAX));
            }
        }
    }
    for id in loaded {
        let v = prog.define(
            Def::Const(format!("file {}", file_name(st, id))),
            String::new(),
            0,
        );
        files.insert(id, v);
    }
    let mut value: Vec<Option<ValueId>> = alloc::vec![None; fold.steps.len()];
    if let Some(s) = first {
        value[s as usize] = Some(format);
    }
    for &s in fold.order.iter().skip(usize::from(first.is_some())) {
        let step = &fold.steps[s as usize];
        let mut operands = Vec::new();
        for &(file, a, b) in lines.get(&s).into_iter().flatten() {
            operands.push(Operand::Named(span(st, file, a, b), files[&file]));
        }
        for a in fold.reads_of(s) {
            match a.0 {
                // (the lines, above)
                Fam::Source => {}
                Fam::Load => {
                    let id = u32::try_from(a.1).unwrap_or(u32::MAX);
                    operands.push(Operand::Named(file_name(st, id), files[&id]));
                }
                _ => {
                    // (a definition reaching it is a live step's before it,
                    // whose value is made)
                    let from = fold
                        .reaching(a, step.key)
                        .and_then(|d| value[d.step as usize])
                        .unwrap_or(format);
                    operands.push(Operand::Named(names.slot(st, *a), from));
                }
            }
        }
        let written = writes(rt, &step.recs);
        let defines: Vec<String> = written.iter().map(|a| names.slot(st, *a)).collect();
        let mut shows = format!("step {s}");
        if step.run > 1 {
            let _ = write!(shows, " (run {})", step.run);
        }
        shows.push(':');
        shows.push_str(&spans(st, lines.get(&s)));
        let ships = shipped(rt, &step.recs);
        shows.push_str(&ships);
        // (a window that ships a page: the page's text, the nodes it read
        // of it; else the text it set)
        let mut text = if ships.is_empty() {
            set_text(rt, step.key, &step.recs)
        } else {
            let reads: Vec<Slot> = fold.reads_of(s).copied().collect();
            page_text(rt, step.key, &reads)
        };
        if text.is_empty() {
            // (else the source it read: the rest of the line it began on,
            // or the first line it read)
            text = match (rests.get(&s), lines.get(&s).and_then(|v| v.first())) {
                (Some(rest), _) => printable(rest.strip_suffix(b"\r").unwrap_or(rest)),
                (None, Some(&(file, a, _))) => source_line(st, file, a),
                (None, None) => String::new(),
            };
        }
        shows.push_str(&excerpt(&text));
        let v = prog.define(
            Def::Op {
                op: String::from("window"),
                operands,
                defines,
            },
            shows,
            0,
        );
        value[s as usize] = Some(v);
    }
    prog
}

/// Whether records `recs` are the job's start's (`Func::Start`).
fn starts(rt: &Runtime<TexSsa>, recs: &[RecId]) -> bool {
    recs.iter().any(|&r| rt.record(r).func == Func::Start)
}

/// A run of lines' name: `file:line`, or `file:first-last`.
fn span(st: &super::RecState, file: u32, a: u32, b: u32) -> String {
    if a == b {
        format!("{}:{a}", file_name(st, file))
    } else {
        format!("{}:{a}-{b}", file_name(st, file))
    }
}

/// The runs of lines `v`, each after a space.
fn spans(st: &super::RecState, v: Option<&Vec<(u32, u32, u32)>>) -> String {
    let mut out = String::new();
    for &(file, a, b) in v.into_iter().flatten() {
        out.push(' ');
        out.push_str(&span(st, file, a, b));
    }
    out
}

/// A text's first characters, its spaces made one, quoted, after a space
/// (nothing for no text).
fn excerpt(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return String::new();
    }
    let mut t: String = text.chars().take(EXCERPT).collect();
    if text.chars().count() > EXCERPT {
        t.push('…');
    }
    format!(" {}", phitex_ir::quote(&t))
}

/// Step `id`'s calls, with their reads, writes and effects, in the
/// trace's text form (`Trace::to_text`); `None` if the step is not live.
#[must_use]
pub fn step_trace<H: Host>(tex: &Tex<H, SsaTracker>, id: StepId) -> Option<String> {
    let rec = tex.tracker.rec.borrow();
    let st = rec.rt.fold.steps.get(id as usize).filter(|s| s.live)?;
    let mut names = Names::new(tex);
    // (the addresses by the view's names, quoted where they would not be
    // one token)
    let mut name = |a: &Slot| phitex_ir::name_text(&names.slot(&rec.st, *a));
    let t: Trace = rec.rt.trace_of(&st.recs, &mut name);
    Some(t.to_text())
}

/// The build's steps as a dependency graph (`PARTEX_SSA_DAG`), to measure
/// how much of it could run at once: per live step, in program order, a
/// line `S id run commands` (the commands its last run ran), then a line
/// per address it read from outside it, `R from at name` (`from`: the step
/// whose definition reached the read, `-` for none: the format's or the
/// engine's), `L from at name` for a file it loaded, and a line per
/// address it defined, `W version at name` (the low 64 bits of the version
/// of its last write there, in hex; `-` if the job ended in it). `at` is
/// how many commands into the step's run the read was made or the address
/// last written, with the steps timed (`SsaTracker::set_timed`), else
/// `-`. Fields are separated by tabs, and a name is the view's.
#[must_use]
pub fn dag<H: Host>(tex: &Tex<H, SsaTracker>) -> String {
    let rec = tex.tracker.rec.borrow();
    let (st, rt) = (&rec.st, &rec.rt);
    let fold = &rt.fold;
    let mut names = Names::new(tex);
    let mut out = String::new();
    let at = |t: Option<u64>| t.map_or_else(|| String::from("-"), |t| t.to_string());
    for &s in &fold.order {
        let step = &fold.steps[s as usize];
        let commands = st.step_commands.get(s as usize).copied().unwrap_or(0);
        let _ = writeln!(out, "S\t{s}\t{}\t{commands}", step.run);
        // (the times of its last run, if timed and as many as its reads)
        let times = rt
            .step_times(s)
            .filter(|t| t.reads.len() == step.reads.len());
        for (i, a) in fold.reads_of(s).enumerate() {
            let kind = if a.0 == Fam::Load { 'L' } else { 'R' };
            let from = fold
                .reaching(a, step.key)
                .map_or_else(|| String::from("-"), |d| d.step.to_string());
            let t = at(times.map(|t| t.reads[i].saturating_sub(t.began)));
            let _ = writeln!(out, "{kind}\t{from}\t{t}\t{}", names.slot(st, *a));
        }
        let wrote: BTreeMap<Slot, u64> = times
            .map(|t| {
                t.wrote
                    .iter()
                    .map(|(a, w)| (*a, w.saturating_sub(t.began)))
                    .collect()
            })
            .unwrap_or_default();
        // (each address once, with its last write's version: the step's
        // definition of it)
        let mut defs: Vec<(Slot, Option<u128>)> = Vec::new();
        let mut ix: BTreeMap<Slot, usize> = BTreeMap::new();
        for &r in &step.recs {
            for (a, v) in rt.writes(r) {
                let v = v.as_ref().map(|v| v.0.0 & u128::from(u64::MAX));
                if let Some(&i) = ix.get(a) {
                    defs[i].1 = v;
                } else {
                    ix.insert(*a, defs.len());
                    defs.push((*a, v));
                }
            }
        }
        for (a, v) in defs {
            let v = v.map_or_else(|| String::from("-"), |v| format!("{v:016x}"));
            let t = at(wrote.get(&a).copied());
            let _ = writeln!(out, "W\t{v}\t{t}\t{}", names.slot(st, a));
        }
    }
    out
}

/// The addresses records `recs` wrote, each once, in order (a later
/// record's write of an address is the same definition).
fn writes(rt: &Runtime<TexSsa>, recs: &[RecId]) -> Vec<Slot> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for &r in recs {
        for (a, _) in rt.writes(r) {
            if seen.insert(*a) {
                out.push(*a);
            }
        }
    }
    out
}

/// The pages records `recs` shipped, after a space: ` ships [3]`, each
/// by the counts the log shows it with (§638), or ` ships a page`.
fn shipped(rt: &Runtime<TexSsa>, recs: &[RecId]) -> String {
    /// The ships under `r`, and what each wrote to the log.
    fn walk(
        rt: &Runtime<TexSsa>,
        r: RecId,
        ship: bool,
        logs: &mut Vec<Vec<u8>>,
        flowed: &mut Vec<u8>,
    ) {
        let rec = rt.record(r);
        let ship = ship || rec.func == Func::ShipOut;
        if rec.func == Func::ShipOut {
            logs.push(Vec::new());
        }
        for it in &rec.items {
            match it {
                Item::Call(c) => walk(rt, *c, ship, logs, flowed),
                // (with the columns the link's, the log's text is in the
                // step's flow ops: `effects/flow.rs`)
                Item::Out(super::Effect::Step(fx)) => {
                    for e in fx.1.iter() {
                        if let crate::effects::Effect::Flow { ops, .. } = e {
                            flowed.extend(crate::effects::flow::log_text(ops));
                        }
                    }
                }
                Item::Out(super::Effect::Bytes(crate::track::Output::Log, b)) if ship => {
                    if let Some(l) = logs.last_mut() {
                        l.extend_from_slice(b);
                    }
                }
                _ => {}
            }
        }
    }
    let mut logs = Vec::new();
    let mut flowed = Vec::new();
    for &r in recs {
        walk(rt, r, false, &mut logs, &mut flowed);
    }
    // (each ship's counts: in its own log bytes, else the `[`s of the
    // step's flowed text in order)
    let mut brackets = flowed
        .iter()
        .enumerate()
        .filter(|&(_, &c)| c == b'[')
        .map(|(i, _)| flowed[i..].to_vec());
    for log in &mut logs {
        if !log.contains(&b'[')
            && let Some(f) = brackets.next()
        {
            *log = f;
        }
    }
    let mut out = String::new();
    for log in &logs {
        // (`[` and the counts, §638)
        let label = log.iter().position(|&c| c == b'[').map(|i| {
            let n = log[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit() || matches!(c, b'.' | b'-'))
                .count();
            printable(&log[i + 1..=i + n])
        });
        match label {
            Some(l) if !l.is_empty() => {
                let _ = write!(out, " [{l}]");
            }
            _ => out.push_str(" [?]"),
        }
    }
    if out.is_empty() {
        out
    } else {
        format!(" ships{out}")
    }
}

/// The text the step at `key` with records `recs` set: the characters of
/// the nodes it put on the page, in order; else what it added to the
/// list of the level it left (the list's text past the text of that
/// level's list that reached it, or all of it).
fn set_text(rt: &Runtime<TexSsa>, key: u64, recs: &[RecId]) -> String {
    use crate::track::list;
    use partex_engine::node::Node;
    let nest = Slot(Fam::List, i64::from(list::COUNT));
    let mut page: BTreeMap<i64, &Node> = BTreeMap::new();
    let mut lists: BTreeMap<Slot, &partex_engine::nodelist::NodeList> = BTreeMap::new();
    let mut depth = None;
    for &r in recs {
        for (a, v) in rt.writes(r) {
            match (a.0, v.as_ref().and_then(|v| v.1.as_deref())) {
                (Fam::PageNode, Some(SValue::Field(f))) => {
                    if let Some(n) = f.get::<Node>() {
                        page.insert(a.1, n);
                    }
                }
                (Fam::List, Some(SValue::Nodes(l))) => {
                    lists.insert(*a, l);
                }
                (Fam::List, Some(SValue::Nest(n))) => depth = Some(n.len()),
                _ => {}
            }
        }
    }
    let mut out = String::new();
    for n in page.values() {
        node_text(rt, n, &mut out);
        if out.chars().count() > EXCERPT {
            break;
        }
    }
    if !out.trim().is_empty() {
        return out;
    }
    let reaching = |a: &Slot| {
        rt.fold
            .reaching(a, key)
            .and_then(|d| rt.writes(d.rec).get(d.ix as usize)?.1.clone())
    };
    // (the level it left: the nest as it left it, or as it reached it)
    let depth = depth
        .or_else(|| match reaching(&nest)?.1.as_deref() {
            Some(SValue::Nest(n)) => Some(n.len()),
            _ => None,
        })
        .unwrap_or(0);
    let list = Slot(Fam::List, i64::from(list::slot(depth, list::LIST)));
    let Some(left) = lists.get(&list) else {
        return String::new();
    };
    let text = |l: &partex_engine::nodelist::NodeList| {
        let mut t = String::new();
        for n in l.iter() {
            node_text(rt, n, &mut t);
        }
        t
    };
    let now = text(left);
    let before = reaching(&list)
        .and_then(|v| match v.1.as_deref() {
            Some(SValue::Nodes(l)) => Some(text(l)),
            _ => None,
        })
        .unwrap_or_default();
    now.strip_prefix(&before).unwrap_or(&now).to_string()
}

/// The text of the page's nodes the step at `key` read (`reads`), in
/// order, as the definitions reaching it hold them: the page a fire
/// shipped.
fn page_text(rt: &Runtime<TexSsa>, key: u64, reads: &[Slot]) -> String {
    let mut nodes: Vec<i64> = reads
        .iter()
        .filter(|a| a.0 == Fam::PageNode)
        .map(|a| a.1)
        .collect();
    nodes.sort_unstable();
    let mut out = String::new();
    for k in nodes {
        let v = rt
            .fold
            .reaching(&Slot(Fam::PageNode, k), key)
            .and_then(|d| rt.writes(d.rec).get(d.ix as usize)?.1.clone());
        if let Some(SValue::Field(f)) = v.as_ref().and_then(|v| v.1.as_deref())
            && let Some(n) = f.get::<partex_engine::node::Node>()
        {
            node_text(rt, n, &mut out);
        }
        if out.chars().count() > EXCERPT {
            break;
        }
    }
    out
}

/// The characters of node `n` (and of the nodes in it), glue as a space;
/// a sealed line's (`seal.rs`) as the last definition of its slot holds
/// them.
fn node_text(rt: &Runtime<TexSsa>, n: &partex_engine::node::Node, out: &mut String) {
    use partex_engine::node::Node;
    match n {
        Node::Glyphs(g) => out.extend(g.chars().iter().map(|&c| text_char(c))),
        Node::Ligature(l) => out.extend(l.original.iter().map(|&c| text_char(c))),
        Node::Disc(d) => {
            for m in &d.replace {
                node_text(rt, m, out);
            }
        }
        Node::Box(b) => {
            let sealed = b.seal.and_then(|k| {
                #[allow(clippy::cast_possible_truncation, reason = "a key's low 64 bits")]
                let a = Slot(Fam::Sealed, (k as u64).cast_signed());
                let d = rt.fold.latest(&a)?;
                match rt
                    .writes(d.rec)
                    .get(d.ix as usize)?
                    .1
                    .as_ref()?
                    .1
                    .as_deref()
                {
                    Some(SValue::Sealed(Some(s))) => Some(s.clone()),
                    _ => None,
                }
            });
            let list = sealed.as_ref().map_or(&b.list, |s| &s.list);
            for m in list {
                node_text(rt, m, out);
                if out.chars().count() > EXCERPT {
                    return;
                }
            }
            // (the lines of a paragraph apart)
            out.push(' ');
        }
        Node::Glue { .. } => out.push(' '),
        _ => {}
    }
}

/// A character of a font's text as shown: ASCII as it is, the rest
/// as `·`.
fn text_char(c: u8) -> char {
    if (0x20..0x7f).contains(&c) {
        char::from(c)
    } else {
        '·'
    }
}

/// Line `line` of the file whose load id is `file`, as it is now.
fn source_line(st: &super::RecState, file: u32, line: u32) -> String {
    // (its newest data: an edit's lines are moved to the data it made)
    let Some(bytes) = st
        .steps
        .line_runs()
        .filter(|l| l.0 == file)
        .last()
        .map(|l| l.1)
    else {
        return String::new();
    };
    let mut from = 0;
    for _ in 1..line {
        if from >= bytes.len() {
            return String::new();
        }
        from = line_bounds(bytes, from).1;
    }
    let end = line_bounds(bytes, from).0;
    printable(&bytes[from..end])
}

/// Runs of consecutive lines, by file, from spans sorted by file and
/// line: `(file, first, last)`, each line once.
fn runs(v: &[(u32, u32, u32)]) -> Vec<(u32, u32, u32)> {
    let mut out: Vec<(u32, u32, u32)> = Vec::new();
    for &(f, a, b) in v {
        match out.last_mut() {
            Some(l) if l.0 == f && a <= l.2 + 1 && b >= l.1 => {
                l.1 = l.1.min(a);
                l.2 = l.2.max(b);
            }
            _ => out.push((f, a, b)),
        }
    }
    out
}

/// Where each line of a data begins, to number lines by their offsets.
struct LineIndex(Vec<usize>);

impl LineIndex {
    fn of(data: &[u8]) -> LineIndex {
        let mut starts = alloc::vec![0];
        let mut at = 0;
        while at < data.len() {
            at = line_bounds(data, at).1;
            if at < data.len() {
                starts.push(at);
            }
        }
        LineIndex(starts)
    }

    /// The number (from 1) of the line byte `at` is on.
    fn line(&self, at: usize) -> u32 {
        u32::try_from(self.0.partition_point(|&s| s <= at)).unwrap_or(u32::MAX)
    }
}

/// The name of the file with load id `id`, as it was looked up.
fn file_name(st: &super::RecState, id: u32) -> String {
    st.loads
        .get(id as usize)
        .map_or_else(|| format!("file{id}"), |l| printable(&l.0))
}

/// `b` without its leading and trailing spaces.
fn trim(b: &[u8]) -> &[u8] {
    let s = b.iter().position(|&c| c != b' ').unwrap_or(b.len());
    let e = b.iter().rposition(|&c| c != b' ').map_or(s, |e| e + 1);
    &b[s..e]
}

/// Bytes as TeX prints them (§49): a character below 32 or from 127 up
/// in `^^` notation.
fn printable(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len());
    for &c in b {
        match c {
            0..=31 => {
                s.push_str("^^");
                s.push(char::from(c + 64));
            }
            127 => s.push_str("^^?"),
            128.. => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                s.push_str("^^");
                s.push(char::from(HEX[usize::from(c >> 4)]));
                s.push(char::from(HEX[usize::from(c & 15)]));
            }
            _ => s.push(char::from(c)),
        }
    }
    s
}

/// §103: a dimension in points as TeX prints it.
fn scaled(mut s: i32) -> String {
    let mut out = String::new();
    if s < 0 {
        out.push('-');
        s = -s;
    }
    out.push_str(&(s / 65536).to_string());
    out.push('.');
    let mut s = 10 * (s % 65536) + 5;
    let mut delta = 10;
    loop {
        if delta > 65536 {
            s += 0o100000 - 50000; // round the last digit
        }
        out.push(char::from(b'0' + u8::try_from(s / 65536).unwrap_or(0)));
        s = 10 * (s % 65536);
        delta *= 10;
        if s <= delta {
            break;
        }
    }
    out.push_str("pt");
    out
}

/// Addresses' names, with what the engine's tables say of them.
/// Slot `a`'s name as the view gives it (a control sequence's, a
/// register's, a font's field), for the rebuild's trace.
pub(super) fn trace_name<H: Host>(
    tex: &Tex<H, SsaTracker>,
    st: &super::RecState,
    a: Slot,
) -> String {
    slot_name(&mut Names::new(tex), st, a)
}

struct Names<'a, H: Host> {
    tex: &'a Tex<H, SsaTracker>,
    /// The fonts' names, by slot.
    fonts: BTreeMap<usize, String>,
    /// The names made, by slot.
    made: BTreeMap<Slot, String>,
}

impl<'a, H: Host> Names<'a, H> {
    fn new(tex: &'a Tex<H, SsaTracker>) -> Self {
        Names {
            tex,
            fonts: BTreeMap::new(),
            made: BTreeMap::new(),
        }
    }

    /// Slot `a`'s name.
    fn slot(&mut self, st: &super::RecState, a: Slot) -> String {
        if let Some(n) = self.made.get(&a) {
            return n.clone();
        }
        let n = slot_name(self, st, a);
        self.made.insert(a, n.clone());
        n
    }

    /// Control sequence `p`'s name, `\` and its characters, as TeX prints
    /// it (§262–§263); one in the frozen part of the hash, or a font's
    /// identifier, with a prefix (its name is another's too).
    fn cs(&self, p: i32) -> String {
        use crate::web::{
            ACTIVE_BASE, EQTB_SIZE, FONT_ID_BASE, FROZEN_CONTROL_SEQUENCE, FROZEN_NULL_FONT,
            HASH_BASE, NULL_CS, PRIM_EQTB_BASE, SINGLE_BASE, UNDEFINED_CONTROL_SEQUENCE,
        };
        if (ACTIVE_BASE..SINGLE_BASE).contains(&p) {
            return format!("active:{}", printable(&[byte(p - ACTIVE_BASE)]));
        }
        if (SINGLE_BASE..NULL_CS).contains(&p) {
            return format!("\\{}", printable(&[byte(p - SINGLE_BASE)]));
        }
        if p == NULL_CS {
            return String::from("\\csname\\endcsname");
        }
        if p == UNDEFINED_CONTROL_SEQUENCE {
            return String::from("undefined");
        }
        let text = self.tex.slot_name(p);
        let name = if text.is_empty() {
            format!("hash:{p}")
        } else {
            format!("\\{}", printable(&text))
        };
        let in_hash = (HASH_BASE..FROZEN_CONTROL_SEQUENCE).contains(&p)
            || (p > EQTB_SIZE && p <= self.tex.eqtb_top);
        if in_hash {
            name
        } else if (FROZEN_NULL_FONT..UNDEFINED_CONTROL_SEQUENCE).contains(&p) {
            format!("fontid:{}:{name}", p - FONT_ID_BASE)
        } else if p >= PRIM_EQTB_BASE && p < FROZEN_NULL_FONT {
            format!("primitive:{name}")
        } else {
            format!("frozen:{name}")
        }
    }

    /// Font slot `f`'s name: its file's, and its size if it is not the
    /// design size.
    fn font(&mut self, f: usize) -> String {
        if let Some(n) = self.fonts.get(&f) {
            return n.clone();
        }
        let tex = self.tex;
        let name = tex.fonts.name.get(f).copied().unwrap_or(0);
        let mut n = printable(&tex.string_bytes(name));
        if n.is_empty() {
            n = format!("font{f}");
        }
        if let Some(m) = tex.fonts.metrics.get(f)
            && m.size != m.design_size
        {
            n.push('@');
            n.push_str(&scaled(m.size));
        }
        self.fonts.insert(f, n.clone());
        n
    }
}

/// `x` as a byte (a character code).
fn byte(x: i32) -> u8 {
    u8::try_from(x).unwrap_or(b'?')
}

/// Slot `a`'s name (the module's doc).
fn slot_name<H: Host>(names: &mut Names<'_, H>, st: &super::RecState, a: Slot) -> String {
    let i = a.1;
    let ix = usize::try_from(i).unwrap_or(usize::MAX);
    let interned = |v: &[Vec<u8>]| v.get(ix).map_or_else(|| format!("{i}"), |b| printable(b));
    match a.0 {
        Fam::Eqtb => eqtb_name(names, i32::try_from(i).unwrap_or(0)),
        Fam::Hash => format!("text:{}", names.cs(i32::try_from(i).unwrap_or(0))),
        Fam::HashNext => format!("next:{}", names.cs(i32::try_from(i).unwrap_or(0))),
        Fam::Font => {
            use crate::track::font;
            let fields = font::FIELDS as usize;
            let (f, k) = (ix / fields, u32::try_from(ix % fields).unwrap_or(0));
            let field = match k {
                font::METRICS => String::from("metrics"),
                font::PARAMS => String::from("fontdimen"),
                font::HYPHEN_CHAR => String::from("hyphenchar"),
                font::SKEW_CHAR => String::from("skewchar"),
                font::EXPAND => String::from("expand"),
                font::GLUE => String::from("glue"),
                font::NUMBER => String::from("number"),
                k if k >= font::CODES => format!("code{}", k - font::CODES),
                k => format!("field{k}"),
            };
            format!("font:{}.{field}", names.font(f))
        }
        Fam::FontTable => String::from("fonts"),
        Fam::Read => format!("read:{i}"),
        Fam::Out if i == i64::from(crate::streams::LOG) => String::from("write:log"),
        Fam::Out => format!("write:{i}"),
        Fam::Random => String::from("random"),
        Fam::Str => format!("strings:{i:#x}"),
        Fam::Source | Fam::Line | Fam::Sealed | Fam::PdfObj | Fam::PdfName | Fam::PdfNum => {
            format!("{a}")
        }
        Fam::Class => format!("class:{}", slot_name(names, st, Slot(Fam::Eqtb, i))),
        Fam::Name => {
            let n = interned(&st.names);
            format!("lookup:\\{n}")
        }
        Fam::Load => file_name(st, u32::try_from(i).unwrap_or(u32::MAX)),
        Fam::Search => format!("search:{}", interned(&st.searches)),
        Fam::HyphWord => format!("hyph:{}", interned(&st.words)),
        Fam::Pool => format!("string:{i}"),
        Fam::Alloc => scalar_name(i),
        Fam::Unknown => format!("unknown:{i}"),
        Fam::List => list_name(i),
        Fam::Save => {
            use crate::track::save;
            match u32::try_from(i).unwrap_or(u32::MAX) {
                save::SAVE_PTR => String::from("save.ptr"),
                save::CUR_LEVEL => String::from("save.level"),
                save::CUR_GROUP => String::from("save.group"),
                save::CUR_BOUNDARY => String::from("save.boundary"),
                save::XCHAIN => String::from("save.xchain"),
                k if k >= save::XENTRY => format!("save.xchain[{}]", k - save::XENTRY),
                k if k >= save::ENTRY => format!("save[{}]", k - save::ENTRY),
                k => format!("save.{k}"),
            }
        }
        Fam::Hyph => match u32::try_from(i).unwrap_or(u32::MAX) {
            crate::track::hyph::PATTERNS => String::from("hyph.patterns"),
            crate::track::hyph::EXCEPTIONS => String::from("hyph.exceptions"),
            k => format!("hyph.{k}"),
        },
        Fam::Glyphs => format!("glyphs:{i}"),
        Fam::Pdf => pdf_name(i),
        Fam::Dvi => match i {
            0 => String::from("dvi.file"),
            1 => String::from("dvi.fonts"),
            2 => String::from("dvi.totals"),
            3 => String::from("dvi.writer"),
            k => format!("dvi.{k}"),
        },
        Fam::Page => page_name(i),
        Fam::Cond => String::from("cond"),
        Fam::Mark => {
            let (c, t) = (i / 5, i % 5);
            let kind = ["top", "first", "bot", "splitfirst", "splitbot"]
                .get(usize::try_from(t).unwrap_or(9))
                .copied()
                .unwrap_or("?");
            if c == 0 {
                format!("{kind}mark")
            } else {
                format!("{kind}marks{c}")
            }
        }
        Fam::PageNode => format!("page[{i}]"),
    }
}

/// Eqtb entry `p`'s name (§222–§251): a control sequence's, a register's,
/// a parameter's, a code's.
fn eqtb_name<H: Host>(names: &Names<'_, H>, p: i32) -> String {
    use crate::equiv::{dimen_param_name, int_param_name};
    use crate::web::*;
    // (a parameter is named without its escape: `\\hsize` is the
    // control sequence, whose meaning is another address)
    if p >= crate::xregs::EXT_BASE {
        let (kind, r) = crate::xregs::ext_reg(p);
        let k = match kind {
            INT_VAL => "count",
            DIMEN_VAL => "dimen",
            GLUE_VAL => "skip",
            MU_VAL => "muskip",
            BOX_VAL => "box",
            _ => "toks",
        };
        return format!("\\{k}{r}");
    }
    if p < GLUE_BASE || p > EQTB_SIZE {
        return names.cs(p);
    }
    if p < SKIP_BASE {
        return skip_param_name(p - GLUE_BASE)
            .map_or_else(|| format!("gluepar{}", p - GLUE_BASE), printable);
    }
    let regions: [(i32, &str); 15] = [
        (SKIP_BASE, "skip"),
        (MU_SKIP_BASE, "muskip"),
        (TOKS_BASE, "toks"),
        (BOX_BASE, "box"),
        (MATH_FONT_BASE, "textfont"),
        (MATH_FONT_BASE + 16, "scriptfont"),
        (MATH_FONT_BASE + 32, "scriptscriptfont"),
        (CAT_CODE_BASE, "catcode"),
        (LC_CODE_BASE, "lccode"),
        (UC_CODE_BASE, "uccode"),
        (SF_CODE_BASE, "sfcode"),
        (MATH_CODE_BASE, "mathcode"),
        (CHAR_SUB_CODE_BASE, "charsubdef"),
        (COUNT_BASE, "count"),
        (DEL_CODE_BASE, "delcode"),
    ];
    let numbered = |lo: i32, hi: i32| (lo..hi).contains(&p);
    let ends = [
        MU_SKIP_BASE,
        LOCAL_BASE,
        TOKS_BASE + 256,
        BOX_BASE + 256,
        MATH_FONT_BASE + 16,
        MATH_FONT_BASE + 32,
        MATH_FONT_BASE + 48,
        LC_CODE_BASE,
        UC_CODE_BASE,
        SF_CODE_BASE,
        MATH_CODE_BASE,
        CHAR_SUB_CODE_BASE,
        INT_BASE,
        DEL_CODE_BASE,
        DIMEN_BASE,
    ];
    for ((lo, name), hi) in regions.iter().zip(ends) {
        if numbered(*lo, hi) {
            return format!("\\{name}{}", p - lo);
        }
    }
    if (SCALED_BASE..=EQTB_SIZE).contains(&p) {
        return format!("\\dimen{}", p - SCALED_BASE);
    }
    if (INT_BASE..COUNT_BASE).contains(&p) {
        return int_param_name(p - INT_BASE)
            .map_or_else(|| format!("intpar{}", p - INT_BASE), printable);
    }
    if (DIMEN_BASE..SCALED_BASE).contains(&p) {
        return dimen_param_name(p - DIMEN_BASE)
            .map_or_else(|| format!("dimenpar{}", p - DIMEN_BASE), printable);
    }
    let local: &[u8] = match p {
        PAR_SHAPE_LOC => b"parshape",
        OUTPUT_ROUTINE_LOC => b"output",
        EVERY_PAR_LOC => b"everypar",
        EVERY_MATH_LOC => b"everymath",
        EVERY_DISPLAY_LOC => b"everydisplay",
        EVERY_HBOX_LOC => b"everyhbox",
        EVERY_VBOX_LOC => b"everyvbox",
        EVERY_JOB_LOC => b"everyjob",
        EVERY_CR_LOC => b"everycr",
        ERR_HELP_LOC => b"errhelp",
        PDF_PAGES_ATTR_LOC => b"pdfpagesattr",
        PDF_PAGE_ATTR_LOC => b"pdfpageattr",
        PDF_PAGE_RESOURCES_LOC => b"pdfpageresources",
        PDF_PK_MODE_LOC => b"pdfpkmode",
        EVERY_EOF_LOC => b"everyeof",
        INTER_LINE_PENALTIES_LOC => b"interlinepenalties",
        CLUB_PENALTIES_LOC => b"clubpenalties",
        WIDOW_PENALTIES_LOC => b"widowpenalties",
        DISPLAY_WIDOW_PENALTIES_LOC => b"displaywidowpenalties",
        CUR_FONT_LOC => b"current_font",
        XORD_CODE_BASE => b"xordcode",
        XCHR_CODE_BASE => b"xchrcode",
        XPRN_CODE_BASE => b"xprncode",
        _ => return format!("eqtb:{p}"),
    };
    printable(local)
}

/// §225: a glue parameter's name.
fn skip_param_name(n: i32) -> Option<&'static [u8]> {
    use crate::web::*;
    Some(match n {
        LINE_SKIP_CODE => b"lineskip",
        BASELINE_SKIP_CODE => b"baselineskip",
        PAR_SKIP_CODE => b"parskip",
        ABOVE_DISPLAY_SKIP_CODE => b"abovedisplayskip",
        BELOW_DISPLAY_SKIP_CODE => b"belowdisplayskip",
        ABOVE_DISPLAY_SHORT_SKIP_CODE => b"abovedisplayshortskip",
        BELOW_DISPLAY_SHORT_SKIP_CODE => b"belowdisplayshortskip",
        LEFT_SKIP_CODE => b"leftskip",
        RIGHT_SKIP_CODE => b"rightskip",
        TOP_SKIP_CODE => b"topskip",
        SPLIT_TOP_SKIP_CODE => b"splittopskip",
        TAB_SKIP_CODE => b"tabskip",
        SPACE_SKIP_CODE => b"spaceskip",
        XSPACE_SKIP_CODE => b"xspaceskip",
        PAR_FILL_SKIP_CODE => b"parfillskip",
        THIN_MU_SKIP_CODE => b"thinmuskip",
        MED_MU_SKIP_CODE => b"medmuskip",
        THICK_MU_SKIP_CODE => b"thickmuskip",
        _ => return None,
    })
}

/// A scalar slot's name (`track::scalar`).
fn scalar_name(k: i64) -> String {
    use crate::track::scalar::*;
    let Ok(k) = u16::try_from(k) else {
        return format!("scalar{k}");
    };
    String::from(match k {
        STR_TOP => "str_ptr",
        HASH_USED => "hash_used",
        HASH_HIGH => "hash_high",
        LAST_BADNESS => "last_badness",
        OUTPUT_ACTIVE => "output_active",
        TERM_OFFSET => "term_offset",
        FILE_OFFSET => "file_offset",
        SELECTOR => "selector",
        INTERACTION => "interaction",
        HISTORY => "history",
        ERROR_COUNT => "error_count",
        SHOWN_MODE => "shown_mode",
        MAG_SET => "mag_set",
        ALIGN_STATE => "align_state",
        DEAD_CYCLES => "dead_cycles",
        AFTER_TOKEN => "after_token",
        LONG_HELP_SEEN => "long_help_seen",
        JOB_NAME => "job_name",
        LOG_NAME => "log_name",
        OUTPUT_FILE_NAME => "output_file_name",
        LOG_OPENED => "log_opened",
        OPEN_PARENS => "open_parens",
        SYS_TIME => "sys_time",
        SYS_DAY => "sys_day",
        SYS_MONTH => "sys_month",
        SYS_YEAR => "sys_year",
        EPOCH_S => "epoch_s",
        EPOCH_US => "epoch_us",
        GLUE_LINEAGE => "glue_lineage",
        FONT_COUNT => "font_count",
        HPACK_RESULT => "hpack_result",
        VPACK_RESULT => "vpack_result",
        LINE_BREAK_RESULT => "line_break_result",
        PAGE_STEP_RESULT => "page_step_result",
        OUTPUT_RESULT => "output_result",
        k if (WRITE_OPEN..WRITE_OPEN + 18).contains(&k) => {
            return format!("write_open[{}]", k - WRITE_OPEN);
        }
        k if (READ_OPEN..READ_OPEN + 17).contains(&k) => {
            return format!("read_open[{}]", k - READ_OPEN);
        }
        k => return format!("scalar{k}"),
    })
}

/// A field of `cur_list`, the nest or the alignment state (`track::list`,
/// `track::align`).
fn list_name(i: i64) -> String {
    use crate::track::{align, list};
    const LIST: [&str; list::COUNT as usize] = [
        "list",
        "mlist",
        "mode",
        "pg",
        "ml",
        "prev_depth",
        "space_factor",
        "clang",
        "incompleat",
        "middle",
        "lr_save",
        "lr_box",
    ];
    const ALIGN: [&str; align::COUNT as usize] = [
        "column", "span", "loop", "adjust", "columns", "tabskips", "stack",
    ];
    let count = i64::from(list::COUNT);
    let name = |names: &[&str], k: i64| {
        usize::try_from(k)
            .ok()
            .and_then(|k| names.get(k))
            .map(|n| (*n).to_string())
    };
    // (a deeper level's fields by its depth: `list1.mode`)
    let stride = i64::from(list::STRIDE);
    if i >= stride {
        return name(&LIST, i % stride).map_or_else(
            || format!("list:{i}"),
            |n| format!("list{}.{n}", i / stride),
        );
    }
    match i.cmp(&count) {
        core::cmp::Ordering::Less => name(&LIST, i).map(|n| format!("list.{n}")),
        core::cmp::Ordering::Equal => Some(String::from("nest")),
        core::cmp::Ordering::Greater => name(&ALIGN, i - count - 1).map(|n| format!("align.{n}")),
    }
    .unwrap_or_else(|| format!("list:{i}"))
}

/// A field of the page builder's state (`track::page`).
fn page_name(i: i64) -> String {
    use crate::track::page::*;
    const SO_FAR_NAMES: [&str; 8] = [
        "goal", "total", "stretch", "fil", "fill", "filll", "shrink", "depth",
    ];
    let Ok(k) = u8::try_from(i) else {
        return format!("page.{i}");
    };
    String::from(match k {
        CONTENTS => "page.contents",
        LIST => "page.list",
        MAX_DEPTH => "page.max_depth",
        LEAST_COST => "page.least_cost",
        BEST_BREAK => "page.best_break",
        BEST_SIZE => "page.best_size",
        INS => "page.ins",
        INSERT_PENALTIES => "page.insert_penalties",
        LAST_GLUE => "page.last_glue",
        LAST_PENALTY => "page.last_penalty",
        LAST_KERN => "page.last_kern",
        LAST_NODE_TYPE => "page.last_node_type",
        DISCARDS => "page.discards",
        LIST_LEN => "page.len",
        LIST_TAIL => "page.tail",
        SPLIT_DISCARDS => "splitdiscards",
        k if (SO_FAR..SO_FAR + 8).contains(&k) => {
            return format!("page.{}", SO_FAR_NAMES[usize::from(k - SO_FAR)]);
        }
        k => return format!("page.{k}"),
    })
}

/// A field of the PDF writer (`pdf::val::field`).
fn pdf_name(i: i64) -> String {
    use crate::pdf::val::field::*;
    let Ok(k) = u8::try_from(i) else {
        return format!("pdf.{i}");
    };
    String::from(match k {
        LAST_MATCH => "pdf.last_match",
        OBJS => "pdf.objs",
        OBJ_TREES => "pdf.obj_trees",
        DESTS => "pdf.dests",
        OUT => "pdf.out",
        OBJ_COUNT => "pdf.obj_count",
        XFORM_COUNT => "pdf.xform_count",
        XIMAGE_COUNT => "pdf.ximage_count",
        INFO_TOKS => "pdfinfo",
        CATALOG_TOKS => "pdfcatalog",
        NAMES_TOKS => "pdfnames",
        TRAILER_TOKS => "pdftrailer",
        TRAILER_ID_TOKS => "pdftrailerid",
        CATALOG_OPENACTION => "pdf.catalog_openaction",
        OUTLINES => "pdf.outlines",
        SPACE_FONT_NAME => "pdf.space_font_name",
        FONT_ATTR => "pdf.font_attr",
        NOBUILTIN_TOUNICODE => "pdf.nobuiltin_tounicode",
        STACKS => "pdf.stacks",
        SHIP => "pdf.ship",
        PDF_FONTS => "pdf.fonts",
        ENCODINGS => "pdf.encodings",
        FONTW => "pdf.font_trees",
        TOUNICODE => "pdf.tounicode",
        FONTMAP => "pdf.fontmap",
        FONTS_MAPPED => "pdf.fonts_mapped",
        EPDF => "pdf.epdf",
        k if (LAST..INFO_TOKS).contains(&k) => return format!("pdf.last[{}]", k - LAST),
        k => return format!("pdf.{k}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;

    /// The lines from byte `from` to the line that begins at `to`.
    fn lines_of(data: &Arc<[u8]>, from: usize, to: usize) -> (u32, u32) {
        let ix = LineIndex::of(data);
        (ix.line(from), ix.line(to - 1))
    }

    #[test]
    fn numbering() {
        let d: Arc<[u8]> = Arc::from(&b"a\nbb\r\nccc\n\nd"[..]);
        assert_eq!(lines_of(&d, 0, 2), (1, 1));
        assert_eq!(lines_of(&d, 2, 10), (2, 3));
        assert_eq!(lines_of(&d, 11, 12), (5, 5));
        assert_eq!(
            runs(&[(0, 1, 2), (0, 3, 3), (0, 5, 6), (1, 1, 1)]),
            [(0, 1, 3), (0, 5, 6), (1, 1, 1)]
        );
        assert_eq!(scaled(12 * 65536), "12.0pt");
        assert_eq!(scaled(65536 / 2), "0.5pt");
        assert_eq!(printable(b"a\rb\xe9"), "a^^Mb^^e9");
    }
}
