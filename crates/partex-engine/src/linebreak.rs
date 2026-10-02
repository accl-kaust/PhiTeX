//! Breaking paragraphs into lines (tex.web parts 38–40, §813–§918).
//!
//! [`line_break`] is a pure function: the paragraph's hlist and every
//! parameter it reads go in; the lines (each an hlist with the width and
//! shift §889 packs it to, and the interline penalties) and the events
//! TeX would print (`\tracingparagraphs`, errors, warnings) come out, in
//! TeX's order. The caller prints the events, then packs each line (§889,
//! a call of its own, DESIGN 7.17.2: `line_break`'s children), printing
//! its report, and appends it and what migrated out of it to its
//! vertical list.
//!
//! The active list keeps tex.web's structure (active and delta entries in
//! one linked list), since the order of its entries decides ties.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::Scaled;
use crate::expand::{self, ExpandEnv};
use crate::font::{LigKern, Tag};
use crate::hyph::{MAX_WORD, Patterns, exception_key, hyphen_values};
use crate::lr;
use crate::margin::{self, At};
use crate::node::{Disc, FontId, GlueSpec, Ligature, Node, Order, Whatsit, push_char};
use crate::pack;
use crate::scaled::{INF_BAD, MAX_DIMEN, badness, fract};

/// §157: `inf_penalty` and `eject_penalty`.
const INF_PENALTY: i32 = 10000;
const EJECT_PENALTY: i32 = -10000;
/// §833: `awful_bad`.
const AWFUL_BAD: i32 = 0o7777777777;
/// §110 (web2c): `max_halfword`.
const MAX_HALFWORD: i32 = 0xFFF_FFFF;
/// §549: `non_char`.
const NON_CHAR: i32 = 256;
/// §155: kern subtypes.
const KERN_NORMAL: u8 = 0;
const KERN_EXPLICIT: u8 = 1;
/// §224: glue parameter numbers (`subtype` is the number plus one).
const RIGHT_SKIP: u8 = 8;
const LEFT_SKIP: u8 = 7;
const PAR_FILL_SKIP: u8 = 14;

/// §817: fitness classes.
const VERY_LOOSE_FIT: usize = 0;
const LOOSE_FIT: usize = 1;
const DECENT_FIT: usize = 2;
const TIGHT_FIT: usize = 3;

/// The parameters line breaking reads (from eqtb and the current list).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Params {
    pub pretolerance: i32,
    pub tolerance: i32,
    pub emergency_stretch: Scaled,
    pub looseness: i32,
    pub line_penalty: i32,
    pub hyphen_penalty: i32,
    pub ex_hyphen_penalty: i32,
    pub adj_demerits: i32,
    pub double_hyphen_demerits: i32,
    pub final_hyphen_demerits: i32,
    pub inter_line_penalty: i32,
    pub club_penalty: i32,
    pub broken_penalty: i32,
    /// `\widowpenalty`, or `\displaywidowpenalty` before a display.
    pub final_widow_penalty: i32,
    /// e-TeX's `\interlinepenalties`, `\clubpenalties` and
    /// `\widowpenalties` (or `\displaywidowpenalties` before a display);
    /// empty if not set. They replace the single penalties.
    pub inter_line_penalties: Vec<i32>,
    pub club_penalties: Vec<i32>,
    pub widow_penalties: Vec<i32>,
    /// `\leftskip`: no glue is added for TeX's shared zero glue (§887
    /// compares pointers).
    pub left_skip: GlueSpec,
    pub right_skip: GlueSpec,
    pub par_fill_skip: GlueSpec,
    pub hsize: Scaled,
    pub hang_indent: Scaled,
    pub hang_after: i32,
    /// `\parshape` as (indent, width) pairs.
    pub par_shape: Vec<(Scaled, Scaled)>,
    /// `\prevgraf` of the enclosing list.
    pub prev_graf: i32,
    /// e-TeX's `\lastlinefit`.
    pub last_line_fit: i32,
    /// `TeXXeT`: the text-direction segments open at the start (closing
    /// subtypes, outermost first; `LR_save`).
    pub lr_open: Vec<u8>,
    /// The language, `\lefthyphenmin` and `\righthyphenmin` at the start.
    pub language: i32,
    pub left_hyphen_min: i32,
    pub right_hyphen_min: i32,
    pub uc_hyph: bool,
    pub tracing: bool,
    /// pdfTeX's `\pdfprotrudechars`: characters protrude into the margins
    /// (if positive), and line breaking allows for it (above 1).
    pub protrude_chars: i32,
    /// pdfTeX's `\pdfadjustspacing`: lines are packed with expanded fonts
    /// (if positive), and line breaking allows for it (above 1).
    pub adjust_spacing: i32,
    /// Packaging of the lines (`\hbadness`, `\hfuzz`, `\overfullrule`,
    /// `TeXXeT`), which the caller does (§889).
    pub pack: pack::Params,
    /// The pdfTeX bugs to reproduce.
    pub pdftex_bugs: crate::bugs::PdftexBugs,
}

/// What line breaking needs besides its parameters.
pub trait Env: ExpandEnv {
    /// `\lccode` of `c`.
    fn lc_code(&self, c: u8) -> u8;
    /// `\hyphenchar` of `f`.
    fn hyphen_char(&self, f: FontId) -> i32;
    fn patterns(&self) -> &Patterns;
    /// §930: the hyphen positions of the word `key`
    /// (`hyph::exception_key`), if it is an exception.
    #[allow(clippy::ptr_arg, reason = "the map's key, looked up as it is")]
    fn exception(&self, key: &Vec<u8>) -> Option<&[u8]>;
}

/// What a feasible break is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BreakKind {
    Par,
    Glue,
    Penalty,
    Discretionary,
    Kern,
    Math,
}

/// What TeX prints while breaking a paragraph, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The trace starts (`begin_diagnostic`, and `@firstpass` if `first`).
    Begin {
        first: bool,
    },
    SecondPass,
    EmergencyPass,
    /// The paragraph text since the last `Text` (§857, `short_display`).
    Text(Vec<Node>),
    /// §856
    Feasible {
        at: BreakKind,
        via: i32,
        badness: Option<i32>,
        penalty: i32,
        demerits: Option<i32>,
    },
    /// §846
    NewBreak {
        serial: i32,
        line: i32,
        fitness: u8,
        hyphenated: bool,
        total: i32,
        previous: i32,
        /// e-TeX's `\lastlinefit` data: the shortfall and the glue
        /// stretch or shrink, or the adjustment if the break ends the
        /// paragraph (`true`).
        last_fit: Option<(Scaled, Scaled, bool)>,
    },
    /// The trace ends (`end_diagnostic(true)`, `normalize_selector`).
    End,
    /// §826: "Infinite glue shrinkage found in a paragraph" (once).
    InfiniteShrink,
    /// §581: the hyphen character is not in the font.
    MissingChar {
        font: FontId,
        ch: i32,
    },
}

