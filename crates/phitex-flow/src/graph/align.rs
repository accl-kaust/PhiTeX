//! Alignment (tex.web part 37, §768–§812), and the display alignment of
//! §1145 and §1206, on the graph.
//!
//! An alignment is a group and its preamble a value. `\halign` (or
//! `\valign`) is an `Open` node; the node after it reads the preamble once
//! (§777–§784) and keeps it in that group's entry of `RState::groups`
//! ([`AlignSpec`]: the templates, the tabskip glue, the loop of a periodic
//! preamble, and the list the alignment began in), then opens the
//! entries' group (§774). The current column and span, how the entry began
//! (`\omit`) and ended (`&`, `\span`, `\cr`, `\crcr`), and `align_state` are
//! the fields of `RState::al` ([`AlignCur`]), handed from node to node; an
//! alignment inside an entry keeps the enclosing one's in its spec and
//! gives them back at its end.
//!
//! An entry: its u template is input, and the template's end (a mark
//! token) sets `align_state` to 0 (§324); a tab or `\cr` read with
//! `align_state` 0 — wherever it is read: `Exec::next_raw` — inserts the
//! v template (§789), which ends with `\endtemplate`. That token is a
//! `Close` node for the entry's group (§791: `fin_col`), and the node
//! after it gathers what the nodes of the entry emitted into an unset box,
//! emitted into the row with the tabskip glue after the column (§796,
//! §795); a node then opens the next entry's group. After `\cr` a node
//! gathers the row (§799: `fin_row`) into an unset row in the alignment's
//! list. A `}` after a row closes the entries' group, then the alignment's;
//! the node after them gathers the alignment's list (rows, `\noalign`
//! material) and does `fin_align` (§800–§812) as a pure function of it:
//! the column widths with their spans, the preamble packed `to` or
//! `spread`, every row and entry set; it emits them into the enclosing list
//! (a row as a box appended to a vertical list: its interline glue is the
//! enclosing gatherer's, as it was the alignment's list's in TeX, from the
//! same depths). Nothing accumulates while commands run.
//!
//! Levels: an alignment begun at level `L` has its list at `L+1`, its rows
//! at `L+2`, its entries at `L+3`.
//!
//! A display alignment (`$$\halign{...}$$`, §1145, §1206) is an alignment
//! begun in a display's math group (`graph::math`): its list starts from
//! the depth of the vertical list before the display, its rows are shifted
//! by `\displayindent`, and once it is set its material waits in the
//! display's group record (`MathKind::DisplayAlign`) for the closing `$$`,
//! whose packaging node puts `\predisplaypenalty`, `\abovedisplayskip`, the
//! alignment, `\postdisplaypenalty` and `\belowdisplayskip` into the
//! vertical list before the paragraph resumes (§1200).
//!
//! TeX's errors here are written as TeX writes them (the message, the
//! context lines, the help): the log is compared.

use std::sync::Arc;

use super::material::{box_start, push_horizontal};
use super::{Exec, Group, GroupKind, Kind, Mode};
use crate::item::{BoxNode, GlueSet, Item, RUNNING, Unset, is_zero};
use crate::pack::{Params, Spec, hpack, vpack};
use crate::page::{IGNORE_DEPTH, push_vertical};
use crate::show::{CTX, HALT, LINE, LN, NL, Place, RAW, THAT_MAKES_100, report, zround};
use crate::val::{Al, DimenPar, Glue, GluePar, IntPar, Key, Meaning, Prim, Tok, TokPar, Val};

/// `align_state` outside an entry (tex.web 309).
const ALIGN_BASE: i32 = 1_000_000;
/// §769: a column no entry has used.
const NULL_FLAG: i32 = -0o10_000_000_000;
/// §421.
const MAX_DIMEN: i32 = 0o7_777_777_777;
/// The tabskip glue's subtype is its parameter's number + 1.
const TAB_SKIP: u8 = GluePar::Tabskip as u8;

/// How an entry ended (tex.web 780: a tab character, `span_code`,
/// `cr_code`, `cr_cr_code`; in this order).
pub(super) const END_TAB: u8 = 1;
pub(super) const END_SPAN: u8 = 2;
pub(super) const END_CR: u8 = 3;
pub(super) const END_CRCR: u8 = 4;

/// The reader's own commands of an alignment.
const PRE: &str = "\u{0}alignpre";
const PEEK: &str = "\u{0}alignpeek";
const COL: &str = "\u{0}aligncol";
const END_TEMPLATE: &str = "\u{0}endtemplate";
const END_U: &str = "\u{0}endu";
const PACK: &str = "\u{0}alignpack";
const NEXT: &str = "\u{0}alignnext";
const ROW: &str = "\u{0}alignrow";
const FIN: &str = "\u{0}alignfin";
const SET: &str = "\u{0}alignset";
const FROZEN_CR: &str = "\u{0}cr";
const END_GRAF: &str = "\u{0}endgraf";
const NORMAL_PARAGRAPH: &str = "\u{0}normalparagraph";

const HELP_TAB: [&str; 6] = [
    "I can't figure out why you would want to use a tab mark",
    "here. If you just want an ampersand, the remedy is",
    "simple: Just type `I\\&' now. But if some right brace",
    "up above has ended a previous alignment prematurely,",
    "you're probably due for more error messages, and you",
    "might try typing `S' now just to see what is salvageable.",
];
const HELP_TAB_CR: [&str; 5] = [
    "I can't figure out why you would want to use a tab mark",
    "or \\cr or \\span just now. If something like a right brace",
    "up above has ended a previous alignment prematurely,",
    "you're probably due for more error messages, and you",
    "might try typing `S' now just to see what is salvageable.",
];
const HELP_FIX: [&str; 3] = [
    "I've put in what seems to be necessary to fix",
    "the current column of the current alignment.",
    "Try to go on, since this might almost work.",
];
const HELP_NOALIGN: [&str; 2] = [
    "I expect to see \\noalign only after the \\cr of",
    "an alignment. Proceed, and I'll ignore this case.",
];
const HELP_OMIT: [&str; 2] = [
    "I expect to see \\omit only after tab marks or the \\cr of",
    "an alignment. Proceed, and I'll ignore this case.",
];
const HELP_SHARP_NONE: [&str; 3] = [
    "There should be exactly one # between &'s, when an",
    "\\halign or \\valign is being set up. In this case you had",
    "none, so I've put one in; maybe that will work.",
];
const HELP_SHARP_MORE: [&str; 3] = [
    "There should be exactly one # between &'s, when an",
    "\\halign or \\valign is being set up. In this case you had",
    "more than one, so I'm ignoring all but the first.",
];
const HELP_EXTRA_TAB: [&str; 3] = [
    "You have given more \\span or & marks than there were",
    "in the preamble to the \\halign or \\valign now in progress.",
    "So I'll assume that you meant to type \\cr instead.",
];
const HELP_MISSING_CR: [&str; 1] = ["I'm guessing that you meant to end an alignment here."];
const HELP_DISPLAY: [&str; 2] = [
    "Displays can use special alignments (like \\eqalignno)",
    "only if nothing but the alignment itself is between $$'s.",
];
const HELP_DOLLAR: [&str; 2] = [
    "The `$' that I just saw supposedly matches a previous `$$'.",
    "So I shall assume that you typed `$$' both times.",
];

