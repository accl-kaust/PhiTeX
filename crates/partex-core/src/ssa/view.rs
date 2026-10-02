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

use partex_ssa::fold::StepId;
use partex_ssa::runtime::{Item, RecId};
use partex_ssa::{Runtime, Trace};
use phitex_ir::{Def, Operand, Program, ValueId};

use super::{Fam, Func, SValue, Slot, SsaTracker, TexSsa, line_bounds};
use crate::host::Host;
use crate::tex::Tex;

/// How many characters of text a window's comment shows.
const EXCERPT: usize = 40;

/// The build `tex` holds as a program: a constant for what no window
/// defined (the format), one per file the windows read, then a window
/// per live step of the fold, in program order. It passes
/// [`Program::check`], and its text form round-trips.
#[must_use]
pub fn view<H: Host>(tex: &Tex<H, SsaTracker>) -> Program {
    let rec = tex.tracker.rec.borrow();
    let rt = &rec.rt;
    let fold = &rt.fold;
    let mut names = Names::new(tex);
    let mut prog = Program::default();
    let ident = tex.str_bytes(usize::try_from(tex.format_ident).unwrap_or(0));
    let format = prog.define(
        Def::Const(String::from("format")),
        printable(trim(ident)),
        0,
    );
    // (the lines each live step's last run read, by step: each file's
    // runs of lines, numbered)
    let mut lines: BTreeMap<StepId, Vec<(u32, u32, u32)>> = BTreeMap::new();
    let mut numbering: BTreeMap<usize, LineIndex> = BTreeMap::new();
    for (name, bytes, from, to, step, run) in rec.st.steps.line_runs() {
        let live = fold
            .steps
            .get(step as usize)
            .is_some_and(|s| s.live && s.run == run);
        if !live || to <= from {
            continue;
        }
        let ix = numbering
            .entry(bytes.as_ptr() as usize)
            .or_insert_with(|| LineIndex::of(bytes));
        lines
            .entry(step)
            .or_default()
            .push((name, ix.line(from), ix.line(to - 1)));
    }
    for v in lines.values_mut() {
        *v = runs(v);
    }
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
        let name = file_name(&rec.st, id);
        let v = prog.define(Def::Const(format!("file {name}")), String::new(), 0);
        files.insert(id, v);
    }
    let mut value: Vec<Option<ValueId>> = alloc::vec![None; fold.steps.len()];
    for &s in &fold.order {
        let st = &fold.steps[s as usize];
        let mut operands = Vec::new();
        let mut spans = Vec::new();
        for &(file, a, b) in lines.get(&s).into_iter().flatten() {
            let span = if a == b {
                format!("{}:{a}", file_name(&rec.st, file))
            } else {
                format!("{}:{a}-{b}", file_name(&rec.st, file))
            };
            operands.push(Operand::Named(span.clone(), files[&file]));
            spans.push(span);
        }
        for a in &st.reads {
            match a.0 {
                // (the lines, above)
                Fam::Source => {}
                Fam::Load => {
                    let id = u32::try_from(a.1).unwrap_or(u32::MAX);
                    operands.push(Operand::Named(file_name(&rec.st, id), files[&id]));
                }
                _ => {
                    // (a definition reaching it is a live step's before it,
                    // whose value is made)
                    let from = fold
                        .reaching(a, st.key)
                        .and_then(|d| value[d.step as usize])
                        .unwrap_or(format);
                    operands.push(Operand::Named(names.slot(&rec.st, *a), from));
                }
            }
        }
        let written = writes(rt, &st.recs);
        let defines: Vec<String> = written.iter().map(|a| names.slot(&rec.st, *a)).collect();
        let shows = shows(tex, rt, s, st.run, &st.recs, &spans, &lines, &rec.st);
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

/// Step `id`'s calls, with their reads, writes and effects, in the
/// trace's text form (`Trace::to_text`); `None` if the step is not live.
#[must_use]
pub fn step_trace<H: Host>(tex: &Tex<H, SsaTracker>, id: StepId) -> Option<String> {
    let rec = tex.tracker.rec.borrow();
    let st = rec.rt.fold.steps.get(id as usize).filter(|s| s.live)?;
    let t: Trace = rec.rt.trace_of(&st.recs);
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

/// A window's comment: its step, the run if it ran again, the source it
/// read, whether it shipped pages, and the text it set.
#[allow(clippy::too_many_arguments)]
fn shows<H: Host>(
    tex: &Tex<H, SsaTracker>,
    rt: &Runtime<TexSsa>,
    s: StepId,
    run: u32,
    recs: &[RecId],
    spans: &[String],
    lines: &BTreeMap<StepId, Vec<(u32, u32, u32)>>,
    st: &super::RecState,
) -> String {
    let mut out = format!("step {s}");
    if run > 1 {
        out.push_str(&format!(" (run {run})"));
    }
    out.push(':');
    for sp in spans {
        out.push(' ');
        out.push_str(sp);
    }
    let ships = recs.iter().map(|&r| ships(rt, r)).sum::<usize>();
    match ships {
        0 => {}
        1 => out.push_str(" ships a page"),
        n => out.push_str(&format!(" ships {n} pages")),
    }
    let mut text = String::new();
    set_text(rt, recs, &mut text);
    if text.trim().is_empty() {
        // (the source it read: its first run of lines)
        text = lines
            .get(&s)
            .and_then(|v| v.first())
            .map(|&(file, a, _)| source_line(tex, st, file, a))
            .unwrap_or_default();
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.is_empty() {
        let mut t: String = text.chars().take(EXCERPT).collect();
        if text.chars().count() > EXCERPT {
            t.push('…');
        }
        out.push_str(&format!(" {}", phitex_ir::quote(&t)));
    }
    out
}

/// The pages record `r`'s subtree shipped.
fn ships(rt: &Runtime<TexSsa>, r: RecId) -> usize {
    let rec = rt.record(r);
    usize::from(rec.func == Func::ShipOut)
        + rec
            .items
            .iter()
            .map(|it| match it {
                Item::Call(c) => ships(rt, *c),
                _ => 0,
            })
            .sum::<usize>()
}

/// The text records `recs` set: the characters of the nodes their writes
/// put on the page, in order, else of the list they left.
fn set_text(rt: &Runtime<TexSsa>, recs: &[RecId], out: &mut String) {
    let mut page: BTreeMap<i64, &partex_engine::node::Node> = BTreeMap::new();
    let mut list = None;
    for &r in recs {
        for (a, v) in &rt.record(r).writes {
            let Some(v) = v.as_ref().and_then(|v| v.1.as_deref()) else {
                continue;
            };
            match (a.0, v) {
                (Fam::PageNode, SValue::Field(f)) => {
                    if let Some(n) = f.get::<partex_engine::node::Node>() {
                        page.insert(a.1, n);
                    }
                }
                (Fam::List, SValue::Nodes(l)) if a.1 == i64::from(crate::track::list::LIST) => {
                    list = Some(l);
                }
                _ => {}
            }
        }
    }
    for n in page.values() {
        node_text(n, out);
        if out.chars().count() > EXCERPT {
            return;
        }
    }
    if out.trim().is_empty()
        && let Some(l) = list
    {
        for n in l.iter() {
            node_text(n, out);
            if out.chars().count() > EXCERPT {
                return;
            }
        }
    }
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
fn source_line<H: Host>(
    tex: &Tex<H, SsaTracker>,
    st: &super::RecState,
    file: u32,
    line: u32,
) -> String {
    let _ = tex;
    let Some(bytes) = st.steps.line_runs().find(|l| l.0 == file).map(|l| l.1) else {
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

/// Runs of consecutive lines, by file, in the order read: `(file, first,
/// last)`, each line read once.
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
}

impl<'a, H: Host> Names<'a, H> {
    fn new(tex: &'a Tex<H, SsaTracker>) -> Self {
        Names {
            tex,
            fonts: BTreeMap::new(),
        }
    }

    /// Slot `a`'s name.
    fn slot(&mut self, st: &super::RecState, a: Slot) -> String {
        slot_name(self, st, a)
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
                format!("\\{kind}mark")
            } else {
                format!("\\{kind}marks{c}")
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
    let esc = |s: &[u8]| format!("\\{}", printable(s));
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
            .map_or_else(|| format!("gluepar{}", p - GLUE_BASE), |n| esc(n));
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
            .map_or_else(|| format!("intpar{}", p - INT_BASE), |n| esc(n));
    }
    if (DIMEN_BASE..SCALED_BASE).contains(&p) {
        return dimen_param_name(p - DIMEN_BASE)
            .map_or_else(|| format!("dimenpar{}", p - DIMEN_BASE), |n| esc(n));
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
        CUR_FONT_LOC => b"font",
        XORD_CODE_BASE => b"xordcode",
        XCHR_CODE_BASE => b"xchrcode",
        XPRN_CODE_BASE => b"xprncode",
        _ => return format!("eqtb:{p}"),
    };
    esc(local)
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
    let Ok(k) = u8::try_from(i) else {
        return format!("page.{i}");
    };
    const SO_FAR_NAMES: [&str; 8] = [
        "goal", "total", "stretch", "fil", "fill", "filll", "shrink", "depth",
    ];
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
        SPLIT_DISCARDS => "\\splitdiscards",
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
        INFO_TOKS => "\\pdfinfo",
        CATALOG_TOKS => "\\pdfcatalog",
        NAMES_TOKS => "\\pdfnames",
        TRAILER_TOKS => "\\pdftrailer",
        TRAILER_ID_TOKS => "\\pdftrailerid",
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
        assert_eq!(runs(&[(0, 1, 2), (0, 3, 3), (1, 1, 1), (0, 4, 5)]), [
            (0, 1, 3),
            (1, 1, 1),
            (0, 4, 5)
        ]);
        assert_eq!(scaled(12 * 65536), "12.0pt");
        assert_eq!(scaled(65536 / 2), "0.5pt");
        assert_eq!(printable(b"a\rb\xe9"), "a^^Mb^^e9");
    }
}