/// One item for the enclosing vertical list.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A line (§889): its hlist, to be packed to `width` (with
    /// `\pdfadjustspacing`, its fonts expanded to fit, `expand::hpack_line`)
    /// with its adjustment material migrating (§888), and shifted by
    /// `shift`.
    Line {
        list: Vec<Node>,
        width: Scaled,
        shift: Scaled,
    },
    Penalty(i32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Broken {
    pub items: Vec<Item>,
    pub events: Vec<Event>,
    /// The new `\prevgraf`.
    pub prev_graf: i32,
    /// `\leftskip` and `\rightskip` made finite by §827 (TeX assigns them).
    pub left_skip: Option<GlueSpec>,
    pub right_skip: Option<GlueSpec>,
    /// Whether the second pass ran (the one that hyphenates: TeX builds
    /// its pattern trie then, §891).
    pub second_pass: bool,
    /// `TeXXeT`: the segments still open at the end (see
    /// [`Params::lr_open`]).
    pub lr_open: Vec<u8>,
}

/// An internal inconsistency (TeX's `confusion`).
pub use crate::pack::Confusion;

/// §819, §822: an entry of the active list.
#[derive(Clone, Copy, Debug)]
enum Entry {
    Active {
        hyphenated: bool,
        fitness: usize,
        line_number: i32,
        total_demerits: i32,
        break_node: Option<usize>,
        /// e-TeX's `\lastlinefit`: `active_short`, `active_glue`.
        short: Scaled,
        glue: Scaled,
    },
    Delta(Widths),
}

/// §821
#[derive(Clone, Copy, Debug)]
struct Passive {
    cur_break: Option<usize>,
    prev_break: Option<usize>,
    serial: i32,
}

/// The active list: entries linked by index; entry 0 is `active` (and
/// `last_active`).
struct ActiveList {
    entries: Vec<(Entry, usize)>,
}

impl ActiveList {
    const HEAD: usize = 0;

    fn new() -> Self {
        // §820: `last_active` is hyphenated, line `max_halfword`.
        let head = Entry::Active {
            hyphenated: true,
            fitness: 0,
            line_number: MAX_HALFWORD,
            total_demerits: 0,
            break_node: None,
            short: 0,
            glue: 0,
        };
        let mut entries = Vec::with_capacity(32);
        entries.push((head, 0));
        Self { entries }
    }
    fn next(&self, i: usize) -> usize {
        self.entries[i].1
    }
    fn set_next(&mut self, i: usize, n: usize) {
        self.entries[i].1 = n;
    }
    fn entry(&self, i: usize) -> Entry {
        self.entries[i].0
    }
    fn insert_after(&mut self, prev: usize, e: Entry) -> usize {
        let i = self.entries.len();
        let next = self.next(prev);
        self.entries.push((e, next));
        self.set_next(prev, i);
        i
    }
    fn delta_mut(&mut self, i: usize) -> &mut Widths {
        match &mut self.entries[i].0 {
            Entry::Delta(d) => d,
            Entry::Active { .. } => unreachable!("not a delta entry"),
        }
    }
    fn is_delta(&self, i: usize) -> bool {
        matches!(self.entries[i].0, Entry::Delta(_))
    }
}

/// §823's width arrays: indices 1–6 as tex.web's; pdfTeX adds the
/// stretch (7) and shrink (8) of font expansion.
type Widths = [Scaled; 9];

/// The characters at the left and right margins of a line, if any, and
/// whether each is a ligature's.
type Margins = [Option<(FontId, u8, bool)>; 2];

/// A margin character of node `n`, as `char_pw` records it.
fn margin_char(n: Option<&Node>, left: bool) -> Option<(FontId, u8, bool)> {
    let n = n?;
    margin::char_at(n, left).map(|(f, c)| (f, c, matches!(n, Node::Ligature(_))))
}

/// `a += b` for the width arrays.
fn add_widths(a: &mut Widths, b: &Widths) {
    for (x, y) in a.iter_mut().zip(b).skip(1) {
        *x += y;
    }
}

/// `a -= b` for the width arrays.
fn sub_widths(a: &mut Widths, b: &Widths) {
    for (x, y) in a.iter_mut().zip(b).skip(1) {
        *x -= y;
    }
}

fn order_index(o: Order) -> usize {
    2 + o.index()
}

/// pdfTeX's state of font expansion while breaking a paragraph.
struct ExpState {
    /// `\pdfadjustspacing>1`: line breaking counts the fonts' stretch and
    /// shrink.
    spacing: bool,
    /// `cur_font_step`, `max_stretch_ratio`, `max_shrink_ratio` (-1 until
    /// the paragraph's first expandable font sets them).
    cur_font_step: i32,
    max_stretch_ratio: i32,
    max_shrink_ratio: i32,
}

/// pdfTeX's errors (`pdf_error("font expansion", …)`) for the fonts of a
/// paragraph that expand differently, which line breaking stops with.
pub const EXPANSION_ERRORS: [&str; 2] = [
    "using fonts with different step of expansion in one paragraph is not allowed",
    "using fonts with different limit of expansion in one paragraph is not allowed",
];

impl ExpState {
    /// pdfTeX's `check_expand_pars`: whether `f` is expandable (all the
    /// paragraph's expandable fonts must expand alike).
    fn check(&mut self, env: &impl ExpandEnv, f: FontId) -> Result<bool, Confusion> {
        let x = env.expansion(f);
        if x.step == 0 || (x.stretch.is_none() && x.shrink.is_none()) {
            return Ok(false);
        }
        if self.cur_font_step < 0 {
            self.cur_font_step = x.step;
        } else if self.cur_font_step != x.step {
            return Self::fail(EXPANSION_ERRORS[0]);
        }
        if let Some(k) = x.stretch {
            let r = env.expansion(k).ratio;
            if self.max_stretch_ratio < 0 {
                self.max_stretch_ratio = r;
            } else if self.max_stretch_ratio != r {
                return Self::fail(EXPANSION_ERRORS[1]);
            }
        }
        if let Some(k) = x.shrink {
            let r = -env.expansion(k).ratio;
            if self.max_shrink_ratio < 0 {
                self.max_shrink_ratio = r;
            } else if self.max_shrink_ratio != r {
                return Self::fail(EXPANSION_ERRORS[1]);
            }
        }
        Ok(true)
    }

    fn fail(m: &'static str) -> Result<bool, Confusion> {
        Err(Confusion(m))
    }

    /// The stretch and shrink of the characters `cs` of `f`, if line
    /// breaking counts them.
    fn chars(
        &mut self,
        env: &impl ExpandEnv,
        f: FontId,
        cs: &[u8],
    ) -> Result<(Scaled, Scaled), Confusion> {
        let (mut st, mut sh) = (0, 0);
        if self.spacing && self.check(env, f)? {
            for &c in cs {
                st += expand::char_stretch(env, f, c);
                sh += expand::char_shrink(env, f, c);
            }
        }
        Ok((st, sh))
    }

    /// The stretch and shrink of a kern of width `w` between `l` and `r`.
    fn kern(
        &self,
        env: &impl ExpandEnv,
        l: Option<(FontId, u8)>,
        w: Scaled,
        r: Option<(FontId, u8)>,
    ) -> (Scaled, Scaled) {
        if self.spacing {
            (
                expand::kern_change(env, l, w, r, true),
                expand::kern_change(env, l, w, r, false),
            )
        } else {
            (0, 0)
        }
    }

    /// The width of the nodes of a discretionary's text (§841, §842,
    /// §870, §871) in slot 1, their fonts' stretch and shrink in 7 and 8;
    /// `after` is the character after the last (as tex.web links it).
    fn part(
        &mut self,
        env: &impl ExpandEnv,
        part: &[Node],
        after: Option<(FontId, u8)>,
        what: &'static str,
    ) -> Result<Widths, Confusion> {
        let mut w = [0; 9];
        for (k, n) in part.iter().enumerate() {
            let (st, sh) = match n {
                Node::Glyphs(g) => {
                    w[1] += env.font(g.font).run_width(g.chars());
                    self.chars(env, g.font, g.chars())?
                }
                Node::Ligature(l) => {
                    w[1] += env.font(l.font).width(l.ch);
                    self.chars(env, l.font, &[l.ch])?
                }
                Node::Kern { width, subtype } => {
                    w[1] += width;
                    if *subtype == KERN_NORMAL {
                        let l = k
                            .checked_sub(1)
                            .and_then(|k| margin::char_at(&part[k], false));
                        let r = part.get(k + 1).map_or(after, |n| margin::char_at(n, true));
                        self.kern(env, l, *width, r)
                    } else {
                        (0, 0)
                    }
                }
                Node::Box(b) => {
                    w[1] += b.width;
                    (0, 0)
                }
                Node::Rule { width, .. } => {
                    w[1] += width;
                    (0, 0)
                }
                _ => return Err(Confusion(what)),
            };
            w[7] += st;
            w[8] += sh;
        }
        Ok(w)
    }
}

struct Breaker<'a, E: Env> {
    env: &'a mut E,
    p: &'a Params,
    list: Vec<Node>,
    events: Vec<Event>,
    active: ActiveList,
    passive: Vec<Passive>,
    /// The first node not yet shown in the trace.
    printed: usize,
    active_width: Widths,
    cur_active_width: Widths,
    background: Widths,
    break_width: Widths,
    no_shrink_error_yet: bool,
    cur_p: Option<usize>,
    second_pass: bool,
    final_pass: bool,
    threshold: i32,
    minimal_demerits: [i32; 4],
    minimum_demerits: i32,
    best_place: [Option<usize>; 4],
    best_pl_line: [i32; 4],
    /// §839's `disc_width` (1, and pdfTeX's 7 and 8).
    disc_width: Widths,
    exp: ExpState,
    easy_line: i32,
    last_special_line: i32,
    first_width: Scaled,
    second_width: Scaled,
    first_indent: Scaled,
    second_indent: Scaled,
    best_bet: usize,
    fewest_demerits: i32,
    best_line: i32,
    actual_looseness: i32,
    /// e-TeX's `\lastlinefit` in effect: `fill_width`, the infinite
    /// stretch of `\parfillskip` by order (`do_last_line_fit`).
    fill_width: Option<[Scaled; 3]>,
    best_pl_short: [Scaled; 4],
    best_pl_glue: [Scaled; 4],
    // §892: hyphenation state.
    cur_lang: i32,
    l_hyf: i32,
    r_hyf: i32,
}

/// §815: break the paragraph `list` (the contents of the horizontal list
/// that `\par` ends) into lines.
pub fn line_break(
    mut list: Vec<Node>,
    p: &Params,
    env: &mut impl Env,
) -> Result<Broken, Confusion> {
    // §816: get ready to start line breaking.
    match list.last_mut() {
        Some(last @ (Node::Glue { .. } | Node::Leaders(_))) => *last = Node::Penalty(INF_PENALTY),
        _ => list.push(Node::Penalty(INF_PENALTY)),
    }
    list.push(Node::Glue {
        spec: p.par_fill_skip,
        subtype: PAR_FILL_SKIP + 1,
    });
    let mut b = Breaker {
        env,
        p,
        list,
        events: Vec::new(),
        active: ActiveList::new(),
        passive: Vec::new(),
        printed: 0,
        active_width: [0; 9],
        cur_active_width: [0; 9],
        background: [0; 9],
        break_width: [0; 9],
        no_shrink_error_yet: true,
        cur_p: None,
        second_pass: false,
        final_pass: false,
        threshold: 0,
        minimal_demerits: [AWFUL_BAD; 4],
        minimum_demerits: AWFUL_BAD,
        best_place: [None; 4],
        best_pl_line: [0; 4],
        disc_width: [0; 9],
        exp: ExpState {
            spacing: p.adjust_spacing > 1,
            cur_font_step: -1,
            max_stretch_ratio: -1,
            max_shrink_ratio: -1,
        },
        easy_line: 0,
        last_special_line: 0,
        first_width: 0,
        second_width: 0,
        first_indent: 0,
        second_indent: 0,
        best_bet: 0,
        fewest_demerits: 0,
        best_line: 0,
        actual_looseness: 0,
        fill_width: None,
        best_pl_short: [0; 4],
        best_pl_glue: [0; 4],
        cur_lang: p.language,
        l_hyf: p.left_hyphen_min,
        r_hyf: p.right_hyphen_min,
    };
    // §827: get ready to start, making `\leftskip` and `\rightskip`
    // finite.
    let left = b.check_shrinkage(p.left_skip);
    let right = b.check_shrinkage(p.right_skip);
    let left_fixed = (left != p.left_skip).then_some(left);
    let right_fixed = (right != p.right_skip).then_some(right);
    let mut bg = [0; 9];
    bg[1] = left.width + right.width;
    bg[order_index(left.stretch_order)] = left.stretch;
    bg[order_index(right.stretch_order)] += right.stretch;
    bg[6] = left.shrink + right.shrink;
    b.background = bg;
    // e-TeX: check for special treatment of the last line of the
    // paragraph: \parfillskip must stretch infinitely, \leftskip and
    // \rightskip finitely.
    let fill = p.par_fill_skip;
    if p.last_line_fit > 0
        && fill.stretch > 0
        && fill.stretch_order != Order::Normal
        && bg[3..6] == [0, 0, 0]
    {
        let mut w = [0; 3];
        w[fill.stretch_order.index() - 1] = fill.stretch;
        b.fill_width = Some(w);
    }
    // §848: get ready to compute the line widths.
    if p.par_shape.is_empty() {
        if p.hang_indent == 0 {
            b.last_special_line = 0;
            b.second_width = p.hsize;
            b.second_indent = 0;
        } else {
            // §849: hanging indentation.
            b.last_special_line = p.hang_after.abs();
            let indent = p.hang_indent.max(0);
            if p.hang_after < 0 {
                b.first_width = p.hsize - p.hang_indent.abs();
                b.first_indent = indent;
                b.second_width = p.hsize;
                b.second_indent = 0;
            } else {
                b.first_width = p.hsize;
                b.first_indent = 0;
                b.second_width = p.hsize - p.hang_indent.abs();
                b.second_indent = indent;
            }
        }
    } else {
        let n = i32::try_from(p.par_shape.len()).unwrap_or(i32::MAX);
        b.last_special_line = n - 1;
        let (indent, width) = p.par_shape[p.par_shape.len() - 1];
        b.second_width = width;
        b.second_indent = indent;
    }
    b.easy_line = if p.looseness == 0 {
        b.last_special_line
    } else {
        MAX_HALFWORD
    };
    b.find_optimal_breakpoints()?;
    if b.fill_width.is_some()
        && let Entry::Active { short, glue, .. } = b.active.entry(b.best_bet)
        && short != 0
    {
        // e-TeX: adjust the \parfillskip of the final line.
        if let Some(Node::Glue { spec, .. }) = b.list.last_mut() {
            spec.width += short - glue;
            spec.stretch = 0;
            spec.shared_zero = false;
        }
    }
    let mut lr_open = p.lr_open.clone();
    let (items, prev_graf) = b.post_line_break(left, right, &mut lr_open)?;
    Ok(Broken {
        lr_open,
        items,
        events: b.events,
        prev_graf,
        left_skip: left_fixed,
        right_skip: right_fixed,
        second_pass: b.second_pass,
    })
}