/// The alignment being read, a field of the reader state (tex.web 309,
/// 770): `align_state`, `cur_align` and `cur_span` (column numbers), and
/// `extra_info(cur_align)`: whether the entry began with `\omit`, how it
/// ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct AlignCur {
    /// An alignment is open: braces are counted, entries' ends looked for.
    pub(super) on: bool,
    pub(super) state: i32,
    pub(super) col: u16,
    pub(super) span: u16,
    pub(super) omit: bool,
    pub(super) end: u8,
}

impl AlignCur {
    pub(super) const OFF: AlignCur = AlignCur {
        on: false,
        state: ALIGN_BASE,
        col: 0,
        span: 0,
        omit: false,
        end: 0,
    };
}

/// A brace put back into the input is counted again when it is read
/// (tex.web 325: `back_input` undoes its count).
pub(super) fn uncount(al: &mut AlignCur, t: &Tok) {
    match t {
        Tok::Char(1, _) => al.state -= 1,
        Tok::Char(2, _) => al.state += 1,
        _ => {}
    }
}

/// A column's templates: u, ending with the mark of its end, and v,
/// ending with `\endtemplate` (§779–§784).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Template {
    pub(super) u: Arc<[Tok]>,
    pub(super) v: Arc<[Tok]>,
}

/// An alignment: its preamble, and the list it began in (TeX's
/// `push_nest` and `push_alignment`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AlignSpec {
    pub(super) valign: bool,
    /// `to` or `spread` (§645: `scan_spec`).
    pub(super) spec: Spec,
    pub(super) cols: Vec<Template>,
    /// The tabskip glue before each column and after the last, and whether
    /// it is TeX's shared zero glue (§778).
    pub(super) tabskips: Vec<(Arc<Glue>, bool)>,
    /// A periodic preamble repeats from this column (§779: `cur_loop`).
    pub(super) looping: Option<u16>,
    /// The template of an omitted entry (§789: `omit_template`).
    pub(super) omit: Arc<[Tok]>,
    /// The list the alignment began in.
    pub(super) level: u16,
    pub(super) mode: Mode,
    pub(super) inner: bool,
    pub(super) space_factor: i32,
    pub(super) prev_depth: i32,
    /// The enclosing list's `\prevgraf` and language state (a new list's
    /// `\prevgraf` is 0: `push_nest`).
    pub(super) prev_graf: i32,
    pub(super) hyph_lang: [u8; 4],
    /// The enclosing alignment's state (§772).
    pub(super) outer: AlignCur,
    /// Begun in a display (§1145).
    pub(super) display: bool,
}

impl AlignSpec {
    /// The column whose templates column `j` has: past the preamble, a
    /// periodic one repeats from its loop (§793–§794).
    fn col_of(&self, j: usize) -> Option<usize> {
        let n = self.cols.len();
        if j < n {
            return Some(j);
        }
        let l = usize::from(self.looping?);
        (l < n).then(|| l + (j - n) % (n - l))
    }

    fn template(&self, j: usize) -> &Template {
        let c = self.col_of(j).unwrap_or(self.cols.len().saturating_sub(1));
        &self.cols[c]
    }

    /// The tabskip glue after column `j` (§795, §793).
    fn tab_after(&self, j: usize) -> (Arc<Glue>, bool) {
        let c = self.col_of(j).unwrap_or(self.cols.len().saturating_sub(1));
        self.tabskips[(c + 1).min(self.tabskips.len() - 1)].clone()
    }
}

fn end_code(a: Al) -> Option<u8> {
    match a {
        Al::Span => Some(END_SPAN),
        Al::Cr | Al::FrozenCr => Some(END_CR),
        Al::Crcr => Some(END_CRCR),
        _ => None,
    }
}

/// A preamble's tab or `\cr` (`\span` never comes: it is expanded).
fn preamble_code(m: &Meaning) -> Option<u8> {
    match m {
        Meaning::Char(4, _) => Some(END_TAB),
        Meaning::Prim(Prim::Align(a)) => end_code(*a).filter(|&c| c != END_SPAN),
        _ => None,
    }
}

fn is_spacer(m: &Meaning) -> bool {
    matches!(m, Meaning::Char(10, _))
}

fn tab_glue((g, zero): (Arc<Glue>, bool)) -> Item {
    Item::Glue {
        g,
        sub: TAB_SKIP + 1,
        zero,
    }
}

/// The glue totals of a list, by order (§656, §671).
fn totals(list: &[Item]) -> ([i32; 4], [i32; 4]) {
    let (mut st, mut sh) = ([0; 4], [0; 4]);
    for it in list {
        if let Item::Glue { g, .. } = it {
            st[usize::from(g.stretch_order).min(3)] += g.stretch;
            sh[usize::from(g.shrink_order).min(3)] += g.shrink;
        }
    }
    (st, sh)
}

/// §659, §665: the highest order with a nonzero total.
fn order(t: &[i32; 4]) -> u8 {
    if t[3] != 0 {
        3
    } else if t[2] != 0 {
        2
    } else {
        u8::from(t[1] != 0)
    }
}

/// §799: a row packed, as an unset box (its glue zero).
fn unset_row(b: BoxNode, inter: Option<Arc<crate::item::Interline>>) -> Unset {
    Unset {
        vertical: b.vertical,
        w: b.w,
        h: b.h,
        d: b.d,
        list: b.list,
        inter,
        ..Unset::default()
    }
}

