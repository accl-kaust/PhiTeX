//! The build printed as a program (DESIGN 4.3 item 7): `PhiTeX`'s
//! document as an SSA program, made of the recorder's steps, so exact.
//!
//! A value per live step of the fold (a *window*), in program order:
//!
//! ```text
//! %0 = format                                  ; (preloaded format=plain 2026.10.2)
//! %1 = file incr.tex                           ; 37 lines
//! %7 = window(incr.tex:9-12=%1, \count5=%6, \words=%3, \section=%4, ...; \count5, \box255, ...) ; step 6: incr.tex:9-12 "Opening Lorem ipsum dolor sit amet, con…"
//! ```
//!
//! - Its operands are its *imports*: each address it read from outside
//!   itself (the step's reads, `Fold::steps`' `reads`), by name, with
//!   the value of the window whose definition reached it
//!   (`Fold::reaching`), or `%0` if none did (the format's definitions
//!   and the engine's initial state); the lines it read are imports of
//!   their file's constant, as runs `file:first-last`, and a file it
//!   loaded whole is imported by its name.
//! - Its names are its *exports*: the addresses its records wrote, by
//!   name ([`slot_name`]).
//! - Its comment (`shows`): the step's id (and the run, after a rebuild
//!   ran it again), the source it read, and a short excerpt of the text
//!   it set, the characters of the nodes it put on the page (else in its
//!   list, else the source's).
//!
//! An address's name: a control sequence's by its name in the hash
//! (`\section`), an eqtb entry otherwise by what TeX calls it
//! (`\count12`, `\catcode92`, `\baselineskip`, `\everypar`,
//! `\textfont1`), and the other families by a prefix and a field
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
            .map(|&r| rt.record(r).writes.len())
            .sum();
        shows = format!("step {s}: the job's start, {shows}, {w} definitions:{spans}");
    }
    let format = prog.define(Def::Const(String::from("format")), shows, 0);
    // (a constant per file: the ones read by lines, and the ones loaded)
    let mut files: BTreeMap<u32, ValueId> = BTreeMap::new();
    let mut loaded: BTreeSet<u32> = lines.values().flatten().map(|l| l.0).collect();
    for &s in &fold.order {
        for a in &fold.steps[s as usize].reads {
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
        for a in &step.reads {
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
            page_text(rt, step.key, &step.reads)
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

/// The addresses records `recs` wrote, each once, in order (a later
/// record's write of an address is the same definition).
fn writes(rt: &Runtime<TexSsa>, recs: &[RecId]) -> Vec<Slot> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for &r in recs {
        for (a, _) in &rt.record(r).writes {
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
    fn walk(rt: &Runtime<TexSsa>, r: RecId, ship: bool, logs: &mut Vec<Vec<u8>>) {
        let rec = rt.record(r);
        let ship = ship || rec.func == Func::ShipOut;
        if rec.func == Func::ShipOut {
            logs.push(Vec::new());
        }
        for it in &rec.items {
            match it {
                Item::Call(c) => walk(rt, *c, ship, logs),
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
    for &r in recs {
        walk(rt, r, false, &mut logs);
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
/// list it left (the list's text past the text of the list that reached
/// it, or all of it).
fn set_text(rt: &Runtime<TexSsa>, key: u64, recs: &[RecId]) -> String {
    use partex_engine::node::Node;
    let list = Slot(Fam::List, i64::from(crate::track::list::LIST));
    let mut page: BTreeMap<i64, &Node> = BTreeMap::new();
    let mut left = None;
    for &r in recs {
        for (a, v) in &rt.record(r).writes {
            match (a.0, v.as_ref().and_then(|v| v.1.as_deref())) {
                (Fam::PageNode, Some(SValue::Field(f))) => {
                    if let Some(n) = f.get::<Node>() {
                        page.insert(a.1, n);
                    }
                }
                (Fam::List, Some(SValue::Nodes(l))) if *a == list => left = Some(l),
                _ => {}
            }
        }
    }
    let mut out = String::new();
    for n in page.values() {
        node_text(n, &mut out);
        if out.chars().count() > EXCERPT {
            break;
        }
    }
    if !out.trim().is_empty() {
        return out;
    }
    let Some(left) = left else {
        return String::new();
    };
    let text = |l: &partex_engine::nodelist::NodeList| {
        let mut t = String::new();
        for n in l.iter() {
            node_text(n, &mut t);
        }
        t
    };
    let now = text(left);
    let before = rt
        .fold
        .reaching(&list, key)
        .and_then(|d| rt.record(d.rec).writes.get(d.ix as usize)?.1.clone())
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
            .and_then(|d| rt.record(d.rec).writes.get(d.ix as usize)?.1.clone());
        if let Some(SValue::Field(f)) = v.as_ref().and_then(|v| v.1.as_deref())
            && let Some(n) = f.get::<partex_engine::node::Node>()
        {
            node_text(n, &mut out);
        }
        if out.chars().count() > EXCERPT {
            break;
        }
    }
    out
}

/// The characters of node `n` (and of the nodes in it), glue as a space.
fn node_text(n: &partex_engine::node::Node, out: &mut String) {
    use partex_engine::node::Node;
    match n {
        Node::Glyphs(g) => out.extend(g.chars().iter().map(|&c| text_char(c))),
        Node::Ligature(l) => out.extend(l.original.iter().map(|&c| text_char(c))),
        Node::Disc(d) => {
            for m in &d.replace {
                node_text(m, out);
            }
        }
        Node::Box(b) => {
            for m in &b.list {
                node_text(m, out);
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
        Fam::Source | Fam::Line => format!("{a}"),
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
        k if (LAST..INFO_TOKS).contains(&k) => return format!("pdf.last[{}]", k - LAST),
        k => return format!("pdf.{k}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;
    use crate::ssa::Recorder;
    use crate::testing::TestHost;
    use alloc::sync::Arc;

    /// A small document for INITEX with one font: definitions, a
    /// register in a group and out of it, two paragraphs, a page.
    const DOC: &[u8] = b"\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6
\\font\\rm=cmr10 \\rm \\hsize=100pt \\vsize=100pt
\\def\\greet#1{Hello #1.}
\\count1=5

\\greet{World} One line.

{\\count1=6 Second paragraph.}

\\end
";

    /// `DOC` built in SSA mode (with `doc` as the edit makes it, if any).
    fn build(doc: &[u8]) -> Tex<TestHost, SsaTracker> {
        let mut host = TestHost::default();
        host.files.insert(
            b"cmr10.tfm".to_vec(),
            include_bytes!("../../testdata/cmr10.tfm").to_vec(),
        );
        host.files.insert(b"doc.tex".to_vec(), doc.to_vec());
        let params = Params {
            ini: true,
            interaction: Some(crate::error::BATCH_MODE),
            ..Params::default()
        };
        let mut tex = Tex::new(host, SsaTracker::new(Recorder::new()), params);
        let rep = crate::ssa::run(&mut tex, b"doc", false, 0);
        assert!(rep.history <= 1, "the build failed: {}", rep.history);
        tex
    }

    /// A view's text, checked and round-tripped.
    fn text(p: &Program) -> String {
        p.check().unwrap();
        let t = p.to_text();
        let q = Program::parse(&t).unwrap();
        assert_eq!(q.values, p.values);
        t
    }

    /// `p` with only the imports from windows (the edges between them),
    /// and the pool's strings without their numbers (the build hands
    /// them out, DESIGN 3.2): what the golden test compares.
    fn edges(p: &Program) -> String {
        let mut q = p.clone();
        let unnumbered = |n: &mut String| {
            if n.starts_with("string:") {
                *n = String::from("string:N");
            }
        };
        for v in &mut q.values {
            if let Def::Op {
                operands, defines, ..
            } = &mut v.def
            {
                operands.retain(|o| {
                    !matches!(o, Operand::Named(_, from)
                        if matches!(p.values[from.0 as usize].def, Def::Const(_)))
                });
                defines.iter_mut().for_each(unnumbered);
            }
        }
        q.to_text()
    }

    /// The view of a small document, and of it rebuilt after a word
    /// changed: a value per window, its imports from the windows that
    /// defined them, its exports, its span and the text it set; after the
    /// edit, what changed ([`WARM`]).
    #[test]
    fn a_small_document() {
        let mut tex = build(DOC);
        let cold = view(&tex);
        text(&cold);
        let edited = String::from_utf8(DOC.to_vec())
            .unwrap()
            .replace("World", "Moon");
        tex.host_mut()
            .files
            .insert(b"doc.tex".to_vec(), edited.into_bytes());
        let r = crate::ssa::rebuild(&mut tex, false, true);
        assert!(r.unsupported.is_none(), "{:?}", r.unsupported);
        let warm = view(&tex);
        text(&warm);
        let a = edges(&cold);
        assert_eq!(a, COLD, "the view's edges are now:\n{a}");
        // (the paragraph's calls, the addresses named as in the view)
        let t = step_trace(&tex, 11).unwrap();
        assert!(t.contains("read @\\greet = "), "{t}");
        assert!(t.contains("call @line_break("), "{t}");
        assert_eq!(Trace::parse(&t).unwrap().to_text(), t);
        let d = changes(&cold, &warm);
        assert_eq!(d, WARM, "the changes are now:\n{d}");
    }

    /// What changed from `a` to `b`, value by value: the comment, then
    /// the imports and exports gone (`-`) and come (`+`).
    fn changes(a: &Program, b: &Program) -> String {
        // (the pool's strings without their numbers, as in `edges`)
        let unnumbered = |n: &str| {
            if n.starts_with("string:") {
                String::from("string:N")
            } else {
                n.to_string()
            }
        };
        let named = |o: &Operand| match o {
            Operand::Named(n, v) => format!("{}={v}", unnumbered(n)),
            _ => String::new(),
        };
        let mut out = String::new();
        for (i, (x, y)) in a.values.iter().zip(&b.values).enumerate() {
            if x == y {
                continue;
            }
            let _ = writeln!(out, "%{i} {}", y.shows);
            if let (
                Def::Op {
                    operands: oa,
                    defines: da,
                    ..
                },
                Def::Op {
                    operands: ob,
                    defines: db,
                    ..
                },
            ) = (&x.def, &y.def)
            {
                let (oa, ob): (Vec<String>, Vec<String>) = (
                    oa.iter().map(named).collect(),
                    ob.iter().map(named).collect(),
                );
                for o in oa.iter().filter(|o| !ob.contains(o)) {
                    let _ = writeln!(out, "  -import {o}");
                }
                for o in ob.iter().filter(|o| !oa.contains(o)) {
                    let _ = writeln!(out, "  +import {o}");
                }
                let (da, db): (Vec<String>, Vec<String>) = (
                    da.iter().map(|d| unnumbered(d)).collect(),
                    db.iter().map(|d| unnumbered(d)).collect(),
                );
                for d in da.iter().filter(|d| !db.contains(d)) {
                    let _ = writeln!(out, "  -export {d}");
                }
                for d in db.iter().filter(|d| !da.contains(d)) {
                    let _ = writeln!(out, "  +export {d}");
                }
            }
        }
        out
    }

    /// The edges of `DOC`'s view.
    const COLD: &str = r#"%0 = format                                  ; step 0: the job's start, (INITEX), 31944 definitions: doc:1
%1 = file doc
%2 = window(; \catcode123)                   ; step 1: doc:1 "\catcode`\{=1 \catcode`\}=2 \catcode`\#=…"
%3 = window(\catcode123=%2; \catcode125)     ; step 2: doc:1 "\catcode`\}=2 \catcode`\#=6"
%4 = window(\catcode123=%2, \catcode125=%3; \catcode35) ; step 3: doc:1 "\catcode`\#=6"
%5 = window(\catcode123=%2, \catcode125=%3, \catcode35=%4; string:N, str_ptr, text:\rm, string:N, font:cmr10.metrics, font:cmr10.fontdimen, font:cmr10.hyphenchar, font:cmr10.skewchar, font:cmr10.expand, font:cmr10.glue, font:cmr10.code0, font:cmr10.code1, font:cmr10.code2, font:cmr10.code3, font:cmr10.code4, font:cmr10.code5, font:cmr10.code6, font:cmr10.code7, fonts, \rm, fontid:1:\rm, text:fontid:1:\rm, current_font) ; step 4: doc:2 "\font\rm=cmr10 \rm \hsize=100pt \vsize=1…"
%6 = window(; hsize)                         ; step 5: doc:2 "\hsize=100pt \vsize=100pt"
%7 = window(; vsize)                         ; step 6: doc:2 "\vsize=100pt"
%8 = window(\catcode35=%4, str_ptr=%5, \catcode123=%2, \catcode125=%3; string:N, str_ptr, text:\greet, align_state, \greet) ; step 7: doc:3 "\def\greet#1{Hello #1.}"
%9 = window(\catcode35=%4, \catcode123=%2, \catcode125=%3; ) ; step 8: doc:3
%10 = window(\catcode35=%4, \catcode123=%2, \catcode125=%3; \count1) ; step 9: doc:4 "\count1=5"
%11 = window(; )                             ; step 10: doc:5
%12 = window(\catcode123=%2, text:\greet=%8, \greet=%8, align_state=%8, \catcode125=%3, current_font=%5, font:cmr10.metrics=%5, font:cmr10.hyphenchar=%5, font:cmr10.glue=%5, font:cmr10.fontdimen=%5, hsize=%6, font:cmr10.expand=%5, text:fontid:1:\rm=%5, vsize=%7; align_state, font:cmr10.glue, list.mlist, list.mode, list.ml, list.space_factor, list.clang, list.incompleat, list.middle, list.lr_box, nest, hyph.patterns, last_badness, file_offset, selector, hpack_result, list.prev_depth, list.pg, line_break_result, list.lr_save, error_count, page.contents, page.goal, page.fil, page.fill, page.filll, page.max_depth, page.least_cost, page.stretch, page.shrink, page[0], page.total, page.depth, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, page[1], page.len, page.tail, list.list) ; step 11: doc:6-7 "Hello World. One line."
%13 = window(\catcode123=%2, align_state=%12, list.mode=%12, nest=%12, list.list=%12; align_state, save[0], save.boundary, save.group, save.level, save.ptr) ; step 12: doc:8 "{\count1=6 Second paragraph.}"
%14 = window(\catcode123=%2, \catcode125=%3, list.mode=%12, save.level=%13, \count1=%10, save.ptr=%13, nest=%12, list.list=%12; save[1], save.ptr, save[2], \count1) ; step 13: doc:8 "\count1=6 Second paragraph.}"
%15 = window(\catcode123=%2, \catcode125=%3, list.mode=%12, list.list=%12, nest=%12, list.mlist=%12, list.ml=%12, list.prev_depth=%12, list.space_factor=%12, list.clang=%12, list.incompleat=%12, list.middle=%12, list.lr_save=%12, list.lr_box=%12, page.contents=%12, page.goal=%12, page.total=%12, page.stretch=%12, page.fil=%12, page.fill=%12, page.filll=%12, page.shrink=%12, page.depth=%12, page.max_depth=%12, page.least_cost=%12, page.len=%12, page.tail=%12, current_font=%5, font:cmr10.metrics=%5, font:cmr10.hyphenchar=%5, font:cmr10.glue=%12, align_state=%13, save.group=%13, save.level=%13, save.ptr=%14, save[2]=%14, save[1]=%14, \count1=%14, save[0]=%13, save.boundary=%13, hsize=%6, hyph.patterns=%12, selector=%12, font:cmr10.expand=%5, text:fontid:1:\rm=%5; page.least_cost, page.best_break, page.best_size, page[2], align_state, save.level, save.ptr, save[1], \count1, save.group, save.boundary, list.mlist, list.mode, list.ml, list.space_factor, list.clang, list.incompleat, list.middle, list.lr_box, nest, last_badness, file_offset, selector, hpack_result, list.prev_depth, list.pg, line_break_result, list.lr_save, error_count, page.stretch, page.shrink, page[3], page.total, page.depth, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, page[4], page.len, page.tail, list.list) ; step 14: doc:8-9 "Second paragraph."
%16 = window(list.mode=%15, page.len=%15, hsize=%6, list.list=%15, list.mlist=%15, list.pg=%15, list.ml=%15, list.prev_depth=%15, list.space_factor=%15, list.clang=%15, list.incompleat=%15, list.middle=%15, list.lr_save=%15, list.lr_box=%15, page.contents=%12, page.total=%15, page.depth=%15, page.max_depth=%12, page.goal=%12, page.stretch=%15, page.fil=%12, page.fill=%12, page.filll=%12, page.shrink=%15, page.least_cost=%15; page[5], page.total, page.fill, page.shrink, page.depth, page[6], page.len, page.tail, page.least_cost, page.best_break, page.best_size, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, list.list) ; step 15: doc:10 "\end"
%17 = window(list.list=%16, page.contents=%12, page.goal=%12, page.total=%16, page.stretch=%15, page.fil=%12, page.fill=%16, page.filll=%12, page.shrink=%16, page.depth=%16, page.max_depth=%12, page.least_cost=%16, page.best_break=%16, page.best_size=%16, page.last_glue=%16, page.last_penalty=%16, page.last_kern=%16, page.last_node_type=%16, page.len=%16, page[0]=%12, page[1]=%12, page[2]=%15, page[3]=%15, page[4]=%15, page[5]=%16, page[6]=%16, file_offset=%15, selector=%15, \count1=%15, str_ptr=%8, list.mlist=%15, list.mode=%15, list.pg=%15, list.ml=%15, list.prev_depth=%15, list.space_factor=%15, list.clang=%15, list.incompleat=%15, list.middle=%15, list.lr_save=%15, list.lr_box=%15, save.level=%15; page.contents, page.goal, page.total, page.stretch, page.fil, page.fill, page.filll, page.shrink, page.depth, page.max_depth, page.least_cost, page.best_break, page.best_size, page.ins, page.insert_penalties, last_badness, vpack_result, outputpenalty, page.len, page.tail, page.discards, string:N, str_ptr, string:N, string:N, output_file_name, dead_cycles, \box255, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, list.list, file_offset, open_parens, newlinechar, mag_set, dvi.file, dvi.fonts, dvi.totals, dvi.writer, write:log, selector) ; step 16: ships [0.5] "Hello World. One line. Second paragraph."
"#;

    /// What changed after the edit: the paragraph ran again, and the
    /// fire; the second paragraph's imports of the font's glue and of the
    /// patterns are now the ones before the first (its run read the glue
    /// and the patterns the cold build had made, which are not placed).
    const WARM: &str = r#"%12 step 11 (run 2): doc:6-7 "Hello Moon. One line."
  -import \catcode87=%0
  -import \catcode100=%0
  -import font:cmr10.fontdimen=%5
  -import \sfcode87=%0
  -import \sfcode114=%0
  -import \sfcode100=%0
  -import \lccode87=%0
  +import \catcode77=%0
  +import \sfcode77=%0
  +import \lccode77=%0
  -export font:cmr10.glue
  -export hyph.patterns
%15 step 14: doc:8-9 "Second paragraph."
  -import font:cmr10.glue=%12
  -import hyph.patterns=%12
  +import font:cmr10.glue=%5
  +import hyph.patterns=%0
%17 step 16 (run 2): ships [0.5] "Hello Moon. One line. Second paragraph."
"#;

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