impl<E: Env> Breaker<'_, E> {
    /// §825, §826: `check_shrinkage`.
    fn check_shrinkage(&mut self, g: GlueSpec) -> GlueSpec {
        if g.shrink_order != Order::Normal && g.shrink != 0 {
            if self.no_shrink_error_yet {
                self.no_shrink_error_yet = false;
                self.events.push(Event::InfiniteShrink);
            }
            return GlueSpec {
                shrink_order: Order::Normal,
                ..g.copy()
            };
        }
        g
    }

    fn trace(&mut self, e: Event) {
        if self.p.tracing {
            self.events.push(e);
        }
    }

    /// §863: find optimal breakpoints.
    fn find_optimal_breakpoints(&mut self) -> Result<(), Confusion> {
        self.threshold = self.p.pretolerance;
        if self.threshold >= 0 {
            self.trace(Event::Begin { first: true });
            self.second_pass = false;
            self.final_pass = false;
        } else {
            self.threshold = self.p.tolerance;
            self.second_pass = true;
            self.final_pass = self.p.emergency_stretch <= 0;
            self.trace(Event::Begin { first: false });
        }
        loop {
            if self.threshold > INF_BAD {
                self.threshold = INF_BAD;
            }
            if self.second_pass {
                // §891: initialize for hyphenating a paragraph.
                self.cur_lang = self.p.language;
                self.l_hyf = self.p.left_hyphen_min;
                self.r_hyf = self.p.right_hyphen_min;
            }
            // §864: create an active breakpoint representing the beginning
            // of the paragraph.
            self.active = ActiveList::new();
            self.active.insert_after(
                ActiveList::HEAD,
                Entry::Active {
                    hyphenated: false,
                    fitness: DECENT_FIT,
                    line_number: self.p.prev_graf + 1,
                    total_demerits: 0,
                    break_node: None,
                    short: 0,
                    glue: 0,
                },
            );
            self.active_width = self.background;
            self.passive.clear();
            self.printed = 0;
            let mut auto_breaking = true;
            let mut i = 0;
            let mut prev_p = 0; // glue at beginning is not a legal breakpoint
            while i < self.list.len() && self.active.next(ActiveList::HEAD) != ActiveList::HEAD {
                // §866: call `try_break` if `cur_p` is a legal breakpoint.
                if let Node::Glyphs(_) = self.list[i] {
                    // §867
                    prev_p = i;
                    while let Some(Node::Glyphs(g)) = self.list.get(i) {
                        self.active_width[1] += self.env.font(g.font).run_width(g.chars());
                        if self.exp.spacing {
                            let (st, sh) = self.exp.chars(&*self.env, g.font, g.chars())?;
                            self.active_width[7] += st;
                            self.active_width[8] += sh;
                        }
                        i += 1;
                    }
                    if i == self.list.len() {
                        break;
                    }
                }
                self.cur_p = Some(i);
                match &self.list[i] {
                    Node::Box(b) => self.active_width[1] += b.width,
                    Node::Rule { width, .. } => self.active_width[1] += width,
                    Node::Whatsit(w) => {
                        if let Whatsit::Language { .. } = **w {
                            let w = (**w).clone();
                            self.adv_past(&w);
                        } else if let Some(d) = w.ref_dims() {
                            // pdfTeX's `act_width` of a form or image
                            self.active_width[1] += d.width;
                        }
                    }
                    Node::Glue { .. } | Node::Leaders(_) => {
                        // §868
                        if auto_breaking && precedes_break(&self.list[prev_p]) {
                            self.try_break(0, false)?;
                        }
                        let spec = match &self.list[i] {
                            Node::Glue { spec, .. } => *spec,
                            Node::Leaders(l) => l.spec,
                            _ => unreachable!(),
                        };
                        let q = self.check_shrinkage(spec);
                        match &mut self.list[i] {
                            Node::Glue { spec, .. } => *spec = q,
                            Node::Leaders(l) => l.spec = q,
                            _ => unreachable!(),
                        }
                        self.active_width[1] += q.width;
                        self.active_width[order_index(q.stretch_order)] += q.stretch;
                        self.active_width[6] += q.shrink;
                        if self.second_pass && auto_breaking {
                            self.try_to_hyphenate(i); // §894
                        }
                    }
                    Node::Kern { width, subtype } => {
                        let w = *width;
                        if *subtype == KERN_EXPLICIT {
                            self.kern_break(i, w, auto_breaking)?;
                        } else {
                            self.active_width[1] += w;
                            if *subtype == KERN_NORMAL && self.exp.spacing {
                                let list = &self.list;
                                let l = margin::prev(list, At::node(i))
                                    .and_then(|a| margin::char_at(margin::get(list, a), false));
                                let r = list.get(i + 1).and_then(|n| margin::char_at(n, true));
                                let (st, sh) = self.exp.kern(&*self.env, l, w, r);
                                self.active_width[7] += st;
                                self.active_width[8] += sh;
                            }
                        }
                    }
                    Node::Ligature(l) => {
                        self.active_width[1] += self.env.font(l.font).width(l.ch);
                        let (st, sh) = self.exp.chars(&*self.env, l.font, &[l.ch])?;
                        self.active_width[7] += st;
                        self.active_width[8] += sh;
                    }
                    Node::Disc(d) => {
                        // §869: try to break after a discretionary fragment.
                        let explicit = d.pre.is_empty();
                        let pre = self.exp.part(&*self.env, &d.pre, None, "disc3")?;
                        self.disc_width = pre;
                        if explicit {
                            self.try_break(self.p.ex_hyphen_penalty, true)?;
                        } else {
                            add_widths(&mut self.active_width, &pre);
                            self.try_break(self.p.hyphen_penalty, true)?;
                            sub_widths(&mut self.active_width, &pre);
                        }
                        let Node::Disc(d) = &self.list[i] else {
                            unreachable!()
                        };
                        let after = self.list.get(i + 1).and_then(|n| margin::char_at(n, true));
                        let replace = self.exp.part(&*self.env, &d.replace, after, "disc4")?;
                        add_widths(&mut self.active_width, &replace);
                    }
                    Node::Math { width, subtype } => {
                        let w = *width;
                        // (text-direction nodes leave it as it is)
                        if *subtype < lr::L_CODE {
                            auto_breaking = lr::is_end(*subtype);
                        }
                        self.kern_break(i, w, auto_breaking)?;
                    }
                    Node::Penalty(pi) => self.try_break(*pi, false)?,
                    Node::Mark(_) | Node::Ins(_) | Node::Adjust(_) => {}
                    Node::Glyphs(_) | Node::Unset(_) | Node::MarginKern { .. } => {
                        return Err(Confusion("paragraph"));
                    }
                }
                prev_p = i;
                i += 1;
            }
            if i >= self.list.len() {
                // §873: try the final line break at the end of the
                // paragraph, and `goto done` if the desired breakpoints
                // have been found.
                self.cur_p = None;
                self.try_break(EJECT_PENALTY, true)?;
                if self.active.next(ActiveList::HEAD) != ActiveList::HEAD {
                    // §874: find an active node with fewest demerits.
                    self.fewest_demerits = AWFUL_BAD;
                    let mut r = self.active.next(ActiveList::HEAD);
                    while r != ActiveList::HEAD {
                        if let Entry::Active { total_demerits, .. } = self.active.entry(r)
                            && total_demerits < self.fewest_demerits
                        {
                            self.fewest_demerits = total_demerits;
                            self.best_bet = r;
                        }
                        r = self.active.next(r);
                    }
                    self.best_line = self.line_number(self.best_bet);
                    if self.p.looseness == 0 {
                        break;
                    }
                    // §875: find the best active node for the desired
                    // looseness.
                    let mut r = self.active.next(ActiveList::HEAD);
                    self.actual_looseness = 0;
                    while r != ActiveList::HEAD {
                        if let Entry::Active {
                            total_demerits,
                            line_number,
                            ..
                        } = self.active.entry(r)
                        {
                            let line_diff = line_number - self.best_line;
                            let al = self.actual_looseness;
                            let looseness = self.p.looseness;
                            if (line_diff < al && looseness <= line_diff)
                                || (line_diff > al && looseness >= line_diff)
                            {
                                self.best_bet = r;
                                self.actual_looseness = line_diff;
                                self.fewest_demerits = total_demerits;
                            } else if line_diff == al && total_demerits < self.fewest_demerits {
                                self.best_bet = r;
                                self.fewest_demerits = total_demerits;
                            }
                        }
                        r = self.active.next(r);
                    }
                    self.best_line = self.line_number(self.best_bet);
                    if self.actual_looseness == self.p.looseness || self.final_pass {
                        break;
                    }
                }
            }
            if self.second_pass {
                self.trace(Event::EmergencyPass);
                self.background[2] += self.p.emergency_stretch;
                self.final_pass = true;
            } else {
                self.trace(Event::SecondPass);
                self.threshold = self.p.tolerance;
                self.second_pass = true;
                self.final_pass = self.p.emergency_stretch <= 0;
            } // if at first you don't succeed, ...
        }
        self.trace(Event::End);
        Ok(())
    }

    fn line_number(&self, r: usize) -> i32 {
        match self.active.entry(r) {
            Entry::Active { line_number, .. } => line_number,
            Entry::Delta(_) => 0,
        }
    }

    /// §1362: `adv_past`.
    fn adv_past(&mut self, w: &Whatsit) {
        if let Whatsit::Language {
            language,
            left_hyphen_min,
            right_hyphen_min,
        } = *w
        {
            self.cur_lang = language;
            self.l_hyf = i32::from(left_hyphen_min);
            self.r_hyf = i32::from(right_hyphen_min);
        }
    }

    /// §866: `kern_break` for the kern or math node at `i` of width `w`.
    fn kern_break(&mut self, i: usize, w: Scaled, auto_breaking: bool) -> Result<(), Confusion> {
        if auto_breaking
            && matches!(
                self.list.get(i + 1),
                Some(Node::Glue { .. } | Node::Leaders(_))
            )
        {
            self.try_break(0, false)?;
        }
        self.active_width[1] += w;
        Ok(())
    }

    /// §837: compute the values of `break_width`.
    fn compute_break_width(&mut self, hyphenated: bool) -> Result<(), Confusion> {
        self.break_width = self.background;
        let Some(cur_p) = self.cur_p else {
            return Ok(());
        };
        let mut s = cur_p;
        if hyphenated {
            // §840: compute the discretionary `break_width` values.
            let Node::Disc(d) = &self.list[cur_p] else {
                return Err(Confusion("disc"));
            };
            let after = self
                .list
                .get(cur_p + 1)
                .and_then(|n| margin::char_at(n, true));
            let replace = self.exp.part(&*self.env, &d.replace, after, "disc1")?;
            let post = self.exp.part(&*self.env, &d.post, None, "disc2")?;
            let has_post = !d.post.is_empty();
            for k in [1, 7, 8] {
                self.break_width[k] += post[k] - replace[k] + self.disc_width[k];
            }
            if has_post {
                return Ok(());
            }
            s = cur_p + 1; // nodes may be discardable after the break
        }
        while let Some(n) = self.list.get(s) {
            match n {
                Node::Glue { spec, .. } => self.subtract_glue(*spec),
                Node::Leaders(l) => self.subtract_glue(l.spec),
                Node::Penalty(_) => {}
                Node::Math { width, .. } => self.break_width[1] -= width,
                Node::Kern { width, subtype } => {
                    if *subtype != KERN_EXPLICIT {
                        return Ok(());
                    }
                    self.break_width[1] -= width;
                }
                _ => return Ok(()),
            }
            s += 1;
        }
        Ok(())
    }

    /// §838: subtract glue from `break_width`.
    fn subtract_glue(&mut self, v: GlueSpec) {
        self.break_width[1] -= v.width;
        self.break_width[order_index(v.stretch_order)] -= v.stretch;
        self.break_width[6] -= v.shrink;
    }

    /// pdfTeX's `char_pw`: how far a character protrudes into the left
    /// or right margin.
    fn char_pw(&self, c: Option<(FontId, u8)>, left: bool) -> Scaled {
        c.map_or(0, |(f, ch)| {
            margin::char_pw(self.env.margin_code(f, ch, left), self.env.font(f).param(6))
        })
    }

    /// pdfTeX's `new_margin_kern` for character `c`, if it protrudes.
    fn margin_kern(&self, c: Option<(FontId, u8)>, left: bool) -> Option<Node> {
        let w = self.char_pw(c, left);
        let (font, ch) = c?;
        (w != 0).then_some(Node::MarginKern {
            width: -w,
            left,
            font,
            ch,
        })
    }

    /// pdfTeX: what is left of `shortfall` for the glue once the fonts'
    /// stretch or shrink (`caw` 7 and 8, and the margin characters') has
    /// taken what it can.
    fn expanded_shortfall(&mut self, shortfall: Scaled, caw: &Widths, margins: Margins) -> Scaled {
        let (mut mk_stretch, mut mk_shrink) = (0, 0);
        if self.p.protrude_chars > 1 {
            // calculate variations of marginal kerns (pdfTeX uses the left
            // protrusion of both)
            //
            // PDFTEX_BUG(lig_margin_kern_var): for a ligature, pdfTeX's
            // `last_leftmost_char`/`last_rightmost_char` is its `lig_char`
            // (`char_pw` stores that), which is no char node (`is_char_node`
            // tests the address, and `type` is the font), so `left_pw` of it
            // is 0, while the expanded copy `cp`, a real char node, gets the
            // character's: the variation is off by the whole protrusion.
            let lig_bug = self.p.pdftex_bugs.lig_margin_kern_var;
            for (f, c, lig) in margins.into_iter().flatten() {
                let pw = if lig && lig_bug {
                    0
                } else {
                    expand::char_pw(&*self.env, f, c, true)
                };
                let k = expand::subst_font(self.env, f, c, 1000);
                if k != f {
                    mk_stretch += pw - expand::char_pw(&*self.env, k, c, true);
                }
                let k = expand::subst_font(self.env, f, c, -1000);
                if k != f {
                    mk_shrink += expand::char_pw(&*self.env, k, c, true) - pw;
                }
            }
        }
        let (st, sh) = (caw[7] + mk_stretch, caw[8] + mk_shrink);
        let e = &self.exp;
        // (pdfTeX divides by zero if a limit is below the step)
        let steps = |max: i32| {
            max.checked_div(e.cur_font_step)
                .filter(|&n| n != 0)
                .unwrap_or(1)
        };
        if shortfall > 0 && st > 0 {
            if st > shortfall {
                (st / steps(e.max_stretch_ratio)) / 2
            } else {
                shortfall - st
            }
        } else if shortfall < 0 && sh > 0 {
            if sh > -shortfall {
                -((sh / steps(e.max_shrink_ratio)) / 2)
            } else {
                shortfall + sh
            }
        } else {
            shortfall
        }
    }

    /// pdfTeX's `total_pw`: how far the characters at both ends of the
    /// line from the break of passive node `from` to `cur_p` protrude;
    /// also those characters (`last_leftmost_char`,
    /// `last_rightmost_char`).
    fn total_pw(&self, from: Option<usize>, cur_p: Option<usize>) -> (Scaled, Margins) {
        let list = &self.list;
        let l = from.and_then(|pb| self.passive[pb].cur_break).unwrap_or(0);
        // the right margin first
        let right = match cur_p.map(|p| &list[p]) {
            // a discretionary's pre-break text: its last character
            Some(Node::Disc(d)) if !d.pre.is_empty() => margin_char(d.pre.last(), false),
            _ => {
                let r = match cur_p {
                    Some(p) => margin::prev(list, At::node(p)),
                    None => margin::last(list),
                };
                margin_char(margin::protchar_right(list, At::node(l), r), false)
            }
        };
        // then the left one
        let left = if let Node::Disc(d) = &list[l] {
            if let Some(n) = d.post.first() {
                // the post-break text: its first character
                margin_char(Some(n), true)
            } else {
                // (past the nodes it replaced)
                let start = At::node((l + 1).min(list.len() - 1));
                margin_char(Some(margin::protchar_left(list, start, true)), true)
            }
        } else {
            margin_char(Some(margin::protchar_left(list, At::node(l), true)), true)
        };
        let pw =
            |m: Option<(FontId, u8, bool)>, left| self.char_pw(m.map(|(f, c, _)| (f, c)), left);
        (pw(left, true) + pw(right, false), [left, right])
    }

    /// §829: consider a break at `cur_p` with penalty `pi`.
    fn try_break(&mut self, mut pi: i32, hyphenated: bool) -> Result<(), Confusion> {
        let cur_p = self.cur_p;
        // §831: make sure that `pi` is in the proper range.
        if pi.abs() >= INF_PENALTY {
            if pi > 0 {
                return Ok(()); // this breakpoint is inhibited by infinite penalty
            }
            pi = EJECT_PENALTY; // this breakpoint will be forced
        }
        let head = ActiveList::HEAD;
        let mut no_break_yet = true;
        let mut prev_r = head;
        let mut prev_prev_r = head;
        let mut old_l = 0;
        let mut line_width = 0;
        self.cur_active_width = self.active_width;
        loop {
            // continue:
            let mut r = self.active.next(prev_r);
            // §832: if node `r` is a delta, update `cur_active_width`.
            if let Entry::Delta(d) = self.active.entry(r) {
                add_widths(&mut self.cur_active_width, &d);
                prev_prev_r = prev_r;
                prev_r = r;
                continue;
            }
            let Entry::Active {
                hyphenated: r_hyphenated,
                fitness: r_fitness,
                line_number: l,
                total_demerits: r_total,
                break_node: r_break,
                short: r_short,
                glue: r_glue,
            } = self.active.entry(r)
            else {
                unreachable!()
            };
            // §835: if a line number class has ended, create new active
            // nodes for the best feasible breaks in that class; then
            // `return` if `r=last_active`, otherwise compute the new
            // `line_width`.
            if l > old_l {
                // now we are no longer in the inner loop
                if self.minimum_demerits < AWFUL_BAD && (old_l != self.easy_line || r == head) {
                    // §836: create new active nodes for the best feasible
                    // breaks just found.
                    if no_break_yet {
                        no_break_yet = false;
                        self.compute_break_width(hyphenated)?;
                    }
                    // §843: insert a delta node to prepare for breaks at
                    // `cur_p`.
                    if self.active.is_delta(prev_r) {
                        let (caw, bw) = (self.cur_active_width, self.break_width);
                        let d = self.active.delta_mut(prev_r);
                        sub_widths(d, &caw);
                        add_widths(d, &bw);
                    } else if prev_r == head {
                        self.active_width = self.break_width;
                    } else {
                        let mut d = self.break_width;
                        sub_widths(&mut d, &self.cur_active_width);
                        let q = self.active.insert_after(prev_r, Entry::Delta(d));
                        prev_prev_r = prev_r;
                        prev_r = q;
                    }
                    let adj = self.p.adj_demerits;
                    if adj.abs() >= AWFUL_BAD - self.minimum_demerits {
                        self.minimum_demerits = AWFUL_BAD - 1;
                    } else {
                        self.minimum_demerits += adj.abs();
                    }
                    for fit_class in VERY_LOOSE_FIT..=TIGHT_FIT {
                        if self.minimal_demerits[fit_class] <= self.minimum_demerits {
                            // §845: insert a new active node from
                            // `best_place[fit_class]` to `cur_p`.
                            let serial = i32::try_from(self.passive.len() + 1).unwrap_or(0);
                            self.passive.push(Passive {
                                cur_break: cur_p,
                                prev_break: self.best_place[fit_class],
                                serial,
                            });
                            let line_number = self.best_pl_line[fit_class] + 1;
                            let total = self.minimal_demerits[fit_class];
                            let (short, glue) =
                                (self.best_pl_short[fit_class], self.best_pl_glue[fit_class]);
                            let q = self.active.insert_after(
                                prev_r,
                                Entry::Active {
                                    hyphenated,
                                    fitness: fit_class,
                                    line_number,
                                    total_demerits: total,
                                    break_node: Some(self.passive.len() - 1),
                                    short,
                                    glue,
                                },
                            );
                            prev_r = q;
                            if self.p.tracing {
                                // §846
                                let previous = self.best_place[fit_class]
                                    .map_or(0, |pb| self.passive[pb].serial);
                                self.events.push(Event::NewBreak {
                                    serial,
                                    line: line_number - 1,
                                    fitness: u8::try_from(fit_class).unwrap_or(0),
                                    hyphenated,
                                    total,
                                    previous,
                                    last_fit: self
                                        .fill_width
                                        .map(|_| (short, glue, cur_p.is_none())),
                                });
                            }
                        }
                        self.minimal_demerits[fit_class] = AWFUL_BAD;
                    }
                    self.minimum_demerits = AWFUL_BAD;
                    // §844: insert a delta node to prepare for the next
                    // active node.
                    if r != head {
                        let mut d = self.cur_active_width;
                        sub_widths(&mut d, &self.break_width);
                        let q = self.active.insert_after(prev_r, Entry::Delta(d));
                        prev_prev_r = prev_r;
                        prev_r = q;
                    }
                }
                if r == head {
                    return Ok(());
                }
                // §850: compute the new line width.
                if l > self.easy_line {
                    line_width = self.second_width;
                    old_l = MAX_HALFWORD - 1;
                } else {
                    old_l = l;
                    line_width = if l > self.last_special_line {
                        self.second_width
                    } else if self.p.par_shape.is_empty() {
                        self.first_width
                    } else {
                        self.p.par_shape[usize::try_from(l - 1).unwrap_or(0)].1
                    };
                }
            }
            // §851: consider the demerits for a line from `r` to `cur_p`.
            let mut artificial_demerits = false;
            let caw = self.cur_active_width;
            let mut shortfall = line_width - caw[1]; // we're this much too short
            let mut margins = [None, None];
            if self.p.protrude_chars > 1 {
                let (pw, m) = self.total_pw(r_break, cur_p);
                shortfall += pw;
                margins = m;
            }
            if self.exp.spacing && shortfall != 0 {
                shortfall = self.expanded_shortfall(shortfall, &caw, margins);
            }
            let (b, fit_class);
            // e-TeX: the glue stretch or shrink (or adjustment) of the line
            let mut g = 0;
            let mut adjusted = None;
            if shortfall > 0 {
                // §852
                if caw[3] != 0 || caw[4] != 0 || caw[5] != 0 {
                    if let Some(fill) = self.fill_width {
                        if cur_p.is_none() {
                            adjusted = self.last_line_fit(fill, r_short, r_glue, shortfall);
                        }
                        if adjusted.is_none() {
                            shortfall = 0;
                        }
                    }
                    b = 0;
                    fit_class = DECENT_FIT; // infinite stretch
                } else if shortfall > 7_230_584 && caw[2] < 1_663_497 {
                    b = INF_BAD;
                    fit_class = VERY_LOOSE_FIT;
                } else {
                    b = badness(shortfall, caw[2]);
                    fit_class = if b > 12 {
                        if b > 99 { VERY_LOOSE_FIT } else { LOOSE_FIT }
                    } else {
                        DECENT_FIT
                    };
                }
            } else {
                // §853
                b = if -shortfall > caw[6] {
                    INF_BAD + 1
                } else {
                    badness(-shortfall, caw[6])
                };
                fit_class = if b > 12 { TIGHT_FIT } else { DECENT_FIT };
            }
            let (b, fit_class) = if let Some((ab, af, ag)) = adjusted {
                g = ag;
                (ab, af)
            } else {
                if self.fill_width.is_some() {
                    // adjust the additional data for the last line
                    if cur_p.is_none() {
                        shortfall = 0;
                    }
                    g = match shortfall.signum() {
                        1 => caw[2],
                        -1 => caw[6],
                        _ => 0,
                    };
                }
                (b, fit_class)
            };
            let mut deactivate = false;
            let node_r_stays_active = if b > INF_BAD || pi == EJECT_PENALTY {
                // §854: prepare to deactivate node `r`.
                if self.final_pass
                    && self.minimum_demerits == AWFUL_BAD
                    && self.active.next(r) == head
                    && prev_r == head
                {
                    artificial_demerits = true; // set demerits zero, this break is forced
                } else if b > self.threshold {
                    deactivate = true;
                }
                false
            } else {
                prev_r = r;
                if b > self.threshold {
                    continue;
                }
                true
            };
            if !deactivate {
                // §855: record a new feasible break.
                let mut d = if artificial_demerits {
                    0
                } else {
                    // §859: compute the demerits, `d`, from `r` to `cur_p`.
                    let mut d = self.p.line_penalty + b;
                    d = if d.abs() >= 10000 { 100_000_000 } else { d * d };
                    if pi != 0 {
                        if pi > 0 {
                            d += pi * pi;
                        } else if pi > EJECT_PENALTY {
                            d -= pi * pi;
                        }
                    }
                    if hyphenated && r_hyphenated {
                        if cur_p.is_none() {
                            d += self.p.final_hyphen_demerits;
                        } else {
                            d += self.p.double_hyphen_demerits;
                        }
                    }
                    if fit_class.abs_diff(r_fitness) > 1 {
                        d += self.p.adj_demerits;
                    }
                    d
                };
                if self.p.tracing {
                    self.print_feasible_break(r_break, b, pi, d, artificial_demerits);
                }
                d += r_total; // the minimum total demerits from the beginning to `cur_p` via `r`
                if d <= self.minimal_demerits[fit_class] {
                    self.minimal_demerits[fit_class] = d;
                    self.best_place[fit_class] = r_break;
                    self.best_pl_line[fit_class] = l;
                    self.best_pl_short[fit_class] = shortfall;
                    self.best_pl_glue[fit_class] = g;
                    if d < self.minimum_demerits {
                        self.minimum_demerits = d;
                    }
                }
                if node_r_stays_active {
                    continue; // `prev_r` has been set to `r`
                }
            }
            // deactivate: §860: deactivate node `r`.
            self.active.set_next(prev_r, self.active.next(r));
            if prev_r == head {
                // §861: update the active widths, since the first active
                // node has been deleted.
                r = self.active.next(head);
                if let Entry::Delta(d) = self.active.entry(r) {
                    add_widths(&mut self.active_width, &d);
                    self.cur_active_width = self.active_width;
                    self.active.set_next(head, self.active.next(r));
                }
            } else if let Entry::Delta(dp) = self.active.entry(prev_r) {
                r = self.active.next(prev_r);
                if r == head {
                    sub_widths(&mut self.cur_active_width, &dp);
                    self.active.set_next(prev_prev_r, head);
                    prev_r = prev_prev_r;
                } else if let Entry::Delta(dr) = self.active.entry(r) {
                    add_widths(&mut self.cur_active_width, &dr);
                    add_widths(self.active.delta_mut(prev_r), &dr);
                    self.active.set_next(prev_r, self.active.next(r));
                }
            }
        }
    }

    /// e-TeX: the badness, fitness class and adjustment `g` of the last
    /// line from active node `r` (with `short` and `glue` of the line
    /// before), or `None` if the special treatment does not apply.
    fn last_line_fit(
        &self,
        fill: [Scaled; 3],
        short: Scaled,
        glue: Scaled,
        shortfall: Scaled,
    ) -> Option<(i32, usize, Scaled)> {
        let caw = self.cur_active_width;
        // the previous line was neither stretched nor shrunk, or was
        // infinitely bad; or this line's infinite stretch is not all
        // \parfillskip's
        if short == 0 || glue <= 0 || caw[3..6] != fill {
            return None;
        }
        let mut g = if short > 0 { caw[2] } else { caw[6] };
        if g <= 0 {
            return None; // no finite stretch resp. no shrink
        }
        let mut err = false;
        g = fract(g, short, glue, MAX_DIMEN, &mut err);
        if self.p.last_line_fit < 1000 {
            g = fract(g, self.p.last_line_fit, 1000, MAX_DIMEN, &mut err);
        }
        if err {
            g = if short > 0 { MAX_DIMEN } else { -MAX_DIMEN };
        }
        match g.cmp(&0) {
            core::cmp::Ordering::Greater => {
                // the badness of the last line for stretching
                g = g.min(shortfall);
                if g > 7_230_584 && caw[2] < 1_663_497 {
                    return Some((INF_BAD, VERY_LOOSE_FIT, g));
                }
                let b = badness(g, caw[2]);
                let fc = if b > 12 {
                    if b > 99 { VERY_LOOSE_FIT } else { LOOSE_FIT }
                } else {
                    DECENT_FIT
                };
                Some((b, fc, g))
            }
            core::cmp::Ordering::Less => {
                // the badness of the last line for shrinking
                g = g.max(-caw[6]);
                let b = badness(-g, caw[6]);
                Some((b, if b > 12 { TIGHT_FIT } else { DECENT_FIT }, g))
            }
            core::cmp::Ordering::Equal => None,
        }
    }

    /// §856: record a symbolic description of this feasible break.
    fn print_feasible_break(
        &mut self,
        r_break: Option<usize>,
        b: i32,
        pi: i32,
        d: i32,
        artificial: bool,
    ) {
        let cur_p = self.cur_p;
        let upto = cur_p.map_or(self.list.len(), |c| c + 1);
        if self.printed < upto {
            // §857: the list between `printed_node` and `cur_p`; the nodes a
            // discretionary at `cur_p` replaces are skipped (§858).
            let mut text = self.list[self.printed..upto].to_vec();
            if cur_p.is_some()
                && let Some(Node::Disc(d)) = text.last_mut()
            {
                d.replace.clear();
            }
            self.events.push(Event::Text(text));
            self.printed = upto;
        }
        let at = match cur_p.map(|c| &self.list[c]) {
            None => BreakKind::Par,
            Some(Node::Penalty(_)) => BreakKind::Penalty,
            Some(Node::Disc(_)) => BreakKind::Discretionary,
            Some(Node::Kern { .. }) => BreakKind::Kern,
            Some(Node::Math { .. }) => BreakKind::Math,
            Some(_) => BreakKind::Glue,
        };
        self.events.push(Event::Feasible {
            at,
            via: r_break.map_or(0, |pb| self.passive[pb].serial),
            badness: (b <= INF_BAD).then_some(b),
            penalty: pi,
            demerits: (!artificial).then_some(d),
        });
    }

    /// §877: break the paragraph at the chosen breakpoints; the items for
    /// the vertical list and the new `\prevgraf`.
    fn post_line_break(
        &mut self,
        left_skip: GlueSpec,
        right_skip: GlueSpec,
        lr_open: &mut Vec<u8>,
    ) -> Result<(Vec<Item>, i32), Confusion> {
        let texxet = self.p.pack.texxet;
        // TeXXeT: follow the segments a math node opens or closes.
        let track = |lr: &mut Vec<u8>, n: &Node| {
            if texxet && let Node::Math { subtype, .. } = *n {
                if !lr::is_end(subtype) {
                    lr.push(lr::end_of(subtype));
                } else if lr.last() == Some(&lr::end_of(subtype)) {
                    lr.pop();
                }
            }
        };
        // §878: the breakpoints, first to last.
        let mut breaks = Vec::new();
        let mut q = match self.active.entry(self.best_bet) {
            Entry::Active { break_node, .. } => break_node,
            Entry::Delta(_) => None,
        };
        while let Some(pb) = q {
            breaks.push(self.passive[pb].cur_break);
            q = self.passive[pb].prev_break;
        }
        breaks.reverse();
        let list = core::mem::take(&mut self.list);
        let (len, mut start) = (list.len(), 0);
        let mut nodes = list.into_iter().enumerate().peekable();
        // Material carried to the start of the next line (a post-break).
        let mut carry: Vec<Node> = Vec::new();
        let mut items = Vec::new();
        let mut cur_line = self.p.prev_graf + 1;
        let nbreaks = breaks.len();
        for (bi, brk) in breaks.iter().enumerate() {
            // §880: justify the line ending at breakpoint `brk`.
            let end = brk.map_or(len, |b| b + 1);
            let room = end.saturating_sub(start) + lr_open.len() + carry.len() + 3;
            start = end;
            let mut line: Vec<Node> = Vec::with_capacity(room);
            // TeXXeT: reopen the segments open at the start of the line.
            for &e in lr_open.iter() {
                line.push(Node::Math {
                    width: 0,
                    subtype: lr::begin_of(e),
                });
            }
            for n in carry.drain(..) {
                append(&mut line, n);
            }
            // §881: modify the end of the line to reflect the nature of the
            // break and to include \rightskip.
            let mut disc_break = false;
            let mut post_disc_break = false;
            let mut add_right_skip = true;
            match *brk {
                None => {
                    for (_, n) in nodes.by_ref() {
                        track(lr_open, &n);
                        append(&mut line, n);
                    }
                }
                Some(at) => {
                    while let Some((i, _)) = nodes.peek() {
                        if *i == at {
                            break;
                        }
                        let (_, n) = nodes.next().expect("peeked");
                        track(lr_open, &n);
                        append(&mut line, n);
                    }
                    let (_, n) = nodes.next().ok_or(Confusion("line breaking"))?;
                    match n {
                        Node::Glue { .. } | Node::Leaders(_) => {
                            line.push(Node::Glue {
                                spec: right_skip,
                                subtype: RIGHT_SKIP + 1,
                            });
                            add_right_skip = false;
                        }
                        Node::Disc(d) => {
                            // §882: change discretionary to compulsory; the
                            // replaced nodes go (§883).
                            let d = *d;
                            if !d.post.is_empty() {
                                // §884: transplant the post-break list.
                                carry = d.post;
                                post_disc_break = true;
                            }
                            line.push(Node::Disc(Box::default()));
                            // §885: transplant the pre-break list.
                            for n in d.pre {
                                append(&mut line, n);
                            }
                            disc_break = true;
                        }
                        Node::Math { subtype, .. } => {
                            let n = Node::Math { width: 0, subtype };
                            track(lr_open, &n);
                            line.push(n);
                        }
                        Node::Kern { subtype, .. } => line.push(Node::Kern { width: 0, subtype }),
                        n => line.push(n),
                    }
                }
            }
            // (a line can be empty: the last one of a paragraph that ends
            // with a forced break, its \parfillskip pruned; pdfTeX's
            // `prev_rightmost` finds nothing there, and no kern goes in)
            if self.p.protrude_chars > 0
                && let Some(q) = line.len().checked_sub(1)
            {
                // pdfTeX: a margin kern for the character that protrudes
                // at the right, before the node at the break (or after a
                // discretionary's pre-break text, which ends the line).
                let (c, at) = if disc_break && !matches!(line[q], Node::Disc(_)) {
                    (margin::char_at(&line[q], false), q + 1)
                } else {
                    let p = margin::prev(&line, At::node(q));
                    (margin::find_protchar_right(&line, At::node(0), p), q)
                };
                if let Some(k) = self.margin_kern(c, false) {
                    line.insert(at, k);
                }
            }
            if add_right_skip {
                // §886: put the \rightskip glue after the break.
                line.push(Node::Glue {
                    spec: right_skip,
                    subtype: RIGHT_SKIP + 1,
                });
            }
            // TeXXeT: close the open segments before the \rightskip.
            let at = line.len() - 1;
            line.splice(
                at..at,
                lr_open
                    .iter()
                    .rev()
                    .map(|&subtype| Node::Math { width: 0, subtype }),
            );
            if self.p.protrude_chars > 0 {
                // pdfTeX: and one for the character at the left.
                let c = margin::find_protchar_left(&line, At::node(0), false);
                if let Some(k) = self.margin_kern(c, true) {
                    line.insert(0, k);
                }
            }
            // §887: put the \leftskip glue at the left.
            if !left_skip.shared_zero {
                line.insert(
                    0,
                    Node::Glue {
                        spec: left_skip,
                        subtype: LEFT_SKIP + 1,
                    },
                );
            }
            // §889: call the packaging subroutine.
            let (cur_width, cur_indent) = if cur_line > self.last_special_line {
                (self.second_width, self.second_indent)
            } else if self.p.par_shape.is_empty() {
                (self.first_width, self.first_indent)
            } else {
                let (indent, width) = self.p.par_shape[usize::try_from(cur_line - 1).unwrap_or(0)];
                (width, indent)
            };
            // (the caller packs the line, and appends the box and what
            // migrated out of it, §888)
            items.push(Item::Line {
                list: line,
                width: cur_width,
                shift: cur_indent,
            });
            // §890: append a penalty node, if a nonzero penalty is
            // appropriate.
            if cur_line + 1 != self.best_line {
                // (e-TeX: the `k`th entry of an array for the `k`th line,
                // the last one for the lines after)
                let entry = |a: &[i32], k: i32| {
                    let k = usize::try_from(k).unwrap_or(0).clamp(1, a.len());
                    a[k - 1]
                };
                let p = self.p;
                let mut pen = if p.inter_line_penalties.is_empty() {
                    p.inter_line_penalty
                } else {
                    entry(&p.inter_line_penalties, cur_line)
                };
                if !p.club_penalties.is_empty() {
                    pen += entry(&p.club_penalties, cur_line - p.prev_graf);
                } else if cur_line == p.prev_graf + 1 {
                    pen += p.club_penalty;
                }
                if !p.widow_penalties.is_empty() {
                    pen += entry(&p.widow_penalties, self.best_line - cur_line - 1);
                } else if cur_line + 2 == self.best_line {
                    pen += p.final_widow_penalty;
                }
                if disc_break {
                    pen += self.p.broken_penalty;
                }
                if pen != 0 {
                    items.push(Item::Penalty(pen));
                }
            }
            cur_line += 1;
            if bi + 1 < nbreaks && !post_disc_break {
                // §879: prune unwanted nodes at the beginning of the next
                // line.
                let next_break = breaks[bi + 1];
                while let Some((i, n)) = nodes.peek() {
                    if Some(*i) == next_break || !discardable(n) {
                        break;
                    }
                    track(lr_open, n);
                    nodes.next();
                }
            }
        }
        if cur_line != self.best_line || nodes.next().is_some() || !carry.is_empty() {
            return Err(Confusion("line breaking"));
        }
        Ok((items, self.best_line - 1))
    }

    // §891–§918: hyphenation.

    /// §894: try to hyphenate the word following the glue at `cur_p`.
    fn try_to_hyphenate(&mut self, cur_p: usize) {
        let env = &*self.env;
        // (e-TeX `set_lc_code`: the language's saved codes, if any)
        let lc = |lang: i32, c: u8| env.patterns().hyph_code(lang, c, |c| env.lc_code(c));
        // §896: skip to node `ha`, or return if no hyphenation should be
        // attempted.
        let mut prev_s = Pos::node(cur_p);
        let mut s = Pos::node(cur_p + 1);
        let hf;
        loop {
            let c;
            match atom(&self.list, s) {
                Some(Atom::Char(f, ch)) => {
                    c = ch;
                    if lc(self.cur_lang, c) != 0 {
                        if lc(self.cur_lang, c) == c || self.p.uc_hyph {
                            hf = f;
                            break;
                        }
                        return;
                    }
                }
                Some(Atom::Node(Node::Ligature(l))) => {
                    if let Some(&first) = l.original.first() {
                        c = first;
                        if lc(self.cur_lang, c) != 0 {
                            if lc(self.cur_lang, c) == c || self.p.uc_hyph {
                                hf = l.font;
                                break;
                            }
                            return;
                        }
                    }
                }
                Some(Atom::Node(Node::Kern { subtype, .. })) if *subtype == KERN_NORMAL => {}
                // (text-direction nodes are skipped)
                Some(Atom::Node(Node::Math { subtype, .. })) if *subtype >= lr::L_CODE => {}
                Some(Atom::Node(Node::Whatsit(w))) => {
                    // §1363: `adv_past`
                    if let Whatsit::Language {
                        language,
                        left_hyphen_min,
                        right_hyphen_min,
                    } = **w
                    {
                        self.cur_lang = language;
                        self.l_hyf = i32::from(left_hyphen_min);
                        self.r_hyf = i32::from(right_hyphen_min);
                    }
                }
                None | Some(Atom::Node(_)) => return,
            }
            prev_s = s;
            s = next_pos(&self.list, s);
        }
        // done2:
        let hyf_char = env.hyphen_char(hf);
        if !(0..=255).contains(&hyf_char) {
            return;
        }
        let ha = prev_s;
        if self.l_hyf + self.r_hyf > 63 {
            return;
        }
        // §897: skip to node `hb`, putting letters into `hu` and `hc`.
        let font = env.font(hf);
        let mut hu = [0i32; MAX_WORD + 2];
        let mut hc = [0u8; MAX_WORD + 2];
        let mut hn = 0usize;
        let mut hb = s;
        let mut hyf_bchar = NON_CHAR;
        loop {
            match atom(&self.list, s) {
                Some(Atom::Char(f, c)) => {
                    if f != hf {
                        break;
                    }
                    hyf_bchar = i32::from(c);
                    if lc(self.cur_lang, c) == 0 || hn == MAX_WORD {
                        break;
                    }
                    hb = s;
                    hn += 1;
                    hu[hn] = i32::from(c);
                    hc[hn] = lc(self.cur_lang, c);
                    hyf_bchar = NON_CHAR;
                }
                Some(Atom::Node(Node::Ligature(l))) => {
                    // §898: move the characters of a ligature node to `hu`
                    // and `hc`; but stop if they are not all letters.
                    if l.font != hf {
                        break;
                    }
                    let mut j = hn;
                    if let Some(&first) = l.original.first() {
                        hyf_bchar = i32::from(first);
                    }
                    let mut letters = true;
                    for &c in &l.original {
                        if lc(self.cur_lang, c) == 0 || j == MAX_WORD {
                            letters = false;
                            break;
                        }
                        j += 1;
                        hu[j] = i32::from(c);
                        hc[j] = lc(self.cur_lang, c);
                    }
                    if !letters {
                        break;
                    }
                    hb = s;
                    hn = j;
                    hyf_bchar = if l.subtype % 2 == 1 {
                        font.bchar
                    } else {
                        NON_CHAR
                    };
                }
                Some(Atom::Node(Node::Kern { subtype, .. })) if *subtype == KERN_NORMAL => {
                    hb = s;
                    hyf_bchar = font.bchar;
                }
                _ => break,
            }
            s = next_pos(&self.list, s);
        }
        // §899: check that the nodes following `hb` permit hyphenation and
        // that at least `l_hyf+r_hyf` letters have been found.
        let (l_hyf, r_hyf) = (
            usize::try_from(self.l_hyf).unwrap_or(0),
            usize::try_from(self.r_hyf).unwrap_or(0),
        );
        if hn < l_hyf + r_hyf {
            return;
        }
        loop {
            match atom(&self.list, s) {
                Some(Atom::Char(..) | Atom::Node(Node::Ligature(_))) => {}
                Some(Atom::Node(Node::Kern { subtype, .. })) => {
                    if *subtype != KERN_NORMAL {
                        break;
                    }
                }
                Some(Atom::Node(
                    Node::Whatsit(_)
                    | Node::Glue { .. }
                    | Node::Leaders(_)
                    | Node::Penalty(_)
                    | Node::Ins(_)
                    | Node::Adjust(_)
                    | Node::Mark(_),
                )) => break,
                Some(Atom::Node(Node::Math { subtype, .. })) if *subtype >= lr::L_CODE => break,
                _ => return,
            }
            s = next_pos(&self.list, s);
        }
        // done4: §895, §923: find hyphen locations for the word.
        let mut hyf = [0u8; MAX_WORD + 2];
        let key = exception_key(&hc[1..=hn], self.cur_lang);
        if !hyphen_values(
            env.patterns(),
            env.exception(&key),
            self.cur_lang,
            &hc[1..=hn],
            r_hyf,
            &mut hyf,
        ) {
            return;
        }
        // found:
        for h in &mut hyf[..l_hyf] {
            *h = 0;
        }
        for j in 0..r_hyf {
            hyf[hn - j] = 0;
        }
        // §902: if no hyphens were found, return.
        if !(l_hyf..=hn - r_hyf).any(|j| hyf[j] % 2 == 1) {
            return;
        }
        // §903: replace nodes `ha..hb` by a sequence of nodes that includes
        // the discretionary hyphens.
        let mut w = Word {
            font: hf,
            hu,
            hyf,
            hn,
            hyf_char,
            init_list: Vec::new(),
            init_lig: false,
            init_lft: false,
            hyphen_passed: 0,
            events: Vec::new(),
        };
        let r = next_pos(&self.list, ha);
        let (start, j) = match atom(&self.list, ha) {
            Some(Atom::Char(f, c)) if f == hf => {
                w.init_list.push(c);
                w.hu[0] = i32::from(c);
                (ha, 0)
            }
            Some(Atom::Node(Node::Ligature(l))) if l.font == hf => {
                w.init_list.clone_from(&l.original);
                w.init_lig = true;
                w.init_lft = l.subtype > 1;
                w.hu[0] = i32::from(l.ch);
                if w.init_list.is_empty() && w.init_lft {
                    w.hu[0] = 256;
                    w.init_lig = false;
                } // in this case a ligature will be reconstructed from scratch
                (ha, 0)
            }
            Some(Atom::Char(..) | Atom::Node(Node::Ligature(_))) => {
                w.hu[0] = 256; // found2
                (r, 0)
            }
            _ => {
                // no punctuation found; look for left boundary
                if let Some(Atom::Node(Node::Ligature(l))) = atom(&self.list, r)
                    && l.subtype > 1
                {
                    w.hu[0] = 256; // found2
                    (r, 0)
                } else {
                    (r, 1)
                }
            }
        };
        let bchar = hyf_bchar;
        let major = w.reconstitute_word(font, j, bchar);
        self.events.append(&mut w.events);
        self.splice(start, hb, &major, hf);
    }

    /// Replace the atoms `from..=to` of the list by `major` (in font
    /// `hf`), keeping glyph runs canonical.
    fn splice(&mut self, from: Pos, to: Pos, major: &[Tmp], hf: FontId) {
        // The affected node span, widened to whole glyph sequences.
        let mut first = from.node;
        while first > 0 && matches!(self.list[first - 1], Node::Glyphs(_)) {
            first -= 1;
        }
        let mut last = to.node;
        while matches!(self.list.get(last + 1), Some(Node::Glyphs(_))) {
            last += 1;
        }
        let old: Vec<Node> = self.list.drain(first..=last).collect();
        let mut new = Vec::with_capacity(old.len() + major.len());
        let mut placed = false;
        for (k, n) in old.into_iter().enumerate() {
            let idx = first + k;
            match n {
                Node::Glyphs(g) => {
                    for (ci, &c) in g.chars().iter().enumerate() {
                        let pos = Pos { node: idx, ch: ci };
                        if pos < from || pos > to {
                            push_char(&mut new, g.font, c);
                        } else if !placed {
                            placed = true;
                            tmp_into(&mut new, major, hf);
                        }
                    }
                }
                n => {
                    let pos = Pos::node(idx);
                    if pos < from || pos > to {
                        append(&mut new, n);
                    } else if !placed {
                        placed = true;
                        tmp_into(&mut new, major, hf);
                    }
                }
            }
        }
        if !placed {
            tmp_into(&mut new, major, hf);
        }
        self.list.splice(first..first, new);
    }
}