/// The alignment's vertical list as its gatherer keeps it: removals
/// applied (the last item, past `\prevdepth` settings; tex.web 1105),
/// boxes to append and `\prevdepth` settings kept for the enclosing list.
fn push_align_v(list: &mut Vec<Item>, it: Item) {
    let Item::Remove(k) = it else {
        list.push(it);
        return;
    };
    if let Some(i) = list.iter().rposition(|x| !matches!(x, Item::PrevDepth(_))) {
        let hit = match &list[i] {
            Item::Penalty(_) => k == 0,
            Item::Kern { .. } => k == 1,
            Item::Glue { .. } => k == 2,
            _ => false,
        };
        if hit {
            list.remove(i);
        }
    }
}

/// §810, §811: entry `r` of the row `q` set to size `w`, as if its size
/// were `t` (with the columns it spans).
fn set_entry(r: &Unset, q: &Unset, t: i32, w: i32, valign: bool) -> BoxNode {
    let d = if valign { r.h } else { r.w };
    let mut glue = GlueSet::default();
    if t > d {
        glue.sign = 1;
        glue.order = r.stretch_order;
        glue.ratio = if r.stretch == 0 {
            0.0
        } else {
            f64::from(t - d) / f64::from(r.stretch)
        };
    } else if t < d {
        glue.order = r.shrink_order;
        glue.sign = -1;
        glue.ratio = if r.shrink == 0 {
            0.0
        } else if glue.order == 0 && d - t > r.shrink {
            1.0
        } else {
            f64::from(d - t) / f64::from(r.shrink)
        };
    }
    if valign {
        BoxNode {
            vertical: true,
            w: q.w,
            h: w,
            d: r.d,
            shift: 0,
            list: r.list.clone(),
            glue,
        }
    } else {
        BoxNode {
            vertical: false,
            w,
            h: q.h,
            d: q.d,
            shift: 0,
            list: r.list.clone(),
            glue,
        }
    }
}

/// §807–§809: the unset row `q` set to the prototype `p`, whose columns
/// are `widths` and its tabskip glue `tabs`; shifted `o` in a display.
fn set_row(
    q: &Unset,
    p: &BoxNode,
    widths: &[i32],
    tabs: &[(Arc<Glue>, bool)],
    valign: bool,
    o: i32,
) -> BoxNode {
    let mut row = BoxNode {
        vertical: valign,
        w: q.w,
        h: q.h,
        d: q.d,
        shift: o,
        list: Vec::with_capacity(q.list.len()),
        glue: p.glue,
    };
    if valign {
        row.h = p.h;
    } else {
        row.w = p.w;
    }
    let mut col = 0usize;
    for it in &q.list {
        let Item::Unset(r) = it else {
            row.list.push(it.clone());
            continue;
        };
        let w = widths.get(col).copied().unwrap_or(0);
        let mut t = w;
        let mut blanks = Vec::new();
        for _ in 0..r.span {
            // (§809: the tabskip glue and an empty box for each column
            // spanned)
            col += 1;
            let Some((v, z)) = tabs.get(col).cloned() else {
                break;
            };
            t += v.width;
            if p.glue.sign == 1 {
                if v.stretch_order == p.glue.order {
                    t += zround(p.glue.ratio * f64::from(v.stretch));
                }
            } else if p.glue.sign == -1 && v.shrink_order == p.glue.order {
                t -= zround(p.glue.ratio * f64::from(v.shrink));
            }
            blanks.push(Item::Glue {
                g: v,
                sub: TAB_SKIP + 1,
                zero: z,
            });
            let ws = widths.get(col).copied().unwrap_or(0);
            t += ws;
            blanks.push(Item::Box(Arc::new(if valign {
                BoxNode {
                    vertical: true,
                    h: ws,
                    ..BoxNode::default()
                }
            } else {
                BoxNode {
                    w: ws,
                    ..BoxNode::default()
                }
            })));
        }
        row.list
            .push(Item::Box(Arc::new(set_entry(r, q, t, w, valign))));
        row.list.extend(blanks);
        col += 1;
    }
    row
}

/// A group entry.
fn group(kind: GroupKind, align: Option<AlignSpec>, at: u32) -> Group {
    Group {
        kind,
        after: Vec::new(),
        boxg: None,
        align: align.map(Arc::new),
        at,
    }
}