/// `precedes_break(p)` (§148), or a non-explicit kern (§868).
fn precedes_break(n: &Node) -> bool {
    match n {
        Node::Glyphs(_)
        | Node::Box(_)
        | Node::Rule { .. }
        | Node::Ins(_)
        | Node::Mark(_)
        | Node::Adjust(_)
        | Node::Ligature(_)
        | Node::Disc(_)
        | Node::Whatsit(_) => true,
        Node::Kern { subtype, .. } => *subtype != KERN_EXPLICIT,
        _ => false,
    }
}

/// §879: glue, penalties, math and explicit kerns vanish at a line start.
fn discardable(n: &Node) -> bool {
    match n {
        Node::Glue { .. } | Node::Leaders(_) | Node::Penalty(_) | Node::Math { .. } => true,
        Node::Kern { subtype, .. } => *subtype == KERN_EXPLICIT,
        _ => false,
    }
}

/// Append `n` to `list`, merging glyph runs to keep them canonical.
fn append(list: &mut Vec<Node>, n: Node) {
    match n {
        // a run that cannot merge with the last one stays canonical
        Node::Glyphs(g) if !matches!(list.last(), Some(Node::Glyphs(h)) if h.font == g.font && !h.is_full()) =>
        {
            list.push(Node::Glyphs(g));
        }
        Node::Glyphs(g) => {
            for &c in g.chars() {
                push_char(list, g.font, c);
            }
        }
        n => list.push(n),
    }
}

/// A position in a list: a node, and a character within a glyph run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pos {
    node: usize,
    ch: usize,
}

impl Pos {
    fn node(node: usize) -> Pos {
        Pos { node, ch: 0 }
    }
}

enum Atom<'a> {
    Char(FontId, u8),
    Node(&'a Node),
}

fn atom(list: &[Node], p: Pos) -> Option<Atom<'_>> {
    match list.get(p.node)? {
        Node::Glyphs(g) => Some(Atom::Char(g.font, g.chars()[p.ch])),
        n => Some(Atom::Node(n)),
    }
}

fn next_pos(list: &[Node], p: Pos) -> Pos {
    if let Some(Node::Glyphs(g)) = list.get(p.node)
        && p.ch + 1 < g.chars().len()
    {
        return Pos {
            node: p.node,
            ch: p.ch + 1,
        };
    }
    Pos::node(p.node + 1)
}

/// Nodes built while reconstituting a hyphenated word (all in its font).
#[derive(Clone, Debug)]
enum Tmp {
    Char(u8),
    Lig {
        ch: u8,
        subtype: u8,
        original: Vec<u8>,
    },
    Kern(Scaled),
    Disc {
        pre: Vec<Tmp>,
        post: Vec<Tmp>,
        replace: Vec<Tmp>,
    },
}