impl Exec<'_> {
    // ------------------------------------------------------------ the input

    /// What an alignment does with a token as it is read (tex.web 342,
    /// 347, 357): braces counted; the end of a u template sets
    /// `align_state` to 0 (§324); a tab or `\cr` with `align_state` 0 ends
    /// the entry: its v template is inserted (§789) and read instead.
    pub(super) fn align_token(&mut self, mut t: Tok) -> Option<Tok> {
        loop {
            match &t {
                Tok::Char(1, _) => self.st.al.state += 1,
                Tok::Char(2, _) => self.st.al.state -= 1,
                Tok::Cs(n) if &**n == END_U => {
                    if self.st.al.state > 500_000 {
                        self.st.al.state = 0;
                    } else {
                        self.interwoven();
                    }
                    t = self.next_raw0()?;
                    continue;
                }
                _ => {}
            }
            if self.st.al.state == 0
                && let Some(code) = self.tab_code(&t)
            {
                self.insert_v(code);
                t = self.next_raw0()?;
                continue;
            }
            return Some(t);
        }
    }

    /// Whether a token is a tab or `\cr` (`tab_mark`, `car_ret`), and
    /// which.
    fn tab_code(&mut self, t: &Tok) -> Option<u8> {
        match t {
            Tok::Char(4, _) => Some(END_TAB),
            Tok::Cs(_) | Tok::Active(_) => match self.tok_meaning(t) {
                Meaning::Char(4, _) => Some(END_TAB),
                Meaning::Prim(Prim::Align(a)) => end_code(a),
                _ => None,
            },
            _ => None,
        }
    }

    /// §789: the entry ended by `code`: its v template goes in.
    fn insert_v(&mut self, code: u8) {
        let Some(a) = self.cur_align() else { return };
        let s = &*a;
        let v = if self.st.al.omit {
            s.omit.clone()
        } else {
            s.template(usize::from(self.st.al.col)).v.clone()
        };
        self.st.al.end = code;
        self.st.pending.push((v, 0, false));
        self.st.al.state = ALIGN_BASE;
    }

    /// The innermost alignment's group entry.
    fn cur_align(&self) -> Option<Arc<AlignSpec>> {
        let v = self.st.groups.0.as_ref()?;
        v.iter().rev().find_map(|g| g.align.clone())
    }

    // ------------------------------------------------------------ commands

    /// An alignment command, or one of the reader's own steps.
    pub(super) fn align_cmd(&mut self, a: Al) {
        let t = self.cmd.clone();
        match a {
            Al::Halign => match self.st.mode {
                Mode::H => self.head_for_vmode(&t, false),
                Mode::V => self.init_align(false, None),
                Mode::M => {
                    if let Some(pd) = self.display_for_alignment(&t) {
                        self.init_align(false, Some(pd));
                    }
                }
            },
            Al::Valign => match self.st.mode {
                Mode::V => self.back_new_graf(&t),
                Mode::H => self.init_align(true, None),
                Mode::M => self.insert_dollar_sign(&t),
            },
            Al::Noalign => self.misplaced("noalign", &HELP_NOALIGN),
            Al::Omit => self.misplaced("omit", &HELP_OMIT),
            Al::Span | Al::Cr | Al::Crcr | Al::FrozenCr => self.align_error(&t),
            Al::Pre => self.preamble(),
            Al::Peek => self.align_peek(),
            Al::Col => self.align_col(),
            Al::EndV => self.do_endv(),
            Al::EndU => {}
            Al::Pack => self.pack_entry(),
            Al::Next => {
                // (§791: the next entry's group)
                self.st.groups.push(group(GroupKind::Align, None, self.chain.nodes.len() as u32));
                self.kind = Kind::Open;
                let p = self.cs(if self.st.al.end >= END_CR { ROW } else { COL });
                self.back(p);
            }
            Al::Row => self.fin_row(),
            Al::Fin => {
                // (§800: the alignment's group closes)
                self.closes();
                let p = self.cs(SET);
                self.back(p);
            }
            Al::Set => self.fin_align(),
        }
    }

    // ------------------------------------------------------------ errors

    /// TeX's error in batch mode (tex.web 72–90, 311–318): the message,
    /// the context — `levels`, the input levels over the current line, top
    /// first (a label, what was read, what was not), as many as
    /// `\errorcontextlines` lets through (§311) — then the current line,
    /// and the help.
    fn tex_error(&mut self, msg: &str, levels: &[(&str, String, String)], help: &[&str]) {
        if self.halted() {
            return;
        }
        let e = self.int_par(IntPar::Errorcontextlines);
        let mut s = String::new();
        s.push(RAW);
        s.push(NL);
        s.push_str("! ");
        s.push_str(msg);
        s.push('.');
        // (§311: `nn` counts the levels shown after the first)
        let mut nn = -1;
        for (k, (label, read, unread)) in levels.iter().enumerate() {
            if k == 0 || nn < e {
                s.push(NL);
                s.push_str(label);
                for part in [read.as_str(), unread.as_str()] {
                    s.push(CTX);
                    s.push_str(part);
                }
                s.push(CTX);
                nn += 1;
            } else if nn == e {
                s.push(NL);
                s.push_str("...");
                nn += 1;
            }
        }
        let (read, unread) = self.file_line();
        s.push(NL);
        s.push_str("l.");
        s.push(LINE);
        s.push('L');
        s.push(LINE);
        s.push(' ');
        for part in [read.as_str(), unread.as_str()] {
            s.push(CTX);
            s.push_str(part);
        }
        s.push(CTX);
        if self.count_error() {
            // (§82: the hundredth, without its help)
            s.push(NL);
            s.push_str(THAT_MAKES_100);
            s.push(HALT);
        } else {
            for h in help {
                s.push(NL);
                s.push_str(h);
            }
            s.push(LN);
            s.push(LN);
        }
        self.log_raw(&s);
    }

    /// The line being read, split where the reader is (tex.web 318; a
    /// line's end read is still that line's).
    fn file_line(&self) -> (String, String) {
        let text = self.txt();
        let pos = self.st.pos.min(text.len());
        let anchor = if pos > 0 && text[pos - 1] == '\r' {
            pos - 1
        } else {
            pos
        };
        let start = text[..anchor]
            .iter()
            .rposition(|&c| c == '\r')
            .map_or(0, |k| k + 1);
        let end = text[start..]
            .iter()
            .position(|&c| c == '\r')
            .map_or(text.len(), |k| start + k);
        let split = pos.clamp(start, end);
        (
            text[start..split].iter().collect(),
            text[split..end].iter().collect(),
        )
    }

    /// Tokens as a context line shows them (the reader's own names as
    /// TeX's frozen ones: `\endtemplate`, `\cr`).
    fn toks_text(&mut self, toks: &[Tok]) -> String {
        let shown: Vec<Tok> = toks
            .iter()
            .map(|t| match t {
                Tok::Cs(n) if n.starts_with('\0') => Tok::Cs(Arc::from(&n[1..])),
                _ => t.clone(),
            })
            .collect();
        let mut s = String::new();
        self.show_toks(&mut s, &shown);
        s
    }

    fn interwoven(&mut self) {
        self.err("(interwoven alignment preambles are not allowed)".into());
    }

    /// §1129: `\noalign`, `\omit` out of place.
    fn misplaced(&mut self, name: &str, help: &[&str]) {
        let mut msg = String::from("Misplaced ");
        self.print_esc(&mut msg, name);
        self.tex_error(&msg, &[], help);
    }

    /// §1127: `align_error` — a tab or `\cr` that ends no entry.
    pub(super) fn align_error(&mut self, t: &Tok) {
        let st = self.st.al.state;
        if st.abs() > 2 {
            // (§1128)
            let mut msg = String::from("Misplaced ");
            match self.tok_meaning(t) {
                Meaning::Prim(Prim::Align(Al::Span)) => self.print_esc(&mut msg, "span"),
                Meaning::Prim(Prim::Align(Al::Cr | Al::FrozenCr)) => self.print_esc(&mut msg, "cr"),
                Meaning::Prim(Prim::Align(Al::Crcr)) => self.print_esc(&mut msg, "crcr"),
                Meaning::Char(_, c) => {
                    msg.push_str("alignment tab character ");
                    msg.push(c);
                }
                _ => {}
            }
            let help: &[&str] = if *t == Tok::Char(4, '&') {
                &HELP_TAB
            } else {
                &HELP_TAB_CR
            };
            self.tex_error(&msg, &[], help);
            return;
        }
        let tt = self.toks_text(std::slice::from_ref(t));
        self.back(t.clone());
        let (msg, brace) = if st < 0 {
            self.st.al.state += 1;
            ("Missing { inserted", Tok::Char(1, '{'))
        } else {
            self.st.al.state -= 1;
            ("Missing } inserted", Tok::Char(2, '}'))
        };
        let bt = self.toks_text(std::slice::from_ref(&brace));
        // (`ins_error`: the brace goes in before the token)
        self.back(brace);
        self.tex_error(
            msg,
            &[
                ("<inserted text> ", String::new(), bt),
                ("<to be read again> ", String::new(), tt),
            ],
            &HELP_FIX,
        );
    }

    /// §1132: a `}` ending an entry: `\cr` goes in before it.
    pub(super) fn missing_cr(&mut self, t: &Tok) {
        let tt = self.toks_text(std::slice::from_ref(t));
        self.back(t.clone());
        let cr = self.cs(FROZEN_CR);
        self.st.pending.push((Arc::from(vec![cr]), 0, false));
        let mut msg = String::from("Missing ");
        self.print_esc(&mut msg, "cr");
        msg.push_str(" inserted");
        let mut c = String::new();
        self.print_esc(&mut c, "cr ");
        self.tex_error(
            &msg,
            &[
                ("<inserted text> ", String::new(), c),
                ("<to be read again> ", String::new(), tt),
            ],
            &HELP_MISSING_CR,
        );
    }

    // ------------------------------------------------------------ the preamble

    /// §774–§776: `\halign` or `\valign` (in a display, `display` is the
    /// depth of the vertical list before it: §775): an `Open` node; the
    /// preamble is read by the next.
    fn init_align(&mut self, valign: bool, display: Option<i32>) {
        let omit = self.cs(END_TEMPLATE);
        let mut spec = AlignSpec {
            valign,
            spec: Spec::NATURAL,
            cols: Vec::new(),
            tabskips: Vec::new(),
            looping: None,
            omit: Arc::from(vec![omit]),
            level: self.st.level,
            mode: self.st.mode,
            inner: self.st.inner,
            space_factor: self.st.space_factor,
            prev_depth: self.st.prev_depth,
            prev_graf: self.st.prev_graf,
            hyph_lang: self.st.hyph_lang,
            outer: self.st.al,
            display: display.is_some(),
        };
        self.st.al = AlignCur {
            on: true,
            state: -ALIGN_BASE,
            ..AlignCur::OFF
        };
        // (§775: the alignment's list, internal; in a display, vertical,
        // with the depth of the vertical list before it)
        if let Some(pd) = display {
            self.st.prev_depth = pd;
            self.st.mode = Mode::V;
        }
        self.st.level += 1;
        self.st.inner = true;
        self.st.prev_graf = 0;
        // (§645: `scan_spec`)
        spec.spec = if self.keyword("to") {
            Spec::Exactly(self.scan_dimen())
        } else if self.keyword("spread") {
            Spec::Additional(self.scan_dimen())
        } else {
            Spec::NATURAL
        };
        self.st.groups.push(group(GroupKind::Align, Some(spec), self.chain.nodes.len() as u32));
        self.kind = Kind::Open;
        self.scan_left_brace();
        let p = self.cs(PRE);
        self.back(p);
    }

    /// §782: `get_preamble_token` — `\span` expands the token after it,
    /// `\tabskip` assignments are made (in the alignment's group: TeX's
    /// `eq_define`, or global under `\globaldefs`); `assigned` is then
    /// whether its value is TeX's shared zero glue.
    fn preamble_token(&mut self, assigned: &mut Option<bool>) -> Option<(Tok, Meaning)> {
        loop {
            let mut t = self.next_raw()?;
            let mut m = self.raw_meaning_of(&t);
            while m == Meaning::Prim(Prim::Align(Al::Span)) {
                t = self.next_raw()?;
                m = self.raw_meaning_of(&t);
                if t.key().is_some() && super::expand::expands(&m) {
                    self.expand(&t, &m);
                    t = self.next_raw()?;
                    m = self.raw_meaning_of(&t);
                }
            }
            if m == Meaning::Prim(Prim::Align(Al::EndV)) {
                self.interwoven();
            }
            if m == Meaning::Prim(Prim::GluePar(GluePar::Tabskip)) {
                self.optional_equals();
                let g = self.scan_glue(false);
                let shared = self.scratch.glue_shared;
                let gd = self.int_par(IntPar::Globaldefs);
                self.define(
                    Key::GluePar(GluePar::Tabskip),
                    Val::Glue(Arc::new(g)),
                    gd > 0,
                );
                *assigned = Some(shared && is_zero(&g));
                continue;
            }
            return Some((t, m));
        }
    }

    /// A token's meaning as an unexpanded read sees it (`\relax` after
    /// `\noexpand`).
    fn raw_meaning_of(&mut self, t: &Tok) -> Meaning {
        let m = self.tok_meaning(t);
        if self.noexp && super::expand::expands(&m) {
            Meaning::Prim(Prim::Relax)
        } else {
            m
        }
    }

    /// §777–§784: the preamble, read once into the alignment's group
    /// entry; then the entries' group opens (this node), `\everycr` and
    /// `align_peek` (§774).
    fn preamble(&mut self) {
        self.kind = Kind::Open;
        let endu = self.cs(END_U);
        let endt = self.cs(END_TEMPLATE);
        self.st.al.state = -ALIGN_BASE;
        let mut cols: Vec<Template> = Vec::new();
        let mut tabskips: Vec<(Arc<Glue>, bool)> = Vec::new();
        let mut looping: Option<u16> = None;
        let mut assigned: Option<bool> = None;
        let mut done = false;
        loop {
            // (§778)
            let g = self.glue_par(GluePar::Tabskip);
            let zero = assigned.unwrap_or_else(|| is_zero(&g));
            tabskips.push((g, zero));
            if done {
                break;
            }
            // (§783: the u template)
            let mut u: Vec<Tok> = Vec::new();
            loop {
                let Some((t, m)) = self.preamble_token(&mut assigned) else {
                    self.err("File ended while scanning a preamble".into());
                    done = true;
                    break;
                };
                if matches!(m, Meaning::Char(6, _)) {
                    break;
                }
                if let Some(code) = preamble_code(&m)
                    && self.st.al.state == -ALIGN_BASE
                {
                    if u.is_empty() && looping.is_none() && code == END_TAB {
                        looping = Some(cols.len() as u16);
                        continue;
                    }
                    let text = self.toks_text(std::slice::from_ref(&t));
                    self.back(t);
                    self.tex_error(
                        "Missing # inserted in alignment preamble",
                        &[("<to be read again> ", String::new(), text)],
                        &HELP_SHARP_NONE,
                    );
                    break;
                }
                if !is_spacer(&m) || !u.is_empty() {
                    u.push(t);
                }
            }
            u.push(endu.clone());
            // (§784: the v template)
            let mut v: Vec<Tok> = Vec::new();
            while !done {
                let Some((t, m)) = self.preamble_token(&mut assigned) else {
                    self.err("File ended while scanning a preamble".into());
                    done = true;
                    break;
                };
                if let Some(code) = preamble_code(&m)
                    && self.st.al.state == -ALIGN_BASE
                {
                    done = code >= END_CR;
                    break;
                }
                if matches!(m, Meaning::Char(6, _)) {
                    self.tex_error("Only one # is allowed per tab", &[], &HELP_SHARP_MORE);
                    continue;
                }
                v.push(t);
            }
            v.push(endt.clone());
            cols.push(Template {
                u: Arc::from(u),
                v: Arc::from(v),
            });
        }
        if let Some(g) = self.st.groups.last_mut()
            && let Some(a) = &mut g.align
        {
            let s = Arc::make_mut(a);
            s.cols = cols;
            s.tabskips = tabskips;
            s.looping = looping;
        }
        self.st.groups.push(group(GroupKind::Align, None, self.chain.nodes.len() as u32));
        let every = self.toks_par(TokPar::Everycr);
        self.rewrite(every);
        let p = self.cs(PEEK);
        self.back(p);
    }

    // ------------------------------------------------------------ rows and entries

    /// §785: `align_peek` — after the preamble, a row or a `\noalign`:
    /// `\noalign`, the end (`}`), `\crcr` (ignored), or a row's first entry
    /// (§786: `init_row`, its first tabskip glue).
    fn align_peek(&mut self) {
        let Some(a) = self.cur_align() else { return };
        let s = &*a;
        loop {
            self.st.al.state = ALIGN_BASE;
            let Some((t, m)) = self.nonblank_x() else {
                self.err("File ended inside an alignment".into());
                return;
            };
            match m {
                Meaning::Prim(Prim::Align(Al::Noalign)) => {
                    self.scan_left_brace();
                    self.st.groups.push(group(GroupKind::NoAlign, None, self.chain.nodes.len() as u32));
                    self.kind = Kind::Open;
                    if !s.valign {
                        let p = self.cs(NORMAL_PARAGRAPH);
                        self.back(p);
                    }
                }
                Meaning::Char(2, _) => {
                    // (§800: the entries' group closes, then the alignment's)
                    let after = self.st.groups.pop().map(|g| g.after).unwrap_or_default();
                    self.closes();
                    self.rewrite(after);
                    let p = self.cs(FIN);
                    self.back(p);
                }
                Meaning::Prim(Prim::Align(Al::Crcr)) => continue,
                _ => {
                    self.st.level = s.level + 2;
                    self.st.inner = true;
                    self.st.prev_graf = 0;
                    if s.valign {
                        self.st.mode = Mode::V;
                        self.st.prev_depth = 0;
                    } else {
                        self.st.mode = Mode::H;
                        self.st.space_factor = 0;
                    }
                    self.emit_here(tab_glue(s.tabskips[0].clone()));
                    self.st.al.col = 0;
                    self.back(t);
                    let p = self.cs(COL);
                    self.back(p);
                }
            }
            return;
        }
    }

    /// §787–§788: an entry begins (`init_span`, `init_col`): its list, and
    /// its first token — `\omit`, or what follows the u template.
    fn align_col(&mut self) {
        let Some(a) = self.cur_align() else { return };
        let s = &*a;
        self.st.level = s.level + 3;
        self.st.inner = true;
        self.st.prev_graf = 0;
        self.st.al.span = self.st.al.col;
        if s.valign {
            self.st.mode = Mode::V;
            self.st.prev_depth = IGNORE_DEPTH;
            self.normal_paragraph();
        } else {
            self.st.mode = Mode::H;
            self.st.space_factor = 1000;
        }
        self.st.al.state = ALIGN_BASE;
        let Some((t, m)) = self.nonblank_x() else {
            return;
        };
        self.init_col(s, t, &m);
    }

    /// §788: `init_col`.
    fn init_col(&mut self, s: &AlignSpec, t: Tok, m: &Meaning) {
        let omit = *m == Meaning::Prim(Prim::Align(Al::Omit));
        self.st.al.omit = omit;
        if omit {
            self.st.al.state = 0;
        } else {
            self.back(t);
            let u = s.template(usize::from(self.st.al.col)).u.clone();
            self.st.pending.push((u, 0, false));
        }
    }

    /// §1131, §791–§792: `\endtemplate` — the entry ends (a `Close` node;
    /// its packaging is the next node's), or with `\span` it goes on in the
    /// next column. Past the preamble's last column, without a loop, the
    /// tab is an error and ends the row.
    fn do_endv(&mut self) {
        let t = self.cmd.clone();
        if self.st.groups.last().map(|g| g.kind) != Some(GroupKind::Align) {
            self.err("An entry's end inside a group of its own is not supported yet".into());
            return;
        }
        if self.st.mode == Mode::H && !self.st.inner {
            // (`end_graf`: a paragraph in a `\valign` entry ends first)
            let e = self.cs(END_GRAF);
            self.back(t);
            self.back(e);
            return;
        }
        let Some(a) = self.cur_align() else { return };
        let s = &*a;
        if self.st.al.state < 500_000 {
            self.interwoven();
        }
        let next = usize::from(self.st.al.col) + 1;
        if s.col_of(next).is_none() && self.st.al.end < END_CR {
            self.extra_tab(s);
            self.st.al.end = END_CR;
        }
        if self.st.al.end == END_SPAN {
            self.st.al.state = ALIGN_BASE;
            let Some((t, m)) = self.nonblank_x() else {
                return;
            };
            self.st.al.col = next as u16;
            self.init_col(s, t, &m);
        } else {
            let after = self.st.groups.pop().map(|g| g.after).unwrap_or_default();
            self.closes();
            self.rewrite(after);
            let p = self.cs(PACK);
            self.back(p);
        }
    }

    /// §792: more entries than columns. Its context: the v template read
    /// (and on top, if a character's lookahead read `\endtemplate`, the
    /// token TeX put back for it: §375).
    fn extra_tab(&mut self, s: &AlignSpec) {
        let v = if self.st.al.omit {
            s.omit.clone()
        } else {
            s.template(usize::from(self.st.al.col)).v.clone()
        };
        let vt = self.toks_text(&v);
        let mut msg = String::from("Extra alignment tab has been changed to ");
        self.print_esc(&mut msg, "cr");
        let nodes = &self.chain.nodes;
        let after_chars = nodes
            .len()
            .checked_sub(1)
            .is_some_and(|i| matches!(nodes[i].cmd, Tok::Char(11 | 12, _)));
        let mut levels = Vec::new();
        if after_chars {
            let mut e = String::new();
            self.print_esc(&mut e, "endtemplate ");
            levels.push(("<recently read> ", e, String::new()));
        }
        levels.push(("<template> ", vt, String::new()));
        self.tex_error(&msg, &levels, &HELP_EXTRA_TAB);
    }

    /// §796, §795: the entry the `Close` node before ended, gathered from
    /// what its nodes emitted, packaged into an unset box, emitted into the
    /// row with the tabskip glue after its last column (a `\halign` entry's
    /// marks go out after the row: §799).
    fn pack_entry(&mut self) {
        self.kind = Kind::Par;
        let Some(a) = self.cur_align() else { return };
        let s = &*a;
        let cell = s.level + 3;
        let chain = self.chain;
        let end = chain.nodes.len().saturating_sub(1);
        let start = box_start(chain, end, cell);
        let mut list = Vec::new();
        let mut pd = IGNORE_DEPTH;
        for n in chain.nodes.range(start, end) {
            if n.out.level != cell {
                continue;
            }
            if s.valign {
                for it in n.vemit() {
                    push_vertical(&mut list, &mut pd, it.clone());
                }
            } else {
                for it in n.emit.iter() {
                    push_horizontal(&mut list, it.clone());
                }
            }
        }
        let ships: Vec<Item> = list
            .iter()
            .filter(|i| matches!(i, Item::Shipout(_)))
            .cloned()
            .collect();
        list.retain(|i| !matches!(i, Item::Shipout(_) | Item::BuildPage(_) | Item::End(_)));
        let (st, sh) = totals(&list);
        let (so, ho) = (order(&st), order(&sh));
        let mut adj = Vec::new();
        let b = if s.valign {
            vpack(list, Spec::NATURAL, 0, &Params::QUIET).b
        } else {
            hpack(list, Spec::NATURAL, &Params::QUIET, Some(&mut adj)).b
        };
        let u = Unset {
            vertical: s.valign,
            w: b.w,
            h: b.h,
            d: b.d,
            span: self.st.al.col - self.st.al.span,
            col: self.st.al.span,
            stretch: st[usize::from(so)],
            stretch_order: so,
            shrink: sh[usize::from(ho)],
            shrink_order: ho,
            list: b.list,
            inter: None,
        };
        // (back in the row)
        self.st.level = s.level + 2;
        self.st.inner = true;
        self.st.prev_graf = 0;
        self.st.hyph_lang = s.hyph_lang;
        if s.valign {
            self.st.mode = Mode::V;
            self.st.prev_depth = 0;
        } else {
            self.st.mode = Mode::H;
            self.st.space_factor = 0;
        }
        self.emit_here(Item::Unset(Arc::new(u)));
        for it in adj.into_iter().chain(ships) {
            self.emit_here(it);
        }
        self.emit_here(tab_glue(s.tab_after(usize::from(self.st.al.col))));
        self.st.al.col += 1;
        let p = self.cs(NEXT);
        self.back(p);
    }

    /// §799: `fin_row` — the row, gathered from what its nodes emitted,
    /// packaged into an unset row in the alignment's list (a `\halign`
    /// row appended to a vertical list: what that reads is kept with it);
    /// then `\everycr` and `align_peek`.
    fn fin_row(&mut self) {
        self.kind = Kind::Par;
        let Some(a) = self.cur_align() else { return };
        let s = &*a;
        let row = s.level + 2;
        let chain = self.chain;
        let start = box_start(chain, chain.nodes.len(), row);
        let mut items = Vec::new();
        for n in chain.nodes.range(start, chain.nodes.len()) {
            if n.out.level == row {
                if s.valign {
                    items.extend(n.vemit().iter().cloned());
                } else {
                    items.extend(n.emit.iter().cloned());
                }
            }
        }
        let (row_items, moved): (Vec<Item>, Vec<Item>) = items
            .into_iter()
            .partition(|i| matches!(i, Item::Unset(_) | Item::Glue { .. }));
        self.st.level = s.level + 1;
        self.st.inner = true;
        self.st.prev_graf = 0;
        self.st.hyph_lang = s.hyph_lang;
        if s.valign {
            let b = vpack(row_items, Spec::NATURAL, MAX_DIMEN, &Params::QUIET).b;
            self.st.mode = Mode::H;
            self.st.space_factor = 1000;
            self.emit.push(Item::Unset(Arc::new(unset_row(b, None))));
            self.emit.extend(moved);
        } else {
            let b = hpack(row_items, Spec::NATURAL, &Params::QUIET, None).b;
            self.st.mode = Mode::V;
            let inter = self.interline();
            self.st.prev_depth = b.d;
            self.vemit
                .push(Item::Unset(Arc::new(unset_row(b, Some(inter)))));
            self.vemit.extend(moved);
        }
        let every = self.toks_par(TokPar::Everycr);
        self.rewrite(every);
        let p = self.cs(PEEK);
        self.back(p);
    }

    /// §1133: the `}` of a `\noalign`: a paragraph in it ends first, the
    /// group closes, `align_peek`.
    pub(super) fn noalign_close(&mut self, t: &Tok) {
        if self.st.mode == Mode::H && !self.st.inner {
            let e = self.cs(END_GRAF);
            self.back(t.clone());
            self.back(e);
            return;
        }
        let after = self.st.groups.pop().map(|g| g.after).unwrap_or_default();
        self.closes();
        self.rewrite(after);
        let p = self.cs(PEEK);
        self.back(p);
    }

    // ------------------------------------------------------------ the end

    /// §800–§812: `fin_align`, after the alignment's groups closed: the
    /// alignment's list gathered from what its nodes emitted; the column
    /// widths (§801–§803); the preamble packed (§804); every row and entry
    /// set (§805–§811); the list into its environment (§812) — or, in a
    /// display, kept for its closing `$$` (§1206).
    fn fin_align(&mut self) {
        self.kind = Kind::Par;
        let Some(g) = self.st.groups.pop() else {
            return;
        };
        let Some(a) = g.align.clone() else { return };
        let s = &*a;
        let list_level = s.level + 1;
        let chain = self.chain;
        let end = chain.nodes.len().saturating_sub(1);
        let start = box_start(chain, end, list_level);
        let mut list: Vec<Item> = Vec::new();
        for n in chain.nodes.range(start, end) {
            if n.out.level != list_level {
                continue;
            }
            if s.valign {
                for it in n.emit.iter() {
                    push_horizontal(&mut list, it.clone());
                }
            } else {
                for it in n.vemit() {
                    push_align_v(&mut list, it.clone());
                }
            }
        }
        // (§801–§803: each column's widest entry; an entry spanning
        // columns widens the last of them by what the others lack)
        let mut ncols = s.cols.len();
        for it in &list {
            if let Item::Unset(q) = it {
                for e in &q.list {
                    if let Item::Unset(e) = e {
                        ncols = ncols.max(usize::from(e.col) + usize::from(e.span) + 1);
                    }
                }
            }
        }
        let mut width = vec![NULL_FLAG; ncols];
        let mut spans: Vec<Vec<(u16, i32)>> = vec![Vec::new(); ncols];
        for it in &list {
            let Item::Unset(q) = it else { continue };
            for e in &q.list {
                let Item::Unset(e) = e else { continue };
                let j = usize::from(e.col);
                let w = if s.valign { e.h } else { e.w };
                if e.span == 0 {
                    width[j] = width[j].max(w);
                } else {
                    match spans[j].binary_search_by_key(&e.span, |x| x.0) {
                        Ok(k) => spans[j][k].1 = spans[j][k].1.max(w),
                        Err(k) => spans[j].insert(k, (e.span, w)),
                    }
                }
            }
        }
        let mut tabs: Vec<(Arc<Glue>, bool)> = Vec::with_capacity(ncols + 1);
        tabs.push(s.tabskips[0].clone());
        for j in 0..ncols {
            tabs.push(s.tab_after(j));
        }
        for j in 0..ncols {
            if width[j] == NULL_FLAG {
                // (§802)
                width[j] = 0;
                tabs[j + 1] = (Arc::new(Glue::default()), true);
            }
            let sp = std::mem::take(&mut spans[j]);
            if sp.is_empty() || j + 1 >= ncols {
                continue;
            }
            let t = width[j] + tabs[j + 1].0.width;
            for (n, w) in sp {
                let w = w - t;
                if n == 1 {
                    if w > width[j + 1] {
                        width[j + 1] = w;
                    }
                } else {
                    match spans[j + 1].binary_search_by_key(&(n - 1), |x| x.0) {
                        Ok(k) => {
                            if w > spans[j + 1][k].1 {
                                spans[j + 1][k].1 = w;
                            }
                        }
                        Err(k) => spans[j + 1].insert(k, (n - 1, w)),
                    }
                }
            }
        }
        // (§804: the preamble, packed as the alignment asked)
        let mut pre = Vec::with_capacity(2 * ncols + 1);
        for j in 0..ncols {
            pre.push(tab_glue(tabs[j].clone()));
            let u = if s.valign {
                Unset {
                    vertical: true,
                    h: width[j],
                    ..Unset::default()
                }
            } else {
                Unset {
                    w: width[j],
                    ..Unset::default()
                }
            };
            pre.push(Item::Unset(Arc::new(u)));
        }
        pre.push(tab_glue(tabs[ncols].clone()));
        let packed = if s.valign {
            let params = Params {
                badness: self.int_par(IntPar::Vbadness),
                fuzz: self.dimen_par(DimenPar::Vfuzz),
                overfull_rule: 0,
            };
            vpack(pre, s.spec, MAX_DIMEN, &params)
        } else {
            let params = Params {
                badness: self.int_par(IntPar::Hbadness),
                fuzz: self.dimen_par(DimenPar::Hfuzz),
                overfull_rule: 0,
            };
            hpack(pre, s.spec, &params, None)
        };
        let p = packed.b;
        if let Some(r) = packed.report {
            let cx = self.show_ctx(std::slice::from_ref(&Item::Box(Arc::new(p.clone()))));
            let txt = report(r, &p, Place::Alignment, &cx);
            self.log_raw(&txt);
        }
        // (§805–§811)
        let o = if s.display {
            self.dimen_par(DimenPar::Displayindent)
        } else {
            0
        };
        let mut out = Vec::with_capacity(list.len());
        for it in list {
            match it {
                Item::Unset(q) => {
                    let b = Arc::new(set_row(&q, &p, &width, &tabs, s.valign, o));
                    out.push(match &q.inter {
                        Some(i) => Item::Append(b, i.clone()),
                        None => Item::Box(b),
                    });
                }
                Item::Rule { w, h, d } => {
                    // (§806)
                    let r = Item::Rule {
                        w: if w == RUNNING { p.w } else { w },
                        h: if h == RUNNING { p.h } else { h },
                        d: if d == RUNNING { p.d } else { d },
                    };
                    if o == 0 {
                        out.push(r);
                    } else {
                        let mut b = hpack(vec![r], Spec::NATURAL, &Params::QUIET, None).b;
                        b.shift = o;
                        out.push(Item::Box(Arc::new(b)));
                    }
                }
                other => out.push(other),
            }
        }
        // (§812: the enclosing list, its `aux` the alignment's)
        self.st.al = s.outer;
        self.st.level = s.level;
        self.st.inner = s.inner;
        self.st.mode = s.mode;
        self.st.prev_graf = s.prev_graf;
        self.st.hyph_lang = s.hyph_lang;
        if s.valign {
            self.st.prev_depth = s.prev_depth;
        } else {
            self.st.space_factor = s.space_factor;
        }
        self.rewrite(g.after);
        if s.display {
            let aux = self.st.prev_depth;
            self.display_aligned(out, aux);
            return;
        }
        for it in out {
            self.emit_here(it);
        }
        if self.st.level == 0 && self.st.mode == Mode::V {
            let c = self.page_call();
            self.vemit.push(Item::BuildPage(c));
        }
    }

    // ------------------------------------------------------------ displays

    /// §1206–§1207 (and §1197): after a display's alignment and its
    /// assignments, `$$`; `bad`, what came instead, is read again after
    /// "Missing $$ inserted".
    pub(super) fn display_alignment_check(&mut self, bad: Option<&Tok>) {
        if let Some(t) = bad {
            let tt = self.toks_text(std::slice::from_ref(t));
            self.back(t.clone());
            self.tex_error(
                "Missing $$ inserted",
                &[("<to be read again> ", String::new(), tt)],
                &HELP_DISPLAY,
            );
            return;
        }
        let x = self.next_x_m(false);
        if !matches!(x, Some((_, Meaning::Char(3, _)))) {
            let mut levels = Vec::new();
            if let Some((u, _)) = x {
                levels.push((
                    "<to be read again> ",
                    String::new(),
                    self.toks_text(std::slice::from_ref(&u)),
                ));
                self.back(u);
            }
            self.tex_error("Display math should end with $$", &levels, &HELP_DOLLAR);
        }
    }
}