fn tmp_into(list: &mut Vec<Node>, items: &[Tmp], font: FontId) {
    for t in items {
        match t {
            Tmp::Char(c) => push_char(list, font, *c),
            Tmp::Lig {
                ch,
                subtype,
                original,
            } => list.push(Node::Ligature(Box::new(Ligature {
                font,
                ch: *ch,
                subtype: *subtype,
                original: original.clone(),
            }))),
            Tmp::Kern(w) => list.push(Node::Kern {
                width: *w,
                subtype: KERN_NORMAL,
            }),
            Tmp::Disc { pre, post, replace } => {
                let mut d = Disc::default();
                tmp_into(&mut d.pre, pre, font);
                tmp_into(&mut d.post, post, font);
                tmp_into(&mut d.replace, replace, font);
                list.push(Node::Disc(Box::new(d)));
            }
        }
    }
}

/// §892, §900: a word being hyphenated.
struct Word {
    font: FontId,
    /// The characters, `hu[1..=hn]` (`hu[0]` the one before, or 256).
    hu: [i32; MAX_WORD + 2],
    hyf: [u8; MAX_WORD + 2],
    hn: usize,
    hyf_char: i32,
    init_list: Vec<u8>,
    init_lig: bool,
    init_lft: bool,
    hyphen_passed: usize,
    events: Vec<Event>,
}

/// §907: the ligature cursor of `reconstitute`.
struct Cursor {
    cur_l: i32,
    cur_r: i32,
    /// `hold.len()` at `cur_q`: characters after it form the next
    /// ligature.
    cur_q: usize,
    /// The stack of inserted ligature characters (top last), each with
    /// the character it consumed, if any.
    lig_stack: Vec<(i32, Option<u8>)>,
    ligature_present: bool,
    lft_hit: bool,
    rt_hit: bool,
}

fn byte(c: i32) -> u8 {
    u8::try_from(c).unwrap_or(0)
}

impl Word {
    fn u(&self, j: usize) -> i32 {
        self.hu[j]
    }

    /// §913: reconstitute nodes for the hyphenated word, inserting
    /// discretionary hyphens; starts at `hu[j]`.
    fn reconstitute_word(
        &mut self,
        font: &crate::font::Font,
        mut j: usize,
        bchar: i32,
    ) -> Vec<Tmp> {
        let hn = self.hn;
        let mut major: Vec<Tmp> = Vec::new();
        loop {
            let mut l = j;
            let (next, mut hold) = self.reconstitute(font, j, hn, bchar, self.hyf_char);
            j = next + 1;
            if self.hyphen_passed == 0 {
                major.append(&mut hold);
                if self.hyf[j - 1] % 2 == 1 {
                    l = j;
                    self.hyphen_passed = j - 1;
                }
            }
            if self.hyphen_passed > 0 {
                // §914 (what `hold` has left starts the replaced material)
                self.discretionaries(font, &mut major, hold, &mut j, &mut l, bchar);
            }
            if j > hn {
                break;
            }
        }
        major
    }

    /// §914–§918: create and append a discretionary as an alternative to
    /// the unhyphenated word, and develop both branches until they become
    /// equivalent. `start` is the unhyphenated material built so far.
    fn discretionaries(
        &mut self,
        font: &crate::font::Font,
        major: &mut Vec<Tmp>,
        start: Vec<Tmp>,
        j: &mut usize,
        l: &mut usize,
        bchar: i32,
    ) {
        let hn = self.hn;
        let mut replace = start;
        loop {
            let mut i = self.hyphen_passed;
            self.hyf[i] = 0;
            // §915: put the characters `hu[l..i]` and a hyphen into
            // `pre_break(r)`.
            let mut pre = Vec::new();
            let hyf_exists = font.glyph(self.hyf_char).is_some();
            let mut c = 0;
            if hyf_exists {
                i += 1;
                c = self.hu[i];
                self.hu[i] = self.hyf_char;
            } else {
                self.events.push(Event::MissingChar {
                    font: self.font,
                    ch: self.hyf_char,
                });
            }
            while *l <= i {
                let (next, hold) = self.reconstitute(font, *l, i, font.bchar, NON_CHAR);
                *l = next + 1;
                pre.extend(hold);
            }
            if hyf_exists {
                self.hu[i] = c; // restore the character in the hyphen position
                *l = i;
            }
            // §916: put the characters `hu[i+1..]` into `post_break(r)`,
            // appending to this list and to `major_tail` until
            // synchronization has been achieved.
            let mut post = Vec::new();
            let mut c_loc = 0;
            if font.bchar_label.is_some() {
                // put left boundary at beginning of new line
                *l -= 1;
                c = self.hu[*l];
                c_loc = *l;
                self.hu[*l] = 256;
            }
            while *l < *j {
                loop {
                    let (next, hold) = self.reconstitute(font, *l, hn, bchar, NON_CHAR);
                    *l = next + 1;
                    if c_loc > 0 {
                        self.hu[c_loc] = c;
                        c_loc = 0;
                    }
                    post.extend(hold);
                    if *l >= *j {
                        break;
                    }
                }
                while *l > *j {
                    // §917: append characters of `hu[j..]` to `major_tail`,
                    // advancing `j`.
                    let (next, hold) = self.reconstitute(font, *j, hn, bchar, NON_CHAR);
                    *j = next + 1;
                    replace.extend(hold);
                }
            }
            // §918: set `replace_count(r)` appropriately.
            let taken = core::mem::take(&mut replace);
            if taken.len() > 127 {
                // we have to forget the discretionary hyphen
                major.extend(taken);
            } else {
                major.push(Tmp::Disc {
                    pre,
                    post,
                    replace: taken,
                });
            }
            self.hyphen_passed = *j - 1;
            if self.hyf[*j - 1].is_multiple_of(2) {
                return;
            }
        }
    }

    /// §908: `set_cur_r`; returns `cur_rh`.
    fn set_cur_r(&self, cur: &mut Cursor, j: usize, n: usize, bchar: i32, hchar: i32) -> i32 {
        cur.cur_r = if j < n { self.u(j + 1) } else { bchar };
        if self.hyf[j] % 2 == 1 {
            hchar
        } else {
            NON_CHAR
        }
    }

    /// §910: `wrap_lig(b)`.
    fn wrap_lig(cur: &mut Cursor, hold: &mut Vec<Tmp>, b: bool) {
        if !cur.ligature_present {
            return;
        }
        let original = hold
            .drain(cur.cur_q..)
            .map(|t| match t {
                Tmp::Char(c) => c,
                _ => 0,
            })
            .collect();
        let mut subtype = 0;
        if cur.lft_hit {
            subtype = 2;
            cur.lft_hit = false;
        }
        if b && cur.lig_stack.is_empty() {
            subtype += 1;
            cur.rt_hit = false;
        }
        hold.push(Tmp::Lig {
            ch: byte(cur.cur_l),
            subtype,
            original,
        });
        cur.ligature_present = false;
    }

    /// §910: `pop_lig_stack`; returns the new `cur_rh`.
    #[allow(clippy::too_many_arguments)]
    fn pop_lig_stack(
        &self,
        cur: &mut Cursor,
        hold: &mut Vec<Tmp>,
        j: &mut usize,
        n: usize,
        bchar: i32,
        hchar: i32,
        cur_rh: i32,
    ) -> i32 {
        let (_, consumed) = cur.lig_stack.pop().expect("lig_stack is not empty");
        if let Some(c) = consumed {
            hold.push(Tmp::Char(c)); // this is a charnode for `hu[j+1]`
            *j += 1;
        }
        if let Some(&(top, _)) = cur.lig_stack.last() {
            cur.cur_r = top;
            cur_rh
        } else {
            self.set_cur_r(cur, *j, n, bchar, hchar)
        }
    }

    /// §906: build a list for `hu[j..=n]`; returns the index of the last
    /// character consumed and the list.
    fn reconstitute(
        &mut self,
        font: &crate::font::Font,
        mut j: usize,
        n: usize,
        mut bchar: i32,
        mut hchar: i32,
    ) -> (usize, Vec<Tmp>) {
        self.hyphen_passed = 0;
        let mut hold: Vec<Tmp> = Vec::new();
        let mut w: Scaled = 0;
        // §908: set up data structures with the cursor following position
        // `j`.
        let mut cur = Cursor {
            cur_l: self.u(j),
            cur_r: 0,
            cur_q: 0,
            lig_stack: Vec::new(),
            ligature_present: false,
            lft_hit: false,
            rt_hit: false,
        };
        if j == 0 {
            cur.ligature_present = self.init_lig;
            if cur.ligature_present {
                cur.lft_hit = self.init_lft;
            }
            for &c in &self.init_list {
                hold.push(Tmp::Char(c));
            }
        } else if cur.cur_l < NON_CHAR {
            hold.push(Tmp::Char(byte(cur.cur_l)));
        }
        let mut cur_rh = self.set_cur_r(&mut cur, j, n, bchar, hchar);
        'continue_: loop {
            // §909: if there's a ligature or kern at the cursor position,
            // update the data structures, possibly advancing `j`; continue
            // until the cursor moves.
            'done: {
                let mut k = if cur.cur_l == NON_CHAR {
                    match font.bchar_label {
                        Some(k) => usize::from(k),
                        None => break 'done,
                    }
                } else {
                    let Some(g) = font.glyph(cur.cur_l) else {
                        break 'done;
                    };
                    let Tag::Lig(start) = g.tag else {
                        break 'done;
                    };
                    let mut k = usize::from(start);
                    let q = font.lig_kerns[k];
                    if q.skip > LigKern::STOP {
                        k = 256 * usize::from(q.op) + usize::from(q.remainder);
                    }
                    k
                }; // now `k` is the starting address of the lig/kern program
                let test_char = if cur_rh < NON_CHAR { cur_rh } else { cur.cur_r };
                loop {
                    let q = font.lig_kerns[k];
                    if i32::from(q.next) == test_char && q.skip <= LigKern::STOP {
                        if cur_rh < NON_CHAR {
                            self.hyphen_passed = j;
                            hchar = NON_CHAR;
                            cur_rh = NON_CHAR;
                            continue 'continue_;
                        }
                        if hchar < NON_CHAR && self.hyf[j] % 2 == 1 {
                            self.hyphen_passed = j;
                            hchar = NON_CHAR;
                        }
                        if q.op < LigKern::KERN {
                            // §911: carry out a ligature replacement,
                            // updating the cursor structure and possibly
                            // advancing `j`.
                            if cur.cur_l == NON_CHAR {
                                cur.lft_hit = true;
                            }
                            if j == n && cur.lig_stack.is_empty() {
                                cur.rt_hit = true;
                            }
                            let rem = i32::from(q.remainder);
                            match q.op {
                                1 | 5 => {
                                    // =:|, =:|>
                                    cur.cur_l = rem;
                                    cur.ligature_present = true;
                                }
                                2 | 6 => {
                                    // |=:, |=:>
                                    cur.cur_r = rem;
                                    if let Some(top) = cur.lig_stack.last_mut() {
                                        top.0 = rem;
                                    } else if j == n {
                                        cur.lig_stack.push((rem, None));
                                        bchar = NON_CHAR;
                                    } else {
                                        cur.lig_stack.push((rem, Some(byte(self.u(j + 1)))));
                                    }
                                }
                                3 => {
                                    // |=:|
                                    cur.cur_r = rem;
                                    cur.lig_stack.push((rem, None));
                                }
                                7 | 11 => {
                                    // |=:|>, |=:|>>
                                    Self::wrap_lig(&mut cur, &mut hold, false);
                                    cur.cur_q = hold.len();
                                    cur.cur_l = rem;
                                    cur.ligature_present = true;
                                }
                                _ => {
                                    // =:
                                    cur.cur_l = rem;
                                    cur.ligature_present = true;
                                    if !cur.lig_stack.is_empty() {
                                        cur_rh = self.pop_lig_stack(
                                            &mut cur, &mut hold, &mut j, n, bchar, hchar, cur_rh,
                                        );
                                    } else if j == n {
                                        break 'done;
                                    } else {
                                        hold.push(Tmp::Char(byte(cur.cur_r)));
                                        j += 1;
                                        cur_rh = self.set_cur_r(&mut cur, j, n, bchar, hchar);
                                    }
                                }
                            }
                            if q.op > 4 && q.op != 7 {
                                break 'done;
                            }
                            continue 'continue_;
                        }
                        w = font.kerns
                            [256 * usize::from(q.op - LigKern::KERN) + usize::from(q.remainder)];
                        break 'done; // this kern will be inserted below
                    }
                    if q.skip >= LigKern::STOP {
                        if cur_rh == NON_CHAR {
                            break 'done;
                        }
                        cur_rh = NON_CHAR;
                        continue 'continue_;
                    }
                    k += usize::from(q.skip) + 1;
                }
            }
            // done: §910: append a ligature and/or kern to the translation;
            // continue if the stack of inserted ligatures is nonempty.
            let rt_hit = cur.rt_hit;
            Self::wrap_lig(&mut cur, &mut hold, rt_hit);
            if w != 0 {
                hold.push(Tmp::Kern(w));
                w = 0;
            }
            if let Some(&(top, _)) = cur.lig_stack.last() {
                cur.cur_q = hold.len();
                cur.cur_l = top;
                cur.ligature_present = true;
                cur_rh = self.pop_lig_stack(&mut cur, &mut hold, &mut j, n, bchar, hchar, cur_rh);
                continue 'continue_;
            }
            break;
        }
        (j, hold)
    }
}
